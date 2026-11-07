//! The one-shape profile store: built-in profiles that can never be changed, and the user's own
//! in `~/.config/truesense/<kind>/NAME.json`.
//!
//! Fan curves, Smart auto profiles, PREDATOR key bindings, CPU and GPU undervolt profiles,
//! lighting profiles and looks all had the same hand-copied code in the Python edition (names
//! check, built-ins first, `.tmp` plus rename, "is built in and read-only"). This is that code
//! once. What differs per kind is data: the directory, the words in messages, the name rule,
//! the built-ins and the validator that turns a decoded JSON value into a checked one.
//!
//! The built-in set always contains `native`, the profile that means "what the vendor
//! software would leave": a user who made a mess can always go back to it.
#![forbid(unsafe_code)]

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use ts_foundation::fsx::{self, FsError, MAX_SETTINGS_FILE};
use ts_foundation::names::{self, NAME_MAX};

/// The profile every kind has: the vendor behaviour.
pub const NATIVE: &str = "native";

/// What a profile name may contain. Both rules exclude every path separator, so a name can
/// never leave its directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameRule {
    /// Letters, digits, `.`, `_` and `-`, one to [`NAME_MAX`] characters, ASCII only (curves,
    /// smart profiles, key profiles, undervolt profiles, looks).
    Standard,
    /// Letters, digits, `-` and `_`, any length, Unicode letters allowed (lighting profiles
    /// and whole-setup profiles, the older kinds).
    Strict,
}

impl NameRule {
    /// The message when a name breaks the rule.
    pub fn message(self) -> &'static str {
        match self {
            NameRule::Standard => "profile names use letters, digits, . _ - (max 40)",
            NameRule::Strict => "profile names: letters, digits, - and _",
        }
    }

    /// Whether `name` is allowed.
    pub fn accepts(self, name: &str) -> bool {
        match self {
            NameRule::Standard => names::is_valid(name),
            NameRule::Strict => {
                !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_')
            }
        }
    }
}

/// Turns free text (what a user typed in a "save as" box) into a name the rule accepts, or
/// `None` when nothing usable is left.
///
/// Runs of characters the rule refuses (spaces, slashes, punctuation) become one `-`; leading
/// and trailing `-` and `.` are dropped, so the result is never a hidden file or `..`; the
/// length is cut to [`NAME_MAX`] for the standard rule. The result is never a built-in
/// decision: the caller still checks that the name is free.
pub fn sanitise_name(rule: NameRule, raw: &str) -> Option<String> {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in raw.trim().chars() {
        let ok = match rule {
            NameRule::Standard => c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'),
            NameRule::Strict => c.is_alphanumeric() || matches!(c, '_' | '-'),
        };
        if ok && c != '-' {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(c);
        } else {
            pending_dash = true;
        }
    }
    let trimmed = out.trim_matches(|c| c == '.' || c == '-');
    let mut name: String = trimmed.to_owned();
    if rule == NameRule::Standard {
        // Cut on a character boundary; the standard rule is ASCII, so this is a byte cut.
        name.truncate(NAME_MAX);
        name = name.trim_end_matches(['.', '-']).to_owned();
    }
    if rule.accepts(&name) { Some(name) } else { None }
}

/// The words a kind of profile uses in its messages.
#[derive(Debug, Clone)]
pub struct Kind {
    /// The directory under `~/.config/truesense/` (`keys`, `curves`, `smart`, ...).
    pub dir_name: &'static str,
    /// How the profile is called in messages: `key profile`, `fan curve`, `undervolt profile`.
    pub label: &'static str,
    /// The command that lists them, shown when a name is not found.
    pub list_hint: &'static str,
    /// The command that copies one (`keys profile copy`), shown when someone tries to change a
    /// built-in; `None` for kinds where the advice is to save under another name.
    pub copy_hint: Option<&'static str>,
    /// What a name may contain.
    pub rule: NameRule,
    /// Old names that mean another profile (`stock` meant `native`).
    pub aliases: &'static [(&'static str, &'static str)],
    /// The order the top-level keys are written in; keys not listed follow, sorted. Files
    /// written here then look like the ones the Python edition wrote, so the three editions
    /// can replace each other without a noisy diff in a user's dotfiles.
    pub key_order: &'static [&'static str],
}

