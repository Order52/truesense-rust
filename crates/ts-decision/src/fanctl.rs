//! The smart fan: turns temperatures into fan duties with the active curve, raises them at
//! once and lowers them slowly, holds a floor while gaming or working hard, hands the fans
//! back to the firmware when the laptop is calm (the firmware is the only way to reach 0 rpm)
//! and drives them to 100 % when it is hot.
//!
//! Three separate pieces, none of which touches hardware or a clock:
//!
//! * [`Smart`]: the controller, a pure state machine (readings in, duties out).
//! * The guards for the user's explicit fan stop: [`plan_manual`] (duty rule, confirmation,
//!   "cool machine"), [`must_return_fans`] and [`StopWatchdog`] (CPU 70 / GPU 65 degC, or the
//!   device stops answering, and ALL fans go back to the firmware).
//! * [`FanWriter`]: writes a duty only when it changed and notices when something else reset
//!   the fans; the hardware sits behind the [`FanHardware`] trait.
//!
//! The safety rule behind all of it: a fan duty is 0 or 20..=100, and nothing here stops a
//! fan without the explicit confirm path of [`plan_manual`].
//!
//! Measured fan response (manual duty, both fans): 20 % about 1700 rpm, 30 2160, 40 2850,
//! 50 3460, 60 4070, 70 4600, 80 5100, 100 6050.

use std::fmt;

use ts_foundation::config::FanConfig;

use crate::curves::{Curve, Follower, Side};

/// A fan may only be stopped while the CPU is below this temperature (degC) ...
pub const STOP_START_CPU_C: f64 = 60.0;
/// ... and the GPU below this one.
pub const STOP_START_GPU_C: f64 = 55.0;
/// The watchdog gives all fans back to the firmware at this CPU temperature (degC) ...
pub const STOP_MAX_CPU_C: f64 = 70.0;
/// ... or at this GPU temperature.
pub const STOP_MAX_GPU_C: f64 = 65.0;
/// The lowest duty a running fan can hold; below it the firmware stalls the fan.
pub const MIN_DUTY: u8 = 20;
/// The highest duty.
pub const MAX_DUTY: u8 = 100;

/// What the laptop is doing, as far as the fans care. The classification (including the
/// "stay hot until 5 degC below the line" latch) belongs to the automatic profile engine; it
/// maps its own classification onto this, so the fan controller compiles and tests alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Activity {
    /// Nothing much is running.
    Idle,
    /// Light use.
    Browsing,
    /// A heavy CPU job.
    Heavy,
    /// A game (the GPU is busy).
    Gaming,
    /// A chip is near its limit: the fans go to 100 %.
    Hot,
}

impl Activity {
    /// The lower-case word the Python edition used in state and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Activity::Idle => "idle",
            Activity::Browsing => "browsing",
            Activity::Heavy => "heavy",
            Activity::Gaming => "gaming",
            Activity::Hot => "hot",
        }
    }
}

impl fmt::Display for Activity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The duties to write. `(0, 0)` ([`Duties::AUTO`]) means "hand the fans to the firmware"; any
/// other value is a manual duty for both fans and is always 20..=100 on each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Duties {
    /// CPU fan percent.
    pub cpu: u8,
    /// GPU fan percent.
    pub gpu: u8,
}

impl Duties {
    /// Firmware control.
    pub const AUTO: Duties = Duties { cpu: 0, gpu: 0 };
    /// Both fans flat out.
    pub const FULL: Duties = Duties { cpu: MAX_DUTY, gpu: MAX_DUTY };

    /// Whether this means "firmware Auto".
    pub fn is_auto(self) -> bool {
        self == Duties::AUTO
    }
}

/// The settings of the smart fan that do not come from the curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    /// The CPU counts as calm below this temperature (degC, strictly).
    pub calm_cpu: f64,
    /// The GPU counts as calm below this temperature (degC, strictly).
    pub calm_gpu: f64,
    /// Calm for this many steps in a row and the fans go back to the firmware. Steps, not
    /// seconds: the daemon steps once a second while anything is happening.
    pub calm_hold: i64,
}

impl Params {
    /// The parameters from `[fan]` in the configuration.
    pub fn from_config(f: &FanConfig) -> Params {
        // Config values are small integers; the conversion is exact.
        Params { calm_cpu: f.calm_cpu as f64, calm_gpu: f.calm_gpu as f64, calm_hold: f.calm_hold }
    }
}

/// What one step looks at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Inputs {
    /// What the laptop is doing.
    pub activity: Activity,
    /// CPU temperature in degC (the daemon smooths it).
    pub tcpu: f64,
    /// GPU temperature in degC.
    pub tgpu: f64,
    /// Chassis (board/VRM) temperature in degC, if the sensor answers. Heat soak there raises
    /// the floor.
    pub tchassis: Option<f64>,
    /// GPU power draw in watts.
    pub gpu_power_w: f64,
    /// On AC power (no battery shift) or on battery.
    pub on_ac: bool,
    /// Seconds since the previous step. The Python edition always used 1.0 whatever the real
    /// interval was (it steps every second or every two); the Go edition passes the measured
    /// time. The caller chooses.
    pub dt_s: f64,
}

/// The activity floor: a game or a heavy job needs air even before the temperature has moved,
/// and a hot board or VRM (the chassis sensor) raises it further. Never forces the fans on
/// when the laptop is idle or browsing.
pub fn floor_pct(activity: Activity, gpu_power_w: f64, tchassis: Option<f64>) -> u8 {
    let mut f = match activity {
        Activity::Gaming if gpu_power_w >= 80.0 => 60,
        Activity::Gaming => 50,
        Activity::Heavy => 40,
        _ => 0,
    };
    // A chassis reading of 0 means "no sensor", as None does.
    if let Some(t) = tchassis.filter(|t| f > 0 && *t != 0.0) {
        if t >= 68.0 {
            f = f.max(60);
        } else if t >= 63.0 {
            f = f.max(50);
        }
    }
    f
}

