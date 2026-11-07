#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use serde_json::{Value, json};
use ts_foundation::userstate::Store;
use ts_fx::themes::Theme;
use ts_platform::hw::{HwError, Rear as HwRear, RearSettings};

use super::*;
use crate::keyboard::{self, KeyboardError};
use crate::keymap::testutil::TempDir;
use crate::looks::{KeyboardControl, Look, LookStore, RearControl};

struct Rig {
    dir: TempDir,
    store: ThemeStore,
    state: Arc<Store>,
}

fn rig() -> Rig {
    let dir = TempDir::new("themes");
    let state = Arc::new(Store::new(dir.join("user.json")));
    let store = ThemeStore::beside(Some(&dir.join("config.toml")), Arc::clone(&state));
    Rig { dir, store, state }
}

/// A user theme of the catalogue's schema: Hearth under another id.
fn mine(id: &str) -> Theme {
    let mut v = serde_json::to_value(ts_fx::themes::find("hearth").unwrap()).unwrap();
    v["id"] = id.into();
    v["name"] = "Mine".into();
    v["top"] = 0.into();
    serde_json::from_value(v).unwrap()
}

// ---- the list ----------------------------------------------------------------------------------

#[test]
fn the_built_in_themes_come_first_in_the_gallery_order_and_the_users_follow() {
    let r = rig();
    let listed = r.store.list();
    assert_eq!(listed.themes.len(), 32);
    assert_eq!(listed.themes[0].id, "predator");
    assert!(listed.themes.iter().all(|t| t.builtin));
    assert!(listed.skipped.is_empty());
    r.store.save(&mine("zeta")).unwrap();
    r.store.save(&mine("alpha")).unwrap();
    let ids: Vec<String> = r.store.list().themes.into_iter().map(|t| t.id).collect();
    assert_eq!(ids.len(), 34);
    assert_eq!(&ids[32..], ["alpha", "zeta"]);
    let user: Vec<bool> = r.store.list().themes.iter().skip(32).map(|t| t.builtin).collect();
    assert_eq!(user, [false, false]);
}

#[test]
fn a_theme_without_a_flash_measurement_is_listed_but_not_runnable() {
    let r = rig();
    let listed = r.store.list();
    let storm = listed.themes.iter().find(|t| t.id == "storm").unwrap();
    assert!(!storm.runnable);
    assert!(listed.themes.iter().filter(|t| t.id != "storm").all(|t| t.runnable));
    let err = r.store.resolve(&ThemeSpec::new("storm")).unwrap_err().to_string();
    assert!(err.contains("flashing has not been measured"), "{err}");
}

#[test]
fn a_broken_user_file_is_skipped_with_its_reason_and_the_rest_still_list() {
    let r = rig();
    r.store.save(&mine("good")).unwrap();
    let dir = r.dir.join("themes");
    std::fs::write(dir.join("junk.json"), "{not json").unwrap();
    // Valid JSON of the schema, but the id and the file name disagree.
    let mut other = serde_json::to_value(mine("other")).unwrap();
    other["id"] = "elsewhere".into();
    std::fs::write(dir.join("other.json"), other.to_string()).unwrap();
    // A field the schema does not have.
    let mut extra = serde_json::to_value(mine("extra")).unwrap();
    extra["code"] = "rm".into();
    std::fs::write(dir.join("extra.json"), extra.to_string()).unwrap();
    // Named like a built-in theme.
    std::fs::write(dir.join("hearth.json"), serde_json::to_string(&mine("hearth")).unwrap()).unwrap();
    // A theme that breaks a rule.
    let mut bad = serde_json::to_value(mine("bad")).unwrap();
    bad["keyboard"]["effect"] = "nope".into();
    std::fs::write(dir.join("bad.json"), bad.to_string()).unwrap();
    // Not a theme file at all.
    std::fs::write(dir.join("notes.txt"), "hello").unwrap();

    let listed = r.store.list();
    let user: Vec<&str> = listed.themes.iter().filter(|t| !t.builtin).map(|t| t.id.as_str()).collect();
    assert_eq!(user, ["good"]);
    let files: Vec<&str> = listed.skipped.iter().map(|s| s.file.as_str()).collect();
    assert_eq!(files, ["bad.json", "extra.json", "hearth.json", "junk.json", "other.json"]);
    assert!(listed.skipped[0].reason.contains("not an effect"), "{:?}", listed.skipped[0]);
    assert!(listed.skipped[2].reason.contains("built-in"));
    assert!(listed.skipped[4].reason.contains("holds the theme 'elsewhere'"));
}

