//! PREDATOR key binding profiles: a named set of what each gesture does (tap, double tap,
//! triple tap, long press, Fn+Esc) and the press timing, plus the mapping from key events to
//! those actions.
//!
//! Built-in profiles are read-only (copy one to make your own); yours live in
//! `~/.config/truesense/keys/NAME.json`, the files 0.3.x wrote. Using a profile writes its
//! actions into the user's configuration (`[keys]`) and records its name; the key service
//! notices the change and reloads by itself. Every action is validated when a profile is
//! loaded or saved, never at key-press time.
//!
//! The storage is the generic [`ProfileStore`]; this module owns what is specific to key
//! bindings: the gestures, the allowed timing, the built-in sets, the validation of a
//! profile, writing the active bindings into the config, and decoding the vendor HID reports
//! of the Fn-key interface into [`KeyEvent`]s.
#![forbid(unsafe_code)]

use std::fmt;
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value};
use ts_foundation::config::{self, Config, ConfigError};

use crate::actions;
use crate::profiles::{Kind, NameRule, ProfileError, ProfileStore, Source, Validator};

/// The bindable gestures, in the order menus show them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Gesture {
    /// One short press.
    Tap,
    /// Two short presses within [`Bindings::multi_ms`].
    Double,
    /// Three short presses within the same window.
    Triple,
    /// A press held for [`Bindings::long_ms`]; it fires while still held, so it feels instant.
    Long,
    /// Fn+Esc.
    FnEsc,
}

impl Gesture {
    /// Every gesture, in menu order.
    pub const ALL: [Gesture; 5] =
        [Gesture::Tap, Gesture::Double, Gesture::Triple, Gesture::Long, Gesture::FnEsc];

    /// The key of the gesture in profile files and the `[keys]` config section.
    pub fn key(self) -> &'static str {
        match self {
            Gesture::Tap => "tap",
            Gesture::Double => "double",
            Gesture::Triple => "triple",
            Gesture::Long => "long",
            Gesture::FnEsc => "fn_esc",
        }
    }

    /// The name shown in menus and in validation messages.
    pub fn label(self) -> &'static str {
        match self {
            Gesture::Tap => "Tap",
            Gesture::Double => "Double tap",
            Gesture::Triple => "Triple tap",
            Gesture::Long => "Long press",
            Gesture::FnEsc => "Fn + Esc",
        }
    }

    /// The gesture with this file key.
    pub fn from_key(key: &str) -> Option<Gesture> {
        Gesture::ALL.into_iter().find(|g| g.key() == key)
    }

    /// The gesture for a count of short presses: 1 tap, 2 double, 3 or more triple; none for 0.
    pub fn from_taps(taps: u32) -> Option<Gesture> {
        match taps {
            0 => None,
            1 => Some(Gesture::Tap),
            2 => Some(Gesture::Double),
            _ => Some(Gesture::Triple),
        }
    }
}

/// How long a press must be held to count as a long press, in milliseconds.
pub const LONG_MS_RANGE: RangeInclusive<i64> = 300..=2000;
/// How long to wait for another tap, in milliseconds.
pub const MULTI_MS_RANGE: RangeInclusive<i64> = 200..=1000;

/// What each gesture does and the press timing: the part of a profile the key service uses.
/// The field names are the 0.3.x JSON names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bindings {
    /// Action of a single tap.
    pub tap: String,
    /// Action of a double tap.
    pub double: String,
    /// Action of a triple tap.
    pub triple: String,
    /// Action of a long press.
    pub long: String,
    /// Action of Fn+Esc.
    pub fn_esc: String,
    /// Milliseconds that make a press long.
    pub long_ms: i64,
    /// Milliseconds within which further taps join the same gesture.
    pub multi_ms: i64,
}

impl Default for Bindings {
    /// The standard behaviour; the other built-in profiles change a few gestures of it.
    fn default() -> Self {
        Bindings {
            tap: "power-next".into(),
            double: "output-toggle".into(),
            triple: "none".into(),
            long: "fan-max-toggle".into(),
            fn_esc: "none".into(),
            long_ms: 800,
            multi_ms: 450,
        }
    }
}

impl Bindings {
    /// The action bound to a gesture.
    pub fn action(&self, gesture: Gesture) -> &str {
        match gesture {
            Gesture::Tap => &self.tap,
            Gesture::Double => &self.double,
            Gesture::Triple => &self.triple,
            Gesture::Long => &self.long,
            Gesture::FnEsc => &self.fn_esc,
        }
    }

    /// Binds an action to a gesture.
    pub fn set_action(&mut self, gesture: Gesture, spec: impl Into<String>) {
        let slot = match gesture {
            Gesture::Tap => &mut self.tap,
            Gesture::Double => &mut self.double,
            Gesture::Triple => &mut self.triple,
            Gesture::Long => &mut self.long,
            Gesture::FnEsc => &mut self.fn_esc,
        };
        *slot = spec.into();
    }

