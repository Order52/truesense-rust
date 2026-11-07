//! The app's own settings file, `~/.config/truesense/ui.json`: window size and position, theme,
//! language, tray behaviour, accessibility. Plain JSON, written atomically, deliberately **not**
//! `QSettings` (its INI dialect and its desktop-wide cache are not ours to debug).
//!
//! Rules taken from the reference app's hard lessons (APP-DESIGN 1.3 and R5):
//!
//! * a file that is **missing** is written with defaults when something changes;
//! * a file that exists but **cannot be read** (broken JSON, not an object) is never overwritten:
//!   [`UiStore::problem`] says why so the app can show it, [`UiStore::save`] refuses, and
//!   [`UiStore::move_unreadable_aside`] is the explicit way out;
//! * one **wrong entry** (a width that is a string) is dropped, not the whole file; members this
//!   version does not know are kept when it saves, so a newer build's file survives an older one.
#![forbid(unsafe_code)]

use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use ts_foundation::fsx::{self, FsError};

/// The values the `language` setting accepts; `system` follows the desktop. `pseudo` is for tests
/// and is not offered to people. A file naming a language that is not here (a catalogue that was
/// dropped, such as Arabic) loses that one entry and falls back to the default, `system`.
pub(crate) const LANGUAGES: &[&str] = &["system", "en", "pseudo"];
/// The desktop languages that have a catalogue of their own in `i18n/`; any other desktop language
/// gets English. English is always loaded underneath, so it is not listed. Add a code here when a
/// `<code>.ts` file is added.
pub(crate) const CATALOGUES: &[&str] = &[];
/// The colour themes.
pub const THEMES: &[&str] = &["dark", "light", "system"];

/// Smallest window the layout is designed for; saved sizes below it are raised to it. Small on
/// purpose: a tiling compositor gives the window the size of its tile, and a larger minimum makes
/// the window overlap its neighbours (the pages scroll, so a short window loses nothing).
pub(crate) const MIN_WINDOW: (u32, u32) = (320, 260);
/// Largest saved window dimension: a corrupt file must not ask for a 10-million-pixel window.
pub(crate) const MAX_WINDOW: u32 = 16_384;
/// Where the window opens the first time.
pub(crate) const DEFAULT_WINDOW: (u32, u32) = (1000, 680);

/// The settings with their defaults (what a missing file means).
#[derive(Debug, Clone, PartialEq)]
pub struct UiSettings {
    /// Window width in pixels; `None` until the person resizes it.
    pub width: Option<u32>,
    /// Window height in pixels.
    pub height: Option<u32>,
    /// Window x position (not meaningful on Wayland; kept for X11).
    pub x: Option<i32>,
    /// Window y position.
    pub y: Option<i32>,
    /// `dark` (default), `light` or `system`.
    pub theme: String,
    /// One of `system`, `en` or `pseudo` (see `LANGUAGES`); `system` by default.
    pub language: String,
    /// Closing the window keeps the app in the tray (where a tray exists).
    pub minimise_to_tray: bool,
    /// Text scale factor, 0.8 to 1.6.
    pub font_scale: f64,
    /// Turns off gauge easing and other non-essential animation.
    pub reduced_motion: bool,
    /// The first-run wizard was finished or skipped; it is shown once.
    pub onboarded: bool,
    extra: Map<String, Value>,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            width: None,
            height: None,
            x: None,
            y: None,
            theme: "dark".to_owned(),
            language: "system".to_owned(),
            minimise_to_tray: true,
            font_scale: 1.0,
            reduced_motion: false,
            onboarded: false,
            extra: Map::new(),
        }
    }
}

fn clamp_scale(v: f64) -> f64 {
    if v.is_finite() { v.clamp(0.8, 1.6) } else { 1.0 }
}

impl UiSettings {
    /// Reads the settings from a JSON object, dropping each entry of the wrong type or outside its
    /// range. Returns what was dropped, for the log.
    pub fn from_map(map: &Map<String, Value>) -> (Self, Vec<String>) {
        let mut out = Self::default();
        let mut dropped = Vec::new();
        for (key, value) in map {
            let ok = match key.as_str() {
                "width" => set_size(&mut out.width, value),
                "height" => set_size(&mut out.height, value),
                "x" => set_pos(&mut out.x, value),
                "y" => set_pos(&mut out.y, value),
                "theme" => set_choice(&mut out.theme, value, THEMES),
                "language" => set_choice(&mut out.language, value, LANGUAGES),
                "minimise_to_tray" => value.as_bool().map(|b| out.minimise_to_tray = b).is_some(),
                "reduced_motion" => value.as_bool().map(|b| out.reduced_motion = b).is_some(),
                "onboarded" => value.as_bool().map(|b| out.onboarded = b).is_some(),
                "font_scale" => value
                    .as_f64()
                    .filter(|f| f.is_finite())
                    .map(|f| out.font_scale = clamp_scale(f))
                    .is_some(),
                _ => {
                    out.extra.insert(key.clone(), value.clone());
                    true
                }
            };
            if !ok {
                dropped.push(format!("{key}: {value}"));
            }
        }
        (out, dropped)
    }