// ---- save, show, copy, delete ------------------------------------------------------------------

#[test]
fn a_saved_theme_reads_back_and_is_the_catalogues_schema() {
    let r = rig();
    r.store.save(&mine("calm")).unwrap();
    assert_eq!(r.store.get("calm").unwrap(), mine("calm"));
    let text = std::fs::read_to_string(r.dir.join("themes/calm.json")).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["id"], "calm");
    assert!(v["keyboard"].is_object() && v["colourways"].is_array());
    assert_eq!(r.store.get("hearth").unwrap().id, "hearth");
}

#[test]
fn built_in_themes_are_read_only() {
    let r = rig();
    let hearth = ts_fx::themes::find("hearth").unwrap().clone();
    let e = r.store.save(&hearth).unwrap_err().to_string();
    assert!(e.contains("built-in") && e.contains("read-only"), "{e}");
    let e = r.store.delete("hearth").unwrap_err().to_string();
    assert!(e.contains("built-in"), "{e}");
    assert!(!r.dir.join("themes").exists(), "nothing was written");
}

#[test]
fn a_theme_that_breaks_a_rule_is_not_saved() {
    let r = rig();
    let mut t = mine("bad");
    t.palette = vec![];
    let e = r.store.save(&t).unwrap_err().to_string();
    assert!(e.contains("theme 'bad'"), "{e}");
    for bad_id in ["../evil", "A", "a b", "", "a/b", "..", "x.json"] {
        let mut t = mine("ok");
        t.id = bad_id.into();
        assert!(r.store.save(&t).is_err(), "{bad_id:?}");
    }
    assert!(!r.dir.join("themes").exists() || std::fs::read_dir(r.dir.join("themes")).unwrap().count() == 0);
    assert!(!r.dir.join("evil.json").exists());
}

