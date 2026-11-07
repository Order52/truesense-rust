//! Desktop alerts: when the laptop should tell its user something without being asked.
//!
//! The rules are pure: [`Evaluator::step`] takes the clock and one [`Sample`] of the readings
//! the daemon already has (smoothed temperatures, the battery, whether the thermal governor is
//! holding the clock down) and answers with [`Event`]s. Showing a message is the caller's job,
//! so these rules run, and are tested, with a fake clock and no desktop.
//!
//! What it watches, in the order it is evaluated (the order also decides who gets the slot when
//! two alerts are due in the same minute):
//!
//! | alert | id | severity | on | off |
//! |---|---|---|---|---|
//! | CPU temperature sensor lost | `sensor-lost` | warning | no CPU reading for N samples | a reading for N samples |
//! | CPU hot | `cpu-temp-high` | warning | at or above the limit for N samples | 5 C below the limit |
//! | GPU hot | `gpu-temp-high` | warning | at or above the limit for N samples | 5 C below the limit |
//! | Battery low | `battery-low` | warning | on battery, at or below the limit for N samples | charger plugged, or 5 points above |
//! | Thermal throttling suspected | `throttling` | info | the governor holds the clock down for N samples | released for N samples |
//!
//! Three things keep a laptop that hovers around a limit from pestering its user:
//!
//! * **debounce**: a condition must hold for N consecutive samples before it starts an episode,
//!   so one noisy reading is not an alert;
//! * **hysteresis**: an episode of a temperature or battery alert ends only 5 C (points) below
//!   the limit, so 89, 90, 89, 90 is one episode, not four;
//! * **rate limits**: an alert is shown once per episode, never twice within `repeat_s` of its
//!   own previous message (an episode that starts too soon waits and is shown when the time is
//!   up, if it is still going), and no two messages of any kind come within [`MIN_GAP_S`].
//!
//! The severity is deliberately only ever [`Severity::Info`] or [`Severity::Warning`]. The
//! reference project's finding 36: a notification with "critical" urgency is never dismissed by
//! the desktop, and a temperature that stays high would leave a permanent banner on the screen.
#![forbid(unsafe_code)]

use ts_foundation::config::AlertsConfig;

/// An episode of a temperature or battery alert ends this far below its limit.
pub(crate) const REARM_BELOW: f64 = 5.0;
/// The least time in seconds between two messages of any kind: one a minute across all sensors.
pub(crate) const MIN_GAP_S: f64 = 60.0;

/// How serious an alert is. There is no "critical" on purpose (see the module documentation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Worth knowing, nothing to do about it.
    Info,
    /// The user may want to act (stop what is running, plug in the charger).
    Warning,
}

impl Severity {
    /// The word used in logs and on the control socket.
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
        }
    }
}

/// Which alert. The ids are part of the log format and of the bus, so they never change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AlertId {
    /// The CPU temperature cannot be read.
    SensorLost,
    /// The CPU is at or above its limit.
    CpuTemp,
    /// The GPU is at or above its limit.
    GpuTemp,
    /// The battery is low and not charging.
    BatteryLow,
    /// The thermal governor is holding the CPU clock down.
    Throttling,
}

impl AlertId {
    /// Every alert, in evaluation order.
    pub const ALL: [AlertId; 5] =
        [AlertId::SensorLost, AlertId::CpuTemp, AlertId::GpuTemp, AlertId::BatteryLow, AlertId::Throttling];

    /// The stable id.
    pub fn as_str(self) -> &'static str {
        match self {
            AlertId::SensorLost => "sensor-lost",
            AlertId::CpuTemp => "cpu-temp-high",
            AlertId::GpuTemp => "gpu-temp-high",
            AlertId::BatteryLow => "battery-low",
            AlertId::Throttling => "throttling",
        }
    }

    /// The icon name the on-screen display is given.
    pub fn icon(self) -> &'static str {
        match self {
            AlertId::BatteryLow => "battery",
            _ => "fan",
        }
    }
}

