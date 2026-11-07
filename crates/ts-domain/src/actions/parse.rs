//! The action vocabulary: parsing and validating `name` or `name:argument` text.
//!
//! An action is validated when it is *configured* (a key binding, a saved profile, `keys
//! test`), never at key-press time, so a typo can never make the PREDATOR key do something
//! unexpected. Parsing produces a typed [`Action`]; everything downstream matches on the enum
//! instead of re-splitting strings, so a new action cannot be added without the compiler
//! pointing at every place that must handle it.
#![forbid(unsafe_code)]

use std::fmt;
use std::sync::Arc;

use ts_platform::hw::{Coded, PowerMode};

use super::ActionError;

/// The keyboard effects `keyboard-effect-next` steps through, in this order. The position is
/// remembered in the user state (`kb_effect`) so the key continues where it left off.
pub const KB_EFFECTS: [&str; 13] = [
    "static",
    "breathing",
    "wave",
    "snake",
    "ripple",
    "neon",
    "raindrop",
    "lightning",
    "spot",
    "star",
    "fireball",
    "snow",
    "heartbeat",
];

/// The rear light effects `rear-effect-next` steps through, in this order (`rear_effect` in the
/// user state). The same five the firmware knows.
pub const REAR_EFFECTS: [&str; 5] = ["static", "breathing", "neon", "wave", "twinkling"];

/// Actions that take no argument.
pub const FIXED_NAMES: [&str; 16] = [
    "auto-next",
    "none",
    "power-next",
    "power-prev",
    "fan-max-toggle",
    "smart-fan-toggle",
    "profile-next",
    "output-toggle",
    "output-cycle",
    "mic-toggle",
    "voice-next",
    "touchpad-toggle",
    "keyboard-effect-next",
    "rear-effect-next",
    "battery-limit-toggle",
    "overdrive-toggle",
];

/// Actions that need an argument after the first colon.
pub const ARG_NAMES: [&str; 14] = [
    "look",
    "auto",
    "power",
    "power-cycle",
    "fan",
    "profile",
    "output",
    "mic",
    "touchpad",
    "keyboard",
    "keyboard-profile",
    "rear",
    "exec",
    "notify",
];

/// The reference text `truesense keys actions` prints and the window shows. It is part of the
/// product: a test checks that every action name appears in it.
pub const CATALOGUE: &str = "An action is `name` or `name:argument`. Each returns a short message (shown on screen by the caller). Unknown names are rejected when configuring, never at key-press time.\n\
\n\
  none\n\
  power-next | power-prev | power:<mode> | power-cycle:<mode,mode,...>     eco quiet balanced performance turbo (a manual choice holds for [profile] hold_minutes)\n\
  fan:smart | fan:auto | fan:max | fan:<percent> | fan-max-toggle | smart-fan-toggle\n\
  profile:<name> | profile-next                                              saved profiles (see `truesense profile`)\n\
  output-toggle | output-cycle | output:speakers | output:headphones\n\
  mic-toggle | mic:mute | mic:unmute | voice-next\n\
  touchpad-toggle | touchpad:on | touchpad:off\n\
  keyboard:on | keyboard:off | keyboard:toggle | keyboard-effect-next | keyboard-profile:<name>\n\
  rear:on | rear:off | rear:toggle | rear-effect-next\n\
  battery-limit-toggle | overdrive-toggle\n\
  look:<name> | auto:<off|native|smart> | auto-next                           lighting looks (truesense look), the mode control\n\
  exec:<shell command>      runs as you, detached      |      notify:<text>";

/// Every action name, with and without argument, sorted. A test or a UI uses it to check that
/// each one is documented in [`CATALOGUE`].
pub fn names() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = FIXED_NAMES.iter().chain(ARG_NAMES.iter()).copied().collect();
    out.sort_unstable();
    out
}

/// Splits `name:argument` at the first colon and trims both parts. The argument of `exec:` may
/// itself contain colons, which is why only the first one separates.
pub fn split(spec: &str) -> (&str, &str) {
    let (name, arg) = spec.trim().split_once(':').unwrap_or((spec.trim(), ""));
    (name.trim(), arg.trim())
}

/// Where the sound goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioOutput {
    /// The laptop speakers.
    Speakers,
    /// The 3.5 mm headphone jack.
    Headphones,
}

