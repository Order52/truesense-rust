//! Tests of the login settings: every branch of the decisions against a fake that can fail on
//! demand (the cases of the Go edition's tests and `tests/test_gui.py`), then the real entry
//! points over the simulated machine.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use ts_api::api::testing::FakeBundles;
use ts_api::api::{Api, Services};
use ts_domain::keyboard::State as KeyState;
use ts_domain::looks::{Look, Rear};
use ts_foundation::config::StartupConfig;
use ts_platform::hw::{Coded, Reading, RearMode, RearSettings, Rgb};

use super::login::{ApplyOptions, Lights, Login};
use super::{apply, apply_with, py_repr, set_option};

/// A fake of the lights: records what was asked and fails the first `fail_first` calls of the
/// device steps (keyboard and rear) when told to.
#[derive(Default)]
struct Fake {
    calls: RefCell<Vec<String>>,
    looks: BTreeMap<String, Look>,
    rear_state: Option<Value>,
    keyboard_last: Option<String>,
    fail_profile: bool,
    fail_device_times: RefCell<u32>,
    pauses: RefCell<Vec<Duration>>,
}

impl Fake {
    fn note(&self, what: impl Into<String>) {
        self.calls.borrow_mut().push(what.into());
    }

    fn device(&self, what: impl Into<String>) -> Result<(), String> {
        self.note(what);
        let mut left = self.fail_device_times.borrow_mut();
        if *left > 0 {
            *left -= 1;
            return Err("keyboard not found".to_owned());
        }
        Ok(())
    }

    fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }
}

fn look_with(keyboard: bool, rear: bool) -> Look {
    Look {
        keyboard: keyboard.then(|| KeyState {
            mode: "effect".into(),
            name: "wave".into(),
            ..KeyState::default()
        }),
        rear: rear.then(|| Rear {
            mode: "neon".into(),
            color: "112233".into(),
            speed: Some(2),
            brightness: Some(40),
            direction: Some(0),
            zones: None,
        }),
        theme: None,
    }
}

impl Lights for Fake {
    fn apply_profile(&self, name: &str) -> Result<String, String> {
        self.note(format!("profile {name}"));
        if self.fail_profile { Err("no such profile".into()) } else { Ok(format!("profile {name} applied")) }
    }

    fn load_look(&self, name: &str) -> Result<Look, String> {
        self.note(format!("load look {name}"));
        self.looks.get(name).cloned().ok_or_else(|| format!("no look {}", py_repr(name)))
    }

    fn apply_look(&self, look: &Look) -> Result<Vec<String>, String> {
        self.device("apply look")?;
        let mut lines = Vec::new();
        if look.keyboard.is_some() {
            lines.push("keyboard: wave".to_owned());
        }
        if look.rear.is_some() {
            lines.push("rear: neon".to_owned());
        }
        Ok(lines)
    }

    fn keyboard_last(&self) -> Result<Option<String>, String> {
        self.device("keyboard last")?;
        Ok(self.keyboard_last.clone())
    }

    fn keyboard_default(&self) -> Result<(), String> {
        self.device("keyboard default")
    }

    fn keyboard_off(&self) -> Result<(), String> {
        self.device("keyboard off")
    }

    fn lighting_profile(&self, name: &str) -> Result<(), String> {
        self.device(format!("lighting profile {name}"))
    }

    fn rear_set(&self, s: &RearSettings) -> Result<(), String> {
        self.device(format!(
            "rear {} {} {} {} {} {:?}",
            s.mode.name(),
            s.color,
            s.speed,
            s.brightness,
            s.direction,
            s.zones.as_ref().map(Vec::len)
        ))
    }

    fn rear_state(&self) -> Option<Value> {
        self.rear_state.clone()
    }

    fn pause(&self, how_long: Duration) {
        self.pauses.borrow_mut().push(how_long);
    }
}

fn settings(profile: &str, keyboard: &str, rear: &str) -> StartupConfig {
    StartupConfig { profile: profile.into(), keyboard: keyboard.into(), rear: rear.into() }
}

fn run(fake: &Fake, s: &StartupConfig) -> Vec<String> {
    Login::new(fake, ApplyOptions::default()).run(s)
}

#[test]
fn defaults_restore_the_last_state_of_both() {
    let fake = Fake {
        keyboard_last: Some("effect wave".into()),
        rear_state: Some(
            json!({"mode": "breathing", "color": "3723c7", "speed": 3, "brightness": 100, "direction": 1}),
        ),
        ..Fake::default()
    };
    assert_eq!(run(&fake, &settings("", "last", "last")), ["keyboard: effect wave", "rear: restored"]);
    assert_eq!(fake.calls(), ["keyboard last", "rear breathing 3723c7 3 100 1 None"]);
}

