//! favourites: the user's short, ordered list of starred library items.
//!
//! A favourite is a *reference* (`kind` and `name`) to something that lives in one of the
//! libraries: a lighting look, a fan curve, a Smart auto profile or a whole-setup profile. The
//! list is what the Home page shows as chips and what the PREDATOR key reaches with the actions
//! `favourite:N` and `favourite-next`, so it lives in a file both the window and the key service
//! (which runs as the user) can read: `~/.config/truesense/favourites.json`, next to the user's
//! configuration.
//!
//! Only things that are safe to apply with one tap can be starred. An undervolt or a power limit
//! needs a confirmation and a danger line, so there is deliberately no kind for them.
//!
//! # File format
//!
//! ```json
//! {"version": 1, "items": [{"kind": "look", "name": "calm"}, {"kind": "fan_curve", "name": "quiet"}]}
//! ```
//!
//! The reader is forgiving and the writer is polite to other versions of the program: a missing
//! or damaged file is an empty list; an entry of a kind this build does not know, an entry with
//! extra fields and unknown top-level fields are all kept and written back, so a newer build's
//! favourites survive an edit by an older one (entries of unknown kinds are moved behind the
//! known ones, and never counted, listed or applied here).
//!
//! # Concurrency
//!
//! The window, the command line and the key service may all edit the list. Every change is a
//! read-modify-write under an exclusive `flock` on `favourites.json.lock` and ends with an atomic
//! rename ([`ts_foundation::fsx::write_atomic_mkdir`]), so edits never interleave and a reader
//! never sees half a file. `flock` locks are per open file, so the lock also serialises threads
//! of one process that each use their own [`FavouriteStore`].
#![forbid(unsafe_code)]

use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use ts_foundation::fsx::{self, FileLock, FsError};

/// How many favourites can be kept: the Home strip has to stay readable and the PREDATOR key
/// cycles through all of them, so a longer list stops being "favourites".
pub const MAX_FAVOURITES: usize = 24;

/// The file name beside the user configuration.
pub(crate) const FILE_NAME: &str = "favourites.json";

/// The longest favourite name accepted (library names are short; this only bounds hostile input).
const MAX_NAME_CHARS: usize = 128;

/// The format version written to the file.
const VERSION: i64 = 1;

/// What a favourite refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A saved lighting look (keyboard and rear light together).
    Look,
    /// A fan curve (built-in or saved).
    FanCurve,
    /// A Smart auto profile.
    SmartProfile,
    /// A whole-setup profile.
    Profile,
}

impl Kind {
    /// Every kind, in the order the interfaces list them.
    pub const ALL: [Kind; 4] = [Kind::Look, Kind::FanCurve, Kind::SmartProfile, Kind::Profile];

    /// The word in the file, on the bus and in JSON.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Look => "look",
            Kind::FanCurve => "fan_curve",
            Kind::SmartProfile => "smart_profile",
            Kind::Profile => "profile",
        }
    }

    /// The label a message starts with (`Look: calm`).
    pub fn label(self) -> &'static str {
        match self {
            Kind::Look => "Look",
            Kind::FanCurve => "Fan curve",
            Kind::SmartProfile => "Smart profile",
            Kind::Profile => "Profile",
        }
    }

    /// Reads the file word, and also the shorter spellings a person types on the command line
    /// (`curve`, `fan-curve`, `smart`, `smart-profile`).
    pub fn parse(text: &str) -> Option<Kind> {
        match text.trim() {
            "look" => Some(Kind::Look),
            "fan_curve" | "fan-curve" | "curve" => Some(Kind::FanCurve),
            "smart_profile" | "smart-profile" | "smart" => Some(Kind::SmartProfile),
            "profile" => Some(Kind::Profile),
            _ => None,
        }
    }

    /// The accepted words in one line, for error messages.
    pub fn choices() -> String {
        Kind::ALL.iter().map(|k| k.name()).collect::<Vec<_>>().join(", ")
    }
}

impl std::str::FromStr for Kind {
    type Err = FavouritesError;