/// One message for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    /// Which alert.
    pub id: AlertId,
    /// How serious.
    pub severity: Severity,
    /// A short English sentence, with the numbers of the moment it was raised.
    pub message: String,
}

/// What [`Evaluator::step`] reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// An episode began: log it (once per episode). It may be shown later, or never when it ends
    /// first.
    Began(Alert),
    /// Show this to the user now.
    Notify(Alert),
    /// The episode is over.
    Cleared(AlertId),
}

/// The rules' settings: [`AlertsConfig`] in the units the rules compute in.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// Alerts at all.
    pub enabled: bool,
    /// CPU limit in degrees C.
    pub cpu_limit_c: f64,
    /// GPU limit in degrees C.
    pub gpu_limit_c: f64,
    /// Least seconds between two messages of the same alert.
    pub repeat_s: f64,
    /// Consecutive samples a condition must hold.
    pub debounce: u32,
    /// Battery alert level in percent; 0 is off.
    pub battery_low_pct: f64,
    /// The throttling alert.
    pub throttling: bool,
    /// The sensor-lost alert.
    pub sensor_lost: bool,
}

impl From<&AlertsConfig> for Settings {
    /// Out-of-range numbers (a file that was never validated) are clamped instead of trusted: a
    /// zero debounce would never end, a negative repeat would let every sample through.
    fn from(c: &AlertsConfig) -> Self {
        Settings {
            enabled: c.enabled,
            cpu_limit_c: c.cpu_limit_c.clamp(30, 150) as f64,
            gpu_limit_c: c.gpu_limit_c.clamp(30, 150) as f64,
            repeat_s: c.repeat_minutes.clamp(1, 24 * 60) as f64 * 60.0,
            debounce: u32::try_from(c.debounce_samples.clamp(1, 600)).unwrap_or(1),
            battery_low_pct: c.battery_low_pct.clamp(0, 100) as f64,
            throttling: c.throttling,
            sensor_lost: c.sensor_lost,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings::from(&AlertsConfig::default())
    }
}

/// The readings of one tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// The smoothed CPU temperature; `None` when no sensor answered this tick.
    pub cpu_c: Option<f64>,
    /// The smoothed GPU temperature; `None` when the GPU gave no reading (asleep, absent).
    pub gpu_c: Option<f64>,
    /// The battery charge in percent.
    pub battery_pct: f64,
    /// Whether the charger is plugged in.
    pub on_ac: bool,
    /// Whether the thermal governor is holding the CPU clock below its maximum.
    pub throttled: bool,
}

/// One alert's episode state.
#[derive(Debug, Clone, Default)]
struct Latch {
    active: bool,
    on_run: u32,
    off_run: u32,
    /// Whether this episode has been shown already.
    sent: bool,
    /// When this alert was last shown, kept across episodes for the repeat limit.
    last_sent: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge {
    Nothing,
    Began,
    Cleared,
}

impl Latch {
    /// Feeds one sample: `on` is the trigger condition, `off` the release condition (they are not
    /// opposites: between them lies the hysteresis band where nothing changes).
    fn feed(&mut self, on: bool, off: bool, need_on: u32, need_off: u32) -> Edge {
        if self.active {
            self.off_run = if off { self.off_run.saturating_add(1) } else { 0 };
            if self.off_run >= need_off {
                self.reset();
                return Edge::Cleared;
            }
        } else {
            self.on_run = if on { self.on_run.saturating_add(1) } else { 0 };
            if self.on_run >= need_on {
                self.active = true;
                self.on_run = 0;
                self.off_run = 0;
                self.sent = false;
                return Edge::Began;
            }
        }
        Edge::Nothing
    }

    /// Ends the episode without a report (the rule was switched off). `last_sent` stays.
    fn reset(&mut self) {
        let last_sent = self.last_sent;
        *self = Latch { last_sent, ..Latch::default() };
    }

