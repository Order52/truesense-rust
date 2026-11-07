//! The theme part of a look: which theme, with which of the user's choices.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_fx::layout::ph16_71;
use ts_fx::themes::{ColourwayChoice, SettingKind, SettingValue, Theme, Values, is_valid_id};

use super::{ThemesError, invalid};

/// The slowest a theme may be played, as a percent of its designed speed.
pub const SPEED_MIN: i64 = 10;
/// The fastest: the designed speed. A theme is only ever slowed down, never sped up, because its
/// flash limit (3 a second) is counted in the theme's own time; playing it faster would count
/// flashes at a rate the guard did not measure.
pub const SPEED_MAX: i64 = 100;
/// The speed of a theme that names none.
pub const DEFAULT_SPEED: i64 = 100;
/// The dimmest a theme may be drawn; 0 would be an upload into darkness (use `keyboard off`).
pub const BRIGHTNESS_MIN: i64 = 1;
/// The brightest, in the percent the keyboard's brightness command uses.
pub const BRIGHTNESS_MAX: i64 = 100;

/// Which colours of the theme: one of its built-in colourways by position, or the user's own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ColourwayPick {
    /// A built-in colourway, counting from 0.
    Index(u32),
    /// The word `custom`: the colours in [`ThemeSpec::custom`].
    Custom(CustomWord),
}

/// The one word that picks the custom colours; a type so that any other string is refused when a
/// look is read, not at apply time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CustomWord {
    /// `"custom"`.
    #[serde(rename = "custom")]
    Custom,
}

impl ColourwayPick {
    /// The custom colours.
    pub const CUSTOM: ColourwayPick = ColourwayPick::Custom(CustomWord::Custom);
}

/// A theme and the user's choices for it, as a look carries them (`"theme": {...}`).
///
/// Everything but the name is optional; what is left out is the theme's own design. Settings
/// (`params`) are keyed by the theme's setting ids (`truesense theme show NAME` lists them) and
/// hold JSON values: a number, `true` or `false`, a key name, or a list of key names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeSpec {
    /// The theme: a built-in id or one of the user's.
    pub name: String,
    /// Which colourway; absent means the theme's first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colourway: Option<ColourwayPick>,
    /// The user's own colours, `#RRGGBB`, when `colourway` is `custom`; a slot left out keeps the
    /// first colourway's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom: Vec<String>,
    /// The theme's settings by id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, Value>,
    /// Percent of the designed speed, [`SPEED_MIN`] to [`SPEED_MAX`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<i64>,
    /// The keyboard brightness while the theme runs, percent; absent follows the keyboard's own
    /// level (Fn+F11 and Fn+F12).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness: Option<i64>,
}

impl ThemeSpec {
    /// The theme as designed, with no choices of the user's.
    pub fn new(name: impl Into<String>) -> ThemeSpec {
        ThemeSpec {
            name: name.into(),
            colourway: None,
            custom: Vec::new(),
            params: BTreeMap::new(),
            speed: None,
            brightness: None,
        }
    }

    /// What can be checked without the theme: the name's spelling and the two percentages. A look
    /// is checked this way when it is saved; the rest is checked when it is applied, because the
    /// theme it names may be one the user adds or removes meanwhile.
    pub fn check_shape(&self) -> Result<(), ThemesError> {
        if !is_valid_id(&self.name) {
            return Err(invalid(format!(
                "theme '{}': use lower-case letters, digits, - and _ (max {})",
                self.name,
                ts_fx::themes::MAX_NAME
            )));
        }
        if let Some(s) = self.speed.filter(|s| !(SPEED_MIN..=SPEED_MAX).contains(s)) {
            return Err(invalid(format!("theme speed {s}: {SPEED_MIN} to {SPEED_MAX} percent")));
        }
        if let Some(b) = self.brightness.filter(|b| !(BRIGHTNESS_MIN..=BRIGHTNESS_MAX).contains(b)) {
            return Err(invalid(format!(
                "theme brightness {b}: {BRIGHTNESS_MIN} to {BRIGHTNESS_MAX} percent"
            )));
        }
        Ok(())
    }

