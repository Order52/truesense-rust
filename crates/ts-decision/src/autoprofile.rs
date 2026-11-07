//! The automatic power mode: what the laptop is doing (idle, everyday use, heavy work, gaming,
//! too hot) and which of the five power modes fits.
//!
//! This is a pure decision engine. Every input (temperatures, load, the monotonic and wall
//! clocks, the charger, the battery, the mode now) is passed in and every output is a
//! [`Decision`] the caller carries out, so the whole behaviour, thresholds, hysteresis and
//! hold times included, is tested at any speed with a fake clock.
//!
//! Three kinds exist ([`AutoKind`]): `native` follows the power source only (what PredatorSense
//! does on Windows), `smart` also watches heat, load and the battery, `off` does nothing.
//!
//! Why so many timers: a game on this laptop sits on the Turbo temperature line for minutes and
//! has menus and loading screens, and the first versions flapped between modes every few
//! seconds. Hence the stay-hot hysteresis of the classifier, the busy hold, the dwell times, the
//! minimum gap between changes and the long Turbo retry pause. All of them are in real seconds,
//! never in ticks, because the service ticks every one or two seconds.
#![forbid(unsafe_code)]

use crate::smartprofile::{Mode, Profile};

/// A smoothing step: the exponential moving average the service applies to temperatures
/// (alpha 0.4 for the CPU and GPU, 0.2 for the chassis) and to the GPU readings (0.3). `None`
/// means "no reading yet", which starts the average at the first value.
pub fn ema(prev: Option<f64>, value: f64, alpha: f64) -> f64 {
    match prev {
        None => value,
        Some(p) => p + alpha * (value - p),
    }
}

/// What the laptop is doing, from quiet to alarming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    /// Nothing much happening.
    Idle,
    /// Everyday use.
    Browsing,
    /// A heavy CPU load.
    Heavy,
    /// A game (the GPU is working).
    Gaming,
    /// Over the heat line.
    Hot,
}

impl Activity {
    /// The word the live state and the journal use.
    pub fn name(self) -> &'static str {
        match self {
            Activity::Idle => "idle",
            Activity::Browsing => "browsing",
            Activity::Heavy => "heavy",
            Activity::Gaming => "gaming",
            Activity::Hot => "hot",
        }
    }

    /// The coarse situation a hand-picked mode is held in: busy (gaming, heavy work, heat) or
    /// light. The hold ends when this changes.
    fn busy(self) -> bool {
        matches!(self, Activity::Gaming | Activity::Heavy | Activity::Hot)
    }
}

/// The smoothed measurements one classification is based on.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Readings {
    /// CPU temperature, degrees C.
    pub tcpu: f64,
    /// GPU temperature, degrees C.
    pub tgpu: f64,
    /// CPU load, percent.
    pub cpu_load: f64,
    /// GPU utilisation, percent.
    pub gpu_util: f64,
    /// GPU power draw, watts.
    pub gpu_power: f64,
}

/// The margin below the hot line at which "hot" ends, so a reading sitting on the line does not
/// flap between hot and not hot.
const HOT_RELEASE_C: f64 = 5.0;

/// Turns readings into an [`Activity`]. Once it says hot it keeps saying hot until both
/// temperatures are 5 degrees below their lines.
#[derive(Debug, Clone)]
pub struct Classifier {
    hot_cpu: f64,
    hot_gpu: f64,
    hot: bool,
}

impl Classifier {
    /// A classifier with the CPU and GPU hot lines (`[fan] hot_cpu` and `hot_gpu`).
    pub fn new(hot_cpu: f64, hot_gpu: f64) -> Classifier {
        Classifier { hot_cpu, hot_gpu, hot: false }
    }

    /// Changes the hot lines (a config reload) without forgetting whether it is hot now.
    pub fn set_limits(&mut self, hot_cpu: f64, hot_gpu: f64) {
        self.hot_cpu = hot_cpu;
        self.hot_gpu = hot_gpu;
    }

    /// Decides the activity for one set of readings.
    ///
    /// Gaming wins over heavy: the GPU at 35 W or 40 % means a game even when the CPU load is
    /// low (a frame-capped game). Idle needs both the CPU under 8 % and the GPU under 10 %.
    pub fn classify(&mut self, r: &Readings) -> Activity {
        let margin = if self.hot { HOT_RELEASE_C } else { 0.0 };
        self.hot = r.tcpu >= self.hot_cpu - margin || r.tgpu >= self.hot_gpu - margin;
        if self.hot {
            Activity::Hot
        } else if r.gpu_power >= 35.0 || r.gpu_util >= 40.0 {
            Activity::Gaming
        } else if r.cpu_load >= 55.0 {
            Activity::Heavy
        } else if r.cpu_load < 8.0 && r.gpu_util < 10.0 {
            Activity::Idle
        } else {
            Activity::Browsing
        }
    }
}

/// Which automatic mode is selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoKind {
    /// Hands off.
    Off,
    /// Follows the power source only.
    Native,
    /// Heat, load and battery aware.
    Smart,
}

impl AutoKind {
    /// The kind from the stored settings: `auto_mode` when it is one of the three words, else
    /// the older `auto_profile` flag (true means smart). The state file of an old install has
    /// only the flag.
    pub fn from_settings(auto_mode: Option<&str>, auto_profile: bool) -> AutoKind {
        match auto_mode {
            Some("off") => AutoKind::Off,
            Some("native") => AutoKind::Native,
            Some("smart") => AutoKind::Smart,
            _ if auto_profile => AutoKind::Smart,
            _ => AutoKind::Off,
        }
    }

    /// The word stored and shown (`off`, `native`, `smart`).
    pub fn name(self) -> &'static str {
        match self {
            AutoKind::Off => "off",
            AutoKind::Native => "native",
            AutoKind::Smart => "smart",
        }
    }
}

/// The power mode the firmware reports now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Current {
    /// Not read yet: nothing is decided until it is.
    Unread,
    /// A mode this engine does not know (the firmware answers `unknown(N)`): it ranks as
    /// Balanced and never equals a target, so the engine still moves away from it.
    Unrecognised,
    /// One of the five modes.
    Known(Mode),
}

impl Current {
    /// The rank used when comparing with a target (an unrecognised mode counts as Balanced).
    fn rank(self) -> u8 {
        match self {
            Current::Known(m) => m.rank(),
            Current::Unread | Current::Unrecognised => Mode::Balanced.rank(),
        }
    }
}

/// The modes `native` auto uses (`[native] ac` and `battery`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeModes {
    /// Mode on power.
    pub ac: Mode,
    /// Mode on battery.
    pub battery: Mode,
}

impl Default for NativeModes {
    fn default() -> Self {
        NativeModes { ac: Mode::Balanced, battery: Mode::Eco }
    }
}

/// Everything one decision looks at.
#[derive(Debug, Clone, Copy)]
pub struct Inputs {
    /// Which automatic mode is selected.
    pub kind: AutoKind,
    /// A monotonic clock in seconds (any origin): every timer here is measured on it.
    pub now: f64,
    /// Unix time in seconds: only the hold is measured on it, because the hold is stored and
    /// must survive a restart.
    pub wall: f64,
    /// The plain activity (the fans use the same classification).
    pub activity: Activity,
    /// Smoothed CPU temperature.
    pub tcpu: f64,
    /// Smoothed GPU temperature.
    pub tgpu: f64,
    /// Whether the charger is plugged in.
    pub ac: bool,
    /// Battery charge in percent (100 when there is no battery).
    pub battery_pct: f64,
    /// The mode the firmware is in.
    pub cur_mode: Current,
    /// The mode a running game rule asks for, if any.
    pub game: Option<Mode>,
    /// Another tool owns the mode (the legacy `predator-auto-off` flag file exists).
    pub disabled: bool,
}

/// What the caller should do after one [`Engine::step`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// The mode to set, or `None` when nothing is to be changed.
    pub mode: Option<Mode>,
    /// The change cannot wait (thermal back-off, empty battery, power source): it skipped the
    /// dwell timers. Pass it to [`Engine::applied`].
    pub urgent: bool,
    /// Why, in words, for the journal and the live state (`mode_reason`).
    pub reason: String,
}

impl Decision {
    fn none(reason: &str) -> Decision {
        Decision { mode: None, urgent: false, reason: reason.to_owned() }
    }
}

