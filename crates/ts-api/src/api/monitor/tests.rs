//! The monitor operations over a fake laptop in a temporary tree, a scripted program runner and
//! the testing services: no real file, process or GPU is touched.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};
use ts_platform::hw::CommandRunner;

use super::*;
use crate::api::testing::{Rig, daemon_state};
use crate::api::{Api, Services};

/// A temporary directory removed when dropped.
struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ts-api-monitor-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn put(&self, rel: &str, text: &str) {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Scripted programs: the answer for each program name and a record of every start.
#[derive(Default)]
struct Programs {
    answers: Mutex<BTreeMap<String, Result<String, io::ErrorKind>>>,
    calls: Mutex<Vec<String>>,
}

impl Programs {
    fn answer(&self, program: &str, output: Result<&str, io::ErrorKind>) {
        let answer = output.map(str::to_owned);
        self.answers.lock().unwrap().insert(program.to_owned(), answer);
    }

    fn count(&self, program: &str) -> usize {
        self.calls.lock().unwrap().iter().filter(|c| c.split(' ').next() == Some(program)).count()
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl CommandRunner for Programs {
    fn output(&self, program: &str, args: &[&str], _timeout: Duration) -> io::Result<Vec<u8>> {
        self.calls.lock().unwrap().push(format!("{program} {}", args.join(" ")));
        match self.answers.lock().unwrap().get(program) {
            Some(Ok(text)) => Ok(text.clone().into_bytes()),
            Some(Err(kind)) => Err((*kind).into()),
            None => Err(io::ErrorKind::NotFound.into()),
        }
    }
}

struct Fx {
    api: Api,
    rig: Rig,
    dir: Dir,
    programs: Arc<Programs>,
}

impl Fx {
    /// A fake laptop: CPU with two cores, hwmon chips, NVIDIA card asleep, Wi-Fi, one disk.
    fn new() -> Self {
        let dir = Dir::new();
        laptop(&dir);
        let (services, rig) = Services::simulated(&dir.0).expect("services");
        let programs = Arc::new(Programs::default());
        programs.answer("nvidia-smi", Ok(GPU_LINE));
        programs.answer("df", Ok("1B-blocks Used Avail Mounted on\n 1000000 250000 750000 /\n"));
        programs.answer("ip", Ok("2: wlan0    inet 192.168.1.5/24 brd 192.168.1.255 scope global wlan0\n"));
        let api = Api::new(services).with_command_runner(programs.clone());
        Self { api, rig, dir, programs }
    }

    fn set_time(&self, secs: u64) {
        self.rig.clock.store(secs, Ordering::SeqCst);
    }

    fn gpu_status(&self, status: &str) {
        self.dir.put("sys/bus/pci/devices/0000:01:00.0/power/runtime_status", &format!("{status}\n"));
    }
}

const GPU_LINE: &str = "NVIDIA GeForce RTX 5070 Ti Laptop GPU, 580.95.05, 47, 3, 1, 12227, 1210, 10780, 14.86, 115.00, [N/A], 5.00, 115.00, 705, 405, 3105, 12001, P8, 1, 5, 16, 16, [N/A], 98.05.50.00.4B\n";

fn stat(user: u64, idle: u64) -> String {
    format!(
        "cpu  {} 0 0 {} 0 0 0 0 0 0\ncpu0 {} 0 0 {} 0 0 0 0 0 0\ncpu1 {} 0 0 {} 0 0 0 0 0 0\n",
        user,
        idle,
        user / 2,
        idle / 2,
        user / 2,
        idle / 2
    )
}

fn laptop(d: &Dir) {
    d.put("proc/stat", &stat(100, 900));
    d.put("proc/loadavg", "0.52 0.58 0.59 1/1234 5678\n");
    d.put(
        "proc/meminfo",
        "MemTotal: 1000 kB\nMemAvailable: 400 kB\nMemFree: 100 kB\nSwapTotal: 100 kB\nSwapFree: 50 kB\n",
    );
    d.put("proc/uptime", "3600.5 100.0\n");
    d.put("proc/cpuinfo", "processor : 0\nmodel name : Test CPU\nphysical id : 0\ncore id : 0\n\nprocessor : 1\nmodel name : Test CPU\nphysical id : 0\ncore id : 1\n");
    d.put("proc/sys/kernel/osrelease", "6.12.1\n");
    d.put("etc/os-release", "PRETTY_NAME=\"Test OS\"\n");
    d.put("etc/passwd", "root:x:0:0::/root:/bin/sh\n");
    for (pid, comm, ticks, rss) in [(10u32, "busy", 100u64, 1000u64), (11, "big", 5, 90_000)] {
        d.put(
            &format!("proc/{pid}/stat"),
            &format!("{pid} ({comm}) S 1 1 1 0 -1 0 0 0 0 0 {ticks} 0 0 0 20 0 1 0 5 1 {rss} 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n"),
        );
    }
    d.put("sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq", "3000000\n");
    d.put("sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq", "5000000\n");
    d.put("sys/devices/system/cpu/cpu0/cpufreq/scaling_governor", "powersave\n");
    d.put("sys/devices/system/cpu/cpu1/cpufreq/scaling_cur_freq", "1000000\n");
    d.put("sys/class/hwmon/hwmon0/name", "acpitz\n");
    d.put("sys/class/hwmon/hwmon0/temp1_input", "28000\n");
    d.put("sys/class/hwmon/hwmon1/name", "nvme\n");
    d.put("sys/class/hwmon/hwmon1/temp1_input", "41000\n");
    d.put("sys/class/hwmon/hwmon1/temp1_label", "Composite\n");
    d.put("sys/class/hwmon/hwmon2/name", "coretemp\n");
    d.put("sys/class/hwmon/hwmon2/temp1_input", "52000\n");
    d.put("sys/class/hwmon/hwmon2/temp1_label", "Package id 0\n");
    d.put("sys/class/hwmon/hwmon3/name", "nouveau\n");
    d.put("sys/class/hwmon/hwmon3/temp1_input", "44000\n");
    d.put("sys/class/net/wlan0/type", "1\n");
    d.put("sys/class/net/wlan0/operstate", "up\n");
    d.put("sys/class/net/wlan0/wireless/x", "\n");
    d.put("sys/class/net/lo/type", "772\n");
    d.put("proc/net/dev", &net_dev(1_000, 500));
    d.put("sys/bus/pci/devices/0000:01:00.0/vendor", "0x10de\n");
    d.put("sys/bus/pci/devices/0000:01:00.0/class", "0x030200\n");
    d.put("sys/bus/pci/devices/0000:01:00.0/device", "0x2c19\n");
    d.put("sys/bus/pci/devices/0000:01:00.0/power/runtime_status", "suspended\n");
    d.put("proc/self/mounts", "/dev/nvme0n1p2 / ext4 rw 0 0\ntmpfs /tmp tmpfs rw 0 0\n");
    d.put("sys/block/nvme0n1/size", "2000409264\n");
}

fn net_dev(rx: u64, tx: u64) -> String {
    format!("h\nh\n    lo: 5 1 0 0 0 0 0 0 5 1 0 0 0 0 0 0\n wlan0: {rx} 1 0 0 0 0 0 0 {tx} 1 0 0 0 0 0 0\n")
}

fn root(fx: &Fx) -> &Path {
    &fx.dir.0
}

// ---- overview and temperatures

#[test]
fn the_overview_lists_every_sensor_with_kinds_and_leaves_the_sleeping_gpu_chip_alone() {
    let fx = Fx::new();
    let o = fx.api.monitor_overview();
    let ids: Vec<&str> = o.temperatures.iter().map(|t| t.id.as_str()).collect();
    assert!(
        ids.contains(&"coretemp/temp1") && ids.contains(&"nvme/temp1") && ids.contains(&"acpitz/temp1"),
        "{ids:?}"
    );
    assert!(!ids.contains(&"nouveau/temp1"), "reading a sleeping GPU's hwmon would wake it");
    assert!(
        ids.contains(&"firmware/gpu"),
        "the firmware's GPU sensor stands in for the missing hwmon one: {ids:?}"
    );
    let cpu = o.temperatures.iter().find(|t| t.id == "coretemp/temp1").unwrap();
    assert_eq!((cpu.celsius, cpu.kind, cpu.label.as_str()), (52.0, SensorKind::CpuPackage, "Package id 0"));
    assert_eq!(o.gpu_state, GpuPower::Suspended);
    assert_eq!(o.uptime_s, Some(3600.5));
    assert_eq!(o.memory_used_pct, Some(60.0));
    assert_eq!(o.swap_used_pct, Some(50.0));
    assert_eq!(o.cpu_freq_mhz, Some(3000));
    assert_eq!(fx.programs.count("nvidia-smi"), 0);
}

#[test]
fn an_awake_gpu_chip_is_read_with_the_rest() {
    let fx = Fx::new();
    fx.gpu_status("active");
    let ids: Vec<String> = fx.api.monitor_overview().temperatures.into_iter().map(|t| t.id).collect();
    assert!(ids.contains(&"nouveau/temp1".to_owned()));
    assert!(!ids.contains(&"firmware/gpu".to_owned()), "a real GPU sensor is not duplicated");
}

#[test]
fn minimums_and_maximums_follow_the_readings_until_reset() {
    let fx = Fx::new();
    let at = |fx: &Fx| {
        fx.api.monitor_overview().temperatures.into_iter().find(|t| t.id == "coretemp/temp1").unwrap()
    };
    assert_eq!((at(&fx).session_min_c, at(&fx).session_max_c), (52.0, 52.0));
    fx.dir.put("sys/class/hwmon/hwmon2/temp1_input", "70000\n");
    fx.dir.put("sys/class/hwmon/hwmon2/temp1_max", "100000\n");
    let hot = at(&fx);
    assert_eq!(
        (hot.celsius, hot.session_min_c, hot.session_max_c, hot.high_c),
        (70.0, 52.0, 70.0, Some(100.0))
    );
    fx.dir.put("sys/class/hwmon/hwmon2/temp1_input", "40000\n");
    let cool = at(&fx);
    assert_eq!((cool.session_min_c, cool.session_max_c), (40.0, 70.0));
    fx.api.monitor_reset();
    let again = at(&fx);
    assert_eq!((again.session_min_c, again.session_max_c), (40.0, 40.0));
}

#[test]
fn a_machine_without_hwmon_still_answers_from_the_firmware() {
    let fx = Fx::new();
    std::fs::remove_dir_all(root(&fx).join("sys/class/hwmon")).unwrap();
    let o = fx.api.monitor_overview();
    let kinds: Vec<SensorKind> = o.temperatures.iter().map(|t| t.kind).collect();
    assert!(
        kinds.contains(&SensorKind::CpuPackage)
            && kinds.contains(&SensorKind::Gpu)
            && kinds.contains(&SensorKind::Board),
        "{kinds:?}"
    );
    assert!(o.temperatures.iter().all(|t| t.chip == "firmware"));
}

// ---- cpu

#[test]
fn the_first_cpu_answer_is_primed_and_the_next_one_measures() {
    let fx = Fx::new();
    let first = fx.api.monitor_cpu(5);
    let slept = fx.rig.sleeps.lock().unwrap_or_else(PoisonError::into_inner).clone();
    assert_eq!(slept, vec![PRIME_DELAY, PRIME_DELAY], "one pause for the CPU, one for the process scan");
    assert_eq!(first.load_pct, None, "no time passed between the two reads of the fake");
    assert_eq!(first.cores.len(), 2);
    assert_eq!(first.cores[0].freq_mhz, Some(3000));
    assert_eq!((first.cores[0].max_freq_mhz, first.cores[1].max_freq_mhz), (Some(5000), None));
    assert_eq!((first.freq_avg_mhz, first.freq_max_mhz), (Some(2000), Some(3000)));
    assert_eq!(first.governor.as_deref(), Some("powersave"));
    assert_eq!(first.load_avg.map(|l| l.one), Some(0.52));
    assert_eq!(first.power_w, None, "RAPL is not readable here");
    assert!(!first.top_valid);
    assert_eq!(first.processes, 2);
    // 1000 ticks pass: 300 busy of them; pid 10 used 200 of them
    fx.dir.put("proc/stat", &stat(400, 1600));
    fx.dir.put("proc/10/stat", "10 (busy) S 1 1 1 0 -1 0 0 0 0 0 300 0 0 0 20 0 1 0 5 1 1000 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n");
    fx.set_time(1_700_000_005);
    let second = fx.api.monitor_cpu(5);
    assert_eq!(second.load_pct, Some(30.0));
    assert_eq!(second.cores[0].load_pct, Some(30.0));
    assert!(second.top_valid);
    assert_eq!(second.top[0].name, "busy");
    // 200 ticks of the 1000 that passed on 2 CPUs: 200 / 1000 * 100 * 2 = 40 % of one core
    assert!((second.top[0].cpu_pct - 40.0).abs() < 1e-9, "{}", second.top[0].cpu_pct);
    assert_eq!(fx.rig.sleeps.lock().unwrap().len(), 2, "primed once");
}

#[test]
fn two_pages_asking_at_once_share_one_scan_and_one_sample() {
    let fx = Fx::new();
    fx.api.monitor_cpu(5);
    fx.dir.put("proc/12/stat", "12 (late) S 1 1 1 0 -1 0 0 0 0 0 1 0 0 0 20 0 1 0 5 1 5 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n");
    assert_eq!(fx.api.monitor_memory(5).processes, 2, "same instant: the scan from a moment ago");
    fx.set_time(1_700_000_002);
    assert_eq!(fx.api.monitor_memory(5).processes, 3, "2 s later: a new scan");
}

#[test]
fn the_ranking_size_is_capped() {
    let fx = Fx::new();
    for pid in 100..160u32 {
        fx.dir.put(&format!("proc/{pid}/stat"), &format!("{pid} (p) S 1 1 1 0 -1 0 0 0 0 0 1 0 0 0 20 0 1 0 5 1 {pid} 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n"));
    }
    assert_eq!(fx.api.monitor_memory(1000).top.len(), MAX_TOP);
    assert_eq!(fx.api.monitor_memory(0).top.len(), 1);
}

// ---- memory, network, disks

#[test]
fn memory_reports_bytes_and_the_biggest_processes() {
    let fx = Fx::new();
    let m = fx.api.monitor_memory(3);
    let mem = m.memory.unwrap();
    assert_eq!((mem.total_bytes, mem.used_bytes, mem.swap_used_bytes), (1_024_000, 614_400, 51_200));
    assert_eq!(m.top[0].name, "big");
    assert_eq!(m.top[0].mem_bytes, 90_000 * 4096);
    let owner = std::os::unix::fs::MetadataExt::uid(&std::fs::metadata(root(&fx).join("proc/11")).unwrap());
    assert_eq!(m.top[0].user, owner.to_string(), "the owner's number when passwd does not know it");
}

#[test]
fn the_network_tab_has_rates_peaks_totals_and_the_address() {
    let fx = Fx::new();
    let first = fx.api.monitor_network();
    assert_eq!(first.sample.rx_bps, None);
    assert_eq!(first.sample.primary.as_deref(), Some("wlan0"), "up Wi-Fi without a route");
    assert_eq!(first.primary_address.as_deref(), Some("192.168.1.5"));
    fx.dir.put("proc/net/dev", &net_dev(3_000, 1_500));
    fx.set_time(1_700_000_002);
    let second = fx.api.monitor_network();
    assert_eq!((second.sample.rx_bps, second.sample.tx_bps), (Some(1000.0), Some(500.0)));
    assert_eq!(second.sample.session_rx_bytes, 2_000);
    assert_eq!((second.sample.peak_rx_bps, second.sample.peak_tx_bps), (1000.0, 500.0));
    let wlan = second.sample.interfaces.iter().find(|i| i.name == "wlan0").unwrap();
    assert_eq!(wlan.kind, ts_system::netstat::InterfaceKind::Wifi);
    assert_eq!(fx.programs.count("ip"), 1, "the addresses are cached for ten seconds");
    fx.set_time(1_700_000_020);
    fx.api.monitor_network();
    assert_eq!(fx.programs.count("ip"), 2);
    fx.api.monitor_reset();
    fx.dir.put("proc/net/dev", &net_dev(3_100, 1_500));
    fx.set_time(1_700_000_022);
    let after = fx.api.monitor_network();
    assert_eq!((after.sample.session_rx_bytes, after.sample.peak_rx_bps), (100, 50.0));
}

#[test]
fn a_missing_ip_command_only_loses_the_address() {
    let fx = Fx::new();
    fx.programs.answer("ip", Err(io::ErrorKind::NotFound));
    let n = fx.api.monitor_network();
    assert!(n.addresses.is_empty());
    assert_eq!(n.primary_address, None);
    assert_eq!(n.sample.primary.as_deref(), Some("wlan0"));
}

#[test]
fn disks_come_from_one_df_run_and_sysfs() {
    let fx = Fx::new();
    fx.dir.put("sys/class/hwmon/hwmon1/temp1_max", "81000\n");
    let d = fx.api.monitor_disks();
    assert_eq!(d.filesystems.len(), 1);
    assert_eq!((d.filesystems[0].mount.as_str(), d.filesystems[0].used_pct), ("/", 25.0));
    assert_eq!(d.drives.len(), 1);
    assert_eq!(d.nvme[0].celsius, 41.0);
    assert_eq!(fx.programs.count("df"), 1);
    assert_eq!(
        fx.programs.calls().iter().find(|c| c.starts_with("df")).unwrap(),
        "df -B1 --output=size,used,avail,target -- /"
    );
}

#[test]
fn without_df_the_disks_tab_still_has_drives_and_temperatures() {
    let fx = Fx::new();
    fx.programs.answer("df", Err(io::ErrorKind::NotFound));
    let d = fx.api.monitor_disks();
    assert!(d.filesystems.is_empty());
    assert_eq!((d.drives.len(), d.nvme.len()), (1, 1));
}

// ---- the GPU

#[test]
fn a_sleeping_gpu_is_reported_and_never_asked() {
    let fx = Fx::new();
    for detail in [false, true] {
        let g = fx.api.monitor_gpu(detail);
        assert_eq!(g.state, GpuPower::Suspended);
        assert!(g.note.unwrap().contains("not woken"));
        assert!(g.detail.is_none() && g.error.is_none());
        assert_eq!(g.source, GpuSource::Firmware, "the firmware's sensor does not wake the card");
        assert!(g.temperature_c.is_some());
    }
    fx.api.system_info();
    fx.api.monitor_overview();
    fx.api.monitor_disks();
    assert_eq!(fx.programs.count("nvidia-smi"), 0, "no process was started for the card");
}

#[test]
fn an_awake_gpu_without_detail_starts_no_process() {
    let fx = Fx::new();
    fx.gpu_status("active");
    let g = fx.api.monitor_gpu(false);
    assert_eq!(g.state, GpuPower::Active);
    assert_eq!(fx.programs.count("nvidia-smi"), 0);
    assert!(g.detail.is_none());
}

#[test]
fn detail_is_one_shot_rate_limited_and_cached_between() {
    let fx = Fx::new();
    fx.gpu_status("active");
    let first = fx.api.monitor_gpu(true);
    assert_eq!(fx.programs.count("nvidia-smi"), 1);
    let d = first.detail.as_ref().unwrap();
    assert_eq!(
        (d.pstate.as_deref(), d.vram_total_mib, d.power_limit_w),
        (Some("P8"), Some(12227.0), Some(115.0))
    );
    assert_eq!(first.source, GpuSource::NvidiaSmi);
    assert_eq!(
        (first.temperature_c, first.utilization_pct, first.power_w),
        (Some(47.0), Some(3.0), Some(14.86))
    );
    assert_eq!(first.name.as_deref(), Some("NVIDIA GeForce RTX 5070 Ti Laptop GPU"));
    assert!(!first.detail_cached);
    assert_eq!(first.next_detail_in_s, nvidia::MIN_QUERY_INTERVAL_SECS);
    let args = fx.programs.calls().into_iter().find(|c| c.starts_with("nvidia-smi")).unwrap();
    assert!(
        args.contains("--query-gpu=") && !args.contains(" -l") && args.contains("--id=0"),
        "a one-shot query, never a stream: {args}"
    );
    // a second later: cached, no new process
    fx.set_time(1_700_000_001);
    let again = fx.api.monitor_gpu(true);
    assert_eq!(fx.programs.count("nvidia-smi"), 1);
    assert!(again.detail_cached);
    assert_eq!(again.detail_age_s, Some(1.0));
    assert_eq!(again.next_detail_in_s, 4.0);
    assert!(again.detail.is_some());
    // ten calls in the same second still cost nothing
    for _ in 0..10 {
        fx.api.monitor_gpu(true);
    }
    assert_eq!(fx.programs.count("nvidia-smi"), 1);
    // five seconds after the first: a new query
    fx.set_time(1_700_000_005);
    let later = fx.api.monitor_gpu(true);
    assert_eq!(fx.programs.count("nvidia-smi"), 2);
    assert!(!later.detail_cached);
}

#[test]
fn a_failed_query_is_reported_and_not_retried_every_second() {
    let fx = Fx::new();
    fx.gpu_status("active");
    fx.programs.answer(
        "nvidia-smi",
        Ok("NVIDIA-SMI has failed because it couldn't communicate with the NVIDIA driver.\n"),
    );
    let g = fx.api.monitor_gpu(true);
    assert!(g.detail.is_none());
    assert!(g.error.unwrap().contains("fields"));
    fx.set_time(1_700_000_001);
    let g = fx.api.monitor_gpu(true);
    assert_eq!(fx.programs.count("nvidia-smi"), 1, "a failure is not a reason to hammer the driver");
    assert!(g.error.is_some() && g.detail_cached);
    fx.programs.answer("nvidia-smi", Err(io::ErrorKind::NotFound));
    fx.set_time(1_700_000_010);
    assert!(fx.api.monitor_gpu(true).error.unwrap().contains("could not be run"));
}

#[test]
fn the_daemons_live_values_are_used_only_when_they_are_real() {
    let fx = Fx::new();
    fx.gpu_status("active");
    fx.rig.daemon.answer_ok("state", daemon_state(json!({}), json!({"gpu": 0.0, "gpuw": 0.0}), 0));
    let idle = fx.api.monitor_gpu(false);
    assert_eq!(
        (idle.utilization_pct, idle.power_w, idle.source),
        (None, None, GpuSource::Firmware),
        "0 and 0 is a stream that is not running"
    );
    fx.rig.daemon.answer_ok("state", daemon_state(json!({}), json!({"gpu": 35.0, "gpuw": 22.0}), 0));
    let fed = fx.api.monitor_gpu(false);
    assert_eq!((fed.utilization_pct, fed.power_w, fed.source), (Some(35.0), Some(22.0), GpuSource::Daemon));
    assert_eq!(fx.programs.count("nvidia-smi"), 0);
}

#[test]
fn a_laptop_without_an_nvidia_card_says_so() {
    let fx = Fx::new();
    std::fs::remove_dir_all(root(&fx).join("sys/bus/pci")).unwrap();
    let g = fx.api.monitor_gpu(true);
    assert_eq!(g.state, GpuPower::Absent);
    assert!(g.note.unwrap().contains("no NVIDIA"));
    assert_eq!(fx.programs.count("nvidia-smi"), 0);
}

#[test]
fn over_a_fake_root_without_a_runner_no_program_can_start() {
    let dir = Dir::new();
    laptop(&dir);
    dir.put("sys/bus/pci/devices/0000:01:00.0/power/runtime_status", "active\n");
    let (services, _rig) = Services::simulated(&dir.0).unwrap();
    let api = Api::new(services);
    let g = api.monitor_gpu(true);
    assert!(
        g.error.unwrap().contains("not available here"),
        "the default runner for a fake root starts nothing"
    );
    assert!(api.monitor_disks().filesystems.is_empty());
}

// ---- system info

#[test]
fn system_info_is_cached_and_names_the_gpu_only_when_it_is_awake() {
    let fx = Fx::new();
    let asleep = fx.api.system_info();
    assert_eq!(asleep.os.pretty_name, "Test OS");
    assert_eq!(asleep.cpu.threads, 2);
    assert_eq!(asleep.kernel.as_deref(), Some("6.12.1"));
    assert_eq!(asleep.gpus[0].name, None);
    assert_eq!(fx.programs.count("nvidia-smi"), 0);
    // it wakes up (somebody started a game): the next call names it, once
    fx.gpu_status("active");
    fx.set_time(1_700_000_100);
    let awake = fx.api.system_info();
    assert_eq!(awake.gpus[0].name.as_deref(), Some("NVIDIA GeForce RTX 5070 Ti Laptop GPU"));
    assert_eq!(awake.gpus[0].driver.as_deref(), Some("580.95.05"));
    assert_eq!(fx.programs.count("nvidia-smi"), 1);
    fx.set_time(1_700_000_200);
    fx.dir.put("proc/uptime", "9999.0 1.0\n");
    let again = fx.api.system_info();
    assert_eq!(again.uptime_s, Some(9999.0), "uptime is fresh, the rest is cached");
    assert_eq!(fx.programs.count("nvidia-smi"), 1, "the name is known: no more queries");
    // the GPU tab uses the cached name too
    assert_eq!(fx.api.monitor_gpu(false).name.as_deref(), Some("NVIDIA GeForce RTX 5070 Ti Laptop GPU"));
}

// ---- the whole set

#[test]
fn nothing_needs_the_daemon_and_nothing_fails_as_a_whole() {
    let fx = Fx::new();
    fx.rig.daemon.set_available(false);
    fx.api.monitor_overview();
    fx.api.monitor_cpu(5);
    fx.api.monitor_gpu(true);
    fx.api.monitor_memory(5);
    fx.api.monitor_network();
    fx.api.monitor_disks();
    fx.api.system_info();
    assert!(fx.rig.daemon.calls().is_empty(), "the daemon was not even asked");
}

#[test]
fn an_empty_machine_answers_with_nulls_not_errors() {
    let dir = Dir::new();
    let (services, _rig) = Services::simulated(&dir.0).unwrap();
    let api = Api::new(services);
    let o = api.monitor_overview();
    assert_eq!((o.cpu_load_pct, o.memory_used_pct, o.net_primary, o.uptime_s), (None, None, None, None));
    let c = api.monitor_cpu(5);
    assert!(c.cores.is_empty() && c.top.is_empty() && c.load_avg.is_none());
    assert!(api.monitor_memory(5).memory.is_none());
    assert!(api.monitor_network().sample.interfaces.is_empty());
    assert!(api.monitor_disks().filesystems.is_empty());
    assert_eq!(api.monitor_gpu(true).state, GpuPower::Absent);
    assert_eq!(api.system_info().machine.tier.as_str(), "unsupported");
}

/// The JSON field names are the contract with the page: a rename must be deliberate.
#[test]
fn the_json_field_names_are_stable() {
    let fx = Fx::new();
    fx.gpu_status("active");
    fx.api.monitor_cpu(1);
    fx.set_time(1_700_000_010);
    let keys = |v: &Value| -> Vec<String> { v.as_object().unwrap().keys().cloned().collect() };
    let check = |v: Value, expected: &[&str]| {
        let mut got = keys(&v);
        got.sort();
        let mut want: Vec<String> = expected.iter().map(|s| (*s).to_owned()).collect();
        want.sort();
        assert_eq!(got, want);
    };
    check(
        serde_json::to_value(fx.api.monitor_overview()).unwrap(),
        &[
            "t",
            "temperatures",
            "cpu_load_pct",
            "cpu_freq_mhz",
            "memory_used_pct",
            "swap_used_pct",
            "gpu_state",
            "net_primary",
            "net_rx_bps",
            "net_tx_bps",
            "uptime_s",
        ],
    );
    let cpu = serde_json::to_value(fx.api.monitor_cpu(2)).unwrap();
    check(
        cpu.clone(),
        &[
            "t",
            "load_pct",
            "cores",
            "freq_avg_mhz",
            "freq_max_mhz",
            "governor",
            "load_avg",
            "power_w",
            "temperature_c",
            "top",
            "top_valid",
            "processes",
        ],
    );
    check(cpu["cores"][0].clone(), &["id", "load_pct", "freq_mhz", "max_freq_mhz"]);
    check(
        cpu["top"][0].clone(),
        &["pid", "name", "cmdline", "user", "state", "cpu_pct", "mem_bytes", "threads"],
    );
    check(cpu["load_avg"].clone(), &["one", "five", "fifteen", "running", "tasks"]);
    check(serde_json::to_value(fx.api.monitor_memory(1)).unwrap(), &["t", "memory", "top", "processes"]);
    let mem = serde_json::to_value(fx.api.monitor_memory(1)).unwrap();
    check(
        mem["memory"].clone(),
        &[
            "total_bytes",
            "used_bytes",
            "available_bytes",
            "free_bytes",
            "cache_bytes",
            "used_pct",
            "swap_total_bytes",
            "swap_used_bytes",
            "swap_used_pct",
        ],
    );
    let net = serde_json::to_value(fx.api.monitor_network()).unwrap();
    check(
        net.clone(),
        &[
            "t",
            "primary",
            "interfaces",
            "rx_bps",
            "tx_bps",
            "session_rx_bytes",
            "session_tx_bytes",
            "peak_rx_bps",
            "peak_tx_bps",
            "addresses",
            "primary_address",
        ],
    );
    check(
        net["interfaces"][1].clone(),
        &[
            "name",
            "kind",
            "state",
            "mac",
            "speed_mbps",
            "rx_bps",
            "tx_bps",
            "peak_rx_bps",
            "peak_tx_bps",
            "session_rx_bytes",
            "session_tx_bytes",
            "total_rx_bytes",
            "total_tx_bytes",
        ],
    );
    let disks = serde_json::to_value(fx.api.monitor_disks()).unwrap();
    check(disks.clone(), &["t", "filesystems", "drives", "nvme"]);
    check(
        disks["filesystems"][0].clone(),
        &["device", "fstype", "mount", "total_bytes", "used_bytes", "avail_bytes", "used_pct"],
    );
    check(disks["drives"][0].clone(), &["name", "model", "size_bytes", "kind"]);
    check(disks["nvme"][0].clone(), &["controller", "model", "celsius", "max_c", "crit_c"]);
    let gpu = serde_json::to_value(fx.api.monitor_gpu(true)).unwrap();
    check(
        gpu.clone(),
        &[
            "t",
            "state",
            "note",
            "name",
            "driver",
            "temperature_c",
            "utilization_pct",
            "power_w",
            "source",
            "detail",
            "detail_age_s",
            "detail_cached",
            "next_detail_in_s",
            "error",
        ],
    );
    check(
        gpu["detail"].clone(),
        &[
            "name",
            "driver",
            "vbios",
            "temperature_c",
            "utilization_pct",
            "memory_utilization_pct",
            "vram_total_mib",
            "vram_used_mib",
            "vram_free_mib",
            "power_w",
            "power_limit_w",
            "power_default_limit_w",
            "power_min_limit_w",
            "power_max_limit_w",
            "clock_graphics_mhz",
            "clock_memory_mhz",
            "clock_graphics_max_mhz",
            "clock_memory_max_mhz",
            "pstate",
            "pcie_gen",
            "pcie_gen_max",
            "pcie_width",
            "pcie_width_max",
            "fan_pct",
        ],
    );
    assert_eq!(gpu["state"], "active");
    assert_eq!(gpu["source"], "nvidia_smi");
    let t = serde_json::to_value(fx.api.monitor_overview()).unwrap();
    check(
        t["temperatures"][0].clone(),
        &["id", "chip", "label", "kind", "celsius", "session_min_c", "session_max_c", "high_c", "critical_c"],
    );
    let info = serde_json::to_value(fx.api.system_info()).unwrap();
    check(
        info.clone(),
        &["machine", "serial", "cpu", "memory_total_bytes", "gpus", "drives", "os", "kernel", "uptime_s"],
    );
    check(info["cpu"].clone(), &["model", "sockets", "cores", "threads", "max_mhz"]);
    check(info["os"].clone(), &["pretty_name", "id", "version"]);
    check(info["gpus"][0].clone(), &["address", "vendor", "device_id", "boot_vga", "name", "driver"]);
}