impl AudioOutput {
    /// The name used in actions, profiles and the sound system's profile table.
    pub fn name(self) -> &'static str {
        match self {
            AudioOutput::Speakers => "speakers",
            AudioOutput::Headphones => "headphones",
        }
    }

    /// The label shown on screen.
    pub fn label(self) -> &'static str {
        match self {
            AudioOutput::Speakers => "Laptop speakers",
            AudioOutput::Headphones => "Headphones (3.5 mm jack)",
        }
    }

    fn from_name(name: &str) -> Option<AudioOutput> {
        match name {
            "speakers" => Some(AudioOutput::Speakers),
            "headphones" => Some(AudioOutput::Headphones),
            _ => None,
        }
    }
}

/// `on`, `off` or `toggle`, for the keyboard and rear light actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Switch {
    /// Switch on.
    On,
    /// Switch off.
    Off,
    /// Flip the current state.
    Toggle,
}

impl Switch {
    fn from_name(name: &str) -> Option<Switch> {
        match name {
            "on" => Some(Switch::On),
            "off" => Some(Switch::Off),
            "toggle" => Some(Switch::Toggle),
            _ => None,
        }
    }

    /// The state this switch leads to from `current`.
    pub fn resolve(self, current: bool) -> bool {
        match self {
            Switch::On => true,
            Switch::Off => false,
            Switch::Toggle => !current,
        }
    }
}

/// What to do with the touchpad. The same words the daemon's `touchpad` request takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchpadAction {
    /// Enable it.
    On,
    /// Disable it.
    Off,
    /// Flip it.
    Toggle,
}

impl TouchpadAction {
    /// The word on the wire.
    pub fn name(self) -> &'static str {
        match self {
            TouchpadAction::On => "on",
            TouchpadAction::Off => "off",
            TouchpadAction::Toggle => "toggle",
        }
    }
}

/// The mode control (which of the daemon's automatic policies chooses the power mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoMode {
    /// No automatic mode control.
    Off,
    /// The firmware's own behaviour, applied by TrueSense.
    Native,
    /// The Smart auto profile.
    Smart,
}

impl AutoMode {
    /// The word used in actions, profiles and the daemon's `auto_mode` setting.
    pub fn name(self) -> &'static str {
        match self {
            AutoMode::Off => "off",
            AutoMode::Native => "native",
            AutoMode::Smart => "smart",
        }
    }

    fn from_name(name: &str) -> Option<AutoMode> {
        match name {
            "off" => Some(AutoMode::Off),
            "native" => Some(AutoMode::Native),
            "smart" => Some(AutoMode::Smart),
            _ => None,
        }
    }

    /// The next one in the `auto-next` cycle: off, native, smart, off. Any other stored value
    /// (a hand-edited state) goes to Smart, as in the Python edition.
    pub fn next_after(current: &str) -> AutoMode {
        match current {
            "off" => AutoMode::Native,
            "native" => AutoMode::Smart,
            "smart" => AutoMode::Off,
            _ => AutoMode::Smart,
        }
    }
}

/// A parsed, validated action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Does nothing (an unbound gesture).
    None,
    /// The next mode of the configured cycle.
    PowerNext,
    /// The previous mode of the configured cycle.
    PowerPrev,
    /// One power mode (a manual choice, held for `[profile] hold_minutes`).
    Power(String),
    /// The next mode of the listed cycle.
    PowerCycle(Vec<String>),
    /// The smart fan curve.
    FanSmart,
    /// Firmware fan control.
    FanAuto,
    /// Maximum fans.
    FanMax,
    /// A manual duty for both fans, 20..=100: a key can never stop a fan.
    FanManual(u8),
    /// Max fans on, and back to the smart curve when they already are.
    FanMaxToggle,
    /// The smart curve on or off.
    SmartFanToggle,
    /// Apply a whole-setup profile.
    Profile(String),
    /// Apply the whole-setup profile after the last one applied.
    ProfileNext,
    /// Flip between speakers and headphones.
    OutputToggle,
    /// Step through the audio outputs.
    OutputCycle,
    /// Choose an audio output.
    Output(AudioOutput),
    /// Flip the microphone mute.
    MicToggle,
    /// Mute or unmute the microphones (`true` is mute).
    Mic(bool),
    /// The next noise-reduction mode.
    VoiceNext,
    /// Flip the touchpad.
    TouchpadToggle,
    /// Enable or disable the touchpad.
    Touchpad(TouchpadAction),
    /// The keyboard backlight.
    Keyboard(Switch),
    /// The next keyboard effect.
    KeyboardEffectNext,
    /// A saved per-key lighting profile.
    KeyboardProfile(String),
    /// The rear light.
    Rear(Switch),
    /// The next rear light effect.
    RearEffectNext,
    /// The firmware 80 % battery limit, flipped.
    BatteryLimitToggle,
    /// The LCD overdrive, flipped.
    OverdriveToggle,
    /// A saved lighting look.
    Look(String),
    /// A mode control.
    Auto(AutoMode),
    /// The next mode control.
    AutoNext,
    /// A shell command, run as the user.
    Exec(String),
    /// A message to show.
    Notify(String),
}