/// Where a profile comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Shipped in the program; read-only.
    BuiltIn,
    /// The user's own file.
    Yours,
}

/// Why a profile operation failed. The messages are the Python edition's.
#[derive(Debug)]
pub enum ProfileError {
    /// The name breaks the kind's rule.
    BadName(&'static str),
    /// There is no such profile.
    NotFound {
        /// `no key profile 'x' (truesense keys profile list)` or just `no key profile 'x'`.
        message: String,
    },
    /// The file exists but is not JSON.
    NotJson {
        /// `key profile 'x' is not valid JSON`.
        message: String,
    },
    /// The content does not pass the kind's validator; the text says what is wrong.
    Invalid(String),
    /// A built-in cannot be saved over or deleted.
    ReadOnly(String),
    /// The set of built-ins given to the store has no `native`.
    MissingNative,
    /// The disk refused.
    Io {
        /// What was being done (`save key profile 'x'`).
        op: String,
        /// The file system error.
        source: FsError,
    },
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProfileError::BadName(m) => f.write_str(m),
            ProfileError::NotFound { message } | ProfileError::NotJson { message } => f.write_str(message),
            ProfileError::Invalid(m) | ProfileError::ReadOnly(m) => f.write_str(m),
            ProfileError::MissingNative => write!(f, "the built-in profiles must include '{NATIVE}'"),
            ProfileError::Io { op, source } => write!(f, "{op}: {source}"),
        }
    }
}

impl std::error::Error for ProfileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ProfileError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Checks a decoded profile and returns it normalised (defaults filled in), or says what is
/// wrong in the words shown to the user. Built-ins go through it too, so they cannot drift
/// from what a saved file must satisfy.
pub type Validator = Arc<dyn Fn(&Value) -> Result<Value, String> + Send + Sync>;

/// A directory of the user's profiles of one kind plus the built-ins of that kind.
pub struct ProfileStore {
    kind: Kind,
    user_dir: PathBuf,
    builtins: Vec<(String, Value)>,
    validate: Validator,
}

impl fmt::Debug for ProfileStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProfileStore")
            .field("kind", &self.kind.dir_name)
            .field("user_dir", &self.user_dir)
            .field("builtins", &self.builtins.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>())
            .finish()
    }
}

/// `<home>/.config/truesense/<kind>`: where the user's profiles of a kind live. The Python
/// edition expanded `~` and ignored `XDG_CONFIG_HOME`; so does this, because the files must be
/// found by every edition.
pub fn default_user_dir(home: &Path, kind: &Kind) -> PathBuf {
    home.join(".config").join("truesense").join(kind.dir_name)
}

impl ProfileStore {
    /// A store over `user_dir` (created on the first save). `builtins` are listed first, in the
    /// order given, and must include [`NATIVE`].
    pub fn new(
        kind: Kind,
        user_dir: impl Into<PathBuf>,
        builtins: Vec<(String, Value)>,
        validate: Validator,
    ) -> Result<ProfileStore, ProfileError> {
        if !builtins.iter().any(|(n, _)| n == NATIVE) {
            return Err(ProfileError::MissingNative);
        }
        Ok(ProfileStore { kind, user_dir: user_dir.into(), builtins, validate })
    }

    /// The kind this store keeps.
    pub fn kind(&self) -> &Kind {
        &self.kind
    }

    /// The directory of the user's profiles.
    pub fn user_dir(&self) -> &Path {
        &self.user_dir
    }

    fn path(&self, name: &str) -> Result<PathBuf, ProfileError> {
        if !self.kind.rule.accepts(name) {
            return Err(ProfileError::BadName(self.kind.rule.message()));
        }
        Ok(self.user_dir.join(format!("{name}.json")))
    }

