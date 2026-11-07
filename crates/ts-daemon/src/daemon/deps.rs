//! deps: everything the daemon touches outside itself, as small traits and one `Deps` struct.
//!
//! The daemon decides nothing and touches nothing directly: every effect (the firmware, the
//! sensors, the CPU clock cap, the RAPL limits, the undervolt registers, the other power
//! managers, the lights, the touchpad, the on-screen display, the process list, the clock, the
//! sleep between two ticks) is reached through a trait defined here, so a test replaces each
//! one with a fake and drives a whole laptop tick by tick without a real sleep or a real file
//! outside a temporary directory. The domain and system crates already hold the concrete
//! types; [`super::adapt`] implements these traits for them, so the assembly layer only has to
//! build the concrete values and put them into [`Deps`].
//!
//! The firmware itself is not behind a trait of ours: `ts_platform::hw::Hw` already is one
//! value over a `Device`, and a test gives it the simulator.

use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ts_decision::crashguard::Guard;
use ts_decision::curves;
use ts_decision::gamerules::Process;
use ts_decision::smartprofile;
use ts_domain::gpuundervolt::{Applied as GpuApplied, State as GpuState};
use ts_domain::looks::Look;
use ts_domain::power::{Applied as PowerApplied, Info as PowerInfo, RawLimits};
use ts_domain::powermgr::{RestoreReport, TakeoverReport, UnitStatus};
use ts_domain::undervolt::Offsets;
use ts_foundation::config::Config;
use ts_foundation::fsx;
use ts_foundation::sdnotify::Notifier;
use ts_foundation::state::{self, Store};
use ts_foundation::sync::locked;
use ts_platform::hw::Hw;
use ts_system::osd::UserDirectory;
use ts_system::sensors::{GpuFeed, GpuReading};

use super::alerts::AlertSettings;

/// What a port reports when its operation failed. The text is what the user sees in the error
/// reply of the control socket, so it is the failing crate's own message, unchanged.
///
/// It is a plain text wrapper on purpose: the daemon never branches on the cause of a failure
/// of the lights or of the RAPL files, it logs it or passes it to the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortError(String);

impl PortError {
    /// A failure with this message.
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    /// A failure with the message of `error`, for adapters over typed error enums.
    pub fn of(error: &dyn fmt::Display) -> Self {
        Self(error.to_string())
    }

    /// The message.
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PortError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PortError {}

/// The result of a port operation.
pub type PortResult<T> = Result<T, PortError>;

/// The two clocks the daemon needs. The wall clock can jump (suspend, NTP) and is what the
/// stored hold and the full-charge record are measured on; the monotonic clock is what every
/// timer runs on, and it stands still while the machine sleeps, which is how a resume from
/// suspend is noticed.
pub trait Clock: Send + Sync {
    /// Seconds since the Unix epoch.
    fn wall(&self) -> f64;
    /// Seconds on a monotonic clock with an arbitrary origin.
    fn mono(&self) -> f64;
}

/// The machine's clocks.
#[derive(Debug, Clone, Copy)]
pub struct SystemClock {
    start: Instant,
}

impl SystemClock {
    /// The real clocks, with the monotonic origin at this moment.
    pub fn new() -> Self {
        Self { start: Instant::now() }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn wall(&self) -> f64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
    }

    fn mono(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }
}

/// A flag that says "stop now" and wakes whoever sleeps on it. The assembly layer holds a clone
/// and calls [`StopToken::request`] from the thread that waits for SIGTERM and SIGINT (the
/// daemon module forbids `unsafe`, so it cannot install a signal handler itself).
#[derive(Debug, Clone, Default)]
pub struct StopToken {
    inner: Arc<(Mutex<bool>, Condvar)>,
}

impl StopToken {
    /// A token nobody has asked to stop yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Asks everything that watches this token to wind down. Safe to call twice.
    pub fn request(&self) {
        let (flag, wake) = &*self.inner;
        *locked(flag) = true;
        wake.notify_all();
    }