    /// A string member this version does not manage itself but keeps in the file (`renderer` is read
    /// by the program before Qt starts, so it is not a setting the window changes).
    pub fn extra_str(&self, key: &str) -> Option<&str> {
        self.extra.get(key).and_then(Value::as_str)
    }

    /// The JSON object written to the file (known members, then the ones this version keeps for others).
    pub fn to_map(&self) -> Map<String, Value> {
        let mut map = self.extra.clone();
        let mut put = |k: &str, v: Value| {
            if !v.is_null() {
                map.insert(k.to_owned(), v);
            }
        };
        put("width", json!(self.width));
        put("height", json!(self.height));
        put("x", json!(self.x));
        put("y", json!(self.y));
        put("theme", json!(self.theme));
        put("language", json!(self.language));
        put("minimise_to_tray", json!(self.minimise_to_tray));
        put("font_scale", json!(self.font_scale));
        put("reduced_motion", json!(self.reduced_motion));
        put("onboarded", json!(self.onboarded));
        map
    }

    /// The size to open the window with: the saved one clamped to sane bounds, else the default.
    pub fn window_size(&self) -> (u32, u32) {
        let (w, h) = (self.width.unwrap_or(DEFAULT_WINDOW.0), self.height.unwrap_or(DEFAULT_WINDOW.1));
        (w.clamp(MIN_WINDOW.0, MAX_WINDOW), h.clamp(MIN_WINDOW.1, MAX_WINDOW))
    }

    /// Changes one setting from QML's `key` and JSON `value`.
    ///
    /// # Errors
    /// [`UiError::BadValue`] for an unknown key, a value of the wrong type or an unknown choice.
    pub fn set(&mut self, key: &str, value: &Value) -> Result<(), UiError> {
        let ok = match key {
            "theme" => set_choice(&mut self.theme, value, THEMES),
            "language" => set_choice(&mut self.language, value, LANGUAGES),
            "minimise_to_tray" => value.as_bool().map(|b| self.minimise_to_tray = b).is_some(),
            "reduced_motion" => value.as_bool().map(|b| self.reduced_motion = b).is_some(),
            "onboarded" => value.as_bool().map(|b| self.onboarded = b).is_some(),
            "font_scale" => {
                value.as_f64().filter(|f| f.is_finite()).map(|f| self.font_scale = clamp_scale(f)).is_some()
            }
            _ => false,
        };
        if ok { Ok(()) } else { Err(UiError::BadValue { key: key.to_owned(), value: value.to_string() }) }
    }

    /// Remembers the window geometry (position is optional: a compositor that does not tell it
    /// gives `None`).
    pub fn set_window(&mut self, size: (u32, u32), pos: Option<(i32, i32)>) {
        self.width = Some(size.0.clamp(MIN_WINDOW.0, MAX_WINDOW));
        self.height = Some(size.1.clamp(MIN_WINDOW.1, MAX_WINDOW));
        if let Some((x, y)) = pos {
            self.x = Some(x);
            self.y = Some(y);
        }
    }
}

fn set_size(slot: &mut Option<u32>, v: &Value) -> bool {
    v.as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| (1..=MAX_WINDOW).contains(n))
        .map(|n| *slot = Some(n))
        .is_some()
}

fn set_pos(slot: &mut Option<i32>, v: &Value) -> bool {
    v.as_i64()
        .and_then(|n| i32::try_from(n).ok())
        .filter(|n| n.abs() <= 100_000)
        .map(|n| *slot = Some(n))
        .is_some()
}

fn set_choice(slot: &mut String, v: &Value, allowed: &[&str]) -> bool {
    match v.as_str().filter(|s| allowed.contains(s)) {
        Some(s) => {
            s.clone_into(slot);
            true
        }
        None => false,
    }
}

/// The language to load for the setting `choice` when the desktop's locale is `system_locale`
/// (`de_DE`, `en_US`, ...): `system` follows the desktop, and a desktop language that is not in
/// [`CATALOGUES`] gets English.
pub(crate) fn resolve_language<'a>(choice: &'a str, system_locale: &str) -> &'a str {
    resolve_in(CATALOGUES, choice, system_locale)
}

