//! The daemon's state and its control loop.
//!
//! One [`Daemon`] owns the ports it was built from and one mutex-guarded [`Inner`] with
//! everything that changes while it runs: the smoothed readings, the decision engines and the
//! bookkeeping between ticks. The control loop and every socket handler take that one lock, so
//! only one thing changes the hardware at a time (the Python daemon's `RLock`).
//!
//! The lock is always taken through [`Daemon::lock`], which ignores poisoning: a step that
//! panicked while holding it must not make every later request, and above all the emergency
//! fan fallback and the shutdown, panic or hang on a poisoned mutex. (The Go port found that
//! deadlock on the real laptop; here it is the same rule with a different mechanism.)

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::Value;
use ts_decision::autoprofile::{Activity, AutoKind, Classifier, Current, Engine, NativeModes};
use ts_decision::crashguard::Guard;
use ts_decision::fanctl::{self, FanHardware, FanWriter, Smart, StopWatchdog};
use ts_decision::gamerules::{Rules, Watcher};
use ts_decision::smartprofile::Mode;
use ts_decision::thermal::{self, Governor};
use ts_domain::power::RawLimits;
use ts_foundation::config::Config;
use ts_foundation::sdnotify::Notifier;
use ts_foundation::state::{LiveState, Store};
use ts_ipc::server::{Server, ServerConfig};
use ts_platform::hw::{Coded, FanDuty, FanMode, Hw, HwError, Reading};
use ts_system::osd::UserDirectory;
use ts_system::sensors::Ema;

use super::DaemonError;
use super::alerts::{AlertSettings, AlertsRuntime};
use super::bindings::Runtime as BindRuntime;
use super::deps::{
    BootLookPort, Clock, ClockCap, CpuUndervoltPort, Deps, GpuUndervoltPort, Level, LivePaths, Logger,
    Notice, OperatingLight, PowerLimitsPort, PowerManagersPort, ProcessList, Sensors, Sleeper, StopToken,
    TouchpadPort,
};

/// The smoothing of the CPU and GPU temperatures (a new reading counts for 40 %).
const TEMP_ALPHA: f64 = 0.4;
/// The smoothing of the board (chassis) temperature, which moves slowly.
const CHASSIS_ALPHA: f64 = 0.2;
/// A loop that finds itself this many seconds behind schedule was asleep (suspend): it resyncs
/// instead of running a burst of catch-up ticks.
const RESYNC_AFTER_S: f64 = 5.0;
/// The pace before the first tick has decided otherwise.
const FIRST_INTERVAL: Duration = Duration::from_secs(1);

/// The fans as `FanWriter` drives them: the firmware's `Hw` with the duty types it demands.
pub(super) struct HwFans(pub(super) Hw);

impl FanHardware for HwFans {
    type Error = HwError;

    fn fan_auto(&mut self) -> Result<(), HwError> {
        self.0.fan_auto()
    }

    fn fan_manual(&mut self, cpu: u8, gpu: u8) -> Result<(), HwError> {
        self.0.fan_manual(FanDuty::new(i64::from(cpu))?, FanDuty::new(i64::from(gpu))?)
    }

    fn fan_is_auto(&mut self) -> Result<bool, HwError> {
        Ok(self.0.fan_state()?.mode == FanMode::Auto)
    }
}