#[test]
fn nothing_saved_is_said_not_failed() {
    let fake = Fake::default();
    assert_eq!(run(&fake, &settings("", "last", "last")), ["keyboard: nothing saved", "rear: nothing saved"]);
    // not an object: not a setting we wrote
    for odd in [json!("x"), json!(7), json!([1, 2]), Value::Null] {
        let fake = Fake { rear_state: Some(odd), ..Fake::default() };
        assert_eq!(run(&fake, &settings("", "none", "last")), ["rear: nothing saved"]);
        assert!(fake.calls().is_empty(), "nothing is written for a state that is not ours");
    }
}

#[test]
fn each_choice_does_its_own_thing() {
    let fake = Fake::default();
    assert_eq!(run(&fake, &settings("", "default", "default")), ["keyboard: stock look", "rear: stock look"]);
    assert_eq!(fake.calls(), ["keyboard default", "rear breathing 3723c7 3 100 1 None"]);

    let fake = Fake::default();
    assert_eq!(run(&fake, &settings("", "off", "off")), ["keyboard: off", "rear: off"]);
    assert_eq!(fake.calls(), ["keyboard off", "rear static 00aec7 3 0 1 None"]);

    let fake = Fake::default();
    assert_eq!(run(&fake, &settings("", "profile:mine", "none")), ["keyboard: profile:mine"]);
    assert_eq!(fake.calls(), ["lighting profile mine"]);

    let fake = Fake::default();
    assert!(run(&fake, &settings("", "none", "none")).is_empty());
    assert!(fake.calls().is_empty(), "none leaves the firmware's own lights");
}

#[test]
fn the_profile_is_applied_first_and_never_retried() {
    let fake = Fake { fail_profile: true, ..Fake::default() };
    let out = Login::new(&fake, ApplyOptions { retries: 3, delay: Duration::from_secs(1) })
        .run(&settings("gaming", "off", "none"));
    assert_eq!(out, ["profile: FAILED (no such profile)", "keyboard: off"]);
    assert_eq!(fake.calls(), ["profile gaming", "keyboard off"]);
    assert!(fake.pauses.borrow().is_empty());

    let ok = Fake::default();
    assert_eq!(run(&ok, &settings("gaming", "none", "none")), ["profile: profile gaming applied"]);
}

#[test]
fn a_look_sets_keyboard_and_rear_together() {
    let mut fake = Fake::default();
    fake.looks.insert("night".into(), look_with(true, true));
    assert_eq!(
        run(&fake, &settings("", "look:night", "look:night")),
        ["keyboard: keyboard: wave", "rear: rear: neon"]
    );
    assert_eq!(fake.calls(), ["load look night", "apply look", "apply look"]);
}

#[test]
fn a_look_for_one_half_leaves_the_other_choice_alone() {
    let mut fake = Fake { keyboard_last: Some("kept".into()), ..Fake::default() };
    fake.looks.insert("night".into(), look_with(true, true));
    assert_eq!(run(&fake, &settings("", "last", "look:night")), ["rear: rear: neon", "keyboard: kept"]);
    let mut fake = Fake::default();
    fake.looks.insert("night".into(), look_with(true, true));
    assert_eq!(run(&fake, &settings("", "look:night", "off")), ["keyboard: keyboard: wave", "rear: off"]);
}

#[test]
fn a_look_without_that_half_is_unchanged() {
    let mut fake = Fake::default();
    fake.looks.insert("kb-only".into(), look_with(true, false));
    assert_eq!(run(&fake, &settings("", "none", "look:kb-only")), ["rear: unchanged"]);
    assert_eq!(fake.calls(), ["load look kb-only"], "nothing is applied for an empty half");
    let mut fake = Fake::default();
    fake.looks.insert("empty".into(), Look::default());
    assert_eq!(
        run(&fake, &settings("", "look:empty", "look:empty")),
        ["keyboard: unchanged", "rear: unchanged"]
    );
}

#[test]
fn with_both_looks_the_keyboards_look_gives_both_parts() {
    // 0.3.x quirk, kept: one look is loaded and the rear's name is not looked at
    let mut fake = Fake::default();
    fake.looks.insert("a".into(), look_with(true, false));
    fake.looks.insert("b".into(), look_with(false, true));
    assert_eq!(
        run(&fake, &settings("", "look:a", "look:b")),
        ["keyboard: keyboard: wave", "rear: unchanged"]
    );
    assert_eq!(fake.calls()[0], "load look a");
}

