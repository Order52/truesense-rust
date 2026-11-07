//! The fake laptop the daemon tests drive tick by tick: the firmware simulator for the fans, the
//! power mode and the battery limit, hand-made fakes for everything else, a clock that only moves
//! when a test says so, and a sleeper that jumps the clock instead of waiting.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Map, Value};
use ts_decision::crashguard::Guard;
use ts_decision::curves;
use ts_decision::gamerules::Process;
use ts_decision::smartprofile;
use ts_domain::gpuundervolt::{Applied as GpuApplied, State as GpuState};
use ts_domain::looks::Look;
use ts_domain::power::{Applied as PowerApplied, Info as PowerInfo, MsrLock, RawLimits};
use ts_domain::powermgr::{CpuState, RestoreReport, TakeoverReport, UnitStatus};
use ts_domain::undervolt::Offsets;
use ts_foundation::config::{AlertsConfig, Config};
use ts_foundation::sdnotify::Notifier;
use ts_foundation::state::Store;
use ts_ipc::wire::{Args, Request};
use ts_platform::firmware::{Device, FirmwareError, Simulator, Snapshot};
use ts_platform::hw::Hw;
use ts_system::osd::{UserDirectory, UserEntry};
use ts_system::sensors::{GpuFeed, GpuReading};

use super::alerts::AlertSettings;
use super::deps::{
    BootLookPort, Clock, ClockCap, CpuUndervoltPort, Deps, GpuUndervoltPort, Level, LivePaths, Logger,
    Notice, OperatingLight, PortError, PortResult, PowerLimitsPort, PowerManagersPort, ProcessList, Sensors,
    Sleeper, StopToken, TouchpadPort,
};
use super::{COMMANDS, Daemon};

fn locked<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A temporary directory removed when dropped.
pub(super) struct TempDir(pub(super) PathBuf);

impl TempDir {
    pub(super) fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tsd-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub(super) fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The uid of the user running the tests (the owner of a file made now).
pub(super) fn my_uid() -> u32 {
    let probe = TempDir::new();
    fs::metadata(&probe.0).unwrap().uid()
}

/// Time that only passes when a test says so.
pub(super) struct FakeClock {
    times: Mutex<(f64, f64)>,
}

impl FakeClock {
    fn new(wall: f64) -> Self {
        Self { times: Mutex::new((wall, 0.0)) }
    }

    /// Both clocks advance.
    pub(super) fn advance(&self, seconds: f64) {
        let mut t = locked(&self.times);
        t.0 += seconds;
        t.1 += seconds;
    }

    /// The wall clock runs but the monotonic one stands still: a suspended machine.
    pub(super) fn suspend(&self, seconds: f64) {
        locked(&self.times).0 += seconds;
    }

    pub(super) fn wall_now(&self) -> f64 {
        locked(&self.times).0
    }
}

impl Clock for FakeClock {
    fn wall(&self) -> f64 {
        locked(&self.times).0
    }

    fn mono(&self) -> f64 {
        locked(&self.times).1
    }
}

/// Jumps the fake clock instead of waiting; can ask for a stop after a number of sleeps.
pub(super) struct FakeSleeper {
    clock: Arc<FakeClock>,
    pub(super) calls: AtomicUsize,
    stop_after: AtomicUsize,
    pub(super) last: Mutex<Duration>,
    hook: Mutex<Option<Hook>>,
}

/// What a test does on the n-th sleep (n counts from 1).
type Hook = Arc<dyn Fn(usize) + Send + Sync>;

impl FakeSleeper {
    /// Runs `f` on every sleep, with the number of the sleep.
    pub(super) fn on_sleep(&self, f: impl Fn(usize) + Send + Sync + 'static) {
        *locked(&self.hook) = Some(Arc::new(f));
    }

    /// Requests the stop on the n-th sleep.
    pub(super) fn stop_after(&self, n: usize) {
        self.stop_after.store(n, Ordering::SeqCst);
    }
}

impl Sleeper for FakeSleeper {
    fn sleep(&self, limit: Duration, stop: &StopToken) -> bool {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        *locked(&self.last) = limit;
        let hook = locked(&self.hook).clone();
        if let Some(hook) = hook {
            hook(n);
        }
        self.clock.advance(limit.as_secs_f64());
        if self.stop_after.load(Ordering::SeqCst) == n {
            stop.request();
        }
        stop.is_requested()
    }
}

/// Collects the journal.
#[derive(Default)]
pub(super) struct TestLog {
    pub(super) lines: Mutex<Vec<String>>,
}

impl TestLog {
    pub(super) fn all(&self) -> String {
        locked(&self.lines).join("\n")
    }

