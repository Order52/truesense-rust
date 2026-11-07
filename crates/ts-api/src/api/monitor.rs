//! The Monitor page's data: temperatures, CPU, GPU, memory, network, disks and the machine's facts.
//!
//! Every operation here reads the machine directly as the user (files below [`Services::root`]
//! and, for the GPU and `ip`/`df`, an injected [`CommandRunner`]); none needs the daemon, and none
//! fails as a whole: a part that cannot be read is `null` and the rest is still answered, because
//! a page must not go blank because one drive is slow.
//!
//! # How often a page may call
//!
//! | operation | cost of a call | call at most |
//! |---|---|---|
//! | `monitor_overview` | hwmon files (the NVMe and Wi-Fi sensors are device commands, about 20 ms together), one firmware read | every 2 s |
//! | `monitor_cpu` | `/proc/stat`, sysfs frequencies, one `/proc` scan shared by all callers | every 1 s |
//! | `monitor_memory` | `/proc/meminfo` and the shared scan | every 1 s |
//! | `monitor_network` | `/proc/net/dev`; `ip` at most every 10 s | every 1 s |
//! | `monitor_disks` | one `df` run, NVMe sensors | every 5 s |
//! | `monitor_gpu(false)` | firmware temperature, daemon state, PCI power state: starts no process | every 1 s |
//! | `monitor_gpu(true)` | one `nvidia-smi` run, **at most once per 5 s** (calls in between answer the cached reading, marked as such) | every 5 s |
//! | `system_info` | cached after the first call | once |
//!
//! Calls faster than that are safe, only wasteful: samples are shared between callers and answers
//! younger than half a second are reused.
//!
//! # The GPU is never woken
//!
//! The discrete GPU is runtime-suspended whenever nothing uses it. Asking it anything resumes it,
//! and every `nvidia-smi` start initialises NVML, which on this laptop stalls all CPU cores for a
//! moment (see `ts_system::nvidia`). So: the power state is read first from `runtime_status`
//! (which does not wake it) and a suspended card is reported as `suspended` with no query; the
//! query is one-shot (never a new `nvidia-smi -l` stream), rate limited, and made only when the
//! page asks for `detail`; the cheap fields come from the firmware temperature and from the
//! daemon's live values, which already exist. A page that polls `detail` keeps the card awake, so
//! it should do so only while its GPU tab is visible.
//!
//! # What the daemon feeds
//!
//! The daemon's `gpu` and `gpuw` live values are averages of an `nvidia-smi -l 2` stream that the
//! daemon runs only while a smart feature needs it, never for a sleeping card, and ends when the
//! card has been idle for a minute so that it can sleep; without the stream they are 0, and the
//! live key `gpu_state` says what that 0 means (`suspended` is true zero, `unsampled` is unknown).
//! They are used only when non-zero, and labelled `source: "daemon"`.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use ts_platform::hw::{CommandRunner, Sleeper};
use ts_system::disks::{self, BlockDevice, DfStats, DiskUsage, NvmeTemp};
use ts_system::netstat::{self, NetSample, NetSampler};
use ts_system::nvidia::{self, GpuMetrics, GpuPower};
use ts_system::sysinfo;
use ts_system::temps::{self, ExtremesTracker, ReadOptions, SensorKind, TempSensor};
use ts_system::usage::{
    CpuLoad, CpuSampler, LoadAverage, MemoryUsage, ProcSampler, ProcessInfo, RaplMeter, read_freqs,
    read_governor, read_loadavg, read_meminfo,
};

pub use ts_system::sysinfo::SystemInfo;

use super::Api;
use ts_foundation::sync::locked;

/// The most processes a ranking can have.
pub const MAX_TOP: usize = 25;

/// Answers younger than this are reused (two page tabs asking in the same instant).
const FRESH_SECS: f64 = 0.5;

/// The pause that gives a new sampler its second sample, so the first answer already has numbers.
const PRIME_DELAY: Duration = Duration::from_millis(300);

/// The shortest time between two scans of `/proc` for the process rankings.
const PROC_INTERVAL_SECS: f64 = 1.5;

/// How long `nvidia-smi` may take before it is given up on.
const GPU_QUERY_TIMEOUT: Duration = Duration::from_secs(4);

/// How long `ip` may take.
const IP_TIMEOUT: Duration = Duration::from_secs(2);

