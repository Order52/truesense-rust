// Adapted from predator-sense by cleyton1986 (GPL-3.0), used with the author's permission.
// (The checks follow the catalogue's own integrity tests; this file is TrueSense's.)
//! Checking a theme that did not come from the built-in catalogue.
//!
//! The catalogue is held to its rules by tests (`integrity.rs`). A theme a user wrote has no
//! such test, so the same rules run at load time: every effect it names exists, every value is
//! one its effect takes, every colour is `#RRGGBB`, every slot and key is real. A theme that
//! breaks one is refused whole, never half-applied, and one that uses a firmware effect whose
//! flash rate nobody has measured is refused too (the engine's flash guard would hold it, but
//! a theme should not rely on that).

use super::catalogue::{Field, Look, SettingKind, Theme};
use crate::engine::{Flash, ParamSpec, ParamValue};
use crate::layout::{Role, ph16_71};

/// The longest id or name a theme may have; files and menus have to hold them.
pub const MAX_NAME: usize = 60;

/// Whether `id` is usable as a theme id and a file name: lower-case letters, digits, `-` and
/// `_`, starting with a letter or digit, up to [`MAX_NAME`] bytes. Nothing that could leave the
/// themes directory (`/`, `.`, `..`) passes.
pub fn is_valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= MAX_NAME
        && b[0].is_ascii_alphanumeric()
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-' || *c == b'_')
}

fn is_colour(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 7 && b[0] == b'#' && b[1..].iter().all(u8::is_ascii_hexdigit)
}

fn whole(x: f64) -> bool {
    x.is_finite() && x.fract() == 0.0
}

