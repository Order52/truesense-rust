//! Tests of the key binding profiles and the key event mapping.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::err_expect,
    clippy::field_reassign_with_default
)]

use std::path::Path;

use serde_json::json;
use ts_foundation::config::Config;

use super::*;
use crate::profiles::testutil::Tmp;

fn fixture(tmp: &Tmp) -> KeyProfiles {
    KeyProfiles::new(tmp.0.join("keys"), tmp.0.join("config.toml"), syntax_check()).unwrap()
}

fn load_config(path: &Path) -> Config {
    Config::load(None, Some(path), &|_| None).unwrap()
}

fn parse(kp: &KeyProfiles, text: &str) -> Result<KeyProfile, String> {
    match serde_json::from_str::<Value>(text) {
        Ok(v) => kp.validate(&v),
        Err(_) => Err("not valid JSON".into()),
    }
}

#[test]
fn builtins_are_valid_and_match_the_python_sets() {
    let tmp = Tmp::new("kb-builtin");
    let kp = fixture(&tmp);
    let want: [(&str, [&str; 5]); 5] = [
        ("native", ["exec:truesense gui", "none", "none", "none", "none"]),
        ("default", ["power-next", "output-toggle", "none", "fan-max-toggle", "none"]),
        (
            "gaming",
            [
                "power-cycle:performance,turbo",
                "fan-max-toggle",
                "keyboard-effect-next",
                "auto-next",
                "mic-toggle",
            ],
        ),
        (
            "work",
            ["power-cycle:eco,quiet,balanced", "output-toggle", "mic-toggle", "touchpad-toggle", "none"],
        ),
        ("lights", ["keyboard-effect-next", "rear-effect-next", "keyboard:toggle", "rear:toggle", "none"]),
    ];
    assert_eq!(kp.names(), BUILTIN_NAMES);
    for (name, w) in want {
        let p = kp.load(name).unwrap();
        let got: Vec<&str> = Gesture::ALL.iter().map(|g| p.bindings.action(*g)).collect();
        assert_eq!(got, w, "{name}");
        assert_eq!((p.bindings.long_ms, p.bindings.multi_ms), (800, 450), "{name}");
        assert!(!p.description.is_empty(), "{name}");
        assert_eq!(kp.source(name), Some(Source::BuiltIn));
    }
    assert!(builtin("nothere").is_none());
    assert!(kp.store().is_builtin("native"), "native is always built in");
}

#[test]
fn save_load_use_delete_like_the_window_does() {
    // tests/test_gui.py: key profiles are read-only built-ins, saved, used and deleted.
    let tmp = Tmp::new("kb-flow");
    let kp = fixture(&tmp);
    let e = kp
        .save("default", &KeyProfile { description: String::new(), bindings: Bindings::default() })
        .err()
        .unwrap();
    assert!(e.to_string().contains("built in and read-only"), "{e}");
    let mut p = kp.load("default").unwrap();
    p.bindings.tap = "power:turbo".into();
    p.bindings.long_ms = 700;
    kp.save("mine", &p).unwrap();
    let got = kp.load("mine").unwrap();
    assert_eq!((got.bindings.tap.as_str(), got.bindings.long_ms), ("power:turbo", 700));
    assert_eq!(kp.names().last().map(String::as_str), Some("mine"));
    assert_eq!(kp.names().len(), 6);
    assert_eq!(kp.source("mine"), Some(Source::Yours));
    kp.use_profile("mine").unwrap();
    let cur = from_config(&load_config(&tmp.0.join("config.toml")));
    assert_eq!(cur.bindings.tap, "power:turbo");
    assert_eq!(cur.profile, "mine");
    assert_eq!(cur.bindings.long_ms, 700);
    assert_eq!(cur.bindings.double, "output-toggle");
    kp.use_profile("default").unwrap();
    let cur = from_config(&load_config(&tmp.0.join("config.toml")));
    assert_eq!((cur.bindings.tap.as_str(), cur.profile.as_str()), ("power-next", "default"));
    kp.delete("mine").unwrap();
    assert_eq!(kp.delete("mine").err().unwrap().to_string(), "no key profile 'mine'");
    assert!(kp.delete("gaming").err().unwrap().to_string().contains("built in and cannot be deleted"));
}

