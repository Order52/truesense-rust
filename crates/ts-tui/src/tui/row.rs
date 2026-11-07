//! One setting on a page: what it looks like, where its change is on the way to the hardware, and
//! which value to show meanwhile.
//!
//! A row is data, not behaviour: its [`Id`] says which setting it is and `pages` knows how to read,
//! guard and write that setting. That keeps the state of the screen a plain value the tests can
//! build, compare and print.

use std::time::{Duration, Instant};

/// How long a freshly chosen value stays on screen after its write finished, so that a slow
/// hardware reading does not flash the old value back.
pub(crate) const VALUE_GRACE: Duration = Duration::from_millis(1500);

/// How long a chosen value is shown while it waits to be written (a change nobody confirms must
/// not stay on screen forever).
pub(crate) const LOCAL_HOLD: Duration = Duration::from_secs(15);

/// A setting's value. Fan duties and brightness are numbers, switches are booleans, everything
/// with a name (a mode, a colour, a profile) is text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Value {
    Bool(bool),
    Int(i64),
    Text(String),
}

impl Value {
    pub(crate) fn text(s: impl Into<String>) -> Self {
        Value::Text(s.into())
    }

    pub(crate) fn as_bool(&self) -> bool {
        matches!(self, Value::Bool(true))
    }

    pub(crate) fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(n) => Some(*n),
            _ => None,
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        match self {
            Value::Text(t) => t,
            _ => "",
        }
    }

    /// The value as the screen prints it when the row has no formatter of its own.
    pub(crate) fn plain(&self) -> String {
        match self {
            Value::Bool(b) => (if *b { "on" } else { "off" }).to_owned(),
            Value::Int(n) => n.to_string(),
            Value::Text(t) => t.clone(),
        }
    }
}

/// Every setting the screen can show, which is what the behaviour tables of `pages` key on.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Id {
    Header,
    // Dashboard and power
    PowerMode,
    AutoMode,
    DashFans,
    SmartProfile,
    Hold,
    ReleaseHold,
    BatteryLimit,
    ChargeFull,
    Calibration,
    UsbCharge,
    GpuMode,
    Overdrive,
    BootSound,
    KbTimeout,
    // Fans
    FanMode,
    FanCpu,
    FanGpu,
    CpuRpm,
    GpuRpm,
    Curve,
    // Lighting
    KbEffect,
    KbColor,
    KbCustom,
    KbBrightness,
    KbSpeed,
    KbDirection,
    KbSaved,
    RearEffect,
    RearColor,
    RearCustom,
    RearBrightness,
    RearSpeed,
    // Audio
    SoundCard,
    FixSound,
    Output,
    SpeakerFilter,
    Dts,
    DtsMode,
    DtsEverywhere,
    DtsOutputs,
    MicMute,
    MicVolume,
    VoiceHeadset,
    VoiceArray,
    // Profiles
    Profile(String),
    NoProfiles,
    Look(String),
    // System
    Version,
    ModuleVersion,
    Service,
    WindowInfo,
    OpenWindow,
    RunDoctor,
    DoctorHint,
    Doctor(String),
}

/// What a row looks like and how it reacts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Header,
    Info,
    Choice,
    Toggle,
    Slider,
    Button,
}

/// Where a change of a row is on its way to the hardware: nothing, chosen and waiting for its
/// pause (or Enter), waiting for a Yes, or being written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowState {
    Idle,
    Pending,
    Asking,
    Writing,
}

/// One option of a choice row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Choice {
    pub value: Value,
    pub label: String,
}

impl Choice {
    pub(crate) fn new(value: Value, label: impl Into<String>) -> Self {
        Self { value, label: label.into() }
    }

    /// An option whose value is its name.
    pub(crate) fn named(name: &str) -> Self {
        Self::new(Value::text(name), name)
    }

    /// An option whose value is its name and whose label is the capitalised name.
    pub(crate) fn capitalised(name: &str) -> Self {
        Self::new(Value::text(name), capitalise(name))
    }
}

/// `eco` becomes `Eco`.
pub(crate) fn capitalise(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map_or_else(String::new, |c| c.to_uppercase().chain(chars.flat_map(char::to_lowercase)).collect())
}

/// One setting. `start` is where an unknown slider begins (else the middle, never the lowest value,
/// which may be a stop). `exact0` makes a slider show its lowest value (a stopped fan) only when
/// the reading is exactly that: a fan at 10 % is not "off".
#[derive(Debug, Clone)]
pub(crate) struct Row {
    pub id: Id,
    pub kind: Kind,
    pub label: String,
    pub hint: String,
    /// The options of a choice (the initial ones when the list is read at run time).
    pub options: Vec<Choice>,
    pub values: Vec<i64>,
    pub start: Option<i64>,
    pub exact0: bool,
    pub wrap: bool,
    /// A button's caption.
    pub text: String,
    /// The options are power modes and are drawn in their own colours.
    pub mode_colors: bool,
    pub state: RowState,
    local: Option<Value>,
    local_until: Option<Instant>,
    pub due: Option<Instant>,
}