/// The curve-driven fan controller. Not safe for concurrent use: the daemon's control loop
/// owns it (the Python edition took the same lock as the request handlers).
#[derive(Debug, Clone)]
pub struct Smart {
    params: Params,
    follower: Follower,
    calm: u32,
    hist_cpu: Vec<f64>,
    hist_gpu: Vec<f64>,
}

impl Smart {
    /// A controller following `curve`, both duties at 0.
    pub fn new(params: Params, curve: Curve) -> Smart {
        Smart { params, follower: Follower::new(curve), calm: 0, hist_cpu: Vec::new(), hist_gpu: Vec::new() }
    }

    /// Switches the curve cleanly (both duties start again from 0).
    pub fn set_curve(&mut self, curve: Curve) {
        self.follower.set_curve(curve);
    }

    /// The curve in use.
    pub fn curve(&self) -> &Curve {
        self.follower.curve()
    }

    /// The parameters in use.
    pub fn params(&self) -> &Params {
        &self.params
    }

    /// Replaces the parameters (the configuration changed).
    pub fn set_params(&mut self, params: Params) {
        self.params = params;
    }

    /// Forgets the calm counter: the smart fan was switched off, or a manual fan command ran.
    pub fn reset(&mut self) {
        self.calm = 0;
    }

    /// How many steps in a row the laptop has been calm.
    pub fn calm_steps(&self) -> u32 {
        self.calm
    }

    /// The current follower duties in percent, before quantising (for diagnostics).
    pub fn follower_duty(&self) -> (f64, f64) {
        self.follower.duty()
    }

    /// The temperature change per sample over the last five samples; 0 until there are five.
    fn slope(hist: &mut Vec<f64>, v: f64) -> f64 {
        hist.push(v);
        if hist.len() > 5 {
            hist.remove(0);
        }
        match (hist.len(), hist.first(), hist.last()) {
            (5, Some(first), Some(last)) => (last - first) / 4.0,
            _ => 0.0,
        }
    }

    /// One control step: the duties to write, `(0, 0)` meaning "hand the fans to the firmware".
    ///
    /// Order of precedence (the same as Python): calm for `calm_hold` steps, or calm with the
    /// follower already under 20 %, gives the fans to the firmware; a hot machine gets 100 %
    /// on both fans at once; otherwise the curve is followed, then the activity floor and the
    /// coupling are applied. The temperature history for the rising test is only fed on the
    /// steps that follow the curve.
    pub fn step(&mut self, inp: &Inputs) -> Duties {
        let calm_now = matches!(inp.activity, Activity::Idle | Activity::Browsing)
            && inp.tcpu < self.params.calm_cpu
            && inp.tgpu < self.params.calm_gpu;
        self.calm = if calm_now { self.calm.saturating_add(1) } else { 0 };
        let (c, g) = self.follower.duty();
        if i64::from(self.calm) >= self.params.calm_hold || (c.max(g) < f64::from(MIN_DUTY) && calm_now) {
            self.follower.set_duty(0.0, 0.0);
            return Duties::AUTO;
        }
        if inp.activity == Activity::Hot {
            self.follower.set_duty(100.0, 100.0);
            return Duties::FULL;
        }
        let shift = self.follower.battery_shift(inp.on_ac);
        let rising_cpu = Self::slope(&mut self.hist_cpu, inp.tcpu) >= 1.0;
        let rising_gpu = Self::slope(&mut self.hist_gpu, inp.tgpu) >= 1.0;
        self.follower.step(Side::Cpu, inp.tcpu, rising_cpu, inp.dt_s, shift);
        self.follower.step(Side::Gpu, inp.tgpu, rising_gpu, inp.dt_s, shift);
        let (cpu, gpu) = self.follower.output(floor_pct(inp.activity, inp.gpu_power_w, inp.tchassis));
        Duties { cpu, gpu }
    }
}

/// The temperatures the stop guards look at. `None` is "no reading yet".
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Temps {
    /// CPU temperature in degC.
    pub cpu: Option<f64>,
    /// GPU temperature in degC.
    pub gpu: Option<f64>,
}

/// Why a manual fan request was refused. `Display` is the sentence the Python edition raised.
#[derive(Debug, Clone, PartialEq)]
pub enum FanError {
    /// A duty is neither 0 nor 20..=100.
    BadDuty,
    /// A stop was asked for without the explicit confirmation.
    NeedsConfirmation,
    /// The machine is too warm (or its temperature unknown) to stop a fan.
    TooWarm {
        /// CPU temperature rounded for the message (0 when unknown).
        cpu: i64,
        /// GPU temperature rounded for the message (0 when unknown).
        gpu: i64,
    },
}

impl fmt::Display for FanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FanError::BadDuty => f.write_str("manual fan duty is 20..100 % per fan, or 0 to stop that fan"),
            FanError::NeedsConfirmation => f.write_str("stopping a fan needs explicit confirmation"),
            FanError::TooWarm { cpu, gpu } => write!(
                f,
                "too warm to stop a fan (CPU {cpu} C, GPU {gpu} C; a fan may only be stopped below {} / {} C)",
                STOP_START_CPU_C, STOP_START_GPU_C
            ),
        }
    }
}

impl std::error::Error for FanError {}

/// An accepted manual fan request: the duties to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManualPlan {
    /// CPU fan percent: 0 (stop) or 20..=100.
    pub cpu: u8,
    /// GPU fan percent: 0 (stop) or 20..=100.
    pub gpu: u8,
}

impl ManualPlan {
    /// Whether this stops a fan (either one or both): the stopped state the watchdog guards.
    pub fn stops_a_fan(self) -> bool {
        self.cpu == 0 || self.gpu == 0
    }