#[test]
fn use_keeps_the_rest_of_the_user_config() {
    let tmp = Tmp::new("kb-use");
    let kp = fixture(&tmp);
    std::fs::write(
        tmp.0.join("config.toml"),
        "[audio]\nnotify = false\n\n[keys]\npower_cycle = [\"eco\", \"turbo\"]\n",
    )
    .unwrap();
    kp.use_profile("gaming").unwrap();
    let cfg = load_config(&tmp.0.join("config.toml"));
    assert!(!cfg.audio.notify);
    assert_eq!(cfg.keys.power_cycle, ["eco", "turbo"]);
    assert_eq!(cfg.keys.tap, "power-cycle:performance,turbo");
    assert_eq!(cfg.keys.fn_esc, "mic-toggle");
    assert_eq!(cfg.keys.profile, "gaming");
    // A broken config file is an error and is left alone.
    std::fs::write(tmp.0.join("config.toml"), "[keys\n").unwrap();
    assert!(matches!(kp.use_profile("gaming"), Err(KeybindsError::Config(_))));
    assert_eq!(std::fs::read_to_string(tmp.0.join("config.toml")).unwrap(), "[keys\n");
    // Using a missing profile writes nothing.
    assert!(kp.use_profile("nothere").is_err());
}

#[test]
fn loads_files_written_by_python() {
    let tmp = Tmp::new("kb-old");
    let kp = fixture(&tmp);
    std::fs::create_dir_all(tmp.0.join("keys")).unwrap();
    // What Python's json.dump(p, f, indent=1) wrote (ensure_ascii escapes).
    let old = "{\n \"description\": \"my set\",\n \"tap\": \"power:turbo\",\n \"double\": \"exec:echo \\u00e9 > /tmp/x\",\n \"triple\": \"none\",\n \"long\": \"fan-max-toggle\",\n \"fn_esc\": \"none\",\n \"long_ms\": 800,\n \"multi_ms\": 450\n}";
    std::fs::write(tmp.0.join("keys").join("old.json"), old).unwrap();
    let p = kp.load("old").unwrap();
    assert_eq!(p.description, "my set");
    assert_eq!(p.bindings.tap, "power:turbo");
    assert_eq!(p.bindings.double, "exec:echo \u{e9} > /tmp/x");
    // What is written has the same names in the same order and the same indent.
    kp.save("new", &p).unwrap();
    let written = std::fs::read_to_string(tmp.0.join("keys").join("new.json")).unwrap();
    let want = "{\n \"description\": \"my set\",\n \"tap\": \"power:turbo\",\n \"double\": \"exec:echo \u{e9} > /tmp/x\",\n \"triple\": \"none\",\n \"long\": \"fan-max-toggle\",\n \"fn_esc\": \"none\",\n \"long_ms\": 800,\n \"multi_ms\": 450\n}";
    assert_eq!(written, want);
    assert_eq!(kp.load("new").unwrap(), p);
}

