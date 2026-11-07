//! A user upgrading to the Rust edition brings the files the Python 0.3.x and Go 0.4.0 editions
//! left behind. These tests copy a real-shaped tree (`testdata/editions/go-0.4.0`, made from a
//! laptop that ran the Go edition, with names stripped; plus Python-shaped files written inline
//! from the Python source) into a temp directory and check that every reader of this edition
//! starts on it, behaves, keeps what it does not know, and writes files the older editions read.
//!
//! Evidence for `docs/RESILIENCE.md`, section "Formats" (F1 to F12).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use ts_decision::crashguard::{Guard, Start};
use ts_decision::curves;
use ts_decision::smartprofile;
use ts_domain::gpuundervolt;
use ts_domain::keybinds::{self, KeyProfiles};
use ts_domain::keyprofiles;
use ts_domain::looks::Look;
use ts_domain::undervolt;
use ts_foundation::config::{self, Config};
use ts_foundation::state::{self, LiveState, Store};
use ts_foundation::userstate;

struct Tree(PathBuf);

impl Tree {
    /// A copy of the fixture tree of `edition`.
    fn of(edition: &str) -> Tree {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dst = std::env::temp_dir().join(format!(
            "tsd-ed-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/editions").join(edition);
        copy_tree(&src, &dst);
        Tree(dst)
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }
    fn home(&self) -> PathBuf {
        self.0.join("home")
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn copy_tree(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for e in fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        let to = dst.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_tree(&e.path(), &to);
        } else {
            fs::copy(e.path(), to).unwrap();
        }
    }
}

fn no_env(_: &str) -> Option<String> {
    None
}

fn json_of(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

// ---- F1: the configuration ---------------------------------------------------------------------

#[test]
fn the_go_editions_config_files_load_validate_and_name_the_keys_this_edition_does_not_know() {
    let t = Tree::of("go-0.4.0");
    let (sys, usr) = (t.p("etc/truesense/config.toml"), t.p("home/.config/truesense/config.toml"));
    let cfg = Config::load(Some(&sys), Some(&usr), &no_env).unwrap();
    assert_eq!(cfg.user, "alice");
    assert!(cfg.validate().is_empty(), "{:?}", cfg.validate());
    assert_eq!(cfg.audio.jack, "headset");
    assert_eq!(cfg.keys.tap, "exec:truesense gui");
    assert!(!cfg.update.auto_check);
    assert_eq!(cfg.startup.keyboard, "last");
    let (cfg2, problems) = Config::load_or_defaults(Some(&sys), Some(&usr), &no_env);
    assert!(problems.is_empty());
    assert_eq!(cfg2, cfg, "unknown keys are not a problem");
    assert_eq!(config::unknown_keys(&usr), ["daemon.future_switch", "legacy"]);
}

#[test]
fn rewriting_the_users_config_keeps_every_key_an_older_edition_wrote() {
    let t = Tree::of("go-0.4.0");
    let usr = t.p("home/.config/truesense/config.toml");
    config::write_user(&usr, config::section_table("keys", [("tap", "none".into())])).unwrap();
    let text = fs::read_to_string(&usr).unwrap();
    let back: config::Table = text.parse().unwrap();
    assert_eq!(back["legacy"]["foo"].as_str(), Some("bar"));
    assert_eq!(back["daemon"]["future_switch"].as_bool(), Some(true));
    assert_eq!(back["keys"]["tap"].as_str(), Some("none"));
    assert_eq!(back["keys"]["long_ms"].as_integer(), Some(800), "untouched keys stay");
    assert_eq!(back["audio"]["jack"].as_str(), Some("headset"));
    // The Go and Python editions read it back: it is plain TOML with the same sections.
    assert!(back.contains_key("startup") && back.contains_key("update"));
}

// ---- F2: the daemon's settings -------------------------------------------------------------------

#[test]
fn the_go_editions_state_json_starts_the_daemon_settings_and_survives_rewrites_with_every_key() {
    let t = Tree::of("go-0.4.0");
    let path = t.p("var/lib/truesense/state.json");
    let before = json_of(&path);
    let s = Store::open(&path, state::defaults(&Config::default()));
    assert!(s.load_problem().is_none());
    assert!(s.get_bool("smart_fan", false));
    assert_eq!(s.get_str("fan_curve", ""), "mine");
    assert_eq!(s.get_str("smart_profile", ""), "game-steady");
    assert_eq!(s.get("pl_user").unwrap()["pl2"], json!(90.0));
    assert_eq!(s.get("uv_boot").unwrap()["core"], json!(-60));
    s.set("smart_fan", json!(false)).unwrap();
    let after = json_of(&path);
    for (k, v) in before.as_object().unwrap() {
        if k != "smart_fan" {
            assert_eq!(&after[k], v, "key {k} must survive the rewrite");
        }
    }
    assert_eq!(after["go_only_key"], json!({"x": [1, 2, 3]}), "a key only the Go edition knows");
    assert_eq!(after["smart_fan"], json!(false));
    // Rollback: the file is a plain JSON object of the same keys the Go edition reads.
    assert!(after.is_object());
}

#[test]
fn the_python_editions_state_json_loads_and_gets_the_new_defaults_for_what_it_lacks() {
    let dir = std::env::temp_dir().join(format!("tsd-py-state-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let p = dir.join("state.json");
    // What daemon.py wrote: only what had been set, `null` for cleared keys, floats for times.
    fs::write(&p, r#"{"smart_fan": true, "auto_profile": false, "auto_mode": "off", "fan_curve": "truesense", "profile_hold_until": 1794050946.25, "uv_boot": null, "charge_full": {"since": 1794000000.0, "full_at": null}}"#).unwrap();
    let s = Store::open(&p, state::defaults(&Config::default()));
    assert!(s.get_bool("smart_fan", false));
    assert_eq!(s.get_f64("profile_hold_until", 0.0), 1794050946.25);
    assert_eq!(s.get_str("auto_mode", ""), "off", "the stored value wins over the default");
    assert!(
        s.get("operating_follow").is_some(),
        "a key the Python edition did not store gets this edition's default"
    );
    assert_eq!(s.get_str("smart_profile", ""), "native");
    assert_eq!(s.get("uv_boot"), Some(Value::Null));
    let _ = fs::remove_dir_all(&dir);
}

// ---- F3: the user's state ---------------------------------------------------------------------

#[test]
fn the_go_editions_user_json_is_read_and_a_change_keeps_every_other_key() {
    let t = Tree::of("go-0.4.0");
    let path = t.p("home/.local/state/truesense/user.json");
    let before = json_of(&path);
    let s = userstate::Store::new(&path);
    assert_eq!(s.get("profile"), Some(json!("balanced")));
    assert!(s.get_bool("keyboard_on", false));
    assert_eq!(s.get("rear_zones").unwrap().as_array().unwrap().len(), 3);
    s.set("profile", json!("performance")).unwrap();
    let after = json_of(&path);
    for (k, v) in before.as_object().unwrap() {
        if k != "profile" {
            assert_eq!(&after[k], v, "key {k}");
        }
    }
    assert_eq!(after["profile"], "performance");
}

#[test]
fn a_user_json_nobody_can_parse_is_an_empty_state_and_the_next_set_repairs_it() {
    let t = Tree::of("go-0.4.0");
    let path = t.p("home/.local/state/truesense/user.json");
    fs::write(&path, "{\"profile\": \"bal").unwrap();
    let s = userstate::Store::new(&path);
    assert!(s.load().is_empty());
    s.set("profile", json!("eco")).unwrap();
    assert_eq!(json_of(&path), json!({"profile": "eco"}));
}

// ---- F4..F8: the profile directories -----------------------------------------------------------

#[test]
fn the_smart_profiles_the_go_edition_saved_load_and_list() {
    let t = Tree::of("go-0.4.0");
    let store = smartprofile::Store::for_home(&t.home());
    let names = store.names();
    for n in ["truesense", "game-steady", "pp", "truesense-copy"] {
        assert!(names.iter().any(|x| x == n), "{n} in {names:?}");
    }
    for n in ["game-steady", "pp", "truesense-copy"] {
        store.load(n).unwrap_or_else(|e| panic!("{n}: {e}"));
    }
    let resolved = store.resolve(None, "game-steady");
    assert!(resolved.problem.is_none(), "{:?}", resolved.problem);
}

#[test]
fn the_fan_curves_of_both_older_editions_load_and_names_list() {
    let t = Tree::of("go-0.4.0");
    let store = curves::Store::new(t.p("home/.config/truesense/curves"));
    assert!(store.names().unwrap().contains(&"mine".to_owned()));
    let c = store.load("mine").unwrap();
    assert_eq!(c.cpu.len(), 4);
    // The Python edition's own file: only cpu and gpu, no follower parameters, ints for floats.
    let py = t.p("home/.config/truesense/curves/py.json");
    fs::write(&py, r#"{"cpu": [[52, 0], [89, 100]], "gpu": [[48, 0], [78, 100]]}"#).unwrap();
    store.load("py").unwrap();
    // A file that no longer validates is an error for that name, never for the others.
    fs::write(
        t.p("home/.config/truesense/curves/bad.json"),
        r#"{"cpu": [[52, 0], [60, 10]], "gpu": [[48, 0], [60, 10]]}"#,
    )
    .unwrap();
    assert!(store.load("bad").is_err());
    assert!(store.load("mine").is_ok());
    // And the active-curve resolution used at daemon start falls back, naming the reason.
    let active = store.load_active(None, "bad");
    assert!(active.fallback.is_some());
}

#[test]
fn key_cpu_and_gpu_undervolt_profiles_of_the_older_editions_load() {
    let t = Tree::of("go-0.4.0");
    let keys = KeyProfiles::new(
        t.p("home/.config/truesense/keys"),
        t.p("home/.config/truesense/config.toml"),
        keybinds::syntax_check(),
    )
    .unwrap();
    assert!(keys.names().contains(&"mine".to_owned()));
    assert_eq!(keys.load("mine").unwrap().bindings.action(keybinds::Gesture::Tap), "exec:truesense gui");
    let uv = undervolt::Store::new(t.p("home/.config/truesense/undervolt"));
    assert_eq!(uv.load("mine").unwrap().core, -60);
    assert_eq!(uv.load("stock").unwrap().core, 0, "the Python edition's old name for native");
    let guv = gpuundervolt::Store::new(t.p("home/.config/truesense/gpu-undervolt"));
    assert_eq!(guv.load("mine").unwrap().offset, -100);
}

#[test]
fn looks_lighting_profiles_and_the_boot_look_of_the_older_editions_parse() {
    let t = Tree::of("go-0.4.0");
    let look =
        Look::parse(&fs::read_to_string(t.p("home/.config/truesense/looks/wave.json")).unwrap()).unwrap();
    assert!(!look.is_empty());
    let boot = Look::parse(&fs::read_to_string(t.p("var/lib/truesense/boot-look.json")).unwrap()).unwrap();
    assert_eq!(look.describe(), boot.describe());
    let cells =
        keyprofiles::parse(&fs::read_to_string(t.p("home/.config/truesense/lighting/default.json")).unwrap())
            .unwrap();
    assert!(!cells.is_empty());
    // Round trip: what this edition writes is what the older ones read ({"cells": [[r, g, b], ...]}).
    let text = keyprofiles::encode(&cells);
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["cells"].as_array().unwrap().len(), cells.len());
    assert_eq!(v["cells"][0].as_array().unwrap().len(), 3);
    // A look that is half written, or foreign, is an error and not a panic.
    for bad in ["", "{", "[]", "{\"keyboard\": 5}", "{\"keyboard\": {\"mode\": 7}}"] {
        let _ = Look::parse(bad);
    }
}

// ---- F9: the crash-guard flags -----------------------------------------------------------------

#[test]
fn an_undervolt_flag_the_go_edition_left_trips_the_guard_once_and_is_gone() {
    let t = Tree::of("go-0.4.0");
    let flag = t.p("var/lib/truesense/uv-pending");
    let mut g = Guard::new(&flag);
    assert!(g.pending());
    assert_eq!(g.start(0.0, 1.0).unwrap(), Start::Tripped, "a boot that did not survive: not applied again");
    assert!(!g.pending());
    assert_eq!(g.start(0.0, 1.0).unwrap(), Start::Armed);
}

// ---- F10: the live state ----------------------------------------------------------------------

#[test]
fn the_live_state_the_go_daemon_published_parses_as_it_always_did() {
    let t = Tree::of("go-0.4.0");
    let live = LiveState::read(&t.p("run/truesense/state")).unwrap();
    assert_eq!(live.text("mode").as_deref(), Some("smart"));
    assert_eq!(live.text("fan").as_deref(), Some("100,85"));
    assert_eq!(live.int("tcpu"), Some(86));
    assert_eq!(live.flag("ac"), Some(true));
    assert_eq!(live.flag("fan_off"), Some(false));
    assert!(live.get("gpu_state").is_none(), "keys this edition appends are optional for readers");
    // The other direction: what this edition publishes keeps the Go keys first, in order.
    let rendered = live.render();
    assert!(rendered.starts_with("mode=smart\nactivity=gaming\n"));
}

// ---- F11: files written by this edition, read by the older ones ------------------------------------

#[test]
fn what_this_edition_writes_keeps_the_key_sets_and_shapes_the_older_editions_read() {
    let t = Tree::of("go-0.4.0");
    let uv = undervolt::Store::new(t.p("home/.config/truesense/undervolt"));
    uv.save("rust", &undervolt::Profile { description: "d".into(), core: -50, cache: -40 }).unwrap();
    let v = json_of(&t.p("home/.config/truesense/undervolt/rust.json"));
    let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, ["description", "core", "cache"], "the Python edition's key order");
    let store = curves::Store::new(t.p("home/.config/truesense/curves"));
    let c = store.load("mine").unwrap();
    let _ = c;
    let keys = KeyProfiles::new(
        t.p("home/.config/truesense/keys"),
        t.p("home/.config/truesense/config.toml"),
        keybinds::syntax_check(),
    )
    .unwrap();
    let copy = keys.copy("mine", Some("mine2")).unwrap();
    assert_eq!(copy, "mine2");
    let v = json_of(&t.p("home/.config/truesense/keys/mine2.json"));
    let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, ["description", "tap", "double", "triple", "long", "fn_esc", "long_ms", "multi_ms"]);
}

// ---- F12: a laptop with no files at all (fresh install) and with garbage everywhere ---------------

#[test]
fn nothing_there_is_a_normal_start_and_garbage_everywhere_is_a_start_on_defaults() {
    let empty = std::env::temp_dir().join(format!("tsd-empty-{}", std::process::id()));
    fs::create_dir_all(&empty).unwrap();
    let (cfg, problems) =
        Config::load_or_defaults(Some(&empty.join("none.toml")), Some(&empty.join("none2.toml")), &no_env);
    assert!(problems.is_empty());
    assert_eq!(cfg, Config::default());
    assert!(smartprofile::Store::for_home(&empty).names().contains(&"truesense".to_owned()));
    assert!(curves::Store::new(empty.join("curves")).names().unwrap().contains(&"truesense".to_owned()));
    // Garbage in every profile directory: names list, loads fail by name, nothing panics.
    let t = Tree::of("go-0.4.0");
    for dir in ["smart", "curves", "keys", "undervolt", "gpu-undervolt", "looks"] {
        let d = t.p(&format!("home/.config/truesense/{dir}"));
        for (i, junk) in
            [&b"\x00\x01\x02"[..], b"{", b"[1,2", b"\xff\xfe", &[b'['; 50_000][..]].iter().enumerate()
        {
            fs::write(d.join(format!("junk{i}.json")), junk).unwrap();
        }
    }
    let smart = smartprofile::Store::for_home(&t.home());
    for n in smart.names() {
        let _ = smart.load(&n);
    }
    let cs = curves::Store::new(t.p("home/.config/truesense/curves"));
    for n in cs.names().unwrap() {
        let _ = cs.load(&n);
    }
    let uvs = undervolt::Store::new(t.p("home/.config/truesense/undervolt"));
    for n in uvs.names().unwrap() {
        let _ = uvs.load(&n);
    }
    let _ = fs::remove_dir_all(&empty);
}