/// How long the local addresses are reused.
const IP_CACHE_SECS: f64 = 10.0;

/// The kernel's page size on every machine this runs on (x86-64); `/proc/<pid>/stat` counts memory in pages.
const PAGE_BYTES: u64 = 4096;

/// A runner for a process that is not wanted (an API over a fake root): every program "is not
/// installed". Chosen automatically whenever the root is not the real `/`, so a test can never run
/// a real `nvidia-smi`.
struct NoPrograms;

impl CommandRunner for NoPrograms {
    fn output(&self, program: &str, _args: &[&str], _timeout: Duration) -> std::io::Result<Vec<u8>> {
        Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("{program}: not available here")))
    }
}

/// The last `nvidia-smi` attempt.
#[derive(Debug, Clone, Default)]
struct GpuCache {
    at: Option<f64>,
    metrics: Option<GpuMetrics>,
    error: Option<String>,
}

/// An answer and when it was made.
type Stamped<T> = Option<(f64, T)>;

/// A sampler with its latest answer, and whether it was primed (so a missing file is not
/// re-primed, and slept for, on every call).
struct Sampled<S, T> {
    sampler: S,
    last: Stamped<T>,
    primed: bool,
}

impl<S, T> Sampled<S, T> {
    fn new(sampler: S) -> Self {
        Self { sampler, last: None, primed: false }
    }

    /// The latest answer when it is younger than [`FRESH_SECS`].
    fn fresh(&self, now: f64) -> Option<&T> {
        self.last.as_ref().filter(|(at, _)| now >= *at && now - *at < FRESH_SECS).map(|(_, v)| v)
    }
}

/// The state of the page's samplers and caches: one per [`Api`].
pub(crate) struct Monitor {
    root: PathBuf,
    runner: Arc<dyn CommandRunner>,
    cpu: Mutex<Sampled<CpuSampler, CpuLoad>>,
    rapl: Mutex<RaplMeter>,
    procs: Mutex<ProcSampler>,
    net: Mutex<Sampled<NetSampler, NetSample>>,
    extremes: Mutex<ExtremesTracker>,
    gpu: Mutex<GpuCache>,
    facts: Mutex<Option<SystemInfo>>,
    addresses: Mutex<Stamped<BTreeMap<String, Vec<String>>>>,
}

impl Monitor {
    /// Samplers reading below `root`. Programs are run only when `root` is the real `/`; use
    /// [`Monitor::with_runner`] to choose.
    pub(crate) fn new(root: PathBuf) -> Self {
        let runner: Arc<dyn CommandRunner> = if root == std::path::Path::new("/") {
            Arc::new(ts_platform::hw::ProcessRunner)
        } else {
            Arc::new(NoPrograms)
        };
        Self {
            cpu: Mutex::new(Sampled::new(CpuSampler::new(&root))),
            rapl: Mutex::new(RaplMeter::new(&root)),
            procs: Mutex::new(ProcSampler::new(&root, MAX_TOP, PROC_INTERVAL_SECS, PAGE_BYTES)),
            net: Mutex::new(Sampled::new(NetSampler::new(&root))),
            extremes: Mutex::new(ExtremesTracker::new()),
            gpu: Mutex::new(GpuCache::default()),
            facts: Mutex::new(None),
            addresses: Mutex::new(None),
            runner,
            root,
        }
    }

    pub(crate) fn with_runner(mut self, runner: Arc<dyn CommandRunner>) -> Self {
        self.runner = runner;
        self
    }
}

/// A temperature on a gauge, with the lowest and highest value seen since the page started.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TemperatureReading {
    /// A name that stays the same between boots (`coretemp/temp1`, `nvme#1/temp1`, `firmware/gpu`).
    pub id: String,
    /// The driver's chip name.
    pub chip: String,
    /// The sensor's label (`Package id 0`, `Composite`).
    pub label: String,
    /// What it measures: `cpu_package`, `cpu_core`, `cpu`, `board`, `nvme`, `wifi`, `gpu`, `memory`, `other`.
    pub kind: SensorKind,
    /// Degrees Celsius.
    pub celsius: f64,
    /// The lowest value since the page started or was reset.
    pub session_min_c: f64,
    /// The highest value since then.
    pub session_max_c: f64,
    /// The driver's warning threshold, when it gives one.
    pub high_c: Option<f64>,
    /// The driver's critical threshold, when it gives one.
    pub critical_c: Option<f64>,
}