    /// Whether a stop was requested.
    pub fn is_requested(&self) -> bool {
        *locked(&self.inner.0)
    }

    /// Waits up to `limit` and returns whether a stop was requested by then (at once when it
    /// already was).
    pub fn wait_timeout(&self, limit: Duration) -> bool {
        let (flag, wake) = &*self.inner;
        let deadline = Instant::now() + limit;
        let mut stopped = locked(flag);
        while !*stopped {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            stopped = wake.wait_timeout(stopped, left).unwrap_or_else(PoisonError::into_inner).0;
        }
        *stopped
    }
}

/// The pause between two ticks (and between the retries of the boot lighting). It belongs to
/// the dependencies so that a test replaces real waiting by a fake clock that jumps.
pub trait Sleeper: Send + Sync {
    /// Waits `limit`, or less when `stop` is requested meanwhile. Returns whether the stop was
    /// requested, so a loop can end the moment it is asked to.
    fn sleep(&self, limit: Duration, stop: &StopToken) -> bool;
}

/// Really waits, and wakes up at once when the stop is requested.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealSleeper;

impl Sleeper for RealSleeper {
    fn sleep(&self, limit: Duration, stop: &StopToken) -> bool {
        stop.wait_timeout(limit)
    }
}

/// How serious a log line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// A normal event worth a line in the journal (a mode change, a setting).
    Info,
    /// Something went wrong that the daemon survives.
    Warn,
}

/// Where the daemon's journal lines go.
pub trait Logger: Send + Sync {
    /// Writes one line.
    fn log(&self, level: Level, message: &str);
}

/// Writes the lines to standard output, where systemd's journal collects them (as the Python
/// edition's `print` did). A closed standard output loses the line and nothing else.
#[derive(Debug, Clone, Copy, Default)]
pub struct StdoutLogger;

impl Logger for StdoutLogger {
    fn log(&self, level: Level, message: &str) {
        use std::io::Write;
        let tag = match level {
            Level::Info => "",
            Level::Warn => "warning: ",
        };
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{tag}{message}");
        let _ = out.flush();
    }
}

/// What the daemon reads from the machine besides the firmware: load, battery, the charger, the
/// CPU package temperature and the GPU's readings.
pub trait Sensors: Send + Sync {
    /// The smoothed CPU load in percent.
    fn cpu_load(&self) -> f64;
    /// The CPU package temperature in degrees C, `None` without a coretemp sensor.
    fn package_temp(&self) -> Option<f64>;
    /// The battery charge in percent (100 without a battery).
    fn battery_pct(&self) -> f64;
    /// The latest GPU reading (temperature, smoothed load and power, when it arrived).
    fn gpu(&self) -> GpuReading;
    /// Whether the charger is plugged in.
    fn on_ac(&self) -> bool;
    /// Asks for the GPU sampler: starts it if it is not running yet. Every question to the card
    /// wakes it from runtime suspend, so the daemon asks only while a smart feature needs the GPU
    /// (and the sampler itself leaves a sleeping card alone and lets an idle one go to sleep).
    /// Idempotent.
    fn ensure_gpu_stream(&self);
    /// Withdraws the request: nothing needs the GPU readings now, so the sampler ends and the
    /// card is left to suspend. Idempotent; the default does nothing (a source without a stream).
    fn release_gpu_stream(&self) {}
    /// What a 0 GPU load or 0 W means right now: a card that sleeps, a live reading, or a card
    /// nobody is reading. The default is the honest "unsampled" for a source that cannot tell.
    fn gpu_state(&self) -> GpuFeed {
        GpuFeed::Unsampled
    }
}

/// The CPU's maximum clock (cpufreq `scaling_max_freq`), which the thermal governor lowers.
pub trait ClockCap: Send + Sync {
    /// Caps every CPU at `khz`. The returned texts are the CPUs that could not be written
    /// (they went offline), for the log.
    fn apply(&self, khz: i64) -> Vec<String>;
    /// Gives every CPU its own hardware maximum back.
    fn uncap(&self) -> Vec<String>;
}

