//! The readings the screen shows, and nothing else: plain values with no handle to the hardware.
//!
//! Two kinds of reading, as in the Python edition. The *fast* reading (every second, in the
//! background) feeds the dashboard and the header; the *slow* readings are the settings of the page
//! that is open, read when the page opens, after every write and on `r`. The screen never waits
//! for either: it draws what it has, and a value that was never read is shown as an ellipsis, never
//! as zero (a fan duty of 0 means a stopped fan, so "unknown" must not look like it).

use std::collections::{HashMap, VecDeque};

/// How many readings the sparklines remember (one per second).
pub(crate) const HISTORY: usize = 160;

/// The power modes in the order the screen lists them (the firmware's own numbers skip 2 and 3,
/// and "eco" is the lowest of the five).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Mode {
    Eco,
    Quiet,
    Balanced,
    Performance,
    Turbo,
}

impl Mode {
    pub(crate) const ALL: [Mode; 5] =
        [Mode::Eco, Mode::Quiet, Mode::Balanced, Mode::Performance, Mode::Turbo];

    /// The word the firmware, the service and the command line use.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Mode::Eco => "eco",
            Mode::Quiet => "quiet",
            Mode::Balanced => "balanced",
            Mode::Performance => "performance",
            Mode::Turbo => "turbo",
        }
    }

    /// The word on screen.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Mode::Eco => "Eco",
            Mode::Quiet => "Quiet",
            Mode::Balanced => "Balanced",
            Mode::Performance => "Performance",
            Mode::Turbo => "Turbo",
        }
    }

    pub(crate) fn from_name(name: &str) -> Option<Mode> {
        Mode::ALL.into_iter().find(|m| m.name() == name)
    }
}

/// Temperatures (degrees C) and fan speeds (rpm) of one reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Sensors {
    pub cpu_temp: i64,
    pub sys_temp: i64,
    pub gpu_temp: i64,
    pub cpu_fan_rpm: i64,
    pub gpu_fan_rpm: i64,
}

/// What the firmware says the fans do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FanMode {
    Auto,
    Max,
    Manual,
    Unknown,
}

impl FanMode {
    pub(crate) fn name(self) -> &'static str {
        match self {
            FanMode::Auto => "auto",
            FanMode::Max => "max",
            FanMode::Manual => "manual",
            FanMode::Unknown => "unknown",
        }
    }

    pub(crate) fn from_name(name: &str) -> FanMode {
        match name {
            "auto" => FanMode::Auto,
            "max" => FanMode::Max,
            "manual" => FanMode::Manual,
            _ => FanMode::Unknown,
        }
    }
}

/// The behaviour and the duty register of each fan in percent (0 means that fan is stopped).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FanState {
    pub mode: FanMode,
    pub cpu_duty: i64,
    pub gpu_duty: i64,
}

/// The charge as the power supply reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Battery {
    pub percent: i64,
}

/// What the TrueSense service knows. `None` in [`Fast`] when it does not run: the screen then reads
/// the hardware itself.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ServiceState {
    /// The service's own reading of this second, when it has one.
    pub sensors: Option<Sensors>,
    /// The power mode, empty when unknown.
    pub power_mode: String,
    pub ac: Option<bool>,
    /// `native`, `smart` or `off`; empty when unknown.
    pub auto_mode: String,
    pub smart_profile: String,
    /// The service keeps both fans stopped on request.
    pub fan_off: bool,
    /// The service's own fan curve is in control.
    pub smart_fan: bool,
    pub fan_curve: String,
    pub cpu_load: Option<i64>,
    pub gpu_load: Option<i64>,
    pub gpu_watts: Option<i64>,
    /// What Smart auto thinks the laptop is doing, and why it last changed the power mode.
    pub activity: String,
    pub mode_reason: String,
    /// Seconds a hand-picked power mode is still held against Smart auto.
    pub hold_left_s: i64,
}

/// One fast reading, as the backend produced it. Each hardware reading carries its own error text:
/// a failing read keeps the previous value and sets the "No device" line, it never blanks the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fast {
    pub service: Option<ServiceState>,
    pub sensors: Result<Sensors, String>,
    pub fan: Result<FanState, String>,
    pub mode: Result<String, String>,
    pub ac: Result<bool, String>,
    pub battery: Option<Battery>,
}

/// The sparklines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hist {
    TempCpu = 0,
    TempGpu,
    LoadCpu,
    LoadGpu,
    Rpm1,
    Rpm2,
}

/// A keyboard backlight setting. An empty colour or a speed of 0 is "unknown".
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct KbLook {
    pub name: String,
    pub color: String,
    pub speed: i64,
    pub brightness: i64,
    pub direction: i64,
}