impl Theme {
    /// Every rule this theme breaks, each as one sentence; empty when it is usable. It does not
    /// look at the id's uniqueness (that is the store's business) but does check its spelling.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut bad = |ok: bool, what: String| {
            if !ok {
                out.push(what);
            }
        };
        bad(is_valid_id(&self.id), format!("id {:?}: use lower-case letters, digits, - and _", self.id));
        bad(
            !self.name.trim().is_empty() && self.name.len() <= MAX_NAME,
            format!("name must be 1 to {MAX_NAME} characters"),
        );
        bad(self.top == 0, "top: only built-in themes are ranked".to_string());
        let n = self.palette.len();
        bad((1..=6).contains(&n), format!("{n} colours in the palette (an effect takes 1 to 6)"));
        bad(!self.colourways.is_empty() && self.colourways.len() <= 12, "colourways: 1 to 12".to_string());
        bad(
            self.colourways.first().is_some_and(|c| c.palette == self.palette),
            "the palette must be the first colourway's".to_string(),
        );
        bad(self.custom.len() == n, format!("{} Custom labels for {n} colours", self.custom.len()));
        for c in &self.colourways {
            bad(c.palette.len() == n, format!("colourway {}: {} colours, not {n}", c.name, c.palette.len()));
            for h in &c.palette {
                bad(is_colour(h), format!("colourway {}: {h} is not #RRGGBB", c.name));
            }
        }
        bad(
            self.bar_fallback.is_some() == self.bar.is_software(),
            "barFallback goes with a software bar and only then".to_string(),
        );
        let mut looks = vec![
            ("keyboard", Role::Keyboard, &self.keyboard),
            ("bar", Role::Bar, &self.bar),
            ("logo", Role::Logo, &self.logo),
        ];
        if let Some(f) = &self.bar_fallback {
            looks.push(("barFallback", Role::Bar, f));
        }
        for (name, light, look) in looks {
            look_problems(name, light, look, n, &mut out);
        }
        self.setting_problems(&mut out);
        out
    }

    fn setting_problems(&self, out: &mut Vec<String>) {
        let mut seen: Vec<&str> = Vec::new();
        for s in &self.settings {
            let at = format!("setting {}", s.id);
            if seen.contains(&s.id.as_str()) {
                out.push(format!("{at}: id used twice"));
            }
            seen.push(&s.id);
            if s.label.trim().is_empty() {
                out.push(format!("{at}: no label"));
            }
            let look = self.look(s.target.light);
            let spec = match (look, &s.target.field) {
                (Look::Software { effect, .. }, Field::Param(id)) => {
                    crate::find(effect).and_then(|d| d.params.iter().find(|p| p.id == id)).map(|p| p.spec)
                }
                (Look::Firmware { .. }, Field::Speed | Field::Dir) => None,
                _ => {
                    out.push(format!("{at}: the target does not fit that light's look"));
                    continue;
                }
            };
            match (&s.kind, &s.target.field, spec) {
                // A firmware speed or direction: whole values only (a fractional EC speed draws black).
                (kind, field @ (Field::Speed | Field::Dir), _) => {
                    // The keyboard's speed is the app's percent, which may have a fraction.
                    let whole_only = *field == Field::Dir || s.target.light != Role::Keyboard;
                    let ok = !whole_only
                        || match kind {
                            SettingKind::Number { min, max, step, .. } => {
                                whole(*min) && whole(*max) && *step == 1.0
                            }
                            SettingKind::Choice { values, .. } => values.iter().all(|v| whole(*v)),
                            SettingKind::Toggle { on, off } => whole(*on) && whole(*off),
                            SettingKind::Keys | SettingKind::Key => false,
                        };
                    if !ok {
                        out.push(format!("{at}: a speed or direction takes whole numbers"));
                    }
                }
                (
                    SettingKind::Number { min, max, step, .. },
                    _,
                    Some(ParamSpec::Num { min: lo, max: hi, .. }),
                ) => {
                    if !(min < max && *step > 0.0 && lo <= *min && *max <= hi) {
                        out.push(format!("{at}: range {min}..{max} is not inside the effect's {lo}..{hi}"));
                    }
                }
                (
                    SettingKind::Choice { options, values },
                    _,
                    Some(ParamSpec::Num { min: lo, max: hi, .. }),
                ) => {
                    if options.is_empty()
                        || options.len() != values.len()
                        || !values.iter().all(|v| (lo..=hi).contains(v))
                    {
                        out.push(format!("{at}: choices outside the effect's {lo}..{hi}"));
                    }
                }
                (
                    SettingKind::Choice { options, values },
                    _,
                    Some(ParamSpec::Choice { options: theirs, .. }),
                ) => {
                    let ok = !options.is_empty()
                        && options.len() == values.len()
                        && values.iter().all(|v| whole(*v) && *v >= 0.0 && (*v as usize) < theirs.len());
                    if !ok {
                        out.push(format!("{at}: choices past the effect's {} options", theirs.len()));
                    }
                }
                (SettingKind::Toggle { on, off }, _, Some(ParamSpec::Num { min: lo, max: hi, .. })) => {
                    if on == off || !(lo..=hi).contains(on) || !(lo..=hi).contains(off) {
                        out.push(format!("{at}: on and off must differ and lie in {lo}..{hi}"));
                    }
                }
                (SettingKind::Toggle { on, off }, _, Some(ParamSpec::Choice { options, .. })) => {
                    let option = |v: f64| whole(v) && v >= 0.0 && (v as usize) < options.len();
                    if on == off || !option(*on) || !option(*off) {
                        out.push(format!("{at}: on and off must be two of the effect's options"));
                    }
                }
                (SettingKind::Keys | SettingKind::Key, _, Some(ParamSpec::Keys { .. })) => {}
                _ => out.push(format!("{at}: this kind of control does not fit the effect's parameter")),
            }
        }
    }
}