    /// The bindings the key service should run with: an invalid action becomes `none` (a typo
    /// in the config must not break the key at press time). Returns them with one warning line
    /// per replaced action, in gesture order.
    pub fn sanitised(&self, check: &ActionCheck) -> (Bindings, Vec<String>) {
        let mut out = self.clone();
        let mut warnings = Vec::new();
        for g in Gesture::ALL {
            let spec = self.action(g);
            if let Err(why) = check(spec) {
                warnings.push(format!("keys.{} = '{spec}' is invalid ({why}); using 'none'", g.key()));
                out.set_action(g, "none");
            }
        }
        (out, warnings)
    }

    /// Picks up a changed config without a restart: a valid new action replaces the old one, an
    /// invalid one is ignored (a half-written file keeps what is running), and the timing is
    /// taken as it is.
    pub fn reload(&mut self, new: &Bindings, check: &ActionCheck) {
        for g in Gesture::ALL {
            if check(new.action(g)).is_ok() {
                self.set_action(g, new.action(g));
            }
        }
        self.long_ms = new.long_ms;
        self.multi_ms = new.multi_ms;
    }

    /// What a key event runs: the label to log and the action, for the events that run an
    /// action directly (Fn+Esc is bound; Fn+F10 is always the touchpad toggle). `None` for
    /// presses and releases of the PREDATOR key, which go through the gesture timing, and for
    /// events nothing is bound to.
    pub fn action_for_event(&self, event: KeyEvent) -> Option<(&'static str, &str)> {
        match event {
            KeyEvent::FnEsc => Some(("fn_esc", self.action(Gesture::FnEsc))),
            KeyEvent::FnF10 => Some(("fn_f10", FN_F10_ACTION)),
            KeyEvent::PredatorDown | KeyEvent::Release | KeyEvent::Unknown => None,
        }
    }

    fn to_object(&self, description: &str) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("description".into(), Value::String(description.to_owned()));
        for g in Gesture::ALL {
            m.insert(g.key().into(), Value::String(self.action(g).to_owned()));
        }
        m.insert("long_ms".into(), Value::from(self.long_ms));
        m.insert("multi_ms".into(), Value::from(self.multi_ms));
        m
    }
}

/// Fn+F10 always toggles the touchpad (the firmware has no other use for it here); it is not
/// one of the bindable gestures.
pub const FN_F10_ACTION: &str = "touchpad-toggle";

/// What the Fn-key HID interface reports (usage page 0x88, reports starting with `0x04`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEvent {
    /// The PREDATOR key went down.
    PredatorDown,
    /// A key was released. It ends *every* vendor press (`04 00 00`), PREDATOR or not; the
    /// gesture logic must ignore a release that does not belong to a PREDATOR press.
    Release,
    /// Fn+Esc.
    FnEsc,
    /// Fn+F10.
    FnF10,
    /// A vendor report we do not know (Fn+F11 and Fn+F12: the keyboard changes brightness by
    /// itself and TrueSense deliberately ignores them).
    Unknown,
}

/// Decodes one report read from the Fn-key interface. `None` for anything that is not a
/// vendor report (too short, or not starting with `0x04`); never panics on hostile input.
pub fn decode_report(report: &[u8]) -> Option<KeyEvent> {
    let [0x04, a, b, ..] = report else { return None };
    Some(match (a, b) {
        (0x81, 0xff) => KeyEvent::PredatorDown,
        (0x00, 0x00) => KeyEvent::Release,
        (0x8a, 0xff) => KeyEvent::FnEsc,
        (0x80, 0xff) => KeyEvent::FnF10,
        _ => KeyEvent::Unknown,
    })
}

/// A named set of bindings with a description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyProfile {
    /// What the profile is for, shown in the list.
    pub description: String,
    /// The bindings.
    pub bindings: Bindings,
}

impl KeyProfile {
    fn to_value(&self) -> Value {
        Value::Object(self.bindings.to_object(&self.description))
    }
}

/// The names of the built-in profiles, in the order they are shown.
pub const BUILTIN_NAMES: [&str; 5] = ["native", "default", "gaming", "work", "lights"];