/// [`resolve_language`] against an explicit table, so the matching is tested with a language that
/// has a catalogue.
fn resolve_in<'a>(catalogues: &[&'a str], choice: &'a str, system_locale: &str) -> &'a str {
    match choice {
        "system" => {
            let lang = system_locale.split(['_', '-', '.', '@']).next().unwrap_or("");
            catalogues.iter().find(|c| c.eq_ignore_ascii_case(lang)).copied().unwrap_or("en")
        }
        other => other,
    }
}

/// What the settings store could not do.
#[derive(Debug)]
pub enum UiError {
    /// A file operation failed.
    Fs(FsError),
    /// The settings file exists but is not readable JSON; it was left untouched.
    Unreadable(String),
    /// A setting QML tried to change does not exist or the value does not fit it.
    BadValue {
        /// The setting's name.
        key: String,
        /// The offered value, as JSON text.
        value: String,
    },
}

impl fmt::Display for UiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fs(e) => write!(f, "settings file: {e}"),
            Self::Unreadable(why) => write!(
                f,
                "the settings file is not valid ({why}) and was left as it is; fix or move it to save settings"
            ),
            Self::BadValue { key, value } => write!(f, "setting '{key}' cannot be {value}"),
        }
    }
}

impl std::error::Error for UiError {}

impl From<FsError> for UiError {
    fn from(e: FsError) -> Self {
        Self::Fs(e)
    }
}

/// The file's state when the store opened it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileState {
    /// There is no file yet: defaults, and saving creates it.
    Missing,
    /// Read fine.
    Loaded,
    /// Exists but cannot be used; the text is why.
    Unreadable(String),
}

/// Where the settings file lives: `$TRUESENSE_UI_CONFIG` (tests and unusual setups), else
/// `$HOME/.config/truesense/ui.json`. `None` when there is no home.
pub fn default_path_from(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    env("TRUESENSE_UI_CONFIG").filter(|p| !p.is_empty()).map(PathBuf::from).or_else(|| {
        env("HOME").filter(|h| !h.is_empty()).map(|h| PathBuf::from(h).join(".config/truesense/ui.json"))
    })
}

/// The settings together with their file.
#[derive(Debug)]
pub struct UiStore {
    path: Option<PathBuf>,
    settings: UiSettings,
    state: FileState,
    dropped: Vec<String>,
}