/// The Monitor page's overview: every temperature and the headline of each tab.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MonitorOverview {
    /// Seconds since the epoch.
    pub t: i64,
    /// Every temperature sensor, hwmon ones first, then the firmware's (GPU, and board or CPU
    /// when hwmon has none).
    pub temperatures: Vec<TemperatureReading>,
    /// CPU load in percent since the previous call of any CPU-reading operation.
    pub cpu_load_pct: Option<f64>,
    /// The highest current core clock in MHz.
    pub cpu_freq_mhz: Option<u32>,
    /// Memory in use as a percentage.
    pub memory_used_pct: Option<f64>,
    /// Swap in use as a percentage.
    pub swap_used_pct: Option<f64>,
    /// Whether the discrete GPU is awake (`active`), asleep (`suspended`), absent or unknown.
    pub gpu_state: GpuPower,
    /// The interface in use.
    pub net_primary: Option<String>,
    /// Receive rate of the physical interfaces, bytes per second.
    pub net_rx_bps: Option<f64>,
    /// Send rate, bytes per second.
    pub net_tx_bps: Option<f64>,
    /// Seconds since boot.
    pub uptime_s: Option<f64>,
}

/// One CPU core on the CPU tab.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MonitorCore {
    /// The kernel's number of the core.
    pub id: usize,
    /// Busy percentage since the previous call.
    pub load_pct: Option<f64>,
    /// Current clock in MHz.
    pub freq_mhz: Option<u32>,
    /// Highest clock in MHz.
    pub max_freq_mhz: Option<u32>,
}

/// The CPU tab.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MonitorCpu {
    /// Seconds since the epoch.
    pub t: i64,
    /// All cores together, percent.
    pub load_pct: Option<f64>,
    /// Each online core.
    pub cores: Vec<MonitorCore>,
    /// Average of the current core clocks in MHz.
    pub freq_avg_mhz: Option<u32>,
    /// The highest current core clock in MHz.
    pub freq_max_mhz: Option<u32>,
    /// The scaling governor (`powersave`).
    pub governor: Option<String>,
    /// Load average.
    pub load_avg: Option<LoadAverage>,
    /// CPU package power in watts; `null` for an ordinary user on current kernels (RAPL is root-only).
    pub power_w: Option<f64>,
    /// The package temperature in degrees C.
    pub temperature_c: Option<f64>,
    /// The busiest processes, highest first.
    pub top: Vec<ProcessInfo>,
    /// False for the first scan of a session (no process percentage can exist yet).
    pub top_valid: bool,
    /// How many processes were looked at.
    pub processes: usize,
}

/// The memory tab.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MonitorMemory {
    /// Seconds since the epoch.
    pub t: i64,
    /// RAM and swap.
    pub memory: Option<MemoryUsage>,
    /// The largest processes by resident memory, biggest first.
    pub top: Vec<ProcessInfo>,
    /// How many processes were looked at.
    pub processes: usize,
}

/// The network tab.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MonitorNetwork {
    /// Seconds since the epoch.
    pub t: i64,
    /// Rates, peaks, totals and the interface in use.
    #[serde(flatten)]
    pub sample: NetSample,
    /// IPv4 addresses by interface name.
    pub addresses: BTreeMap<String, Vec<String>>,
    /// The first address of the interface in use.
    pub primary_address: Option<String>,
}

/// The disks tab.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MonitorDisks {
    /// Seconds since the epoch.
    pub t: i64,
    /// Mounted filesystems.
    pub filesystems: Vec<DiskUsage>,
    /// Physical drives.
    pub drives: Vec<BlockDevice>,
    /// NVMe temperatures.
    pub nvme: Vec<NvmeTemp>,
}

/// Where the GPU tab's live numbers came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuSource {
    /// A one-shot `nvidia-smi` query (possibly cached: see `detail_cached`).
    NvidiaSmi,
    /// The service's live values.
    Daemon,
    /// The firmware's temperature sensor only.
    Firmware,
    /// Nothing could be read.
    None,
}

