//! The laptop of the integration tests: the real daemon on a temporary socket, the real client and
//! the real assembly of the application layer, around one firmware simulator.
//!
//! Only what would touch the machine is replaced: the firmware is the simulator, the sensors,
//! the CPU clock cap and the undervolt registers are fakes, `systemctl` is a script, the sysfs
//! files are a tree in a temporary directory, and time is a clock that only moves when the
//! daemon's sleeper moves it. Everything between the `Api` and the firmware (the assembly of
//! the services, the facades, the socket client, the wire format, the server, the command
//! table, the handlers, the settings store, the decision engines) is the production code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use ts_daemon::daemon::{
    Clock, ClockCap, ConfigFileAlerts, CpuUndervoltPort, Daemon, DaemonError, Deps, GpuUndervoltPort, Level,
    LivePaths, Logger, Notice, PortError, PortResult, ProcessList, Sensors, Sleeper, StopToken,
};
use ts_decision::crashguard::Guard;
use ts_decision::gamerules::Process;
use ts_decision::{curves, smartprofile};
use ts_domain::gpuundervolt::{Applied as GpuApplied, State as GpuState};
use ts_domain::keyboard::{Keyboard, SimHid};
use ts_domain::looks::LookStore;
use ts_domain::power::{HostSysfs, PowerLimits};
use ts_domain::powermgr::PowerManagers;
use ts_domain::undervolt::Offsets;
use ts_foundation::config::Config;
use ts_foundation::sdnotify::Notifier;
use ts_foundation::state::{self, Store};
use ts_foundation::userstate;
use ts_ipc::client::Client;
use ts_platform::firmware::Simulator;
use ts_platform::hw::{CommandRunner, Hw};
use ts_system::doctor::{self, Host};
use ts_system::osd::{UserDirectory, UserEntry};
use ts_system::sensors::GpuReading;
use ts_system::touchpad::Touchpad;

use super::super::Machine;
use super::super::testkit::TempDir;
use crate::api::testing::NullRunner;
use crate::api::{Api, Services};

pub(super) fn locked<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Time that moves only when the daemon's sleeper says so (the wall and the monotonic clock
/// together).
pub(super) struct FakeClock(Mutex<f64>);

impl FakeClock {
    pub(super) fn advance(&self, seconds: f64) {
        *locked(&self.0) += seconds;
    }
}

impl Clock for FakeClock {
    fn wall(&self) -> f64 {
        *locked(&self.0)
    }

    fn mono(&self) -> f64 {
        *locked(&self.0) - 1_800_000_000.0
    }
}

/// Between two ticks it waits a few real milliseconds (so a test's request can slip in between)
/// and moves the fake clock a twentieth of the interval the loop asked for: ticks come about
/// 250 times a second and time runs 12 times faster than the clock on the wall. That is fast
/// enough for the daemon's timers (dwell times, hold, watchdogs counted in seconds) to elapse
/// within a test, and slow enough that the jobs that run every few minutes (the power-manager
/// re-check) never fire in the middle of one.
struct PacedSleeper(Arc<FakeClock>);

impl Sleeper for PacedSleeper {
    fn sleep(&self, limit: Duration, stop: &StopToken) -> bool {
        let stopped = stop.wait_timeout(Duration::from_millis(4));
        self.0.advance(limit.as_secs_f64() / 20.0);
        stopped
    }
}

/// Collects the journal.
#[derive(Default)]
pub(super) struct Journal(Mutex<Vec<String>>);

impl Journal {
    pub(super) fn text(&self) -> String {
        locked(&self.0).join("\n")
    }
}

impl Logger for Journal {
    fn log(&self, _level: Level, message: &str) {
        locked(&self.0).push(message.to_owned());
    }
}

/// Load, battery and charger as a test sets them.
pub(super) struct FakeSensors {
    on_ac: AtomicBool,
    battery: Mutex<f64>,
}

impl FakeSensors {
    pub(super) fn set_ac(&self, on: bool) {
        self.on_ac.store(on, Ordering::SeqCst);
    }
}

impl Sensors for FakeSensors {
    fn cpu_load(&self) -> f64 {
        0.0
    }

    fn package_temp(&self) -> Option<f64> {
        None
    }

    fn battery_pct(&self) -> f64 {
        *locked(&self.battery)
    }

