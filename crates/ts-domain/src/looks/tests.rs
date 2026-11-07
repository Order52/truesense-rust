#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde_json::{Value, json};
use ts_platform::firmware::{Device, Simulator};

use super::*;
use crate::keymap::testutil::TempDir;

// ---- fakes ----------------------------------------------------------------------------------

#[derive(Default)]
struct FakeKeyboard {
    last: Mutex<keyboard::State>,
    calls: Mutex<Vec<String>>,
    /// What `replay` answers; `None` is "nothing to restore".
    replay_result: Mutex<Option<String>>,
    fail: bool,
}

impl KeyboardControl for FakeKeyboard {
    fn last(&self) -> keyboard::State {
        self.last.lock().unwrap().clone()
    }

    fn replay(&self, state: &keyboard::State) -> Result<Option<String>, KeyboardError> {
        self.calls.lock().unwrap().push(format!("replay {}", state.mode));
        if self.fail {
            return Err(KeyboardError::NotFound);
        }
        Ok(self.replay_result.lock().unwrap().clone())
    }

    fn off(&self) -> Result<(), KeyboardError> {
        self.calls.lock().unwrap().push("off".into());
        Ok(())
    }
}

#[derive(Default)]
struct FakeRear {
    sent: Mutex<Vec<RearSettings>>,
    fail: bool,
}

impl RearControl for FakeRear {
    fn rear_set(&self, settings: &RearSettings) -> Result<HwRear, HwError> {
        self.sent.lock().unwrap().push(settings.clone());
        if self.fail {
            return Err(HwError::Invalid("the bar is away".into()));
        }
        Ok(HwRear {
            raw: None,
            mode: ts_platform::hw::Reading::Known(settings.mode),
            speed: 3,
            brightness: 100,
            direction: None,
            color: settings.color,
            zones: None,
        })
    }
}

struct Rig {
    dir: TempDir,
    store: LookStore,
    kb: Arc<FakeKeyboard>,
    rear: Arc<FakeRear>,
    state: Arc<Store>,
}

fn rig() -> Rig {
    rig_with(FakeKeyboard::default(), FakeRear::default())
}

fn rig_with(kb: FakeKeyboard, rear: FakeRear) -> Rig {
    let dir = TempDir::new("looks");
    let state = Arc::new(Store::new(dir.join("user.json")));
    let (kb, rear) = (Arc::new(kb), Arc::new(rear));
    let store = LookStore::new(
        dir.join("looks"),
        dir.join("var/boot-look.json"),
        Arc::clone(&state),
        Arc::clone(&kb) as Arc<dyn KeyboardControl>,
        Arc::clone(&rear) as Arc<dyn RearControl>,
    );
    Rig { dir, store, kb, rear, state }
}

fn golden() -> Value {
    serde_json::from_str(include_str!("../../testdata/looks_golden.json")).expect("golden json")
}

fn look_of(v: &Value) -> Look {
    serde_json::from_value(v.clone()).expect("look")
}

fn effect_look() -> Look {
    look_of(
        &json!({"keyboard": {"mode": "effect", "name": "wave", "color": "00ff00", "speed": 4, "brightness": 20, "direction": 0},
                    "rear": {"mode": "breathing", "color": "ff0000", "speed": 2, "brightness": 80, "direction": 1, "zones": null}}),
    )
}

// ---- Python's behaviour ---------------------------------------------------------------------

#[test]
fn describe_matches_python() {
    for pair in golden()["describe"].as_array().unwrap() {
        assert_eq!(look_of(&pair[0]).describe(), pair[1].as_str().unwrap(), "{}", pair[0]);
    }
}

#[test]
fn the_builtin_native_look_is_what_predatorsense_leaves_on_windows() {
    let r = rig();
    let python = golden()["builtin"].clone();
    let native = r.store.load("native").unwrap();
    assert_eq!(serde_json::to_value(&native).unwrap(), python);
    let k = native.keyboard.as_ref().unwrap();
    assert_eq!((k.mode.as_str(), k.brightness, k.cells.len()), ("perkey", Some(12), 128));
    assert!(k.cells[..126].iter().all(|c| c == "00aec7") && k.cells[126..].iter().all(|c| c == "000000"));
    assert_eq!(native.rear.as_ref().unwrap().color, "3723c7");
    assert_eq!(r.store.names(), ["native"]);
}