/// The CPU package power limits (RAPL).
pub trait PowerLimitsPort: Send + Sync {
    /// What the power-limit files say now, for `cpu_info` and `cpu_limits`.
    fn info(&self) -> PowerInfo;
    /// PL1 and PL2 exactly as stored, in microwatts.
    fn read_raw(&self) -> PortResult<RawLimits>;
    /// Writes PL1 and PL2 as they are (the daemon keeps them where the user wants them).
    fn write_raw(&self, limits: RawLimits) -> PortResult<()>;
    /// Validates, writes and verifies a pair in watts; the previous limits come back when the
    /// hardware did not keep them.
    fn set_limits(&self, pl1: f64, pl2: f64) -> PortResult<PowerApplied>;
}

/// The CPU undervolt (the voltage-offset mailbox).
pub trait CpuUndervoltPort: Send + Sync {
    /// The offsets now.
    fn read(&self) -> PortResult<Offsets>;
    /// Writes both offsets (needs `confirm`), verified by reading them back.
    fn apply(&self, core_mv: i32, cache_mv: i32, confirm: bool) -> PortResult<Offsets>;
    /// Back to stock voltage.
    fn reset(&self) -> PortResult<Offsets>;
}

/// The GPU undervolt (NVML curve offset and clock cap).
pub trait GpuUndervoltPort: Send + Sync {
    /// The offset now and the range the driver allows.
    fn read(&self) -> PortResult<GpuState>;
    /// Applies an offset (MHz) and a clock cap (MHz, 0 = none), verified.
    fn apply(&self, offset_mhz: i32, cap_mhz: i32) -> PortResult<GpuApplied>;
    /// Back to stock.
    fn reset(&self) -> PortResult<GpuApplied>;
}

/// The detection and takeover of the other power managers.
pub trait PowerManagersPort: Send + Sync {
    /// Known managers that are running or enabled.
    fn status(&self) -> Vec<UnitStatus>;
    /// Stops them, fixes the CPU defaults and records what was done.
    fn takeover(&self) -> PortResult<TakeoverReport>;
    /// Undoes the takeover.
    fn restore(&self) -> RestoreReport;
    /// Whether a takeover is on record.
    fn recorded(&self) -> bool;
}

/// The lighting chosen for boot.
pub trait BootLookPort: Send + Sync {
    /// The stored boot look, if any.
    fn read_boot(&self) -> Option<Look>;
    /// Stores the boot look, or removes it with `None`.
    fn write_boot(&self, look: Option<&Look>) -> PortResult<()>;
    /// Puts a look on the keyboard and the rear light; one line per part that was done.
    fn apply(&self, look: &Look) -> PortResult<Vec<String>>;
}

/// The keyboard's operating-mode colour that follows the power mode.
pub trait OperatingLight: Send + Sync {
    /// Re-sends the colour for `mode`; `colors` overrides the stock colours per mode.
    fn follow_mode(&self, mode: &str, colors: &std::collections::BTreeMap<String, String>) -> PortResult<()>;
}

/// The touchpad switch.
pub trait TouchpadPort: Send + Sync {
    /// Whether the touchpad is on; `None` when it cannot be told.
    fn enabled(&self) -> Option<bool>;
    /// Switches it and returns the state reached.
    fn set_enabled(&self, on: bool) -> PortResult<bool>;
}

/// The on-screen messages ("Power mode: turbo ...").
pub trait Notice: Send + Sync {
    /// Shows `text` with an icon name; whether a helper took it does not matter to the daemon.
    fn show(&self, text: &str, icon: &str);
}

/// The running programs, for the game rules.
pub trait ProcessList: Send + Sync {
    /// The programs running now (their names and command lines).
    fn list(&self) -> Vec<Process>;
}