    fn gpu(&self) -> GpuReading {
        GpuReading { temp: None, util: 0.0, power: 0.0, seen: None }
    }

    fn on_ac(&self) -> bool {
        self.on_ac.load(Ordering::SeqCst)
    }

    fn ensure_gpu_stream(&self) {}
}

struct NoCap;

impl ClockCap for NoCap {
    fn apply(&self, _khz: i64) -> Vec<String> {
        Vec::new()
    }

    fn uncap(&self) -> Vec<String> {
        Vec::new()
    }
}

/// The CPU undervolt registers as two numbers.
#[derive(Default)]
pub(super) struct FakeCpuUv {
    pub(super) offsets: Mutex<(i32, i32)>,
    /// Makes the next apply fail like a BIOS that locks undervolting.
    pub(super) locked: AtomicBool,
}

impl CpuUndervoltPort for FakeCpuUv {
    fn read(&self) -> PortResult<Offsets> {
        let (core_mv, cache_mv) = *locked(&self.offsets);
        Ok(Offsets { core_mv, cache_mv })
    }

    fn apply(&self, core_mv: i32, cache_mv: i32, _confirm: bool) -> PortResult<Offsets> {
        if self.locked.load(Ordering::SeqCst) {
            return Err(PortError::new("readback 0 mV != -50 mV (BIOS lock?); reset with: uv reset"));
        }
        *locked(&self.offsets) = (core_mv, cache_mv);
        Ok(Offsets { core_mv, cache_mv })
    }

    fn reset(&self) -> PortResult<Offsets> {
        *locked(&self.offsets) = (0, 0);
        Ok(Offsets::default())
    }
}

/// The GPU's curve offset and cap.
#[derive(Default)]
pub(super) struct FakeGpuUv(pub(super) Mutex<(i32, i32)>);

impl GpuUndervoltPort for FakeGpuUv {
    fn read(&self) -> PortResult<GpuState> {
        Ok(GpuState { offset_mhz: locked(&self.0).0, range_mhz: [-300, 250] })
    }

    fn apply(&self, offset_mhz: i32, cap_mhz: i32) -> PortResult<GpuApplied> {
        *locked(&self.0) = (offset_mhz, cap_mhz);
        Ok(GpuApplied { offset_mhz, cap_mhz })
    }

    fn reset(&self) -> PortResult<GpuApplied> {
        *locked(&self.0) = (0, 0);
        Ok(GpuApplied { offset_mhz: 0, cap_mhz: 0 })
    }
}

struct NoOsd;

impl Notice for NoOsd {
    fn show(&self, _text: &str, _icon: &str) {}
}

/// The programs running now, as a test sets them.
#[derive(Default)]
pub(super) struct FakeProcs(Mutex<Vec<Process>>);

impl FakeProcs {
    pub(super) fn run(&self, name: &str) {
        *locked(&self.0) = vec![Process {
            comm: format!("{name}\n").into_bytes(),
            cmdline: format!("{name}\0").into_bytes(),
        }];
    }
}

impl ProcessList for FakeProcs {
    fn list(&self) -> Vec<Process> {
        locked(&self.0).clone()
    }
}

/// The desktop user of the tests: the user running them.
struct TestUsers {
    uid: u32,
    home: PathBuf,
}

impl UserDirectory for TestUsers {
    fn lookup(&self, name: &str) -> Option<UserEntry> {
        (name == "tester").then(|| UserEntry { uid: self.uid, home: self.home.display().to_string() })
    }
}

/// A `systemctl` that knows which of the other power managers run: `disable --now` stops one.
#[derive(Default)]
pub(super) struct FakeSystemctl {
    active: Mutex<BTreeSet<String>>,
    enabled: Mutex<BTreeSet<String>>,
    pub(super) calls: Mutex<Vec<String>>,
}

impl FakeSystemctl {
    pub(super) fn install(&self, unit: &str) {
        locked(&self.active).insert(unit.to_owned());
        locked(&self.enabled).insert(unit.to_owned());
    }
}

