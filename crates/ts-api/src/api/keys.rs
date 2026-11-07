//! The PREDATOR key and Fn+Esc: what each gesture does, the press timing, and the saved binding
//! profiles.
//!
//! An action is checked when it is bound, never at key-press time, and the key service reloads the
//! configuration by itself.
#![forbid(unsafe_code)]

use std::fmt;

use serde::Serialize;
use ts_domain::actions;
use ts_domain::keybinds::{self, Bindings, Gesture, KeyProfile, LONG_MS_RANGE, MULTI_MS_RANGE};
use ts_foundation::config::{Config, Value, section_table};

use super::{Api, ApiError, ApiResult, NamedList};

/// What each gesture does and the press timing; the field names are the 0.3.x JSON names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeyBindings {
    /// A single tap.
    pub tap: String,
    /// Two taps.
    pub double: String,
    /// Three taps.
    pub triple: String,
    /// A long press.
    pub long: String,
    /// Fn+Esc.
    pub fn_esc: String,
    /// Milliseconds that make a press long.
    pub long_ms: i64,
    /// Milliseconds within which further taps join the same gesture.
    pub multi_ms: i64,
}

impl From<&Bindings> for KeyBindings {
    fn from(b: &Bindings) -> Self {
        Self {
            tap: b.tap.clone(),
            double: b.double.clone(),
            triple: b.triple.clone(),
            long: b.long.clone(),
            fn_esc: b.fn_esc.clone(),
            long_ms: b.long_ms,
            multi_ms: b.multi_ms,
        }
    }
}

impl KeyBindings {
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

    fn timing_line(&self) -> String {
        format!("long press after {} ms; taps within {} ms count together", self.long_ms, self.multi_ms)
    }
}

/// The `[keys]` section of the configuration: the bindings in force, the profile they came from and
/// the modes the power key cycles through.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeysSettings {
    /// The profile the bindings came from (`custom` after a hand edit).
    pub profile: String,
    /// The bindings.
    #[serde(flatten)]
    pub bindings: KeyBindings,
    /// The modes `power-next` and `power-prev` walk through.
    pub power_cycle: Vec<String>,
}

impl fmt::Display for KeysSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for g in Gesture::ALL {
            writeln!(f, "{:<8} {}", g.key(), self.bindings.action(g))?;
        }
        write!(f, "({})", self.bindings.timing_line())
    }
}

/// A binding profile: its description and bindings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeyProfileView {
    /// What the profile is for.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// The bindings.
    #[serde(flatten)]
    pub bindings: KeyBindings,
}

impl From<&KeyProfile> for KeyProfileView {
    fn from(p: &KeyProfile) -> Self {
        Self { description: p.description.clone(), bindings: (&p.bindings).into() }
    }
}

impl fmt::Display for KeyProfileView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for g in Gesture::ALL {
            writeln!(f, "  {:<11} {}", g.label(), self.bindings.action(g))?;
        }
        write!(f, "  {}", self.bindings.timing_line())
    }
}

/// The long-press and multi-tap timing in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct KeyTiming {
    /// Milliseconds that make a press long.
    pub long_ms: i64,
    /// Milliseconds within which further taps join the same gesture.
    pub multi_ms: i64,
}

/// What `keys profile save` changes: the action of each gesture and the timing, when given, over
/// the profile to start from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyProfileEdit {
    /// The profile to start from; else your own profile of that name, else the defaults.
    pub from: Option<String>,
    /// The action to bind to each gesture named.
    pub actions: Vec<(Gesture, String)>,
    /// A new long-press time.
    pub long_ms: Option<i64>,
    /// A new multi-tap window.
    pub multi_ms: Option<i64>,
}

fn keys_of(cfg: &Config) -> KeysSettings {
    let active = keybinds::from_config(cfg);
    KeysSettings {
        profile: cfg.keys.profile.clone(),
        bindings: (&active.bindings).into(),
        power_cycle: cfg.keys.power_cycle.clone(),
    }
}

impl Api {
    /// The `[keys]` section as configured now.
    pub fn key_settings(&self) -> ApiResult<KeysSettings> {
        Ok(keys_of(&self.load_config()?))
    }