    pub(super) fn has(&self, needle: &str) -> bool {
        self.all().contains(needle)
    }
}

impl Logger for TestLog {
    fn log(&self, _level: Level, message: &str) {
        locked(&self.lines).push(message.to_owned());
    }
}

/// The firmware as the daemon's `Hw` sees it, which a test can make die.
pub(super) struct FaultDevice {
    pub(super) sim: Arc<Simulator>,
    pub(super) dead: AtomicBool,
    /// The firmware answers yes to a manual fan command but its duty registers do not move to a
    /// stop (what the hardware layer calls a refused stop).
    pub(super) stuck_stop: AtomicBool,
    /// Only the sensor reads fail (the firmware still takes commands): a device that has stopped
    /// answering the daemon's reads.
    pub(super) sensors_dead: AtomicBool,
    /// Every command the firmware was sent, in order.
    pub(super) sent: Mutex<Vec<String>>,
}

impl FaultDevice {
    /// How many commands starting with `prefix` were sent.
    pub(super) fn count(&self, prefix: &str) -> usize {
        locked(&self.sent).iter().filter(|c| c.starts_with(prefix)).count()
    }
}

impl Device for FaultDevice {
    fn run(&self, command: &str) -> Result<String, FirmwareError> {
        locked(&self.sent).push(command.to_owned());
        if self.sensors_dead.load(Ordering::SeqCst) && command.starts_with("sensors") {
            return Err(FirmwareError::BadReply { detail: "sensors do not answer".to_owned() });
        }
        if self.dead.load(Ordering::SeqCst) {
            return Err(FirmwareError::BadReply { detail: "no such device".to_owned() });
        }
        if self.stuck_stop.load(Ordering::SeqCst)
            && command.starts_with("set_fan_manual")
            && command.contains(" 0")
        {
            self.sim.run("set_fan_manual 30 30")?;
            return Ok("ok\n".to_owned());
        }
        self.sim.run(command)
    }
}

#[derive(Clone)]
struct SensorState {
    load: f64,
    pkg: Option<f64>,
    battery: f64,
    gpu: GpuReading,
    ac: bool,
    explode: bool,
    gpu_feed: GpuFeed,
}

/// Load, battery, charger and GPU as a test sets them.
pub(super) struct FakeSensors {
    state: Mutex<SensorState>,
    pub(super) gpu_streams: AtomicUsize,
    pub(super) gpu_releases: AtomicUsize,
}

impl FakeSensors {
    fn new() -> Self {
        Self {
            state: Mutex::new(SensorState {
                load: 0.0,
                pkg: None,
                battery: 80.0,
                gpu: GpuReading { temp: None, util: 0.0, power: 0.0, seen: None },
                ac: true,
                explode: false,
                gpu_feed: GpuFeed::Unsampled,
            }),
            gpu_streams: AtomicUsize::new(0),
            gpu_releases: AtomicUsize::new(0),
        }
    }

    pub(super) fn set_load(&self, v: f64) {
        locked(&self.state).load = v;
    }

    pub(super) fn set_pkg(&self, v: Option<f64>) {
        locked(&self.state).pkg = v;
    }

    pub(super) fn set_battery(&self, v: f64) {
        locked(&self.state).battery = v;
    }

    pub(super) fn set_ac(&self, v: bool) {
        locked(&self.state).ac = v;
    }

    pub(super) fn set_gpu(&self, temp: Option<f64>, util: f64, power: f64, seen: Option<f64>) {
        locked(&self.state).gpu = GpuReading { temp, util, power, seen };
    }

    pub(super) fn set_gpu_feed(&self, feed: GpuFeed) {
        locked(&self.state).gpu_feed = feed;
    }

    pub(super) fn explode(&self, on: bool) {
        locked(&self.state).explode = on;
    }
}

impl Sensors for FakeSensors {
    fn cpu_load(&self) -> f64 {
        let s = locked(&self.state).clone();
        assert!(!s.explode, "sensor exploded");
        s.load
    }

    fn package_temp(&self) -> Option<f64> {
        locked(&self.state).pkg
    }

    fn battery_pct(&self) -> f64 {
        locked(&self.state).battery
    }

    fn gpu(&self) -> GpuReading {
        locked(&self.state).gpu
    }

    fn on_ac(&self) -> bool {
        locked(&self.state).ac
    }