/// The GPU tab.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MonitorGpu {
    /// Seconds since the epoch.
    pub t: i64,
    /// `active`, `suspended` (asleep: nothing was asked of it), `absent` or `unknown`.
    pub state: GpuPower,
    /// A sentence for the page when there is something to explain.
    pub note: Option<String>,
    /// Marketing name, when known.
    pub name: Option<String>,
    /// Driver version, when known.
    pub driver: Option<String>,
    /// Temperature in degrees C (the firmware's sensor, which never wakes the card).
    pub temperature_c: Option<f64>,
    /// Utilisation in percent.
    pub utilization_pct: Option<f64>,
    /// Power draw in watts.
    pub power_w: Option<f64>,
    /// Where `utilization_pct` and `power_w` came from.
    pub source: GpuSource,
    /// Video memory, clocks, P-state, PCIe link, limits; present only for a `detail` request
    /// while the card is awake.
    pub detail: Option<GpuMetrics>,
    /// How old the detail is, in seconds.
    pub detail_age_s: Option<f64>,
    /// True when the detail is the previous query's answer, because a new one is not allowed yet.
    pub detail_cached: bool,
    /// Seconds until a new query is allowed (0: the next call may make one).
    pub next_detail_in_s: f64,
    /// Why the last query failed.
    pub error: Option<String>,
}

fn average(values: impl Iterator<Item = u32>) -> Option<u32> {
    let (sum, n) = values.fold((0u64, 0u64), |(s, n), v| (s + u64::from(v), n + 1));
    (n > 0).then(|| u32::try_from(sum / n).unwrap_or(u32::MAX))
}

impl Api {
    fn monitor_now(&self) -> f64 {
        (self.s.now)()
    }

    /// Gives a new sampler its baseline: the first answer of a session would otherwise be all `null`.
    fn prime(sleep: &Sleeper) {
        sleep(PRIME_DELAY);
    }

    /// Replaces the program runner used for `nvidia-smi`, `df` and `ip` (the assembly layer passes
    /// the machine's real one; tests pass a script). Without it, programs run only when the
    /// services' root is the real `/`.
    #[must_use]
    pub fn with_command_runner(mut self, runner: Arc<dyn CommandRunner>) -> Self {
        self.monitor = self.monitor.with_runner(runner);
        self
    }

    /// The CPU load since the previous sample, reusing an answer younger than half a second.
    fn cpu_load(&self, now: f64) -> CpuLoad {
        let mut state = locked(&self.monitor.cpu);
        if let Some(load) = state.fresh(now) {
            return load.clone();
        }
        if !state.primed {
            state.primed = true;
            state.sampler.sample();
            Self::prime(&self.s.sleep);
        }
        let load = state.sampler.sample();
        state.last = Some((now, load.clone()));
        load
    }

    /// The process rankings from the shared scan.
    fn process_scan(&self, now: f64, top: usize) -> (Vec<ProcessInfo>, Vec<ProcessInfo>, bool, usize) {
        let top = top.clamp(1, MAX_TOP);
        let mut sampler = locked(&self.monitor.procs);
        if !sampler.has_baseline() {
            sampler.rescan(now);
            Self::prime(&self.s.sleep);
            let later = (self.s.now)();
            sampler.rescan(later.max(now));
        }
        let snapshot = sampler.sample(self.monitor_now());
        let cut = |list: &[ProcessInfo]| list.iter().take(top).cloned().collect::<Vec<_>>();
        (cut(&snapshot.by_cpu), cut(&snapshot.by_mem), snapshot.cpu_valid, snapshot.scanned)
    }

    /// The network sample, reusing an answer younger than half a second.
    fn net_sample(&self, now: f64) -> NetSample {
        let mut state = locked(&self.monitor.net);
        if let Some(sample) = state.fresh(now) {
            return sample.clone();
        }
        if !state.primed {
            state.primed = true;
            state.sampler.sample(now);
            Self::prime(&self.s.sleep);
        }
        let sample = state.sampler.sample((self.s.now)().max(now));
        state.last = Some((now, sample.clone()));
        sample
    }

    /// The firmware's temperatures, from the daemon's live values when it runs (they exist
    /// already) and from the firmware otherwise: (cpu, board, gpu), each `None` when 0 or unread.
    fn firmware_temps(&self) -> (Option<f64>, Option<f64>, Option<f64>) {
        let positive = |v: i64| (v > 0).then_some(v as f64);
        match self.s.hw.sensors() {
            Ok(s) => (positive(s.cpu_temp), positive(s.sys_temp), positive(s.gpu_temp)),
            Err(_) => {
                let state = if self.daemon_available() { self.daemon_state().ok() } else { None };
                let live = |k: &str| state.as_ref().and_then(|d| d.live_f64(k)).filter(|v| *v > 0.0);
                (live("tcpu"), live("chassis"), live("tgpu"))
            }
        }
    }