    /// Whether this episode may be shown now: not yet shown, and not too soon after the last one.
    fn due(&self, now: f64, repeat_s: f64) -> bool {
        self.active && !self.sent && self.last_sent.is_none_or(|t| now < t || now - t >= repeat_s)
    }

    fn mark_sent(&mut self, now: f64) {
        self.sent = true;
        self.last_sent = Some(now);
    }
}

/// The rules with their memory. One per daemon.
#[derive(Debug, Clone)]
pub struct Evaluator {
    settings: Settings,
    latches: [Latch; 5],
    last_any: Option<f64>,
}

impl Evaluator {
    /// Rules with these settings and no history.
    pub fn new(settings: Settings) -> Self {
        Evaluator { settings, latches: Default::default(), last_any: None }
    }

    /// The settings in force.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Replaces the settings (the configuration file changed). Turning everything off ends every
    /// episode silently; a changed limit simply applies to the next sample.
    pub fn set_settings(&mut self, settings: Settings) {
        self.settings = settings;
        if !self.settings.enabled {
            self.latches.iter_mut().for_each(Latch::reset);
        }
    }

    /// Whether an episode of `id` is going on.
    pub fn is_active(&self, id: AlertId) -> bool {
        self.latches[index(id)].active
    }

    /// Evaluates one sample taken at `now` (monotonic seconds) and returns what happened, in
    /// evaluation order.
    pub fn step(&mut self, now: f64, sample: &Sample) -> Vec<Event> {
        let mut events = Vec::new();
        if !self.settings.enabled {
            return events;
        }
        let s = self.settings.clone();
        let cpu = sample.cpu_c.filter(|t| t.is_finite());
        let gpu = sample.gpu_c.filter(|t| t.is_finite());
        let n = s.debounce;
        let low = s.battery_low_pct;
        for id in AlertId::ALL {
            let (switched_on, on, off, need_on, need_off) = match id {
                AlertId::SensorLost => (s.sensor_lost, cpu.is_none(), cpu.is_some(), n, n),
                AlertId::CpuTemp => (true, hot(cpu, s.cpu_limit_c), cooled(cpu, s.cpu_limit_c), n, 1),
                AlertId::GpuTemp => (true, hot(gpu, s.gpu_limit_c), cooled(gpu, s.gpu_limit_c), n, 1),
                AlertId::BatteryLow => (
                    low > 0.0,
                    !sample.on_ac && sample.battery_pct <= low,
                    sample.on_ac || sample.battery_pct >= low + REARM_BELOW,
                    n,
                    1,
                ),
                AlertId::Throttling => (s.throttling, sample.throttled, !sample.throttled, n, n),
            };
            let latch = &mut self.latches[index(id)];
            if !switched_on {
                latch.reset();
                continue;
            }
            match latch.feed(on, off, need_on, need_off) {
                Edge::Began => events.push(Event::Began(describe(id, sample, &s))),
                Edge::Cleared => events.push(Event::Cleared(id)),
                Edge::Nothing => {}
            }
            let gap_over = self.last_any.is_none_or(|t| now < t || now - t >= MIN_GAP_S);
            if gap_over && latch.due(now, s.repeat_s) {
                latch.mark_sent(now);
                self.last_any = Some(now);
                events.push(Event::Notify(describe(id, sample, &s)));
            }
        }
        events
    }
}

fn index(id: AlertId) -> usize {
    match id {
        AlertId::SensorLost => 0,
        AlertId::CpuTemp => 1,
        AlertId::GpuTemp => 2,
        AlertId::BatteryLow => 3,
        AlertId::Throttling => 4,
    }
}

fn hot(temp: Option<f64>, limit: f64) -> bool {
    temp.is_some_and(|t| t >= limit)
}

/// A missing reading ends a temperature episode too: a GPU that went to sleep is not hot.
fn cooled(temp: Option<f64>, limit: f64) -> bool {
    temp.is_none_or(|t| t <= limit - REARM_BELOW)
}

fn describe(id: AlertId, sample: &Sample, s: &Settings) -> Alert {
    let shown =
        |t: Option<f64>| t.map_or_else(|| "?".to_owned(), |t| format!("{}", t.round_ties_even() as i64));
    let (severity, message) = match id {
        AlertId::SensorLost => (
            Severity::Warning,
            "The CPU temperature cannot be read; the fans are left to the firmware".to_owned(),
        ),
        AlertId::CpuTemp => (
            Severity::Warning,
            format!("CPU is hot: {} C (alert limit {} C)", shown(sample.cpu_c), s.cpu_limit_c),
        ),
        AlertId::GpuTemp => (
            Severity::Warning,
            format!("GPU is hot: {} C (alert limit {} C)", shown(sample.gpu_c), s.gpu_limit_c),
        ),
        AlertId::BatteryLow => (
            Severity::Warning,
            format!("Battery low: {} %, plug in the charger", sample.battery_pct.round_ties_even() as i64),
        ),
        AlertId::Throttling => {
            (Severity::Info, "The CPU clock is held down to keep the laptop cool".to_owned())
        }
    };
    Alert { id, severity, message }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clock the tests move by hand, so no test waits for real.
    struct FakeClock(f64);

    impl FakeClock {
        fn advance(&mut self, s: f64) -> f64 {
            self.0 += s;
            self.0
        }
    }

    fn cool() -> Sample {
        Sample { cpu_c: Some(50.0), gpu_c: Some(45.0), battery_pct: 80.0, on_ac: true, throttled: false }
    }

    fn cpu(t: f64) -> Sample {
        Sample { cpu_c: Some(t), ..cool() }
    }

    fn settings() -> Settings {
        Settings {
            enabled: true,
            cpu_limit_c: 90.0,
            gpu_limit_c: 90.0,
            repeat_s: 300.0,
            debounce: 3,
            battery_low_pct: 15.0,
            throttling: true,
            sensor_lost: true,
        }
    }

    /// Runs `samples` one second apart and returns the notified ids with the second they came at.
    fn run(ev: &mut Evaluator, clock: &mut FakeClock, samples: &[Sample]) -> Vec<(AlertId, u32)> {
        let mut out = Vec::new();
        for (i, s) in samples.iter().enumerate() {
            let now = clock.advance(1.0);
            for e in ev.step(now, s) {
                if let Event::Notify(a) = e {
                    out.push((a.id, i as u32 + 1));
                }
            }
        }
        out
    }

    fn temps(values: &[f64]) -> Vec<Sample> {
        values.iter().map(|t| cpu(*t)).collect()
    }

    #[test]
    fn ids_severities_and_messages_are_stable() {
        let ids: Vec<&str> = AlertId::ALL.iter().map(|i| i.as_str()).collect();
        assert_eq!(ids, ["sensor-lost", "cpu-temp-high", "gpu-temp-high", "battery-low", "throttling"]);
        assert_eq!(Severity::Info.as_str(), "info");
        assert_eq!(Severity::Warning.as_str(), "warning");
        let s = settings();
        let all =
            Sample { cpu_c: Some(93.4), gpu_c: Some(91.6), battery_pct: 12.0, on_ac: false, throttled: true };
        let got: Vec<(AlertId, Severity, String)> = AlertId::ALL
            .iter()
            .map(|i| describe(*i, &all, &s))
            .map(|a| (a.id, a.severity, a.message))
            .collect();
        assert_eq!(
            got,
            [
                (
                    AlertId::SensorLost,
                    Severity::Warning,
                    "The CPU temperature cannot be read; the fans are left to the firmware".to_owned()
                ),
                (AlertId::CpuTemp, Severity::Warning, "CPU is hot: 93 C (alert limit 90 C)".to_owned()),
                (AlertId::GpuTemp, Severity::Warning, "GPU is hot: 92 C (alert limit 90 C)".to_owned()),
                (AlertId::BatteryLow, Severity::Warning, "Battery low: 12 %, plug in the charger".to_owned()),
                (
                    AlertId::Throttling,
                    Severity::Info,
                    "The CPU clock is held down to keep the laptop cool".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn fires_only_after_the_debounce_and_only_once_per_episode() {
        let mut ev = Evaluator::new(settings());
        let mut clock = FakeClock(1000.0);
        // Two hot samples are not enough, the third is; 60 more hot seconds say nothing new.
        let mut samples = temps(&[91.0, 92.0, 93.0]);
        samples.extend(temps(&[95.0; 60]));
        assert_eq!(run(&mut ev, &mut clock, &samples), [(AlertId::CpuTemp, 3)]);
        assert!(ev.is_active(AlertId::CpuTemp));
    }

    #[test]
    fn one_noisy_sample_is_not_an_alert() {
        let mut ev = Evaluator::new(settings());
        let mut clock = FakeClock(0.0);
        let samples = temps(&[91.0, 91.0, 80.0, 91.0, 91.0, 80.0, 91.0, 91.0, 80.0]);
        assert_eq!(run(&mut ev, &mut clock, &samples), []);
        assert!(!ev.is_active(AlertId::CpuTemp));
    }

    #[test]
    fn the_limit_itself_counts_and_one_below_does_not() {
        let mut ev = Evaluator::new(settings());
        let mut clock = FakeClock(0.0);
        assert_eq!(run(&mut ev, &mut clock, &temps(&[89.9; 10])), []);
        assert_eq!(run(&mut ev, &mut clock, &temps(&[90.0; 3])), [(AlertId::CpuTemp, 3)]);
    }

    #[test]
    fn hysteresis_keeps_one_episode_until_five_degrees_below() {
        let mut ev = Evaluator::new(settings());
        ev.set_settings(Settings { repeat_s: 1.0, ..settings() }); // only the hysteresis may hold it back
        let mut clock = FakeClock(0.0);
        let mut samples = temps(&[91.0; 3]);
        // Flapping 86..90 stays inside the band (limit - 5 = 85 is the way out): no new alert.
        for _ in 0..20 {
            samples.extend(temps(&[86.0, 91.0, 87.0, 90.0]));
        }
        assert_eq!(run(&mut ev, &mut clock, &samples), [(AlertId::CpuTemp, 3)]);
        assert!(ev.is_active(AlertId::CpuTemp));
        // Exactly 85 ends the episode; the next climb is a new one (the repeat limit is 1 s here).
        assert_eq!(run(&mut ev, &mut clock, &temps(&[85.0])), []);
        assert!(!ev.is_active(AlertId::CpuTemp));
        assert_eq!(run(&mut ev, &mut clock, &temps(&[91.0; 3])), [(AlertId::CpuTemp, 3)]);
    }

    #[test]
    fn a_new_episode_inside_the_repeat_interval_waits_and_is_shown_when_it_is_up() {
        let mut ev = Evaluator::new(settings()); // repeat 300 s
        let mut clock = FakeClock(0.0);
        assert_eq!(run(&mut ev, &mut clock, &temps(&[91.0; 3])), [(AlertId::CpuTemp, 3)]);
        // Cool down fully, heat up again 10 s after the first message: a new episode, held back.
        let mut again = temps(&[70.0]);
        again.extend(temps(&[92.0; 100]));
        assert_eq!(run(&mut ev, &mut clock, &again), []);
        assert!(ev.is_active(AlertId::CpuTemp), "the episode goes on, only its message waits");
        // 300 s after the first message (first sent at t=3) it comes, while still hot.
        let rest = temps(&[92.0; 250]);
        let got = run(&mut ev, &mut clock, &rest);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(clock.0 as u32, 3 + 1 + 100 + 250);
        // t = 3 + 300 = 303 is the first second it may be shown: sample number 303 - 104 = 199.
        assert_eq!(got[0], (AlertId::CpuTemp, 199));
    }

    #[test]
    fn a_held_back_episode_that_ends_first_is_never_shown() {
        let mut ev = Evaluator::new(settings());
        let mut clock = FakeClock(0.0);
        assert_eq!(run(&mut ev, &mut clock, &temps(&[91.0; 3])), [(AlertId::CpuTemp, 3)]);
        let mut samples = temps(&[70.0]);
        samples.extend(temps(&[92.0; 20]));
        samples.extend(temps(&[70.0; 5]));
        assert_eq!(run(&mut ev, &mut clock, &samples), []);
        assert!(!ev.is_active(AlertId::CpuTemp));
    }

    #[test]
    fn events_carry_began_notify_and_cleared_in_order() {
        let mut ev = Evaluator::new(settings());
        let mut got = Vec::new();
        for (i, t) in [91.0, 91.0, 91.0, 91.0, 80.0].iter().enumerate() {
            got.extend(ev.step(i as f64, &cpu(*t)));
        }
        let kinds: Vec<String> = got
            .iter()
            .map(|e| match e {
                Event::Began(a) => format!("began {}", a.id.as_str()),
                Event::Notify(a) => format!("notify {}", a.id.as_str()),
                Event::Cleared(id) => format!("cleared {}", id.as_str()),
            })
            .collect();
        assert_eq!(kinds, ["began cpu-temp-high", "notify cpu-temp-high", "cleared cpu-temp-high"]);
    }

    #[test]
    fn debounce_of_one_fires_on_the_first_sample() {
        let mut ev = Evaluator::new(Settings { debounce: 1, ..settings() });
        let mut clock = FakeClock(0.0);
        assert_eq!(run(&mut ev, &mut clock, &temps(&[95.0, 95.0])), [(AlertId::CpuTemp, 1)]);
    }

    #[test]
    fn cpu_and_gpu_are_separate_but_share_the_minute() {
        let mut ev = Evaluator::new(settings());
        let mut clock = FakeClock(0.0);
        let both = Sample { cpu_c: Some(95.0), gpu_c: Some(95.0), ..cool() };
        let got = run(&mut ev, &mut clock, &[both; 70]);
        // Both are due at second 3; the CPU takes the slot, the GPU follows one minute later.
        assert_eq!(got, [(AlertId::CpuTemp, 3), (AlertId::GpuTemp, 63)]);
    }

    #[test]
    fn global_gap_is_one_minute_and_applies_to_different_alerts() {
        let mut ev = Evaluator::new(settings());
        let mut clock = FakeClock(0.0);
        let mut samples = vec![Sample { battery_pct: 10.0, on_ac: false, ..cool() }; 3];
        samples.extend(vec![Sample { throttled: true, ..cool() }; 3]);
        // Battery notified at 3; throttling is due at 6 but waits for 63.
        let got =
            run(&mut ev, &mut clock, &[samples, vec![Sample { throttled: true, ..cool() }; 70]].concat());
        assert_eq!(got, [(AlertId::BatteryLow, 3), (AlertId::Throttling, 63)]);
    }

    #[test]
    fn battery_low_needs_the_battery_and_clears_on_the_charger_or_five_points() {
        let mut ev = Evaluator::new(settings());
        let mut clock = FakeClock(0.0);
        let low = |p: f64| Sample { battery_pct: p, on_ac: false, ..cool() };
        // On the charger nothing, whatever the level.
        let charging = Sample { battery_pct: 3.0, on_ac: true, ..cool() };
        assert_eq!(run(&mut ev, &mut clock, &[charging; 10]), []);
        // 15 % exactly counts; 16 does not.
        assert_eq!(run(&mut ev, &mut clock, &[low(16.0); 5]), []);
        assert_eq!(run(&mut ev, &mut clock, &[low(15.0); 3]), [(AlertId::BatteryLow, 3)]);
        // 19 % is inside the band: still the same episode; 20 % ends it.
        run(&mut ev, &mut clock, &[low(19.0)]);
        assert!(ev.is_active(AlertId::BatteryLow));
        run(&mut ev, &mut clock, &[low(20.0)]);
        assert!(!ev.is_active(AlertId::BatteryLow));
        // The charger also ends an episode.
        run(&mut ev, &mut clock, &[low(10.0); 3]);
        assert!(ev.is_active(AlertId::BatteryLow));
        run(&mut ev, &mut clock, &[charging]);
        assert!(!ev.is_active(AlertId::BatteryLow));
    }

    #[test]
    fn battery_alert_off_at_zero() {
        let mut ev = Evaluator::new(Settings { battery_low_pct: 0.0, ..settings() });
        let mut clock = FakeClock(0.0);
        let dead = Sample { battery_pct: 0.0, on_ac: false, ..cool() };
        assert_eq!(run(&mut ev, &mut clock, &[dead; 10]), []);
    }

    #[test]
    fn throttling_is_debounced_both_ways_and_is_info() {
        let mut ev = Evaluator::new(settings());
        let mut clock = FakeClock(0.0);
        let held = Sample { throttled: true, ..cool() };
        // The governor steps the cap down and up while it works: gaps shorter than the debounce
        // do not end the episode.
        let mut samples = vec![held; 3];
        samples.extend([cool(), cool(), held, held, cool(), held]);
        assert_eq!(run(&mut ev, &mut clock, &samples), [(AlertId::Throttling, 3)]);
        assert!(ev.is_active(AlertId::Throttling));
        run(&mut ev, &mut clock, &[cool(); 3]);
        assert!(!ev.is_active(AlertId::Throttling));
        let a = describe(AlertId::Throttling, &held, &settings());
        assert_eq!(a.severity, Severity::Info);
    }

    #[test]
    fn a_lost_sensor_is_reported_after_the_debounce_and_clears_when_it_returns() {
        let mut ev = Evaluator::new(settings());
        let mut clock = FakeClock(0.0);
        let lost = Sample { cpu_c: None, ..cool() };
        assert_eq!(run(&mut ev, &mut clock, &[lost; 2]), []);
        assert_eq!(run(&mut ev, &mut clock, &[lost; 5]), [(AlertId::SensorLost, 1)]);
        assert!(ev.is_active(AlertId::SensorLost));
        run(&mut ev, &mut clock, &[cool(), cool()]);
        assert!(ev.is_active(AlertId::SensorLost), "needs the debounce to come back too");
        run(&mut ev, &mut clock, &[cool()]);
        assert!(!ev.is_active(AlertId::SensorLost));
    }

    #[test]
    fn a_nan_temperature_is_a_missing_one() {
        let mut ev = Evaluator::new(settings());
        let mut clock = FakeClock(0.0);
        let nan = Sample { cpu_c: Some(f64::NAN), ..cool() };
        assert_eq!(run(&mut ev, &mut clock, &[nan; 3]), [(AlertId::SensorLost, 3)]);
        let inf = Sample { cpu_c: Some(f64::INFINITY), gpu_c: Some(f64::INFINITY), ..cool() };
        let mut ev = Evaluator::new(settings());
        assert_eq!(run(&mut ev, &mut clock, &[inf; 5]), [(AlertId::SensorLost, 3)], "never a hot alert");
    }

    #[test]
    fn a_missing_gpu_never_alerts_and_ends_a_gpu_episode() {
        let mut ev = Evaluator::new(settings());
        let mut clock = FakeClock(0.0);
        let hot = Sample { gpu_c: Some(95.0), ..cool() };
        assert_eq!(run(&mut ev, &mut clock, &[hot; 3]), [(AlertId::GpuTemp, 3)]);
        let asleep = Sample { gpu_c: None, ..cool() };
        run(&mut ev, &mut clock, &[asleep]);
        assert!(!ev.is_active(AlertId::GpuTemp));
        assert_eq!(run(&mut ev, &mut clock, &[asleep; 20]), []);
    }

    #[test]
    fn disabled_rules_are_silent_and_enabling_starts_fresh() {
        let mut ev = Evaluator::new(Settings { enabled: false, ..settings() });
        let mut clock = FakeClock(0.0);
        assert_eq!(run(&mut ev, &mut clock, &temps(&[99.0; 10])), []);
        ev.set_settings(settings());
        assert_eq!(run(&mut ev, &mut clock, &temps(&[99.0; 3])), [(AlertId::CpuTemp, 3)]);
        ev.set_settings(Settings { enabled: false, ..settings() });
        assert!(!ev.is_active(AlertId::CpuTemp), "switching off ends the episode");
    }

    #[test]
    fn switched_off_features_are_silent() {
        let mut ev = Evaluator::new(Settings { throttling: false, sensor_lost: false, ..settings() });
        let mut clock = FakeClock(0.0);
        let s = Sample { cpu_c: None, throttled: true, ..cool() };
        assert_eq!(run(&mut ev, &mut clock, &[s; 20]), []);
    }

    #[test]
    fn a_raised_limit_applies_to_the_next_sample() {
        let mut ev = Evaluator::new(settings());
        let mut clock = FakeClock(0.0);
        assert_eq!(run(&mut ev, &mut clock, &temps(&[91.0; 3])), [(AlertId::CpuTemp, 3)]);
        ev.set_settings(Settings { cpu_limit_c: 100.0, ..settings() });
        // 91 is now above the new re-arm point (95)? No: 91 <= 95, so the episode ends at once.
        run(&mut ev, &mut clock, &temps(&[91.0]));
        assert!(!ev.is_active(AlertId::CpuTemp));
    }

    #[test]
    fn a_clock_that_goes_backwards_does_not_lock_an_alert_out() {
        let mut ev = Evaluator::new(Settings { repeat_s: 60.0, ..settings() });
        for i in 0..3 {
            ev.step(1000.0 + f64::from(i), &cpu(95.0));
        }
        ev.step(1003.0, &cpu(50.0));
        let mut got = Vec::new();
        for i in 0..3 {
            got.extend(ev.step(10.0 + f64::from(i), &cpu(95.0)));
        }
        assert!(got.iter().any(|e| matches!(e, Event::Notify(_))), "{got:?}");
    }

    #[test]
    fn settings_from_config_clamp_hostile_numbers() {
        let hostile = AlertsConfig {
            enabled: true,
            cpu_limit_c: i64::MIN,
            gpu_limit_c: i64::MAX,
            repeat_minutes: i64::MIN,
            debounce_samples: 0,
            battery_low_pct: i64::MAX,
            throttling: true,
            sensor_lost: true,
        };
        let s = Settings::from(&hostile);
        assert_eq!(
            (s.cpu_limit_c, s.gpu_limit_c, s.repeat_s, s.debounce, s.battery_low_pct),
            (30.0, 150.0, 60.0, 1, 100.0)
        );
        let d = Settings::from(&AlertsConfig::default());
        assert_eq!((d.cpu_limit_c, d.repeat_s, d.debounce, d.battery_low_pct), (90.0, 300.0, 3, 15.0));
        assert_eq!(Settings::default(), d);
    }

    #[test]
    fn a_long_flapping_run_around_the_limit_stays_quiet() {
        // 90 C limit, 3 samples debounce: a sensor that bounces between 89 and 91 every second
        // never holds three in a row, so nothing is ever shown.
        let mut ev = Evaluator::new(settings());
        let mut clock = FakeClock(0.0);
        let samples: Vec<Sample> = (0..3600).map(|i| cpu(if i % 2 == 0 { 89.0 } else { 91.0 })).collect();
        assert_eq!(run(&mut ev, &mut clock, &samples), []);
        // A slower flap (4 up, 4 down) does hold three, and is limited by the repeat interval:
        // 3600 s is at most 12 messages at 300 s, and the 85 C re-arm is never reached so it is
        // one episode and one message.
        let samples: Vec<Sample> =
            (0..3600).map(|i| cpu(if (i / 4) % 2 == 0 { 86.0 } else { 91.0 })).collect();
        let mut ev = Evaluator::new(settings());
        assert_eq!(run(&mut ev, &mut clock, &samples).len(), 1);
    }
}