fn look_problems(name: &str, light: Role, look: &Look, palette: usize, out: &mut Vec<String>) {
    let at = format!("{name} look");
    let keys = ph16_71();
    match look {
        Look::Software { effect, params } => {
            let Some(def) = crate::find(effect).filter(|d| !d.hardware) else {
                out.push(format!("{at}: {effect:?} is not an effect"));
                return;
            };
            if def.flash == Flash::Unmeasured {
                out.push(format!("{at}: {effect} has no flash measurement"));
            }
            for (id, v) in params {
                let Some(spec) = def.params.iter().find(|q| q.id == id).map(|q| q.spec) else {
                    out.push(format!("{at}: {effect} has no parameter {id}"));
                    continue;
                };
                let ok = match (spec, v) {
                    (ParamSpec::Num { min, max, .. }, ParamValue::Num(x)) => (min..=max).contains(x),
                    (ParamSpec::Choice { options, .. }, ParamValue::Num(x)) => {
                        whole(*x) && *x >= 0.0 && (*x as usize) < options.len()
                    }
                    (ParamSpec::Keys { .. }, ParamValue::Keys(names)) => {
                        names.iter().all(|k| keys.iter().any(|key| key.name == *k))
                    }
                    _ => false,
                };
                if !ok {
                    out.push(format!("{at}: {id} = {v:?} is not a value {effect} takes"));
                }
            }
        }
        Look::Firmware { hw, slots, speed, dir } => {
            let keyboard = light == Role::Keyboard;
            let id = format!("{}{hw}", if keyboard { "kb." } else { "hw." });
            let Some(def) = crate::find(&id).filter(|d| d.hardware) else {
                out.push(format!("{at}: {id} is not a firmware effect"));
                return;
            };
            if def.flash == Flash::Unmeasured {
                out.push(format!("{at}: {id} has no flash measurement"));
            }
            let zones = if light == Role::Bar { 3 } else { 1 };
            if slots.len() != zones || !slots.iter().all(|s| *s < palette) {
                out.push(format!("{at}: needs {zones} colour slots, each below {palette}"));
            }
            if keyboard {
                if !speed.is_none_or(|s| (0.0..=100.0).contains(&s)) {
                    out.push(format!("{at}: speed {speed:?} is not a percent"));
                }
                if !dir.is_none_or(|d| hw == "wave" && whole(d) && (1.0..=6.0).contains(&d)) {
                    out.push(format!("{at}: direction {dir:?} is not 1 to 6 on a wave"));
                }
            } else {
                if !speed.is_none_or(|s| whole(s) && (0.0..=5.0).contains(&s)) {
                    out.push(format!("{at}: speed {speed:?} is not a whole 0 to 5"));
                }
                if !dir.is_none_or(|d| (hw == "wave" || hw == "rainbow") && (d == 1.0 || d == 2.0)) {
                    out.push(format!("{at}: direction {dir:?} is not 1 or 2 on a wave or rainbow"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::themes::catalogue::{catalogue, find, parse};

    /// The catalogue's theme `id` as JSON, to edit.
    fn json_of(id: &str) -> serde_json::Value {
        serde_json::to_value(find(id).unwrap()).unwrap()
    }

    fn problems_of(v: &serde_json::Value) -> Vec<String> {
        let themes = parse(&serde_json::Value::Array(vec![v.clone()]).to_string()).unwrap();
        themes[0].problems()
    }

    #[test]
    fn every_built_in_theme_but_the_one_pending_its_flash_measurement_is_clean() {
        for t in catalogue() {
            let mut own = t.clone();
            // Built-ins are ranked; a user's copy is not.
            own.top = 0;
            let problems = own.problems();
            if t.id == "storm" {
                assert!(problems.iter().any(|p| p.contains("no flash measurement")), "{problems:?}");
            } else {
                assert!(problems.is_empty(), "{}: {problems:?}", t.id);
            }
        }
    }

    #[test]
    fn a_theme_written_back_as_json_reads_back_the_same() {
        for t in catalogue() {
            let text = serde_json::to_string_pretty(t).unwrap();
            let back: Theme = serde_json::from_str(&text).unwrap();
            assert_eq!(&back, t, "{}", t.id);
        }
    }

    #[test]
    fn ids_are_file_names_that_stay_in_the_directory() {
        for ok in ["a", "my-theme", "x_1", "0day", &"a".repeat(MAX_NAME)] {
            assert!(is_valid_id(ok), "{ok}");
        }
        for bad in
            ["", "-a", "_a", "A", "a b", "a/b", "..", ".", "a.json", "../x", "é", &"a".repeat(MAX_NAME + 1)]
        {
            assert!(!is_valid_id(bad), "{bad}");
        }
    }

    #[test]
    fn a_broken_theme_is_refused_with_the_reason() {
        let cases: [(&str, fn(&mut serde_json::Value), &str); 9] = [
            ("id", |t| t["id"] = "Bad Id".into(), "id"),
            ("name", |t| t["name"] = "".into(), "name"),
            ("effect", |t| t["keyboard"]["effect"] = "nope".into(), "not an effect"),
            ("param", |t| t["keyboard"]["params"]["nope"] = 1.into(), "no parameter"),
            ("range", |t| t["keyboard"]["params"]["speed"] = 100000.into(), "not a value"),
            ("colour", |t| t["colourways"][0]["palette"][0] = "red".into(), "not #RRGGBB"),
            ("slots", |t| t["palette"] = serde_json::json!([]), "colours in the palette"),
            ("ranked", |t| t["top"] = 3.into(), "ranked"),
            ("setting", |t| t["settings"][0]["target"] = "keyboard.params.nope".into(), "does not fit"),
        ];
        for (what, edit, expect) in cases {
            let mut v = json_of("hearth");
            v["top"] = 0.into();
            edit(&mut v);
            let problems = problems_of(&v);
            assert!(problems.iter().any(|p| p.contains(expect)), "{what}: {problems:?}");
        }
    }

    #[test]
    fn a_firmware_look_must_name_a_firmware_effect_with_whole_speeds() {
        let mut v = json_of("kitt");
        v["top"] = 0.into();
        assert!(problems_of(&v).is_empty());
        v["bar"]["speed"] = 2.5.into();
        assert!(problems_of(&v).iter().any(|p| p.contains("whole")));
        v["bar"]["speed"] = 3.into();
        v["bar"]["hw"] = "nope".into();
        assert!(problems_of(&v).iter().any(|p| p.contains("not a firmware effect")));
    }

    #[test]
    fn a_user_theme_may_leave_out_rank_and_suits() {
        let mut v = json_of("hearth");
        v.as_object_mut().unwrap().remove("top");
        v.as_object_mut().unwrap().remove("suits");
        assert!(problems_of(&v).is_empty());
    }

    #[test]
    fn a_theme_runs_on_the_keyboard() {
        let theme = find("hearth").unwrap();
        let mut fx = theme.keyboard_effect(&Default::default()).unwrap();
        let frame = fx.frame(0.0, 0.0, &Default::default());
        assert_eq!(frame.len(), 103);
    }

    /// The flash limit, measured on what a host would send: every runnable theme on the keyboard,
    /// sampled at 15 frames a second for twelve seconds, never changes brightness by a flash more
    /// than three times in any second (WCAG 2.3.1). The engine's guard enforces it; this holds the
    /// whole catalogue to it through the public path a host uses.
    #[test]
    fn no_runnable_theme_flashes_more_than_three_times_a_second_on_the_keyboard() {
        use crate::guard::FlashMeter;
        let layout = crate::layout::ph16_71();
        for theme in catalogue().iter().filter(|t| t.id != "storm") {
            let mut fx = theme.keyboard_effect(&Default::default()).unwrap();
            let mut meter = FlashMeter::default();
            let dt = 1.0 / 15.0;
            for i in 0..180 {
                let t = f64::from(i) * dt;
                let frame = fx.frame(t, dt, &Default::default()).to_vec();
                meter.push_frame(t, &frame, Some(layout));
            }
            assert!(meter.max() <= 3.0, "{}: {} flashes a second", theme.id, meter.max());
        }
    }
}