    /// [`Kind::parse`] with the error a client shows: the words that are accepted.
    fn from_str(text: &str) -> Result<Kind, FavouritesError> {
        Kind::parse(text).ok_or_else(|| {
            FavouritesError::Invalid(format!("'{}': the kind is one of {}", text.trim(), Kind::choices()))
        })
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// One starred item: what kind of thing and its name in that library.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Favourite {
    /// The library the name belongs to.
    pub kind: Kind,
    /// The item's name there.
    pub name: String,
}

impl Favourite {
    /// A favourite of this kind and name (not checked; the store checks on [`FavouriteStore::add`]).
    pub fn new(kind: Kind, name: impl Into<String>) -> Favourite {
        Favourite { kind, name: name.into() }
    }
}

impl fmt::Display for Favourite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.kind, self.name)
    }
}

/// Why a favourites operation failed.
#[derive(Debug)]
pub enum FavouritesError {
    /// There is no place to keep the file (no home directory).
    NoLocation,
    /// The name or position is not usable; the text says why.
    Invalid(String),
    /// [`MAX_FAVOURITES`] are already starred.
    Full,
    /// The file could not be written.
    File(FsError),
}

impl fmt::Display for FavouritesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FavouritesError::NoLocation => {
                f.write_str("there is no place to keep favourites (HOME is not set)")
            }
            FavouritesError::Invalid(m) => f.write_str(m),
            FavouritesError::Full => {
                write!(f, "favourites are full ({MAX_FAVOURITES}): remove one first")
            }
            FavouritesError::File(e) => write!(f, "favourites: {e}"),
        }
    }
}

impl std::error::Error for FavouritesError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FavouritesError::File(e) => Some(e),
            _ => None,
        }
    }
}

impl From<FsError> for FavouritesError {
    fn from(e: FsError) -> Self {
        FavouritesError::File(e)
    }
}

/// The position after `previous` in a cycle of `len` entries, wrapping; `previous` is whatever
/// the user state held (a hand-edited or stale value starts the cycle at the first entry again,
/// a position past a list that shrank wraps too). `None` for an empty list.
///
/// Shared by the key action and the command line so both continue from the same place.
pub fn next_position(previous: Option<i64>, len: usize) -> Option<usize> {
    let len = i64::try_from(len).ok().filter(|l| *l > 0)?;
    let next = previous.filter(|p| *p >= 0).map_or(0, |p| p.saturating_add(1).rem_euclid(len));
    usize::try_from(next).ok()
}

/// Where the file lives given the user's configuration file: beside it, so a `TRUESENSE_CONFIG`
/// override (tests, a second profile of the program) moves the favourites with it.
pub(crate) fn path_beside(user_config: &Path) -> PathBuf {
    user_config.parent().unwrap_or(Path::new("")).join(FILE_NAME)
}

/// One known entry with the fields of it this build does not understand.
#[derive(Debug, Clone)]
struct Entry {
    fav: Favourite,
    extra: Map<String, Value>,
}

/// The whole file in memory.
#[derive(Debug, Default)]
struct Doc {
    entries: Vec<Entry>,
    /// Entries of kinds this build does not know, written back untouched.
    foreign: Vec<Value>,
    /// Top-level fields other than `version` and `items`.
    top: Map<String, Value>,
}

impl Doc {
    fn parse(bytes: &[u8]) -> Doc {
        let Ok(Value::Object(mut top)) = serde_json::from_slice::<Value>(bytes) else {
            return Doc::default();
        };
        top.remove("version");
        let items = match top.remove("items") {
            Some(Value::Array(a)) => a,
            _ => Vec::new(),
        };
        let mut doc = Doc { top, ..Doc::default() };
        for item in items {
            let Value::Object(mut obj) = item else { continue };
            let kind = obj.get("kind").and_then(Value::as_str).map(str::to_owned);
            let name = obj.get("name").and_then(Value::as_str).map(str::to_owned);
            match (kind.as_deref().and_then(Kind::parse), name) {
                (Some(kind), Some(name)) if valid_name(&name).is_ok() => {
                    obj.remove("kind");
                    obj.remove("name");
                    let fav = Favourite { kind, name };
                    // A hand-edited file may list one twice; the first wins.
                    if doc.entries.iter().all(|e| e.fav != fav) {
                        doc.entries.push(Entry { fav, extra: obj });
                    }
                }
                // A kind we do not know (a newer build's): keep it as it is.
                (None, _) if kind.is_some() => doc.foreign.push(Value::Object(obj)),
                // No usable kind or name: damaged, dropped.
                _ => {}
            }
        }
        doc
    }