/// The files the live state is published to: `/run/truesense/state` (what `health.sh` watches),
/// a copy under `/tmp` for status bars whose sandbox cannot see `/run`, and the legacy widgets'
/// file when `[compat] predator_widgets` is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LivePaths {
    /// The primary file.
    pub run_state: PathBuf,
    /// The world-readable copy for sandboxed status bars.
    pub tmp_state: PathBuf,
    /// The file the old Predator widgets read; only written with the compatibility switch.
    pub predator_state: PathBuf,
}

impl LivePaths {
    /// The files below `run_dir` (normally `/run/truesense`) and the usual `/tmp` copies.
    pub fn in_run_dir(run_dir: impl Into<PathBuf>) -> Self {
        Self {
            run_state: run_dir.into().join("state"),
            tmp_state: PathBuf::from(state::TMP_LIVE_PATH),
            predator_state: PathBuf::from(state::PREDATOR_COMPAT_PATH),
        }
    }

    /// The real machine: `$TRUESENSE_RUN` or `/run/truesense`.
    pub fn system() -> Self {
        Self::in_run_dir(fsx::env_nonempty("TRUESENSE_RUN").unwrap_or_else(|| state::RUN_DIR.to_owned()))
    }
}

/// Everything the daemon is built from. Every field is replaced by a fake in the tests.
pub struct Deps {
    /// The merged configuration (a configuration with problems has already been replaced by the
    /// built-in defaults by the caller, as the Python daemon did at start).
    pub config: Config,
    /// Problems found while the configuration was loaded (the caller already replaced a
    /// configuration with problems by the built-in defaults). They are logged at start and shown
    /// in the live state as `warning`, so a typo in a config file is visible and not only a line
    /// in a journal nobody reads.
    pub warnings: Vec<String>,
    /// The persistent settings.
    pub state: Arc<Store>,
    /// The firmware.
    pub hw: Hw,
    /// Load, battery, charger, temperatures.
    pub sensors: Arc<dyn Sensors>,
    /// The CPU clock cap.
    pub cpu_cap: Arc<dyn ClockCap>,
    /// The RAPL limits.
    pub power: Arc<dyn PowerLimitsPort>,
    /// The CPU undervolt.
    pub cpu_undervolt: Arc<dyn CpuUndervoltPort>,
    /// The GPU undervolt.
    pub gpu_undervolt: Arc<dyn GpuUndervoltPort>,
    /// The other power managers.
    pub power_managers: Arc<dyn PowerManagersPort>,
    /// The boot look.
    pub looks: Arc<dyn BootLookPort>,
    /// The keyboard's operating-mode light.
    pub keyboard: Arc<dyn OperatingLight>,
    /// The touchpad.
    pub touchpad: Arc<dyn TouchpadPort>,
    /// The on-screen display.
    pub osd: Arc<dyn Notice>,
    /// Where the `[alerts]` settings are re-read from while the daemon runs.
    pub alert_settings: Arc<dyn AlertSettings>,
    /// The process list.
    pub processes: Arc<dyn ProcessList>,
    /// Finds the desktop user (its uid may use the socket, its home holds the legacy flag files).
    pub users: Arc<dyn UserDirectory>,
    /// The saved fan curves.
    pub curves: curves::Store,
    /// The saved Smart auto profiles.
    pub smart_profiles: smartprofile::Store,
    /// The crash guard of the CPU undervolt applied at boot.
    pub uv_guard: Guard,
    /// The crash guard of the GPU undervolt applied at boot.
    pub gpu_uv_guard: Guard,
    /// Where the live state goes.
    pub live_paths: LivePaths,
    /// The systemd notification socket (disabled outside systemd).
    pub notifier: Notifier,
    /// How often systemd wants `WATCHDOG=1` (half the configured timeout); `None` pings after
    /// every tick.
    pub watchdog_every: Option<Duration>,
    /// The clocks.
    pub clock: Arc<dyn Clock>,
    /// The pause between ticks.
    pub sleeper: Arc<dyn Sleeper>,
    /// The journal.
    pub log: Arc<dyn Logger>,
    /// Set from outside (SIGTERM, SIGINT) to end the loop; the daemon also sets it on exit so
    /// that its background threads stop.
    pub stop: StopToken,
}