    fn builtin(&self, name: &str) -> Option<&Value> {
        self.builtins.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    fn resolve_alias<'a>(&self, name: &'a str) -> &'a str {
        self.kind.aliases.iter().find(|(old, _)| *old == name).map_or(name, |(_, new)| new)
    }

    /// Whether `name` is a built-in (read-only) profile.
    pub fn is_builtin(&self, name: &str) -> bool {
        self.builtin(name).is_some()
    }

    /// Where a profile comes from, `None` when there is none. A user file with a built-in's name
    /// is never listed or loaded: the built-in wins.
    pub fn source(&self, name: &str) -> Option<Source> {
        let name = self.resolve_alias(name);
        if self.is_builtin(name) {
            return Some(Source::BuiltIn);
        }
        let path = self.path(name).ok()?;
        path.is_file().then_some(Source::Yours)
    }

    /// The built-in names first, in the order given, then the user's, sorted. Files whose name
    /// the kind would refuse are not listed (they could never be loaded).
    pub fn names(&self) -> Vec<String> {
        let mut out: Vec<String> = self.builtins.iter().map(|(n, _)| n.clone()).collect();
        let mut mine: Vec<String> = match std::fs::read_dir(&self.user_dir) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .filter(|e| e.path().is_file())
                .filter_map(|e| e.file_name().into_string().ok())
                .filter_map(|f| f.strip_suffix(".json").map(str::to_owned))
                .filter(|n| self.kind.rule.accepts(n) && !self.is_builtin(n))
                .collect(),
            Err(_) => Vec::new(),
        };
        mine.sort_unstable();
        out.extend(mine);
        out
    }

    /// A built-in or saved profile, validated.
    pub fn load(&self, name: &str) -> Result<Value, ProfileError> {
        let name = self.resolve_alias(name);
        if let Some(v) = self.builtin(name) {
            return (self.validate)(v).map_err(ProfileError::Invalid);
        }
        let path = self.path(name)?;
        let bytes = match fsx::read_limited(&path, MAX_SETTINGS_FILE) {
            Ok(b) => b,
            Err(e) if e.is_not_found() => {
                return Err(ProfileError::NotFound {
                    message: format!("no {} '{name}' ({})", self.kind.label, self.kind.list_hint),
                });
            }
            Err(source) => {
                return Err(ProfileError::Io { op: format!("read {} '{name}'", self.kind.label), source });
            }
        };
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| ProfileError::NotJson {
            message: format!("{} '{name}' is not valid JSON", self.kind.label),
        })?;
        (self.validate)(&value).map_err(ProfileError::Invalid)
    }

    /// Validates and writes a profile, returning what was written. A built-in name is refused
    /// so a built-in can never be altered: copy it under another name instead.
    pub fn save(&self, name: &str, profile: &Value) -> Result<Value, ProfileError> {
        if self.is_builtin(name) {
            return Err(ProfileError::ReadOnly(match self.kind.copy_hint {
                Some(copy) => {
                    format!("'{name}' is built in and read-only; copy it ({copy} {name}) and edit the copy")
                }
                None => format!("'{name}' is built in and read-only; save yours under another name"),
            }));
        }
        let clean = (self.validate)(profile).map_err(ProfileError::Invalid)?;
        let path = self.path(name)?;
        let text = encode(&clean, self.kind.key_order);
        fsx::write_atomic_mkdir(&path, text.as_bytes(), 0o644).map_err(|source| ProfileError::Io {
            op: format!("save {} '{name}'", self.kind.label),
            source,
        })?;
        Ok(clean)
    }

    /// Removes a saved profile.
    pub fn delete(&self, name: &str) -> Result<(), ProfileError> {
        if self.is_builtin(name) {
            return Err(ProfileError::ReadOnly(format!("'{name}' is built in and cannot be deleted")));
        }
        let path = self.path(name)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(ProfileError::NotFound { message: format!("no {} '{name}'", self.kind.label) })
            }
            Err(e) => Err(ProfileError::Io {
                op: format!("delete {} '{name}'", self.kind.label),
                source: FsError::Io { op: "remove", path, source: e },
            }),
        }
    }

    /// Saves a profile under a new name (default `NAME-copy`, numbered `-2`, `-3`... until
    /// free) and returns that name.
    pub fn copy(&self, name: &str, new_name: Option<&str>) -> Result<String, ProfileError> {
        let base = match new_name {
            Some(n) if !n.is_empty() => n.to_owned(),
            _ => format!("{name}-copy"),
        };
        let taken = self.names();
        let mut target = base.clone();
        let mut n = 2u32;
        while taken.contains(&target) {
            target = format!("{base}-{n}");
            n += 1;
        }
        let profile = self.load(name)?;
        self.save(&target, &profile)?;
        Ok(target)
    }
}