#[test]
fn a_missing_look_is_a_failed_step_not_a_crash() {
    let fake = Fake::default();
    assert_eq!(
        run(&fake, &settings("", "look:ghost", "look:ghost")),
        ["keyboard: FAILED (no look 'ghost')", "rear: FAILED (no look 'ghost')"]
    );
    assert_eq!(run(&fake, &settings("", "none", "look:ghost")), ["rear: FAILED (no look 'ghost')"]);
}

#[test]
fn a_failing_step_never_stops_the_others() {
    let fake = Fake { fail_device_times: RefCell::new(1), ..Fake::default() };
    let out = run(&fake, &settings("", "off", "off"));
    assert_eq!(out, ["keyboard: FAILED (keyboard not found)", "rear: off"]);
}

#[test]
fn a_keyboard_that_has_not_enumerated_is_retried() {
    let fake = Fake { fail_device_times: RefCell::new(2), ..Fake::default() };
    let out = Login::new(&fake, ApplyOptions { retries: 3, delay: Duration::from_millis(1500) })
        .run(&settings("", "default", "none"));
    assert_eq!(out, ["keyboard: stock look"]);
    assert_eq!(fake.calls().len(), 3);
    assert_eq!(*fake.pauses.borrow(), [Duration::from_millis(1500); 2]);
}

#[test]
fn the_retries_run_out() {
    let fake = Fake { fail_device_times: RefCell::new(99), ..Fake::default() };
    let out = Login::new(&fake, ApplyOptions { retries: 2, delay: Duration::from_secs(1) })
        .run(&settings("", "off", "none"));
    assert_eq!(out, ["keyboard: FAILED (keyboard not found)"]);
    assert_eq!(fake.calls().len(), 3, "one try and two retries");
    assert_eq!(fake.pauses.borrow().len(), 2);
}

#[test]
fn without_retries_a_step_is_tried_once() {
    let fake = Fake { fail_device_times: RefCell::new(99), ..Fake::default() };
    run(&fake, &settings("", "off", "off"));
    assert_eq!(fake.calls().len(), 2);
    assert!(fake.pauses.borrow().is_empty());
}

#[test]
fn an_incomplete_saved_rear_setting_is_reported() {
    for state in [
        json!({"mode": "static", "color": "00aec7", "speed": 3}),
        json!({"mode": "static", "color": "00aec7", "brightness": 10}),
        json!({"color": "00aec7", "speed": 3, "brightness": 10}),
        json!({"mode": "static", "speed": 3, "brightness": 10}),
        json!({}),
    ] {
        let fake = Fake { rear_state: Some(state.clone()), ..Fake::default() };
        let out = run(&fake, &settings("", "none", "last"));
        // an empty object is the same as nothing remembered
        if state == json!({}) {
            assert_eq!(out.len(), 1, "{out:?}");
        } else {
            assert_eq!(out, ["rear: FAILED (the saved rear light setting is incomplete)"], "{state}");
        }
        assert!(fake.calls().is_empty());
    }
}

#[test]
fn a_saved_rear_setting_that_is_wrong_is_reported_with_the_hardware_layers_words() {
    let fake = Fake {
        rear_state: Some(json!({"mode": "disco", "color": "00aec7", "speed": 3, "brightness": 10})),
        ..Fake::default()
    };
    assert_eq!(
        run(&fake, &settings("", "none", "last")),
        ["rear: FAILED (rear mode: static, breathing, neon, wave, twinkling)"]
    );
    let fake = Fake {
        rear_state: Some(json!({"mode": "static", "color": "nothex", "speed": 3, "brightness": 10})),
        ..Fake::default()
    };
    assert_eq!(run(&fake, &settings("", "none", "last")), ["rear: FAILED (color must be RRGGBB hex)"]);
}

#[test]
fn the_saved_rear_zones_and_direction_come_back() {
    let fake = Fake {
        rear_state: Some(json!({
            "mode": "static", "color": "ff0000", "speed": 2, "brightness": 50,
            "zones": ["ff0000", "00ff00", "0000ff"]
        })),
        ..Fake::default()
    };
    assert_eq!(run(&fake, &settings("", "none", "last")), ["rear: restored"]);
    assert_eq!(fake.calls(), ["rear static ff0000 2 50 1 Some(3)"], "a missing direction is 1");
}

