//! assembly: builds every concrete component of the root service once and starts the daemon.
//!
//! The daemon itself ([`crate::daemon`]) is wiring only and takes its collaborators as traits;
//! this module is the one place that knows which real type stands behind each trait, reads the
//! environment (`TRUESENSE_SIM`, `TRUESENSE_RUN`, ...) and installs the signal handling. Tests
//! never go through here: they build [`Deps`] from fakes.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use ts_decision::thermal::CapWriter;
use ts_decision::{crashguard, curves, smartprofile};
use ts_domain::gpuundervolt::{GpuUndervolt, helper::HelperNvml};
use ts_domain::keyboard::{HidOpener, HidrawOpener, Keyboard, SimHid};
use ts_domain::looks::{self, LookStore};
use ts_domain::power::{HostSysfs, MsrReader, PowerLimits};
use ts_domain::powermgr::{self, PowerManagers};
use ts_domain::undervolt::{DevMsr, Undervolt};
use ts_foundation::config::Config;
use ts_foundation::state::{self, Store};
use ts_foundation::{fsx, sdnotify, userstate};
use ts_platform::firmware::DeviceKind;
use ts_platform::hw::{Hw, LinuxSensors, ProcessRunner, RearMemory};
use ts_system::osd::{Osd, PasswdFile};
use ts_system::sensors::{ProcessLineSource, Sampler};
use ts_system::touchpad::Touchpad;

use crate::daemon::adapt::{ProcFs, SystemSensors};
use crate::daemon::{
    self, DaemonError, Deps, Level, LivePaths, Logger, RealSleeper, StdoutLogger, StopToken, SystemClock,
};

/// Where the root service's home is when `HOME` is not set (systemd units run it as root).
const ROOT_HOME: &str = "/root";

/// Why the service could not be assembled or started.
#[derive(Debug)]
pub enum AssemblyError {
    /// The run directory (`/run/truesense`) could not be created.
    RunDir(String),
    /// The signal handling could not be installed.
    Signals(io::Error),
    /// The daemon failed.
    Daemon(DaemonError),
}

impl std::fmt::Display for AssemblyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RunDir(e) => write!(f, "cannot create the run directory: {e}"),
            Self::Signals(e) => write!(f, "cannot install the signal handling: {e}"),
            Self::Daemon(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for AssemblyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::RunDir(_) => None,
            Self::Signals(e) => Some(e),
            Self::Daemon(e) => Some(e),
        }
    }
}

/// Remembers the rear light's state in the user-state file, where the lighting profiles look
/// for it (`rear_state`, `rear_zones`).
struct StateMemory(Arc<userstate::Store>);

impl RearMemory for StateMemory {
    fn remember(&self, key: &str, value: Value) {
        // A failed write only means a profile capture will not see the rear light: not worth
        // failing the hardware write that already succeeded.
        let _ = self.0.set(key, value);
    }
}

fn env(key: &str) -> Option<String> {
    fsx::env_nonempty(key)
}

/// The configuration the service runs with: the merged files, or the built-in defaults for the
/// whole configuration when anything in them is wrong (a typo must not stop the fan control).
/// The problems are returned so they can be logged.
pub fn load_config() -> (Config, Vec<String>) {
    match Config::load_standard() {
        Ok(config) => {
            let problems = config.validate();
            if problems.is_empty() { (config, problems) } else { (Config::default(), problems) }
        }
        Err(e) => (Config::default(), vec![e.to_string()]),
    }
}

/// Blocks SIGTERM and SIGINT in this thread (and so in every thread started later) and starts a
/// thread that waits for them and asks `stop` to wind down. Must run before any other thread
/// exists, otherwise a signal could be delivered to a thread that does not block it.
fn install_signal_stop(stop: StopToken) -> io::Result<()> {
    // SAFETY: `set` is a zeroed `sigset_t` that `sigemptyset` initialises before any other use;
    // every call only reads or writes this local value and the calling thread's signal mask.
    let set = unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        if libc::sigemptyset(&mut set) != 0
            || libc::sigaddset(&mut set, libc::SIGTERM) != 0
            || libc::sigaddset(&mut set, libc::SIGINT) != 0
        {
            return Err(io::Error::last_os_error());
        }
        let rc = libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        if rc != 0 {
            return Err(io::Error::from_raw_os_error(rc));
        }
        set
    };
    std::thread::Builder::new().name("signals".into()).spawn(move || {
        let mut signal = 0;
        // SAFETY: `set` is a valid, initialised signal set and `signal` a valid out-pointer.
        let rc = unsafe { libc::sigwait(&set, &mut signal) };
        if rc == 0 {
            stop.request();
        }
    })?;
    Ok(())
}

