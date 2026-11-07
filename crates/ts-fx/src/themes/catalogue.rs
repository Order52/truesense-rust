// Adapted from predator-sense by cleyton1986 (GPL-3.0), used with the author's permission.
//! The theme catalogue as typed data: effects.js's `THEMES`, read from `catalogue.json`
//! (which gen-golden.js cuts out of effects.js byte for byte) and parsed strictly: a field
//! the types do not name fails the parse, so nothing effects.js adds is dropped unseen.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::engine::{ParamValue, Params};
use crate::layout::Role;

/// The catalogue file, exactly effects.js's THEMES block.
pub const CATALOGUE_JSON: &str = include_str!("catalogue.json");

/// A built-in theme: one look per light, its colourways, Custom slot labels and settings.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    /// The stable id (`predator`, `hearth`) saved looks refer to.
    pub id: String,
    /// English; [`Theme::text_in`] gives it in the page's language.
    pub name: String,
    /// Rank among the twelve shown first, 1 to 12; 0 for the rest of the catalogue.
    #[serde(default)]
    pub top: u8,
    /// One sentence describing the theme; English.
    pub mood: String,
    /// The built-in colourways ("Colours" on the page); the first is the default.
    pub colourways: Vec<Colourway>,
    /// What each palette slot does in the effect: the Custom colourway's slot labels.
    pub custom: Vec<String>,
    /// The colours the looks use: the first colourway's in the catalogue, the chosen ones
    /// after [`Theme::apply`].
    pub palette: Vec<String>,
    /// The keyboard's look.
    pub keyboard: Look,
    /// The light bar's look.
    pub bar: Look,
    /// The logo's look.
    pub logo: Look,
    /// Only the settings that matter for this theme.
    pub settings: Vec<SettingDef>,
    /// What it suits (power modes, activities). Not shown by the reference design, so not
    /// translated.
    #[serde(default)]
    pub suits: Vec<String>,
    /// The bar's firmware look where the bar cannot show its software look (a one-colour
    /// bar). Present exactly when the bar's look is software.
    #[serde(rename = "barFallback", default, skip_serializing_if = "Option::is_none")]
    pub bar_fallback: Option<Look>,
}

/// A named palette.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Colourway {
    /// The colourway's name, English; it is also what the translation keys are built from.
    pub name: String,
    /// `#RRGGBB` colours, one per palette slot.
    pub palette: Vec<String>,
}

/// What one light of a theme shows.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(try_from = "RawLook", into = "RawLook")]
pub enum Look {
    /// Drawn by the app: a catalogue effect and the parameters the theme sets (the rest
    /// take the effect's defaults).
    Software {
        /// The catalogue effect's id.
        effect: String,
        /// The parameters the theme sets; the rest are the effect's defaults.
        params: Params,
    },
    /// One of the light's own effects: `kb.<hw>` on the keyboard, `hw.<hw>` on the bar and
    /// the logo. `slots` holds a palette slot per zone (one on the keyboard and the logo,
    /// three on the bar). `speed` is the app's percent on the keyboard and the EC's 0..5 on
    /// the bar and logo; `dir` is Wave's direction. Absent ones take lookFor's defaults.
    Firmware {
        /// The firmware effect's name without its `kb.` or `hw.` prefix.
        hw: String,
        /// Palette slot per zone.
        slots: Vec<usize>,
        /// Speed, in the light's own unit (see above).
        speed: Option<f64>,
        /// Wave direction.
        dir: Option<f64>,
    },
}

impl Look {
    /// Whether the look is drawn by the host frame by frame, rather than by the light itself.
    pub fn is_software(&self) -> bool {
        matches!(self, Look::Software { .. })
    }
}

/// A look as the JSON writes it: `{effect, params}` or `{hw, colors, speed, dir}`.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RawLook {
    #[serde(skip_serializing_if = "Option::is_none")]
    effect: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<BTreeMap<String, RawParam>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hw: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    colors: Option<Vec<usize>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    speed: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dir: Option<f64>,
}

#[derive(Deserialize, Serialize)]
#[serde(untagged)]
enum RawParam {
    Num(f64),
    Keys(Vec<String>),
}

impl From<Look> for RawLook {
    fn from(look: Look) -> RawLook {
        match look {
            Look::Software { effect, params } => RawLook {
                effect: Some(effect),
                params: Some(
                    params
                        .into_iter()
                        .map(|(k, v)| {
                            let v = match v {
                                ParamValue::Num(x) => RawParam::Num(x),
                                ParamValue::Keys(names) => RawParam::Keys(names),
                            };
                            (k, v)
                        })
                        .collect(),
                ),
                hw: None,
                colors: None,
                speed: None,
                dir: None,
            },
            Look::Firmware { hw, slots, speed, dir } => {
                RawLook { effect: None, params: None, hw: Some(hw), colors: Some(slots), speed, dir }
            }
        }
    }
}