#[test]
fn hostile_rear_states_never_panic() {
    for state in [
        json!({"mode": 5, "color": 6}),
        json!({"mode": "static", "color": "00aec7", "speed": "fast", "brightness": 1}),
        json!({"mode": "static", "color": "00aec7", "speed": 2.5, "brightness": 1}),
        json!({"mode": "static", "color": "00aec7", "speed": 3, "brightness": 1, "zones": "x"}),
        json!({"mode": "static", "color": "00aec7", "speed": 3, "brightness": 1, "zones": [1, 2]}),
        json!({"mode": "static", "color": "00aec7", "speed": 3, "brightness": 1, "zones": []}),
        json!({"mode": "\u{0}", "color": "\u{0}", "speed": i64::MAX, "brightness": i64::MIN}),
    ] {
        let fake = Fake { rear_state: Some(state), ..Fake::default() };
        let out = run(&fake, &settings("", "none", "last"));
        assert_eq!(out.len(), 1);
    }
}

// ---- the real entry points over the simulated machine --------------------------------------

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ts-keys-startup-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct World {
    api: Api,
    bundles: std::sync::Arc<FakeBundles>,
    dir: TempDir,
}

fn world() -> World {
    let dir = TempDir::new();
    let (services, rig) = Services::simulated(&dir.0).expect("simulated services");
    rig.bundles.add("gaming", "turbo and loud");
    rig.bundles.add("quiet", "eco and silent");
    World { api: Api::new(services), bundles: rig.bundles, dir }
}

fn write_user_config(w: &World, text: &str) {
    let path = w.dir.0.join("home/config.toml");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    std::fs::write(path, text).expect("write config");
}

fn user_config(w: &World) -> String {
    std::fs::read_to_string(w.dir.0.join("home/config.toml")).unwrap_or_default()
}

#[test]
fn set_option_stores_and_answers_the_value() {
    let w = world();
    w.api.save_look_value("smokelook", &look_with(false, true)).expect("a saved look");
    for (target, value) in [
        ("keyboard", "default"),
        ("keyboard", "last"),
        ("keyboard", "none"),
        ("keyboard", "off"),
        ("keyboard", "look:smokelook"),
        ("rear", "off"),
        ("rear", "look:smokelook"),
        ("rear", "default"),
    ] {
        assert_eq!(set_option(&w.api, target, value).as_deref().ok(), Some(value), "{target} {value}");
        let s = w.api.startup();
        let stored = if target == "keyboard" { s.keyboard } else { s.rear };
        assert_eq!(stored, value);
    }
    assert_eq!(set_option(&w.api, "profile", "gaming").as_deref().ok(), Some("gaming"));
    assert_eq!(w.api.startup().profile, "gaming");
    assert_eq!(set_option(&w.api, "profile", "none").as_deref().ok(), Some(""), "none clears the profile");
    assert_eq!(w.api.startup().profile, "");
}

#[test]
fn set_option_with_a_saved_lighting_profile() {
    let w = world();
    w.api.services().lighting.save("mine", &vec![ts_domain::keymap::Color::default(); 128]).expect("save");
    assert_eq!(set_option(&w.api, "keyboard", "profile:mine").as_deref().ok(), Some("profile:mine"));
}

#[test]
fn set_option_refuses_and_writes_nothing() {
    let w = world();
    w.api.save_look_value("smokelook", &look_with(false, true)).expect("a saved look");
    for (target, value, want) in [
        ("colour", "red", "target: keyboard | rear | profile"),
        ("profile", "ghost", "no profile 'ghost': gaming, quiet"),
        ("keyboard", "profile:ghost", "no lighting profile 'ghost': "),
        ("keyboard", "look:nosuchlook", "no look 'nosuchlook': "),
        ("rear", "look:nosuchlook", "no look 'nosuchlook': "),
        ("keyboard", "bogus", "[startup] keyboard must be"),
        ("rear", "profile:x", "[startup] rear must be"),
        ("keyboard", "", "[startup] keyboard must be"),
        ("profile", "look:smokelook", "no profile 'look:smokelook'"),
    ] {
        let err = set_option(&w.api, target, value).expect_err(&format!("{target} {value}"));
        assert!(err.to_string().contains(want), "{target} {value}: {err} (wanted {want})");
    }
    assert_eq!(user_config(&w), "", "a refused option writes nothing");
    assert_eq!((w.api.startup().keyboard.as_str(), w.api.startup().rear.as_str()), ("last", "last"));
}

#[test]
fn set_option_keeps_the_rest_of_the_users_configuration() {
    let w = world();
    write_user_config(&w, "[audio]\nnotify = false\n[startup]\nrear = \"off\"\n");
    set_option(&w.api, "keyboard", "off").expect("set");
    let text = user_config(&w);
    assert!(text.contains("notify = false"), "{text}");
    assert!(text.contains("rear = \"off\""), "{text}");
    assert!(text.contains("keyboard = \"off\""), "{text}");
}