/// The keyboard: the real hidraw device, or the simulator under `TRUESENSE_SIM`.
fn keyboard(state: Arc<userstate::Store>) -> Arc<Keyboard> {
    let opener: Arc<dyn HidOpener> = if env("TRUESENSE_SIM").is_some() {
        Arc::new(SimHid::shared())
    } else {
        Arc::new(HidrawOpener::system())
    };
    Arc::new(Keyboard::new(opener, state))
}

/// Everything `truesense daemon` needs, built from the real machine (or the simulator under
/// `TRUESENSE_SIM=1`).
pub fn build_deps(config: Config, stop: StopToken, log: Arc<dyn Logger>) -> Deps {
    let user_state = Arc::new(userstate::Store::at_default());
    let keyboard = keyboard(user_state.clone());
    let hw = Hw::new(DeviceKind::from_process_env().open())
        .with_kb_timeout(Arc::new(keyboard.timeout_flag()))
        .with_rear_memory(Arc::new(StateMemory(user_state.clone())));

    let runner = Arc::new(ProcessRunner);
    let sysfs = Arc::new(HostSysfs::system());
    let msr = Arc::new(DevMsr::cpu0());
    let msr_reader: Arc<dyn MsrReader> = msr.clone();
    let power = Arc::new(PowerLimits::new(sysfs.clone(), Some(msr_reader), runner.clone()));

    let home = env("HOME").map_or_else(|| PathBuf::from(ROOT_HOME), PathBuf::from);
    let look_store = LookStore::new(
        looks::default_dir(&home),
        looks::default_boot_file(&env),
        user_state,
        keyboard.clone(),
        Arc::new(hw.clone()),
    );
    let sensors = SystemSensors::new(
        Arc::new(Sampler::system()),
        LinuxSensors::system(),
        Arc::new(ProcessLineSource),
        Arc::new(std::thread::sleep),
        stop.clone(),
    );
    let state = Arc::new(Store::open(state::default_path(), state::defaults(&config)));
    let curves_dir = curves::Store::default_dir().unwrap_or_else(|| home.join(".config/truesense/curves"));
    Deps {
        users: Arc::new(PasswdFile::new("/etc/passwd")),
        osd: Arc::new(Osd::system(&config.osd.style, &config.user)),
        config,
        state,
        hw,
        sensors: Arc::new(sensors),
        cpu_cap: Arc::new(CapWriter::system()),
        power,
        cpu_undervolt: Arc::new(Undervolt::new(msr)),
        gpu_undervolt: Arc::new(GpuUndervolt::new(Arc::new(HelperNvml::system(runner.clone())))),
        power_managers: Arc::new(PowerManagers::new(
            runner,
            sysfs,
            powermgr::DEFAULT_CPU_DIR,
            powermgr::record_path(),
        )),
        looks: Arc::new(look_store),
        keyboard,
        touchpad: Arc::new(Touchpad::system()),
        processes: Arc::new(ProcFs::system()),
        curves: curves::Store::new(curves_dir),
        smart_profiles: smartprofile::Store::for_home(&home),
        uv_guard: crashguard::Guard::new(crashguard::uv_pending_path(&env)),
        gpu_uv_guard: crashguard::Guard::new(crashguard::gpu_uv_pending_path(&env)),
        live_paths: LivePaths::system(),
        notifier: sdnotify::Notifier::from_env(),
        watchdog_every: sdnotify::watchdog_interval_from_env(),
        clock: Arc::new(SystemClock::new()),
        sleeper: Arc::new(RealSleeper),
        log,
        stop,
    }
}

/// `truesense daemon`: the root service. Returns when SIGTERM or SIGINT arrives (the fans and
/// the clocks are handed back on every way out) or when the daemon fails.
pub fn run_daemon() -> Result<(), AssemblyError> {
    let stop = StopToken::new();
    install_signal_stop(stop.clone()).map_err(AssemblyError::Signals)?;
    let log: Arc<dyn Logger> = Arc::new(StdoutLogger);
    let (config, problems) = load_config();
    for problem in &problems {
        log.log(Level::Warn, &format!("config problem: {problem}"));
    }
    if !problems.is_empty() {
        log.log(Level::Warn, "falling back to the built-in defaults for the whole configuration");
    }
    let live = LivePaths::system();
    if let Some(dir) = live.run_state.parent() {
        fsx::ensure_dir(dir, 0o755).map_err(|e| AssemblyError::RunDir(e.to_string()))?;
    }
    let deps = build_deps(config, stop, log);
    daemon::run(deps, ts_ipc::wire::socket_path()).map_err(AssemblyError::Daemon)
}