#[test]
fn saved_files_are_what_python_writes() {
    let r = rig();
    for pair in golden()["files"].as_array().unwrap() {
        let look = look_of(&pair[0]);
        // An empty `{}` part and a rear part without direction/zones keys cannot be written back
        // key for key (this edition writes the keys it knows); those compare as values below.
        let simple = look.rear.as_ref().is_none_or(|r| !r.is_empty() && r.direction.is_some());
        r.store.save("probe", Some(&look)).unwrap();
        let text = std::fs::read_to_string(r.dir.join("looks/probe.json")).unwrap();
        if simple {
            assert_eq!(text, pair[1].as_str().unwrap(), "{}", pair[0]);
        }
        assert_eq!(r.store.load("probe").unwrap(), look);
        assert_eq!(Look::parse(pair[1].as_str().unwrap()).unwrap(), look, "Python's file loads");
    }
}

#[test]
fn errors_and_names_match_python() {
    let g = golden();
    let r = rig();
    let want = g["errors"].as_object().unwrap();
    let err = |label: &str| want[label].as_str().unwrap().to_owned();
    let empty = Look::default();
    assert_eq!(r.store.save("native", Some(&empty)).unwrap_err().to_string(), err("save native"));
    assert_eq!(r.store.save("bad/name", Some(&empty)).unwrap_err().to_string(), err("save bad"));
    assert_eq!(r.store.save(&"x".repeat(41), Some(&empty)).unwrap_err().to_string(), err("save long"));
    assert_eq!(r.store.load("nosuch").unwrap_err().to_string(), err("load none"));
    assert_eq!(r.store.load("a b").unwrap_err().to_string(), err("load bad"));
    assert_eq!(r.store.delete("native").unwrap_err().to_string(), err("delete native"));
    assert_eq!(r.store.delete("nosuch").unwrap_err().to_string(), err("delete none"));
    assert_eq!(r.store.delete("a/b").unwrap_err().to_string(), err("delete bad"));
}