/// Everything that changes while the daemon runs. Only ever touched with the lock held.
pub(super) struct Inner {
    /// Ticks run so far (the first is 1); the periodic jobs key off it.
    pub(super) tick: u64,
    pub(super) tcpu: Ema,
    pub(super) tgpu: Ema,
    pub(super) tchassis: Ema,
    /// Fan speeds in rpm, as the firmware last said.
    pub(super) rpm: (i64, i64),
    pub(super) classifier: Classifier,
    pub(super) activity: Activity,
    /// Whether the firmware answered on the last tick.
    pub(super) hw_ok: bool,
    /// The charger on the last tick; `None` before the first.
    pub(super) last_ac: Option<bool>,
    /// The power mode the firmware reported last; `None` until it was read.
    pub(super) cur_mode: Option<Reading<ts_platform::hw::PowerMode>>,
    pub(super) engine: Engine,
    pub(super) fan: Smart,
    pub(super) writer: FanWriter<HwFans>,
    /// A user-stopped fan and its temperature watchdog.
    pub(super) stop_watch: StopWatchdog,
    pub(super) governor: Governor,
    /// What the firmware had set for PL1 and PL2, kept to hand back; `None` until captured.
    pub(super) pl_orig: Option<RawLimits>,
    /// Whether this daemon changed the power limits and so owes them back.
    pub(super) pl_touched: bool,
    pub(super) watcher: Watcher,
    pub(super) uv_guard: Guard,
    pub(super) gpu_uv_guard: Guard,
    /// Wall and monotonic clock of the previous tick, to notice a resume from suspend.
    pub(super) last_clock: Option<(f64, f64)>,
    /// The power mode the operating-mode light was last sent for.
    pub(super) op_mode: Option<String>,
    /// What the mode-bindings hook remembers between ticks.
    pub(super) bind: BindRuntime,
    /// The readings of the last tick, as the `state` request shows them.
    pub(super) live: LiveState,
    /// The pause until the next tick.
    pub(super) interval: Duration,
    /// On-screen messages decided during a tick, shown once the lock is released.
    pub(super) notices: Vec<(String, &'static str)>,
    /// The alert rules and what they have decided to say (see `alerts.rs`).
    pub(super) alerts: AlertsRuntime,
}

impl Inner {
    /// The firmware's power mode in the engine's terms.
    pub(super) fn current(&self) -> Current {
        match self.cur_mode {
            None => Current::Unread,
            Some(Reading::Known(m)) => Mode::parse(m.name()).map_or(Current::Unrecognised, Current::Known),
            Some(Reading::Unknown(_)) => Current::Unrecognised,
        }
    }

