#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Mutex;

use serde_json::Value;
use ts_foundation::userstate::Store;

use super::*;
use crate::keymap::testutil::TempDir;

#[derive(Default)]
struct FakeKeyboard {
    uploads: Mutex<Vec<(Vec<Color>, Option<i64>)>>,
    fail: bool,
}

impl PerKeyUpload for FakeKeyboard {
    fn per_key(&self, cells: &[Color], brightness: Option<i64>) -> Result<(), KeyboardError> {
        self.uploads.lock().unwrap().push((cells.to_vec(), brightness));
        if self.fail { Err(KeyboardError::NotFound) } else { Ok(()) }
    }
}

fn rig() -> (TempDir, ProfileStore, Arc<FakeKeyboard>) {
    let dir = TempDir::new("profiles");
    let kb = Arc::new(FakeKeyboard::default());
    let store = ProfileStore::new(dir.join("lighting"), Arc::clone(&kb) as Arc<dyn PerKeyUpload>);
    (dir, store, kb)
}

fn golden() -> Value {
    serde_json::from_str(include_str!("../../testdata/keyprofiles_golden.json")).expect("golden json")
}

fn pattern() -> Vec<Color> {
    (0..128u32).map(|i| [i as u8, (255 - i) as u8, (i * 7 % 256) as u8]).collect()
}

#[test]
fn files_are_byte_for_byte_what_python_writes() {
    let g = golden();
    let (dir, store, _) = rig();
    store.save("full", &pattern()).unwrap();
    store.save("short", &[[1, 2, 3], [4, 5, 6]]).unwrap();
    store.save("a-b_c9", &[]).unwrap();
    for (name, text) in g["files"].as_object().unwrap() {
        assert_eq!(
            std::fs::read_to_string(dir.join(format!("lighting/{name}.json"))).unwrap(),
            text.as_str().unwrap(),
            "{name}"
        );
    }
    assert_eq!(store.load("full").unwrap(), pattern());
}

#[test]
fn the_commands_answer_like_python() {
    let g = golden();
    let (dir, store, kb) = rig();
    // The profiles the Python run had before the command list started.
    store.save("full", &pattern()).unwrap();
    store.save("short", &[[1, 2, 3], [4, 5, 6]]).unwrap();
    store.save("a-b_c9", &[]).unwrap();
    for pair in g["commands"].as_array().unwrap() {
        let args = pair[0].as_array().unwrap();
        let action = args[0].as_str().unwrap();
        let name = args.get(1).and_then(Value::as_str);
        let spec: Vec<&str> = args
            .get(2)
            .map(|s| s.as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect())
            .unwrap_or_default();
        let got = match store.command(action, name, &spec) {
            Ok(CommandOutput::Names(n)) => serde_json::json!(n),
            Ok(CommandOutput::Message(m)) => serde_json::json!(m),
            Err(e) => serde_json::json!(format!("ERR: {e}")),
        };
        assert_eq!(got, pair[1], "{args:?}");
    }
    for (name, text) in g["saved"].as_object().unwrap() {
        assert_eq!(
            std::fs::read_to_string(dir.join(format!("lighting/{name}.json"))).unwrap(),
            text.as_str().unwrap(),
            "{name}"
        );
    }
    assert_eq!(kb.uploads.lock().unwrap().len(), 1, "only the one successful load reached the keyboard");
    assert_eq!(kb.uploads.lock().unwrap()[0].1, None, "a loaded profile keeps the user's brightness");
}

#[test]
fn exists_matches_python() {
    let g = golden();
    let (_dir, store, _) = rig();
    store.save("full", &pattern()).unwrap();
    for pair in g["exists"].as_array().unwrap() {
        assert_eq!(store.exists(pair[0].as_str().unwrap()), pair[1].as_bool().unwrap(), "{pair}");
    }
}

#[test]
fn apply_uploads_the_colours_and_passes_the_brightness_on() {
    let (_dir, store, kb) = rig();
    store.save("p", &pattern()).unwrap();
    store.apply("p", Some(30)).unwrap();
    store.apply("p", None).unwrap();
    let uploads = kb.uploads.lock().unwrap();
    assert_eq!((uploads[0].0.clone(), uploads[0].1), (pattern(), Some(30)));
    assert_eq!(uploads[1].1, None);
}

#[test]
fn a_keyboard_failure_is_reported_as_such() {
    let dir = TempDir::new("profiles-fail");
    let kb = Arc::new(FakeKeyboard { fail: true, ..FakeKeyboard::default() });
    let store = ProfileStore::new(dir.join("l"), kb);
    store.save("p", &[]).unwrap();
    assert!(matches!(store.apply("p", None), Err(ProfileError::Keyboard(KeyboardError::NotFound))));
    assert!(matches!(store.command("load", Some("p"), &[]), Err(ProfileError::Keyboard(_))));
}

#[test]
fn applying_through_the_simulated_keyboard_paints_the_keys() {
    let dir = TempDir::new("profiles-sim");
    let (kb, sim) = Keyboard::simulated(Arc::new(Store::new(dir.join("user.json"))));
    let store = ProfileStore::new(dir.join("lighting"), Arc::new(kb));
    assert!(matches!(store.command("save", Some("rain"), &["rainbow"]), Ok(CommandOutput::Message(_))));
    store.apply("rain", Some(20)).unwrap();
    assert_eq!(sim.last_cells().unwrap(), keymap::rainbow());
    assert_eq!(sim.brightness(), 20);
}