    /// Adds the lowest and highest values seen and turns hwmon sensors into readings.
    fn readings(&self, sensors: Vec<TempSensor>) -> Vec<TemperatureReading> {
        let mut tracker = locked(&self.monitor.extremes);
        sensors
            .into_iter()
            .map(|s| {
                let e = tracker.observe(&s.id, s.celsius);
                TemperatureReading {
                    session_min_c: e.map_or(s.celsius, |e| e.min),
                    session_max_c: e.map_or(s.celsius, |e| e.max),
                    id: s.id,
                    chip: s.chip,
                    label: s.label,
                    kind: s.kind,
                    celsius: s.celsius,
                    high_c: s.max_c,
                    critical_c: s.crit_c,
                }
            })
            .collect()
    }

    /// Every temperature the machine has: hwmon, plus the firmware's GPU sensor (the proprietary
    /// NVIDIA driver has no hwmon entry) and its board and CPU sensors when hwmon has none.
    fn all_temperatures(&self, gpu: GpuPower) -> Vec<TemperatureReading> {
        let root = &self.monitor.root;
        let mut sensors = temps::read_all(root, ReadOptions { skip_gpu_chips: !gpu.may_query() });
        let (cpu, board, gpu_c) = self.firmware_temps();
        let has =
            |sensors: &[TempSensor], kinds: &[SensorKind]| sensors.iter().any(|s| kinds.contains(&s.kind));
        let firmware = |id: &str, label: &str, kind: SensorKind, celsius: f64| TempSensor {
            id: format!("firmware/{id}"),
            chip: "firmware".to_owned(),
            label: label.to_owned(),
            kind,
            celsius,
            max_c: None,
            crit_c: None,
        };
        if !has(&sensors, &[SensorKind::Board]) {
            let zone = temps::read_thermal_zone(root, "acpitz");
            sensors
                .extend(zone.or_else(|| board.map(|c| firmware("system", "System", SensorKind::Board, c))));
        }
        if !has(&sensors, &[SensorKind::CpuPackage]) {
            sensors.extend(cpu.map(|c| firmware("cpu", "CPU", SensorKind::CpuPackage, c)));
        }
        if !has(&sensors, &[SensorKind::Gpu]) {
            sensors.extend(gpu_c.map(|c| firmware("gpu", "GPU", SensorKind::Gpu, c)));
        }
        self.readings(sensors)
    }

    /// The Monitor page's overview. Cheap; call it every two seconds at most.
    pub fn monitor_overview(&self) -> MonitorOverview {
        let now = self.monitor_now();
        let gpu_state = nvidia::gpu_power(&self.monitor.root);
        let cpu = self.cpu_load(now);
        let net = self.net_sample(now);
        let memory = read_meminfo(&self.monitor.root).map(|m| MemoryUsage::from_info(&m));
        MonitorOverview {
            t: now as i64,
            temperatures: self.all_temperatures(gpu_state),
            cpu_load_pct: cpu.total_pct,
            cpu_freq_mhz: read_freqs(&self.monitor.root).iter().filter_map(|f| f.cur_mhz).max(),
            memory_used_pct: memory.map(|m| m.used_pct),
            swap_used_pct: memory.filter(|m| m.swap_total_bytes > 0).map(|m| m.swap_used_pct),
            gpu_state,
            net_primary: net.primary,
            net_rx_bps: net.rx_bps,
            net_tx_bps: net.tx_bps,
            uptime_s: sysinfo::read_uptime(&self.monitor.root),
        }
    }

    /// Forgets the temperature minimums and maximums and the network peaks and session totals.
    pub fn monitor_reset(&self) {
        locked(&self.monitor.extremes).reset();
        locked(&self.monitor.net).sampler.reset();
    }