#[test]
fn names_and_copies_follow_python() {
    let g = golden();
    let r = rig();
    for i in 0..6 {
        r.store.save(&format!("look{i}"), Some(&look_of(&g["files"][i][0]))).unwrap();
    }
    assert_eq!(r.store.names().len(), 7);
    assert_eq!(r.store.names()[..3], ["native", "look0", "look1"]);
    let copies = vec![
        r.store.copy("look0", None).unwrap(),
        r.store.copy("look0", None).unwrap(),
        r.store.copy("look0", Some("mine")).unwrap(),
        r.store.copy("look0", Some("mine")).unwrap(),
        r.store.copy("native", None).unwrap(),
    ];
    let want: Vec<&str> = g["copies"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(copies, want);
    let names_after: Vec<String> =
        g["names_after"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
    assert_eq!(r.store.names(), names_after);
    assert_eq!(r.store.load("mine-2").unwrap(), r.store.load("look0").unwrap());
}

// ---- file handling ---------------------------------------------------------------------------

#[test]
fn save_load_delete_round_trip() {
    let r = rig();
    assert_eq!(r.store.names(), ["native"], "no directory yet");
    let look = effect_look();
    assert_eq!(r.store.save("calm", Some(&look)).unwrap(), look);
    assert_eq!(r.store.load("calm").unwrap(), look);
    assert_eq!(r.store.names(), ["native", "calm"]);
    r.store.delete("calm").unwrap();
    assert_eq!(r.store.names(), ["native"]);
    assert!(r.store.delete("calm").is_err());
}

#[test]
fn a_user_file_named_like_a_builtin_is_not_listed_and_the_builtin_wins() {
    let r = rig();
    std::fs::create_dir_all(r.dir.join("looks")).unwrap();
    std::fs::write(r.dir.join("looks/native.json"), r#"{"keyboard": null, "rear": null}"#).unwrap();
    std::fs::write(r.dir.join("looks/zeta.json"), "{}").unwrap();
    std::fs::write(r.dir.join("looks/alpha.json"), "{}").unwrap();
    std::fs::write(r.dir.join("looks/notes.txt"), "x").unwrap();
    assert_eq!(r.store.names(), ["native", "alpha", "zeta"]);
    assert!(r.store.load("native").unwrap().keyboard.is_some());
}

#[test]
fn looks_written_by_older_editions_load_whatever_parts_they_have() {
    let r = rig();
    std::fs::create_dir_all(r.dir.join("looks")).unwrap();
    let write =
        |name: &str, text: &str| std::fs::write(r.dir.join(format!("looks/{name}.json")), text).unwrap();
    write(
        "kbonly",
        r#"{"keyboard": {"mode": "effect", "name": "wave", "color": "00ff00", "speed": 4, "brightness": 20, "direction": 0}, "rear": null}"#,
    );
    write(
        "rearonly",
        r#"{"rear": {"mode": "static", "color": "ff0000", "speed": 3, "brightness": 100, "direction": 1, "zones": ["ff0000","00ff00","0000ff"]}}"#,
    );
    write("unknown-fields", r#"{"keyboard": {"mode": "off", "future": 1}, "extra": true}"#);
    assert!(r.store.load("kbonly").unwrap().rear.is_none());
    assert_eq!(r.store.load("rearonly").unwrap().rear.unwrap().zones.unwrap().len(), 3);
    assert_eq!(r.store.load("unknown-fields").unwrap().keyboard.unwrap().mode, "off");
}

#[test]
fn corrupt_look_files_are_errors_that_name_the_look() {
    let r = rig();
    std::fs::create_dir_all(r.dir.join("looks")).unwrap();
    for (name, text) in [
        ("junk", "{nope"),
        ("array", "[1, 2]"),
        ("badkb", r#"{"keyboard": {"mode": 5}}"#),
        ("empty", ""),
        ("big", &"x".repeat(2 << 20)),
    ] {
        std::fs::write(r.dir.join(format!("looks/{name}.json")), text).unwrap();
        let err = r.store.load(name).unwrap_err();
        assert!(matches!(err, LooksError::Parse { .. } | LooksError::File { .. }), "{name}: {err}");
        if name != "big" {
            assert!(err.to_string().contains(&format!("look '{name}'")), "{err}");
        }
    }
}

#[test]
fn look_names_are_validated() {
    let r = rig();
    for ok in ["a", "My.look_1-x", &"x".repeat(40), ".", ".."] {
        assert!(valid_name(ok), "{ok}");
    }
    for bad in ["", &"x".repeat(41), "a b", "a/b", "a\\b", "é", "a\n", "ä-ö", "../x"] {
        assert!(!valid_name(bad), "{bad:?}");
        assert!(r.store.load(bad).is_err());
        assert!(r.store.delete(bad).is_err());
        assert!(r.store.save(bad, Some(&effect_look())).is_err());
    }
    assert!(
        !r.dir.join("looks").exists() || r.store.names() == ["native"],
        "nothing was written for a bad name"
    );
}

// ---- capture ---------------------------------------------------------------------------------

#[test]
fn capture_and_save_current_lights() {
    let r = rig();
    assert_eq!(
        r.store.save("now", None).unwrap_err().to_string(),
        "nothing to save yet: set the keyboard or rear light first"
    );
    assert!(r.store.capture().is_err());
    *r.kb.last.lock().unwrap() = serde_json::from_value(json!({"mode": "effect", "name": "wave", "color": "00ff00", "speed": 4, "brightness": 20, "direction": 0})).unwrap();
    let look = r.store.capture().unwrap();
    assert!(look.keyboard.is_some() && look.rear.is_none());
    r.state
        .set("rear_state", json!({"mode": "static", "color": "ff0000", "speed": 3, "brightness": 90, "direction": 1, "zones": ["ff0000", "00ff00", "0000ff"]}))
        .unwrap();
    let saved = r.store.save("now", None).unwrap();
    assert_eq!(saved.rear.as_ref().unwrap().zones.as_ref().unwrap()[2], "0000ff");
    assert_eq!(r.store.load("now").unwrap(), saved);
    // Only the rear light set: the keyboard part is absent, not an empty object.
    *r.kb.last.lock().unwrap() = keyboard::State::default();
    let rear_only = r.store.capture().unwrap();
    assert!(rear_only.keyboard.is_none() && rear_only.rear.is_some());
    // An empty or malformed remembered rear state is nothing.
    for bad in [json!({}), json!("x"), json!(5), json!({"speed": "fast"})] {
        r.state.set("rear_state", bad).unwrap();
        assert!(r.store.capture().is_err());
    }
}

// ---- apply -----------------------------------------------------------------------------------

#[test]
fn apply_sends_both_parts_and_reports_each() {
    let r = rig();
    *r.kb.replay_result.lock().unwrap() = Some("effect wave".into());
    let done = r.store.apply(&effect_look()).unwrap();
    assert_eq!(done, ["keyboard: effect wave", "rear: breathing"]);
    assert_eq!(*r.kb.calls.lock().unwrap(), ["replay effect"]);
    let sent = r.rear.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0],
        RearSettings {
            mode: RearMode::Breathing,
            color: Rgb([0xff, 0, 0]),
            speed: 2,
            brightness: 80,
            direction: 1,
            zones: None,
            verify: true,
        }
    );
}

#[test]
fn apply_handles_off_and_partial_looks() {
    let r = rig();
    let off = look_of(&json!({"keyboard": {"mode": "off"}}));
    assert_eq!(r.store.apply(&off).unwrap(), ["keyboard: off"]);
    assert_eq!(*r.kb.calls.lock().unwrap(), ["replay off", "off"]);
    assert!(r.rear.sent.lock().unwrap().is_empty(), "the rear part was left alone");
    r.kb.calls.lock().unwrap().clear();
    let nothing = look_of(&json!({"keyboard": {"mode": "effect", "name": "nosuch"}}));
    assert_eq!(r.store.apply(&nothing).unwrap(), ["keyboard: nothing to restore"]);
    for empty in [Look::default(), look_of(&json!({"keyboard": {}, "rear": {}}))] {
        r.kb.calls.lock().unwrap().clear();
        assert_eq!(r.store.apply(&empty).unwrap(), Vec::<String>::new());
        assert!(r.kb.calls.lock().unwrap().is_empty());
    }
    let rear_only = look_of(
        &json!({"rear": {"mode": "static", "color": "ff0000", "speed": 3, "brightness": 100, "zones": ["ff0000"]}}),
    );
    assert_eq!(r.store.apply(&rear_only).unwrap(), ["rear: static"]);
    let sent = r.rear.sent.lock().unwrap()[0].clone();
    assert_eq!((sent.direction, sent.zones), (1, Some(vec![Rgb([0xff, 0, 0])])), "an absent direction is 1");
}

#[test]
fn apply_refuses_an_incomplete_or_invalid_rear_part_without_sending_zeros() {
    let r = rig();
    for part in [
        json!({"mode": "static", "color": "ff0000", "speed": 3}),
        json!({"mode": "static", "color": "ff0000", "brightness": 3}),
        json!({"mode": "static", "speed": 3, "brightness": 3}),
        json!({"color": "ff0000", "speed": 3, "brightness": 3}),
        json!({"brightness": 0}),
    ] {
        let err = r.store.apply(&look_of(&json!({ "rear": part }))).unwrap_err();
        assert_eq!(err.to_string(), "the rear part of this look needs mode, color, speed and brightness");
    }
    let bad_mode =
        look_of(&json!({"rear": {"mode": "disco", "color": "ff0000", "speed": 3, "brightness": 3}}));
    assert_eq!(
        r.store.apply(&bad_mode).unwrap_err().to_string(),
        "rear mode: static, breathing, neon, wave, twinkling"
    );
    let bad_color = look_of(&json!({"rear": {"mode": "wave", "color": "zz", "speed": 3, "brightness": 3}}));
    assert_eq!(r.store.apply(&bad_color).unwrap_err().to_string(), "color must be RRGGBB hex");
    let bad_zone = look_of(
        &json!({"rear": {"mode": "static", "color": "ff0000", "speed": 3, "brightness": 3, "zones": ["ff0000", "x"]}}),
    );
    assert!(r.store.apply(&bad_zone).is_err());
    assert!(r.rear.sent.lock().unwrap().is_empty());
}

#[test]
fn apply_stops_at_the_first_failure() {
    let r = rig_with(FakeKeyboard { fail: true, ..FakeKeyboard::default() }, FakeRear::default());
    assert!(matches!(r.store.apply(&effect_look()), Err(LooksError::Keyboard(KeyboardError::NotFound))));
    assert!(r.rear.sent.lock().unwrap().is_empty(), "the rear part is not tried after the keyboard failed");
    let r = rig_with(FakeKeyboard::default(), FakeRear { fail: true, ..FakeRear::default() });
    let err = r.store.apply(&effect_look()).unwrap_err();
    assert!(matches!(err, LooksError::Rear(_)));
    assert_eq!(err.to_string(), "the bar is away");
}

/// The place where the hw layer remembers the rear light: the user state.
struct StateMemory(Arc<Store>);

impl ts_platform::hw::RearMemory for StateMemory {
    fn remember(&self, key: &str, value: Value) {
        let _ = self.0.set(key, value);
    }
}

#[test]
fn the_real_keyboard_and_firmware_take_a_look_end_to_end() {
    let dir = TempDir::new("looks-e2e");
    let state = Arc::new(Store::new(dir.join("user.json")));
    let (kb, sim_kb) = Keyboard::simulated(Arc::clone(&state));
    let sim_fw = Arc::new(Simulator::new());
    let hw = Hw::new(Arc::clone(&sim_fw) as Arc<dyn Device>)
        .with_sleeper(Arc::new(|_| {}))
        .with_rear_memory(Arc::new(StateMemory(Arc::clone(&state))));
    let store = LookStore::new(
        dir.join("looks"),
        dir.join("boot.json"),
        Arc::clone(&state),
        Arc::new(kb),
        Arc::new(hw),
    );
    let done = store.apply(&store.load("native").unwrap()).unwrap();
    assert_eq!(done, ["keyboard: per-key colours", "rear: breathing"]);
    assert_eq!(sim_kb.brightness(), 12);
    assert_eq!(sim_kb.last_cells().unwrap()[0], [0x00, 0xAE, 0xC7]);
    assert_eq!(sim_fw.snapshot().rear[1], 1, "breathing");
    // ... and what is on the lights can be saved again.
    let saved = store.save("again", None).unwrap();
    assert_eq!(saved.keyboard.unwrap().mode, "perkey");
    let rear = saved.rear.expect("the rear light was remembered by the hw layer");
    assert_eq!(
        (rear.mode.as_str(), rear.color.as_str(), rear.speed, rear.brightness),
        ("breathing", "3723c7", Some(3), Some(100))
    );
}

// ---- the boot look ---------------------------------------------------------------------------

#[test]
fn the_boot_look_is_stored_read_and_cleared() {
    let r = rig();
    assert_eq!(r.store.read_boot(), None);
    r.store.write_boot(None).unwrap();
    let look = effect_look();
    r.store.write_boot(Some(&look)).unwrap();
    assert_eq!(r.store.read_boot(), Some(look.clone()));
    let on_disk: Value =
        serde_json::from_slice(&std::fs::read(r.dir.join("var/boot-look.json")).unwrap()).unwrap();
    assert_eq!(on_disk["keyboard"]["name"], "wave");
    r.store.write_boot(None).unwrap();
    assert_eq!(r.store.read_boot(), None);
    r.store.write_boot(None).unwrap();
}

#[test]
fn an_unreadable_boot_look_is_no_boot_look() {
    let r = rig();
    std::fs::create_dir_all(r.dir.join("var")).unwrap();
    for text in ["", "{nope", "[1]", "null", r#"{"keyboard": 5}"#] {
        std::fs::write(r.dir.join("var/boot-look.json"), text).unwrap();
        assert_eq!(r.store.read_boot(), None, "{text:?}");
    }
    std::fs::write(r.dir.join("var/boot-look.json"), vec![b' '; 2 << 20]).unwrap();
    assert_eq!(r.store.read_boot(), None, "a file past the limit is not read");
}

#[test]
fn the_default_paths() {
    let env =
        |vars: BTreeMap<&'static str, &'static str>| move |k: &str| vars.get(k).map(|v| (*v).to_owned());
    assert_eq!(default_boot_file(&env(BTreeMap::new())), PathBuf::from("/var/lib/truesense/boot-look.json"));
    assert_eq!(
        default_boot_file(&env(BTreeMap::from([("TRUESENSE_BOOT_LOOK", "/tmp/x.json")]))),
        PathBuf::from("/tmp/x.json")
    );
    assert_eq!(default_dir(Path::new("/home/me")), PathBuf::from("/home/me/.config/truesense/looks"));
}

#[test]
fn hostile_look_text_is_an_error_not_a_panic() {
    for text in [
        "",
        "null",
        "5",
        "[]",
        "{",
        r#"{"keyboard": []}"#,
        r#"{"rear": {"speed": "x"}}"#,
        r#"{"rear": {"mode": null}}"#,
        "\u{0}",
        &"[".repeat(10_000),
    ] {
        let _ = Look::parse(text);
    }
    assert!(Look::parse("{}").unwrap().is_empty());
    assert!(!Look::parse(r#"{"rear": {"brightness": 0}}"#).unwrap().is_empty());
    assert!(Look::parse("nope").unwrap_err().to_string().starts_with("look: "));
}

#[test]
fn the_errors_say_what_failed() {
    let io = LooksError::File {
        op: "save look",
        source: FsError::Io { op: "rename to", path: "/x".into(), source: io::Error::from_raw_os_error(13) },
    };
    assert!(io.to_string().starts_with("save look: "));
    assert!(std::error::Error::source(&io).is_some());
    assert!(std::error::Error::source(&LooksError::Invalid("x".into())).is_none());
    assert_eq!(
        LooksError::Keyboard(KeyboardError::NotFound).to_string(),
        KeyboardError::NotFound.to_string()
    );
}
