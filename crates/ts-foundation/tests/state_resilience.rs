//! The settings file under crashes and bad disks: what `/var/lib/truesense/state.json` looks
//! like after a kill at any moment, a truncation at any byte, a bit flip anywhere, a disk that
//! is full or read-only for a while.
//!
//! Evidence for `docs/RESILIENCE.md` (failure modes S1 to S6).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, Value, json};
use ts_foundation::fsx::{self, FsError};
use ts_foundation::state::{Store, Writer};

struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Dir {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "tsf-res-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&p).unwrap();
        Dir(p)
    }
    fn join(&self, n: &str) -> PathBuf {
        self.0.join(n)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn defaults() -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("smart_fan".into(), json!(false));
    m.insert("fan_curve".into(), json!("native"));
    m
}

/// A state file as the Go edition 0.4.0 leaves it on a laptop that uses the Smart fan.
fn real_state_text() -> String {
    serde_json::to_string(&json!({
        "smart_fan": true, "auto_profile": true, "auto_mode": "smart", "fan_curve": "truesense",
        "fan_curve_data": null, "pl_user": {"pl1": 70.0, "pl2": 90.0}, "smart_profile": "game-steady",
        "smart_profile_data": null, "operating_follow": true, "profile_hold_until": 0,
        "uv_boot": {"core": -60, "cache": -60}, "uv_applied": null,
        "pm_takeover": false, "charge_full": {"until": 1_800_000_000.5, "note": "ünïcode ✓"}
    }))
    .unwrap()
}

/// S1: whatever prefix of a valid file survives a crash or a failing disk, opening never panics,
/// and the answer is either the whole file or the defaults, never half of each.
#[test]
fn a_file_cut_at_every_byte_offset_opens_as_defaults_or_whole() {
    let full = real_state_text();
    let want: Map<String, Value> = {
        let mut m = defaults();
        m.extend(serde_json::from_str::<Map<String, Value>>(&full).unwrap());
        m
    };
    let d = Dir::new("cut");
    for cut in 0..=full.len() {
        let p = d.join("state.json");
        let _ = fs::remove_file(d.join("state.json.corrupt"));
        fs::write(&p, &full.as_bytes()[..cut]).unwrap();
        let s = Store::open(&p, defaults());
        let snap = s.snapshot();
        if cut == full.len() {
            assert_eq!(snap, want, "the whole file");
            assert!(s.load_problem().is_none());
        } else {
            assert_eq!(snap, defaults(), "cut at {cut}: defaults, never a half-read file");
            assert!(s.load_problem().is_some(), "cut at {cut}: the problem is reported");
            assert!(
                d.join("state.json.corrupt").exists(),
                "cut at {cut}: the damaged file is kept for repair"
            );
            assert!(!p.exists(), "and moved out of the way");
        }
    }
}

/// S1b: one flipped byte anywhere either still parses to something sensible or is reported;
/// nothing panics and the daemon's keys keep their types through `get_*`.
#[test]
fn a_flipped_byte_at_every_offset_never_panics() {
    let full = real_state_text().into_bytes();
    let d = Dir::new("flip");
    for at in 0..full.len() {
        for flip in [0x01u8, 0x80, 0xff] {
            let mut bad = full.clone();
            bad[at] ^= flip;
            let p = d.join("state.json");
            let _ = fs::remove_file(d.join("state.json.corrupt"));
            fs::write(&p, &bad).unwrap();
            let s = Store::open(&p, defaults());
            let _ = (
                s.get_bool("smart_fan", false),
                s.get_str("fan_curve", ""),
                s.get_f64("profile_hold_until", 0.0),
                s.snapshot(),
            );
        }
    }
}