/// A built-in profile.
pub fn builtin(name: &str) -> Option<KeyProfile> {
    let mut b = Bindings::default();
    let description = match name {
        "native" => {
            b.tap = "exec:truesense gui".into();
            b.double = "none".into();
            b.long = "none".into();
            "PredatorSense default: the PREDATOR key opens the app, nothing else is bound"
        }
        "default" => {
            "The standard behaviour: tap steps the power mode, double tap switches speakers/headphones, long press toggles Max fans"
        }
        "gaming" => {
            b.tap = "power-cycle:performance,turbo".into();
            b.double = "fan-max-toggle".into();
            b.triple = "keyboard-effect-next".into();
            b.long = "auto-next".into();
            b.fn_esc = "mic-toggle".into();
            "Gaming: tap toggles Performance and Turbo, double tap Max fans, triple tap the next lighting effect, long press Smart auto, Fn+Esc mutes the microphone"
        }
        "work" => {
            b.tap = "power-cycle:eco,quiet,balanced".into();
            b.double = "output-toggle".into();
            b.triple = "mic-toggle".into();
            b.long = "touchpad-toggle".into();
            "Work: tap steps Eco, Quiet and Balanced, double tap switches audio, triple tap mutes the microphone, long press toggles the touchpad"
        }
        "lights" => {
            b.tap = "keyboard-effect-next".into();
            b.double = "rear-effect-next".into();
            b.triple = "keyboard:toggle".into();
            b.long = "rear:toggle".into();
            "Lighting: tap the next keyboard effect, double tap the next rear effect, triple tap the keyboard light, long press the rear light"
        }
        _ => return None,
    };
    Some(KeyProfile { description: description.to_owned(), bindings: b })
}

/// Checks one action text; the text of the error is shown to the user. The application passes
/// a check that knows the saved looks (`Runner::validator`); [`syntax_check`] is the plain one.
pub type ActionCheck = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// The check that only looks at the action's syntax.
pub fn syntax_check() -> ActionCheck {
    Arc::new(|spec| actions::validate(spec).map_err(|e| e.to_string()))
}

/// The check of an action [`actions::Validator`] (which knows the saved looks).
pub fn check_with(validator: actions::Validator) -> ActionCheck {
    Arc::new(move |spec| validator.validate(spec).map_err(|e| e.to_string()))
}

/// Checks a decoded profile object and returns it with defaults filled in for what is missing.
/// Every action is checked here, so a bad action is refused when it is set. The messages are
/// the Python edition's.
pub fn validate(value: &Value, check: &ActionCheck) -> Result<KeyProfile, String> {
    let Value::Object(m) = value else {
        return Err("a key profile is an object with tap, double, triple, long and fn_esc".into());
    };
    let mut b = Bindings::default();
    let description = m.get("description").and_then(Value::as_str).unwrap_or("").to_owned();
    for g in Gesture::ALL {
        let spec = match m.get(g.key()) {
            None => b.action(g).to_owned(),
            Some(Value::String(s)) => s.clone(),
            Some(_) => return Err(format!("{}: an action is text", g.label())),
        };
        check(&spec).map_err(|why| format!("{}: {why}", g.label()))?;
        b.set_action(g, spec);
    }
    b.long_ms = timing(m, "long_ms", b.long_ms, &LONG_MS_RANGE)?;
    b.multi_ms = timing(m, "multi_ms", b.multi_ms, &MULTI_MS_RANGE)?;
    Ok(KeyProfile { description, bindings: b })
}

/// An integer in `range`. A number with a fractional part or exponent, a string, a boolean or
/// a number too big for 64 bits is refused, as 0.3.x did.
fn timing(
    m: &Map<String, Value>,
    key: &str,
    default: i64,
    range: &RangeInclusive<i64>,
) -> Result<i64, String> {
    let Some(raw) = m.get(key) else { return Ok(default) };
    match raw.as_i64() {
        Some(v) if range.contains(&v) && !matches!(raw, Value::Bool(_)) => Ok(v),
        _ => Err(format!("{key}: {}..{}", range.start(), range.end())),
    }
}

/// Why a key-profile operation failed.
#[derive(Debug)]
pub enum KeybindsError {
    /// The profile store refused (name, missing, read-only, invalid, disk).
    Profile(ProfileError),
    /// Writing the user's config failed.
    Config(ConfigError),
}

impl fmt::Display for KeybindsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeybindsError::Profile(e) => fmt::Display::fmt(e, f),
            KeybindsError::Config(e) => write!(f, "use key profile: {e}"),
        }
    }
}

impl std::error::Error for KeybindsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            KeybindsError::Profile(e) => Some(e),
            KeybindsError::Config(e) => Some(e),
        }
    }
}

impl From<ProfileError> for KeybindsError {
    fn from(e: ProfileError) -> Self {
        KeybindsError::Profile(e)
    }
}

/// The words of key profiles in messages and where they are kept.
pub const KIND: Kind = Kind {
    dir_name: "keys",
    label: "key profile",
    list_hint: "truesense keys profile list",
    copy_hint: Some("keys profile copy"),
    rule: NameRule::Standard,
    aliases: &[],
    key_order: &["description", "tap", "double", "triple", "long", "fn_esc", "long_ms", "multi_ms"],
};