/// The file text: indented by one space as the Python edition wrote it, the top-level keys
/// in `order` first and the rest sorted, so the order is the same on every run.
fn encode(value: &Value, order: &[&str]) -> String {
    let Value::Object(map) = value else { return pretty(value) };
    let mut keys: Vec<&String> = Vec::with_capacity(map.len());
    for k in order {
        if let Some((key, _)) = map.get_key_value(*k) {
            keys.push(key);
        }
    }
    keys.extend(map.keys().filter(|k| !order.contains(&k.as_str())));
    if keys.is_empty() {
        return "{}".to_owned();
    }
    let body: Vec<String> = keys
        .into_iter()
        .filter_map(|k| {
            let v = map.get(k)?;
            // A nested value is rendered at depth zero and pushed one level in. JSON strings
            // never contain a raw newline, so the replacement touches only the layout.
            Some(format!(" {}: {}", Value::String(k.clone()), pretty(v).replace('\n', "\n ")))
        })
        .collect();
    format!("{{\n{}\n}}", body.join(",\n"))
}

fn pretty(value: &Value) -> String {
    use serde::Serialize;
    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b" ");
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, formatter);
    if value.serialize(&mut ser).is_err() {
        return value.to_string();
    }
    String::from_utf8(buf).unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::err_expect,
    clippy::field_reassign_with_default
)]
mod tests {
    use super::*;
    use crate::profiles::testutil::Tmp;
    use serde_json::json;

    const KIND: Kind = Kind {
        dir_name: "things",
        label: "thing",
        list_hint: "truesense thing list",
        copy_hint: Some("thing copy"),
        rule: NameRule::Standard,
        aliases: &[("stock", "native")],
        key_order: &["z", "n"],
    };

    fn validator() -> Validator {
        Arc::new(|v: &Value| match v.get("n").and_then(Value::as_i64) {
            Some(n) if (0..=10).contains(&n) => Ok(json!({"n": n})),
            _ => Err("n: 0..10".to_owned()),
        })
    }

    fn store(dir: &Path) -> ProfileStore {
        ProfileStore::new(
            KIND,
            dir.join("things"),
            vec![(NATIVE.into(), json!({"n": 0})), ("big".into(), json!({"n": 9}))],
            validator(),
        )
        .expect("store")
    }

    #[test]
    fn a_store_without_native_is_refused() {
        let e = ProfileStore::new(KIND, "/x", vec![("big".into(), json!({"n": 1}))], validator())
            .err()
            .expect("error");
        assert_eq!(e.to_string(), "the built-in profiles must include 'native'");
    }