impl Row {
    fn base(id: Id, kind: Kind, label: &str) -> Self {
        Self {
            id,
            kind,
            label: label.to_owned(),
            hint: String::new(),
            options: Vec::new(),
            values: Vec::new(),
            start: None,
            exact0: false,
            wrap: false,
            text: String::new(),
            mode_colors: false,
            state: RowState::Idle,
            local: None,
            local_until: None,
            due: None,
        }
    }

    pub(crate) fn header(text: &str) -> Self {
        Self::base(Id::Header, Kind::Header, text)
    }

    pub(crate) fn info(id: Id, label: &str) -> Self {
        Self::base(id, Kind::Info, label)
    }

    pub(crate) fn choice(id: Id, label: &str, options: Vec<Choice>) -> Self {
        Self { options, ..Self::base(id, Kind::Choice, label) }
    }

    pub(crate) fn toggle(id: Id, label: &str) -> Self {
        Self::base(id, Kind::Toggle, label)
    }

    pub(crate) fn slider(id: Id, label: &str, values: Vec<i64>) -> Self {
        Self { values, ..Self::base(id, Kind::Slider, label) }
    }

    pub(crate) fn button(id: Id, label: &str, text: &str) -> Self {
        Self { text: text.to_owned(), ..Self::base(id, Kind::Button, label) }
    }

    pub(crate) fn hint(mut self, hint: &str) -> Self {
        self.hint = hint.to_owned();
        self
    }

    pub(crate) fn wrapped(mut self) -> Self {
        self.wrap = true;
        self
    }

    pub(crate) fn exact0(mut self) -> Self {
        self.exact0 = true;
        self
    }

    pub(crate) fn start(mut self, at: i64) -> Self {
        self.start = Some(at);
        self
    }

    pub(crate) fn mode_colors(mut self) -> Self {
        self.mode_colors = true;
        self
    }

    /// True while a change of this row is on its way.
    pub(crate) fn busy(&self) -> bool {
        self.state != RowState::Idle
    }

    /// The value to show: the one just chosen until the hardware has caught up (or the write
    /// failed), else the latest reading.
    pub(crate) fn shown(&self, now: Instant, reading: Option<Value>) -> Option<Value> {
        match (&self.local, self.local_until) {
            (Some(v), Some(until)) if now < until => Some(v.clone()),
            _ => reading,
        }
    }

    /// Whether a value was chosen here and not yet confirmed or expired.
    #[cfg(test)]
    pub(crate) fn has_local(&self) -> bool {
        self.local.is_some()
    }

    /// The value chosen and waiting to be written.
    pub(crate) fn local(&self) -> Option<&Value> {
        self.local.as_ref()
    }

    /// Shows `v` now and marks the change as waiting.
    pub(crate) fn set_local(&mut self, v: Value, now: Instant) {
        self.local = Some(v);
        self.local_until = Some(now + LOCAL_HOLD);
        self.state = RowState::Pending;
    }

    /// A write finished: on success the chosen value stays on screen for a moment so a slow
    /// reading does not flash the old one; on failure the reading is shown again.
    pub(crate) fn settle(&mut self, ok: bool, now: Instant) {
        if ok && self.local.is_some() {
            self.local_until = Some(now + VALUE_GRACE);
        } else {
            self.local = None;
            self.local_until = None;
        }
        if self.state == RowState::Writing {
            self.state = RowState::Idle;
        }
    }

    /// Drops a change that was not written.
    pub(crate) fn forget(&mut self) {
        self.local = None;
        self.local_until = None;
        self.state = RowState::Idle;
        self.due = None;
    }

    /// A chosen value that was never confirmed expires. A change that waits for its pause is idle
    /// again; one that is being asked about or written keeps its state (the answer or the result
    /// is still to come).
    pub(crate) fn expire(&mut self, now: Instant) {
        if self.local.is_some() && self.local_until.is_some_and(|u| now >= u) {
            self.local = None;
            self.local_until = None;
            if self.state == RowState::Pending {
                self.state = RowState::Idle;
            }
        }
    }
}

/// The position of `value` among the options of a choice; `None` when unknown.
pub(crate) fn choice_index(options: &[Choice], value: Option<&Value>) -> Option<usize> {
    let v = value?;
    options.iter().position(|o| &o.value == v)
}

/// The position of the slider value nearest to `n`. An `exact0` slider shows its first value
/// only for an exact reading of it.
pub(crate) fn slider_index(values: &[i64], exact0: bool, value: Option<&Value>) -> Option<usize> {
    let n = value?.as_int()?;
    let first = *values.first()?;
    let skip_first = exact0 && n != first;
    values
        .iter()
        .enumerate()
        .filter(|(_, x)| !(skip_first && **x == first))
        .min_by_key(|(i, x)| ((**x - n).abs(), *i))
        .map(|(i, _)| i)
}

/// Where an unknown slider begins: `start`, else the middle (never the lowest value).
pub(crate) fn start_index(values: &[i64], start: Option<i64>) -> usize {
    if values.is_empty() {
        return 0;
    }
    match start {
        Some(s) => values.iter().enumerate().min_by_key(|(i, x)| ((**x - s).abs(), *i)).map_or(0, |(i, _)| i),
        None => values.len() / 2,
    }
}