impl TryFrom<RawLook> for Look {
    type Error = String;

    fn try_from(r: RawLook) -> Result<Look, String> {
        match (r.effect, r.hw) {
            (Some(effect), None) => {
                if r.colors.is_some() || r.speed.is_some() || r.dir.is_some() {
                    return Err(format!(
                        "the software look {effect} has firmware fields (colors, speed, dir)"
                    ));
                }
                let params = r
                    .params
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(k, v)| {
                        let v = match v {
                            RawParam::Num(x) => ParamValue::Num(x),
                            RawParam::Keys(names) => ParamValue::Keys(names),
                        };
                        (k, v)
                    })
                    .collect();
                Ok(Look::Software { effect, params })
            }
            (None, Some(hw)) => {
                if r.params.is_some() {
                    return Err(format!("the firmware look {hw} has params"));
                }
                let slots = r.colors.ok_or_else(|| format!("the firmware look {hw} has no colors"))?;
                Ok(Look::Firmware { hw, slots, speed: r.speed, dir: r.dir })
            }
            (Some(e), Some(h)) => Err(format!("a look names both an effect ({e}) and hw ({h})")),
            (None, None) => Err("a look names neither an effect nor hw".to_string()),
        }
    }
}

/// One of a theme's settings: what the page shows and which look value it writes.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(try_from = "RawSetting", into = "RawSetting")]
pub struct SettingDef {
    /// The id the setting is saved under.
    pub id: String,
    /// English; [`Theme::text_in`] gives it in the page's language.
    pub label: String,
    /// Which look value it writes.
    pub target: Target,
    /// What control it is.
    pub kind: SettingKind,
}

/// What kind of control a setting is, and the range or options it takes.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingKind {
    /// A slider. applySettings writes the value as given; the page keeps it in range. In
    /// unit "h" (the catalogue's "time" type) it is hours of the day, shown as a clock time.
    Number {
        /// The lowest value.
        min: f64,
        /// The highest value.
        max: f64,
        /// The slider's step.
        step: f64,
        /// What the number counts (`"%"`, `"s"`, `"h"`, ...), for the label.
        unit: String,
    },
    /// Options (English labels) and the value each one writes.
    Choice {
        /// The option labels, English.
        options: Vec<String>,
        /// The value each option writes, in the same order.
        values: Vec<f64>,
    },
    /// On or off, writing `on` or `off`.
    Toggle {
        /// The value written when on.
        on: f64,
        /// The value written when off.
        off: f64,
    },
    /// A set of keys picked on the keyboard map, by layout name.
    Keys,
    /// One key picked on the keyboard map, stored as a one-name set.
    Key,
}

/// The look value a setting writes: `<light>.speed`, `<light>.dir` or
/// `<light>.params.<id>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The light whose look is written.
    pub light: Role,
    /// What is written.
    pub field: Field,
}

/// Which value of a look a setting writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Field {
    /// A firmware look's speed.
    Speed,
    /// A firmware look's direction.
    Dir,
    /// A software look's parameter.
    Param(String),
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let light = match self.light {
            Role::Keyboard => "keyboard",
            Role::Bar => "bar",
            Role::Logo => "logo",
        };
        match &self.field {
            Field::Speed => write!(f, "{light}.speed"),
            Field::Dir => write!(f, "{light}.dir"),
            Field::Param(id) => write!(f, "{light}.params.{id}"),
        }
    }
}

impl std::str::FromStr for Target {
    type Err = String;

    fn from_str(path: &str) -> Result<Target, String> {
        let parts: Vec<&str> = path.split('.').collect();
        let light = match parts[0] {
            "keyboard" => Role::Keyboard,
            "bar" => Role::Bar,
            "logo" => Role::Logo,
            _ => return Err(format!("a setting target on no light: {path}")),
        };
        let field = match parts[1..] {
            ["speed"] => Field::Speed,
            ["dir"] => Field::Dir,
            ["params", id] if !id.is_empty() => Field::Param(id.to_string()),
            _ => return Err(format!("a setting target this does not know: {path}")),
        };
        Ok(Target { light, field })
    }
}

/// A setting as the JSON writes it.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RawSetting {
    id: String,
    label: String,
    #[serde(rename = "type")]
    kind: String,
    target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    step: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    values: Option<Vec<f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    on: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    off: Option<f64>,
}