    /// What the request answers: `"off"` (both fans stopped), `"partial-off"` (one stopped) or
    /// `"manual"`.
    pub fn outcome(self) -> &'static str {
        match (self.cpu, self.gpu) {
            (0, 0) => "off",
            (0, _) | (_, 0) => "partial-off",
            _ => "manual",
        }
    }

    /// The words for the log line "... stopped by you".
    pub fn stopped_which(self) -> Option<&'static str> {
        match (self.cpu, self.gpu) {
            (0, 0) => Some("both fans"),
            (0, _) => Some("the CPU fan"),
            (_, 0) => Some("the GPU fan"),
            _ => None,
        }
    }
}

/// Checks a manual fan request. A duty is 0 or 20..=100 per fan (`gpu` defaults to `cpu`).
/// Stopping a fan (a 0) additionally needs `confirm`, a known CPU temperature under 60 degC
/// and a GPU temperature (unknown counts as 0, as in Python) under 55 degC. The order of the
/// checks (duty, confirmation, warmth) decides which message the user sees.
pub fn plan_manual(cpu: i64, gpu: Option<i64>, confirm: bool, temps: Temps) -> Result<ManualPlan, FanError> {
    let gpu = gpu.unwrap_or(cpu);
    let duty = |v: i64| match v {
        0 => Ok(0),
        20..=100 => u8::try_from(v).map_err(|_| FanError::BadDuty),
        _ => Err(FanError::BadDuty),
    };
    let plan = ManualPlan { cpu: duty(cpu)?, gpu: duty(gpu)? };
    if plan.stops_a_fan() {
        if !confirm {
            return Err(FanError::NeedsConfirmation);
        }
        can_stop(temps)?;
    }
    Ok(plan)
}

/// The "fans off" request: both fans stopped, with the same guards.
pub fn plan_off(confirm: bool, temps: Temps) -> Result<ManualPlan, FanError> {
    plan_manual(0, Some(0), confirm, temps)
}

/// Whether a fan may be stopped at these temperatures: the CPU temperature must be known and
/// under 60 degC, the GPU's (0 when unknown) under 55 degC.
pub fn can_stop(temps: Temps) -> Result<(), FanError> {
    let gpu = temps.gpu.unwrap_or(0.0);
    match temps.cpu {
        Some(cpu) if cpu < STOP_START_CPU_C && gpu < STOP_START_GPU_C => Ok(()),
        _ => Err(FanError::TooWarm {
            cpu: temps.cpu.unwrap_or(0.0).round_ties_even() as i64,
            gpu: gpu.round_ties_even() as i64,
        }),
    }
}

/// The watchdog test for a user-stopped fan: when the laptop warms up (CPU 70 / GPU 65 degC)
/// or the hardware stops answering, all fans must go back to firmware Auto. A missing
/// temperature counts as 0.
pub fn must_return_fans(temps: Temps, hw_ok: bool) -> bool {
    temps.cpu.unwrap_or(0.0) >= STOP_MAX_CPU_C || temps.gpu.unwrap_or(0.0) >= STOP_MAX_GPU_C || !hw_ok
}

/// The state of "a fan was stopped by the user" and its watchdog. The daemon arms it after a
/// stop reached the hardware, disarms it on any other fan command, and calls
/// [`StopWatchdog::check`] every step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StopWatchdog {
    armed: bool,
}

impl StopWatchdog {
    /// A watchdog with no stop in force.
    pub fn new() -> StopWatchdog {
        StopWatchdog::default()
    }

    /// A fan was stopped; watch the temperatures from now on.
    pub fn arm(&mut self) {
        self.armed = true;
    }

    /// The stop ended (any other fan command, or the smart fan took over).
    pub fn disarm(&mut self) {
        self.armed = false;
    }

    /// Whether a user stop is in force.
    pub fn is_armed(&self) -> bool {
        self.armed
    }

    /// True exactly once when a stop is in force and the laptop got warm (or the device stopped
    /// answering): the caller must hand ALL fans to the firmware. The stop is over afterwards.
    pub fn check(&mut self, temps: Temps, hw_ok: bool) -> bool {
        if self.armed && must_return_fans(temps, hw_ok) {
            self.armed = false;
            return true;
        }
        false
    }
}

/// What [`FanWriter`] needs from the fans.
pub trait FanHardware {
    /// What a failed call reports.
    type Error: std::error::Error;

    /// Hands the fans to the firmware.
    fn fan_auto(&mut self) -> Result<(), Self::Error>;

    /// Manual duties for both fans (0 or 20..=100 each).
    fn fan_manual(&mut self, cpu: u8, gpu: u8) -> Result<(), Self::Error>;

    /// Whether the firmware controls the fans (false: something set them manually or to max).
    fn fan_is_auto(&mut self) -> Result<bool, Self::Error>;
}

/// Sends duties to the hardware only when they change and notices when something else (a plug
/// or unplug, a resume) reset the fans behind its back. `(0, 0)` means firmware Auto.
#[derive(Debug)]
pub struct FanWriter<H: FanHardware> {
    hw: H,
    written: Option<Duties>,
}

impl<H: FanHardware> FanWriter<H> {
    /// A writer over `hw`; nothing has been written yet.
    pub fn new(hw: H) -> FanWriter<H> {
        FanWriter { hw, written: None }
    }

    /// The hardware (for the daemon's explicit fan commands, which write through the same
    /// handle).
    pub fn hardware(&mut self) -> &mut H {
        &mut self.hw
    }

    /// Applies the duties if they differ from the last ones written. A single write holds
    /// (verified on the hardware), so there is no periodic rewrite. Returns whether anything
    /// was sent. A failed write is forgotten, so the next call tries again. A manual duty
    /// under 20 is raised to 20: the firmware would refuse it (a partial result, one fan
    /// handing back to Auto, cannot be expressed to the firmware).
    pub fn write(&mut self, duties: Duties) -> Result<bool, H::Error> {
        if self.written == Some(duties) {
            return Ok(false);
        }
        let sent = if duties.is_auto() {
            self.hw.fan_auto()
        } else {
            self.hw.fan_manual(duties.cpu.max(MIN_DUTY), duties.gpu.max(MIN_DUTY))
        };
        match sent {
            Ok(()) => {
                self.written = Some(duties);
                Ok(true)
            }
            Err(e) => {
                self.written = None;
                Err(e)
            }
        }
    }

