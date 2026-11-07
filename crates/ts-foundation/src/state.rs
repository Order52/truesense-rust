//! The two kinds of state the service publishes.
//!
//! * [`Store`]: the daemon's persistent settings (`/var/lib/truesense/state.json`), which
//!   survive reboots: smart fan on or off, the chosen curve, the user's power limits, the
//!   undervolt-at-boot request and its crash-guard bookkeeping. Defaults come from the config;
//!   what is on disk wins over them.
//! * [`LiveState`]: the readings of the running control loop (temperatures, fan speeds, mode),
//!   written as `key=value` lines to `/run/truesense/state` and `/tmp/truesense-state` for
//!   status bars and for `health.sh`, which restarts the service when the file goes stale. It is
//!   rewritten every tick and never read back by the daemon.
//!
//! Both are best effort by design: a missing, corrupt or unwritable file never stops the service.
//! A read-only disk means the setting applies for this boot only, and a corrupt file means the
//! defaults apply until the next change repairs it.
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::{Map, Value};

use crate::config::Config;
use crate::fsx::{self, FsError};

/// Where the daemon keeps its settings when nothing overrides it.
pub(crate) const STATE_PATH: &str = "/var/lib/truesense/state.json";

/// The runtime directory of the service; the live state file is `state` inside it.
pub const RUN_DIR: &str = "/run/truesense";

/// A copy of the live state in a place whose sandbox status bars can see (some cannot see `/run`).
pub const TMP_LIVE_PATH: &str = "/tmp/truesense-state";

/// The legacy file the old Predator widgets read, written only when `[compat] predator_widgets` is on.
pub const PREDATOR_COMPAT_PATH: &str = "/tmp/predator-state";

/// The settings file path: `$TRUESENSE_STATE` or [`STATE_PATH`].
pub fn default_path_from(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    env("TRUESENSE_STATE").map_or_else(|| PathBuf::from(STATE_PATH), PathBuf::from)
}

/// [`default_path_from`] for the running process.
pub fn default_path() -> PathBuf {
    default_path_from(&fsx::env_nonempty)
}

/// The settings a first start begins with, taken from the config exactly as the Python daemon
/// builds them. Keys the daemon sets later (`fan_curve_data`, `pl_user`, ...) start as `null`
/// where the Python edition lists them, so a `get` answers "unset" rather than "unknown".
pub fn defaults(cfg: &Config) -> Map<String, Value> {
    let auto_mode = if cfg.daemon.auto_profile { "native" } else { "off" };
    let entries = [
        ("smart_fan", Value::Bool(cfg.daemon.smart_fan)),
        ("auto_profile", Value::Bool(cfg.daemon.auto_profile)),
        ("auto_mode", Value::from(auto_mode)),
        ("fan_curve", Value::from("native")),
        ("fan_curve_data", Value::Null),
        ("pl_user", Value::Null),
        ("smart_profile", Value::from("native")),
        ("smart_profile_data", Value::Null),
        ("operating_follow", Value::Bool(cfg.keyboard.operating_follow)),
        ("profile_hold_until", Value::from(0)),
    ];
    entries.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()
}

/// How [`Store`] puts bytes on disk: replaced in tests by a writer that fails or crashes.
pub type Writer = Arc<dyn Fn(&Path, &[u8]) -> Result<(), FsError> + Send + Sync>;

/// The settings and whether the disk is behind them.
#[derive(Debug)]
struct Data {
    map: Map<String, Value>,
    /// The last write failed (read-only or full disk): memory is ahead of the file, and
    /// [`Store::flush`] writes it when the disk works again.
    dirty: bool,
}

/// The daemon's persistent settings: a JSON object kept in memory and rewritten atomically on
/// every change. Safe to share between threads (the control loop and the socket handlers).
pub struct Store {
    path: PathBuf,
    data: Mutex<Data>,
    writer: Writer,
    /// Why the file on disk was not used at open (corrupt, unreadable); `None` when it was fine
    /// or simply missing.
    load_problem: Option<String>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").field("path", &self.path).field("data", &self.data).finish_non_exhaustive()
    }
}