    /// The catalogue of actions a gesture can be bound to.
    pub fn key_actions() -> &'static str {
        actions::CATALOGUE
    }

    /// The gesture named by text (`tap`, `double`, `triple`, `long`, `fn_esc`).
    pub fn parse_gesture(text: &str) -> ApiResult<Gesture> {
        Gesture::from_key(text).ok_or_else(|| {
            ApiError::invalid(format!("gesture: tap | double | triple | long | fn_esc (got '{text}')"))
        })
    }

    /// Binds an action to a gesture. The action is checked now; the key service reloads by itself.
    pub fn set_key_action(&self, gesture: Gesture, action: &str) -> ApiResult<()> {
        // Same rule as the key service at start-up: a `look:NAME` must name a saved look.
        let looks = self.s.looks.names();
        actions::Validator::with_looks(std::sync::Arc::new(move || looks.clone()))
            .validate(action)
            .map_err(|e| ApiError::invalid(format!("{e}   (truesense keys actions)")))?;
        self.s.config.write_user(section_table(
            "keys",
            [
                (gesture.key(), Value::String(action.to_owned())),
                ("profile", Value::String("custom".to_owned())),
            ],
        ))
    }

    /// Puts the default action back on one gesture, or on all of them, and answers which.
    pub fn reset_keys(&self, gesture: Option<Gesture>) -> ApiResult<Vec<Gesture>> {
        let gestures: Vec<Gesture> = gesture.map_or_else(|| Gesture::ALL.to_vec(), |g| vec![g]);
        let defaults = Bindings::default();
        let pairs: Vec<(&str, Value)> =
            gestures.iter().map(|g| (g.key(), Value::String(defaults.action(*g).to_owned()))).collect();
        self.s.config.write_user(section_table("keys", pairs))?;
        Ok(gestures)
    }

    /// Runs an action now, as the key would, and answers its message.
    pub fn test_action(&self, action: &str) -> ApiResult<String> {
        self.run_action(action)
    }

    /// The timing as configured now.
    pub fn key_timing(&self) -> ApiResult<KeyTiming> {
        let k = self.key_settings()?.bindings;
        Ok(KeyTiming { long_ms: k.long_ms, multi_ms: k.multi_ms })
    }

    /// Changes the timing (`None` leaves a value alone) within the allowed range, and answers the
    /// timing now configured.
    pub fn set_key_timing(&self, long_ms: Option<i64>, multi_ms: Option<i64>) -> ApiResult<KeyTiming> {
        let mut pairs: Vec<(&str, Value)> = Vec::new();
        if let Some(ms) = long_ms {
            if !LONG_MS_RANGE.contains(&ms) {
                return Err(ApiError::invalid(format!(
                    "long_ms is {}..{}",
                    LONG_MS_RANGE.start(),
                    LONG_MS_RANGE.end()
                )));
            }
            pairs.push(("long_ms", Value::Integer(ms)));
        }
        if let Some(ms) = multi_ms {
            if !MULTI_MS_RANGE.contains(&ms) {
                return Err(ApiError::invalid(format!(
                    "multi_ms is {}..{}",
                    MULTI_MS_RANGE.start(),
                    MULTI_MS_RANGE.end()
                )));
            }
            pairs.push(("multi_ms", Value::Integer(ms)));
        }
        if !pairs.is_empty() {
            self.s.config.write_user(section_table("keys", pairs))?;
        }
        self.key_timing()
    }

    /// Lists the key binding profiles with the one in use starred.
    pub fn key_profiles(&self) -> ApiResult<NamedList> {
        let current = keybinds::from_config(&self.load_config()?).profile;
        let mut list =
            NamedList { names: self.s.key_profiles.names(), current: Some(current), ..NamedList::default() };
        for n in &list.names {
            if let Some(p) = keybinds::builtin(n) {
                list.descriptions.insert(n.clone(), p.description);
            }
        }
        Ok(list)
    }

    /// The bindings in force and the profile they came from.
    pub fn key_profile_in_use(&self) -> ApiResult<(String, KeyProfileView)> {
        let active = keybinds::from_config(&self.load_config()?);
        let view = KeyProfileView { description: String::new(), bindings: (&active.bindings).into() };
        Ok((active.profile, view))
    }

    /// Reads one profile, built-in or yours.
    pub fn key_profile(&self, name: &str) -> ApiResult<KeyProfileView> {
        Ok((&self.s.key_profiles.load(name)?).into())
    }

    /// Makes a profile the active one (the key service reloads by itself).
    pub fn use_key_profile(&self, name: &str) -> ApiResult<()> {
        if name.is_empty() {
            return Err(ApiError::invalid("which profile? (truesense keys profile list)"));
        }
        self.s.key_profiles.use_profile(name)?;
        Ok(())
    }

    /// Saves a copy under `new_name` (`NAME-copy` when none) and answers the name.
    pub fn copy_key_profile(&self, name: &str, new_name: Option<&str>) -> ApiResult<String> {
        if name.is_empty() {
            return Err(ApiError::invalid("copy which profile?"));
        }
        Ok(self.s.key_profiles.copy(name, new_name)?)
    }

    /// Saves a profile: starts from `from` (or from your own profile of that name, else the
    /// defaults) and changes what the edit gives.
    pub fn save_key_profile(&self, name: &str, edit: &KeyProfileEdit) -> ApiResult<KeyProfileView> {
        if name.is_empty() {
            return Err(ApiError::invalid("save under which name?"));
        }
        let mut profile = match edit.from.as_deref().filter(|f| !f.is_empty()) {
            Some(from) => self.s.key_profiles.load(from)?,
            None if !keybinds::BUILTIN_NAMES.contains(&name)
                && self.s.key_profiles.names().iter().any(|n| n == name) =>
            {
                self.s.key_profiles.load(name)?
            }
            None => KeyProfile { description: String::new(), bindings: Bindings::default() },
        };
        for (gesture, action) in &edit.actions {
            profile.bindings.set_action(*gesture, action.clone());
        }
        if let Some(ms) = edit.long_ms {
            profile.bindings.long_ms = ms;
        }
        if let Some(ms) = edit.multi_ms {
            profile.bindings.multi_ms = ms;
        }
        Ok((&self.s.key_profiles.save(name, &profile)?).into())
    }

    /// Removes one of your profiles.
    pub fn delete_key_profile(&self, name: &str) -> ApiResult<()> {
        Ok(self.s.key_profiles.delete(name)?)
    }
}