/// Checks and parses actions. `look:NAME` needs the saved looks to be checked against; without
/// a look source only the syntax of a look action is checked.
#[derive(Clone, Default)]
pub struct Validator {
    looks: Option<Arc<dyn Fn() -> Vec<String> + Send + Sync>>,
}

impl fmt::Debug for Validator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Validator").field("looks", &self.looks.is_some()).finish()
    }
}

impl Validator {
    /// A validator that also checks `look:NAME` against the names `looks` returns.
    pub fn with_looks(looks: Arc<dyn Fn() -> Vec<String> + Send + Sync>) -> Validator {
        Validator { looks: Some(looks) }
    }

    /// Parses an action, or says why it is not valid in the words shown to the user. It is used
    /// when configuring keys and profiles, so it is strict.
    pub fn parse(&self, spec: &str) -> Result<Action, ActionError> {
        let (name, arg) = split(spec);
        let bad = |m: String| Err(ActionError::Invalid(m));
        if FIXED_NAMES.contains(&name) {
            if !arg.is_empty() {
                return bad(format!("'{name}' takes no argument"));
            }
            return Ok(fixed(name));
        }
        if !ARG_NAMES.contains(&name) {
            return bad(format!("unknown action '{name}'"));
        }
        if arg.is_empty() {
            return bad(format!("'{name}' needs an argument (e.g. {name}:...)"));
        }
        let action = match name {
            "auto" => match AutoMode::from_name(arg) {
                Some(m) => Action::Auto(m),
                None => return bad("auto: off | native | smart".into()),
            },
            "look" => {
                if let Some(looks) = &self.looks
                    && !looks().iter().any(|l| l == arg)
                {
                    return bad(format!("no saved look '{arg}' (truesense look list)"));
                }
                Action::Look(arg.to_owned())
            }
            "power" => {
                if PowerMode::from_name(arg).is_err() {
                    return bad(format!("power mode must be one of {}", mode_names()));
                }
                Action::Power(arg.to_owned())
            }
            "power-cycle" => {
                let modes: Vec<String> = arg.split(',').map(str::to_owned).collect();
                if modes.iter().any(|m| PowerMode::from_name(m).is_err()) {
                    return bad("power-cycle needs a comma list of power modes".into());
                }
                Action::PowerCycle(modes)
            }
            "fan" => match arg {
                "smart" => Action::FanSmart,
                "auto" => Action::FanAuto,
                "max" => Action::FanMax,
                _ => match fan_percent(arg) {
                    Some(n) => Action::FanManual(n),
                    None => return bad("fan: smart | auto | max | 20..100".into()),
                },
            },
            "output" => match AudioOutput::from_name(arg) {
                Some(o) => Action::Output(o),
                None => return bad("output: speakers | headphones".into()),
            },
            "mic" => match arg {
                "mute" => Action::Mic(true),
                "unmute" => Action::Mic(false),
                _ => return bad("mic: mute | unmute".into()),
            },
            "touchpad" => match arg {
                "on" => Action::Touchpad(TouchpadAction::On),
                "off" => Action::Touchpad(TouchpadAction::Off),
                _ => return bad("touchpad: on | off".into()),
            },
            "keyboard" => match Switch::from_name(arg) {
                Some(s) => Action::Keyboard(s),
                None => return bad("keyboard: on | off | toggle".into()),
            },
            "rear" => match Switch::from_name(arg) {
                Some(s) => Action::Rear(s),
                None => return bad("rear: on | off | toggle".into()),
            },
            "profile" => Action::Profile(arg.to_owned()),
            "keyboard-profile" => Action::KeyboardProfile(arg.to_owned()),
            "exec" => Action::Exec(arg.to_owned()),
            "notify" => Action::Notify(arg.to_owned()),
            // Every name of ARG_NAMES is matched above; a name added to the list without an
            // arm is reported instead of silently accepted.
            _ => return bad(format!("unknown action '{name}'")),
        };
        Ok(action)
    }

    /// [`Validator::parse`] for callers that only want the verdict.
    pub fn validate(&self, spec: &str) -> Result<(), ActionError> {
        self.parse(spec).map(|_| ())
    }
}