    fn ensure_gpu_stream(&self) {
        self.gpu_streams.fetch_add(1, Ordering::SeqCst);
    }

    fn release_gpu_stream(&self) {
        self.gpu_releases.fetch_add(1, Ordering::SeqCst);
    }

    fn gpu_state(&self) -> GpuFeed {
        locked(&self.state).gpu_feed
    }
}

#[derive(Default)]
pub(super) struct FakeCap {
    pub(super) caps: Mutex<Vec<i64>>,
    pub(super) uncapped: AtomicUsize,
}

impl ClockCap for FakeCap {
    fn apply(&self, khz: i64) -> Vec<String> {
        locked(&self.caps).push(khz);
        Vec::new()
    }

    fn uncap(&self) -> Vec<String> {
        self.uncapped.fetch_add(1, Ordering::SeqCst);
        Vec::new()
    }
}

/// RAPL as plain numbers.
pub(super) struct FakePower {
    pub(super) raw: Mutex<RawLimits>,
    pub(super) writes: Mutex<Vec<RawLimits>>,
}

impl PowerLimitsPort for FakePower {
    fn info(&self) -> PowerInfo {
        let raw = *locked(&self.raw);
        PowerInfo {
            pl1_w: Some(raw.pl1_uw as f64 / 1e6),
            pl2_w: Some(raw.pl2_uw as f64 / 1e6),
            rapl_enabled: Some("1".to_owned()),
            msr_lock_bit: MsrLock::Locked(false),
            nvidia_power_limit_w: None,
        }
    }

    fn read_raw(&self) -> PortResult<RawLimits> {
        Ok(*locked(&self.raw))
    }

    fn write_raw(&self, limits: RawLimits) -> PortResult<()> {
        *locked(&self.raw) = limits;
        locked(&self.writes).push(limits);
        Ok(())
    }

    fn set_limits(&self, pl1: f64, pl2: f64) -> PortResult<PowerApplied> {
        *locked(&self.raw) = RawLimits { pl1_uw: (pl1 * 1e6) as u64, pl2_uw: (pl2 * 1e6) as u64 };
        Ok(PowerApplied { pl1_w: pl1, pl2_w: pl2 })
    }
}

#[derive(Default)]
pub(super) struct FakeCpuUv {
    pub(super) offsets: Mutex<(i32, i32)>,
    pub(super) applied: Mutex<Vec<(i32, i32, bool)>>,
    pub(super) fail_apply: AtomicBool,
}

impl CpuUndervoltPort for FakeCpuUv {
    fn read(&self) -> PortResult<Offsets> {
        let (core_mv, cache_mv) = *locked(&self.offsets);
        Ok(Offsets { core_mv, cache_mv })
    }

    fn apply(&self, core_mv: i32, cache_mv: i32, confirm: bool) -> PortResult<Offsets> {
        if self.fail_apply.load(Ordering::SeqCst) {
            return Err(PortError::new("the CPU kept its stock voltage"));
        }
        *locked(&self.offsets) = (core_mv, cache_mv);
        locked(&self.applied).push((core_mv, cache_mv, confirm));
        Ok(Offsets { core_mv, cache_mv })
    }

    fn reset(&self) -> PortResult<Offsets> {
        *locked(&self.offsets) = (0, 0);
        Ok(Offsets::default())
    }
}

#[derive(Default)]
pub(super) struct FakeGpuUv {
    pub(super) state: Mutex<(i32, i32)>,
}

impl GpuUndervoltPort for FakeGpuUv {
    fn read(&self) -> PortResult<GpuState> {
        Ok(GpuState { offset_mhz: locked(&self.state).0, range_mhz: [-300, 250] })
    }

    fn apply(&self, offset_mhz: i32, cap_mhz: i32) -> PortResult<GpuApplied> {
        *locked(&self.state) = (offset_mhz, cap_mhz);
        Ok(GpuApplied { offset_mhz, cap_mhz })
    }

    fn reset(&self) -> PortResult<GpuApplied> {
        *locked(&self.state) = (0, 0);
        Ok(GpuApplied { offset_mhz: 0, cap_mhz: 0 })
    }
}

#[derive(Default)]
pub(super) struct FakePm {
    pub(super) found: Mutex<Vec<UnitStatus>>,
    pub(super) recorded: AtomicBool,
    pub(super) takeovers: AtomicUsize,
}

impl PowerManagersPort for FakePm {
    fn status(&self) -> Vec<UnitStatus> {
        locked(&self.found).clone()
    }