impl CommandRunner for FakeSystemctl {
    fn output(&self, program: &str, args: &[&str], _timeout: Duration) -> io::Result<Vec<u8>> {
        if program != "systemctl" {
            return Err(io::Error::new(io::ErrorKind::NotFound, format!("{program}: not started in tests")));
        }
        locked(&self.calls).push(args.join(" "));
        let unit =
            |i: usize| args.get(i).map_or(String::new(), |u| u.trim_end_matches(".service").to_owned());
        let answer = |yes: bool, no: &str, ok: &str| if yes { ok.to_owned() } else { no.to_owned() };
        let text = match args.first().copied() {
            Some("is-active") => answer(locked(&self.active).contains(&unit(1)), "inactive", "active"),
            Some("is-enabled") => answer(locked(&self.enabled).contains(&unit(1)), "disabled", "enabled"),
            Some("disable") => {
                locked(&self.active).remove(&unit(2));
                locked(&self.enabled).remove(&unit(2));
                String::new()
            }
            Some("enable") => {
                locked(&self.enabled).insert(unit(1));
                String::new()
            }
            Some("start") => {
                locked(&self.active).insert(unit(1));
                String::new()
            }
            _ => String::new(),
        };
        Ok(text.into_bytes())
    }
}

/// A system where only `truesense.service` is running (the health check asks systemd first and
/// the socket second).
struct OnlyTheService;

impl ts_system::services::Runner for OnlyTheService {
    fn run(&self, program: &str, args: &[&str]) -> ts_system::services::Output {
        if program == "systemctl" && args == ["is-active", "truesense.service"] {
            return ts_system::services::Output::success("active\n");
        }
        ts_system::services::Output::not_runnable(format!("{program}: not started in tests"))
    }
}

/// An operating system with nobody logged in and nothing installed.
struct EmptyHost;

impl Host for EmptyHost {
    fn env_var(&self, _name: &str) -> Option<String> {
        None
    }

    fn euid(&self) -> u32 {
        1000
    }

    fn lookup_uid(&self, _user: &str) -> Option<u32> {
        None
    }

    fn program_on_path(&self, _name: &str) -> bool {
        false
    }

    fn can_access(&self, _path: &Path, _write: bool) -> bool {
        false
    }

    fn now(&self) -> std::time::SystemTime {
        std::time::UNIX_EPOCH
    }
}