    fn render(&self) -> Result<Vec<u8>, FavouritesError> {
        let mut items: Vec<Value> = self
            .entries
            .iter()
            .map(|e| {
                let mut obj = Map::new();
                obj.insert("kind".to_owned(), json!(e.fav.kind.name()));
                obj.insert("name".to_owned(), json!(e.fav.name));
                for (k, v) in &e.extra {
                    obj.insert(k.clone(), v.clone());
                }
                Value::Object(obj)
            })
            .collect();
        items.extend(self.foreign.iter().cloned());
        let mut top = self.top.clone();
        top.insert("version".to_owned(), json!(VERSION));
        top.insert("items".to_owned(), Value::Array(items));
        let mut bytes = serde_json::to_vec_pretty(&Value::Object(top))
            .map_err(|e| FavouritesError::Invalid(format!("favourites could not be encoded: {e}")))?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    fn position(&self, kind: Kind, name: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.fav.kind == kind && e.fav.name == name)
    }
}

/// A name is a non-empty line of ordinary text: it becomes a file name or a settings key
/// elsewhere, and is shown on screen, so control characters are refused.
fn valid_name(name: &str) -> Result<(), FavouritesError> {
    if name.trim().is_empty() {
        return Err(FavouritesError::Invalid("a favourite needs a name".into()));
    }
    if name.chars().count() > MAX_NAME_CHARS || name.chars().any(char::is_control) {
        return Err(FavouritesError::Invalid(format!(
            "'{}' is not a usable favourite name",
            name.chars().filter(|c| !c.is_control()).take(40).collect::<String>()
        )));
    }
    Ok(())
}

/// The favourites file. Cheap to create; it keeps no state in memory, every call reads the file
/// again so a change by another process is seen at once.
#[derive(Debug, Clone)]
pub struct FavouriteStore {
    path: PathBuf,
}

impl FavouriteStore {
    /// A store on the file at `path`. Nothing is read or created yet.
    pub fn new(path: impl Into<PathBuf>) -> FavouriteStore {
        FavouriteStore { path: path.into() }
    }

    /// The store beside the user's configuration file.
    pub fn beside(user_config: &Path) -> FavouriteStore {
        FavouriteStore::new(path_beside(user_config))
    }