/// A light bar setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RearLook {
    pub mode: String,
    pub color: String,
    pub speed: i64,
    pub brightness: i64,
    pub direction: i64,
}

/// The verdict on the sound card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SoundCard {
    Missing,
    Ok,
    Broken(String),
}

/// The state of the microphones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Microphone {
    pub muted: bool,
    pub volume: Option<i64>,
}

/// The noise-reduction mode of each input (`off`, `conference`, `personal`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct VoiceModes {
    pub headset: Option<String>,
    pub array: Option<String>,
}

/// An output the DTS sound processor knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DtsOutput {
    pub label: String,
    pub profile: String,
    pub current: bool,
}

/// The grade of one health check (INFO counts as OK: the screen has no fourth colour).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Grade {
    Ok,
    Warn,
    Fail,
}

impl Grade {
    pub(crate) fn word(self) -> &'static str {
        match self {
            Grade::Ok => "OK",
            Grade::Warn => "WARN",
            Grade::Fail => "FAIL",
        }
    }
}

/// The outcome of one health check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DoctorRow {
    pub check: String,
    pub grade: Grade,
    pub detail: String,
    pub fix: String,
}

/// The names of a list of profiles and the one in use.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Names {
    pub names: Vec<String>,
    pub current: Option<String>,
}

/// The settings of the open page, read in the background; each has its own key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ReadKey {
    Curves,
    SmartProfiles,
    Limit,
    Calibration,
    UsbCharge,
    GpuMode,
    Overdrive,
    BootSound,
    KbTimeout,
    ChargeFull,
    KbLast,
    KbNow,
    Rear,
    SavedLightings,
    Sound,
    Output,
    SpeakerFilter,
    Mic,
    Voice,
    DtsOutputs,
    DtsOn,
    ModuleVersion,
    /// Not read with the page: set when the health check has run.
    Doctor,
}

/// A slow reading's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SlowValue {
    Bool(bool),
    /// A switch the firmware may be unable to tell.
    MaybeBool(Option<bool>),
    Int(i64),
    Text(String),
    Names(Names),
    List(Vec<String>),
    KbLast(KbLook),
    Rear(RearLook),
    Sound(SoundCard),
    Mic(Microphone),
    Voice(VoiceModes),
    Dts(Vec<DtsOutput>),
    Doctor(Vec<DoctorRow>),
}

/// One slow reading as the backend produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SlowReading {
    pub key: ReadKey,
    pub value: Result<SlowValue, String>,
}

/// The latest readings.
#[derive(Debug, Clone, Default)]
pub(crate) struct Data {
    service: Option<ServiceState>,
    sensors: Option<Sensors>,
    fan: Option<FanState>,
    mode: String,
    ac: Option<bool>,
    battery: Option<Battery>,
    err: String,
    hist: [VecDeque<i64>; 6],
    slow: HashMap<ReadKey, SlowValue>,
    errors: HashMap<ReadKey, String>,
}

impl Data {
    /// Takes in a fast reading. A failing read keeps the previous value; the history only grows
    /// from a complete reading, so a gap does not draw as a drop to zero.
    pub(crate) fn apply_fast(&mut self, f: Fast) {
        let mut first_error = String::new();
        let mut note = |e: &str| {
            if first_error.is_empty() {
                first_error = if e.is_empty() { "error".to_owned() } else { e.to_owned() };
            }
        };
        match &f.sensors {
            Ok(s) => self.sensors = Some(*s),
            Err(e) => note(e),
        }
        match &f.fan {
            Ok(s) => self.fan = Some(*s),
            Err(e) => note(e),
        }
        match &f.mode {
            Ok(m) => self.mode.clone_from(m),
            Err(e) => note(e),
        }
        match &f.ac {
            Ok(a) => self.ac = Some(*a),
            Err(e) => note(e),
        }
        self.battery = f.battery;
        self.err = first_error;
        self.service = f.service;
        if self.err.is_empty() {
            let s = self.sensors.unwrap_or_default();
            let (cpu, gpu) =
                self.service.as_ref().map_or((0, 0), |v| (v.cpu_load.unwrap_or(0), v.gpu_load.unwrap_or(0)));
            self.push(Hist::TempCpu, s.cpu_temp);
            self.push(Hist::TempGpu, s.gpu_temp);
            self.push(Hist::Rpm1, s.cpu_fan_rpm);
            self.push(Hist::Rpm2, s.gpu_fan_rpm);
            self.push(Hist::LoadCpu, cpu);
            self.push(Hist::LoadGpu, gpu);
        }
    }