impl From<SettingDef> for RawSetting {
    /// The `time` type is written back as the `number` in hours it was read as.
    fn from(s: SettingDef) -> RawSetting {
        let mut raw = RawSetting {
            id: s.id,
            label: s.label,
            kind: String::new(),
            target: s.target.to_string(),
            min: None,
            max: None,
            step: None,
            unit: None,
            options: None,
            values: None,
            on: None,
            off: None,
        };
        match s.kind {
            SettingKind::Number { min, max, step, unit } => {
                raw.kind = "number".into();
                (raw.min, raw.max, raw.step, raw.unit) = (Some(min), Some(max), Some(step), Some(unit));
            }
            SettingKind::Choice { options, values } => {
                raw.kind = "choice".into();
                (raw.options, raw.values) = (Some(options), Some(values));
            }
            SettingKind::Toggle { on, off } => {
                raw.kind = "toggle".into();
                (raw.on, raw.off) = (Some(on), Some(off));
            }
            SettingKind::Keys => raw.kind = "keys".into(),
            SettingKind::Key => raw.kind = "key".into(),
        }
        raw
    }
}

impl TryFrom<RawSetting> for SettingDef {
    type Error = String;

    fn try_from(r: RawSetting) -> Result<SettingDef, String> {
        let target = r.target.parse()?;
        let id = &r.id;
        let number = [r.min.is_some(), r.max.is_some(), r.step.is_some(), r.unit.is_some()];
        let choice = [r.options.is_some(), r.values.is_some()];
        let toggle = [r.on.is_some(), r.off.is_some()];
        let none = |fields: &[bool]| fields.iter().all(|f| !f);
        let kind = match r.kind.as_str() {
            "number" if none(&choice) && none(&toggle) => match (r.min, r.max, r.step, r.unit) {
                (Some(min), Some(max), Some(step), Some(unit)) => {
                    SettingKind::Number { min, max, step, unit }
                }
                _ => return Err(format!("{id}: a number needs min, max, step and unit")),
            },
            // Hours of the day, set to the minute: a number in hours whose page control is a
            // clock time, so the looks still read fractional hours.
            "time" if none(&number[2..]) && none(&choice) && none(&toggle) => match (r.min, r.max) {
                (Some(min), Some(max)) => {
                    SettingKind::Number { min, max, step: 1.0 / 60.0, unit: "h".into() }
                }
                _ => return Err(format!("{id}: a time needs min and max")),
            },
            "choice" if none(&number) && none(&toggle) => {
                let options = r.options.ok_or_else(|| format!("{id}: a choice has no options"))?;
                // effects.js: `s.values || s.options.map((o, i) => i)`.
                let values = r.values.unwrap_or_else(|| (0..options.len()).map(|i| i as f64).collect());
                if values.len() != options.len() {
                    return Err(format!("{id}: {} options, {} values", options.len(), values.len()));
                }
                SettingKind::Choice { options, values }
            }
            "toggle" if none(&number) && none(&choice) => match (r.on, r.off) {
                (Some(on), Some(off)) => SettingKind::Toggle { on, off },
                _ => return Err(format!("{id}: a toggle needs on and off")),
            },
            "keys" | "key" if none(&number) && none(&choice) && none(&toggle) => {
                if r.kind == "keys" {
                    SettingKind::Keys
                } else {
                    SettingKind::Key
                }
            }
            other => return Err(format!("{id}: type {other} with fields it does not take")),
        };
        Ok(SettingDef { id: r.id, label: r.label, target, kind })
    }
}

/// Themes waiting for a flash measurement on the real hardware: each uses a firmware effect
/// whose flash rate nobody has counted yet ([`Flash::Unmeasured`]). Storm uses the
/// keyboard's Lightning; its flashes per second are to be counted on the hardware, and Storm
/// stays only if they are 3 or fewer, so it stays out of the twelve until then. The integrity
/// tests hold this list to the effects the themes use.
///
/// [`Flash::Unmeasured`]: crate::engine::Flash::Unmeasured
#[cfg(test)]
pub const PENDING_FLASH_MEASUREMENT: &[&str] = &["storm"];

#[cfg(test)]
impl Theme {
    /// Waiting for its flash measurement ([`PENDING_FLASH_MEASUREMENT`]).
    pub fn flash_pending(&self) -> bool {
        PENDING_FLASH_MEASUREMENT.contains(&self.id.as_str())
    }
}

/// Parses a catalogue (effects.js's THEMES block).
pub fn parse(json: &str) -> Result<Vec<Theme>, serde_json::Error> {
    serde_json::from_str(json)
}

/// The catalogue, in effects.js's order, parsed once.
pub fn catalogue() -> &'static [Theme] {
    static THEMES: OnceLock<Vec<Theme>> = OnceLock::new();
    // The file is compiled in and a test parses it; should it ever fail, the catalogue is
    // empty (no theme to run) rather than a panic in a service.
    THEMES.get_or_init(|| parse(CATALOGUE_JSON).unwrap_or_default())
}

