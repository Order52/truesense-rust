//! The small per-user memory, `~/.local/state/truesense/user.json`: what the user last set on
//! the lights, which keyboard profile was applied, the window size mode, and similar
//! conveniences.
//!
//! Nothing here is required. Every reader has a default, a missing or corrupt file reads as empty,
//! and a read-only disk is an error the caller may ignore. What it must get right is *concurrency*:
//! the window, the key service and the command line all update the same file, and each update is a
//! read-modify-write. It therefore runs under a process-wide mutex (threads) and an exclusive
//! `flock` on `user.json.lock` (processes), so two writers never lose each other's change.
//!
//! The simulator (`TRUESENSE_SIM`) must never touch the real file: the key service running on the
//! real machine would act on it. In simulator mode the default path is a throwaway directory
//! under the temp directory unless `TRUESENSE_USERSTATE` says otherwise.
#![forbid(unsafe_code)]

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::fsx::{self, FileLock, FsError};
use crate::sync::locked;

/// The keys of `user.json` as written by the Python and Go editions. They are the on-disk
/// format; the shape of each value belongs to the layer that owns it (keyboard, rear lighting,
/// per-key effects), so this module only names them.
pub mod keys {
    /// The keyboard's current look: `{"mode": "off" | "effect" | "perkey" | "operating", ...}`.
    pub const KEYBOARD: &str = "keyboard";
    /// The keyboard look to return to when the backlight is switched back on.
    pub const KB_RESTORE: &str = "kb_restore";
    /// Whether the keyboard backlight is on (boolean, default true).
    pub const KEYBOARD_ON: &str = "keyboard_on";
    /// Name of the last keyboard lighting profile applied.
    pub const KEYBOARD_PROFILE: &str = "keyboard_profile";
    /// Index of the last effect chosen by the effect-cycling key.
    pub const KB_EFFECT: &str = "kb_effect";
    /// The per-key animation settings (`mode`, `speed`, `keys`, `base`, `t0`).
    pub const PERKEY_FX: &str = "perkey_fx";
    /// The rear light's current state, including its three static zone colours.
    pub const REAR_STATE: &str = "rear_state";
    /// The rear light settings from before it was switched off.
    pub const REAR_LAST: &str = "rear_last";
    /// The three part colours of the rear static effect, kept through off and on.
    pub const REAR_ZONES: &str = "rear_zones";
    /// Index of the last effect chosen by the rear-effect key.
    pub const REAR_EFFECT: &str = "rear_effect";
    /// Name of the last whole-machine profile applied.
    pub const PROFILE: &str = "profile";
    /// Whether the two fan sliders move together (boolean, default true).
    pub const FAN_LINK: &str = "fan_link";
    /// Whether the window keeps its compact fixed size (boolean, default true).
    pub const GUI_FIXED: &str = "gui_fixed";
    /// Audio outputs already put on the DTS sink (list of node names).
    pub const DTS_SEEN: &str = "dts_seen";
}

/// The user-state file path: `$TRUESENSE_USERSTATE`; else, when `TRUESENSE_SIM` is set, a
/// throwaway `truesense-sim-<uid>/user.json` in the temp directory; else
/// `$HOME/.local/state/truesense/user.json`. Without `HOME` it falls back to a file in the
/// temp directory so nothing is ever written to a surprising place.
pub fn default_path_from(env: &dyn Fn(&str) -> Option<String>, temp_dir: &Path) -> PathBuf {
    if let Some(p) = env("TRUESENSE_USERSTATE") {
        return PathBuf::from(p);
    }
    if env("TRUESENSE_SIM").is_some() {
        return temp_dir.join(format!("truesense-sim-{}", current_uid())).join("user.json");
    }
    match env("HOME") {
        Some(home) => PathBuf::from(home).join(".local/state/truesense/user.json"),
        None => temp_dir.join("truesense-user.json"),
    }
}

/// [`default_path_from`] for the running process.
pub fn default_path() -> PathBuf {
    default_path_from(&fsx::env_nonempty, &std::env::temp_dir())
}

/// The effective user id, read from the ownership of `/proc/self` (no `unsafe` needed).
fn current_uid() -> u32 {
    std::fs::metadata("/proc/self").map_or(0, |m| m.uid())
}

/// One `user.json`. Cheap to create; hold one per process and share it between threads.
#[derive(Debug)]
pub struct Store {
    path: PathBuf,
    /// Serialises writers in this process; the file lock serialises the other processes.
    gate: Mutex<()>,
}

impl Store {
    /// A store on the file at `path`. Nothing is read or created yet.
    pub fn new(path: impl Into<PathBuf>) -> Store {
        Store { path: path.into(), gate: Mutex::new(()) }
    }