    /// The speed as a factor of the designed time (1.0 is as designed).
    pub fn speed_factor(&self) -> f64 {
        self.speed.unwrap_or(DEFAULT_SPEED).clamp(SPEED_MIN, SPEED_MAX) as f64 / 100.0
    }

    /// The choices as the engine takes them, checked against `theme`: the colourway exists, the
    /// custom colours are colours, every setting is one the theme has and its value one the
    /// setting takes (a number inside its range, a choice that exists, keys that are keys).
    pub fn values(&self, theme: &Theme) -> Result<Values, ThemesError> {
        self.check_shape()?;
        let at = |what: String| invalid(format!("theme {}: {what}", self.name));
        let colourway = match &self.colourway {
            None => ColourwayChoice::Builtin(0),
            Some(ColourwayPick::Index(i)) => {
                let i = *i as usize;
                if i >= theme.colourways.len() {
                    return Err(at(format!("colourway {i}: it has {}", theme.colourways.len())));
                }
                ColourwayChoice::Builtin(i)
            }
            Some(ColourwayPick::Custom(_)) => ColourwayChoice::Custom,
        };
        if self.custom.len() > theme.palette.len() {
            return Err(at(format!(
                "{} custom colours, it has {} slots",
                self.custom.len(),
                theme.palette.len()
            )));
        }
        for c in &self.custom {
            let b = c.as_bytes();
            if b.len() != 7 || b[0] != b'#' || !b[1..].iter().all(u8::is_ascii_hexdigit) {
                return Err(at(format!("custom colour '{c}': use #RRGGBB")));
            }
        }
        let mut settings = BTreeMap::new();
        for (id, value) in &self.params {
            let Some(def) = theme.settings.iter().find(|s| s.id == *id) else {
                let known: Vec<&str> = theme.settings.iter().map(|s| s.id.as_str()).collect();
                return Err(at(format!("no setting '{id}' (it has: {})", known.join(", "))));
            };
            let typed: SettingValue = serde_json::from_value(value.clone()).map_err(|_| {
                at(format!("setting '{id}': {value} is not a number, true/false or key names"))
            })?;
            check_setting(&def.kind, &typed).map_err(|why| at(format!("setting '{id}': {why}")))?;
            settings.insert(id.clone(), typed);
        }
        Ok(Values { colourway, custom: (!self.custom.is_empty()).then(|| self.custom.clone()), settings })
    }
}

/// Whether `value` is something setting `kind` takes.
fn check_setting(kind: &SettingKind, value: &SettingValue) -> Result<(), String> {
    let keys_exist = |names: &[String]| names.iter().all(|n| ph16_71().iter().any(|k| k.name == *n));
    match (kind, value) {
        (SettingKind::Number { min, max, .. }, SettingValue::Num(x)) => {
            if x.is_finite() && (min..=max).contains(&x) {
                Ok(())
            } else {
                Err(format!("{x} is outside {min} to {max}"))
            }
        }
        (SettingKind::Choice { options, .. }, SettingValue::Num(x)) => {
            if x.fract() == 0.0 && *x >= 0.0 && (*x as usize) < options.len() {
                Ok(())
            } else {
                Err(format!("{x} is not one of 0 to {}", options.len().saturating_sub(1)))
            }
        }
        (SettingKind::Toggle { .. }, SettingValue::Bool(_) | SettingValue::Num(_)) => Ok(()),
        (SettingKind::Keys, SettingValue::Keys(names)) => {
            if keys_exist(names) {
                Ok(())
            } else {
                Err("a name is not a key of this keyboard".to_owned())
            }
        }
        (SettingKind::Key, SettingValue::Keys(names)) if names.len() == 1 => {
            if keys_exist(names) {
                Ok(())
            } else {
                Err("not a key of this keyboard".to_owned())
            }
        }
        (SettingKind::Key, SettingValue::Text(name)) => {
            if keys_exist(std::slice::from_ref(name)) {
                Ok(())
            } else {
                Err("not a key of this keyboard".to_owned())
            }
        }
        _ => Err("the wrong kind of value for this setting".to_owned()),
    }
}