#[test]
fn missing_keys_take_defaults() {
    let tmp = Tmp::new("kb-defaults");
    let kp = fixture(&tmp);
    let p = parse(&kp, r#"{"tap":"power:eco"}"#).unwrap();
    assert_eq!(p.bindings.tap, "power:eco");
    assert_eq!(p.bindings.double, "output-toggle");
    assert_eq!(p.bindings.long, "fan-max-toggle");
    assert_eq!((p.bindings.long_ms, p.bindings.multi_ms), (800, 450));
    assert_eq!(p.description, "");
}

#[test]
fn validation_refusals() {
    let tmp = Tmp::new("kb-refuse");
    let kp = fixture(&tmp);
    let table = [
        ("[]", "a key profile is an object"),
        (r#""x""#, "a key profile is an object"),
        ("null", "a key profile is an object"),
        (r#"{"tap":5}"#, "Tap: an action is text"),
        (r#"{"tap":null}"#, "Tap: an action is text"),
        (r#"{"double":"bogus"}"#, "Double tap: unknown action 'bogus'"),
        (r#"{"triple":"power:warp"}"#, "Triple tap: power mode must be one of"),
        (r#"{"long":""}"#, "Long press: unknown action ''"),
        (r#"{"fn_esc":"fan:5"}"#, "Fn + Esc: fan:"),
        (r#"{"long_ms":299}"#, "long_ms: 300..2000"),
        (r#"{"long_ms":2001}"#, "long_ms: 300..2000"),
        (r#"{"multi_ms":199}"#, "multi_ms: 200..1000"),
        (r#"{"multi_ms":1001}"#, "multi_ms: 200..1000"),
        (r#"{"long_ms":800.0}"#, "long_ms: 300..2000"),
        (r#"{"long_ms":"800"}"#, "long_ms: 300..2000"),
        (r#"{"long_ms":true}"#, "long_ms: 300..2000"),
        (r#"{"multi_ms":1e2}"#, "multi_ms: 200..1000"),
        (r#"{"long_ms":99999999999999999999}"#, "long_ms: 300..2000"),
        (r#"{"long_ms":-800}"#, "long_ms: 300..2000"),
        (r#"{"long_ms":null}"#, "long_ms: 300..2000"),
        ("{", "not valid JSON"),
        ("", "not valid JSON"),
        ("{} {}", "not valid JSON"),
    ];
    for (input, want) in table {
        let e = parse(&kp, input).err().unwrap_or_else(|| panic!("{input} should be refused"));
        assert!(e.contains(want), "{input}: got {e:?}, want {want:?}");
    }
    for ok in [
        r#"{"long_ms":300,"multi_ms":200}"#,
        r#"{"long_ms":2000,"multi_ms":1000}"#,
        r#"{"description":5}"#,
        r#"{"extra":1}"#,
    ] {
        assert!(parse(&kp, ok).is_ok(), "{ok}");
    }
}

#[test]
fn the_injected_check_decides_which_actions_are_allowed() {
    let tmp = Tmp::new("kb-check");
    let check: ActionCheck = Arc::new(|spec| {
        if spec == "look:ghost" {
            Err("no saved look 'ghost' (truesense look list)".to_owned())
        } else {
            Ok(())
        }
    });
    let kp = KeyProfiles::new(tmp.0.join("keys"), tmp.0.join("c.toml"), check).unwrap();
    let e = kp.validate(&json!({"tap": "look:ghost"})).err().unwrap();
    assert!(e.contains("Tap: no saved look 'ghost'"), "{e}");
    assert!(kp.validate(&json!({"tap": "anything goes with this check"})).is_ok());
    // The same through an actions validator that knows the looks.
    let v = actions::Validator::with_looks(Arc::new(|| vec!["native".to_owned()]));
    let check = check_with(v);
    assert!(check("look:native").is_ok());
    assert!(check("look:ghost").is_err());
}

#[test]
fn names_cannot_escape() {
    let tmp = Tmp::new("kb-names");
    let kp = fixture(&tmp);
    let p = kp.load("default").unwrap();
    for bad in ["../etc/passwd", "a b", "", "x/y", &"a".repeat(41), "a\0b"] {
        assert!(
            kp.load(bad).err().unwrap().to_string().contains("profile names use letters"),
            "load {bad:?}"
        );
        assert!(kp.save(bad, &p).is_err(), "save {bad:?}");
        assert!(kp.delete(bad).is_err(), "delete {bad:?}");
    }
    for good in ["a", "my.set-1_x", &"a".repeat(40), "..x"] {
        assert!(KIND.rule.accepts(good), "{good}");
    }
    assert!(!tmp.0.join("etc").exists());
}

#[test]
fn load_errors() {
    let tmp = Tmp::new("kb-loaderr");
    let kp = fixture(&tmp);
    assert_eq!(
        kp.load("nothere").err().unwrap().to_string(),
        "no key profile 'nothere' (truesense keys profile list)"
    );
    let dir = tmp.0.join("keys");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("junk.json"), "{not json").unwrap();
    assert_eq!(kp.load("junk").err().unwrap().to_string(), "key profile 'junk' is not valid JSON");
    std::fs::write(dir.join("bad.json"), r#"{"tap":"bogus"}"#).unwrap();
    assert!(kp.load("bad").err().unwrap().to_string().contains("unknown action 'bogus'"));
}

#[test]
fn names_list_only_json_and_hide_shadowed_builtins() {
    let tmp = Tmp::new("kb-list");
    let kp = fixture(&tmp);
    let dir = tmp.0.join("keys");
    std::fs::create_dir_all(&dir).unwrap();
    for f in ["zeta.json", "alpha.json", "gaming.json", "notes.txt", "x.json.tmp"] {
        std::fs::write(dir.join(f), "{}").unwrap();
    }
    let mut want: Vec<String> = BUILTIN_NAMES.iter().map(|s| (*s).to_owned()).collect();
    want.extend(["alpha".to_owned(), "zeta".to_owned()]);
    assert_eq!(kp.names(), want);
    // The shadowed file does not change the built-in.
    assert_eq!(kp.load("gaming").unwrap(), builtin("gaming").unwrap());
}

#[test]
fn copy_numbers_until_free() {
    let tmp = Tmp::new("kb-copy");
    let kp = fixture(&tmp);
    for want in ["gaming-copy", "gaming-copy-2", "gaming-copy-3"] {
        assert_eq!(kp.copy("gaming", None).unwrap(), want);
    }
    assert_eq!(kp.copy("gaming", Some("kcopy")).unwrap(), "kcopy");
    assert_eq!(kp.copy("gaming", Some("kcopy")).unwrap(), "kcopy-2");
    assert_eq!(kp.copy("gaming", Some("default")).unwrap(), "default-2");
    assert!(kp.copy("nothere", None).is_err());
    assert!(kp.copy("gaming", Some("bad name")).is_err());
    assert_eq!(kp.load("kcopy").unwrap().bindings.tap, "power-cycle:performance,turbo");
}

#[test]
fn save_writes_nothing_when_invalid() {
    let tmp = Tmp::new("kb-invalid");
    let kp = fixture(&tmp);
    let bad = KeyProfile {
        description: String::new(),
        bindings: Bindings { tap: "bogus".into(), ..Bindings::default() },
    };
    assert!(kp.save("x", &bad).is_err());
    assert!(!tmp.0.join("keys").exists() || std::fs::read_dir(tmp.0.join("keys")).unwrap().count() == 0);
    // A zero timing (never set) is refused, not silently written.
    let zero =
        KeyProfile { description: String::new(), bindings: Bindings { long_ms: 0, ..Bindings::default() } };
    assert!(kp.save("y", &zero).is_err());
}

#[test]
fn current_defaults_the_profile_name() {
    let mut cfg = Config::default();
    cfg.keys.profile = String::new();
    let a = from_config(&cfg);
    assert_eq!((a.profile.as_str(), a.bindings.tap.as_str()), ("default", "power-next"));
    cfg.keys.profile = "gaming".into();
    assert_eq!(from_config(&cfg).profile, "gaming");
}

#[test]
fn bindings_accessors_and_gestures() {
    let mut b = Bindings::default();
    for g in Gesture::ALL {
        b.set_action(g, format!("x-{}", g.key()));
        assert_eq!(b.action(g), format!("x-{}", g.key()));
        assert_eq!(Gesture::from_key(g.key()), Some(g));
    }
    assert_eq!(Gesture::from_key("nope"), None);
    assert_eq!(Gesture::FnEsc.label(), "Fn + Esc");
    assert_eq!(Gesture::Long.label(), "Long press");
    assert_eq!(Gesture::ALL.map(Gesture::key), ["tap", "double", "triple", "long", "fn_esc"]);
    assert_eq!(Gesture::from_taps(0), None);
    assert_eq!(Gesture::from_taps(1), Some(Gesture::Tap));
    assert_eq!(Gesture::from_taps(2), Some(Gesture::Double));
    assert_eq!(Gesture::from_taps(3), Some(Gesture::Triple));
    assert_eq!(Gesture::from_taps(9), Some(Gesture::Triple));
}

#[test]
fn an_invalid_configured_action_becomes_none_with_a_warning() {
    let check = syntax_check();
    let mut b = Bindings::default();
    b.tap = "frobnicate".into();
    b.fn_esc = "fan:5".into();
    let (clean, warnings) = b.sanitised(&check);
    assert_eq!(clean.tap, "none");
    assert_eq!(clean.fn_esc, "none");
    assert_eq!(clean.double, "output-toggle");
    assert_eq!(
        warnings,
        [
            "keys.tap = 'frobnicate' is invalid (unknown action 'frobnicate'); using 'none'",
            "keys.fn_esc = 'fan:5' is invalid (fan: smart | auto | max | 20..100); using 'none'"
        ]
    );
    let (same, none) = Bindings::default().sanitised(&check);
    assert_eq!(same, Bindings::default());
    assert!(none.is_empty());
}

#[test]
fn a_reload_takes_valid_actions_and_timing_and_ignores_invalid_actions() {
    let check = syntax_check();
    let mut running = Bindings::default();
    let mut new = Bindings::default();
    new.tap = "power:turbo".into();
    new.double = "bogus".into();
    new.long_ms = 1200;
    new.multi_ms = 300;
    running.reload(&new, &check);
    assert_eq!(running.tap, "power:turbo");
    assert_eq!(running.double, "output-toggle", "the invalid one keeps what is running");
    assert_eq!((running.long_ms, running.multi_ms), (1200, 300));
}

#[test]
fn hid_reports_decode_to_events() {
    let table: [(&[u8], Option<KeyEvent>); 12] = [
        (&[0x04, 0x81, 0xff, 0, 0, 0, 0, 0], Some(KeyEvent::PredatorDown)),
        (&[0x04, 0x00, 0x00, 0, 0, 0, 0, 0], Some(KeyEvent::Release)),
        (&[0x04, 0x8a, 0xff, 0, 0, 0, 0, 0], Some(KeyEvent::FnEsc)),
        (&[0x04, 0x80, 0xff, 0, 0, 0, 0, 0], Some(KeyEvent::FnF10)),
        (&[0x04, 0x81, 0xff], Some(KeyEvent::PredatorDown)),
        (&[0x04, 0x82, 0xff, 0, 0, 0, 0, 0], Some(KeyEvent::Unknown)),
        (&[0x04, 0x81, 0x00, 0, 0, 0, 0, 0], Some(KeyEvent::Unknown)),
        (&[0x05, 0x81, 0xff, 0, 0, 0, 0, 0], None),
        (&[0x04, 0x81], None),
        (&[0x04], None),
        (&[], None),
        (&[0xff; 64], None),
    ];
    for (report, want) in table {
        assert_eq!(decode_report(report), want, "{report:02x?}");
    }
}

#[test]
fn events_map_to_actions() {
    let mut b = Bindings::default();
    b.fn_esc = "mic-toggle".into();
    assert_eq!(b.action_for_event(KeyEvent::FnEsc), Some(("fn_esc", "mic-toggle")));
    assert_eq!(b.action_for_event(KeyEvent::FnF10), Some(("fn_f10", "touchpad-toggle")));
    for e in [KeyEvent::PredatorDown, KeyEvent::Release, KeyEvent::Unknown] {
        assert_eq!(b.action_for_event(e), None);
    }
    // Every action a binding or a built-in event can carry is a valid action.
    assert!(actions::validate(FN_F10_ACTION).is_ok());
}

#[test]
fn builtin_profiles_all_validate_with_the_syntax_check() {
    let check = syntax_check();
    for name in BUILTIN_NAMES {
        let p = builtin(name).unwrap();
        assert_eq!(validate(&p.to_value(), &check), Ok(p), "{name}");
    }
}