/// `1..=n` stepped, both ends included.
pub(crate) fn int_range(lo: i64, hi: i64, step: i64) -> Vec<i64> {
    let step = step.max(1);
    (0..).map(|i| lo + i * step).take_while(|v| *v <= hi).collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn a_row_shows_what_was_chosen_until_the_hardware_caught_up() {
        let now = t0();
        let mut r = Row::toggle(Id::BootSound, "Boot sound");
        assert_eq!(r.shown(now, Some(Value::Bool(false))), Some(Value::Bool(false)));
        r.set_local(Value::Bool(true), now);
        assert_eq!(r.state, RowState::Pending);
        assert_eq!(r.shown(now, Some(Value::Bool(false))), Some(Value::Bool(true)));
        r.state = RowState::Writing;
        r.settle(true, now);
        assert_eq!(r.state, RowState::Idle);
        let within = now + VALUE_GRACE - Duration::from_millis(1);
        assert_eq!(r.shown(within, Some(Value::Bool(false))), Some(Value::Bool(true)), "grace period");
        let after = now + VALUE_GRACE;
        assert_eq!(r.shown(after, Some(Value::Bool(false))), Some(Value::Bool(false)), "then the reading");
    }

    #[test]
    fn a_failed_write_shows_the_reading_again_at_once() {
        let now = t0();
        let mut r = Row::toggle(Id::BootSound, "Boot sound");
        r.set_local(Value::Bool(true), now);
        r.state = RowState::Writing;
        r.settle(false, now);
        assert_eq!(r.shown(now, Some(Value::Bool(false))), Some(Value::Bool(false)));
        assert_eq!(r.state, RowState::Idle);
    }

    #[test]
    fn a_chosen_value_that_is_never_written_expires() {
        let now = t0();
        let mut r = Row::toggle(Id::BootSound, "Boot sound");
        r.set_local(Value::Bool(true), now);
        r.expire(now + LOCAL_HOLD - Duration::from_millis(1));
        assert_eq!(r.state, RowState::Pending);
        r.expire(now + LOCAL_HOLD);
        assert_eq!((r.state, r.has_local()), (RowState::Idle, false));
    }

    #[test]
    fn forget_drops_an_unsent_change() {
        let now = t0();
        let mut r = Row::toggle(Id::BootSound, "Boot sound");
        r.set_local(Value::Bool(true), now);
        r.due = Some(now);
        r.forget();
        assert_eq!((r.state, r.has_local(), r.due), (RowState::Idle, false, None));
    }

    #[test]
    fn slider_index_picks_the_nearest_and_exact0_keeps_off_for_a_real_zero() {
        let duties: Vec<i64> = std::iter::once(0).chain(int_range(20, 100, 5)).collect();
        let idx = |exact0, n| slider_index(&duties, exact0, Some(&Value::Int(n)));
        assert_eq!(idx(true, 0), Some(0));
        assert_eq!(idx(true, 10), Some(1), "10 % is 20 %, not off");
        assert_eq!(idx(true, 3), Some(1));
        assert_eq!(idx(true, 100), Some(17));
        assert_eq!(idx(false, 10), Some(0), "a tie goes to the lower position");
        assert_eq!(idx(false, 9), Some(0));
        assert_eq!(slider_index(&duties, true, None), None);
        assert_eq!(slider_index(&duties, true, Some(&Value::text("x"))), None);
        assert_eq!(slider_index(&[], true, Some(&Value::Int(1))), None);
    }

    #[test]
    fn an_unknown_slider_starts_at_start_or_in_the_middle_never_at_the_lowest_value() {
        let duties: Vec<i64> = std::iter::once(0).chain(int_range(20, 100, 5)).collect();
        assert_eq!(duties[start_index(&duties, Some(40))], 40);
        assert_eq!(duties[start_index(&duties, Some(41))], 40);
        assert_ne!(start_index(&duties, None), 0);
        assert_eq!(start_index(&[], None), 0);
    }

    #[test]
    fn choice_index_matches_by_value() {
        let opts = vec![Choice::named("a"), Choice::new(Value::Int(3), "three")];
        assert_eq!(choice_index(&opts, Some(&Value::text("a"))), Some(0));
        assert_eq!(choice_index(&opts, Some(&Value::Int(3))), Some(1));
        assert_eq!(choice_index(&opts, Some(&Value::Int(4))), None);
        assert_eq!(choice_index(&opts, None), None);
    }

    #[test]
    fn small_helpers() {
        assert_eq!(capitalise("eco"), "Eco");
        assert_eq!(capitalise("GAME1"), "Game1");
        assert_eq!(capitalise(""), "");
        assert_eq!(int_range(0, 50, 5).len(), 11);
        assert_eq!(int_range(1, 5, 1), vec![1, 2, 3, 4, 5]);
        assert_eq!(Value::Bool(true).plain(), "on");
        assert_eq!(Value::Int(7).plain(), "7");
    }
}