/// The real writer: temp file, `fsync`, rename, so a crash leaves the old file or the new one.
fn atomic_writer() -> Writer {
    Arc::new(|path, bytes| fsx::write_atomic_mkdir(path, bytes, 0o644))
}

impl Store {
    /// Loads the file over `defaults`. A missing file, an unreadable one, invalid JSON, or JSON
    /// that is not an object are all "no settings on disk": the defaults apply. A file that is
    /// there but unusable is renamed to `<name>.corrupt` (best effort) and the reason is kept in
    /// [`Store::load_problem`], so the next change does not destroy what an administrator might
    /// want to repair by hand.
    pub fn open(path: impl Into<PathBuf>, defaults: Map<String, Value>) -> Store {
        Self::open_with(path, defaults, atomic_writer())
    }

    /// [`Store::open`] with another way of writing (tests: a disk that fails or fills up).
    pub fn open_with(path: impl Into<PathBuf>, defaults: Map<String, Value>, writer: Writer) -> Store {
        let path = path.into();
        let mut map = defaults;
        let mut load_problem = None;
        match fsx::read_limited(&path, fsx::MAX_SETTINGS_FILE) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(Value::Object(on_disk)) => map.extend(on_disk),
                Ok(_) => load_problem = Some("the settings file does not hold a JSON object".to_owned()),
                Err(e) => load_problem = Some(format!("the settings file is not valid JSON ({e})")),
            },
            Err(e) if e.is_not_found() => {}
            Err(e) => load_problem = Some(format!("the settings file cannot be read ({e})")),
        }
        if let Some(why) = &mut load_problem {
            if quarantine(&path) {
                why.push_str("; it was kept as the .corrupt file beside it");
            }
        }
        Store { path, data: Mutex::new(Data { map, dirty: false }), writer, load_problem }
    }

    /// The file this store persists to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Why the settings on disk were ignored at open, when they were.
    pub fn load_problem(&self) -> Option<&str> {
        self.load_problem.as_deref()
    }

    // A panic in another thread while holding the lock must not take the fan controller down
    // with it: the map is always in a valid state, so a poisoned lock is simply used.
    fn lock(&self) -> MutexGuard<'_, Data> {
        self.data.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The value of `key`, or `None` if never set. A stored JSON `null` is `Some(Value::Null)`:
    /// "set to nothing" (the daemon clears `uv_boot` that way) is different from "never set".
    pub fn get(&self, key: &str) -> Option<Value> {
        self.lock().map.get(key).cloned()
    }

    /// A text value, or `default` when unset or not a string.
    pub fn get_str(&self, key: &str, default: &str) -> String {
        match self.lock().map.get(key) {
            Some(Value::String(s)) => s.clone(),
            _ => default.to_owned(),
        }
    }

    /// A boolean value, or `default` when unset or not a boolean.
    pub fn get_bool(&self, key: &str, default: bool) -> bool {
        self.lock().map.get(key).and_then(Value::as_bool).unwrap_or(default)
    }

    /// A numeric value, or `default` when unset or not a number.
    pub fn get_f64(&self, key: &str, default: f64) -> f64 {
        self.lock().map.get(key).and_then(Value::as_f64).unwrap_or(default)
    }

    /// An integer value, or `default` when unset or not an integer.
    pub fn get_i64(&self, key: &str, default: i64) -> i64 {
        self.lock().map.get(key).and_then(Value::as_i64).unwrap_or(default)
    }

    /// Changes one value and rewrites the file. The change always applies in memory; the `Err`
    /// says only that it could not be persisted (read-only or full disk), which callers may log
    /// once and otherwise ignore: the setting still holds for this boot, and [`Store::flush`]
    /// writes it as soon as the disk works again.
    pub fn set(&self, key: &str, value: Value) -> Result<(), FsError> {
        // The lock is held across the write so concurrent changes reach the disk in order and
        // the file never lags behind a value another thread already read.
        let mut data = self.lock();
        data.map.insert(key.to_owned(), value);
        self.persist(&mut data)
    }

    /// Removes the temp files a crashed write left beside the settings file (see
    /// [`fsx::remove_stale_temps`]); the daemon calls it once at start, under its lock.
    pub fn sweep_stale_temp(&self) -> usize {
        fsx::remove_stale_temps(&self.path)
    }

    /// Whether the last write failed, so that what the daemon runs on is not what the file says.
    pub fn is_dirty(&self) -> bool {
        self.lock().dirty
    }

    /// Writes the settings again if the last write failed. `Ok(true)` when it did write,
    /// `Ok(false)` when there was nothing to do. The daemon calls it from its loop, so a disk
    /// that was full or read-only for a while ends up holding the settings made meanwhile
    /// without waiting for the next change.
    pub fn flush(&self) -> Result<bool, FsError> {
        let mut data = self.lock();
        if !data.dirty {
            return Ok(false);
        }
        self.persist(&mut data).map(|()| true)
    }

    fn persist(&self, data: &mut Data) -> Result<(), FsError> {
        let json = serde_json::to_vec(&data.map).map_err(|e| FsError::Io {
            op: "encode",
            path: self.path.clone(),
            source: std::io::Error::other(e.to_string()),
        })?;
        let result = (self.writer)(&self.path, &json);
        data.dirty = result.is_err();
        result
    }

    /// A copy of every setting, for the `state` reply.
    pub fn snapshot(&self) -> Map<String, Value> {
        self.lock().map.clone()
    }
}