#[test]
fn set_option_with_a_broken_configuration_says_so() {
    let w = world();
    write_user_config(&w, "[startup\n");
    assert!(set_option(&w.api, "keyboard", "off").is_err());
}

#[test]
fn py_repr_quotes_like_python() {
    for (text, want) in [
        ("x", "'x'"),
        ("it's", "\"it's\""),
        ("a\"b", "'a\"b'"),
        ("a'\"b", "'a\\'\"b'"),
        ("", "''"),
        ("a\\b", "'a\\\\b'"),
        ("a\nb", "'a\\nb'"),
        ("\u{1}", "'\\x01'"),
    ] {
        assert_eq!(py_repr(text), want, "{text:?}");
    }
}

#[test]
fn apply_defaults_on_the_simulated_machine() {
    let w = world();
    write_user_config(&w, "[startup]\nkeyboard = \"default\"\nrear = \"off\"\nprofile = \"gaming\"\n");
    let out = apply(&w.api).expect("apply");
    assert_eq!(out, ["profile: applied gaming", "keyboard: stock look", "rear: off"]);
    assert_eq!(w.bundles.applied(), ["gaming"]);
    let rear = w.api.services().hw.rear_get().expect("rear");
    assert_eq!(rear.brightness, 0, "off is dark");
    assert_eq!(rear.mode, Reading::Known(RearMode::Static));
}

#[test]
fn apply_default_rear_is_the_stock_breathing_look() {
    let w = world();
    write_user_config(&w, "[startup]\nkeyboard = \"none\"\nrear = \"default\"\n");
    assert_eq!(apply(&w.api).expect("apply"), ["rear: stock look"]);
    let rear = w.api.services().hw.rear_get().expect("rear");
    assert_eq!((rear.brightness, rear.color), (100, Rgb([0x37, 0x23, 0xc7])));
    assert_eq!(rear.mode, Reading::Known(RearMode::Breathing));
}

#[test]
fn apply_last_brings_back_what_the_hardware_layer_remembered() {
    let w = world();
    write_user_config(&w, "[startup]\nkeyboard = \"none\"\nrear = \"last\"\n");
    assert_eq!(apply(&w.api).expect("apply"), ["rear: nothing saved"]);
    // a rear light set through the hardware layer is remembered in the user state
    let neon = RearSettings {
        mode: RearMode::Neon,
        color: Rgb([1, 2, 3]),
        speed: 4,
        brightness: 60,
        ..RearSettings::default()
    };
    w.api.services().hw.rear_set(&neon).expect("rear set");
    let dark = RearSettings { brightness: 0, ..neon.clone() };
    w.api.services().hw.rear_set(&dark).expect("rear set");
    let _ = w.api.services().user.set(
        "rear_state",
        json!({"mode": "neon", "color": "010203", "speed": 4, "brightness": 60, "direction": 1}),
    );
    assert_eq!(apply(&w.api).expect("apply"), ["rear: restored"]);
    assert_eq!(w.api.services().hw.rear_get().expect("rear").brightness, 60);
}

#[test]
fn apply_keyboard_look_is_applied_to_the_simulated_keyboard() {
    let w = world();
    write_user_config(&w, "[startup]\nkeyboard = \"look:native\"\nrear = \"look:native\"\n");
    let out = apply(&w.api).expect("apply");
    assert_eq!(out.len(), 2, "{out:?}");
    assert!(out[0].starts_with("keyboard: keyboard: "), "{out:?}");
    assert_eq!(out[1], "rear: rear: breathing");
}

#[test]
fn apply_reports_a_missing_profile_as_a_step() {
    let w = world();
    write_user_config(&w, "[startup]\nprofile = \"ghost\"\nkeyboard = \"none\"\nrear = \"none\"\n");
    let out = apply(&w.api).expect("apply");
    assert_eq!(out.len(), 1);
    assert!(out[0].starts_with("profile: FAILED ("), "{out:?}");
}

#[test]
fn apply_with_retries_uses_the_services_sleep() {
    let w = world();
    write_user_config(&w, "[startup]\nkeyboard = \"profile:ghost\"\nrear = \"none\"\n");
    let out =
        apply_with(&w.api, ApplyOptions { retries: 2, delay: Duration::from_millis(7) }).expect("apply");
    assert!(out[0].starts_with("keyboard: FAILED ("), "{out:?}");
}

#[test]
fn apply_with_a_broken_configuration_is_an_error() {
    let w = world();
    write_user_config(&w, "[startup\n");
    assert!(apply(&w.api).is_err());
}