/// S2: a writer that is killed at any point of its own protocol (temp file written partly, fully,
/// before the rename) leaves the target file as it was: the old content, complete.
#[test]
fn a_writer_that_fails_at_every_point_leaves_the_old_file_whole() {
    let d = Dir::new("proto");
    let p = d.join("state.json");
    let old = real_state_text();
    fs::write(&p, &old).unwrap();
    let fail_after = Arc::new(AtomicUsize::new(0));
    let fa = Arc::clone(&fail_after);
    // The protocol of `fsx::write_atomic`, with a crash injected after `fa` bytes of the temp file.
    let writer: Writer = Arc::new(move |path: &Path, bytes: &[u8]| {
        let n = fa.load(Ordering::SeqCst);
        let tmp = path.with_file_name(".state.json.crash.tmp");
        fs::write(&tmp, &bytes[..n.min(bytes.len())]).map_err(|e| FsError::Io {
            op: "write",
            path: tmp.clone(),
            source: e,
        })?;
        if n < bytes.len() {
            return Err(FsError::Io { op: "crash", path: tmp, source: std::io::Error::other("killed") });
        }
        fs::rename(&tmp, path).map_err(|e| FsError::Io { op: "rename", path: path.to_path_buf(), source: e })
    });
    let s = Store::open_with(&p, defaults(), writer);
    let mut sizes = Vec::new();
    let mut previous: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
    for n in (0..700).step_by(7) {
        fail_after.store(n, Ordering::SeqCst);
        let r = s.set("marker", json!(n));
        sizes.push((n, r.is_ok()));
        let on_disk: Value =
            serde_json::from_slice(&fs::read(&p).unwrap()).expect("the target is always complete JSON");
        if r.is_err() {
            assert_eq!(on_disk, previous, "n={n}: a failed write leaves the previous file exactly");
        } else {
            assert_eq!(on_disk["marker"], json!(n), "n={n}");
            previous = on_disk;
        }
    }
    assert!(
        sizes.iter().any(|(_, ok)| !*ok) && sizes.iter().any(|(_, ok)| *ok),
        "both outcomes were exercised"
    );
}

/// S2b: the real thing. A child process rewrites the file in a tight loop and is killed with
/// SIGKILL at random moments; after every kill the file parses, holds a generation number that
/// never goes backwards, and the leftover temp files are swept at the next start.
#[test]
fn killing_a_writer_with_sigkill_never_leaves_a_damaged_file() {
    if let Ok(path) = std::env::var("TS_STATE_CHILD") {
        let s = Store::open(&path, defaults());
        let mut generation = s.get_i64("gen", 0);
        let padding: Vec<Value> = (0..300).map(|i| json!(format!("padding-{i}"))).collect();
        loop {
            generation += 1;
            let _ = s.set("padding", Value::Array(padding.clone()));
            let _ = s.set("gen", json!(generation));
        }
    }
    let d = Dir::new("kill");
    let p = d.join("state.json");
    let exe = std::env::current_exe().unwrap();
    let mut last = 0;
    for round in 0..40u64 {
        let mut child = Command::new(&exe)
            .args([
                "--exact",
                "killing_a_writer_with_sigkill_never_leaves_a_damaged_file",
                "--test-threads=1",
            ])
            .env("TS_STATE_CHILD", &p)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(15 + (round * 7) % 31));
        child.kill().unwrap();
        child.wait().unwrap();
        let s = Store::open(&p, defaults());
        assert!(s.load_problem().is_none(), "round {round}: {:?}", s.load_problem());
        let generation = s.get_i64("gen", 0);
        assert!(generation >= last, "round {round}: generation went back from {last} to {generation}");
        last = generation;
        s.sweep_stale_temp();
        let stray: Vec<_> = fs::read_dir(&d.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(stray.is_empty(), "round {round}: {stray:?}");
    }
    assert!(last > 0, "the child managed to write at least once over 40 rounds");
}

/// S3: a disk that refuses writes for a while: the setting holds in memory, the failure is
/// reported, nothing is lost once the disk works, and `flush` does it without another change.
#[test]
fn a_disk_that_fails_for_a_while_catches_up_by_itself() {
    let d = Dir::new("flaky");
    let p = d.join("state.json");
    let failing = Arc::new(AtomicUsize::new(1));
    let f = Arc::clone(&failing);
    let writer: Writer = Arc::new(move |path: &Path, bytes: &[u8]| {
        if f.load(Ordering::SeqCst) == 1 {
            return Err(FsError::Io {
                op: "write",
                path: path.to_path_buf(),
                source: std::io::Error::from_raw_os_error(28),
            }); // ENOSPC
        }
        fsx::write_atomic_mkdir(path, bytes, 0o644)
    });
    let s = Store::open_with(&p, defaults(), writer);
    assert!(!s.is_dirty());
    assert!(s.set("smart_fan", json!(true)).is_err());
    assert!(s.set("pl_user", json!({"pl1": 60.0, "pl2": 80.0})).is_err());
    assert!(s.is_dirty());
    assert!(s.get_bool("smart_fan", false), "the daemon keeps running on what was asked");
    assert!(!p.exists());
    assert!(s.flush().is_err(), "still full: reported, not lost");
    assert!(s.is_dirty());
    failing.store(0, Ordering::SeqCst);
    assert!(s.flush().unwrap(), "the disk works again: written without any new change");
    assert!(!s.is_dirty());
    assert!(!s.flush().unwrap(), "nothing left to do");
    let back = Store::open(&p, defaults());
    assert!(back.get_bool("smart_fan", false));
    assert_eq!(back.get("pl_user").unwrap()["pl2"], json!(80.0));
}