/// The active bindings: what the key service uses, and the profile they came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Active {
    /// The bindings in force.
    pub bindings: Bindings,
    /// The name of the profile they came from.
    pub profile: String,
}

/// The bindings in a configuration (`[keys]`). An empty profile name reads as `default`.
pub fn from_config(cfg: &Config) -> Active {
    let k = &cfg.keys;
    Active {
        bindings: Bindings {
            tap: k.tap.clone(),
            double: k.double.clone(),
            triple: k.triple.clone(),
            long: k.long.clone(),
            fn_esc: k.fn_esc.clone(),
            long_ms: k.long_ms,
            multi_ms: k.multi_ms,
        },
        profile: if k.profile.is_empty() { "default".to_owned() } else { k.profile.clone() },
    }
}

/// The key profiles on disk plus the user's configuration file they are used through.
pub struct KeyProfiles {
    store: ProfileStore,
    check: ActionCheck,
    config_path: PathBuf,
}

impl fmt::Debug for KeyProfiles {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyProfiles")
            .field("store", &self.store)
            .field("config_path", &self.config_path)
            .finish()
    }
}

impl KeyProfiles {
    /// Profiles in `user_dir` (`~/.config/truesense/keys`), used through the config file at
    /// `config_path` (`~/.config/truesense/config.toml`), validating actions with `check`.
    pub fn new(
        user_dir: impl Into<PathBuf>,
        config_path: impl Into<PathBuf>,
        check: ActionCheck,
    ) -> Result<KeyProfiles, KeybindsError> {
        let builtins: Vec<(String, Value)> = BUILTIN_NAMES
            .iter()
            .filter_map(|n| builtin(n).map(|p| ((*n).to_owned(), p.to_value())))
            .collect();
        let for_store = Arc::clone(&check);
        let validate_for_store: Validator =
            Arc::new(move |v: &Value| validate(v, &for_store).map(|p| p.to_value()));
        // The built-ins include `native`, which is all the store insists on.
        let store = ProfileStore::new(KIND, user_dir, builtins, validate_for_store)?;
        Ok(KeyProfiles { store, check, config_path: config_path.into() })
    }

    /// The generic store underneath (listing, sources, name rules).
    pub fn store(&self) -> &ProfileStore {
        &self.store
    }

    /// Checks a decoded profile object with this store's action check.
    pub fn validate(&self, value: &Value) -> Result<KeyProfile, String> {
        validate(value, &self.check)
    }

    /// The built-in names first, then yours sorted.
    pub fn names(&self) -> Vec<String> {
        self.store.names()
    }

    /// Where a profile comes from.
    pub fn source(&self, name: &str) -> Option<Source> {
        self.store.source(name)
    }

    /// A built-in or saved profile, validated.
    pub fn load(&self, name: &str) -> Result<KeyProfile, KeybindsError> {
        let v = self.store.load(name)?;
        self.validate(&v).map_err(|m| KeybindsError::Profile(ProfileError::Invalid(m)))
    }

    /// Validates and writes a profile. Built-in names are refused so a built-in can never be
    /// altered: copy it under another name instead.
    pub fn save(&self, name: &str, profile: &KeyProfile) -> Result<KeyProfile, KeybindsError> {
        let v = self.store.save(name, &profile.to_value())?;
        self.validate(&v).map_err(|m| KeybindsError::Profile(ProfileError::Invalid(m)))
    }

    /// Removes a saved profile.
    pub fn delete(&self, name: &str) -> Result<(), KeybindsError> {
        Ok(self.store.delete(name)?)
    }

    /// Saves a profile under a new name (default `NAME-copy`, numbered until free) and returns
    /// that name.
    pub fn copy(&self, name: &str, new_name: Option<&str>) -> Result<String, KeybindsError> {
        Ok(self.store.copy(name, new_name)?)
    }

    /// Makes a profile the active one: its actions go into the user's configuration under
    /// `[keys]` together with its name; the key service reloads on its own. Everything else in
    /// the user's config is kept.
    pub fn use_profile(&self, name: &str) -> Result<KeyProfile, KeybindsError> {
        let p = self.load(name)?;
        let mut pairs: Vec<(&str, config::Value)> = Gesture::ALL
            .iter()
            .map(|g| (g.key(), config::Value::String(p.bindings.action(*g).to_owned())))
            .collect();
        pairs.push(("long_ms", config::Value::Integer(p.bindings.long_ms)));
        pairs.push(("multi_ms", config::Value::Integer(p.bindings.multi_ms)));
        pairs.push(("profile", config::Value::String(name.to_owned())));
        config::write_user(&self.config_path, config::section_table("keys", pairs))
            .map_err(KeybindsError::Config)?;
        Ok(p)
    }
}

#[cfg(test)]
#[path = "keybinds_tests.rs"]
mod tests;