    fn push(&mut self, which: Hist, v: i64) {
        let ring = &mut self.hist[which as usize];
        if ring.len() == HISTORY {
            ring.pop_front();
        }
        ring.push_back(v);
    }

    /// Takes in a slow reading: a failure keeps the old value and remembers why.
    pub(crate) fn apply_slow(&mut self, r: SlowReading) {
        match r.value {
            Ok(v) => {
                self.slow.insert(r.key, v);
                self.errors.remove(&r.key);
            }
            Err(e) => {
                self.errors.insert(r.key, if e.is_empty() { "error".to_owned() } else { e });
            }
        }
    }

    /// Stores a result that is not read with the page (the health check).
    pub(crate) fn set(&mut self, key: ReadKey, v: SlowValue) {
        self.slow.insert(key, v);
        self.errors.remove(&key);
    }

    pub(crate) fn history(&self, which: Hist) -> Vec<i64> {
        self.hist[which as usize].iter().copied().collect()
    }

    /// The service answered.
    pub(crate) fn daemon(&self) -> bool {
        self.service.is_some()
    }

    /// The service's state; the default (empty) one when none runs.
    pub(crate) fn service(&self) -> ServiceState {
        self.service.clone().unwrap_or_default()
    }

    pub(crate) fn fan(&self) -> Option<FanState> {
        self.fan
    }

    pub(crate) fn power_mode(&self) -> &str {
        &self.mode
    }

    pub(crate) fn sensors(&self) -> Option<Sensors> {
        self.sensors
    }

    pub(crate) fn ac(&self) -> Option<bool> {
        self.ac
    }

    pub(crate) fn battery(&self) -> Option<&Battery> {
        self.battery.as_ref()
    }

    /// The "No device" text, empty when the last fast reading was complete.
    pub(crate) fn error(&self) -> &str {
        &self.err
    }

    pub(crate) fn get(&self, key: ReadKey) -> Option<&SlowValue> {
        self.slow.get(&key)
    }

    /// The error text of the last slow read of `key`.
    #[cfg(test)]
    pub(crate) fn read_error(&self, key: ReadKey) -> Option<&str> {
        self.errors.get(&key).map(String::as_str)
    }

    pub(crate) fn bool(&self, key: ReadKey) -> Option<bool> {
        match self.get(key)? {
            SlowValue::Bool(b) => Some(*b),
            SlowValue::MaybeBool(b) => *b,
            _ => None,
        }
    }

    pub(crate) fn int(&self, key: ReadKey) -> Option<i64> {
        match self.get(key)? {
            SlowValue::Int(n) => Some(*n),
            _ => None,
        }
    }

    pub(crate) fn text(&self, key: ReadKey) -> Option<&str> {
        match self.get(key)? {
            SlowValue::Text(t) => Some(t),
            _ => None,
        }
    }

    pub(crate) fn names(&self, key: ReadKey) -> Option<&Names> {
        match self.get(key)? {
            SlowValue::Names(n) => Some(n),
            _ => None,
        }
    }

    pub(crate) fn list(&self, key: ReadKey) -> Option<&[String]> {
        match self.get(key)? {
            SlowValue::List(l) => Some(l),
            _ => None,
        }
    }