#[test]
fn save_json_takes_the_schema_and_refuses_anything_else() {
    let r = rig();
    let t = r.store.save_json(&serde_json::to_string(&mine("fromjson")).unwrap()).unwrap();
    assert_eq!(t.id, "fromjson");
    assert!(r.store.save_json("[1,2]").is_err());
    assert!(r.store.save_json(r#"{"id":"x"}"#).is_err());
    let mut v = serde_json::to_value(mine("sneaky")).unwrap();
    v["script"] = "echo".into();
    assert!(r.store.save_json(&v.to_string()).is_err(), "unknown fields are refused, never dropped");
}

#[test]
fn copy_makes_a_user_theme_from_any_theme_and_numbers_the_name_until_free() {
    let r = rig();
    assert_eq!(r.store.copy("hearth", None).unwrap(), "hearth-copy");
    assert_eq!(r.store.copy("hearth", None).unwrap(), "hearth-copy-2");
    assert_eq!(r.store.copy("hearth-copy", Some("mine")).unwrap(), "mine");
    // The built-in names are taken as well, so a copy never shadows one.
    assert_eq!(r.store.copy("hearth", Some("predator")).unwrap(), "predator-2");
    let copy = r.store.get("hearth-copy").unwrap();
    assert_eq!(copy.top, 0);
    assert!(copy.name.ends_with("(copy)"));
    assert!(r.store.copy("nothing", None).is_err());
    // A theme that cannot run for want of a flash measurement cannot be saved as one's own either.
    assert!(r.store.copy("storm", None).is_err());
}

#[test]
fn delete_removes_a_user_theme_and_says_so_for_an_unknown_one() {
    let r = rig();
    r.store.save(&mine("gone")).unwrap();
    r.store.delete("gone").unwrap();
    assert!(r.store.get("gone").is_err());
    assert!(r.store.delete("gone").unwrap_err().to_string().contains("no theme 'gone'"));
    assert!(r.store.delete("../x").is_err());
}

#[test]
fn without_a_home_there_are_only_the_built_in_themes() {
    let dir = TempDir::new("themes-nohome");
    let store = ThemeStore::beside(None, Arc::new(Store::new(dir.join("user.json"))));
    assert_eq!(store.list().themes.len(), 32);
    assert!(matches!(store.save(&mine("x")), Err(ThemesError::NoLocation)));
    assert!(store.get("hearth").is_ok());
}

// ---- the choices -------------------------------------------------------------------------------

#[test]
fn a_spec_with_no_choices_is_the_theme_as_designed() {
    let r = rig();
    let resolved = r.store.resolve(&ThemeSpec::new("hearth")).unwrap();
    assert_eq!(resolved.speed, 1.0);
    assert_eq!(resolved.brightness, None);
    assert_eq!(resolved.values, ts_fx::themes::Values::default());
    let mut effect = resolved.effect().unwrap();
    assert_eq!(effect.frame(0.0, 0.0, &Default::default()).len(), 103);
}

#[test]
fn the_choices_are_checked_against_the_theme() {
    let r = rig();
    let hearth = r.store.get("hearth").unwrap();
    let first = hearth.settings.first().unwrap().id.clone();
    let spec = |v: Value| -> ThemeSpec { serde_json::from_value(v).unwrap() };
    let bad = [
        (json!({"name": "hearth", "colourway": 99}), "colourway 99"),
        (json!({"name": "hearth", "custom": ["red"]}), "use #RRGGBB"),
        (
            json!({"name": "hearth", "custom": ["#000000","#000000","#000000","#000000","#000000","#000000","#000000","#000000"]}),
            "custom colours",
        ),
        (json!({"name": "hearth", "params": {"nope": 1}}), "no setting 'nope'"),
        (json!({"name": "hearth", "params": {first.clone(): "text"}}), "setting"),
        (json!({"name": "hearth", "params": {first.clone(): 1.0e9}}), "outside"),
        (json!({"name": "hearth", "speed": 5}), "10 to 100"),
        (json!({"name": "hearth", "speed": 101}), "10 to 100"),
        (json!({"name": "hearth", "brightness": 0}), "1 to 100"),
        (json!({"name": "hearth", "brightness": 101}), "1 to 100"),
        (json!({"name": "no-such-theme"}), "no theme"),
        (json!({"name": "Bad Name"}), "lower-case"),
    ];
    for (v, expect) in bad {
        let e = r.store.resolve(&spec(v.clone())).unwrap_err().to_string();
        assert!(e.contains(expect), "{v}: {e}");
    }
    let good = spec(json!({"name": "hearth", "colourway": 1, "speed": 50, "brightness": 40}));
    let resolved = r.store.resolve(&good).unwrap();
    assert_eq!(resolved.speed, 0.5);
    assert_eq!(resolved.brightness, Some(40));
    let custom = spec(json!({"name": "hearth", "colourway": "custom", "custom": ["#112233"]}));
    assert!(r.store.resolve(&custom).is_ok());
}

#[test]
fn a_setting_of_each_kind_takes_its_values_and_refuses_the_rest() {
    let r = rig();
    // Take one setting of each kind from the catalogue.
    let mut seen = std::collections::HashSet::new();
    for t in ts_fx::themes::catalogue() {
        if t.id == "storm" {
            continue;
        }
        for s in &t.settings {
            let kind = std::mem::discriminant(&s.kind);
            if !seen.insert(kind) {
                continue;
            }
            let (good, bad): (Value, Value) = match &s.kind {
                ts_fx::themes::SettingKind::Number { min, max, .. } => (json!(min), json!(max + 1000.0)),
                ts_fx::themes::SettingKind::Choice { options, .. } => (json!(0), json!(options.len())),
                ts_fx::themes::SettingKind::Toggle { .. } => (json!(true), json!("yes")),
                ts_fx::themes::SettingKind::Keys => (json!(["w_key", "a_key"]), json!(["no_such_key"])),
                ts_fx::themes::SettingKind::Key => (json!("w_key"), json!(["w_key", "a_key"])),
            };
            let mk = |v: Value| ThemeSpec {
                params: [(s.id.clone(), v)].into_iter().collect(),
                ..ThemeSpec::new(&t.id)
            };
            assert!(r.store.resolve(&mk(good.clone())).is_ok(), "{} / {}: {good}", t.id, s.id);
            assert!(r.store.resolve(&mk(bad.clone())).is_err(), "{} / {}: {bad}", t.id, s.id);
        }
    }
    assert_eq!(seen.len(), 5, "the catalogue has a setting of every kind");
}

// ---- the running theme -------------------------------------------------------------------------

#[test]
fn start_records_the_theme_and_stop_forgets_it() {
    let r = rig();
    assert_eq!(r.store.active(), None);
    let spec = ThemeSpec { speed: Some(60), ..ThemeSpec::new("hearth") };
    let active = r.store.start(&spec, 1000.5).unwrap();
    assert_eq!(active.started, 1000.5);
    assert_eq!(r.store.active(), Some(active));
    // The state file is where the key service reads it.
    let v = r.state.get(ACTIVE_KEY).unwrap();
    assert_eq!(v["spec"]["name"], "hearth");
    assert_eq!(v["spec"]["speed"], 60);
    assert!(r.store.stop().unwrap());
    assert_eq!(r.store.active(), None);
    assert!(!r.store.stop().unwrap(), "nothing was running");
}

#[test]
fn a_theme_that_cannot_be_resolved_does_not_start_and_leaves_the_running_one_alone() {
    let r = rig();
    r.store.start(&ThemeSpec::new("hearth"), 1.0).unwrap();
    assert!(r.store.start(&ThemeSpec::new("no-such-theme"), 2.0).is_err());
    assert!(r.store.start(&ThemeSpec::new("storm"), 2.0).is_err());
    assert_eq!(r.store.active().unwrap().started, 1.0);
}

#[test]
fn a_halt_is_about_one_start_and_a_new_start_runs_again() {
    let r = rig();
    r.store.start(&ThemeSpec::new("hearth"), 1.0).unwrap();
    assert_eq!(r.store.halt_reason(), None);
    r.store.halt(1.0, "the keyboard went away");
    assert_eq!(r.store.halt_reason().as_deref(), Some("the keyboard went away"));
    assert!(r.store.active().is_some(), "the record of the theme is left alone");
    // Started again (even the same theme): it is a new start and the old halt does not apply.
    r.store.start(&ThemeSpec::new("hearth"), 2.0).unwrap();
    assert_eq!(r.store.halt_reason(), None);
    // A halt for a start that is long gone cannot stop the one running now.
    r.store.halt(1.0, "late");
    assert_eq!(r.store.halt_reason(), None);
    r.store.stop().unwrap();
    assert_eq!(r.store.halt_reason(), None, "nothing runs, nothing is halted");
}

// ---- Look.theme --------------------------------------------------------------------------------

#[test]
fn a_look_without_a_theme_is_written_exactly_as_before() {
    let look = Look::parse(r#"{"keyboard": {"mode": "effect", "name": "wave"}, "rear": null}"#).unwrap();
    assert_eq!(look.theme, None);
    let text = serde_json::to_string(&look).unwrap();
    assert!(!text.contains("theme"), "{text}");
}

#[test]
fn a_look_with_a_theme_round_trips_and_names_it() {
    let text = r##"{"keyboard": {"mode": "effect", "name": "static", "color": "112233"}, "rear": null,
        "theme": {"name": "hearth", "colourway": 1, "custom": ["#112233"], "params": {"x": 3}, "speed": 50, "brightness": 30}}"##;
    let look = Look::parse(text).unwrap();
    let theme = look.theme.clone().unwrap();
    assert_eq!(theme.name, "hearth");
    assert_eq!(theme.colourway, Some(ColourwayPick::Index(1)));
    assert_eq!((theme.speed, theme.brightness), (Some(50), Some(30)));
    let back: Look = serde_json::from_str(&serde_json::to_string(&look).unwrap()).unwrap();
    assert_eq!(back, look);
    assert!(look.describe().ends_with("; theme hearth"));
    assert!(!look.is_empty());
    assert!(!Look { theme: Some(ThemeSpec::new("hearth")), ..Look::default() }.is_empty());
    let custom: Look =
        serde_json::from_str(r#"{"theme": {"name": "hearth", "colourway": "custom"}}"#).unwrap();
    assert_eq!(custom.theme.unwrap().colourway, Some(ColourwayPick::CUSTOM));
    assert!(serde_json::from_str::<Look>(r#"{"theme": {"name": "hearth", "colourway": "other"}}"#).is_err());
}

/// What an older program (or a newer one, with members this one does not know) does with the
/// look file: `Look` has no `deny_unknown_fields`, so unknown members are skipped and the static
/// parts still apply. The Go and Python editions' behaviour is unverified here.
#[test]
fn a_look_with_members_this_program_does_not_know_is_still_a_look() {
    let look = Look::parse(r#"{"keyboard": {"mode": "effect", "name": "static"}, "future": {"a": 1}, "theme": {"name": "hearth", "later": true}}"#)
        .unwrap();
    assert_eq!(look.keyboard.unwrap().name, "static");
    assert_eq!(look.theme.unwrap().name, "hearth");
}

/// The look as a program that predates themes reads it.
#[test]
fn a_program_without_the_theme_member_reads_the_rest_of_the_look() {
    #[derive(serde::Deserialize)]
    struct OldLook {
        keyboard: Option<keyboard::State>,
        #[allow(dead_code)]
        rear: Option<crate::looks::Rear>,
    }
    let text = serde_json::to_string(&Look {
        keyboard: Some(keyboard::State {
            mode: "effect".into(),
            name: "static".into(),
            ..Default::default()
        }),
        theme: Some(ThemeSpec::new("hearth")),
        ..Look::default()
    })
    .unwrap();
    let old: OldLook = serde_json::from_str(&text).unwrap();
    assert_eq!(old.keyboard.unwrap().name, "static");
}

struct NoKeyboard;
impl KeyboardControl for NoKeyboard {
    fn last(&self) -> keyboard::State {
        keyboard::State::default()
    }
    fn replay(&self, _: &keyboard::State) -> Result<Option<String>, KeyboardError> {
        Ok(None)
    }
    fn off(&self) -> Result<(), KeyboardError> {
        Ok(())
    }
}
struct NoRear;
impl RearControl for NoRear {
    fn rear_set(&self, _: &RearSettings) -> Result<HwRear, HwError> {
        Err(HwError::Invalid("no rear in this test".into()))
    }
}

fn looks(r: &Rig) -> LookStore {
    LookStore::new(
        r.dir.join("looks"),
        r.dir.join("boot.json"),
        Arc::clone(&r.state),
        Arc::new(NoKeyboard),
        Arc::new(NoRear),
    )
}

#[test]
fn saving_a_look_checks_its_theme_part_and_the_running_theme_is_captured() {
    let r = rig();
    let looks = looks(&r);
    let bad =
        Look { theme: Some(ThemeSpec { speed: Some(1), ..ThemeSpec::new("hearth") }), ..Look::default() };
    assert!(looks.save("bad", Some(&bad)).is_err());
    let good = Look { theme: Some(ThemeSpec::new("hearth")), ..Look::default() };
    looks.save("themed", Some(&good)).unwrap();
    assert_eq!(looks.load("themed").unwrap().theme.unwrap().name, "hearth");
    // Nothing on the lights and no theme: there is nothing to capture.
    assert!(looks.capture().is_err());
    r.store.start(&ThemeSpec::new("hearth"), 1.0).unwrap();
    let captured = looks.capture().unwrap();
    assert_eq!(captured.theme.unwrap().name, "hearth");
}

// ---- applying a look starts and ends the theme --------------------------------------------------

fn static_keyboard() -> keyboard::State {
    keyboard::State {
        mode: "effect".into(),
        name: "static".into(),
        color: "112233".into(),
        ..Default::default()
    }
}

#[test]
fn applying_a_themed_look_records_the_theme_after_the_static_parts() {
    let r = rig();
    let looks = looks(&r).with_clock(|| 42.0);
    let look = Look {
        keyboard: Some(static_keyboard()),
        theme: Some(ThemeSpec { speed: Some(50), ..ThemeSpec::new("hearth") }),
        ..Look::default()
    };
    let done = looks.apply(&look).unwrap();
    assert_eq!(done, ["keyboard: nothing to restore", "theme: hearth"]);
    let active = r.store.active().unwrap();
    assert_eq!((active.spec.name.as_str(), active.spec.speed, active.started), ("hearth", Some(50), 42.0));
}

#[test]
fn applying_a_look_with_a_keyboard_and_no_theme_ends_the_running_theme() {
    let r = rig();
    let looks = looks(&r);
    r.store.start(&ThemeSpec::new("hearth"), 1.0).unwrap();
    let done = looks.apply(&Look { keyboard: Some(static_keyboard()), ..Look::default() }).unwrap();
    assert_eq!(done.last().unwrap(), "theme: stopped");
    assert_eq!(r.store.active(), None);
    // With nothing running there is nothing to report.
    let done = looks.apply(&Look { keyboard: Some(static_keyboard()), ..Look::default() }).unwrap();
    assert_eq!(done, ["keyboard: nothing to restore"]);
}

#[test]
fn a_rear_only_look_leaves_the_running_theme_alone() {
    let r = rig();
    let looks = looks(&r);
    r.store.start(&ThemeSpec::new("hearth"), 1.0).unwrap();
    let rear: crate::looks::Rear =
        serde_json::from_value(json!({"mode": "static", "color": "ff0000", "speed": 2, "brightness": 50}))
            .unwrap();
    // The fake rear light refuses (it is not under test); the theme must not have been touched
    // by the attempt either way.
    let _ = looks.apply(&Look { rear: Some(rear), ..Look::default() });
    assert!(r.store.active().is_some());
}

#[test]
fn a_theme_part_with_a_bad_shape_is_refused_when_applied() {
    let r = rig();
    let looks = looks(&r);
    let bad = Look {
        theme: Some(ThemeSpec { brightness: Some(500), ..ThemeSpec::new("hearth") }),
        ..Look::default()
    };
    assert!(looks.apply(&bad).is_err());
    assert_eq!(r.store.active(), None);
}