    /// The CPU tab with the `top` busiest processes (at most [`MAX_TOP`]).
    pub fn monitor_cpu(&self, top: usize) -> MonitorCpu {
        let now = self.monitor_now();
        let root = &self.monitor.root;
        let load = self.cpu_load(now);
        let freqs = read_freqs(root);
        let mut ids: Vec<usize> = load.cores.iter().map(|c| c.id).chain(freqs.iter().map(|f| f.id)).collect();
        ids.sort_unstable();
        ids.dedup();
        let cores = ids
            .into_iter()
            .map(|id| {
                let freq = freqs.iter().find(|f| f.id == id);
                MonitorCore {
                    id,
                    load_pct: load.cores.iter().find(|c| c.id == id).and_then(|c| c.load_pct),
                    freq_mhz: freq.and_then(|f| f.cur_mhz),
                    max_freq_mhz: freq.and_then(|f| f.max_mhz),
                }
            })
            .collect();
        let (by_cpu, _, top_valid, processes) = self.process_scan(now, top);
        MonitorCpu {
            t: now as i64,
            load_pct: load.total_pct,
            cores,
            freq_avg_mhz: average(freqs.iter().filter_map(|f| f.cur_mhz)),
            freq_max_mhz: freqs.iter().filter_map(|f| f.cur_mhz).max(),
            governor: read_governor(root),
            load_avg: read_loadavg(root),
            power_w: locked(&self.monitor.rapl).watts(now),
            temperature_c: self.s.linux.cpu_temp(),
            top: by_cpu,
            top_valid,
            processes,
        }
    }

    /// The memory tab with the `top` biggest processes (at most [`MAX_TOP`]).
    pub fn monitor_memory(&self, top: usize) -> MonitorMemory {
        let now = self.monitor_now();
        let (_, by_mem, _, processes) = self.process_scan(now, top);
        MonitorMemory {
            t: now as i64,
            memory: read_meminfo(&self.monitor.root).map(|m| MemoryUsage::from_info(&m)),
            top: by_mem,
            processes,
        }
    }

    /// The IPv4 addresses by interface, from `ip`, reused for ten seconds.
    fn local_addresses(&self, now: f64) -> BTreeMap<String, Vec<String>> {
        let mut cache = locked(&self.monitor.addresses);
        if let Some((at, addresses)) = cache.as_ref()
            && now >= *at
            && now - *at < IP_CACHE_SECS
        {
            return addresses.clone();
        }
        let out =
            self.monitor.runner.output("ip", &["-o", "-4", "addr", "show"], IP_TIMEOUT).unwrap_or_default();
        let addresses = netstat::parse_ip_addr(&String::from_utf8_lossy(&out));
        *cache = Some((now, addresses.clone()));
        addresses
    }

    /// The network tab.
    pub fn monitor_network(&self) -> MonitorNetwork {
        let now = self.monitor_now();
        let sample = self.net_sample(now);
        let addresses = self.local_addresses(now);
        let primary_address =
            sample.primary.as_ref().and_then(|p| addresses.get(p)).and_then(|a| a.first()).cloned();
        MonitorNetwork { t: now as i64, sample, addresses, primary_address }
    }

    /// The disks tab.
    pub fn monitor_disks(&self) -> MonitorDisks {
        let stats = DfStats::new(Arc::clone(&self.monitor.runner));
        let root = &self.monitor.root;
        MonitorDisks {
            t: self.monitor_now() as i64,
            filesystems: disks::list_disks(root, &stats),
            drives: disks::block_devices(root),
            nvme: disks::nvme_temps(root),
        }
    }

    /// One `nvidia-smi` query, at most once per [`nvidia::MIN_QUERY_INTERVAL_SECS`]: the answer and
    /// whether it is the previous one, its age and the seconds until a new query is allowed.
    fn gpu_query(&self, now: f64) -> (Option<GpuMetrics>, Option<String>, bool, f64, f64) {
        let mut cache = locked(&self.monitor.gpu);
        let age = cache.at.filter(|at| now >= *at).map(|at| now - at);
        let wait = age.map_or(0.0, |a| (nvidia::MIN_QUERY_INTERVAL_SECS - a).max(0.0));
        if let (Some(age), true) = (age, wait > 0.0) {
            return (cache.metrics.clone(), cache.error.clone(), true, age, wait);
        }
        let args = nvidia::query_args();
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let result = self
            .monitor
            .runner
            .output("nvidia-smi", &args, GPU_QUERY_TIMEOUT)
            .map_err(|e| format!("nvidia-smi could not be run: {e}"))
            .and_then(|out| nvidia::parse_query(&String::from_utf8_lossy(&out)).map_err(|e| e.to_string()));
        cache.at = Some(now);
        match result {
            Ok(metrics) => {
                cache.metrics = Some(metrics);
                cache.error = None;
            }
            Err(e) => {
                cache.metrics = None;
                cache.error = Some(e);
            }
        }
        (cache.metrics.clone(), cache.error.clone(), false, 0.0, nvidia::MIN_QUERY_INTERVAL_SECS)
    }