    /// The mode's name for logs and the live state; `None` while it is unread.
    pub(super) fn mode_name(&self) -> Option<String> {
        self.cur_mode.map(|m| m.to_string())
    }
}

/// The root service. Build it with [`Daemon::new`], run it with [`Daemon::run`].
pub struct Daemon {
    /// Itself, for the code that needs an `Arc` to hand to a thread.
    pub(super) me: Weak<Daemon>,
    pub(super) cfg: Config,
    pub(super) state: Arc<Store>,
    pub(super) hw: Hw,
    pub(super) sensors: Arc<dyn Sensors>,
    pub(super) cpu_cap: Arc<dyn ClockCap>,
    pub(super) power: Arc<dyn PowerLimitsPort>,
    pub(super) cpu_undervolt: Arc<dyn CpuUndervoltPort>,
    pub(super) gpu_undervolt: Arc<dyn GpuUndervoltPort>,
    pub(super) power_managers: Arc<dyn PowerManagersPort>,
    pub(super) looks: Arc<dyn BootLookPort>,
    pub(super) keyboard: Arc<dyn OperatingLight>,
    pub(super) touchpad: Arc<dyn TouchpadPort>,
    pub(super) osd: Arc<dyn Notice>,
    pub(super) processes: Arc<dyn ProcessList>,
    pub(super) users: Arc<dyn UserDirectory>,
    pub(super) curves: ts_decision::curves::Store,
    pub(super) smart_profiles: ts_decision::smartprofile::Store,
    pub(super) live_paths: LivePaths,
    pub(super) notifier: Notifier,
    pub(super) watchdog_every: Option<Duration>,
    pub(super) clock: Arc<dyn Clock>,
    pub(super) sleeper: Arc<dyn Sleeper>,
    pub(super) log: Arc<dyn Logger>,
    pub(super) stop: StopToken,
    pub(super) inner: Mutex<Inner>,
    /// Threads the daemon started (the boot lighting); joined when it ends.
    pub(super) background: Mutex<Vec<JoinHandle<()>>>,
    /// When each throttled log line was last written (monotonic seconds).
    throttle: Mutex<HashMap<&'static str, f64>>,
    /// Set once `STOPPING=1` was sent, so a second `shutdown` does not repeat it.
    pub(super) stopping_sent: AtomicBool,
}

impl Daemon {
    /// Builds the daemon: loads the saved fan curve and Smart auto profile (a stored one that no
    /// longer validates is replaced by the built-in, with a line in the journal) and the held
    /// power-mode pick, and creates the engines. Nothing touches the hardware yet.
    pub fn new(deps: Deps) -> Arc<Daemon> {
        let Deps {
            config,
            state,
            hw,
            sensors,
            cpu_cap,
            power,
            cpu_undervolt,
            gpu_undervolt,
            power_managers,
            looks,
            keyboard,
            touchpad,
            osd,
            alert_settings,
            processes,
            users,
            curves,
            smart_profiles,
            uv_guard,
            gpu_uv_guard,
            live_paths,
            notifier,
            watchdog_every,
            clock,
            sleeper,
            log,
            stop,
        } = deps;
        let inner = Inner::build(
            &config,
            &state,
            &hw,
            &curves,
            &smart_profiles,
            uv_guard,
            gpu_uv_guard,
            alert_settings,
            log.as_ref(),
        );
        Arc::new_cyclic(|me| Daemon {
            me: me.clone(),
            cfg: config,
            state,
            hw,
            sensors,
            cpu_cap,
            power,
            cpu_undervolt,
            gpu_undervolt,
            power_managers,
            looks,
            keyboard,
            touchpad,
            osd,
            processes,
            users,
            curves,
            smart_profiles,
            live_paths,
            notifier,
            watchdog_every,
            clock,
            sleeper,
            log,
            stop,
            inner: Mutex::new(inner),
            background: Mutex::new(Vec::new()),
            throttle: Mutex::new(HashMap::new()),
            stopping_sent: AtomicBool::new(false),
        })
    }

    /// Takes the state lock, also when a panic left it poisoned: the data is valid (every change
    /// is a field assignment) and the fan fallback and the shutdown must still run.
    pub(super) fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    // ---- the settings the decisions depend on ----------------------------------------------------

    /// The automatic power mode selected in the settings.
    pub(super) fn auto_kind(&self) -> AutoKind {
        let mode = self.state.get("auto_mode");
        AutoKind::from_settings(
            mode.as_ref().and_then(Value::as_str),
            self.state.get_bool("auto_profile", false),
        )
    }

    /// The user's own CPU power limits `(pl1, pl2)` in watts, if set.
    pub(super) fn user_limits(&self) -> Option<(f64, f64)> {
        let stored = self.state.get("pl_user")?;
        let object = stored.as_object()?;
        Some((object.get("pl1")?.as_f64()?, object.get("pl2")?.as_f64()?))
    }

    /// Whether TrueSense manages the machine itself (a smart feature was chosen). Otherwise the
    /// firmware is left alone, like PredatorSense on Windows.
    pub(super) fn smart_active(&self) -> bool {
        self.state.get_bool("smart_fan", false)
            || self.auto_kind() == AutoKind::Smart
            || self.state.get("pl_user").is_some_and(|v| v.as_object().is_some_and(|o| !o.is_empty()))
    }

    /// The configured game rules (none when nothing, or something unusable, is stored).
    pub(super) fn game_rules(&self) -> Rules {
        self.state.get("game_rules").and_then(|v| Rules::from_json(&v).ok()).unwrap_or_default()
    }

    /// Whether a legacy flag file of the old Predator widgets (`~/.cache/<name>`) says another
    /// tool owns that feature. Only honoured with `[compat] predator_widgets`.
    pub(super) fn legacy(&self, name: &str) -> bool {
        if !self.cfg.compat.predator_widgets {
            return false;
        }
        let home = self.users.lookup(&self.cfg.user).map_or_else(|| "/root".to_owned(), |u| u.home);
        ts_foundation::fsx::exists(&PathBuf::from(home).join(".cache").join(name))
    }