/// Moves an unusable settings file aside; `true` when that worked. Only regular files are moved.
fn quarantine(path: &Path) -> bool {
    let is_file = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file());
    if !is_file {
        return false;
    }
    let mut name = path.as_os_str().to_owned();
    name.push(".corrupt");
    std::fs::rename(path, PathBuf::from(name)).is_ok()
}

/// One value in the live state. Its text form (`1`/`0` for a boolean) is what status bars read
/// and its JSON form is what the control socket's `state` reply carries.
#[derive(Debug, Clone, PartialEq)]
pub enum LiveValue {
    /// Free text (`power_mode`, `fan`, `mode_reason`).
    Text(String),
    /// A whole number (temperatures, rpm, percentages).
    Int(i64),
    /// A flag, written as `1` or `0`.
    Bool(bool),
}

impl LiveValue {
    /// The text written to the `key=value` file.
    pub fn to_text(&self) -> String {
        match self {
            LiveValue::Text(s) => s.clone(),
            LiveValue::Int(n) => n.to_string(),
            LiveValue::Bool(b) => u8::from(*b).to_string(),
        }
    }

    /// The JSON value for the control socket.
    pub fn to_json(&self) -> Value {
        match self {
            LiveValue::Text(s) => Value::from(s.as_str()),
            LiveValue::Int(n) => Value::from(*n),
            LiveValue::Bool(b) => Value::Bool(*b),
        }
    }
}

impl From<&str> for LiveValue {
    fn from(s: &str) -> Self {
        LiveValue::Text(s.to_owned())
    }
}
impl From<String> for LiveValue {
    fn from(s: String) -> Self {
        LiveValue::Text(s)
    }
}
impl From<i64> for LiveValue {
    fn from(n: i64) -> Self {
        LiveValue::Int(n)
    }
}
impl From<bool> for LiveValue {
    fn from(b: bool) -> Self {
        LiveValue::Bool(b)
    }
}

/// The live readings, in a fixed order (the order the status bars and `health.sh` were written
/// against: `mode`, `activity`, `fan`, `tcpu`, ...).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LiveState {
    entries: Vec<(String, LiveValue)>,
}

impl LiveState {
    /// An empty state.
    pub fn new() -> LiveState {
        LiveState::default()
    }