    /// The file this store uses.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read(&self) -> Doc {
        match fsx::read_limited(&self.path, fsx::MAX_SETTINGS_FILE) {
            Ok(bytes) => Doc::parse(&bytes),
            Err(_) => Doc::default(),
        }
    }

    /// The favourites in order. A missing, unreadable or damaged file is an empty list: a list of
    /// shortcuts must never stop a key press or a page from loading.
    pub fn list(&self) -> Vec<Favourite> {
        self.read().entries.into_iter().map(|e| e.fav).collect()
    }

    /// The favourite at a 0-based position.
    pub fn get(&self, index: usize) -> Option<Favourite> {
        self.list().into_iter().nth(index)
    }

    /// Whether this item is starred.
    pub fn contains(&self, kind: Kind, name: &str) -> bool {
        self.read().position(kind, name).is_some()
    }

    /// Runs one read-modify-write under the cross-process lock. `change` answers whether the
    /// document changed (an unchanged one is not rewritten, so a no-op never touches the disk).
    fn edit<T>(
        &self,
        change: impl FnOnce(&mut Doc) -> Result<(T, bool), FavouritesError>,
    ) -> Result<T, FavouritesError> {
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            fsx::ensure_dir(dir, 0o755)?;
        }
        let mut lock_path = self.path.clone().into_os_string();
        lock_path.push(".lock");
        // Without the lock (a read-only directory, say) the write below fails with a clear
        // message anyway; losing a race on a convenience file is not worth refusing the edit.
        let _lock = FileLock::exclusive(Path::new(&lock_path)).ok();
        let mut doc = self.read();
        let (value, changed) = change(&mut doc)?;
        if changed {
            fsx::write_atomic_mkdir(&self.path, &doc.render()?, 0o644)?;
        }
        Ok(value)
    }

    /// Stars an item at the end of the list. Returns `false` when it was already starred (the
    /// list is unchanged: no duplicates); [`FavouritesError::Full`] at [`MAX_FAVOURITES`].
    pub fn add(&self, kind: Kind, name: &str) -> Result<bool, FavouritesError> {
        valid_name(name)?;
        self.edit(|doc| {
            if doc.position(kind, name).is_some() {
                return Ok((false, false));
            }
            if doc.entries.len() >= MAX_FAVOURITES {
                return Err(FavouritesError::Full);
            }
            doc.entries.push(Entry { fav: Favourite::new(kind, name), extra: Map::new() });
            Ok((true, true))
        })
    }

    /// Un-stars an item; `false` when it was not starred.
    pub fn remove(&self, kind: Kind, name: &str) -> Result<bool, FavouritesError> {
        self.edit(|doc| match doc.position(kind, name) {
            Some(i) => {
                doc.entries.remove(i);
                Ok((true, true))
            }
            None => Ok((false, false)),
        })
    }

    /// Stars the item when it is not starred and un-stars it when it is: the star button.
    /// Returns whether it is starred afterwards.
    pub fn toggle(&self, kind: Kind, name: &str) -> Result<bool, FavouritesError> {
        valid_name(name)?;
        self.edit(|doc| match doc.position(kind, name) {
            Some(i) => {
                doc.entries.remove(i);
                Ok((false, true))
            }
            None => {
                if doc.entries.len() >= MAX_FAVOURITES {
                    return Err(FavouritesError::Full);
                }
                doc.entries.push(Entry { fav: Favourite::new(kind, name), extra: Map::new() });
                Ok((true, true))
            }
        })
    }

    /// Moves the entry at 0-based `from` so that it ends up at 0-based `to` (the others shift),
    /// which is what dragging a chip in the list does.
    pub fn move_to(&self, from: usize, to: usize) -> Result<(), FavouritesError> {
        self.edit(|doc| {
            let len = doc.entries.len();
            if from >= len || to >= len {
                return Err(FavouritesError::Invalid(if len == 0 {
                    "there are no favourites yet".to_owned()
                } else {
                    format!("position out of range: favourites are 1..{len}")
                }));
            }
            if from == to {
                return Ok(((), false));
            }
            let entry = doc.entries.remove(from);
            doc.entries.insert(to, entry);
            Ok(((), true))
        })
    }

    /// Removes every favourite whose item `exists` no longer finds (a look that was deleted) and
    /// returns what was removed, in order. Nothing is written when nothing is dangling.
    pub fn prune(&self, exists: impl Fn(&Favourite) -> bool) -> Result<Vec<Favourite>, FavouritesError> {
        self.edit(|doc| {
            let (keep, gone): (Vec<Entry>, Vec<Entry>) =
                std::mem::take(&mut doc.entries).into_iter().partition(|e| exists(&e.fav));
            doc.entries = keep;
            let changed = !gone.is_empty();
            Ok((gone.into_iter().map(|e| e.fav).collect(), changed))
        })
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Barrier};

    struct Dir(PathBuf);

    impl Dir {
        fn new() -> Dir {
            static N: AtomicU32 = AtomicU32::new(0);
            let p = std::env::temp_dir().join(format!(
                "ts-favourites-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&p).expect("temp dir");
            Dir(p)
        }

        fn store(&self) -> FavouriteStore {
            FavouriteStore::new(self.0.join("sub").join(FILE_NAME))
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fav(kind: Kind, name: &str) -> Favourite {
        Favourite::new(kind, name)
    }

    #[test]
    fn missing_file_is_an_empty_list_and_add_creates_the_directory() {
        let dir = Dir::new();
        let s = dir.store();
        assert!(s.list().is_empty());
        assert!(!s.path().exists());
        assert_eq!(s.add(Kind::Look, "calm").ok(), Some(true));
        assert_eq!(s.list(), vec![fav(Kind::Look, "calm")]);
        let text = std::fs::read_to_string(s.path()).expect("file");
        assert!(text.contains("\"version\": 1") && text.contains("\"kind\": \"look\""), "{text}");
    }

    #[test]
    fn corrupt_files_are_empty_lists_and_are_replaced_by_the_next_edit() {
        let dir = Dir::new();
        let s = dir.store();
        std::fs::create_dir_all(s.path().parent().unwrap()).unwrap();
        let hostile: [&[u8]; 8] = [
            b"",
            b"not json at all",
            b"[1,2,3]",
            b"null",
            b"{\"items\": 7}",
            b"{\"items\": [1, \"x\", null, {\"kind\": 3, \"name\": \"a\"}, {\"kind\": \"look\"}]}",
            b"\xff\xfe\x00garbage",
            b"{\"items\": [{\"kind\":\"look\",\"name\":\"ok\"},",
        ];
        for bytes in hostile {
            std::fs::write(s.path(), bytes).unwrap();
            assert!(s.list().is_empty(), "{bytes:?}");
        }
        std::fs::write(s.path(), b"{{{").unwrap();
        assert_eq!(s.add(Kind::Profile, "gaming").ok(), Some(true));
        assert_eq!(s.list(), vec![fav(Kind::Profile, "gaming")]);
    }

    #[test]
    fn an_oversized_file_is_ignored() {
        let dir = Dir::new();
        let s = dir.store();
        std::fs::create_dir_all(s.path().parent().unwrap()).unwrap();
        std::fs::write(s.path(), vec![b' '; (fsx::MAX_SETTINGS_FILE + 1) as usize]).unwrap();
        assert!(s.list().is_empty());
    }

    #[test]
    fn order_is_preserved_and_duplicates_are_refused() {
        let dir = Dir::new();
        let s = dir.store();
        for (k, n) in [(Kind::Look, "b"), (Kind::FanCurve, "a"), (Kind::Look, "a"), (Kind::SmartProfile, "a")]
        {
            assert_eq!(s.add(k, n).ok(), Some(true));
        }
        assert_eq!(s.add(Kind::Look, "a").ok(), Some(false), "same kind and name");
        assert_eq!(s.add(Kind::Profile, "a").ok(), Some(true), "same name, other kind");
        let names: Vec<String> = s.list().iter().map(ToString::to_string).collect();
        assert_eq!(names, ["look b", "fan_curve a", "look a", "smart_profile a", "profile a"]);
        assert_eq!(s.get(2), Some(fav(Kind::Look, "a")));
        assert_eq!(s.get(9), None);
        assert!(s.contains(Kind::FanCurve, "a") && !s.contains(Kind::FanCurve, "b"));
    }

    #[test]
    fn a_duplicate_in_a_hand_edited_file_collapses_to_the_first() {
        let dir = Dir::new();
        let s = dir.store();
        std::fs::create_dir_all(s.path().parent().unwrap()).unwrap();
        std::fs::write(
            s.path(),
            br#"{"items":[{"kind":"look","name":"a"},{"kind":"profile","name":"p"},{"kind":"look","name":"a"}]}"#,
        )
        .unwrap();
        assert_eq!(s.list(), vec![fav(Kind::Look, "a"), fav(Kind::Profile, "p")]);
    }

    #[test]
    fn the_list_is_capped() {
        let dir = Dir::new();
        let s = dir.store();
        for i in 0..MAX_FAVOURITES {
            assert_eq!(s.add(Kind::Look, &format!("l{i}")).ok(), Some(true));
        }
        let err = s.add(Kind::Look, "one-too-many").expect_err("full");
        assert!(matches!(err, FavouritesError::Full));
        assert_eq!(err.to_string(), "favourites are full (24): remove one first");
        assert!(matches!(s.toggle(Kind::Profile, "x"), Err(FavouritesError::Full)));
        // An existing one is still a quiet no-op, and removing makes room.
        assert_eq!(s.add(Kind::Look, "l0").ok(), Some(false));
        assert_eq!(s.remove(Kind::Look, "l0").ok(), Some(true));
        assert_eq!(s.add(Kind::Look, "one-too-many").ok(), Some(true));
        assert_eq!(s.list().len(), MAX_FAVOURITES);
    }

    #[test]
    fn a_file_longer_than_the_cap_is_read_whole_but_cannot_grow() {
        let dir = Dir::new();
        let s = dir.store();
        std::fs::create_dir_all(s.path().parent().unwrap()).unwrap();
        let items: Vec<Value> = (0..30).map(|i| json!({"kind": "look", "name": format!("l{i}")})).collect();
        std::fs::write(s.path(), json!({"items": items}).to_string()).unwrap();
        assert_eq!(s.list().len(), 30);
        assert!(matches!(s.add(Kind::Look, "new"), Err(FavouritesError::Full)));
        assert_eq!(s.remove(Kind::Look, "l0").ok(), Some(true));
        assert_eq!(s.list().len(), 29);
    }

    #[test]
    fn remove_and_toggle() {
        let dir = Dir::new();
        let s = dir.store();
        assert_eq!(s.remove(Kind::Look, "none").ok(), Some(false));
        assert_eq!(s.toggle(Kind::Look, "calm").ok(), Some(true));
        assert!(s.contains(Kind::Look, "calm"));
        assert_eq!(s.toggle(Kind::Look, "calm").ok(), Some(false));
        assert!(s.list().is_empty());
        assert_eq!(s.toggle(Kind::FanCurve, "q").ok(), Some(true));
        assert_eq!(s.remove(Kind::FanCurve, "q").ok(), Some(true));
        assert!(s.list().is_empty());
    }

    #[test]
    fn move_reorders_and_checks_the_range() {
        let dir = Dir::new();
        let s = dir.store();
        for n in ["a", "b", "c", "d"] {
            s.add(Kind::Look, n).unwrap();
        }
        let order = |s: &FavouriteStore| s.list().into_iter().map(|f| f.name).collect::<String>();
        s.move_to(0, 2).unwrap();
        assert_eq!(order(&s), "bcad");
        s.move_to(3, 0).unwrap();
        assert_eq!(order(&s), "dbca");
        s.move_to(1, 1).unwrap();
        assert_eq!(order(&s), "dbca");
        let err = s.move_to(0, 4).expect_err("range");
        assert_eq!(err.to_string(), "position out of range: favourites are 1..4");
        assert!(s.move_to(7, 0).is_err());
        assert_eq!(order(&s), "dbca", "a refused move changes nothing");
        let empty = Dir::new().store();
        assert_eq!(empty.move_to(0, 0).expect_err("empty").to_string(), "there are no favourites yet");
    }

    #[test]
    fn prune_removes_dangling_entries_in_order_and_skips_the_write_when_clean() {
        let dir = Dir::new();
        let s = dir.store();
        for (k, n) in
            [(Kind::Look, "keep"), (Kind::Look, "gone1"), (Kind::Profile, "keep2"), (Kind::FanCurve, "gone2")]
        {
            s.add(k, n).unwrap();
        }
        let gone = s.prune(|f| f.name.starts_with("keep")).unwrap();
        assert_eq!(gone, vec![fav(Kind::Look, "gone1"), fav(Kind::FanCurve, "gone2")]);
        assert_eq!(s.list(), vec![fav(Kind::Look, "keep"), fav(Kind::Profile, "keep2")]);
        let before = std::fs::metadata(s.path()).unwrap().modified().unwrap();
        assert!(s.prune(|_| true).unwrap().is_empty());
        assert_eq!(std::fs::metadata(s.path()).unwrap().modified().unwrap(), before);
    }

    #[test]
    fn unknown_kinds_fields_and_top_level_keys_survive_an_edit() {
        let dir = Dir::new();
        let s = dir.store();
        std::fs::create_dir_all(s.path().parent().unwrap()).unwrap();
        std::fs::write(
            s.path(),
            br##"{"version": 2, "theme": "x", "items": [
                {"kind": "look", "name": "a", "colour": "#fff"},
                {"kind": "hologram", "name": "h", "size": 3},
                {"kind": "profile", "name": "p"}]}"##,
        )
        .unwrap();
        assert_eq!(
            s.list(),
            vec![fav(Kind::Look, "a"), fav(Kind::Profile, "p")],
            "unknown kinds are not listed"
        );
        s.add(Kind::Look, "b").unwrap();
        s.move_to(0, 2).unwrap();
        let v: Value = serde_json::from_slice(&std::fs::read(s.path()).unwrap()).unwrap();
        assert_eq!(v["version"], 1, "this build writes its own version");
        assert_eq!(v["theme"], "x");
        let items = v["items"].as_array().unwrap();
        assert_eq!(items.len(), 4);
        assert_eq!(items[0], json!({"kind": "profile", "name": "p"}));
        assert_eq!(items[1], json!({"kind": "look", "name": "b"}));
        assert_eq!(items[2], json!({"kind": "look", "name": "a", "colour": "#fff"}));
        assert_eq!(items[3], json!({"kind": "hologram", "name": "h", "size": 3}));
    }

    #[test]
    fn names_are_checked() {
        let dir = Dir::new();
        let s = dir.store();
        for bad in ["", "   ", "a\nb", "a\u{0}b", &"x".repeat(MAX_NAME_CHARS + 1)] {
            assert!(matches!(s.add(Kind::Look, bad), Err(FavouritesError::Invalid(_))), "{bad:?}");
            assert!(matches!(s.toggle(Kind::Look, bad), Err(FavouritesError::Invalid(_))), "{bad:?}");
        }
        assert!(s.list().is_empty());
        assert!(!s.path().exists(), "a refused name does not create the file");
        assert_eq!(s.add(Kind::Look, "Gaming night (v2) \u{2605}").ok(), Some(true));
        assert_eq!(s.add(Kind::Look, &"x".repeat(MAX_NAME_CHARS)).ok(), Some(true));
    }

    #[test]
    fn kinds_parse_their_words_and_the_short_spellings() {
        for k in Kind::ALL {
            assert_eq!(Kind::parse(k.name()), Some(k));
        }
        assert_eq!(Kind::parse("curve"), Some(Kind::FanCurve));
        assert_eq!(Kind::parse("fan-curve"), Some(Kind::FanCurve));
        assert_eq!(Kind::parse("smart"), Some(Kind::SmartProfile));
        assert_eq!(Kind::parse(" look "), Some(Kind::Look));
        assert_eq!(Kind::parse("undervolt"), None);
        assert_eq!(Kind::parse(""), None);
        assert_eq!(Kind::choices(), "look, fan_curve, smart_profile, profile");
        assert_eq!("curve".parse::<Kind>().ok(), Some(Kind::FanCurve));
        assert_eq!(
            "undervolt".parse::<Kind>().expect_err("not a kind").to_string(),
            "'undervolt': the kind is one of look, fan_curve, smart_profile, profile"
        );
    }

    #[test]
    fn next_position_wraps_and_survives_stale_values() {
        assert_eq!(next_position(None, 3), Some(0));
        assert_eq!(next_position(Some(0), 3), Some(1));
        assert_eq!(next_position(Some(2), 3), Some(0));
        assert_eq!(next_position(Some(7), 3), Some(2), "a list that shrank wraps");
        assert_eq!(next_position(Some(-5), 3), Some(0));
        assert_eq!(next_position(Some(i64::MAX), 3), Some(1), "no overflow");
        assert_eq!(next_position(Some(1), 0), None);
    }

    #[test]
    fn the_file_sits_beside_the_user_configuration() {
        assert_eq!(
            path_beside(Path::new("/home/u/.config/truesense/config.toml")),
            PathBuf::from("/home/u/.config/truesense/favourites.json")
        );
        assert_eq!(path_beside(Path::new("config.toml")), PathBuf::from("favourites.json"));
    }

    #[test]
    fn concurrent_writers_do_not_lose_each_other() {
        let dir = Dir::new();
        let path = dir.store().path().to_path_buf();
        let threads = 12;
        let barrier = Arc::new(Barrier::new(threads));
        let handles: Vec<_> = (0..threads)
            .map(|i| {
                let path = path.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    // Each writer has its own store, as the window, the CLI and the key service do.
                    let s = FavouriteStore::new(path);
                    barrier.wait();
                    s.add(Kind::Look, &format!("look{i}")).expect("add")
                })
            })
            .collect();
        for h in handles {
            assert!(h.join().expect("thread"));
        }
        let mut names: Vec<String> = FavouriteStore::new(&path).list().into_iter().map(|f| f.name).collect();
        names.sort();
        let mut want: Vec<String> = (0..threads).map(|i| format!("look{i}")).collect();
        want.sort();
        assert_eq!(names, want);
    }

    #[test]
    fn an_unwritable_location_is_an_error_not_a_panic() {
        let dir = Dir::new();
        // The parent "directory" is a file.
        let blocker = dir.0.join("blocker");
        std::fs::write(&blocker, b"x").unwrap();
        let s = FavouriteStore::new(blocker.join(FILE_NAME));
        assert!(matches!(s.add(Kind::Look, "a"), Err(FavouritesError::File(_))));
        assert!(s.list().is_empty());
    }
}