    #[test]
    fn name_rules() {
        let std_ok = ["a", "A1", "my-profile_2.x", "..", ".hidden", &"x".repeat(40)];
        for n in std_ok {
            assert!(NameRule::Standard.accepts(n), "{n}");
        }
        let std_bad =
            ["", "a b", "a/b", "../x", "a\\b", "é", "a:b", "a\0", "a\n", &"x".repeat(41), "a*", "~"];
        for n in std_bad {
            assert!(!NameRule::Standard.accepts(n), "{n:?}");
        }
        for n in ["a", "my_profile-1", "é", "名前"] {
            assert!(NameRule::Strict.accepts(n), "{n}");
        }
        for n in ["", "a.b", "a b", "a/b", "../x", "a\0"] {
            assert!(!NameRule::Strict.accepts(n), "{n:?}");
        }
        assert_eq!(NameRule::Standard.message(), "profile names use letters, digits, . _ - (max 40)");
        assert_eq!(NameRule::Strict.message(), "profile names: letters, digits, - and _");
    }

    #[test]
    fn sanitising_makes_acceptable_names_or_nothing() {
        let table = [
            ("My Profile!", Some("My-Profile")),
            ("  spaced   out  ", Some("spaced-out")),
            ("../../etc/passwd", Some("etc-passwd")),
            ("a--b", Some("a-b")),
            ("..", None),
            ("...", None),
            ("", None),
            ("   ", None),
            ("!!!", None),
            ("-x-", Some("x")),
            (".hidden", Some("hidden")),
            ("v1.2", Some("v1.2")),
            ("naïve café", Some("na-ve-caf")),
            ("a_b", Some("a_b")),
            ("tab\tsep", Some("tab-sep")),
        ];
        for (raw, want) in table {
            assert_eq!(sanitise_name(NameRule::Standard, raw).as_deref(), want, "{raw:?}");
        }
        let long = "x".repeat(60);
        assert_eq!(sanitise_name(NameRule::Standard, &long).map(|n| n.len()), Some(40));
        assert_eq!(
            sanitise_name(NameRule::Standard, &format!("{}.-", "y".repeat(39))).as_deref(),
            Some(&*"y".repeat(39))
        );
        assert_eq!(sanitise_name(NameRule::Strict, "naïve café").as_deref(), Some("naïve-café"));
        assert_eq!(sanitise_name(NameRule::Strict, "a.b").as_deref(), Some("a-b"));
        assert_eq!(sanitise_name(NameRule::Strict, "..").as_deref(), None);
        // Whatever comes out is always accepted by its rule.
        for raw in ["\u{0}", "a\u{0}b", "\u{202e}x", "😀", "a😀b", "x/../y", "CON", " . "] {
            for rule in [NameRule::Standard, NameRule::Strict] {
                if let Some(n) = sanitise_name(rule, raw) {
                    assert!(rule.accepts(&n), "{raw:?} -> {n:?}");
                }
            }
        }
    }

    #[test]
    fn builtins_first_then_yours_sorted() {
        let t = Tmp::new("names");
        let s = store(&t.0);
        assert_eq!(s.names(), ["native", "big"]);
        for n in ["zeta", "alpha", "Beta"] {
            s.save(n, &json!({"n": 1})).expect("save");
        }
        assert_eq!(s.names(), ["native", "big", "Beta", "alpha", "zeta"]);
        // Junk in the directory is not listed: wrong suffix, a directory, a name the rule refuses, a built-in's name.
        let dir = s.user_dir();
        std::fs::write(dir.join("notes.txt"), "x").expect("write");
        std::fs::create_dir(dir.join("sub.json")).expect("mkdir");
        std::fs::write(dir.join("bad name.json"), "{}").expect("write");
        std::fs::write(dir.join("native.json"), "{\"n\": 7}").expect("write");
        assert_eq!(s.names(), ["native", "big", "Beta", "alpha", "zeta"]);
        // The built-in wins over a user file of its name.
        assert_eq!(s.load("native").expect("load"), json!({"n": 0}));
        assert_eq!(s.source("native"), Some(Source::BuiltIn));
    }