/// A hand-picked mode outranks the automatic one until the situation it was picked in changes.
#[derive(Debug, Clone, Copy)]
struct HoldContext {
    ac: bool,
    busy: bool,
}

/// The state kept between decisions.
#[derive(Debug, Clone)]
pub struct Engine {
    profile: Profile,
    native: NativeModes,
    busy_kind: Option<Activity>,
    busy_strong: f64,
    t_prev: Option<f64>,
    heavy_for: f64,
    idle_for: f64,
    turbo_block: f64,
    mode_changed: f64,
    last_try: f64,
    last_say: f64,
    last_target: Option<Mode>,
    /// A decision was issued and the caller has not confirmed it with [`Engine::applied`].
    attempt_open: bool,
    /// How many times in a row the same target was tried again after it failed.
    retries: u32,
    /// The battery counts as low (with the release margin above the line).
    low_battery: bool,
    latched: bool,
    hold_until: f64,
    hold_ctx: Option<HoldContext>,
    reason: String,
}

/// "Never": the timers that start in the far past so the first change is not held back.
const LONG_AGO: f64 = -1e9;
/// A target that failed to apply is not tried again for this long; every further failure of the
/// same target doubles the wait, up to [`RETRY_DOUBLINGS`] times. A mode the firmware keeps
/// refusing (one this BIOS does not offer) would otherwise be written every 30 seconds for as
/// long as the daemon runs, and every power-mode write makes the firmware blink the mode key.
const RETRY_AFTER_S: f64 = 30.0;
/// How many times the retry wait doubles (30, 60, 120, 240, 480 s).
const RETRY_DOUBLINGS: u32 = 4;
/// Once the battery has counted as low (below `battery_low_pct`) it only stops counting as low
/// this many points above the line, or on power: a gauge that wobbles by a point under load
/// would otherwise flip the mode between Eco and the normal battery mode at the line.
const LOW_RELEASE_PCT: f64 = 3.0;
/// At most one on-screen notice this often (the journal has every change).
const NOTICE_EVERY_S: f64 = 30.0;
/// The longest time step counted as elapsed: a stalled loop or a resume must not count as
/// minutes of heavy load.
const MAX_STEP_S: f64 = 5.0;
/// A game rule never raises the mode when the battery is below this percentage.
const GAME_MIN_BATTERY_PCT: f64 = 20.0;

impl Engine {
    /// An engine with the given Smart settings and native modes and no history.
    pub fn new(profile: Profile, native: NativeModes) -> Engine {
        Engine {
            profile,
            native,
            busy_kind: None,
            busy_strong: LONG_AGO,
            t_prev: None,
            heavy_for: 0.0,
            idle_for: 0.0,
            turbo_block: 0.0,
            mode_changed: LONG_AGO,
            last_try: LONG_AGO,
            last_say: LONG_AGO,
            last_target: None,
            attempt_open: false,
            retries: 0,
            low_battery: false,
            latched: false,
            hold_until: 0.0,
            hold_ctx: None,
            reason: String::new(),
        }
    }

    /// The Smart settings in use.
    pub fn profile(&self) -> &Profile {
        &self.profile
    }

    /// Switches the Smart settings (the user chose another profile). The timers and the hold
    /// carry over: only the thresholds change.
    pub fn set_profile(&mut self, profile: Profile) {
        self.profile = profile;
    }

    /// Changes the modes native auto uses (a config reload).
    pub fn set_native(&mut self, native: NativeModes) {
        self.native = native;
    }

    /// Why the last decision was taken, as the live state shows it (`mode_reason`).
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// How long the load has been heavy, in seconds (the journal prints it).
    pub fn heavy_for(&self) -> f64 {
        self.heavy_for
    }

    /// The unix time the user's own pick stays in force, 0 when there is no hold. The caller
    /// stores it (`profile_hold_until`) whenever it changed, so it survives a restart.
    pub fn hold_until(&self) -> f64 {
        self.hold_until
    }

    /// Seconds of hold left, for the `state` reply.
    pub fn hold_left_s(&self, wall: f64) -> u64 {
        (self.hold_until - wall).max(0.0) as u64
    }

    /// Sets a hold read back from the stored state at start-up. It has no situation to break
    /// on (the situation was lost with the old process), so it runs until its time is up.
    pub fn restore_hold(&mut self, until: f64) {
        self.hold_until = until;
        self.hold_ctx = None;
    }

    /// Makes the user's own pick override the automatic mode for `minutes` (0 or less clears
    /// it). The hold ends early when the situation it was picked in changes: plug or unplug,
    /// the load starts or stops, heat, or a low battery. Returns the minutes held.
    pub fn set_hold(&mut self, wall: f64, minutes: f64, ac: bool, activity: Activity) -> f64 {
        if minutes <= 0.0 {
            self.hold_until = 0.0;
            self.hold_ctx = None;
            return 0.0;
        }
        self.hold_until = wall + minutes * 60.0;
        self.hold_ctx = Some(HoldContext { ac, busy: activity.busy() });
        minutes
    }

    /// Choosing an automatic kind means "take over now": forget the hold and the timers so the
    /// next decision is not held back by an old pick or a recent change.
    pub fn take_over_now(&mut self) {
        self.hold_until = 0.0;
        self.hold_ctx = None;
        self.mode_changed = LONG_AGO;
        self.last_target = None;
        self.heavy_for = 0.0;
    }

    /// Forgets what was last tried: a plug or unplug can reset the driver state, and a changed
    /// game rule changes what is wanted, so the next decision starts fresh.
    pub fn resync(&mut self) {
        self.last_target = None;
    }

    /// Records that the mode was really set (the caller calls this only on success, a failed
    /// write is retried after 30 s without counting as a change). Returns whether to show an
    /// on-screen notice: at most one every 30 s, or at once for an urgent change.
    pub fn applied(&mut self, now: f64, urgent: bool) -> bool {
        self.mode_changed = now;
        self.attempt_open = false;
        self.retries = 0;
        if now - self.last_say >= NOTICE_EVERY_S || urgent {
            self.last_say = now;
            true
        } else {
            false
        }
    }

    /// Makes one decision. The caller sets the mode when `mode` is `Some` and calls
    /// [`Engine::applied`] if that worked. Nothing is decided while the kind is off, another
    /// tool owns the mode, the current mode is unread, or a hold is in force.
    pub fn step(&mut self, input: &Inputs) -> Decision {
        if input.kind == AutoKind::Off || input.disabled || input.cur_mode == Current::Unread {
            return Decision::none(&self.reason);
        }
        if input.wall < self.hold_until && !self.hold_broken(input) {
            return Decision::none(&self.reason); // your own pick is still in force
        }
        let (mut target, urgent) = if input.kind == AutoKind::Native {
            // Follows the plug at once.
            (if input.ac { self.native.ac } else { self.native.battery }, true)
        } else {
            self.smart_target(input)
        };
        if let Some(mut g) = input.game.filter(|g| *g > target) {
            // A configured game is running: it can only raise the mode.
            if input.activity == Activity::Hot && g == Mode::Turbo {
                g = Mode::Performance; // the same thermal back-off as Smart auto
            }
            if !input.ac && input.battery_pct < GAME_MIN_BATTERY_PCT {
                // A nearly empty battery wins over a game.
                g = target;
            }
            target = g;
        }
        self.reason = self.describe(input);
        let reason = self.reason.clone();
        if input.cur_mode == Current::Known(target) {
            self.last_target = Some(target);
            self.attempt_open = false;
            self.retries = 0;
            return Decision::none(&reason);
        }
        let now = input.now;
        let wait = RETRY_AFTER_S * f64::from(1u32 << self.retries.min(RETRY_DOUBLINGS));
        if self.last_target == Some(target) && now - self.last_try < wait {
            return Decision::none(&reason); // it already failed recently
        }
        let up = target.rank() > input.cur_mode.rank();
        let dwell = f64::from(if up { self.profile.up_dwell_s } else { self.profile.down_dwell_s });
        let since = now - self.mode_changed;
        let flapping =
            since < dwell || (input.kind == AutoKind::Smart && since < f64::from(self.profile.min_change_s));
        if !urgent && flapping {
            return Decision::none(&reason);
        }
        // The same target again with the last attempt never confirmed: that attempt failed.
        self.retries = if self.attempt_open && self.last_target == Some(target) {
            self.retries.saturating_add(1)
        } else {
            0
        };
        self.attempt_open = true;
        self.last_target = Some(target);
        self.last_try = now;
        Decision { mode: Some(target), urgent, reason }
    }