    fn takeover(&self) -> PortResult<TakeoverReport> {
        self.takeovers.fetch_add(1, Ordering::SeqCst);
        self.recorded.store(true, Ordering::SeqCst);
        let found: Vec<String> =
            std::mem::take(&mut *locked(&self.found)).into_iter().map(|u| u.unit).collect();
        Ok(TakeoverReport {
            found: found.clone(),
            stopped: found,
            failed: Vec::new(),
            cpu_not_set: Vec::new(),
            cpu: CpuState::default(),
            ceiling_fixed: true,
        })
    }

    fn restore(&self) -> RestoreReport {
        self.recorded.store(false, Ordering::SeqCst);
        RestoreReport { restored: Vec::new(), cpu_not_set: Vec::new(), cpu: CpuState::default(), note: None }
    }

    fn recorded(&self) -> bool {
        self.recorded.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
pub(super) struct FakeLooks {
    pub(super) boot: Mutex<Option<Look>>,
    pub(super) applied: Mutex<Vec<Look>>,
    /// The first this many applications fail (the keyboard enumerates late).
    pub(super) fail_first: AtomicUsize,
}

impl BootLookPort for FakeLooks {
    fn read_boot(&self) -> Option<Look> {
        locked(&self.boot).clone()
    }

    fn write_boot(&self, look: Option<&Look>) -> PortResult<()> {
        *locked(&self.boot) = look.cloned();
        Ok(())
    }

    fn apply(&self, look: &Look) -> PortResult<Vec<String>> {
        if self.fail_first.load(Ordering::SeqCst) > 0 {
            self.fail_first.fetch_sub(1, Ordering::SeqCst);
            return Err(PortError::new("keyboard not found"));
        }
        locked(&self.applied).push(look.clone());
        Ok(vec!["keyboard: wave".to_owned()])
    }
}

#[derive(Default)]
pub(super) struct FakeKeyboard {
    pub(super) followed: Mutex<Vec<String>>,
}

impl OperatingLight for FakeKeyboard {
    fn follow_mode(&self, mode: &str, _colors: &BTreeMap<String, String>) -> PortResult<()> {
        locked(&self.followed).push(mode.to_owned());
        Ok(())
    }
}

pub(super) struct FakeTouchpad {
    pub(super) on: Mutex<Option<bool>>,
}

impl TouchpadPort for FakeTouchpad {
    fn enabled(&self) -> Option<bool> {
        *locked(&self.on)
    }

    fn set_enabled(&self, on: bool) -> PortResult<bool> {
        *locked(&self.on) = Some(on);
        Ok(on)
    }
}

#[derive(Default)]
pub(super) struct FakeOsd {
    pub(super) shown: Mutex<Vec<String>>,
    /// Makes the notifier panic, as a broken helper might.
    pub(super) explode: AtomicBool,
}

impl Notice for FakeOsd {
    fn show(&self, text: &str, _icon: &str) {
        assert!(!self.explode.load(Ordering::SeqCst), "the notifier blew up");
        locked(&self.shown).push(text.to_owned());
    }
}

/// The `[alerts]` settings a test changes while the daemon runs; every change moves the stamp, as
/// a rewritten configuration file moves its modification time.
pub(super) struct FakeAlertSettings {
    current: Mutex<Result<AlertsConfig, String>>,
    stamp: std::sync::atomic::AtomicU64,
}

impl FakeAlertSettings {
    fn new(initial: AlertsConfig) -> Self {
        Self { current: Mutex::new(Ok(initial)), stamp: std::sync::atomic::AtomicU64::new(1) }
    }

    pub(super) fn set(&self, config: Result<AlertsConfig, String>) {
        *locked(&self.current) = config;
        self.stamp.fetch_add(1, Ordering::SeqCst);
    }
}

impl AlertSettings for FakeAlertSettings {
    fn stamp(&self) -> u64 {
        self.stamp.load(Ordering::SeqCst)
    }

    fn load(&self) -> Result<AlertsConfig, String> {
        locked(&self.current).clone()
    }
}

#[derive(Default)]
pub(super) struct FakeProcs {
    pub(super) list: Mutex<Vec<Process>>,
}

impl FakeProcs {
    pub(super) fn run(&self, name: &str) {
        *locked(&self.list) = vec![Process {
            comm: format!("{name}\n").into_bytes(),
            cmdline: format!("{name}\0").into_bytes(),
        }];
    }

    pub(super) fn none(&self) {
        locked(&self.list).clear();
    }
}

impl ProcessList for FakeProcs {
    fn list(&self) -> Vec<Process> {
        locked(&self.list).clone()
    }
}

/// The desktop user of the tests.
pub(super) struct FakeUsers {
    pub(super) uid: u32,
    pub(super) home: PathBuf,
}

impl UserDirectory for FakeUsers {
    fn lookup(&self, name: &str) -> Option<UserEntry> {
        (name == "tester").then(|| UserEntry { uid: self.uid, home: self.home.display().to_string() })
    }
}

/// A whole fake laptop around a daemon.
pub(super) struct Rig {
    pub(super) daemon: Arc<Daemon>,
    pub(super) router: ts_ipc::server::Router,
    pub(super) sim: Arc<Simulator>,
    pub(super) device: Arc<FaultDevice>,
    pub(super) sensors: Arc<FakeSensors>,
    pub(super) cap: Arc<FakeCap>,
    pub(super) power: Arc<FakePower>,
    pub(super) uv: Arc<FakeCpuUv>,
    pub(super) gpu_uv: Arc<FakeGpuUv>,
    pub(super) pm: Arc<FakePm>,
    pub(super) looks: Arc<FakeLooks>,
    pub(super) keyboard: Arc<FakeKeyboard>,
    pub(super) touchpad: Arc<FakeTouchpad>,
    pub(super) osd: Arc<FakeOsd>,
    pub(super) alert_settings: Arc<FakeAlertSettings>,
    pub(super) procs: Arc<FakeProcs>,
    pub(super) clock: Arc<FakeClock>,
    pub(super) sleeper: Arc<FakeSleeper>,
    pub(super) log: Arc<TestLog>,
    pub(super) state: Arc<Store>,
    pub(super) stop: StopToken,
    pub(super) dir: TempDir,
    pub(super) home: PathBuf,
}

/// What differs between the rigs of the tests.
pub(super) struct Setup {
    pub(super) config: Config,
    /// Extra settings over the Python test's (`smart_fan`, `auto_profile`, `operating_follow`).
    pub(super) settings: Vec<(&'static str, Value)>,
    /// The notifier (a socket path to listen on), disabled by default.
    pub(super) notifier: Notifier,
    pub(super) watchdog_every: Option<Duration>,
}

impl Default for Setup {
    #[allow(clippy::field_reassign_with_default)]
    fn default() -> Self {
        let mut config = Config::default();
        config.user = "tester".to_owned();
        Self { config, settings: Vec::new(), notifier: Notifier::disabled(), watchdog_every: None }
    }
}

impl Rig {
    /// The laptop of the Python `TestDaemonLogic.make`: smart fan on, automatic profile on (so the
    /// automatic mode is Smart), AC, 80 % battery, an idle machine.
    pub(super) fn new() -> Self {
        Self::with(Setup::default())
    }

    pub(super) fn with(setup: Setup) -> Self {
        let dir = TempDir::new();
        let home = dir.join("home");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(dir.join("run")).unwrap(); // what the service manager creates for the daemon
        let sim = Arc::new(Simulator::new());
        let device = Arc::new(FaultDevice {
            sim: Arc::clone(&sim),
            dead: AtomicBool::new(false),
            stuck_stop: AtomicBool::new(false),
            sensors_dead: AtomicBool::new(false),
            sent: Mutex::new(Vec::new()),
        });
        let hw = Hw::new(device.clone()).with_sleeper(Arc::new(|_| {}));
        // The state of the Python test: only these four settings, so the curve and the Smart
        // profile are the built-in `truesense` ones (a real first start has `native` for both).
        let mut defaults: Map<String, Value> = Map::new();
        for (k, v) in [
            ("smart_fan", Value::Bool(true)),
            ("auto_profile", Value::Bool(true)),
            ("operating_follow", Value::Bool(false)),
            ("profile_hold_until", Value::from(0)),
        ] {
            defaults.insert(k.to_owned(), v);
        }
        for (k, v) in &setup.settings {
            defaults.insert((*k).to_owned(), v.clone());
        }
        let state = Arc::new(Store::open(dir.join("state.json"), defaults));
        let clock = Arc::new(FakeClock::new(1_800_000_000.0));
        let sleeper = Arc::new(FakeSleeper {
            clock: Arc::clone(&clock),
            calls: AtomicUsize::new(0),
            stop_after: AtomicUsize::new(0),
            last: Mutex::new(Duration::ZERO),
            hook: Mutex::new(None),
        });
        let sensors = Arc::new(FakeSensors::new());
        let cap = Arc::new(FakeCap::default());
        let power = Arc::new(FakePower {
            raw: Mutex::new(RawLimits { pl1_uw: 80_000_000, pl2_uw: 100_000_000 }),
            writes: Mutex::new(Vec::new()),
        });
        let uv = Arc::new(FakeCpuUv::default());
        let gpu_uv = Arc::new(FakeGpuUv::default());
        let pm = Arc::new(FakePm::default());
        let looks = Arc::new(FakeLooks::default());
        let keyboard = Arc::new(FakeKeyboard::default());
        let touchpad = Arc::new(FakeTouchpad { on: Mutex::new(Some(true)) });
        let osd = Arc::new(FakeOsd::default());
        let alert_settings = Arc::new(FakeAlertSettings::new(setup.config.alerts.clone()));
        let procs = Arc::new(FakeProcs::default());
        let log = Arc::new(TestLog::default());
        let stop = StopToken::new();
        let deps = Deps {
            config: setup.config,
            state: Arc::clone(&state),
            hw,
            sensors: sensors.clone(),
            cpu_cap: cap.clone(),
            power: power.clone(),
            cpu_undervolt: uv.clone(),
            gpu_undervolt: gpu_uv.clone(),
            power_managers: pm.clone(),
            looks: looks.clone(),
            keyboard: keyboard.clone(),
            touchpad: touchpad.clone(),
            osd: osd.clone(),
            alert_settings: alert_settings.clone(),
            processes: procs.clone(),
            users: Arc::new(FakeUsers { uid: my_uid(), home: home.clone() }),
            curves: curves::Store::new(dir.join("curves")),
            smart_profiles: smartprofile::Store::new(dir.join("smart")),
            uv_guard: Guard::new(dir.join("var/uv-pending")),
            gpu_uv_guard: Guard::new(dir.join("var/gpu-uv-pending")),
            live_paths: LivePaths {
                run_state: dir.join("run/state"),
                tmp_state: dir.join("tmp-state"),
                predator_state: dir.join("predator-state"),
            },
            notifier: setup.notifier,
            watchdog_every: setup.watchdog_every,
            clock: clock.clone(),
            sleeper: sleeper.clone(),
            log: log.clone(),
            stop: stop.clone(),
        };
        let daemon = Daemon::new(deps);
        let router = daemon.router();
        Self {
            daemon,
            router,
            sim,
            device,
            sensors,
            cap,
            power,
            uv,
            gpu_uv,
            pm,
            looks,
            keyboard,
            touchpad,
            osd,
            alert_settings,
            procs,
            clock,
            sleeper,
            log,
            state,
            stop,
            dir,
            home,
        }
    }

    /// Runs `n` ticks of one second at the given readings (CPU and GPU temperature in degrees C,
    /// CPU load in percent).
    pub(super) fn drive(&self, n: usize, cpu: u32, gpu: u32, load: f64) {
        for _ in 0..n {
            self.sim.set_sensors([cpu, 55, gpu], [0, 0]);
            self.sensors.set_load(load);
            self.daemon.guarded_step();
            self.clock.advance(1.0);
        }
    }

    /// Sends a request through the command table, as the socket server would.
    pub(super) fn call(&self, cmd: &str, args: Args) -> Result<Value, String> {
        use ts_ipc::server::Handler;
        assert!(COMMANDS.contains(&cmd) || cmd == "nope", "{cmd} is not in the table");
        self.router.handle(&Request::new(cmd, args), 0)
    }

    pub(super) fn ok(&self, cmd: &str, args: Args) -> Value {
        match self.call(cmd, args.clone()) {
            Ok(v) => v,
            Err(e) => panic!("{cmd} {args:?} failed: {e}"),
        }
    }

    pub(super) fn clock_mono(&self) -> f64 {
        use super::deps::Clock;
        self.clock.mono()
    }

    pub(super) fn snap(&self) -> Snapshot {
        self.sim.snapshot()
    }

    pub(super) fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    pub(super) fn read(&self, path: &Path) -> String {
        fs::read_to_string(path).unwrap_or_default()
    }

    pub(super) fn ok_set_smart_fan(&self, on: bool) {
        self.ok("set", Args::new().with("key", "smart_fan").with("value", on));
    }

    pub(super) fn ok_fan_manual(&self, percent: i64) {
        self.ok("fan", Args::new().with("mode", "manual").with("cpu", percent).with("gpu", percent));
    }

    pub(super) fn auto_mode(&self, kind: &str) {
        self.ok("set", Args::new().with("key", "auto_mode").with("value", kind));
    }
}