#[test]
fn names_are_validated_like_python() {
    for ok in ["a", "A1", "a-b", "a_b", "-a", "ünï", "日本", "9"] {
        assert!(valid_name(ok), "{ok}");
    }
    for bad in ["", "-", "---", "_-_", "a b", "a/b", "a.b", "../x", "a\n", "a\0", "é!"] {
        assert!(!valid_name(bad), "{bad:?}");
    }
    let (dir, store, _) = rig();
    for bad in ["", "..", "a/../b", "/etc/passwd"] {
        assert!(store.save(bad, &[]).is_err());
        assert!(store.load(bad).is_err());
        assert!(store.delete(bad).is_err());
        assert!(!store.exists(bad));
    }
    assert!(!dir.join("lighting").exists(), "no file or directory was made for a bad name");
}

#[test]
fn names_are_sorted_and_only_json_files() {
    let (dir, store, _) = rig();
    assert_eq!(store.names(), Vec::<String>::new(), "no directory yet");
    store.save("zeta", &[]).unwrap();
    store.save("alpha", &[]).unwrap();
    std::fs::write(dir.join("lighting/notes.txt"), "x").unwrap();
    std::fs::write(dir.join("lighting/.hidden.123.tmp"), "x").unwrap();
    std::fs::create_dir(dir.join("lighting/sub.json")).unwrap();
    assert_eq!(store.names(), ["alpha", "zeta"]);
}

#[test]
fn files_written_by_the_earlier_editions_load() {
    let (dir, store, _) = rig();
    std::fs::create_dir_all(dir.join("lighting")).unwrap();
    std::fs::write(dir.join("lighting/py.json"), r#"{"cells": [[0, 174, 199], [255, 0, 1]]}"#).unwrap();
    std::fs::write(dir.join("lighting/go.json"), r#"{"cells":[[1,2,3]],"extra":true}"#).unwrap();
    assert_eq!(store.load("py").unwrap(), [[0, 174, 199], [255, 0, 1]]);
    assert_eq!(store.load("go").unwrap(), [[1, 2, 3]]);
    let long = format!(r#"{{"cells": [{}]}}"#, vec!["[9, 9, 9]"; 200].join(","));
    std::fs::write(dir.join("lighting/long.json"), long).unwrap();
    assert_eq!(store.load("long").unwrap().len(), 128, "at most 128 cells are kept");
}

#[test]
fn corrupt_profiles_are_refused_whole_and_name_the_profile() {
    let (dir, store, _) = rig();
    std::fs::create_dir_all(dir.join("lighting")).unwrap();
    for (i, text) in [
        "",
        "{nope",
        "[]",
        "null",
        "{}",
        r#"{"cells": null}"#,
        r#"{"cells": 5}"#,
        r#"{"cells": [[1, 2]]}"#,
        r#"{"cells": [[1, 2, 3, 4]]}"#,
        r#"{"cells": [[1, 2, 256]]}"#,
        r#"{"cells": [[-1, 2, 3]]}"#,
        r#"{"cells": [[1, 2, "x"]]}"#,
        r#"{"cells": [[1.5, 2, 3]]}"#,
        r#"{"cells": [[1, 2, 3], 7]}"#,
        r#"{"cells": [[]]}"#,
        r#"{"cells": [[99999999999999999999, 0, 0]]}"#,
    ]
    .iter()
    .enumerate()
    {
        std::fs::write(dir.join(format!("lighting/bad{i}.json")), text).unwrap();
        let err = store.load(&format!("bad{i}")).unwrap_err();
        assert!(
            err.to_string().starts_with(&format!("lighting profile 'bad{i}': parse profile: ")),
            "{text}: {err}"
        );
    }
    std::fs::write(dir.join("lighting/binary.json"), [0xff, 0xfe, 0x00]).unwrap();
    assert!(store.load("binary").is_err());
    std::fs::write(dir.join("lighting/huge.json"), vec![b' '; 2 << 20]).unwrap();
    assert!(matches!(store.load("huge"), Err(ProfileError::File { .. })));
}

#[test]
fn parse_never_panics_on_hostile_text() {
    for text in [
        "\u{0}",
        "{\"cells\": [[",
        &"[".repeat(100_000),
        "{\"cells\": [[1,2,3]]} trailing",
        "{\"cells\": {\"a\": 1}}",
    ] {
        let _ = parse(text);
    }
    assert_eq!(parse(&encode(&[[1, 2, 3]])).unwrap(), [[1, 2, 3]]);
    assert_eq!(encode(&[]), r#"{"cells": []}"#);
}

#[test]
fn the_errors_say_what_failed() {
    assert!(ProfileError::Keyboard(KeyboardError::NotFound).to_string().contains("not found"));
    assert_eq!(
        ProfileError::Colour(KeymapError::BadColour("zz".into())).to_string(),
        "bad colour 'zz', use RRGGBB"
    );
    let e = ProfileError::File {
        op: "save lighting profile",
        source: FsError::Io { op: "rename to", path: "/x".into(), source: io::Error::from_raw_os_error(13) },
    };
    assert!(e.to_string().starts_with("save lighting profile: ") && std::error::Error::source(&e).is_some());
    assert_eq!(default_dir(Path::new("/home/me")), PathBuf::from("/home/me/.config/truesense/lighting"));
}