    /// Makes the next [`FanWriter::write`] send its duties again (after a plug or unplug, a
    /// resume, or any fan change made by someone else, such as the user's explicit command).
    pub fn forget(&mut self) {
        self.written = None;
    }

    /// What was last written; `None` when unknown.
    pub fn written(&self) -> Option<Duties> {
        self.written
    }

    /// The word for the live state: `"auto"` for firmware control (or nothing written),
    /// `"cpu,gpu"` otherwise.
    pub fn written_label(&self) -> String {
        match self.written {
            Some(d) if !d.is_auto() => format!("{},{}", d.cpu, d.gpu),
            _ => "auto".to_owned(),
        }
    }

    /// Re-asserts the last duties if the firmware or the driver reset the fans behind our back
    /// (call it every 15 steps while the smart fan runs). Does nothing when nothing was written
    /// or the device is not answering (`hw_ok` false), and ignores a failed read.
    pub fn verify(&mut self, hw_ok: bool) {
        let Some(written) = self.written else { return };
        if !hw_ok {
            return;
        }
        let Ok(auto) = self.hw.fan_is_auto() else { return };
        if !auto != !written.is_auto() {
            self.written = None;
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::curves::Curve;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    /// A clock the test moves: the controller takes the elapsed seconds as an input, so the
    /// fake clock is what turns "time passes" into `dt_s`.
    struct FakeClock {
        now_s: f64,
    }

    impl FakeClock {
        fn new() -> FakeClock {
            FakeClock { now_s: 0.0 }
        }

        /// Advances by `secs` and returns the elapsed time for the step.
        fn advance(&mut self, secs: f64) -> f64 {
            self.now_s += secs;
            secs
        }
    }

    fn params() -> Params {
        Params::from_config(&FanConfig::default())
    }

    fn smart() -> Smart {
        Smart::new(params(), Curve::builtin("truesense").expect("built in"))
    }

    fn inp(activity: Activity, tcpu: f64, tgpu: f64) -> Inputs {
        Inputs { activity, tcpu, tgpu, tchassis: None, gpu_power_w: 0.0, on_ac: true, dt_s: 1.0 }
    }

    fn run(s: &mut Smart, clock: &mut FakeClock, n: usize, mut i: Inputs) -> Duties {
        let mut last = Duties::AUTO;
        for _ in 0..n {
            i.dt_s = clock.advance(1.0);
            last = s.step(&i);
        }
        last
    }

    #[test]
    fn config_defaults_match_python() {
        let p = params();
        assert_eq!((p.calm_cpu, p.calm_gpu, p.calm_hold), (58.0, 55.0, 20));
    }

    // ---- the controller -----------------------------------------------------------------------

    #[test]
    fn idle_heavy_hot_and_back() {
        // The Python test_idle_heavy_hot_and_back, without the temperature smoothing.
        let (mut s, mut clock) = (smart(), FakeClock::new());
        assert_eq!(run(&mut s, &mut clock, 25, inp(Activity::Idle, 45.0, 45.0)), Duties::AUTO);
        let d = run(&mut s, &mut clock, 8, inp(Activity::Heavy, 70.0, 60.0));
        assert!(d.cpu >= 20 && d.gpu >= 20, "heavy load: manual duties, got {d:?}");
        assert_eq!(run(&mut s, &mut clock, 1, inp(Activity::Hot, 93.0, 70.0)), Duties::FULL);
        assert_eq!(s.follower_duty(), (100.0, 100.0));
        // Cool again: the duty falls slowly, then after 20 calm steps the firmware has it.
        let d = run(&mut s, &mut clock, 45, inp(Activity::Idle, 50.0, 48.0));
        assert_eq!(d, Duties::AUTO);
        assert_eq!(s.follower_duty(), (0.0, 0.0));
    }

    #[test]
    fn calm_hold_is_exact() {
        // 45 C idle: the follower is at 0, so the very first calm step already hands over.
        let (mut s, mut clock) = (smart(), FakeClock::new());
        assert_eq!(run(&mut s, &mut clock, 1, inp(Activity::Idle, 45.0, 45.0)), Duties::AUTO);
        // Calm but the follower is above 20: stay on the curve until calm_hold (20) steps.
        let mut s = smart();
        s.follower.set_duty(100.0, 100.0);
        let mut i = inp(Activity::Idle, 57.0, 54.0);
        for step in 1..=19 {
            i.dt_s = clock.advance(1.0);
            let d = s.step(&i);
            assert!(!d.is_auto(), "step {step}: the fans stay with the curve while the hold runs");
            assert_eq!(s.calm_steps(), step);
        }
        i.dt_s = clock.advance(1.0);
        assert_eq!(s.step(&i), Duties::AUTO, "the 20th calm step hands over");
        assert_eq!(s.follower_duty(), (0.0, 0.0));
    }

    #[test]
    fn calm_thresholds_are_strict_and_per_chip() {
        // (activity, cpu, gpu, calm?)
        let cases = [
            (Activity::Idle, 57.99, 54.99, true),
            (Activity::Browsing, 57.99, 54.99, true),
            (Activity::Idle, 58.0, 45.0, false),
            (Activity::Idle, 45.0, 55.0, false),
            (Activity::Heavy, 45.0, 45.0, false),
            (Activity::Gaming, 45.0, 45.0, false),
            (Activity::Hot, 45.0, 45.0, false),
        ];
        for (a, c, g, calm) in cases {
            let mut s = smart();
            s.step(&inp(a, c, g));
            assert_eq!(s.calm_steps(), u32::from(calm), "{a} {c}/{g}");
        }
        // Not calm resets the counter.
        let mut s = smart();
        s.follower.set_duty(100.0, 100.0);
        s.step(&inp(Activity::Idle, 50.0, 50.0));
        s.step(&inp(Activity::Idle, 50.0, 50.0));
        assert_eq!(s.calm_steps(), 2);
        s.step(&inp(Activity::Idle, 60.0, 50.0));
        assert_eq!(s.calm_steps(), 0);
        s.reset();
        assert_eq!(s.calm_steps(), 0);
    }

    #[test]
    fn calm_hold_of_zero_or_less_always_hands_over() {
        for hold in [0, -3] {
            let mut s = Smart::new(
                Params { calm_hold: hold, ..params() },
                Curve::builtin("truesense").expect("built in"),
            );
            s.follower.set_duty(100.0, 100.0);
            assert_eq!(s.step(&inp(Activity::Heavy, 80.0, 70.0)), Duties::AUTO, "hold {hold}");
        }
    }

    #[test]
    fn hot_forces_full_speed_on_both_fans_even_on_battery() {
        let mut s = smart();
        let mut i = inp(Activity::Hot, 93.0, 40.0);
        i.on_ac = false;
        assert_eq!(s.step(&i), Duties::FULL);
        assert_eq!(s.calm_steps(), 0);
        // Hot beats the calm shortcut only when it is not calm; a hot activity never is.
        i.tcpu = 30.0;
        i.tgpu = 30.0;
        assert_eq!(s.step(&i), Duties::FULL);
    }

    #[test]
    fn the_history_is_only_fed_on_curve_steps() {
        // Four curve steps at +1 C each, a hot step (it returns before the history is fed),
        // then a fifth curve step: the slope over the five fed samples is 1 C per sample, so
        // the lookup is `predict` (5) degrees ahead. Had the hot step been fed (95 C), the
        // slope would be large and the window would differ. The follower is zeroed after the
        // hot step so the duty shows the raw target.
        let mut s = smart();
        for k in 0..4 {
            s.step(&inp(Activity::Heavy, 60.0 + f64::from(k), 50.0));
        }
        assert_eq!(s.step(&inp(Activity::Hot, 95.0, 50.0)), Duties::FULL);
        s.follower.set_duty(0.0, 0.0);
        s.step(&inp(Activity::Heavy, 64.0, 50.0));
        let cpu = s.curve().cpu.clone();
        assert_eq!(s.follower_duty().0, crate::curves::interp(&cpu, 64.0 + 5.0));
        assert_ne!(s.follower_duty().0, crate::curves::interp(&cpu, 64.0));
    }

    #[test]
    fn rising_needs_one_degree_per_sample_over_five_samples() {
        let curve = Curve::builtin("truesense").expect("built in");
        let at = |temps: [f64; 5]| {
            let mut s = Smart::new(params(), curve.clone());
            let mut last = Duties::AUTO;
            for t in temps {
                last = s.step(&inp(Activity::Browsing, t, 40.0));
            }
            (last, s.follower_duty().0)
        };
        // 63 -> 67 is +1.0 per sample: rising, so 67 + 5 is looked up.
        let (_, rising) = at([63.0, 64.0, 65.0, 66.0, 67.0]);
        assert_eq!(rising, crate::curves::interp(&curve.cpu, 72.0));
        // 63 -> 66.9 is slope 0.975: not rising, looked up at 66.9 as it is.
        let (_, steady) = at([63.0, 64.0, 65.0, 66.0, 66.9]);
        assert!(steady < rising);
    }

    #[test]
    fn battery_makes_the_fans_come_in_later() {
        let (mut ac, mut bat) = (smart(), smart());
        let mut a = inp(Activity::Browsing, 66.0, 60.0);
        let mut b = a;
        b.on_ac = false;
        let (mut ac1, mut bat1) = (Duties::AUTO, Duties::AUTO);
        for _ in 0..5 {
            ac1 = ac.step(&a);
            bat1 = bat.step(&b);
        }
        assert!(bat1.cpu < ac1.cpu, "on battery the fans must run softer: ac {ac1:?}, battery {bat1:?}");
        // The shift is exactly battery_shift: 66 C on battery looks up 63 C.
        let curve = Curve::builtin("truesense").expect("built in");
        assert_eq!(bat.follower_duty().0, crate::curves::interp(&curve.cpu, 66.0 - 3.0));
        a.on_ac = false;
        assert_eq!(a, b);
    }

    #[test]
    fn falling_is_slow_and_follows_dt() {
        let (mut s, mut clock) = (smart(), FakeClock::new());
        let mut i = inp(Activity::Heavy, 80.0, 40.0);
        i.dt_s = clock.advance(1.0);
        s.step(&i);
        assert_eq!(s.follower_duty().0, 70.0);
        // Down to 74 C at 2 s per step: 1.5 %/s * 2 s = 3 % per step.
        i.tcpu = 74.0;
        i.dt_s = clock.advance(2.0);
        s.step(&i);
        assert_eq!(s.follower_duty().0, 67.0);
        assert_eq!(clock.now_s, 3.0);
    }

    #[test]
    fn set_curve_resets_and_switches_the_curve() {
        let mut s = smart();
        s.step(&inp(Activity::Heavy, 89.0, 78.0));
        assert_eq!(s.follower_duty(), (100.0, 100.0));
        s.set_curve(Curve::builtin("quiet").expect("built in"));
        assert_eq!(s.follower_duty(), (0.0, 0.0));
        assert_eq!(s.curve(), &Curve::builtin("quiet").expect("built in"));
        s.set_params(Params { calm_hold: 3, ..params() });
        assert_eq!(s.params().calm_hold, 3);
    }

    #[test]
    fn every_output_obeys_the_fan_rule() {
        // A sweep over activities, temperatures and power sources: a duty is never 1..19 or
        // above 100, and (0, 0) is the only way to ask for firmware control.
        for curve in crate::curves::BUILTIN_NAMES {
            let mut s = Smart::new(params(), Curve::builtin(curve).expect("built in"));
            for act in [Activity::Idle, Activity::Browsing, Activity::Heavy, Activity::Gaming, Activity::Hot]
            {
                for t in (30..=105).step_by(3) {
                    for on_ac in [true, false] {
                        let mut i = inp(act, f64::from(t), f64::from(t) - 8.0);
                        i.on_ac = on_ac;
                        i.gpu_power_w = f64::from(t);
                        i.tchassis = Some(f64::from(t) - 10.0);
                        let d = s.step(&i);
                        let ok = |v: u8| (MIN_DUTY..=MAX_DUTY).contains(&v);
                        assert!(d.is_auto() || (ok(d.cpu) && ok(d.gpu)), "{curve} {act} {t}: {d:?}");
                        if act == Activity::Hot {
                            assert_eq!(d, Duties::FULL);
                        }
                    }
                }
            }
        }
    }

    // ---- the floor ----------------------------------------------------------------------------

    #[test]
    fn floor_while_gaming_and_working() {
        // (activity, gpu watts, chassis, floor)
        let cases = [
            (Activity::Idle, 0.0, None, 0),
            (Activity::Browsing, 0.0, None, 0),
            (Activity::Heavy, 10.0, None, 40),
            (Activity::Gaming, 40.0, None, 50),
            (Activity::Gaming, 79.99, None, 50),
            (Activity::Gaming, 80.0, None, 60),
            (Activity::Gaming, 90.0, None, 60),
            (Activity::Heavy, 0.0, Some(62.99), 40),
            (Activity::Heavy, 0.0, Some(63.0), 50),
            (Activity::Heavy, 0.0, Some(64.0), 50),
            (Activity::Heavy, 0.0, Some(67.99), 50),
            (Activity::Heavy, 0.0, Some(68.0), 60),
            (Activity::Gaming, 40.0, Some(70.0), 60),
            (Activity::Gaming, 90.0, Some(70.0), 60),
            (Activity::Idle, 0.0, Some(70.0), 0),
            (Activity::Browsing, 100.0, Some(90.0), 0),
            (Activity::Hot, 100.0, Some(90.0), 0),
            (Activity::Heavy, 0.0, Some(0.0), 40),
        ];
        for (a, w, chassis, want) in cases {
            assert_eq!(floor_pct(a, w, chassis), want, "{a} {w} W chassis {chassis:?}");
        }
    }

    #[test]
    fn the_floor_is_applied_to_the_output() {
        let mut s = smart();
        let mut i = inp(Activity::Gaming, 50.0, 50.0);
        i.gpu_power_w = 90.0;
        let d = s.step(&i);
        assert!(d.cpu >= 60 && d.gpu >= 60, "a gaming floor of 60 % must hold even at 50 C: {d:?}");
        let mut s = smart();
        let d = s.step(&inp(Activity::Heavy, 50.0, 50.0));
        assert_eq!((d.cpu, d.gpu), (40, 40));
    }

    // ---- the explicit stop --------------------------------------------------------------------

    fn temps(cpu: f64, gpu: f64) -> Temps {
        Temps { cpu: Some(cpu), gpu: Some(gpu) }
    }

    #[test]
    fn duty_rule_for_manual_requests() {
        for v in [1, 10, 19, 101, 150, -1, -20, i64::MAX, i64::MIN] {
            assert_eq!(plan_manual(v, Some(60), true, temps(40.0, 40.0)), Err(FanError::BadDuty), "cpu {v}");
            assert_eq!(plan_manual(60, Some(v), true, temps(40.0, 40.0)), Err(FanError::BadDuty), "gpu {v}");
        }
        for v in [20, 21, 55, 99, 100] {
            let p = plan_manual(v, None, false, temps(99.0, 99.0)).expect("a normal duty needs no guard");
            assert_eq!((i64::from(p.cpu), i64::from(p.gpu)), (v, v));
            assert!(!p.stops_a_fan());
            assert_eq!(p.outcome(), "manual");
            assert_eq!(p.stopped_which(), None);
        }
        // The python test: {"cpu": 10, "gpu": 60} and {"cpu": 0, "gpu": 150} with confirm.
        assert_eq!(plan_manual(10, Some(60), true, temps(45.0, 45.0)), Err(FanError::BadDuty));
        assert_eq!(plan_manual(0, Some(150), true, temps(45.0, 45.0)), Err(FanError::BadDuty));
        // The bad duty is reported before the missing confirmation.
        assert_eq!(plan_manual(10, Some(0), false, temps(45.0, 45.0)), Err(FanError::BadDuty));
    }

    #[test]
    fn stopping_needs_confirmation_then_a_cool_machine() {
        let cool = temps(45.0, 45.0);
        // Python test_fans_off_is_guarded: no confirmation.
        assert_eq!(plan_off(false, cool), Err(FanError::NeedsConfirmation));
        assert_eq!(plan_manual(0, Some(60), false, cool), Err(FanError::NeedsConfirmation));
        assert_eq!(plan_manual(60, Some(0), false, cool), Err(FanError::NeedsConfirmation));
        assert_eq!(plan_manual(0, None, false, cool), Err(FanError::NeedsConfirmation));
        // Confirmed and cool: both fans, or either one.
        let off = plan_off(true, cool).expect("cool");
        assert_eq!((off.cpu, off.gpu, off.outcome(), off.stopped_which()), (0, 0, "off", Some("both fans")));
        let cpu_stop = plan_manual(0, Some(60), true, cool).expect("cool");
        assert_eq!((cpu_stop.outcome(), cpu_stop.stopped_which()), ("partial-off", Some("the CPU fan")));
        let gpu_stop = plan_manual(70, Some(0), true, cool).expect("cool");
        assert_eq!((gpu_stop.cpu, gpu_stop.gpu, gpu_stop.stopped_which()), (70, 0, Some("the GPU fan")));
        assert!(gpu_stop.stops_a_fan());
        // Too warm to start (Python: drive 66 C CPU).
        let e = plan_off(true, temps(66.0, 50.0)).expect_err("warm");
        assert_eq!(
            e.to_string(),
            "too warm to stop a fan (CPU 66 C, GPU 50 C; a fan may only be stopped below 60 / 55 C)"
        );
    }

    #[test]
    fn stop_start_thresholds_are_60_and_55_strictly_below() {
        let confirm = true;
        assert!(plan_off(confirm, temps(59.99, 54.99)).is_ok());
        assert!(plan_off(confirm, temps(60.0, 40.0)).is_err(), "CPU at 60 is too warm");
        assert!(plan_off(confirm, temps(40.0, 55.0)).is_err(), "GPU at 55 is too warm");
        assert!(plan_off(confirm, temps(60.01, 54.0)).is_err());
        // An unknown CPU temperature refuses; an unknown GPU temperature counts as 0.
        let e = plan_off(confirm, Temps { cpu: None, gpu: Some(40.0) }).expect_err("unknown cpu");
        assert_eq!(e, FanError::TooWarm { cpu: 0, gpu: 40 });
        assert!(plan_off(confirm, Temps { cpu: Some(40.0), gpu: None }).is_ok());
        assert!(plan_off(confirm, Temps::default()).is_err());
        // The message rounds half to even like Python's round(): 59.5 -> 60, 60.5 -> 60.
        assert_eq!(can_stop(temps(60.5, 40.0)), Err(FanError::TooWarm { cpu: 60, gpu: 40 }));
        assert_eq!(can_stop(temps(61.5, 55.5)), Err(FanError::TooWarm { cpu: 62, gpu: 56 }));
        // A stop is only ever gated; a normal duty is never refused for warmth.
        assert!(plan_manual(50, Some(50), false, temps(80.0, 80.0)).is_ok());
    }

    #[test]
    fn watchdog_thresholds_are_70_and_65() {
        // Python test_fans_off_is_guarded: 75 C CPU gives the fans back; 50/45 does not.
        assert!(!must_return_fans(temps(69.99, 64.99), true), "below the line the stop stands");
        assert!(must_return_fans(temps(70.0, 40.0), true));
        assert!(must_return_fans(temps(40.0, 65.0), true));
        assert!(must_return_fans(temps(75.0, 50.0), true));
        assert!(must_return_fans(temps(40.0, 40.0), false), "a silent device gives the fans back");
        assert!(!must_return_fans(Temps::default(), true), "no reading yet: nothing to act on");
        assert!(must_return_fans(Temps::default(), false));
        // The stop-start line is below the watchdog line, so a fan can never be stopped into
        // an immediate hand-back.
        const {
            assert!(STOP_START_CPU_C < STOP_MAX_CPU_C && STOP_START_GPU_C < STOP_MAX_GPU_C);
        }
    }

    #[test]
    fn the_watchdog_fires_once_per_stop() {
        let mut w = StopWatchdog::new();
        assert!(!w.is_armed());
        assert!(!w.check(temps(90.0, 90.0), false), "nothing stopped, nothing to give back");
        w.arm();
        assert!(w.is_armed());
        for _ in 0..3 {
            assert!(!w.check(temps(50.0, 45.0), true), "stays off while it is cool");
        }
        assert!(w.check(temps(75.0, 50.0), true), "the laptop got warm: give ALL fans back");
        assert!(!w.is_armed());
        assert!(!w.check(temps(75.0, 50.0), true), "once only");
        // A normal manual speed, or the smart fan, ends the stopped state.
        w.arm();
        w.disarm();
        assert!(!w.check(temps(99.0, 99.0), false));
        // The device failing also fires it.
        w.arm();
        assert!(w.check(temps(40.0, 40.0), false));
    }

    #[test]
    fn a_stop_scenario_over_a_fake_clock() {
        // Python test_one_fan_can_be_stopped_with_the_same_guards, as readings over time.
        let mut clock = FakeClock::new();
        let mut w = StopWatchdog::new();
        let plan = plan_manual(0, Some(60), true, temps(45.0, 45.0)).expect("cool");
        assert!(plan.stops_a_fan());
        w.arm();
        let mut fired_at = None;
        for tick in 0..20 {
            clock.advance(1.0);
            let t = if tick < 3 { temps(50.0, 45.0) } else { temps(75.0, 50.0) };
            if w.check(t, true) {
                fired_at = Some(clock.now_s);
                break;
            }
        }
        assert_eq!(fired_at, Some(4.0));
    }

    // ---- the writer ---------------------------------------------------------------------------

    #[derive(Debug)]
    struct Boom;

    impl fmt::Display for Boom {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("boom")
        }
    }

    impl std::error::Error for Boom {}

    #[derive(Default)]
    struct State {
        calls: Vec<String>,
        auto: bool,
        fail: bool,
        read_fails: bool,
        manual_args: Vec<(u8, u8)>,
    }

    #[derive(Clone, Default)]
    struct FakeFans(Rc<RefCell<State>>);

    impl FanHardware for FakeFans {
        type Error = Boom;

        fn fan_auto(&mut self) -> Result<(), Boom> {
            let mut s = self.0.borrow_mut();
            s.calls.push("auto".into());
            s.auto = true;
            if s.fail { Err(Boom) } else { Ok(()) }
        }

        fn fan_manual(&mut self, cpu: u8, gpu: u8) -> Result<(), Boom> {
            let mut s = self.0.borrow_mut();
            s.calls.push("manual".into());
            s.manual_args.push((cpu, gpu));
            s.auto = false;
            if s.fail { Err(Boom) } else { Ok(()) }
        }

        fn fan_is_auto(&mut self) -> Result<bool, Boom> {
            let s = self.0.borrow();
            if s.read_fails { Err(Boom) } else { Ok(s.auto) }
        }
    }

    fn d(cpu: u8, gpu: u8) -> Duties {
        Duties { cpu, gpu }
    }

    #[test]
    fn writer_only_writes_on_change() {
        let fans = FakeFans::default();
        fans.0.borrow_mut().auto = true;
        let mut w = FanWriter::new(fans.clone());
        assert_eq!(w.written_label(), "auto");
        assert!(w.write(d(40, 40)).expect("write"));
        assert!(!w.write(d(40, 40)).expect("write"));
        w.write(d(50, 50)).expect("write");
        w.write(Duties::AUTO).expect("write");
        assert!(!w.write(Duties::AUTO).expect("write"));
        assert_eq!(fans.0.borrow().calls, ["manual", "manual", "auto"]);
        assert_eq!(w.written_label(), "auto");
        assert_eq!(w.written(), Some(Duties::AUTO));
        w.write(d(35, 60)).expect("write");
        assert_eq!(w.written_label(), "35,60");
        w.hardware().fan_auto().expect("direct");
        assert_eq!(fans.0.borrow().calls.last().map(String::as_str), Some("auto"));
    }

    #[test]
    fn writer_never_sends_a_duty_under_20() {
        let fans = FakeFans::default();
        let mut w = FanWriter::new(fans.clone());
        w.write(d(0, 40)).expect("write");
        w.write(d(30, 5)).expect("write");
        assert_eq!(fans.0.borrow().manual_args, [(20, 40), (30, 20)]);
    }

    #[test]
    fn a_failed_write_is_retried() {
        let fans = FakeFans::default();
        fans.0.borrow_mut().fail = true;
        let mut w = FanWriter::new(fans.clone());
        assert!(w.write(d(40, 40)).is_err());
        assert_eq!(w.written(), None);
        fans.0.borrow_mut().fail = false;
        assert!(w.write(d(40, 40)).expect("retry"));
        assert_eq!(fans.0.borrow().calls.len(), 2, "a failed write must be tried again");
        // A failure after a good write forgets it too.
        fans.0.borrow_mut().fail = true;
        assert!(w.write(d(50, 50)).is_err());
        assert_eq!(w.written(), None);
    }

    #[test]
    fn verify_notices_fans_reset_behind_our_back() {
        let fans = FakeFans::default();
        let mut w = FanWriter::new(fans.clone());
        w.write(d(50, 50)).expect("write");
        fans.0.borrow_mut().auto = true; // something handed the fans back (resume, plug)
        w.verify(true);
        assert_eq!(w.written(), None);
        w.write(d(50, 50)).expect("write");
        assert_eq!(fans.0.borrow().calls.len(), 2, "after a reset the duties are asserted again");
        w.verify(true); // consistent now
        w.write(d(50, 50)).expect("write");
        assert_eq!(fans.0.borrow().calls.len(), 2, "no spurious rewrite");
        // The other direction: we think Auto but the firmware is on manual.
        w.write(Duties::AUTO).expect("write");
        fans.0.borrow_mut().auto = false;
        w.verify(true);
        assert_eq!(w.written(), None);
    }

    #[test]
    fn verify_does_nothing_without_a_write_a_device_or_an_answer() {
        let fans = FakeFans::default();
        let mut w = FanWriter::new(fans.clone());
        w.verify(true);
        assert_eq!(w.written(), None);
        w.write(d(50, 50)).expect("write");
        fans.0.borrow_mut().auto = true;
        w.verify(false);
        assert_eq!(w.written(), Some(d(50, 50)), "device not answering: leave it");
        fans.0.borrow_mut().read_fails = true;
        w.verify(true);
        assert_eq!(w.written(), Some(d(50, 50)), "a failed read changes nothing");
        w.forget();
        assert_eq!(w.written(), None);
    }

    #[test]
    fn the_controller_drives_the_writer_like_the_daemon_does() {
        // 25 idle steps, a heavy burst, a hot spike and a long calm, with verify every 15 steps.
        let fans = FakeFans::default();
        fans.0.borrow_mut().auto = true;
        let mut w = FanWriter::new(fans.clone());
        let (mut s, mut clock) = (smart(), FakeClock::new());
        let script = [
            (25, Activity::Idle, 45.0, 45.0),
            (8, Activity::Heavy, 70.0, 60.0),
            (2, Activity::Hot, 93.0, 70.0),
            (45, Activity::Idle, 50.0, 48.0),
        ];
        let mut tick = 0;
        let mut checkpoints = Vec::new();
        for (n, act, c, g) in script {
            for _ in 0..n {
                tick += 1;
                let mut i = inp(act, c, g);
                i.dt_s = clock.advance(1.0);
                w.write(s.step(&i)).expect("write");
                if tick % 15 == 0 {
                    w.verify(true);
                }
            }
            checkpoints.push((w.written_label(), fans.0.borrow().auto));
        }
        assert_eq!(checkpoints[0], ("auto".to_owned(), true));
        assert!(!checkpoints[1].1);
        assert_eq!(checkpoints[2], ("100,100".to_owned(), false));
        assert_eq!(checkpoints[3], ("auto".to_owned(), true));
    }

    #[test]
    fn activity_words_match_python() {
        let words: Vec<_> =
            [Activity::Idle, Activity::Browsing, Activity::Heavy, Activity::Gaming, Activity::Hot]
                .iter()
                .map(|a| a.to_string())
                .collect();
        assert_eq!(words, ["idle", "browsing", "heavy", "gaming", "hot"]);
    }

    #[test]
    fn a_queue_of_readings_is_deterministic() {
        // Same readings, same duties: the controller has no clock and no hidden input.
        let mut script: VecDeque<Inputs> = VecDeque::new();
        for k in 0..200u32 {
            let t = 45.0 + f64::from(k % 50);
            let act = if t > 85.0 {
                Activity::Hot
            } else if t > 60.0 {
                Activity::Heavy
            } else {
                Activity::Idle
            };
            let mut i = inp(act, t, t - 10.0);
            i.on_ac = k % 4 != 0;
            script.push_back(i);
        }
        let play = |script: &VecDeque<Inputs>| {
            let mut s = smart();
            script.iter().map(|i| s.step(i)).collect::<Vec<_>>()
        };
        assert_eq!(play(&script), play(&script));
    }
}