    /// The uids that may use the control socket: the configured desktop user (root is always
    /// allowed by the server). An unknown user leaves root alone, as in the Python daemon.
    pub fn allowed_uids(&self) -> Vec<u32> {
        self.users.lookup(&self.cfg.user).map(|u| u.uid).into_iter().collect()
    }

    /// The pause the loop takes before the next tick.
    pub fn interval(&self) -> Duration {
        self.lock().interval
    }

    /// How many ticks have run.
    pub fn ticks(&self) -> u64 {
        self.lock().tick
    }

    // ---- persistence and the journal -------------------------------------------------------------

    /// Stores a setting. A disk that cannot be written means the setting holds for this boot only
    /// (the store keeps it in memory), which is worth one line a minute, not a failure.
    pub(super) fn save(&self, key: &str, value: impl Into<Value>) {
        if let Err(e) = self.state.set(key, value.into()) {
            self.warn_every("save", 60.0, format!("could not save the setting {key}: {e}"));
        }
    }

    pub(super) fn info(&self, message: impl AsRef<str>) {
        self.log.log(Level::Info, message.as_ref());
    }

    pub(super) fn warn(&self, message: impl AsRef<str>) {
        self.log.log(Level::Warn, message.as_ref());
    }

    /// A warning at most once every `seconds` per `key`: a failing device is retried every tick
    /// and must not fill the journal.
    pub(super) fn warn_every(&self, key: &'static str, seconds: f64, message: String) {
        let now = self.clock.mono();
        let mut last = self.throttle.lock().unwrap_or_else(PoisonError::into_inner);
        if last.get(key).is_some_and(|t| now - t < seconds) {
            return;
        }
        last.insert(key, now);
        drop(last);
        self.warn(message);
    }

    /// Shows an on-screen message at once (when the user wants them).
    pub(super) fn say(&self, text: &str, icon: &str) {
        if self.cfg.profile.osd {
            self.osd.show(text, icon);
        }
    }

    // ---- the loop ----------------------------------------------------------------------------------

    /// Serves the control socket and runs the control loop until the stop token is requested.
    ///
    /// The socket starts first (clients may connect while the boot settings are applied), then
    /// the boot sequence, then `READY=1`. Whatever ends the loop, the fans go back to the
    /// firmware and the clock cap is lifted first, then the socket is closed, then the
    /// hand-back is repeated for anything a request in flight did meanwhile.
    pub fn run(self: &Arc<Self>, socket: impl Into<PathBuf>) -> Result<(), DaemonError> {
        let config = ServerConfig::new(socket.into(), self.allowed_uids());
        let mut server = Server::start(config, self.router()).map_err(DaemonError::Socket)?;
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            self.boot();
            if let Err(e) = self.notifier.ready() {
                self.warn(format!("sd_notify READY failed: {e}"));
            }
            self.main_loop();
        }));
        self.shutdown();
        server.stop();
        self.shutdown();
        self.join_background();
        outcome.map_err(|payload| DaemonError::Panicked(super::step::panic_text(payload.as_ref())))
    }

    /// Runs ticks at the adaptive pace until the stop token is requested.
    fn main_loop(&self) {
        let mut next = self.clock.mono();
        let mut last_ping = f64::NEG_INFINITY;
        while !self.stop.is_requested() {
            self.guarded_step();
            let interval = self.interval();
            self.ping_watchdog(&mut last_ping, interval);
            next += interval.as_secs_f64();
            let now = self.clock.mono();
            if next < now - RESYNC_AFTER_S {
                // Woke from suspend: resync instead of racing to catch up.
                next = now;
                let mut g = self.lock();
                g.writer.forget();
                let tick = g.tick;
                g.governor.resync(tick);
            }
            let wait = Duration::try_from_secs_f64((next - now).max(0.0)).unwrap_or(Duration::ZERO);
            if self.sleeper.sleep(wait, &self.stop) {
                break;
            }
        }
    }

    /// Tells systemd the loop is alive: after every tick, or, when systemd gave an interval,
    /// whenever the next tick would otherwise be too late for it (half the configured timeout
    /// is what `sd_watchdog_enabled(3)` recommends, so there is room for one slow tick).
    fn ping_watchdog(&self, last_ping: &mut f64, interval: Duration) {
        let now = self.clock.mono();
        if let Some(every) = self.watchdog_every
            && now - *last_ping + interval.as_secs_f64() < every.as_secs_f64()
        {
            return;
        }
        *last_ping = now;
        if let Err(e) = self.notifier.watchdog() {
            self.warn_every("watchdog", 60.0, format!("sd_notify WATCHDOG failed: {e}"));
        }
    }

    /// Waits for the threads the daemon started (they stop quickly: the stop token is set).
    pub fn join_background(&self) {
        let handles: Vec<JoinHandle<()>> =
            std::mem::take(&mut *self.background.lock().unwrap_or_else(PoisonError::into_inner));
        for handle in handles {
            let _ = handle.join();
        }
    }
}