/// The syntax-only check, for callers with no look store at hand.
pub fn validate(spec: &str) -> Result<(), ActionError> {
    Validator::default().validate(spec)
}

fn mode_names() -> String {
    PowerMode::ALL.iter().map(|m| m.name()).collect::<Vec<_>>().join(", ")
}

fn fixed(name: &str) -> Action {
    match name {
        "auto-next" => Action::AutoNext,
        "power-next" => Action::PowerNext,
        "power-prev" => Action::PowerPrev,
        "fan-max-toggle" => Action::FanMaxToggle,
        "smart-fan-toggle" => Action::SmartFanToggle,
        "profile-next" => Action::ProfileNext,
        "output-toggle" => Action::OutputToggle,
        "output-cycle" => Action::OutputCycle,
        "mic-toggle" => Action::MicToggle,
        "voice-next" => Action::VoiceNext,
        "touchpad-toggle" => Action::TouchpadToggle,
        "keyboard-effect-next" => Action::KeyboardEffectNext,
        "rear-effect-next" => Action::RearEffectNext,
        "battery-limit-toggle" => Action::BatteryLimitToggle,
        "overdrive-toggle" => Action::OverdriveToggle,
        _ => Action::None,
    }
}

/// A manual duty: ASCII digits only, 20..=100. A fan is never stopped by a key, so 0 is not an
/// option here (the Python edition used `str.isdigit`, which also accepts digits such as
/// superscripts that `int` then cannot read; those are refused here).
fn fan_percent(arg: &str) -> Option<u8> {
    if arg.is_empty() || !arg.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u64 = arg.parse().ok()?;
    if (20..=100).contains(&n) { u8::try_from(n).ok() } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(spec: &str) -> Option<String> {
        validate(spec).err().map(|e| e.to_string())
    }

    #[test]
    fn good_actions_validate() {
        let good = [
            "none",
            "power-next",
            "power-prev",
            "power:turbo",
            "power-cycle:eco,turbo",
            "fan:smart",
            "fan:auto",
            "fan:max",
            "fan:60",
            "fan:020",
            "fan:100",
            "fan-max-toggle",
            "smart-fan-toggle",
            "profile:gaming",
            "profile-next",
            "output-toggle",
            "output-cycle",
            "output:speakers",
            "output:headphones",
            "mic-toggle",
            "mic:mute",
            "mic:unmute",
            "voice-next",
            "touchpad-toggle",
            "touchpad:on",
            "touchpad:off",
            "keyboard:on",
            "keyboard:off",
            "keyboard:toggle",
            "keyboard-effect-next",
            "keyboard-profile:mine",
            "rear:on",
            "rear:off",
            "rear:toggle",
            "rear-effect-next",
            "battery-limit-toggle",
            "overdrive-toggle",
            "look:native",
            "auto:smart",
            "auto-next",
            "exec:truesense gui",
            "exec:notify-send a:b",
            "notify:hi there",
            "  power:eco  ",
            "power : eco",
        ];
        for g in good {
            assert_eq!(msg(g), None, "{g}");
        }
    }

    #[test]
    fn bad_actions_say_why() {
        let table = [
            ("", "unknown action ''"),
            (":x", "unknown action ''"),
            ("frobnicate", "unknown action 'frobnicate'"),
            ("none:x", "'none' takes no argument"),
            ("power-next:1", "'power-next' takes no argument"),
            ("power", "'power' needs an argument (e.g. power:...)"),
            ("power:", "'power' needs an argument (e.g. power:...)"),
            ("exec", "'exec' needs an argument (e.g. exec:...)"),
            ("auto:fast", "auto: off | native | smart"),
            ("power:warp", "power mode must be one of quiet, balanced, performance, turbo, eco"),
            ("power-cycle:eco,warp", "power-cycle needs a comma list of power modes"),
            ("power-cycle:eco,,turbo", "power-cycle needs a comma list of power modes"),
            ("power-cycle:eco, turbo", "power-cycle needs a comma list of power modes"),
            ("fan:0", "fan: smart | auto | max | 20..100"),
            ("fan:19", "fan: smart | auto | max | 20..100"),
            ("fan:101", "fan: smart | auto | max | 20..100"),
            ("fan:-30", "fan: smart | auto | max | 20..100"),
            ("fan:+30", "fan: smart | auto | max | 20..100"),
            ("fan:3.5", "fan: smart | auto | max | 20..100"),
            ("fan:\u{b2}0", "fan: smart | auto | max | 20..100"),
            ("fan:99999999999999999999999", "fan: smart | auto | max | 20..100"),
            ("fan:turbo", "fan: smart | auto | max | 20..100"),
            ("output:hdmi", "output: speakers | headphones"),
            ("mic:half", "mic: mute | unmute"),
            ("touchpad:toggle", "touchpad: on | off"),
            ("keyboard:dim", "keyboard: on | off | toggle"),
            ("rear:dim", "rear: on | off | toggle"),
        ];
        for (spec, want) in table {
            assert_eq!(msg(spec).as_deref(), Some(want), "{spec:?}");
        }
    }

    #[test]
    fn look_is_checked_against_the_saved_looks_when_known() {
        let v = Validator::with_looks(Arc::new(|| vec!["native".to_owned(), "calm".to_owned()]));
        assert!(v.validate("look:calm").is_ok());
        assert_eq!(
            v.validate("look:gone").err().map(|e| e.to_string()).as_deref(),
            Some("no saved look 'gone' (truesense look list)")
        );
        // Without a look source only the syntax is checked.
        assert!(validate("look:gone").is_ok());
        assert!(validate("look").is_err());
    }

    #[test]
    fn exec_keeps_colons_and_spaces_in_its_argument() {
        assert_eq!(
            Validator::default().parse("exec: sh -c 'echo a:b' ").ok(),
            Some(Action::Exec("sh -c 'echo a:b'".into()))
        );
    }

    #[test]
    fn parse_gives_typed_actions() {
        let v = Validator::default();
        assert_eq!(v.parse("fan:60").ok(), Some(Action::FanManual(60)));
        assert_eq!(
            v.parse("power-cycle:eco,turbo").ok(),
            Some(Action::PowerCycle(vec!["eco".into(), "turbo".into()]))
        );
        assert_eq!(v.parse("rear:toggle").ok(), Some(Action::Rear(Switch::Toggle)));
        assert_eq!(v.parse("mic:mute").ok(), Some(Action::Mic(true)));
        assert_eq!(v.parse("auto:native").ok(), Some(Action::Auto(AutoMode::Native)));
    }

    #[test]
    fn every_name_is_in_the_catalogue_and_every_listed_name_parses_to_something() {
        for n in names() {
            assert!(CATALOGUE.contains(n), "{n} missing from the documented catalogue");
        }
        assert_eq!(names().len(), FIXED_NAMES.len() + ARG_NAMES.len());
        // The list is sorted and free of duplicates.
        let mut sorted = names();
        sorted.dedup();
        assert_eq!(sorted.len(), names().len());
        for w in names().windows(2) {
            assert!(w[0] < w[1]);
        }
        // Every name is handled by the parser: a fixed name parses bare, an argument name has a
        // valid example.
        for n in FIXED_NAMES {
            assert!(validate(n).is_ok(), "{n}");
        }
        let examples = [
            "look:x",
            "auto:off",
            "power:eco",
            "power-cycle:eco",
            "fan:50",
            "profile:x",
            "output:speakers",
            "mic:mute",
            "touchpad:on",
            "keyboard:on",
            "keyboard-profile:x",
            "rear:on",
            "exec:true",
            "notify:x",
        ];
        assert_eq!(examples.len(), ARG_NAMES.len());
        for e in examples {
            assert!(validate(e).is_ok(), "{e}");
        }
    }

    #[test]
    fn catalogue_text_matches_the_python_edition() {
        assert!(CATALOGUE.starts_with("An action is `name` or `name:argument`."));
        assert!(
            CATALOGUE.ends_with("exec:<shell command>      runs as you, detached      |      notify:<text>")
        );
        assert_eq!(CATALOGUE.lines().count(), 14);
    }

    #[test]
    fn split_handles_hostile_text() {
        assert_eq!(split(""), ("", ""));
        assert_eq!(split(":"), ("", ""));
        assert_eq!(split("a:b:c"), ("a", "b:c"));
        assert_eq!(split(" a : b "), ("a", "b"));
        assert_eq!(split("\u{0}:\u{0}"), ("\u{0}", "\u{0}"));
    }

    #[test]
    fn auto_cycle_and_switch_rules() {
        assert_eq!(AutoMode::next_after("off"), AutoMode::Native);
        assert_eq!(AutoMode::next_after("native"), AutoMode::Smart);
        assert_eq!(AutoMode::next_after("smart"), AutoMode::Off);
        assert_eq!(AutoMode::next_after("bogus"), AutoMode::Smart);
        assert!(Switch::Toggle.resolve(false));
        assert!(!Switch::Toggle.resolve(true));
        assert!(Switch::On.resolve(true));
        assert!(!Switch::Off.resolve(true));
    }
}