impl UiStore {
    /// Opens `path` (`None`: no home directory, settings live in memory only).
    pub fn open(path: Option<PathBuf>) -> Self {
        let Some(file) = path.as_deref() else {
            return Self {
                path,
                settings: UiSettings::default(),
                state: FileState::Missing,
                dropped: Vec::new(),
            };
        };
        let (settings, state, dropped) = match fsx::read_limited(file, 1 << 20) {
            Err(e) if e.is_not_found() => (UiSettings::default(), FileState::Missing, Vec::new()),
            Err(e) => (UiSettings::default(), FileState::Unreadable(e.to_string()), Vec::new()),
            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                Ok(Value::Object(map)) => {
                    let (s, dropped) = UiSettings::from_map(&map);
                    (s, FileState::Loaded, dropped)
                }
                Ok(_) => {
                    (UiSettings::default(), FileState::Unreadable("not a JSON object".to_owned()), Vec::new())
                }
                Err(e) => (UiSettings::default(), FileState::Unreadable(e.to_string()), Vec::new()),
            },
        };
        Self { path, settings, state, dropped }
    }

    /// The current settings.
    pub fn settings(&self) -> &UiSettings {
        &self.settings
    }

    /// The file this store reads and writes.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Why the file could not be used, if so; the app shows it and logs it.
    pub fn problem(&self) -> Option<&str> {
        match &self.state {
            FileState::Unreadable(why) => Some(why),
            _ => None,
        }
    }

    /// Entries of the file that were dropped for being the wrong type, for the log.
    pub fn dropped(&self) -> &[String] {
        &self.dropped
    }

    /// Changes one setting and saves.
    ///
    /// # Errors
    /// [`UiError::BadValue`] (nothing changed), or the error of [`Self::save`] (the change stays in
    /// memory for this run).
    pub fn set(&mut self, key: &str, value: &Value) -> Result<(), UiError> {
        self.settings.set(key, value)?;
        self.save()
    }

    /// Remembers the window geometry and saves.
    ///
    /// # Errors
    /// As [`Self::save`].
    pub fn set_window(&mut self, size: (u32, u32), pos: Option<(i32, i32)>) -> Result<(), UiError> {
        self.settings.set_window(size, pos);
        self.save()
    }

    /// Writes the file atomically. Without a home directory this does nothing.
    ///
    /// # Errors
    /// [`UiError::Unreadable`] when the file on disk was not usable at start (it is never
    /// overwritten), or [`UiError::Fs`].
    pub fn save(&mut self) -> Result<(), UiError> {
        if let FileState::Unreadable(why) = &self.state {
            return Err(UiError::Unreadable(why.clone()));
        }
        let Some(path) = &self.path else { return Ok(()) };
        let mut text = serde_json::to_string_pretty(&Value::Object(self.settings.to_map()))
            .map_err(|e| UiError::Unreadable(e.to_string()))?;
        text.push('\n');
        fsx::write_atomic_mkdir(path, text.as_bytes(), 0o644)?;
        self.state = FileState::Loaded;
        Ok(())
    }

    /// The explicit way out of [`FileState::Unreadable`]: renames the bad file to `ui.json.broken`
    /// (so nothing is lost) and lets the next save write a fresh one.
    ///
    /// # Errors
    /// [`UiError::Fs`] when the rename fails.
    pub fn move_unreadable_aside(&mut self) -> Result<(), UiError> {
        let (FileState::Unreadable(_), Some(path)) = (&self.state, &self.path) else { return Ok(()) };
        let mut aside = path.clone().into_os_string();
        aside.push(".broken");
        std::fs::rename(path, &aside)
            .map_err(|e| UiError::Fs(FsError::Io { op: "rename", path: path.clone(), source: e }))?;
        self.state = FileState::Missing;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let p = std::env::temp_dir().join(format!(
                "ts-ui-settings-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn file(&self) -> PathBuf {
            self.0.join("sub/ui.json")
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn unmanaged_string_members_are_kept_and_readable() {
        let map = match json!({"theme": "dark", "renderer": "opengl", "other": 3}) {
            Value::Object(m) => m,
            _ => unreachable!(),
        };
        let (s, dropped) = UiSettings::from_map(&map);
        assert!(dropped.is_empty());
        assert_eq!(s.extra_str("renderer"), Some("opengl"));
        assert_eq!(s.extra_str("other"), None, "not a string");
        assert_eq!(s.extra_str("missing"), None);
        assert_eq!(s.to_map().get("renderer"), Some(&json!("opengl")), "written back");
    }

    #[test]
    fn defaults_when_the_file_is_missing_and_saving_creates_it_with_its_directory() {
        let t = Temp::new();
        let mut s = UiStore::open(Some(t.file()));
        assert_eq!(s.settings(), &UiSettings::default());
        assert_eq!(s.problem(), None);
        assert_eq!(s.settings().window_size(), DEFAULT_WINDOW);
        s.set_window((1200, 800), Some((10, 20))).unwrap();
        let text = std::fs::read_to_string(t.file()).unwrap();
        assert!(text.contains("\"width\": 1200") && text.contains("\"x\": 10"), "{text}");
        let again = UiStore::open(Some(t.file()));
        assert_eq!(again.settings().window_size(), (1200, 800));
        assert_eq!(again.settings().x, Some(10));
    }

    #[test]
    fn an_unreadable_file_is_never_overwritten() {
        let t = Temp::new();
        std::fs::create_dir_all(t.file().parent().unwrap()).unwrap();
        std::fs::write(t.file(), "{ this is not json").unwrap();
        let mut s = UiStore::open(Some(t.file()));
        assert!(s.problem().is_some());
        assert!(matches!(s.set("theme", &json!("light")), Err(UiError::Unreadable(_))));
        assert_eq!(std::fs::read_to_string(t.file()).unwrap(), "{ this is not json");
        // The explicit way out keeps the bad file and then saving works.
        s.move_unreadable_aside().unwrap();
        assert_eq!(
            std::fs::read_to_string(format!("{}.broken", t.file().display())).unwrap(),
            "{ this is not json"
        );
        s.set("theme", &json!("light")).unwrap();
        assert_eq!(UiStore::open(Some(t.file())).settings().theme, "light");
    }

    #[test]
    fn a_json_array_is_unreadable_too() {
        let t = Temp::new();
        std::fs::create_dir_all(t.file().parent().unwrap()).unwrap();
        std::fs::write(t.file(), "[1,2]").unwrap();
        assert_eq!(UiStore::open(Some(t.file())).problem(), Some("not a JSON object"));
    }

    #[test]
    fn one_wrong_entry_is_dropped_and_unknown_members_survive_a_save() {
        let t = Temp::new();
        std::fs::create_dir_all(t.file().parent().unwrap()).unwrap();
        std::fs::write(
            t.file(),
            r#"{"width":"wide","height":700,"theme":"purple","language":"ar","future":{"a":1},"font_scale":9}"#,
        )
        .unwrap();
        let mut s = UiStore::open(Some(t.file()));
        assert_eq!(s.settings().width, None);
        assert_eq!(s.settings().height, Some(700));
        assert_eq!(s.settings().theme, "dark", "an unknown theme falls back to the default");
        assert_eq!(s.settings().language, "system", "a dropped catalogue (Arabic) falls back to the default");
        assert_eq!(s.settings().font_scale, 1.6, "out-of-range scale is clamped");
        assert_eq!(s.dropped().len(), 3);
        s.set("reduced_motion", &json!(true)).unwrap();
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(t.file()).unwrap()).unwrap();
        assert_eq!(saved["future"], json!({"a": 1}));
        assert_eq!(saved["reduced_motion"], json!(true));
        assert!(saved.get("width").is_none(), "an unset size is not written as null");
    }

    #[test]
    fn set_validates_keys_and_values() {
        let mut s = UiSettings::default();
        assert!(s.set("theme", &json!("light")).is_ok());
        assert!(s.set("theme", &json!("neon")).is_err());
        assert!(s.set("theme", &json!(3)).is_err());
        assert!(s.set("language", &json!("pseudo")).is_ok());
        assert!(s.set("language", &json!("xx")).is_err());
        assert!(s.set("minimise_to_tray", &json!("yes")).is_err());
        assert!(s.set("minimise_to_tray", &json!(false)).is_ok());
        assert!(s.set("onboarded", &json!(true)).is_ok() && s.onboarded);
        assert!(s.set("onboarded", &json!("yes")).is_err());
        assert!(s.set("font_scale", &json!(0.1)).is_ok());
        assert_eq!(s.font_scale, 0.8);
        assert!(s.set("width", &json!(100)).is_err(), "geometry is set with set_window only");
        assert!(s.set("nope", &json!(1)).is_err());
    }

    #[test]
    fn window_size_is_clamped_to_sane_bounds() {
        let mut s = UiSettings::default();
        s.set_window((10, 10), None);
        assert_eq!(s.window_size(), MIN_WINDOW);
        s.set_window((u32::MAX, u32::MAX), None);
        assert_eq!(s.window_size(), (MAX_WINDOW, MAX_WINDOW));
        let (loaded, dropped) = UiSettings::from_map(
            json!({"width": 0, "height": 99999999999u64, "x": 1e9}).as_object().unwrap(),
        );
        assert_eq!(dropped.len(), 3);
        assert_eq!(loaded.window_size(), DEFAULT_WINDOW);
    }

    #[test]
    fn default_path_prefers_the_override_then_home() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| (*v).to_owned())
        };
        assert_eq!(
            default_path_from(&env(&[("HOME", "/h")])),
            Some(PathBuf::from("/h/.config/truesense/ui.json"))
        );
        assert_eq!(
            default_path_from(&env(&[("HOME", "/h"), ("TRUESENSE_UI_CONFIG", "/x/u.json")])),
            Some(PathBuf::from("/x/u.json"))
        );
        assert_eq!(default_path_from(&env(&[])), None);
        assert_eq!(default_path_from(&env(&[("HOME", "")])), None);
    }

    #[test]
    fn language_follows_the_desktop_only_when_asked_to() {
        assert_eq!(resolve_language("system", "en_US"), "en");
        assert_eq!(resolve_language("system", "de_DE"), "en", "no catalogue: English");
        assert_eq!(resolve_language("system", "ar_EG.UTF-8"), "en", "Arabic was dropped: English");
        assert_eq!(resolve_language("system", ""), "en");
        assert_eq!(resolve_language("en", "ar_EG"), "en");
        assert_eq!(resolve_language("pseudo", "de"), "pseudo");
    }

    #[test]
    fn a_language_with_a_catalogue_is_matched_by_its_code_only_when_asked_to() {
        let table = ["de", "pt"];
        assert_eq!(resolve_in(&table, "system", "de_DE.UTF-8"), "de");
        assert_eq!(resolve_in(&table, "system", "PT-br"), "pt");
        assert_eq!(resolve_in(&table, "system", "fr_FR"), "en");
        assert_eq!(resolve_in(&table, "en", "de_DE"), "en");
    }

    #[test]
    fn without_a_path_everything_stays_in_memory() {
        let mut s = UiStore::open(None);
        s.set("theme", &json!("light")).unwrap();
        assert_eq!(s.settings().theme, "light");
        assert!(s.path().is_none());
    }
}