/// S3b: a real read-only directory (skipped for root, which ignores directory modes).
#[test]
fn a_read_only_directory_is_an_error_and_not_a_panic() {
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::PermissionsExt;
    if fs::metadata("/proc/self").is_ok_and(|m| m.uid() == 0) {
        return;
    }
    let d = Dir::new("ro");
    let sub = d.join("var");
    fs::create_dir(&sub).unwrap();
    fs::write(sub.join("state.json"), real_state_text()).unwrap();
    fs::set_permissions(&sub, fs::Permissions::from_mode(0o555)).unwrap();
    let s = Store::open(sub.join("state.json"), defaults());
    assert!(s.get_bool("smart_fan", false), "reading works");
    assert!(s.set("smart_fan", json!(false)).is_err());
    assert!(!s.get_bool("smart_fan", true), "applies for this boot");
    assert!(s.is_dirty());
    fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(s.flush().unwrap());
    assert!(!Store::open(sub.join("state.json"), defaults()).get_bool("smart_fan", true));
}

/// S4: keys another edition wrote are kept on every rewrite, with their types and order of values.
#[test]
fn keys_the_store_does_not_know_survive_rewrites() {
    let d = Dir::new("extra");
    let p = d.join("state.json");
    let text = r#"{"smart_fan": true, "from_the_future": {"a": [1, 2.5, null, "x"], "b": {"c": true}}, "legacy_key": "kept"}"#;
    fs::write(&p, text).unwrap();
    let s = Store::open(&p, defaults());
    for i in 0..5 {
        s.set("smart_fan", json!(i % 2 == 0)).unwrap();
    }
    let v: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
    assert_eq!(v["from_the_future"], json!({"a": [1, 2.5, null, "x"], "b": {"c": true}}));
    assert_eq!(v["legacy_key"], "kept");
}

/// S5: the corrupt file is kept (not overwritten by the first change), once.
#[test]
fn the_first_change_after_a_corrupt_start_does_not_destroy_the_evidence() {
    let d = Dir::new("evidence");
    let p = d.join("state.json");
    fs::write(&p, "{\"smart_fan\": tru").unwrap();
    let log = Arc::new(Mutex::new(Vec::<String>::new()));
    let s = Store::open(&p, defaults());
    log.lock().unwrap().push(s.load_problem().unwrap().to_owned());
    s.set("smart_fan", json!(true)).unwrap();
    assert_eq!(fs::read_to_string(d.join("state.json.corrupt")).unwrap(), "{\"smart_fan\": tru");
    assert!(Store::open(&p, defaults()).get_bool("smart_fan", false));
    assert!(log.lock().unwrap()[0].contains("not valid JSON"));
}

/// S6: a hostile file: huge, deeply nested, not UTF-8, a directory: all are "no settings".
#[test]
fn hostile_settings_files_are_no_settings() {
    let d = Dir::new("hostile");
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("huge", vec![b' '; (fsx::MAX_SETTINGS_FILE as usize) + 10]),
        ("deep", vec![b'['; 200_000]),
        ("binary", vec![0xff, 0xfe, 0x00, 0x01]),
        ("nulbytes", vec![0u8; 4096]),
    ];
    for (name, body) in cases {
        let p = d.join(name);
        fs::write(&p, body).unwrap();
        let s = Store::open(&p, defaults());
        assert_eq!(s.snapshot(), defaults(), "{name}");
        assert!(s.load_problem().is_some(), "{name}");
    }
    let dir_as_file = d.join("adir");
    fs::create_dir(&dir_as_file).unwrap();
    let s = Store::open(&dir_as_file, defaults());
    assert_eq!(s.snapshot(), defaults());
    assert!(dir_as_file.is_dir(), "a directory is never moved");
}