    #[test]
    fn save_load_roundtrip_and_source() {
        let t = Tmp::new("roundtrip");
        let s = store(&t.0);
        assert_eq!(s.source("mine"), None);
        let saved = s.save("mine", &json!({"n": 4, "extra": "dropped by the validator"})).expect("save");
        assert_eq!(saved, json!({"n": 4}));
        assert_eq!(s.load("mine").expect("load"), json!({"n": 4}));
        assert_eq!(s.source("mine"), Some(Source::Yours));
        assert_eq!(s.source("big"), Some(Source::BuiltIn));
        let text = std::fs::read_to_string(s.user_dir().join("mine.json")).expect("read");
        assert_eq!(text, "{\n \"n\": 4\n}", "one-space indent like the Python edition");
        // No temporary files are left behind.
        let left: Vec<String> = std::fs::read_dir(s.user_dir())
            .expect("dir")
            .map(|e| e.expect("e").file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, ["mine.json"]);
        // Saving again overwrites.
        s.save("mine", &json!({"n": 5})).expect("save");
        assert_eq!(s.load("mine").expect("load"), json!({"n": 5}));
    }

    #[test]
    fn builtins_are_read_only() {
        let t = Tmp::new("builtin");
        let s = store(&t.0);
        let e = s.save("native", &json!({"n": 1})).err().expect("refused").to_string();
        assert_eq!(e, "'native' is built in and read-only; copy it (thing copy native) and edit the copy");
        assert!(matches!(s.save("big", &json!({"n": 1})), Err(ProfileError::ReadOnly(_))));
        assert_eq!(
            s.delete("big").err().expect("refused").to_string(),
            "'big' is built in and cannot be deleted"
        );
        assert!(!s.user_dir().exists(), "nothing was written");
        let mut kind = KIND;
        kind.copy_hint = None;
        let s2 = ProfileStore::new(kind, t.0.join("x"), vec![(NATIVE.into(), json!({"n": 0}))], validator())
            .expect("store");
        assert_eq!(
            s2.save("native", &json!({"n": 1})).err().expect("refused").to_string(),
            "'native' is built in and read-only; save yours under another name"
        );
    }

    #[test]
    fn invalid_content_is_refused_on_save_and_on_load() {
        let t = Tmp::new("invalid");
        let s = store(&t.0);
        assert_eq!(s.save("x", &json!({"n": 11})).err().expect("e").to_string(), "n: 0..10");
        assert!(!s.user_dir().join("x.json").exists());
        std::fs::create_dir_all(s.user_dir()).expect("mkdir");
        std::fs::write(s.user_dir().join("junk.json"), "{ not json").expect("w");
        assert_eq!(s.load("junk").err().expect("e").to_string(), "thing 'junk' is not valid JSON");
        std::fs::write(s.user_dir().join("trail.json"), "{\"n\": 1} x").expect("w");
        assert!(matches!(s.load("trail"), Err(ProfileError::NotJson { .. })));
        std::fs::write(s.user_dir().join("bin.json"), [0xff, 0xfe, 0x00]).expect("w");
        assert!(matches!(s.load("bin"), Err(ProfileError::NotJson { .. })));
        std::fs::write(s.user_dir().join("deep.json"), "[".repeat(10_000)).expect("w");
        assert!(
            matches!(s.load("deep"), Err(ProfileError::NotJson { .. })),
            "deep nesting is an error, not a crash"
        );
        std::fs::write(s.user_dir().join("wrong.json"), "[1]").expect("w");
        assert_eq!(s.load("wrong").err().expect("e").to_string(), "n: 0..10");
        std::fs::write(s.user_dir().join("empty.json"), "").expect("w");
        assert!(matches!(s.load("empty"), Err(ProfileError::NotJson { .. })));
    }

    #[test]
    fn missing_and_hostile_names() {
        let t = Tmp::new("names-hostile");
        let s = store(&t.0);
        assert_eq!(
            s.load("nothere").err().expect("e").to_string(),
            "no thing 'nothere' (truesense thing list)"
        );
        assert_eq!(s.delete("nothere").err().expect("e").to_string(), "no thing 'nothere'");
        for bad in ["../etc/passwd", "a b", "", "x/y", "a\0b", &"x".repeat(41)] {
            assert!(matches!(s.load(bad), Err(ProfileError::BadName(_))), "load {bad:?}");
            assert!(matches!(s.save(bad, &json!({"n": 1})), Err(ProfileError::BadName(_))), "save {bad:?}");
            assert!(matches!(s.delete(bad), Err(ProfileError::BadName(_))), "delete {bad:?}");
            assert_eq!(s.source(bad), None);
        }
        assert!(!t.0.join("etc").exists());
    }