/// What differs between the laptops of the tests.
#[derive(Default)]
pub(super) struct Setup {
    /// Settings over the ones every test starts with (`smart_fan` off, no automatic mode).
    pub(super) settings: Vec<(&'static str, Value)>,
    /// The other power managers installed and running at the start.
    pub(super) managers: Vec<&'static str>,
    /// The configured desktop user, whose uid may use the socket (default `tester`, the user
    /// running the tests).
    pub(super) user: Option<&'static str>,
}

/// The whole fake laptop.
pub(super) struct Rig {
    pub(super) dir: TempDir,
    pub(super) root: PathBuf,
    pub(super) home: PathBuf,
    pub(super) sock: PathBuf,
    pub(super) sim: Arc<Simulator>,
    pub(super) daemon: Arc<Daemon>,
    /// The application layer as a client builds it: `Services::assemble` over the machine below.
    pub(super) api: Api,
    /// A bare socket client, for the tests that look at the wire.
    pub(super) client: Client,
    pub(super) machine: Machine,
    pub(super) sensors: Arc<FakeSensors>,
    pub(super) uv: Arc<FakeCpuUv>,
    pub(super) gpu_uv: Arc<FakeGpuUv>,
    pub(super) procs: Arc<FakeProcs>,
    pub(super) systemctl: Arc<FakeSystemctl>,
    pub(super) state: Arc<Store>,
    /// The daemon's journal lines.
    pub(super) journal: Arc<Journal>,
    stop: StopToken,
    thread: Mutex<Option<JoinHandle<Result<(), DaemonError>>>>,
}

fn my_uid(dir: &Path) -> u32 {
    fs::metadata(dir).unwrap().uid()
}

/// A file with its folders.
pub(super) fn put(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

impl Rig {
    pub(super) fn new() -> Self {
        Self::with(Setup::default())
    }

    pub(super) fn with(setup: Setup) -> Self {
        let dir = TempDir::new();
        let root = dir.0.join("root");
        let home = dir.0.join("home");
        let run = dir.0.join("run");
        for d in [&root, &home, &run] {
            fs::create_dir_all(d).unwrap();
        }
        let sock = run.join("truesense.sock");

        // The sysfs files the power code reads and writes, in the tree under `root`.
        let rapl = root.join("sys/class/powercap/intel-rapl:0");
        put(&rapl.join("constraint_0_power_limit_uw"), "80000000");
        put(&rapl.join("constraint_1_power_limit_uw"), "100000000");
        put(&rapl.join("enabled"), "1");
        let cpu = root.join("sys/devices/system/cpu");
        put(&cpu.join("intel_pstate/no_turbo"), "1");
        for n in ["cpu0", "cpu1"] {
            put(&cpu.join(n).join("cpufreq/scaling_governor"), "performance");
            put(&cpu.join(n).join("cpufreq/energy_performance_preference"), "performance");
            put(&cpu.join(n).join("cpufreq/scaling_max_freq"), "3000000");
            put(&cpu.join(n).join("cpufreq/cpuinfo_max_freq"), "5100000");
        }
        put(
            &root.join("proc/bus/input/devices"),
            "N: Name=\"SYNA2BA6:00 06CB:CD7E Touchpad\"\nH: Handlers=mouse1 event13 \n",
        );
        put(&root.join("sys/class/input/event13/device/inhibited"), "0");

        let sim = Arc::new(Simulator::new());
        let no_sleep: ts_platform::hw::Sleeper = Arc::new(|_| {});
        let hw = Hw::new(sim.clone()).with_sleeper(Arc::clone(&no_sleep));
        let state_defaults = {
            let mut m: Map<String, Value> = state::defaults(&Config::default());
            for (k, v) in [
                ("smart_fan", Value::Bool(false)),
                ("auto_profile", Value::Bool(false)),
                ("auto_mode", Value::from("off")),
                ("operating_follow", Value::Bool(false)),
            ] {
                m.insert(k.to_owned(), v);
            }
            for (k, v) in &setup.settings {
                m.insert((*k).to_owned(), v.clone());
            }
            m
        };
        let state = Arc::new(Store::open(dir.0.join("state.json"), state_defaults));
        let clock = Arc::new(FakeClock(Mutex::new(1_800_000_000.0)));
        let sensors = Arc::new(FakeSensors { on_ac: AtomicBool::new(true), battery: Mutex::new(80.0) });
        let uv = Arc::new(FakeCpuUv::default());
        let gpu_uv = Arc::new(FakeGpuUv::default());
        let procs = Arc::new(FakeProcs::default());
        let systemctl = Arc::new(FakeSystemctl::default());
        for unit in &setup.managers {
            systemctl.install(unit);
        }
        let journal = Arc::new(Journal::default());
        let stop = StopToken::new();

        // The daemon's own keyboard and look store (it applies the boot look at start).
        let daemon_user = Arc::new(userstate::Store::new(dir.0.join("daemon-user.json")));
        let (daemon_keyboard, _daemon_hid) = Keyboard::simulated(Arc::clone(&daemon_user));
        let daemon_keyboard = Arc::new(daemon_keyboard);
        let looks = LookStore::new(
            dir.0.join("daemon-looks"),
            dir.0.join("boot-look.json"),
            daemon_user,
            daemon_keyboard.clone(),
            Arc::new(hw.clone()),
        );
        let sys = Arc::new(HostSysfs::new(root.clone()));
        let pm = PowerManagers::new(
            systemctl.clone(),
            sys.clone(),
            PathBuf::from(ts_domain::powermgr::DEFAULT_CPU_DIR),
            dir.0.join("pm-record.json"),
        );
        let config = Config { user: setup.user.unwrap_or("tester").to_owned(), ..Config::default() };
        let deps = Deps {
            config,
            state: Arc::clone(&state),
            hw: hw.clone(),
            sensors: sensors.clone(),
            cpu_cap: Arc::new(NoCap),
            power: Arc::new(PowerLimits::new(sys, None, Arc::new(NullRunner))),
            cpu_undervolt: uv.clone(),
            gpu_undervolt: gpu_uv.clone(),
            power_managers: Arc::new(pm),
            looks: Arc::new(looks),
            keyboard: daemon_keyboard,
            touchpad: Arc::new(Touchpad::new(
                root.join("proc/bus/input/devices"),
                root.join("sys/class/input"),
            )),
            osd: Arc::new(NoOsd),
            alert_settings: Arc::new(ConfigFileAlerts::new(None, None)),
            processes: procs.clone(),
            users: Arc::new(TestUsers { uid: my_uid(&dir.0), home: home.clone() }),
            curves: curves::Store::new(dir.0.join("daemon-curves")),
            smart_profiles: smartprofile::Store::new(dir.0.join("daemon-smart")),
            uv_guard: Guard::new(dir.0.join("var/uv-pending")),
            gpu_uv_guard: Guard::new(dir.0.join("var/gpu-uv-pending")),
            live_paths: LivePaths {
                run_state: run.join("state"),
                tmp_state: dir.0.join("tmp-state"),
                predator_state: dir.0.join("predator-state"),
            },
            notifier: Notifier::disabled(),
            watchdog_every: None,
            clock: clock.clone(),
            sleeper: Arc::new(PacedSleeper(clock.clone())),
            log: journal.clone(),
            stop: stop.clone(),
        };
        let daemon = Daemon::new(deps);
        let runner = Arc::clone(&daemon);
        let path = sock.clone();
        let thread =
            thread::Builder::new().name("test-daemon".to_owned()).spawn(move || runner.run(path)).unwrap();

        // The client side: the real assembly over the same firmware and the same socket.
        let env: BTreeMap<&str, String> = BTreeMap::from([
            ("HOME", home.display().to_string()),
            ("TRUESENSE_SOCKET", sock.display().to_string()),
            ("TRUESENSE_USERSTATE", dir.0.join("user.json").display().to_string()),
            ("TRUESENSE_OSD", "none".to_owned()),
            ("TRUESENSE_PM_RECORD", dir.0.join("client-pm-record.json").display().to_string()),
            ("TRUESENSE_PROFILES_DIR", dir.0.join("shipped-profiles").display().to_string()),
        ]);
        let doctor_root = root.clone();
        let machine = Machine {
            env: Arc::new(move |k| env.get(k).cloned()),
            root: root.clone(),
            euid: my_uid(&dir.0),
            device: Some(sim.clone()),
            hid: Some(Arc::new(SimHid::new())),
            runner: Arc::new(NullRunner),
            units: Arc::new(NullRunner),
            doctor_env: Arc::new(move |a| {
                let mut env = doctor::Env::system(a.firmware, a.daemon, a.audio, a.probes);
                env.root = doctor_root.clone();
                env.runner = Arc::new(OnlyTheService);
                env.host = Arc::new(EmptyHost);
                env.config = Arc::new(|| Ok(Config::default()));
                env.kernel = "test".to_owned();
                env
            }),
            dts_locator: Arc::new(|| None),
            sleep: no_sleep,
            clock: Arc::new(|| 1_800_000_000.0),
        };
        let services = Services::assemble(&machine).unwrap();
        let client = Client::new(&sock).with_timeout(Duration::from_secs(5));
        let rig = Self {
            dir,
            root,
            home,
            sock,
            sim,
            daemon,
            api: Api::new(services),
            client,
            machine,
            sensors,
            uv,
            gpu_uv,
            procs,
            systemctl,
            state,
            journal,
            stop,
            thread: Mutex::new(Some(thread)),
        };
        rig.wait_for(|| rig.sock.exists(), "the control socket");
        rig.ticks(3);
        rig
    }

    /// Polls until `done` holds, or fails the test after five seconds.
    pub(super) fn wait_for(&self, done: impl Fn() -> bool, what: &str) {
        let start = Instant::now();
        while !done() {
            assert!(start.elapsed() < Duration::from_secs(5), "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(2));
        }
    }

    /// Waits until the daemon has run `n` more control ticks.
    pub(super) fn ticks(&self, n: u64) {
        let target = self.daemon.ticks() + n;
        self.wait_for(|| self.daemon.ticks() >= target, "control ticks");
    }

    /// Sets the temperatures the firmware reports (CPU, system, GPU) and waits until the
    /// daemon's smoothed readings have caught up.
    pub(super) fn heat(&self, cpu: u32, system: u32, gpu: u32) {
        self.sim.set_sensors([cpu, system, gpu], [2400, 2200]);
        self.ticks(25);
    }

    /// Stops the daemon and answers how it ended. The fans are back on the firmware when it did.
    pub(super) fn shutdown(&self) -> Result<(), DaemonError> {
        self.stop.request();
        let thread = locked(&self.thread).take();
        thread.map_or(Ok(()), |t| t.join().expect("the daemon thread panicked"))
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.stop.request();
        if let Some(t) = locked(&self.thread).take() {
            let _ = t.join();
        }
    }
}