impl Inner {
    #[allow(clippy::too_many_arguments)]
    fn build(
        cfg: &Config,
        state: &Store,
        hw: &Hw,
        curves: &ts_decision::curves::Store,
        profiles: &ts_decision::smartprofile::Store,
        uv_guard: Guard,
        gpu_uv_guard: Guard,
        alert_settings: Arc<dyn AlertSettings>,
        log: &dyn Logger,
    ) -> Inner {
        let curve = curves
            .load_active(state.get("fan_curve_data").as_ref(), &state.get_str("fan_curve", "truesense"));
        if let Some(why) = &curve.fallback {
            log.log(Level::Warn, &format!("fan curve invalid, using the built-in: {why}"));
        }
        let profile = profiles
            .resolve(state.get("smart_profile_data").as_ref(), &state.get_str("smart_profile", "truesense"));
        if let Some(why) = &profile.problem {
            log.log(Level::Warn, &format!("smart profile invalid, using the built-in: {why}"));
        }
        let native = NativeModes {
            ac: Mode::parse(&cfg.native.ac).unwrap_or(Mode::Balanced),
            battery: Mode::parse(&cfg.native.battery).unwrap_or(Mode::Eco),
        };
        let mut engine = Engine::new(profile.profile, native);
        // A pick held when the daemon last stopped keeps running until its time is up.
        engine.restore_hold(state.get_f64("profile_hold_until", 0.0));
        Inner {
            tick: 0,
            tcpu: Ema::new(TEMP_ALPHA),
            tgpu: Ema::new(TEMP_ALPHA),
            tchassis: Ema::new(CHASSIS_ALPHA),
            rpm: (0, 0),
            classifier: Classifier::new(cfg.fan.hot_cpu as f64, cfg.fan.hot_gpu as f64),
            activity: Activity::Idle,
            hw_ok: true,
            last_ac: None,
            cur_mode: None,
            engine,
            fan: Smart::new(fanctl::Params::from_config(&cfg.fan), curve.curve),
            writer: FanWriter::new(HwFans(hw.clone())),
            stop_watch: StopWatchdog::new(),
            governor: Governor::new(thermal::Params::from(&cfg.cpu)),
            pl_orig: None,
            pl_touched: false,
            watcher: Watcher::new(),
            uv_guard,
            gpu_uv_guard,
            last_clock: None,
            op_mode: None,
            bind: BindRuntime::default(),
            live: LiveState::new(),
            interval: FIRST_INTERVAL,
            notices: Vec::new(),
            alerts: AlertsRuntime::new(alert_settings, &cfg.alerts),
        }
    }
}