    #[test]
    fn delete_removes_only_yours() {
        let t = Tmp::new("delete");
        let s = store(&t.0);
        s.save("mine", &json!({"n": 1})).expect("save");
        s.delete("mine").expect("delete");
        assert_eq!(s.source("mine"), None);
        assert_eq!(s.delete("mine").err().expect("e").to_string(), "no thing 'mine'");
    }

    #[test]
    fn aliases_load_the_new_name() {
        let t = Tmp::new("alias");
        let s = store(&t.0);
        assert_eq!(s.load("stock").expect("alias"), json!({"n": 0}));
        assert_eq!(s.source("stock"), Some(Source::BuiltIn));
    }

    #[test]
    fn copy_numbers_until_free() {
        let t = Tmp::new("copy");
        let s = store(&t.0);
        assert_eq!(s.copy("big", None).expect("copy"), "big-copy");
        assert_eq!(s.copy("big", None).expect("copy"), "big-copy-2");
        assert_eq!(s.copy("big", None).expect("copy"), "big-copy-3");
        assert_eq!(s.copy("big", Some("mine")).expect("copy"), "mine");
        assert_eq!(s.copy("big", Some("mine")).expect("copy"), "mine-2");
        assert_eq!(s.copy("big", Some("native")).expect("copy"), "native-2", "a built-in name is taken");
        assert_eq!(s.load("big-copy-2").expect("load"), json!({"n": 9}));
        assert!(s.copy("missing", None).is_err());
        assert!(matches!(s.copy("big", Some("bad name")), Err(ProfileError::BadName(_))));
        let long = "y".repeat(40);
        assert!(s.copy(&long, None).is_err());
    }

    #[test]
    fn a_blocked_directory_is_an_io_error_naming_the_operation() {
        let t = Tmp::new("io");
        let blocker = t.0.join("things");
        std::fs::write(&blocker, "a file where the directory should be").expect("w");
        let s = store(&t.0);
        let e = s.save("x", &json!({"n": 1})).err().expect("e");
        assert!(matches!(e, ProfileError::Io { .. }));
        assert!(e.to_string().starts_with("save thing 'x': "), "{e}");
        assert!(std::error::Error::source(&e).is_some());
        assert_eq!(s.names(), ["native", "big"], "an unreadable directory lists only the built-ins");
    }

    #[test]
    fn files_are_written_in_the_declared_key_order_with_one_space_indent() {
        let v = json!({"n": 1, "z": "last in the alphabet", "a": [1, {"b": null}], "m": {}});
        assert_eq!(
            encode(&v, &["z", "n"]),
            "{\n \"z\": \"last in the alphabet\",\n \"n\": 1,\n \"a\": [\n  1,\n  {\n   \"b\": null\n  }\n ],\n \"m\": {}\n}"
        );
        assert_eq!(encode(&json!({}), &["z"]), "{}");
        assert_eq!(encode(&json!([1]), &[]), "[\n 1\n]");
        // The result is valid JSON that reads back identically.
        let back: Value = serde_json::from_str(&encode(&v, &["z", "n"])).expect("json");
        assert_eq!(back, v);
        let tricky = json!({"s": "line\nbreak and \"quotes\" and \u{e9}"});
        assert_eq!(serde_json::from_str::<Value>(&encode(&tricky, &[])).expect("json"), tricky);
    }

    #[test]
    fn default_dir_is_under_config_truesense() {
        assert_eq!(
            default_user_dir(Path::new("/home/u"), &KIND),
            PathBuf::from("/home/u/.config/truesense/things")
        );
    }
}