    /// The journal text: the activity, the power source, and for Smart the seconds of heavy
    /// load and whether a busy mode is being held through a quiet moment.
    fn describe(&self, input: &Inputs) -> String {
        let mut s =
            format!("{}, {}", input.activity.name(), if input.ac { "on power" } else { "on battery" });
        if input.kind == AutoKind::Smart && self.heavy_for != 0.0 {
            s.push_str(&format!(", {} s heavy", self.heavy_for as i64));
        }
        if input.kind == AutoKind::Smart && self.latched {
            s.push_str(" (holding the busy mode through a quiet moment)");
        }
        s
    }

    /// True (and the hold is dropped) once the situation the pick was made in is over.
    fn hold_broken(&mut self, input: &Inputs) -> bool {
        let Some(ctx) = self.hold_ctx else { return false };
        let low = !input.ac && input.battery_pct < f64::from(self.profile.battery_low_pct);
        if ctx.ac != input.ac || ctx.busy != input.activity.busy() || input.activity == Activity::Hot || low {
            self.hold_until = 0.0;
            self.hold_ctx = None;
            self.mode_changed = LONG_AGO;
            self.last_target = None;
            return true;
        }
        false
    }

    /// For the power-mode decision only: gaming and heavy start at once but end only after
    /// `busy_hold_s` seconds without either, so a menu, a loading screen or a frame cap does not
    /// look like browsing and drop the mode. (The fans keep using the plain classification.)
    /// Hot is never held and does not forget the busy kind.
    fn busy_latched(&mut self, a: Activity, now: f64, hold: f64) -> Activity {
        if matches!(a, Activity::Gaming | Activity::Heavy) {
            self.busy_kind = Some(a);
            self.busy_strong = now;
            return a;
        }
        if let Some(kind) = self.busy_kind {
            if a != Activity::Hot && hold > 0.0 && now - self.busy_strong < hold {
                return kind;
            }
        }
        if a != Activity::Hot {
            self.busy_kind = None;
        }
        a
    }