/// The theme with this id.
pub fn find(id: &str) -> Option<&'static Theme> {
    catalogue().iter().find(|t| t.id == id)
}

/// The catalogue in the order the gallery lists it: the twelve by
/// rank, then the rest in catalogue order.
pub fn builtin() -> Vec<&'static Theme> {
    let mut themes: Vec<&Theme> = catalogue().iter().collect();
    themes.sort_by_key(|t| if t.top == 0 { 99 } else { t.top });
    themes
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFERENCE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tools/effects-reference.js");

    #[test]
    fn the_catalogue_is_effects_js_themes_block() {
        let js = std::fs::read_to_string(REFERENCE).unwrap_or_else(|e| panic!("{REFERENCE}: {e}"));
        let (begin, end) = ("/*THEMES-JSON*/", "/*END-THEMES-JSON*/");
        assert_eq!(js.matches(begin).count(), 1, "effects.js has one {begin}");
        assert_eq!(js.matches(end).count(), 1, "effects.js has one {end}");
        let block = &js[js.find(begin).unwrap() + begin.len()..js.find(end).unwrap()];
        assert!(
            block == CATALOGUE_JSON,
            "catalogue.json is not effects.js's THEMES block: run `gjs tools/gen-golden.js`"
        );
    }

    #[test]
    fn the_catalogue_parses_into_32_themes() {
        let themes = catalogue();
        assert_eq!(themes.len(), 32);
        assert_eq!(themes[0].id, "predator");
        assert_eq!(themes.iter().filter(|t| t.bar_fallback.is_some()).count(), 11);
        let kitt = find("kitt").unwrap();
        assert_eq!(kitt.name, "K.I.T.T.");
        assert_eq!(
            kitt.bar,
            Look::Firmware { hw: "scanner".into(), slots: vec![0, 0, 0], speed: Some(3.0), dir: None }
        );
        assert_eq!(kitt.settings[0].target, Target { light: Role::Bar, field: Field::Speed });
        let focus = find("focus").unwrap();
        let Look::Software { effect, params } = &focus.keyboard else {
            panic!("Focus draws its keyboard");
        };
        assert_eq!(effect, "tide");
        assert_eq!(
            params.get("focus"),
            Some(&ParamValue::Keys(vec!["f_key".into(), "j_key".into(), "escape".into()]))
        );
    }

    #[test]
    fn the_gallery_order_is_the_twelve_then_the_rest() {
        let ids: Vec<&str> = builtin().iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids.len(), 32);
        assert_eq!(&ids[..3], ["predator", "hearth", "synthwave"]);
        assert_eq!(ids[11], "starlight");
        // The rest keep the catalogue's order.
        assert_eq!(&ids[12..14], ["sunset", "rave"]);
        assert_eq!(ids[31], "banked-fire");
    }

    /// Wraps one theme's JSON with a change, for the strictness tests.
    fn predator_with(edit: impl FnOnce(&mut serde_json::Value)) -> Result<Vec<Theme>, String> {
        let mut all: serde_json::Value = serde_json::from_str(CATALOGUE_JSON).unwrap();
        let mut one = all.as_array_mut().unwrap()[0].take();
        edit(&mut one);
        parse(&serde_json::Value::Array(vec![one]).to_string()).map_err(|e| e.to_string())
    }

    #[test]
    fn a_field_the_types_do_not_name_fails_the_parse() {
        assert!(predator_with(|_| {}).is_ok());
        let cases: [(&str, fn(&mut serde_json::Value)); 6] = [
            ("theme", |t| t["flash"] = "pending".into()),
            ("colourway", |t| t["colourways"][0]["id"] = "x".into()),
            ("software look", |t| t["keyboard"]["speed"] = 3.into()),
            ("firmware look", |t| t["bar"]["fps"] = 10.into()),
            ("setting", |t| t["settings"][1]["default"] = 4.into()),
            ("setting of another type", |t| t["settings"][1]["on"] = 1.into()),
        ];
        for (what, edit) in cases {
            assert!(predator_with(edit).is_err(), "an unknown field in a {what} parsed");
        }
    }

    #[test]
    fn a_target_must_name_a_light_and_a_field() {
        assert_eq!(
            "bar.params.bpm".parse::<Target>(),
            Ok(Target { light: Role::Bar, field: Field::Param("bpm".into()) })
        );
        for bad in ["keys.speed", "keyboard", "keyboard.params", "keyboard.colors", "bar.params.a.b"] {
            assert!(bad.parse::<Target>().is_err(), "{bad}");
        }
    }
}