    pub(crate) fn doctor(&self) -> &[DoctorRow] {
        match self.get(ReadKey::Doctor) {
            Some(SlowValue::Doctor(rows)) => rows,
            _ => &[],
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn sensors(cpu: i64) -> Sensors {
        Sensors { cpu_temp: cpu, sys_temp: 40, gpu_temp: 50, cpu_fan_rpm: 1000, gpu_fan_rpm: 1100 }
    }

    fn ok(cpu: i64) -> Fast {
        Fast {
            service: None,
            sensors: Ok(sensors(cpu)),
            fan: Ok(FanState { mode: FanMode::Auto, cpu_duty: 30, gpu_duty: 30 }),
            mode: Ok("balanced".into()),
            ac: Ok(true),
            battery: Some(Battery { percent: 80 }),
        }
    }

    #[test]
    fn a_complete_reading_replaces_everything_and_feeds_the_history() {
        let mut d = Data::default();
        d.apply_fast(ok(60));
        assert_eq!(d.sensors().map(|s| s.cpu_temp), Some(60));
        assert_eq!(
            (d.power_mode(), d.ac(), d.battery().map(|b| b.percent)),
            ("balanced", Some(true), Some(80))
        );
        assert_eq!(d.error(), "");
        assert_eq!(d.history(Hist::TempCpu), vec![60]);
        assert!(!d.daemon());
    }

    #[test]
    fn a_failing_read_keeps_the_previous_values_and_sets_the_no_device_text() {
        let mut d = Data::default();
        d.apply_fast(ok(60));
        let mut bad = ok(70);
        bad.sensors = Err("no device".into());
        d.apply_fast(bad);
        assert_eq!(d.sensors().map(|s| s.cpu_temp), Some(60), "the old reading stays");
        assert_eq!(d.error(), "no device");
        assert_eq!(d.history(Hist::TempCpu), vec![60], "a gap does not draw as a drop");
        let mut worse = ok(70);
        worse.mode = Err(String::new());
        d.apply_fast(worse);
        assert_eq!(d.error(), "error", "an error with no text still says something");
        d.apply_fast(ok(71));
        assert_eq!(d.error(), "");
    }

    #[test]
    fn the_service_state_is_kept_or_dropped_with_the_reading() {
        let mut d = Data::default();
        let mut f = ok(60);
        f.service =
            Some(ServiceState { auto_mode: "smart".into(), cpu_load: Some(12), ..ServiceState::default() });
        d.apply_fast(f);
        assert!(d.daemon());
        assert_eq!(d.service().auto_mode, "smart");
        assert_eq!(d.history(Hist::LoadCpu), vec![12]);
        d.apply_fast(ok(61));
        assert!(!d.daemon(), "the service stopped answering");
        assert_eq!(d.service(), ServiceState::default());
    }

    #[test]
    fn the_history_is_a_bounded_ring() {
        let mut d = Data::default();
        for n in 0..(HISTORY as i64 + 10) {
            d.apply_fast(ok(n));
        }
        let h = d.history(Hist::TempCpu);
        assert_eq!(h.len(), HISTORY);
        assert_eq!(h.last(), Some(&(HISTORY as i64 + 9)));
        assert_eq!(h.first(), Some(&10));
    }

    #[test]
    fn slow_reads_keep_results_and_errors_per_key() {
        let mut d = Data::default();
        d.apply_slow(SlowReading { key: ReadKey::Limit, value: Ok(SlowValue::Bool(true)) });
        d.apply_slow(SlowReading { key: ReadKey::Calibration, value: Err("boom".into()) });
        assert_eq!(d.bool(ReadKey::Limit), Some(true));
        assert_eq!(d.bool(ReadKey::Calibration), None);
        assert_eq!(d.read_error(ReadKey::Calibration), Some("boom"));
        assert_eq!(d.read_error(ReadKey::Limit), None);
        d.apply_slow(SlowReading { key: ReadKey::Limit, value: Err("later failure".into()) });
        assert_eq!(d.bool(ReadKey::Limit), Some(true), "a failing read keeps the old value");
        assert_eq!(d.read_error(ReadKey::Limit), Some("later failure"));
        d.apply_slow(SlowReading { key: ReadKey::Limit, value: Ok(SlowValue::Bool(false)) });
        assert_eq!((d.bool(ReadKey::Limit), d.read_error(ReadKey::Limit)), (Some(false), None));
        d.apply_slow(SlowReading { key: ReadKey::Output, value: Err(String::new()) });
        assert_eq!(d.read_error(ReadKey::Output), Some("error"));
    }

    #[test]
    fn typed_getters_answer_none_for_a_value_of_another_type() {
        let mut d = Data::default();
        d.apply_slow(SlowReading { key: ReadKey::Limit, value: Ok(SlowValue::Text("x".into())) });
        assert_eq!((d.bool(ReadKey::Limit), d.int(ReadKey::Limit)), (None, None));
        assert_eq!(d.text(ReadKey::Limit), Some("x"));
        assert!(d.names(ReadKey::Limit).is_none() && d.list(ReadKey::Limit).is_none());
        assert!(d.doctor().is_empty());
        d.apply_slow(SlowReading { key: ReadKey::Overdrive, value: Ok(SlowValue::MaybeBool(None)) });
        assert_eq!(d.bool(ReadKey::Overdrive), None, "a switch the firmware cannot tell is unknown");
    }

    #[test]
    fn modes_are_listed_lowest_first_and_found_by_name() {
        let names: Vec<&str> = Mode::ALL.iter().map(|m| m.name()).collect();
        assert_eq!(names, ["eco", "quiet", "balanced", "performance", "turbo"]);
        assert_eq!(Mode::from_name("turbo"), Some(Mode::Turbo));
        assert_eq!(Mode::from_name("?"), None);
        assert_eq!(Mode::Performance.label(), "Performance");
        assert_eq!(FanMode::from_name("manual"), FanMode::Manual);
        assert_eq!(FanMode::from_name("0x00"), FanMode::Unknown);
    }
}