    /// Smart auto: picks the mode from what the laptop is doing and how hot it is. Returns the
    /// mode and whether the change is urgent (it then skips the dwell timers: a thermal
    /// back-off or an empty battery cannot wait).
    fn smart_target(&mut self, input: &Inputs) -> (Mode, bool) {
        let s = self.profile.clone();
        let now = input.now;
        let a = self.busy_latched(input.activity, now, f64::from(s.busy_hold_s));
        self.latched = a != input.activity;
        let dt = self.t_prev.map_or(0.0, |p| (now - p).clamp(0.0, MAX_STEP_S));
        self.t_prev = Some(now);
        let heavy = matches!(a, Activity::Gaming | Activity::Heavy);
        let hot = a == Activity::Hot;
        self.heavy_for = if heavy { self.heavy_for + dt } else { (self.heavy_for - dt).max(0.0) };
        self.idle_for = if a == Activity::Idle { self.idle_for + dt } else { 0.0 };
        if hot {
            self.turbo_block = now + f64::from(s.backoff_hold_s);
        }
        if input.ac {
            self.low_battery = false;
            if heavy || hot {
                let mut t = s.ac_busy;
                if hot && input.cur_mode == Current::Known(Mode::Turbo) {
                    self.turbo_block = self.turbo_block.max(now + f64::from(s.turbo_retry_s));
                    return (Mode::Performance, true); // thermal back-off, at once
                }
                if s.ac_turbo {
                    let cpu_cap = f64::from(s.turbo_max_cpu);
                    let gpu_cap = f64::from(s.turbo_max_gpu);
                    if input.cur_mode == Current::Known(Mode::Turbo) {
                        // Already in Turbo: stay until it really reaches the limit (no margin to
                        // LEAVE, or a CPU sitting near the entry temperature would flap).
                        if input.tcpu < cpu_cap && input.tgpu < gpu_cap {
                            t = Mode::Turbo;
                        } else {
                            // Too hot: step down and do not retry for a while.
                            self.turbo_block = self.turbo_block.max(now + f64::from(s.turbo_retry_s));
                            return (Mode::Performance, true);
                        }
                    } else {
                        // To ENTER Turbo: well below the limit, after sustained load, and not
                        // during a back-off. The plain activity counts here, not the latched
                        // one: a quiet moment never starts Turbo.
                        let margin = f64::from(s.turbo_margin_c);
                        if matches!(input.activity, Activity::Gaming | Activity::Heavy)
                            && self.heavy_for >= f64::from(s.turbo_after_s)
                            && input.tcpu < cpu_cap - margin
                            && input.tgpu < gpu_cap - margin
                            && now >= self.turbo_block
                        {
                            t = Mode::Turbo;
                        }
                    }
                }
                return (t, false);
            }
            if a == Activity::Browsing || self.idle_for < f64::from(s.idle_after_s) {
                return (s.ac_browsing, false);
            }
            return (s.ac_idle, false);
        }
        let line = f64::from(s.battery_low_pct);
        self.low_battery = input.battery_pct < if self.low_battery { line + LOW_RELEASE_PCT } else { line };
        if self.low_battery {
            return (Mode::Eco, true);
        }
        if hot {
            return (s.battery_hot, true);
        }
        if heavy {
            return (s.battery_busy, false);
        }
        (if a == Activity::Browsing { s.battery_browsing } else { s.battery_idle }, false)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::smartprofile::builtin;

    fn profile(name: &str) -> Profile {
        builtin(name).expect("built-in profile")
    }

    /// Drives the engine the way the daemon's loop does: smoothed temperatures, the
    /// classification, one decision per tick, the decision carried out and recorded. The
    /// clocks are fields the test advances, so a simulated hour runs in microseconds.
    struct Laptop {
        engine: Engine,
        classifier: Classifier,
        tcpu: Option<f64>,
        tgpu: Option<f64>,
        mode: Current,
        now: f64,
        wall: f64,
        ac: bool,
        battery: f64,
        kind: AutoKind,
        game: Option<Mode>,
        disabled: bool,
        log: Vec<Decision>,
        notices: u32,
        /// The activity as the last tick classified it.
        activity: Activity,
        /// Readings the test sets, like the Python tests set `b.cpu_load` directly.
        gpu_util: f64,
        gpu_power: f64,
    }

    impl Laptop {
        fn new(p: Profile) -> Laptop {
            Laptop {
                engine: Engine::new(p, NativeModes::default()),
                classifier: Classifier::new(92.0, 82.0),
                tcpu: None,
                tgpu: None,
                mode: Current::Known(Mode::Eco),
                now: 0.0,
                wall: 1_000_000.0,
                ac: true,
                battery: 80.0,
                kind: AutoKind::Smart,
                game: None,
                disabled: false,
                log: Vec::new(),
                notices: 0,
                activity: Activity::Idle,
                gpu_util: 0.0,
                gpu_power: 0.0,
            }
        }

        fn mode(&self) -> Mode {
            match self.mode {
                Current::Known(m) => m,
                other => panic!("mode is {other:?}"),
            }
        }

        /// One tick of `dt` seconds with the given raw temperatures and loads. Returns the new
        /// mode when it changed.
        fn tick(
            &mut self,
            dt: f64,
            cpu_t: f64,
            gpu_t: f64,
            load: f64,
            gpu_util: f64,
            gpu_power: f64,
        ) -> Option<Mode> {
            self.now += dt;
            self.wall += dt;
            let tcpu = ema(self.tcpu, cpu_t, 0.4);
            let tgpu = ema(self.tgpu, gpu_t, 0.4);
            self.tcpu = Some(tcpu);
            self.tgpu = Some(tgpu);
            let r = Readings { tcpu, tgpu, cpu_load: load, gpu_util, gpu_power };
            self.activity = self.classifier.classify(&r);
            let d = self.engine.step(&Inputs {
                kind: self.kind,
                now: self.now,
                wall: self.wall,
                activity: self.activity,
                tcpu,
                tgpu,
                ac: self.ac,
                battery_pct: self.battery,
                cur_mode: self.mode,
                game: self.game,
                disabled: self.disabled,
            });
            self.log.push(d.clone());
            let m = d.mode?;
            self.mode = Current::Known(m);
            if self.engine.applied(self.now, d.urgent) {
                self.notices += 1;
            }
            Some(m)
        }

        /// The Python `drive`: `n` ticks of one second at constant readings (GPU idle).
        fn drive(&mut self, n: u32, cpu: f64, gpu: f64, load: f64) {
            for _ in 0..n {
                self.tick(1.0, cpu, gpu, load, self.gpu_util, self.gpu_power);
            }
        }

        /// The Python `play`: `seconds` of game time (GPU 65 C, 16 % CPU load, GPU at 98 % and
        /// 71 W unless told otherwise); returns every (time, mode) change.
        fn play(&mut self, seconds: f64, cpu_of: impl Fn(f64) -> f64, o: Play) -> Vec<(f64, Mode)> {
            let mut out = Vec::new();
            let mut t = 0.0;
            while t < seconds {
                if let Some(m) = self.tick(o.dt, cpu_of(t).round(), o.gpu, o.load, o.gpu_util, o.gpu_power) {
                    out.push((self.now, m));
                }
                t += o.dt;
            }
            out
        }

        /// Idle on power at 40 C for 70 s: the starting point of the scenarios.
        fn settled(p: Profile) -> Laptop {
            let want = p.ac_idle;
            let mut l = Laptop::new(p);
            for _ in 0..70 {
                l.tick(1.0, 40.0, 40.0, 5.0, 5.0, 5.0);
            }
            assert_eq!(l.mode(), want, "idle on power must settle on the idle mode");
            l
        }
    }

    #[derive(Clone, Copy)]
    struct Play {
        gpu: f64,
        load: f64,
        gpu_util: f64,
        gpu_power: f64,
        dt: f64,
    }

    const GAME: Play = Play { gpu: 65.0, load: 16.0, gpu_util: 98.0, gpu_power: 71.0, dt: 1.0 };
    /// A menu or a loading screen: the GPU nearly idle.
    const MENU: Play = Play { load: 20.0, gpu_util: 15.0, gpu_power: 18.0, ..GAME };
    /// Really quiet.
    const CALM: Play = Play { load: 3.0, gpu_util: 2.0, gpu_power: 5.0, ..GAME };
    /// A CPU-heavy job with the GPU idle (the Python `drive(b, n, 70, 60, 90)`).
    const CRUNCH: Play = Play { gpu: 60.0, load: 90.0, gpu_util: 0.0, gpu_power: 0.0, ..GAME };

    fn constant(v: f64) -> impl Fn(f64) -> f64 {
        move |_| v
    }

    fn rd(cpu: f64, gpu: f64, load: f64, util: f64, power: f64) -> Readings {
        Readings { tcpu: cpu, tgpu: gpu, cpu_load: load, gpu_util: util, gpu_power: power }
    }

    #[test]
    fn ema_starts_at_the_first_value_then_moves_by_alpha() {
        assert_eq!(ema(None, 50.0, 0.4), 50.0);
        assert!((ema(Some(50.0), 100.0, 0.4) - 70.0).abs() < 1e-12);
        assert!((ema(Some(70.0), 40.0, 0.25) - 62.5).abs() < 1e-12);
    }

    #[test]
    fn classifier_table() {
        let cases = [
            ("idle", rd(40.0, 40.0, 2.0, 1.0, 0.0), Activity::Idle),
            ("browsing", rd(50.0, 45.0, 20.0, 15.0, 0.0), Activity::Browsing),
            ("heavy by cpu", rd(70.0, 50.0, 80.0, 0.0, 0.0), Activity::Heavy),
            ("gaming by gpu util", rd(60.0, 60.0, 10.0, 70.0, 0.0), Activity::Gaming),
            ("gaming by gpu power", rd(60.0, 60.0, 0.0, 0.0, 40.0), Activity::Gaming),
            ("hot", rd(93.0, 60.0, 99.0, 0.0, 0.0), Activity::Hot),
            ("hot by gpu", rd(40.0, 82.0, 1.0, 0.0, 0.0), Activity::Hot),
            // exact edges
            ("cpu load 55 is heavy", rd(50.0, 50.0, 55.0, 0.0, 0.0), Activity::Heavy),
            ("cpu load 54.9 is browsing", rd(50.0, 50.0, 54.9, 0.0, 0.0), Activity::Browsing),
            ("gpu power 35 is gaming", rd(50.0, 50.0, 0.0, 0.0, 35.0), Activity::Gaming),
            ("gpu power 34.9 is not", rd(50.0, 50.0, 0.0, 0.0, 34.9), Activity::Idle),
            ("gpu util 40 is gaming", rd(50.0, 50.0, 0.0, 40.0, 0.0), Activity::Gaming),
            ("gpu util 39.9 is browsing", rd(50.0, 50.0, 0.0, 39.9, 0.0), Activity::Browsing),
            ("cpu load 8 is browsing", rd(50.0, 50.0, 8.0, 0.0, 0.0), Activity::Browsing),
            ("cpu load 7.9 is idle", rd(50.0, 50.0, 7.9, 9.9, 0.0), Activity::Idle),
            ("gpu util 10 is browsing", rd(50.0, 50.0, 7.9, 10.0, 0.0), Activity::Browsing),
            ("gaming beats heavy", rd(50.0, 50.0, 99.0, 40.0, 0.0), Activity::Gaming),
            ("hot beats gaming", rd(92.0, 50.0, 99.0, 99.0, 99.0), Activity::Hot),
        ];
        for (name, r, want) in cases {
            let mut c = Classifier::new(92.0, 82.0);
            assert_eq!(c.classify(&r), want, "{name}");
        }
    }

    #[test]
    fn once_hot_it_stays_hot_until_five_degrees_below_both_lines() {
        let mut c = Classifier::new(92.0, 82.0);
        assert_eq!(c.classify(&rd(92.0, 0.0, 99.0, 0.0, 0.0)), Activity::Hot, "at the line it is hot");
        assert_eq!(
            c.classify(&rd(87.0, 0.0, 99.0, 0.0, 0.0)),
            Activity::Hot,
            "exactly 5 below the line: still hot"
        );
        assert_eq!(
            c.classify(&rd(86.9, 0.0, 99.0, 0.0, 0.0)),
            Activity::Heavy,
            "more than 5 below it cools down"
        );
        assert_eq!(
            c.classify(&rd(89.0, 0.0, 99.0, 0.0, 0.0)),
            Activity::Heavy,
            "and 89 is not hot again (the line is 92)"
        );
        c.classify(&rd(0.0, 82.0, 0.0, 0.0, 0.0));
        assert_eq!(
            c.classify(&rd(0.0, 77.0, 0.0, 0.0, 0.0)),
            Activity::Hot,
            "the GPU line releases 5 below too"
        );
        assert_eq!(c.classify(&rd(86.0, 76.9, 0.0, 0.0, 0.0)), Activity::Idle);
        c.set_limits(60.0, 60.0);
        assert_eq!(c.classify(&rd(61.0, 0.0, 0.0, 0.0, 0.0)), Activity::Hot);
    }

    #[test]
    fn auto_kind_follows_auto_mode_else_the_old_flag() {
        assert_eq!(AutoKind::from_settings(Some("smart"), false), AutoKind::Smart);
        assert_eq!(AutoKind::from_settings(Some("native"), true), AutoKind::Native);
        assert_eq!(AutoKind::from_settings(Some("off"), true), AutoKind::Off);
        assert_eq!(AutoKind::from_settings(None, true), AutoKind::Smart);
        assert_eq!(AutoKind::from_settings(None, false), AutoKind::Off);
        assert_eq!(AutoKind::from_settings(Some("bogus"), true), AutoKind::Smart);
        assert_eq!(AutoKind::from_settings(Some("bogus"), false), AutoKind::Off);
        assert_eq!(AutoKind::Smart.name(), "smart");
    }

    #[test]
    fn native_follows_the_power_source_only() {
        let mut l = Laptop::new(profile("truesense"));
        l.kind = AutoKind::Native;
        l.tick(1.0, 40.0, 40.0, 95.0, 0.0, 0.0);
        assert_eq!(l.mode(), Mode::Balanced, "on AC: balanced, even under load (no heat or load logic)");
        l.ac = false;
        l.tick(1.0, 40.0, 40.0, 95.0, 0.0, 0.0);
        assert_eq!(l.mode(), Mode::Eco, "on battery: eco");
        l.tick(1.0, 99.0, 99.0, 99.0, 99.0, 99.0);
        assert_eq!(l.mode(), Mode::Eco, "heat is not looked at");
        l.engine.set_native(NativeModes { ac: Mode::Performance, battery: Mode::Quiet });
        l.tick(1.0, 40.0, 40.0, 5.0, 0.0, 0.0);
        assert_eq!(l.mode(), Mode::Quiet);
    }

    #[test]
    fn native_changes_at_once_whatever_the_dwell_times() {
        let mut l = Laptop::new(profile("steady")); // 180 s dwell and minimum gap
        l.kind = AutoKind::Native;
        l.tick(1.0, 40.0, 40.0, 5.0, 0.0, 0.0);
        l.ac = false;
        assert_eq!(
            l.tick(1.0, 40.0, 40.0, 5.0, 0.0, 0.0),
            Some(Mode::Eco),
            "urgent: the plug is followed immediately"
        );
        l.ac = true;
        assert_eq!(l.tick(1.0, 40.0, 40.0, 5.0, 0.0, 0.0), Some(Mode::Balanced));
    }

    #[test]
    fn off_and_disabled_and_unread_decide_nothing() {
        for mutate in [
            (|l: &mut Laptop| l.kind = AutoKind::Off) as fn(&mut Laptop),
            |l| l.disabled = true,
            |l| l.mode = Current::Unread,
        ] {
            let mut l = Laptop::new(profile("truesense"));
            mutate(&mut l);
            assert_eq!(l.tick(1.0, 99.0, 99.0, 99.0, 99.0, 99.0), None);
            assert!(l.engine.reason().is_empty(), "nothing was evaluated");
        }
    }

    #[test]
    fn smart_escalates_and_backs_off_for_heat() {
        let mut l = Laptop::settled(profile("truesense"));
        l.play(10.0, constant(70.0), CRUNCH);
        assert_eq!(l.mode(), Mode::Performance, "heavy load: performance first");
        l.play(70.0, constant(70.0), CRUNCH);
        assert_eq!(
            l.mode(),
            Mode::Turbo,
            "sustained, cool enough and not changed for a while (45 s minimum between changes): turbo"
        );
        l.play(10.0, constant(93.0), Play { gpu: 70.0, load: 95.0, ..CRUNCH });
        assert_eq!(l.mode(), Mode::Performance, "hot: back off from turbo at once");
        l.play(40.0, constant(70.0), CRUNCH);
        assert_eq!(l.mode(), Mode::Performance, "and stay there during the back-off hold");
        l.play(100.0, constant(40.0), Play { gpu: 40.0, load: 3.0, ..CALM });
        assert!(matches!(l.mode(), Mode::Balanced | Mode::Performance));
    }

    #[test]
    fn smart_on_battery_saves_power() {
        let mut l = Laptop::new(profile("truesense"));
        l.ac = false;
        l.drive(40, 40.0, 40.0, 4.0);
        assert_eq!(l.mode(), Mode::Eco, "idle on battery: eco");
        l.drive(40, 70.0, 60.0, 90.0);
        assert_eq!(l.mode(), Mode::Balanced, "busy on battery: balanced, not turbo");
        l.battery = 10.0;
        l.tick(1.0, 40.0, 40.0, 4.0, 4.0, 4.0);
        assert_eq!(l.mode(), Mode::Eco, "a nearly empty battery forces eco at once");
    }

    #[test]
    fn battery_edge_is_strictly_below_the_profile_level() {
        let mut l = Laptop::new(profile("truesense"));
        l.ac = false;
        l.battery = 20.0; // battery_low_pct is 20: at 20 it is not low yet
        l.mode = Current::Known(Mode::Performance);
        l.engine.applied(l.now, false); // a change has just happened
        assert_eq!(
            l.tick(1.0, 40.0, 40.0, 4.0, 0.0, 0.0),
            None,
            "20 % is not low: the down dwell (60 s) still applies"
        );
        l.battery = 19.9;
        assert_eq!(l.tick(1.0, 40.0, 40.0, 4.0, 0.0, 0.0), Some(Mode::Eco), "19.9 % is low: eco, urgent");
        assert!(l.log.last().expect("logged").urgent);
    }

    #[test]
    fn a_low_battery_is_released_only_a_few_points_above_the_line() {
        let mut l = Laptop::new(profile("truesense"));
        l.ac = false;
        l.drive(40, 70.0, 60.0, 90.0);
        assert_eq!(l.mode(), Mode::Balanced, "busy on battery: balanced");
        l.battery = 19.9;
        l.drive(1, 70.0, 60.0, 90.0);
        assert_eq!(l.mode(), Mode::Eco, "below the line: eco at once");
        // The gauge wobbles by a point or two under load, as gauges do.
        for pct in [20.4, 19.8, 20.9, 21.5, 22.9, 21.0] {
            l.battery = pct;
            l.drive(30, 70.0, 60.0, 90.0);
            assert_eq!(l.mode(), Mode::Eco, "{pct} % is inside the release margin: no flip back");
        }
        l.battery = 23.0;
        l.drive(70, 70.0, 60.0, 90.0);
        assert_eq!(l.mode(), Mode::Balanced, "three points above the line it is released");
        l.battery = 21.0;
        l.drive(70, 70.0, 60.0, 90.0);
        assert_eq!(
            l.mode(),
            Mode::Balanced,
            "and below the release margin but above the line it is not low again"
        );
        l.battery = 19.9;
        l.drive(1, 70.0, 60.0, 90.0);
        assert_eq!(l.mode(), Mode::Eco, "a new crossing of the line");
    }

    #[test]
    fn plugging_in_clears_the_low_battery_latch() {
        let mut l = Laptop::new(profile("truesense"));
        l.ac = false;
        l.battery = 10.0;
        l.drive(5, 40.0, 40.0, 4.0);
        assert_eq!(l.mode(), Mode::Eco);
        l.ac = true;
        l.drive(70, 40.0, 40.0, 4.0);
        assert_eq!(l.mode(), Mode::Balanced, "on power the latch is gone");
        l.ac = false;
        l.battery = 22.0;
        l.drive(70, 70.0, 60.0, 90.0);
        assert_eq!(l.mode(), Mode::Balanced, "22 % on battery after a re-plug is not low (the line is 20)");
    }

    /// Drives a native-auto engine whose writes always fail (the firmware refuses the mode) and
    /// returns the seconds at which it asked for a write.
    fn attempts_of_a_refused_mode(seconds: u32, confirm_at: Option<u32>) -> Vec<u32> {
        let mut e = Engine::new(profile("truesense"), NativeModes::default());
        let mut asked = Vec::new();
        for t in 1..=seconds {
            let d = e.step(&Inputs {
                kind: AutoKind::Native,
                now: f64::from(t),
                wall: 1_000_000.0 + f64::from(t),
                activity: Activity::Idle,
                tcpu: 40.0,
                tgpu: 40.0,
                ac: true,
                battery_pct: 80.0,
                cur_mode: Current::Known(Mode::Eco),
                game: None,
                disabled: false,
            });
            if d.mode.is_some() {
                asked.push(t);
                if confirm_at == Some(t) {
                    e.applied(f64::from(t), d.urgent);
                }
            }
        }
        asked
    }

    #[test]
    fn a_mode_the_firmware_keeps_refusing_is_retried_ever_more_rarely() {
        let asked = attempts_of_a_refused_mode(3600, None);
        let gaps: Vec<u32> = asked.windows(2).map(|w| w[1] - w[0]).collect();
        assert_eq!(&gaps[..6], [30, 60, 120, 240, 480, 480], "{asked:?}");
        assert!(gaps.iter().all(|g| *g <= 480), "never rarer than every eight minutes: {gaps:?}");
        assert!(asked.len() <= 12, "{} writes in an hour, not 120", asked.len());
    }

    #[test]
    fn a_confirmed_write_resets_the_retry_wait() {
        // Two refusals (waits 30 then 60), then the third attempt works and `applied` is called.
        let asked = attempts_of_a_refused_mode(1000, Some(1 + 30 + 60));
        assert_eq!(&asked[..3], [1, 31, 91]);
        // The mode never actually changes in this harness, so the next ask is the post-success
        // grace of 30 s, not the doubled wait the refusals had built up.
        assert_eq!(asked[3] - asked[2], 30, "{asked:?}");
    }

    #[test]
    fn hot_on_battery_uses_the_battery_hot_mode_at_once() {
        let mut l = Laptop::new(profile("truesense"));
        l.ac = false;
        l.drive(3, 40.0, 40.0, 4.0);
        l.tcpu = Some(99.0); // already smoothed to this value
        l.tgpu = Some(99.0);
        let d = l.tick(1.0, 99.0, 99.0, 99.0, 0.0, 0.0);
        assert_eq!(d, Some(Mode::Quiet), "battery_hot of the truesense profile");
    }

    #[test]
    fn a_cpu_sitting_on_the_turbo_line_does_not_make_the_mode_flap() {
        let mut l = Laptop::settled(profile("truesense"));
        // the real session: GPU at 98 %, CPU between 80 and 89 C for minutes (the old logic went
        // Turbo, Performance, Turbo... every few seconds at 85 C)
        let changes = l.play(900.0, |t| 84.5 + 4.5 * (t / 9.0).sin(), GAME);
        assert!(changes.len() <= 2, "mode changes during 15 minutes at the Turbo line: {changes:?}");
        assert_eq!(l.mode(), Mode::Performance, "it settles on Performance");
    }

    #[test]
    fn turbo_is_entered_well_below_its_limit_and_left_only_at_it_with_a_long_retry_pause() {
        let mut l = Laptop::settled(profile("truesense"));
        l.play(200.0, constant(70.0), GAME); // cool: up to Turbo (after 30 s of heavy load and the 45 s minimum gap)
        assert_eq!(l.mode(), Mode::Turbo);
        l.play(20.0, constant(86.0), GAME);
        assert_eq!(l.mode(), Mode::Performance, "at the limit it steps down at once");
        let t_down = l.now;
        l.play(120.0, constant(70.0), GAME);
        assert_eq!(
            l.mode(),
            Mode::Performance,
            "and does not climb back for 180 s even though it is cool again"
        );
        l.play(120.0, constant(70.0), GAME);
        assert_eq!(l.mode(), Mode::Turbo, "after the pause it may try again");
        assert!(l.now - t_down >= 180.0);
        l.play(60.0, constant(82.0), GAME);
        assert_eq!(
            l.mode(),
            Mode::Turbo,
            "82 C is under the limit: it stays (the old logic left Turbo near its entry temperature)"
        );
    }

    #[test]
    fn turbo_needs_the_margin_below_the_limit_to_enter() {
        // margin 6, limit 85: enter only below 79 (the smoothed CPU value)
        let mut l = Laptop::settled(profile("truesense"));
        l.play(200.0, constant(80.0), GAME);
        assert_eq!(l.mode(), Mode::Performance, "80 C is not below 79");
        l.play(120.0, constant(78.0), GAME);
        assert_eq!(l.mode(), Mode::Turbo, "78 C is");
    }

    #[test]
    fn a_quiet_moment_in_a_game_keeps_the_mode() {
        let mut l = Laptop::settled(profile("truesense"));
        l.play(40.0, constant(70.0), GAME);
        assert_eq!(
            l.mode(),
            Mode::Performance,
            "40 s of game: Performance (Turbo would need 30 s of load and a 45 s gap)"
        );
        let quiet = l.play(45.0, constant(70.0), MENU);
        assert_eq!((quiet.len(), l.mode()), (0, Mode::Performance), "a 45 s menu does not drop the mode");
        assert!(l.engine.reason().contains("holding the busy mode"), "{}", l.engine.reason());
        l.play(400.0, constant(50.0), CALM);
        assert!(
            matches!(l.mode(), Mode::Balanced | Mode::Quiet),
            "really quiet for minutes: it does come back down"
        );
    }

    #[test]
    fn the_busy_hold_ends_after_busy_hold_s_seconds() {
        let mut l = Laptop::settled(profile("truesense")); // busy_hold_s = 60, down dwell 60
        l.play(40.0, constant(70.0), GAME);
        // the quiet moment is held for 60 s counted from the last busy tick, then the mode may drop
        l.play(58.0, constant(70.0), MENU);
        assert!(l.engine.reason().contains("holding"));
        l.play(5.0, constant(70.0), MENU);
        assert!(!l.engine.reason().contains("holding"), "{}", l.engine.reason());
    }

    #[test]
    fn a_quiet_moment_never_starts_turbo() {
        let mut l = Laptop::settled(profile("truesense"));
        l.play(20.0, constant(70.0), GAME); // busy for 20 s only (Turbo needs 30)
        l.play(120.0, constant(70.0), MENU);
        assert_ne!(l.mode(), Mode::Turbo, "a held quiet moment keeps the current mode, it never raises it");
    }

    #[test]
    fn the_steady_profile_never_enters_turbo_and_changes_rarely() {
        let mut l = Laptop::settled(profile("steady"));
        let changes = l.play(900.0, |t| 70.0 + 8.0 * (t / 15.0).sin(), GAME);
        assert!(changes.iter().all(|(_, m)| *m != Mode::Turbo), "no Turbo with the steady profile");
        assert!(changes.len() <= 1, "{changes:?}");
    }

    #[test]
    fn timers_are_real_seconds_whatever_the_tick_length() {
        for dt in [1.0, 2.0] {
            // the service ticks every 1 s or every 2 s
            let mut l = Laptop::settled(profile("truesense"));
            let start = l.now;
            let changes = l.play(140.0, constant(70.0), Play { dt, ..GAME });
            let turbo_at = changes.iter().find(|(_, m)| *m == Mode::Turbo).map(|(t, _)| *t);
            let turbo_at = turbo_at.unwrap_or_else(|| panic!("dt={dt}: no Turbo: {changes:?}"));
            assert!(turbo_at - start >= 30.0, "Turbo not before 30 s of heavy load (dt={dt}): {changes:?}");
        }
    }

    #[test]
    fn a_stalled_loop_counts_as_at_most_five_seconds_of_load() {
        let mut l = Laptop::settled(profile("truesense"));
        l.tick(1.0, 70.0, 65.0, 16.0, 98.0, 71.0);
        let before = l.engine.heavy_for();
        l.tick(3600.0, 70.0, 65.0, 16.0, 98.0, 71.0); // the machine was suspended for an hour
        assert_eq!(l.engine.heavy_for(), before + 5.0, "one step counts for at most 5 s");
        assert_ne!(l.mode(), Mode::Turbo);
    }

    #[test]
    fn a_clock_that_goes_backwards_adds_nothing() {
        let mut l = Laptop::settled(profile("truesense"));
        l.tick(1.0, 70.0, 65.0, 16.0, 98.0, 71.0);
        let before = l.engine.heavy_for();
        l.tick(-30.0, 70.0, 65.0, 16.0, 98.0, 71.0);
        assert_eq!(l.engine.heavy_for(), before);
    }

    #[test]
    fn every_decision_carries_its_reason_and_the_seconds_of_heavy_load() {
        let mut l = Laptop::settled(profile("truesense"));
        l.play(30.0, constant(70.0), GAME);
        let first = l.log.iter().find(|d| d.mode == Some(Mode::Performance)).expect("went to performance");
        assert!(first.reason.starts_with("gaming, on power"), "{}", first.reason);
        assert!(l.engine.reason().contains(" s heavy"), "{}", l.engine.reason());
        let mut b = Laptop::new(profile("truesense"));
        b.ac = false;
        b.tick(1.0, 40.0, 40.0, 5.0, 0.0, 0.0);
        assert!(b.engine.reason().starts_with("idle, on battery"), "{}", b.engine.reason());
    }

    #[test]
    fn profiles_change_how_smart_behaves() {
        let mut l = Laptop::new(profile("truesense"));
        l.drive(70, 40.0, 40.0, 5.0);
        l.engine.set_profile(profile("battery-first"));
        for _ in 0..60 {
            l.tick(1.0, 70.0, 60.0, 90.0, 0.0, 0.0);
        }
        assert_eq!(l.mode(), Mode::Performance, "battery-first never uses Turbo, even when sustained");
        l.engine.set_profile(profile("performance-first"));
        for _ in 0..25 {
            l.tick(1.0, 70.0, 60.0, 90.0, 0.0, 0.0);
        }
        assert_eq!(l.mode(), Mode::Turbo, "performance-first: Turbo after 10 s");
        assert_eq!(l.engine.profile().turbo_after_s, 10);
    }

    #[test]
    fn leaving_turbo_at_its_limit_blocks_it_for_the_retry_pause() {
        // performance-first: Turbo limit 90 C, hot line 92 C, retry pause 180 s
        let mut l = Laptop::settled(profile("performance-first"));
        l.play(60.0, constant(70.0), GAME);
        assert_eq!(l.mode(), Mode::Turbo);
        l.play(10.0, constant(91.0), Play { gpu: 70.0, ..GAME });
        assert_eq!(l.mode(), Mode::Performance, "at the Turbo limit, below the hot line: step down at once");
        let t = l.now;
        l.play(150.0, constant(70.0), GAME);
        assert_eq!(
            l.mode(),
            Mode::Performance,
            "the 180 s retry pause (longer than the 45 s back-off hold) applies"
        );
        l.play(60.0, constant(70.0), GAME);
        assert_eq!(l.mode(), Mode::Turbo);
        assert!(l.now - t >= 180.0);
    }

    #[test]
    fn a_real_overheat_blocks_turbo_only_for_the_backoff_hold() {
        // Python behaviour kept as it is: every hot tick RESETS the block to now + backoff_hold_s,
        // so after a real overheat (above the hot line) the 180 s retry pause set by the step
        // down is replaced by the shorter 45 s hold once the laptop has been hot for a while.
        let mut l = Laptop::settled(profile("performance-first"));
        l.play(60.0, constant(70.0), GAME);
        assert_eq!(l.mode(), Mode::Turbo);
        l.play(10.0, constant(95.0), Play { gpu: 70.0, ..GAME });
        assert_eq!(l.mode(), Mode::Performance);
        let t = l.now;
        l.play(150.0, constant(70.0), GAME);
        assert_eq!(l.mode(), Mode::Turbo, "back in Turbo well before 180 s: the block was the 45 s hold");
        assert!(l.now - t < 180.0);
    }

    // ---- hold ---------------------------------------------------------------------------------

    #[test]
    fn a_hold_keeps_your_pick_and_ends_when_the_situation_changes() {
        let mut l = Laptop::settled(profile("truesense"));
        l.mode = Current::Known(Mode::Eco);
        l.engine.set_hold(l.wall, 30.0, l.ac, Activity::Idle);
        for _ in 0..30 {
            l.tick(1.0, 40.0, 40.0, 5.0, 0.0, 0.0);
        }
        assert_eq!(l.mode(), Mode::Eco, "same situation: your pick stays");
        l.play(8.0, constant(70.0), CRUNCH);
        assert_eq!(
            l.mode(),
            Mode::Performance,
            "a game starts: smart auto takes over without anyone pressing resume"
        );
        assert_eq!(l.engine.hold_until(), 0.0, "and the hold is gone (the caller stores that)");
        l.mode = Current::Known(Mode::Eco);
        l.engine.set_hold(l.wall, 30.0, l.ac, Activity::Heavy);
        l.ac = false;
        l.play(8.0, constant(70.0), CRUNCH);
        assert_ne!(l.mode(), Mode::Eco, "unplugging also ends the hold");
    }

    #[test]
    fn a_hold_wins_while_the_situation_is_the_same_and_ends_by_time() {
        let mut l = Laptop::settled(profile("truesense"));
        l.engine.set_hold(l.wall, 30.0, l.ac, Activity::Idle);
        l.mode = Current::Known(Mode::Turbo);
        assert_eq!(l.tick(5.0, 40.0, 40.0, 4.0, 0.0, 0.0), None, "a hold must be respected");
        assert_eq!(l.engine.hold_left_s(l.wall), 30 * 60 - 5);
        l.tick(30.0 * 60.0, 40.0, 40.0, 4.0, 0.0, 0.0);
        assert_eq!(l.mode(), Mode::Balanced, "when the 30 minutes are up the automatic mode takes over");
        assert_eq!(l.engine.hold_left_s(l.wall), 0);
    }

    #[test]
    fn releasing_a_hold_lets_auto_act_again() {
        let mut l = Laptop::settled(profile("truesense"));
        l.engine.set_hold(l.wall, 30.0, l.ac, Activity::Idle);
        l.mode = Current::Known(Mode::Turbo);
        assert_eq!(l.engine.set_hold(l.wall, 0.0, l.ac, Activity::Idle), 0.0);
        assert!(
            l.tick(61.0, 40.0, 40.0, 4.0, 0.0, 0.0).is_some(),
            "after the hold is released the automatic mode takes over"
        );
        assert_eq!(l.engine.set_hold(l.wall, -5.0, l.ac, Activity::Idle), 0.0, "a negative hold is no hold");
        assert_eq!(l.engine.set_hold(l.wall, 2.0, l.ac, Activity::Idle), 2.0);
    }

    #[test]
    fn a_hold_ends_on_heat_or_a_low_battery() {
        for (ac, battery, cpu) in [(true, 80.0, 99.0), (false, 5.0, 40.0)] {
            let mut l = Laptop::settled(profile("truesense"));
            l.ac = ac;
            l.engine.set_hold(l.wall, 30.0, ac, Activity::Idle);
            l.battery = battery;
            l.mode = Current::Known(Mode::Turbo);
            l.tcpu = Some(cpu); // already smoothed to this value
            l.tgpu = Some(cpu);
            l.tick(1.0, cpu, cpu, 4.0, 0.0, 0.0);
            assert_eq!(l.engine.hold_until(), 0.0, "ac={ac} battery={battery} cpu={cpu}");
        }
    }

    #[test]
    fn a_restored_hold_has_no_situation_and_runs_out_by_time() {
        let mut l = Laptop::settled(profile("truesense"));
        let until = l.wall + 100.0;
        l.engine.restore_hold(until);
        l.mode = Current::Known(Mode::Turbo);
        l.ac = false; // a plug change cannot end it, nobody remembers the old plug state
        assert_eq!(l.tick(1.0, 99.0, 99.0, 99.0, 0.0, 0.0), None, "not even heat ends a restored hold");
        assert_eq!(l.engine.hold_until(), until);
        assert_eq!(
            l.tick(100.0, 40.0, 40.0, 4.0, 0.0, 0.0),
            Some(Mode::Eco),
            "when its time is up auto acts"
        );
    }

    #[test]
    fn choosing_auto_takes_over_immediately_even_during_a_hold() {
        let mut l = Laptop::new(profile("truesense"));
        l.kind = AutoKind::Off;
        l.tick(5.0, 40.0, 40.0, 5.0, 0.0, 0.0);
        l.mode = Current::Known(Mode::Turbo);
        l.engine.set_hold(l.wall, 30.0, l.ac, Activity::Idle);
        l.kind = AutoKind::Native;
        l.engine.take_over_now();
        l.tick(1.0, 40.0, 40.0, 5.0, 0.0, 0.0);
        assert_eq!(
            l.mode(),
            Mode::Balanced,
            "native auto (AC: balanced) must not wait for the old hold to run out"
        );
        assert_eq!(l.engine.hold_until(), 0.0);
        assert_eq!(l.engine.hold_left_s(l.wall), 0);
    }

    // ---- game rules ---------------------------------------------------------------------------

    #[test]
    fn a_game_rule_raises_the_mode_while_the_game_runs_and_auto_takes_over_after() {
        let mut l = Laptop::settled(profile("truesense"));
        l.game = Some(Mode::Performance);
        for _ in 0..40 {
            l.tick(1.0, 50.0, 50.0, 5.0, 0.0, 0.0);
        }
        assert_eq!(
            l.mode(),
            Mode::Performance,
            "a running game rule raises the mode even when the load is light"
        );
        l.game = Some(Mode::Turbo);
        for _ in 0..40 {
            l.tick(1.0, 93.0, 93.0, 99.0, 0.0, 0.0);
        }
        assert_ne!(l.mode(), Mode::Turbo, "turbo still backs off when hot");
        l.game = None;
        l.drive(80, 45.0, 45.0, 2.0);
        assert_eq!(l.mode(), Mode::Balanced, "back to the automatic profile when the game ends");
    }

    #[test]
    fn a_game_rule_never_lowers_the_mode_and_a_low_battery_wins() {
        let mut l = Laptop::new(profile("truesense"));
        l.game = Some(Mode::Turbo);
        l.tick(1.0, 40.0, 40.0, 5.0, 0.0, 0.0);
        assert_eq!(l.mode(), Mode::Turbo);
        let mut l = Laptop::new(profile("truesense"));
        l.game = Some(Mode::Eco);
        l.mode = Current::Known(Mode::Performance);
        l.tick(1.0, 40.0, 40.0, 5.0, 0.0, 0.0);
        assert_eq!(l.mode(), Mode::Balanced, "a rule can never lower the mode below what auto wants");
        let mut l = Laptop::new(profile("truesense"));
        l.game = Some(Mode::Turbo);
        l.ac = false;
        l.battery = 19.9;
        l.tick(1.0, 40.0, 40.0, 5.0, 0.0, 0.0);
        assert_eq!(l.mode(), Mode::Eco, "a nearly empty battery wins over a game rule");
        let mut l = Laptop::new(profile("truesense"));
        l.game = Some(Mode::Turbo);
        l.ac = false;
        l.battery = 20.0;
        l.tick(1.0, 40.0, 40.0, 5.0, 0.0, 0.0);
        assert_eq!(l.mode(), Mode::Turbo, "20 % is not below 20");
    }

    #[test]
    fn a_game_rule_also_applies_to_native_auto() {
        let mut l = Laptop::new(profile("truesense"));
        l.kind = AutoKind::Native;
        l.game = Some(Mode::Performance);
        l.tick(1.0, 40.0, 40.0, 5.0, 0.0, 0.0);
        assert_eq!(l.mode(), Mode::Performance);
    }

    #[test]
    fn a_hot_game_asks_for_performance_not_turbo() {
        let mut l = Laptop::new(profile("silent"));
        l.game = Some(Mode::Turbo);
        l.tick(1.0, 99.0, 99.0, 99.0, 0.0, 0.0);
        assert_eq!(l.mode(), Mode::Performance);
    }

    // ---- retries, dwell, unrecognised mode ----------------------------------------------------

    #[test]
    fn a_failed_write_is_not_retried_for_thirty_seconds() {
        let mut e = Engine::new(profile("native"), NativeModes::default());
        let mut inp = Inputs {
            kind: AutoKind::Native,
            now: 100.0,
            wall: 1e6,
            activity: Activity::Idle,
            tcpu: 40.0,
            tgpu: 40.0,
            ac: true,
            battery_pct: 80.0,
            cur_mode: Current::Known(Mode::Eco),
            game: None,
            disabled: false,
        };
        assert_eq!(e.step(&inp).mode, Some(Mode::Balanced));
        // the caller's write failed: it does not call applied()
        inp.now = 110.0;
        assert_eq!(e.step(&inp).mode, None, "already failed recently");
        inp.now = 129.9;
        assert_eq!(e.step(&inp).mode, None);
        inp.now = 130.0;
        assert_eq!(e.step(&inp).mode, Some(Mode::Balanced), "after 30 s it tries again");
        e.resync();
        inp.now = 131.0;
        assert_eq!(e.step(&inp).mode, Some(Mode::Balanced), "a resync forgets the failure at once");
    }

    #[test]
    fn dwell_times_apply_to_non_urgent_changes_and_min_change_only_to_smart() {
        // truesense: up 5 s, down 60 s, smart minimum gap 45 s
        let mut l = Laptop::new(profile("truesense"));
        l.mode = Current::Known(Mode::Eco);
        l.engine.applied(l.now, false); // a change just happened at t=0
        assert_eq!(
            l.tick(10.0, 40.0, 40.0, 30.0, 0.0, 0.0),
            None,
            "10 s after a change: inside the 45 s minimum gap"
        );
        assert_eq!(
            l.tick(35.0, 40.0, 40.0, 30.0, 0.0, 0.0),
            Some(Mode::Balanced),
            "45 s after it: up to the everyday mode"
        );
        // native has no minimum gap
        let mut n = Laptop::new(profile("steady"));
        n.kind = AutoKind::Native;
        n.engine.set_native(NativeModes { ac: Mode::Performance, battery: Mode::Eco });
        n.mode = Current::Known(Mode::Quiet);
        n.engine.applied(n.now, true);
        assert_eq!(
            n.tick(0.5, 40.0, 40.0, 5.0, 0.0, 0.0),
            Some(Mode::Performance),
            "native is urgent: no waiting"
        );
    }

    #[test]
    fn going_down_waits_for_the_down_dwell() {
        let mut l = Laptop::new(profile("truesense"));
        l.mode = Current::Known(Mode::Performance);
        l.engine.applied(l.now, false);
        // browsing on power wants balanced (down). down dwell 60 s and min gap 45 s: the longer wins
        assert_eq!(l.tick(59.0, 50.0, 45.0, 20.0, 15.0, 0.0), None);
        assert_eq!(l.tick(1.0, 50.0, 45.0, 20.0, 15.0, 0.0), Some(Mode::Balanced));
    }

    #[test]
    fn an_unrecognised_firmware_mode_is_left_for_the_target() {
        let mut l = Laptop::new(profile("native"));
        l.mode = Current::Unrecognised;
        l.kind = AutoKind::Native;
        assert_eq!(l.tick(1.0, 40.0, 40.0, 5.0, 0.0, 0.0), Some(Mode::Balanced));
        assert_eq!(Current::Unrecognised.rank(), Mode::Balanced.rank());
        assert_eq!(Current::Unread.rank(), 2);
    }

    #[test]
    fn idle_needs_to_last_before_the_idle_mode_is_used() {
        let mut p = profile("truesense");
        p.ac_idle = Mode::Quiet; // distinguishable from ac_browsing (balanced)
        let mut l = Laptop::new(p);
        l.mode = Current::Known(Mode::Balanced);
        for _ in 0..59 {
            l.tick(1.0, 40.0, 40.0, 2.0, 0.0, 0.0);
        }
        assert_eq!(l.mode(), Mode::Balanced, "idle for less than idle_after_s (60): still the everyday mode");
        for _ in 0..80 {
            l.tick(1.0, 40.0, 40.0, 2.0, 0.0, 0.0);
        }
        assert_eq!(l.mode(), Mode::Quiet, "idle long enough");
        l.tick(1.0, 50.0, 45.0, 20.0, 15.0, 0.0);
        assert_eq!(l.engine.reason(), "browsing, on power");
    }

    #[test]
    fn notices_are_rate_limited_but_urgent_changes_always_show() {
        let mut e = Engine::new(profile("truesense"), NativeModes::default());
        assert!(e.applied(1000.0, false), "the first change is announced");
        assert!(!e.applied(1010.0, false), "10 s later: no second notice");
        assert!(!e.applied(1029.9, false));
        assert!(e.applied(1040.0, false), "30 s after the last notice");
        assert!(e.applied(1041.0, true), "urgent always");
        assert!(!e.applied(1042.0, false));
    }

    #[test]
    fn busy_latching_only_follows_the_hold_and_never_outlives_hot() {
        let mut e = Engine::new(profile("truesense"), NativeModes::default());
        assert_eq!(e.busy_latched(Activity::Gaming, 0.0, 60.0), Activity::Gaming);
        assert_eq!(e.busy_latched(Activity::Browsing, 59.9, 60.0), Activity::Gaming, "held");
        assert_eq!(e.busy_latched(Activity::Hot, 61.0, 60.0), Activity::Hot, "hot is never held");
        assert_eq!(e.busy_latched(Activity::Idle, 61.0, 60.0), Activity::Idle, "60 s have passed");
        assert_eq!(e.busy_latched(Activity::Idle, 61.1, 60.0), Activity::Idle);
        let mut e = Engine::new(profile("truesense"), NativeModes::default());
        e.busy_latched(Activity::Heavy, 0.0, 0.0);
        assert_eq!(
            e.busy_latched(Activity::Idle, 0.1, 0.0),
            Activity::Idle,
            "a hold of 0 means no holding (native profile)"
        );
        // hot in the middle keeps the remembered kind: quiet right after it is still held
        let mut e = Engine::new(profile("truesense"), NativeModes::default());
        e.busy_latched(Activity::Gaming, 0.0, 60.0);
        e.busy_latched(Activity::Hot, 10.0, 60.0);
        assert_eq!(e.busy_latched(Activity::Browsing, 20.0, 60.0), Activity::Gaming);
    }
}