    /// The GPU tab. With `detail` false it starts no process and never wakes the card; with
    /// `detail` true it makes (at most one per five seconds) a one-shot `nvidia-smi` query, and
    /// only while the card is awake. See the module documentation.
    pub fn monitor_gpu(&self, detail: bool) -> MonitorGpu {
        let now = self.monitor_now();
        let state = nvidia::gpu_power(&self.monitor.root);
        let (_, _, firmware_temp) = self.firmware_temps();
        let (mut name, mut driver) = (None, None);
        let mut report = MonitorGpu {
            t: now as i64,
            state,
            note: None,
            name: None,
            driver: None,
            temperature_c: firmware_temp,
            utilization_pct: None,
            power_w: None,
            source: if firmware_temp.is_some() { GpuSource::Firmware } else { GpuSource::None },
            detail: None,
            detail_age_s: None,
            detail_cached: false,
            next_detail_in_s: 0.0,
            error: None,
        };
        if let Some(info) = locked(&self.monitor.facts).as_ref()
            && let Some(g) = info.gpus.iter().find(|g| g.name.is_some())
        {
            name = g.name.clone();
            driver = g.driver.clone();
        }
        match state {
            GpuPower::Absent => report.note = Some("this laptop has no NVIDIA GPU".to_owned()),
            GpuPower::Suspended => {
                report.note = Some(
                    "the GPU is asleep to save power; it was not woken to read it. It wakes when a program uses it."
                        .to_owned(),
                );
            }
            GpuPower::Active | GpuPower::Unknown => {
                // The service's own readings, when it has any (they are 0 without its stream).
                let daemon = if self.daemon_available() { self.daemon_state().ok() } else { None };
                let live = |k: &str| daemon.as_ref().and_then(|d| d.live_f64(k));
                let (util, power) = (live("gpu").unwrap_or(0.0), live("gpuw").unwrap_or(0.0));
                if util > 0.0 || power > 0.0 {
                    report.utilization_pct = Some(util);
                    report.power_w = Some(power);
                    report.source = GpuSource::Daemon;
                }
                if detail {
                    let (metrics, error, cached, age, wait) = self.gpu_query(now);
                    report.error = error;
                    report.detail_cached = cached;
                    report.next_detail_in_s = wait;
                    if let Some(m) = metrics {
                        report.detail_age_s = Some(age);
                        report.utilization_pct = m.utilization_pct.or(report.utilization_pct);
                        report.power_w = m.power_w.or(report.power_w);
                        report.temperature_c = m.temperature_c.or(report.temperature_c);
                        report.source = GpuSource::NvidiaSmi;
                        name = m.name.clone().or(name);
                        driver = m.driver.clone().or(driver);
                        report.detail = Some(m);
                    }
                }
            }
        }
        report.name = name;
        report.driver = driver;
        report
    }

    /// The machine's facts: laptop, CPU, memory, GPUs, drives, OS, kernel, uptime. The files are
    /// read once; the NVIDIA card's name and driver are added the first time the card is awake and
    /// the query is allowed (through the same rate-limited query as [`Api::monitor_gpu`]), never
    /// by waking it.
    pub fn system_info(&self) -> SystemInfo {
        let root = &self.monitor.root;
        let cached = locked(&self.monitor.facts).clone();
        let mut base = cached.unwrap_or_else(|| sysinfo::read(root));
        let needs_name = base.gpus.iter().any(|g| g.vendor == "NVIDIA" && g.name.is_none());
        if needs_name && nvidia::gpu_power(root).may_query() {
            let (metrics, ..) = self.gpu_query(self.monitor_now());
            if let Some(m) = metrics
                && let Some(g) = base.gpus.iter_mut().find(|g| g.vendor == "NVIDIA" && g.name.is_none())
            {
                g.name = m.name;
                g.driver = m.driver;
            }
        }
        base.uptime_s = sysinfo::read_uptime(root);
        *locked(&self.monitor.facts) = Some(base.clone());
        base
    }
}

#[cfg(test)]
mod tests;