    /// Sets `key`, keeping its position if it exists and appending otherwise. Chainable.
    ///
    /// A key must be non-empty and free of `=` and line breaks, and a text value free of line
    /// breaks, or the file would not parse back as the same lines; such input is stored
    /// sanitised (line breaks become spaces, `=` in a key becomes `_`, an empty key is dropped).
    pub fn set(&mut self, key: &str, value: impl Into<LiveValue>) -> &mut Self {
        let key = key.replace(['\n', '\r'], " ").replace('=', "_");
        let key = key.trim();
        if key.is_empty() {
            return self;
        }
        let value = match value.into() {
            LiveValue::Text(s) => LiveValue::Text(s.replace(['\n', '\r'], " ")),
            other => other,
        };
        match self.entries.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value,
            None => self.entries.push((key.to_owned(), value)),
        }
        self
    }

    /// The value of `key`.
    pub fn get(&self, key: &str) -> Option<&LiveValue> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// The entries in order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &LiveValue)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// The file text: one `key=value` line per entry, booleans as `1`/`0`.
    pub fn render(&self) -> String {
        self.entries.iter().map(|(k, v)| format!("{k}={}\n", v.to_text())).collect()
    }

    /// The JSON object for the control socket's `state` reply, with native types.
    pub fn to_json(&self) -> Value {
        Value::Object(self.entries.iter().map(|(k, v)| (k.clone(), v.to_json())).collect())
    }

    /// Reads a live-state file's text. Tolerant: lines without `=` and lines with an empty key
    /// are skipped, a later duplicate wins, the value is everything after the first `=`. Values
    /// that look like whole numbers become [`LiveValue::Int`], everything else stays text (a file
    /// cannot say whether `1` was a flag, so callers that want a flag use [`LiveState::flag`]).
    pub fn parse(text: &str) -> LiveState {
        let mut state = LiveState::new();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let key = key.trim();
            if key.is_empty() {
                continue;
            }
            let value = value.trim_end_matches('\r');
            match value.parse::<i64>() {
                Ok(n) => state.set(key, n),
                Err(_) => state.set(key, value),
            };
        }
        state
    }

    /// Reads and parses the file at `path`. `None` when it is missing or unreadable (the daemon
    /// is not running); a damaged file parses to whatever lines are intact.
    pub fn read(path: &Path) -> Option<LiveState> {
        fsx::read_to_string(path).ok().map(|t| LiveState::parse(&t))
    }

    /// A flag: `1` is true, `0` is false, a real boolean is itself; anything else or a missing key is `None`.
    pub fn flag(&self, key: &str) -> Option<bool> {
        match self.get(key)? {
            LiveValue::Bool(b) => Some(*b),
            LiveValue::Int(1) => Some(true),
            LiveValue::Int(0) => Some(false),
            _ => None,
        }
    }

    /// A whole number, or `None`.
    pub fn int(&self, key: &str) -> Option<i64> {
        match self.get(key)? {
            LiveValue::Int(n) => Some(*n),
            _ => None,
        }
    }

    /// The value as text, or `None` if the key is absent.
    pub fn text(&self, key: &str) -> Option<String> {
        self.get(key).map(LiveValue::to_text)
    }

    /// Writes [`LiveState::render`] to every path, atomically, world-readable (`0o644`), without
    /// `fsync` (it is rewritten every tick and worthless after a reboot). Best effort: a path that
    /// cannot be written (read-only `/tmp`, no run directory) is reported in the returned list and
    /// the others are still written; the caller usually ignores the list.
    pub fn publish(&self, paths: &[&Path]) -> Vec<FsError> {
        let text = self.render();
        paths.iter().filter_map(|p| fsx::write_atomic_nosync(p, text.as_bytes(), 0o644).err()).collect()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::fsx::testing::TempDir;
    use std::fs;
    use std::sync::Arc;

    fn defaults_of(pairs: &[(&str, Value)]) -> Map<String, Value> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect()
    }

    #[test]
    fn defaults_then_file_then_persistence() {
        // Port of the Go TestDefaultsFileAndPersistence and the Python restart check.
        let d = TempDir::new("state-persist");
        let p = d.join("sub/state.json");
        let defs = defaults_of(&[("smart_fan", Value::Bool(false)), ("fan_curve", "native".into())]);
        let s = Store::open(&p, defs.clone());
        assert!(!s.get_bool("smart_fan", true));
        assert_eq!(s.get_str("fan_curve", ""), "native", "defaults apply with no file");
        assert!(!p.exists(), "opening writes nothing");
        s.set("smart_fan", Value::Bool(true)).expect("set");
        s.set("pl_user", serde_json::json!({"pl1": 70.0})).expect("set");
        let s2 = Store::open(&p, defs);
        assert!(s2.get_bool("smart_fan", false), "the file wins over the defaults");
        assert_eq!(s2.get_str("fan_curve", ""), "native", "untouched defaults stay");
        assert_eq!(s2.get("pl_user").expect("set").get("pl1").and_then(Value::as_f64), Some(70.0));
        assert_eq!(s2.path(), p.as_path());
    }

    #[test]
    fn typed_getters_fall_back_on_wrong_type_or_missing() {
        let d = TempDir::new("state-typed");
        let s = Store::open(
            d.join("s.json"),
            defaults_of(&[("n", 5.into()), ("f", 1.5.into()), ("t", "x".into()), ("z", Value::Null)]),
        );
        assert_eq!((s.get_i64("n", 0), s.get_f64("n", 0.0)), (5, 5.0));
        assert_eq!((s.get_i64("f", -1), s.get_f64("f", 0.0)), (-1, 1.5));
        assert_eq!(
            (s.get_str("n", "d"), s.get_bool("t", true), s.get_str("missing", "d")),
            ("d".into(), true, "d".into())
        );
        assert_eq!(s.get("z"), Some(Value::Null), "null is set, as in Python's st.set('uv_boot', None)");
        assert!(s.get_bool("z", true));
        assert_eq!(s.get("missing"), None);
    }

    #[test]
    fn missing_corrupt_and_non_object_files_fall_back_to_defaults() {
        let d = TempDir::new("state-corrupt");
        let defs = defaults_of(&[("a", "x".into())]);
        let cases: [(&str, &[u8]); 8] = [
            ("broken", b"{broken"),
            ("empty", b""),
            ("list", b"[1, 2]"),
            ("string", b"\"hello\""),
            ("null", b"null"),
            ("binary", &[0xff, 0xfe, 0x00]),
            ("truncated", b"{\"a\": \"y\", \"b\":"),
            ("deep", &[b'['; 100_000]),
        ];
        for (name, body) in cases {
            let p = d.join(name);
            fs::write(&p, body).expect("write");
            let s = Store::open(&p, defs.clone());
            assert_eq!(s.get_str("a", ""), "x", "{name}");
            assert_eq!(s.snapshot(), defs, "{name}");
        }
        assert_eq!(Store::open(d.join("absent"), defs.clone()).snapshot(), defs);
        // A directory in place of the file is just "no settings" too.
        assert_eq!(Store::open(&d.0, defs.clone()).snapshot(), defs);
    }

    #[test]
    fn the_next_change_repairs_a_corrupt_file() {
        let d = TempDir::new("state-repair");
        let p = d.join("state.json");
        fs::write(&p, "{broken").expect("write");
        let s = Store::open(&p, Map::new());
        s.set("b", 1.into()).expect("set");
        assert_eq!(Store::open(&p, Map::new()).get_i64("b", 0), 1);
    }

    #[test]
    fn unwritable_disk_is_an_error_but_the_setting_still_applies() {
        let d = TempDir::new("state-readonly");
        // A regular file where the directory should be: nothing can be created below it.
        let blocker = d.join("blocker");
        fs::write(&blocker, b"x").expect("write");
        let s = Store::open(blocker.join("state.json"), Map::new());
        assert!(s.set("a", 1.into()).is_err());
        assert_eq!(s.get_i64("a", 0), 1, "the setting still applies for this boot");
        assert!(Store::open("/proc/nonexistent/state.json", Map::new()).set("a", 1.into()).is_err());
    }

    #[test]
    fn on_disk_keys_unknown_to_the_defaults_are_kept() {
        let d = TempDir::new("state-extra");
        let p = d.join("state.json");
        fs::write(&p, "{\"charge_full\": {\"until\": 5}, \"smart_fan\": true}").expect("write");
        let s = Store::open(&p, defaults_of(&[("smart_fan", Value::Bool(false))]));
        s.set("other", 1.into()).expect("set");
        let text = fs::read_to_string(&p).expect("read");
        let v: Value = serde_json::from_str(&text).expect("json");
        assert_eq!(v["charge_full"]["until"], 5);
        assert_eq!(v["smart_fan"], true);
        assert_eq!(v["other"], 1);
    }

    #[test]
    fn concurrent_sets_are_all_applied_in_memory_and_the_file_stays_valid() {
        let d = TempDir::new("state-threads");
        let p = d.join("state.json");
        let s = Arc::new(Store::open(&p, Map::new()));
        let handles: Vec<_> = (0..16)
            .map(|i| {
                let s = Arc::clone(&s);
                std::thread::spawn(move || {
                    s.set(&format!("k{i}"), i.into()).expect("set");
                    s.snapshot();
                })
            })
            .collect();
        for h in handles {
            h.join().expect("thread");
        }
        assert_eq!(s.snapshot().len(), 16);
        assert_eq!(Store::open(&p, Map::new()).snapshot().len(), 16, "the last write holds everything");
    }

    #[test]
    fn a_poisoned_lock_does_not_take_the_store_down() {
        let d = TempDir::new("state-poison");
        let s = Arc::new(Store::open(d.join("s.json"), Map::new()));
        let s2 = Arc::clone(&s);
        let _ = std::thread::spawn(move || {
            let _guard = s2.data.lock().expect("lock");
            std::panic::resume_unwind(Box::new("boom"));
        })
        .join();
        s.set("a", 1.into()).expect("set after poisoning");
        assert_eq!(s.get_i64("a", 0), 1);
    }

    #[test]
    fn defaults_follow_the_config_like_the_python_daemon() {
        let mut cfg = Config::default();
        let d = defaults(&cfg);
        assert_eq!(d["smart_fan"], false);
        assert_eq!(d["auto_profile"], true);
        assert_eq!(d["auto_mode"], "native");
        assert_eq!(d["fan_curve"], "native");
        assert_eq!(d["smart_profile"], "native");
        assert_eq!(d["fan_curve_data"], Value::Null);
        assert_eq!(d["profile_hold_until"], 0);
        assert_eq!(d["operating_follow"], false);
        cfg.daemon.auto_profile = false;
        cfg.daemon.smart_fan = true;
        cfg.keyboard.operating_follow = true;
        let d = defaults(&cfg);
        assert_eq!(
            (d["auto_mode"].as_str(), d["smart_fan"].as_bool(), d["operating_follow"].as_bool()),
            (Some("off"), Some(true), Some(true))
        );
    }

    #[test]
    fn default_path_honours_the_override() {
        let over = |k: &str| (k == "TRUESENSE_STATE").then(|| "/x/s.json".to_owned());
        assert_eq!(default_path_from(&over), PathBuf::from("/x/s.json"));
        assert_eq!(default_path_from(&|_| None), PathBuf::from("/var/lib/truesense/state.json"));
    }

    fn sample_live() -> LiveState {
        let mut l = LiveState::new();
        l.set("mode", "manual")
            .set("fan", "auto")
            .set("tcpu", 61i64)
            .set("ac", true)
            .set("fan_off", false)
            .set("mode_reason", "");
        l
    }

    #[test]
    fn live_state_renders_key_value_lines_with_booleans_as_digits() {
        // Python: "".join(f"{k}={v}\n" if not isinstance(v, bool) else f"{k}={int(v)}\n" ...)
        assert_eq!(sample_live().render(), "mode=manual\nfan=auto\ntcpu=61\nac=1\nfan_off=0\nmode_reason=\n");
        assert_eq!(LiveState::new().render(), "");
    }

    #[test]
    fn live_state_json_keeps_native_types_for_the_socket() {
        let j = sample_live().to_json();
        assert_eq!(
            j,
            serde_json::json!({"mode": "manual", "fan": "auto", "tcpu": 61, "ac": true, "fan_off": false, "mode_reason": ""})
        );
    }

    #[test]
    fn set_replaces_in_place_and_keeps_the_order() {
        let mut l = sample_live();
        l.set("fan", "55,55").set("extra", 1i64);
        let keys: Vec<&str> = l.iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["mode", "fan", "tcpu", "ac", "fan_off", "mode_reason", "extra"]);
        assert_eq!(l.text("fan").as_deref(), Some("55,55"));
    }

    #[test]
    fn live_state_cannot_be_made_to_inject_lines() {
        let mut l = LiveState::new();
        l.set("mode_reason", "hot\nauto=1").set("bad=key\n", "v").set("  ", "dropped");
        let text = l.render();
        assert_eq!(text.lines().count(), 2, "{text:?}");
        let back = LiveState::parse(&text);
        assert_eq!(back.get("auto"), None);
        assert_eq!(back.text("mode_reason").as_deref(), Some("hot auto=1"));
        assert_eq!(back.text("bad_key").as_deref(), Some("v"));
    }

    #[test]
    fn live_state_parse_is_tolerant() {
        let back = LiveState::parse(
            "mode=smart\r\ngarbage line\n=nokey\ntcpu=61\nac=1\nfan=40,40\ntcpu=70\nreason=a=b\n\n  spaced = x\n",
        );
        assert_eq!(back.text("mode").as_deref(), Some("smart"));
        assert_eq!(back.int("tcpu"), Some(70), "a later duplicate wins");
        assert_eq!(back.flag("ac"), Some(true));
        assert_eq!(back.text("fan").as_deref(), Some("40,40"));
        assert_eq!(back.int("fan"), None);
        assert_eq!(back.text("reason").as_deref(), Some("a=b"), "the value is everything after the first =");
        assert_eq!(back.text("spaced").as_deref(), Some(" x"));
        assert_eq!(back.iter().count(), 6);
        assert_eq!(back.flag("mode"), None);
        assert_eq!(back.flag("missing"), None);
    }

    #[test]
    fn live_state_survives_a_render_parse_round_trip_as_text() {
        let back = LiveState::parse(&sample_live().render());
        assert_eq!(back.render(), sample_live().render());
        assert_eq!(back.flag("ac"), Some(true));
        assert_eq!(back.flag("fan_off"), Some(false));
    }

    #[test]
    fn hostile_live_files_do_not_panic() {
        for body in
            ["", "=", "====", "\0\0\0", "a=\u{1F980}", &"k=v\n".repeat(10_000), "a=9999999999999999999999"]
        {
            let l = LiveState::parse(body);
            let _ = l.render();
        }
        assert_eq!(LiveState::parse("a=9999999999999999999999").int("a"), None);
    }

    #[test]
    fn publish_writes_every_path_world_readable_and_reports_the_ones_that_fail() {
        use std::os::unix::fs::PermissionsExt;
        let d = TempDir::new("state-publish");
        fs::create_dir(d.join("run")).expect("dir");
        let ok1 = d.join("run/state");
        let ok2 = d.join("truesense-state");
        let bad = d.join("missing-dir/state");
        let errors = sample_live().publish(&[&ok1, &bad, &ok2]);
        assert_eq!(errors.len(), 1, "{errors:?}");
        for p in [&ok1, &ok2] {
            assert_eq!(fs::read_to_string(p).expect("read"), sample_live().render());
            assert_eq!(fs::metadata(p).expect("stat").permissions().mode() & 0o777, 0o644);
        }
        // Rewriting replaces the content.
        let mut l = sample_live();
        l.set("tcpu", 99i64);
        assert!(l.publish(&[&ok1]).is_empty());
        assert_eq!(LiveState::read(&ok1).expect("read").int("tcpu"), Some(99));
        assert!(LiveState::read(&d.join("none")).is_none());
    }
}