    /// A store at [`default_path`].
    pub fn at_default() -> Store {
        Store::new(default_path())
    }

    /// The file this store uses.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The whole state; empty when the file is missing, unreadable, invalid JSON, or not an
    /// object. Re-read on every call, so changes by other processes are seen at once.
    pub fn load(&self) -> Map<String, Value> {
        match fsx::read_limited(&self.path, fsx::MAX_SETTINGS_FILE).map(|b| serde_json::from_slice(&b)) {
            Ok(Ok(Value::Object(map))) => map,
            _ => Map::new(),
        }
    }

    /// One key, or `None` when absent (a stored `null` is `Some(Value::Null)`).
    pub fn get(&self, key: &str) -> Option<Value> {
        self.load().remove(key)
    }

    /// One key decoded into `T`; `None` when absent or when it does not have that shape, so a
    /// hand-edited or older file degrades to the caller's default instead of an error.
    pub fn get_as<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        serde_json::from_value(self.get(key)?).ok()
    }

    /// A boolean key, or `default` when absent or not a boolean.
    pub fn get_bool(&self, key: &str, default: bool) -> bool {
        self.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
    }

    /// Changes one key. The `Err` can be ignored: the file is a convenience, never a requirement.
    ///
    /// The read-modify-write is atomic with respect to other writers (threads and processes). If
    /// the lock file cannot be created or locked the update still goes ahead, because losing a
    /// concurrent change to a convenience file is better than not remembering anything.
    pub fn set(&self, key: &str, value: Value) -> Result<(), FsError> {
        let _threads = locked(&self.gate);
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            fsx::ensure_dir(dir, 0o755)?;
        }
        let mut lock_path = self.path.clone().into_os_string();
        lock_path.push(".lock");
        let _processes = FileLock::exclusive(Path::new(&lock_path)).ok();
        let mut state = self.load();
        state.insert(key.to_owned(), value);
        let json = serde_json::to_vec(&state).map_err(|e| FsError::Io {
            op: "encode",
            path: self.path.clone(),
            source: std::io::Error::other(e.to_string()),
        })?;
        fsx::write_atomic(&self.path, &json, 0o644)
    }

    /// [`Store::set`] for any serialisable value.
    pub fn set_as<T: Serialize>(&self, key: &str, value: &T) -> Result<(), FsError> {
        let v = serde_json::to_value(value).map_err(|e| FsError::Io {
            op: "encode",
            path: self.path.clone(),
            source: std::io::Error::other(e.to_string()),
        })?;
        self.set(key, v)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::fsx::testing::TempDir;
    use serde::Deserialize;
    use serde_json::json;
    use std::fs;
    use std::sync::Arc;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Rear {
        mode: String,
        brightness: u32,
    }

    #[test]
    fn set_get_round_trip_creates_the_directory() {
        let d = TempDir::new("us-roundtrip");
        let s = Store::new(d.join("sub/user.json"));
        assert_eq!(s.get("x"), None);
        assert!(s.load().is_empty());
        s.set("rear_last", json!({"mode": "wave", "brightness": 60})).expect("set");
        assert_eq!(s.get_as::<Rear>("rear_last"), Some(Rear { mode: "wave".into(), brightness: 60 }));
        assert_eq!(s.get_as::<Rear>("missing"), None);
        assert_eq!(s.get_as::<Vec<u8>>("rear_last"), None, "wrong shape is None, not an error");
        s.set_as("rear_state", &Rear { mode: "static".into(), brightness: 5 }).expect("set_as");
        assert_eq!(s.get("rear_state"), Some(json!({"mode": "static", "brightness": 5})));
        assert_eq!(s.load().len(), 2);
    }

    #[test]
    fn typed_bool_default() {
        let d = TempDir::new("us-bool");
        let s = Store::new(d.join("user.json"));
        assert!(s.get_bool(keys::KEYBOARD_ON, true));
        s.set(keys::KEYBOARD_ON, Value::Bool(false)).expect("set");
        assert!(!s.get_bool(keys::KEYBOARD_ON, true));
        s.set(keys::KEYBOARD_ON, json!("yes")).expect("set");
        assert!(s.get_bool(keys::KEYBOARD_ON, true), "a non-boolean falls back to the default");
    }

    #[test]
    fn concurrent_writers_in_separate_stores_lose_nothing() {
        // Port of the Go TestConcurrentWritersLoseNothing: separate stores share only the file lock.
        let d = TempDir::new("us-concurrent");
        let path = d.join("user.json");
        let handles: Vec<_> = (0..20)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || Store::new(path).set(&format!("k{i}"), i.into()).expect("set"))
            })
            .collect();
        for h in handles {
            h.join().expect("thread");
        }
        assert_eq!(Store::new(&path).load().len(), 20, "every key survived");
    }

    #[test]
    fn concurrent_writers_in_one_store_lose_nothing() {
        let d = TempDir::new("us-shared");
        let s = Arc::new(Store::new(d.join("user.json")));
        let handles: Vec<_> = (0..20)
            .map(|i| {
                let s = Arc::clone(&s);
                std::thread::spawn(move || s.set(&format!("k{i}"), i.into()).expect("set"))
            })
            .collect();
        for h in handles {
            h.join().expect("thread");
        }
        assert_eq!(s.load().len(), 20);
    }

    #[test]
    fn corrupt_and_odd_files_read_as_empty_and_are_repaired_by_the_next_write() {
        let d = TempDir::new("us-corrupt");
        let p = d.join("user.json");
        let s = Store::new(&p);
        for body in [&b"{not json"[..], b"", b"[1,2]", b"null", b"3", &[0xff, 0xfe], &[b'['; 50_000][..]] {
            fs::write(&p, body).expect("write");
            assert!(s.load().is_empty(), "{body:?}");
            assert_eq!(s.get("a"), None);
        }
        fs::write(&p, "{not json").expect("write");
        s.set("b", 2.into()).expect("repair");
        assert_eq!(s.get("b"), Some(json!(2)));
    }

    #[test]
    fn keys_unknown_to_this_edition_are_preserved() {
        let d = TempDir::new("us-unknown");
        let p = d.join("user.json");
        fs::write(&p, r#"{"keyboard": {"mode": "effect", "name": "neon"}, "from_the_future": [1, 2]}"#)
            .expect("write");
        let s = Store::new(&p);
        s.set(keys::PROFILE, "gaming".into()).expect("set");
        let v = s.load();
        assert_eq!(v["from_the_future"], json!([1, 2]));
        assert_eq!(v["keyboard"]["name"], "neon");
        assert_eq!(v["profile"], "gaming");
    }

    #[test]
    fn a_failing_disk_is_an_error_not_a_panic() {
        let d = TempDir::new("us-readonly");
        let blocker = d.join("blocker");
        fs::write(&blocker, b"x").expect("write");
        let s = Store::new(blocker.join("user.json"));
        assert!(s.set("a", 1.into()).is_err());
        assert_eq!(s.get("a"), None);
    }

    #[test]
    fn the_simulator_uses_a_throwaway_path_never_the_real_one() {
        let tmp = Path::new("/tmp-for-test");
        let sim = |k: &str| match k {
            "TRUESENSE_SIM" => Some("1".to_owned()),
            "HOME" => Some("/home/u".to_owned()),
            _ => None,
        };
        let p = default_path_from(&sim, tmp);
        assert!(p.starts_with(tmp) && p.ends_with("user.json"), "{p:?}");
        assert!(!p.to_string_lossy().contains(".local/state/truesense"), "{p:?}");
        // The explicit override wins, also in the simulator.
        let both = |k: &str| match k {
            "TRUESENSE_SIM" | "TRUESENSE_USERSTATE" => Some("/x/user.json".to_owned()),
            _ => None,
        };
        assert_eq!(default_path_from(&both, tmp), PathBuf::from("/x/user.json"));
    }

    #[test]
    fn the_real_path_is_under_home_and_degrades_without_it() {
        let tmp = Path::new("/tmp-for-test");
        let home = |k: &str| (k == "HOME").then(|| "/home/u".to_owned());
        assert_eq!(default_path_from(&home, tmp), PathBuf::from("/home/u/.local/state/truesense/user.json"));
        assert_eq!(default_path_from(&|_| None, tmp), PathBuf::from("/tmp-for-test/truesense-user.json"));
    }

    #[test]
    fn the_key_names_are_the_ones_the_python_edition_wrote() {
        // Format compatibility: these strings are on disk in existing users' files.
        let names = [
            keys::KEYBOARD,
            keys::KB_RESTORE,
            keys::KEYBOARD_ON,
            keys::KEYBOARD_PROFILE,
            keys::KB_EFFECT,
            keys::PERKEY_FX,
            keys::REAR_STATE,
            keys::REAR_LAST,
            keys::REAR_ZONES,
            keys::REAR_EFFECT,
            keys::PROFILE,
            keys::FAN_LINK,
            keys::GUI_FIXED,
            keys::DTS_SEEN,
        ];
        assert_eq!(
            names,
            [
                "keyboard",
                "kb_restore",
                "keyboard_on",
                "keyboard_profile",
                "kb_effect",
                "perkey_fx",
                "rear_state",
                "rear_last",
                "rear_zones",
                "rear_effect",
                "profile",
                "fan_link",
                "gui_fixed",
                "dts_seen"
            ]
        );
    }
}
