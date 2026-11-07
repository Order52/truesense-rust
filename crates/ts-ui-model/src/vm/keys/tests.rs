//! Tests of the Keys view-model. The important ones probe the real action parser (through the
//! application layer's `set_key_action`, which runs it) so that the mirror in `keys.rs` cannot drift.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::{Value, json};
use ts_api::api::testing::Rig;
use ts_api::api::{Api, KeyProfileEdit, Services};

use super::*;

/// A fresh temporary directory, removed when dropped.
struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!(
            "ts-ui-keys-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        Temp(p)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The application layer over simulated services: the real parser and the real profile store.
struct Machine {
    api: Api,
    _rig: Rig,
    _root: Temp,
}

fn machine() -> Machine {
    let root = Temp::new();
    let (services, rig) = Services::simulated(&root.0).unwrap();
    Machine { api: Api::new(services), _rig: rig, _root: root }
}

fn parse(spec: &str) -> Value {
    call("parse", &json!({"spec": spec})).unwrap()
}

fn compose(state: &Value) -> Value {
    call("compose", &json!({"state": state})).unwrap()
}

/// What the real parser says about `spec`; `known_looks` are the saved looks `look:NAME` is
/// checked against, exactly as the key service does.
fn real_verdict(m: &Machine, spec: &str) -> Result<(), String> {
    let tap = Api::parse_gesture("tap").unwrap();
    m.api.set_key_action(tap, spec).map_err(|e| e.to_string())
}

/// The verdict of the mirror, with the same look list the real parser has.
fn mirror_verdict(m: &Machine, spec: &str) -> Result<(), String> {
    let known = json!({"look": m.api.looks()});
    let r = call("parse", &json!({"spec": spec, "known": known})).unwrap();
    if r["valid"] == json!(false) {
        return Err(r["error"].as_str().unwrap().to_owned());
    }
    if r["missing"] == json!(true) && r["state"]["kind"] == json!("look") {
        return Err("no saved look".to_owned());
    }
    Ok(())
}

/// Every action text the real parser accepts, with the text the editor writes back for it.
const GOOD: &[(&str, &str)] = &[
    ("none", "none"),
    ("power-next", "power-next"),
    ("power-prev", "power-prev"),
    ("power:eco", "power:eco"),
    ("power:quiet", "power:quiet"),
    ("power:balanced", "power:balanced"),
    ("power:performance", "power:performance"),
    ("power:turbo", "power:turbo"),
    ("power-cycle:eco,turbo", "power-cycle:eco,turbo"),
    ("power-cycle:turbo", "power-cycle:turbo"),
    ("power-cycle:performance,turbo", "power-cycle:performance,turbo"),
    ("power-cycle:eco,quiet,balanced,performance,turbo", "power-cycle:eco,quiet,balanced,performance,turbo"),
    ("power-cycle:eco,eco", "power-cycle:eco,eco"),
    ("fan:smart", "fan:smart"),
    ("fan:auto", "fan:auto"),
    ("fan:max", "fan:max"),
    ("fan:20", "fan:20"),
    ("fan:60", "fan:60"),
    ("fan:100", "fan:100"),
    ("fan:020", "fan:20"),
    ("fan-max-toggle", "fan-max-toggle"),
    ("smart-fan-toggle", "smart-fan-toggle"),
    ("profile:gaming", "profile:gaming"),
    ("profile:a:b", "profile:a:b"),
    ("profile-next", "profile-next"),
    ("favourite:1", "favourite:1"),
    ("favourite:24", "favourite:24"),
    ("favourite:024", "favourite:24"),
    (" favourite : 12 ", "favourite:12"),
    ("favourite-next", "favourite-next"),
    ("output-toggle", "output-toggle"),
    ("output-cycle", "output-cycle"),
    ("output:speakers", "output:speakers"),
    ("output:headphones", "output:headphones"),
    ("mic-toggle", "mic-toggle"),
    ("mic:mute", "mic:mute"),
    ("mic:unmute", "mic:unmute"),
    ("voice-next", "voice-next"),
    ("touchpad-toggle", "touchpad-toggle"),
    ("touchpad:on", "touchpad:on"),
    ("touchpad:off", "touchpad:off"),
    ("keyboard:on", "keyboard:on"),
    ("keyboard:off", "keyboard:off"),
    ("keyboard:toggle", "keyboard:toggle"),
    ("keyboard-effect-next", "keyboard-effect-next"),
    ("keyboard-profile:mine", "keyboard-profile:mine"),
    ("rear:on", "rear:on"),
    ("rear:off", "rear:off"),
    ("rear:toggle", "rear:toggle"),
    ("rear-effect-next", "rear-effect-next"),
    ("battery-limit-toggle", "battery-limit-toggle"),
    ("overdrive-toggle", "overdrive-toggle"),
    ("look:native", "look:native"),
    ("auto:off", "auto:off"),
    ("auto:native", "auto:native"),
    ("auto:smart", "auto:smart"),
    ("auto-next", "auto-next"),
    ("exec:truesense gui", "exec:truesense gui"),
    ("exec:notify-send a:b", "exec:notify-send a:b"),
    ("exec: sh -c 'echo a:b' ", "exec:sh -c 'echo a:b'"),
    ("notify:hi there", "notify:hi there"),
    ("  power:eco  ", "power:eco"),
    ("power : eco", "power:eco"),
];

/// Texts the real parser refuses, with its message.
const BAD: &[(&str, &str)] = &[
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
    ("power:Turbo", "power mode must be one of quiet, balanced, performance, turbo, eco"),
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
    ("favourite", "'favourite' needs an argument (e.g. favourite:...)"),
    ("favourite:0", "favourite: a position from 1 to 24"),
    ("favourite:25", "favourite: a position from 1 to 24"),
    ("favourite:-1", "favourite: a position from 1 to 24"),
    ("favourite:+1", "favourite: a position from 1 to 24"),
    ("favourite:two", "favourite: a position from 1 to 24"),
    ("favourite:\u{b2}", "favourite: a position from 1 to 24"),
    ("favourite:99999999999999999999999", "favourite: a position from 1 to 24"),
    ("favourite-next:2", "'favourite-next' takes no argument"),
    ("output:hdmi", "output: speakers | headphones"),
    ("mic:half", "mic: mute | unmute"),
    ("touchpad:toggle", "touchpad: on | off"),
    ("keyboard:dim", "keyboard: on | off | toggle"),
    ("rear:dim", "rear: on | off | toggle"),
];

#[test]
fn every_action_the_parser_accepts_round_trips_through_the_composition() {
    let m = machine();
    for (spec, canonical) in GOOD {
        assert_eq!(real_verdict(&m, spec), Ok(()), "the real parser must accept {spec:?}");
        let parsed = parse(spec);
        assert_eq!(parsed["valid"], json!(true), "{spec:?}: {parsed}");
        let composed = compose(&parsed["state"]);
        assert_eq!(composed, json!({"valid": true, "spec": canonical}), "{spec:?}");
        // What the editor writes is itself an action the real parser accepts, and reading it again
        // gives the same state (nothing drifts when a page is opened and closed).
        assert_eq!(real_verdict(&m, canonical), Ok(()), "the editor wrote {canonical:?}");
        assert_eq!(parse(canonical)["state"], parsed["state"], "{spec:?}");
    }
}

#[test]
fn every_action_the_parser_refuses_is_refused_here_with_its_message() {
    let m = machine();
    for (spec, message) in BAD {
        let real = real_verdict(&m, spec).unwrap_err();
        assert!(real.contains(message), "real parser for {spec:?}: {real}");
        let parsed = parse(spec);
        assert_eq!(parsed["valid"], json!(false), "{spec:?}");
        assert_eq!(parsed["error"], json!(message), "{spec:?}");
        // The state of an unreadable action is the blank "nothing", never a half-parsed one.
        assert_eq!(parsed["state"]["kind"], json!("none"), "{spec:?}");
    }
}

/// The words of the documented catalogue: every action name appears in it (a test of the domain
/// crate enforces that), so a word that the parser accepts, bare or with an argument, is a name.
fn documented_names(m: &Machine) -> BTreeSet<String> {
    let text = Api::key_actions();
    let probes = ["", ":on", ":eco", ":1", ":50", ":mute", ":speakers", ":smart", ":native", ":x"];
    let mut words = BTreeSet::new();
    let mut word = String::new();
    for c in text.chars().chain(std::iter::once(' ')) {
        if c.is_ascii_lowercase() || c == '-' {
            word.push(c);
        } else {
            if word.len() > 1 {
                words.insert(std::mem::take(&mut word));
            }
            word.clear();
        }
    }
    words
        .into_iter()
        .filter(|w| probes.iter().any(|p| real_verdict(m, &format!("{w}{p}")).is_ok()))
        .collect()
}

#[test]
fn the_picker_offers_exactly_the_actions_the_catalogue_documents() {
    let m = machine();
    let documented = documented_names(&m);
    let offered: BTreeSet<String> = call("catalogue", &json!({})).unwrap()["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["kinds"].as_array().unwrap().iter())
        .map(|k| k["kind"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(offered, documented);
    // 32 names today; a new action in the domain crate fails here until the picker has it.
    assert_eq!(offered.len(), 32);
}

#[test]
fn the_mirror_and_the_real_parser_agree_on_a_wide_sweep_of_hostile_texts() {
    let m = machine();
    let mut names: Vec<String> = documented_names(&m).into_iter().collect();
    names.extend(["", "Power", "POWER", "power ", "frob", "pow", "-", "a:b", "none "].map(str::to_owned));
    let args = [
        "", "on", "off", "toggle", "eco", "quiet", "balanced", "performance", "turbo", "Turbo", "warp", "smart",
        "native", "max", "auto", "mute", "unmute", "speakers", "headphones", "0", "1", "19", "20", "24", "25",
        "99", "100", "101", "020", "-1", "+1", "3.5", "\u{b2}", "eco,turbo", "eco,,turbo", "eco, turbo",
        "eco,warp", ",", "a:b", "x y", "\u{0}", "99999999999999999999999",
    ];
    let mut accepted = 0;
    for name in &names {
        for arg in args {
            for spec in [format!("{name}:{arg}"), format!(" {name} : {arg} "), name.clone()] {
                let real = real_verdict(&m, &spec);
                let mirror = mirror_verdict(&m, &spec);
                assert_eq!(real.is_ok(), mirror.is_ok(), "{spec:?}: real {real:?}, mirror {mirror:?}");
                if real.is_ok() {
                    accepted += 1;
                    // Whatever is accepted composes to something the real parser accepts again.
                    let parsed = parse(&spec);
                    let back = compose(&parsed["state"]);
                    let again = back["spec"].as_str().unwrap();
                    assert_eq!(real_verdict(&m, again), Ok(()), "{spec:?} composed to {again:?}");
                }
            }
        }
    }
    assert!(accepted > 100, "the sweep must reach the accepting paths ({accepted})");
}

#[test]
fn compose_refuses_states_that_are_not_actions() {
    let m = machine();
    let bad = [
        json!({"kind": "frobnicate"}),
        json!({"kind": ""}),
        json!({"kind": "power", "choice": "warp"}),
        json!({"kind": "power", "choice": ""}),
        json!({"kind": "power-cycle", "modes": []}),
        json!({"kind": "power-cycle", "modes": ["eco", "warp"]}),
        json!({"kind": "fan", "choice": "manual", "percent": 0}),
        json!({"kind": "fan", "choice": "manual", "percent": 19}),
        json!({"kind": "fan", "choice": "manual", "percent": 101}),
        json!({"kind": "fan", "choice": "stop"}),
        json!({"kind": "favourite", "position": 0}),
        json!({"kind": "favourite", "position": 25}),
        json!({"kind": "exec", "text": ""}),
        json!({"kind": "exec", "text": "   "}),
        json!({"kind": "look", "text": ""}),
        json!({"kind": "keyboard", "choice": "dim"}),
        json!({"kind": "touchpad", "choice": "toggle"}),
    ];
    for state in bad {
        let r = compose(&state);
        assert_eq!(r["valid"], json!(false), "{state}");
        assert!(r["spec"].is_null(), "{state}");
        assert!(r["error"].as_str().is_some_and(|e| !e.is_empty()), "{state}");
    }
    // A fan can never be stopped by a key: 0 is refused above and 20 is the floor.
    assert_eq!(compose(&json!({"kind": "fan", "choice": "manual", "percent": 20}))["spec"], json!("fan:20"));
    // Whatever text a person types is trimmed; the real parser takes the same text.
    let typed = compose(&json!({"kind": "exec", "text": "  notify-send 'a: b'  "}));
    assert_eq!(typed["spec"], json!("exec:notify-send 'a: b'"));
    assert_eq!(real_verdict(&m, typed["spec"].as_str().unwrap()), Ok(()));
    // QML sends sliders as floats; a whole float is a whole number, a fraction is a mistake.
    assert_eq!(compose(&json!({"kind": "favourite", "position": 3.0}))["spec"], json!("favourite:3"));
    for hostile in [
        json!({"kind": "fan", "choice": "manual", "percent": 30.5}),
        json!({"kind": "fan", "choice": "manual", "percent": -1}),
        json!({"kind": "power", "choice": 5}),
        json!({"kind": "power-cycle", "modes": "eco"}),
        json!({"kind": "power-cycle", "modes": [1]}),
        json!({"choice": "eco"}),
    ] {
        assert!(call("compose", &json!({"state": hostile})).is_err(), "{hostile}");
    }
    assert!(call("compose", &json!({})).is_err());
}

#[test]
fn kinds_have_sensible_first_states_and_every_first_state_composes() {
    let known = json!({"profile": ["gaming", "work"], "look": ["native", "calm"], "keyboard-profile": []});
    let cat = call("catalogue", &json!({})).unwrap();
    for g in cat["groups"].as_array().unwrap() {
        for k in g["kinds"].as_array().unwrap() {
            let kind = k["kind"].as_str().unwrap();
            let s = call("defaults", &json!({"kind": kind, "known": known})).unwrap();
            assert_eq!(s["kind"], json!(kind));
            let c = compose(&s);
            // Text kinds and an empty library start empty: the person must fill them in, and the
            // Test and Use buttons say so instead of sending nonsense.
            let needs_input = matches!(kind, "exec" | "notify" | "keyboard-profile");
            assert_eq!(c["valid"], json!(!needs_input), "{kind}: {c}");
        }
    }
    assert_eq!(compose(&call("defaults", &json!({"kind": "power"})).unwrap())["spec"], json!("power:balanced"));
    assert_eq!(compose(&call("defaults", &json!({"kind": "rear"})).unwrap())["spec"], json!("rear:toggle"));
    assert_eq!(compose(&call("defaults", &json!({"kind": "auto"})).unwrap())["spec"], json!("auto:native"));
    assert_eq!(
        compose(&call("defaults", &json!({"kind": "look", "known": known})).unwrap())["spec"],
        json!("look:native")
    );
    assert_eq!(compose(&call("defaults", &json!({"kind": "fan"})).unwrap())["spec"], json!("fan:smart"));
    assert!(call("defaults", &json!({"kind": "nope"})).is_err());
    assert!(call("defaults", &json!({})).is_err());
}

#[test]
fn a_library_name_that_no_longer_exists_is_marked_missing_but_kept() {
    let known = json!({"profile": ["gaming"], "look": ["native", "calm"]});
    let gone = call("parse", &json!({"spec": "look:old", "known": known})).unwrap();
    assert_eq!(gone["valid"], json!(true));
    assert_eq!(gone["missing"], json!(true));
    assert_eq!(gone["state"]["text"], json!("old"));
    let here = call("parse", &json!({"spec": "look:calm", "known": known})).unwrap();
    assert_eq!(here["missing"], json!(false));
    // A list that is not loaded yet never makes a name look missing.
    assert_eq!(call("parse", &json!({"spec": "look:old"})).unwrap()["missing"], json!(false));
    // Kinds without a library are never missing.
    assert_eq!(call("parse", &json!({"spec": "power:eco", "known": known})).unwrap()["missing"], json!(false));
}

#[test]
fn rows_read_every_gesture_and_keep_unreadable_text_visible() {
    let bindings = json!({
        "tap": "power-next", "double": "output-toggle", "triple": "none", "long": "fan:nope",
        "long_ms": 800, "multi_ms": 450
    });
    let rows = call("rows", &json!({"bindings": bindings})).unwrap();
    let rows = rows.as_array().unwrap();
    let gestures: Vec<&str> = rows.iter().map(|r| r["gesture"].as_str().unwrap()).collect();
    assert_eq!(gestures, ["tap", "double", "triple", "long", "fn_esc"]);
    assert_eq!(rows[0]["state"]["kind"], json!("power-next"));
    assert_eq!(rows[3]["valid"], json!(false));
    assert_eq!(rows[3]["spec"], json!("fan:nope"));
    // fn_esc is absent from the bindings: an unbound key, not an error.
    assert_eq!(rows[4]["spec"], json!("none"));
    assert_eq!(rows[4]["valid"], json!(true));
    assert_eq!(call("rows", &json!({})).unwrap().as_array().unwrap().len(), 5);
}

#[test]
fn status_tells_when_the_draft_changed_and_what_cannot_be_saved() {
    let loaded = json!({"tap": "power-next", "double": "output-toggle", "triple": "none", "long": "fan-max-toggle",
        "fn_esc": "none", "long_ms": 800, "multi_ms": 450});
    let same = call("status", &json!({"loaded": loaded, "draft": loaded})).unwrap();
    assert_eq!(same["changed"], json!(false));
    assert_eq!(same["can_save"], json!(true));
    // Spacing is not a change; a different action or timing is.
    let mut spaced = loaded.clone();
    spaced["tap"] = json!(" power-next ");
    assert_eq!(call("status", &json!({"loaded": loaded, "draft": spaced})).unwrap()["changed"], json!(false));
    let mut other = loaded.clone();
    other["tap"] = json!("power-prev");
    assert_eq!(call("status", &json!({"loaded": loaded, "draft": other})).unwrap()["changed"], json!(true));
    let mut slower = loaded.clone();
    slower["long_ms"] = json!(900);
    assert_eq!(call("status", &json!({"loaded": loaded, "draft": slower})).unwrap()["changed"], json!(true));
    // An unreadable action blocks Save and Use and names the gesture.
    let mut broken = loaded.clone();
    broken["triple"] = json!("exec");
    let s = call("status", &json!({"loaded": loaded, "draft": broken})).unwrap();
    assert_eq!(s["invalid"], json!(["triple"]));
    assert_eq!(s["can_save"], json!(false));
    assert_eq!(s["can_use"], json!(false));
    // A built-in profile can be used but never saved.
    let locked = call("status", &json!({"loaded": loaded, "draft": loaded, "locked": true})).unwrap();
    assert_eq!(locked["can_save"], json!(false));
    assert_eq!(locked["can_use"], json!(true));
}

#[test]
fn timing_is_clamped_into_its_range_and_snapped_to_the_step() {
    let t = |a: Value| call("timing", &a).unwrap();
    assert_eq!(t(json!({"long_ms": 800, "multi_ms": 450})), json!({"long_ms": 800, "multi_ms": 450}));
    assert_eq!(t(json!({})), json!({"long_ms": 800, "multi_ms": 450}));
    assert_eq!(t(json!({"long_ms": 0, "multi_ms": 99999})), json!({"long_ms": 300, "multi_ms": 1000}));
    assert_eq!(t(json!({"long_ms": 99999, "multi_ms": -5})), json!({"long_ms": 2000, "multi_ms": 200}));
    assert_eq!(t(json!({"long_ms": 812, "multi_ms": 480})), json!({"long_ms": 800, "multi_ms": 500}));
    assert_eq!(t(json!({"long_ms": "slow", "multi_ms": null})), json!({"long_ms": 800, "multi_ms": 450}));
    // The ranges are the ones the domain crate enforces: its refusals bound ours.
    let m = machine();
    assert!(m.api.set_key_timing(Some(300), Some(200)).is_ok());
    assert!(m.api.set_key_timing(Some(2000), Some(1000)).is_ok());
    assert!(m.api.set_key_timing(Some(299), None).is_err());
    assert!(m.api.set_key_timing(Some(2001), None).is_err());
    assert!(m.api.set_key_timing(None, Some(199)).is_err());
    assert!(m.api.set_key_timing(None, Some(1001)).is_err());
}

#[test]
fn save_args_carry_every_gesture_and_the_timing_and_refuse_unreadable_actions() {
    let draft = json!({"tap": " power-next ", "double": "none", "triple": "none", "long": "fan:60",
        "fn_esc": "mic-toggle", "long_ms": 900, "multi_ms": 7});
    let a = call("save_args", &json!({"name": "mine", "draft": draft})).unwrap();
    assert_eq!(
        a,
        json!({"action": "save", "name": "mine", "tap": "power-next", "double": "none", "triple": "none",
            "long": "fan:60", "fn_esc": "mic-toggle", "long_ms": 900, "multi_ms": 200})
    );
    let bad = json!({"tap": "power", "long_ms": 800, "multi_ms": 450});
    let e = call("save_args", &json!({"name": "mine", "draft": bad})).unwrap_err();
    assert!(e.starts_with("tap: "), "{e}");
    assert!(call("save_args", &json!({"draft": draft})).is_err());
}

#[test]
fn save_names_follow_the_profile_library_rule() {
    let m = machine();
    let names = [
        "mine", "a.b_c-d", "x", "My Profile", "", "   ", "a/b", "..", ".hidden", "-x", "\u{e9}t\u{e9}",
        "default", "native", "gaming", "work", "lights", &"a".repeat(40), &"a".repeat(41), "a\u{0}b",
    ];
    for name in names {
        let verdict = call("save_name", &json!({"name": name})).unwrap();
        let edit = KeyProfileEdit::default();
        let trimmed = name.trim();
        let real = m.api.save_key_profile(trimmed, &edit);
        // `..` and `.hidden` are valid characters for the library but not a good name to offer:
        // the library accepts them today, so the page may too (the file lives in its own folder).
        assert_eq!(verdict["ok"], json!(real.is_ok()), "{name:?}: real {real:?}, vm {verdict}");
        if verdict["ok"] == json!(false) {
            assert!(["empty", "length", "chars", "builtin"].contains(&verdict["code"].as_str().unwrap()));
        }
    }
    let over = call("save_name", &json!({"name": "mine", "existing": ["mine", "gaming"]})).unwrap();
    assert_eq!(over["overwrites"], json!(true));
    let fresh = call("save_name", &json!({"name": "new", "existing": ["mine"]})).unwrap();
    assert_eq!(fresh["overwrites"], json!(false));
    assert_eq!(call("save_name", &json!({"name": " mine "})).unwrap()["name"], json!("mine"));
}

#[test]
fn the_builtin_profiles_are_the_ones_the_library_ships_and_read_cleanly_here() {
    let m = machine();
    let shipped = m.api.key_profiles().unwrap().names;
    assert_eq!(shipped, BUILTIN_PROFILES.map(str::to_owned).to_vec());
    for name in BUILTIN_PROFILES {
        let view = serde_json::to_value(m.api.key_profile(name).unwrap()).unwrap();
        let rows = call("rows", &json!({"bindings": view})).unwrap();
        for r in rows.as_array().unwrap() {
            assert_eq!(r["valid"], json!(true), "{name}: {r}");
        }
        let status = call("status", &json!({"loaded": view, "draft": view})).unwrap();
        assert_eq!(status["changed"], json!(false), "{name}");
        assert_eq!(timing_of(&view, "long_ms", LONG_MS), view["long_ms"].as_i64().unwrap());
    }
    assert_eq!(LONG_MS.default, m.api.key_profile("default").unwrap().bindings.long_ms);
    assert_eq!(MULTI_MS.default, m.api.key_profile("default").unwrap().bindings.multi_ms);
    // The page saves through `save_args`: the library takes exactly what it makes.
    let draft = json!({"tap": "power-cycle:eco,turbo", "double": "none", "triple": "none", "long": "none",
        "fn_esc": "none", "long_ms": 1000, "multi_ms": 300});
    let a = call("save_args", &json!({"name": "round", "draft": draft})).unwrap();
    let mut edit = KeyProfileEdit::default();
    for g in GESTURES {
        edit.actions.push((Api::parse_gesture(g).unwrap(), a[g].as_str().unwrap().to_owned()));
    }
    edit.long_ms = a["long_ms"].as_i64();
    edit.multi_ms = a["multi_ms"].as_i64();
    let saved = m.api.save_key_profile("round", &edit).unwrap();
    assert_eq!(saved.bindings.tap, "power-cycle:eco,turbo");
    assert_eq!(saved.bindings.long_ms, 1000);
}

#[test]
fn the_key_service_read_out_comes_from_the_service_check() {
    let rows = |active: &str, enabled: &str, fix: &str| {
        json!({"rows": [
            {"unit": "truesense.service", "scope": "system", "enabled": "enabled", "active": "active", "ok": true, "fix": ""},
            {"unit": "truesense-keys.service", "scope": "user", "enabled": enabled, "active": active, "ok": active == "active", "fix": fix},
        ]})
    };
    let s = |a: Value| call("service", &a).unwrap();
    assert_eq!(s(rows("active", "enabled", ""))["state"], json!("running"));
    let stopped = s(rows("inactive", "enabled", "truesense service repair"));
    assert_eq!(stopped["state"], json!("stopped"));
    assert_eq!(stopped["fix"], json!("truesense service repair"));
    assert_eq!(s(rows("failed", "disabled", "truesense service repair"))["state"], json!("stopped"));
    assert_eq!(s(rows("unknown", "unknown", ""))["state"], json!("unknown"));
    // No answer yet, an error result, or a list without the unit: unknown, never "stopped".
    for a in [json!({}), json!({"rows": null}), json!({"rows": []}), json!({"rows": "x"}), json!({"rows": [1, null]})] {
        assert_eq!(s(a)["state"], json!("unknown"));
    }
}

#[test]
fn unknown_verbs_and_missing_arguments_are_errors_with_text() {
    assert!(call("nope", &json!({})).unwrap_err().contains("nope"));
    // `parse` of nothing is the unreadable empty action, not a panic.
    assert_eq!(call("parse", &json!({})).unwrap()["valid"], json!(false));
    assert_eq!(call("parse", &json!({"spec": 5})).unwrap()["valid"], json!(false));
}
