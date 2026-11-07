//! The themes the user can pick from, and the one that is running.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use ts_foundation::fsx::{self, FsError};
use ts_foundation::userstate::Store;
use ts_fx::engine::{Effect, Flash};
use ts_fx::themes::{MAX_NAME, Theme, Values, builtin, find, is_valid_id};

use super::spec::ThemeSpec;
use super::{ThemesError, invalid};

/// Where the running theme is kept in the user state: the key service reads it.
pub const ACTIVE_KEY: &str = "theme_active";
/// Where the reason the last theme stopped by itself is kept, for `theme show`.
pub const HALT_KEY: &str = "theme_halt";
/// The largest theme file read; the catalogue's biggest theme is a few kilobytes.
pub const MAX_THEME_FILE: u64 = 256 * 1024;
/// How many user themes are read; a directory with more is cut off, so a full disk of files
/// cannot stall a listing.
const MAX_USER_THEMES: usize = 500;

/// One theme in a listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ThemeEntry {
    /// The id.
    pub id: String,
    /// The name, English.
    pub name: String,
    /// The one-sentence mood.
    pub mood: String,
    /// Built in (read-only) or the user's own.
    pub builtin: bool,
    /// The rank among the twelve shown first (1 to 12) for a built-in theme; 0 for the rest.
    pub top: u8,
    /// Whether it can be run on this keyboard: a theme whose keyboard look has no flash
    /// measurement is listed but refused.
    pub runnable: bool,
}

/// A user file that was not used, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Skipped {
    /// The file name.
    pub file: String,
    /// What is wrong with it.
    pub reason: String,
}

/// The themes on offer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Themes {
    /// Built-in themes in the gallery's order, then the user's by id.
    pub themes: Vec<ThemeEntry>,
    /// User files that were skipped.
    pub skipped: Vec<Skipped>,
}

/// A theme with the user's choices applied, ready to draw.
#[derive(Debug, Clone)]
pub struct Resolved {
    /// The theme.
    pub theme: Theme,
    /// The choices.
    pub values: Values,
    /// The speed factor of the spec (1.0 as designed).
    pub speed: f64,
    /// The brightness percent of the spec, if it names one.
    pub brightness: Option<i64>,
}

impl Resolved {
    /// The effect that draws this theme on the keyboard; `None` for an effect the engine lacks,
    /// which [`ThemeStore::resolve`] has already ruled out.
    pub fn effect(&self) -> Option<Box<dyn Effect>> {
        self.theme.keyboard_effect(&self.values)
    }
}

/// The theme that is running: what was asked for and when it started (seconds since the epoch;
/// the theme's clock runs from here, and a new value is how the key service notices a re-apply).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Active {
    /// The theme and the user's choices.
    pub spec: ThemeSpec,
    /// When it was started.
    pub started: f64,
}

/// A store of user themes in a directory, over the user state for the running theme.
pub struct ThemeStore {
    dir: Option<PathBuf>,
    state: Arc<Store>,
}

/// Whether the keyboard look of `theme` uses a firmware effect nobody has measured the flashing of.
fn flash_unmeasured(theme: &Theme) -> bool {
    use ts_fx::themes::Look;
    match &theme.keyboard {
        Look::Firmware { hw, .. } => {
            ts_fx::find(&format!("kb.{hw}")).is_some_and(|d| d.flash == Flash::Unmeasured)
        }
        Look::Software { effect, .. } => ts_fx::find(effect).is_some_and(|d| d.flash == Flash::Unmeasured),
    }
}

fn file_error(op: &'static str) -> impl FnOnce(FsError) -> ThemesError {
    move |source| ThemesError::File { op, source }
}

impl ThemeStore {
    /// A store over `dir` (user themes are `dir/ID.json`) and the user state `state`.
    pub fn new(dir: impl Into<PathBuf>, state: Arc<Store>) -> ThemeStore {
        ThemeStore { dir: Some(dir.into()), state }
    }

    /// The store whose themes directory is `themes` beside the user's configuration file
    /// (`~/.config/truesense/themes`); without a configuration path (no `HOME`) there is no place
    /// for user themes, but the built-in ones and the running theme still work.
    pub fn beside(config_file: Option<&Path>, state: Arc<Store>) -> ThemeStore {
        let dir = config_file.and_then(Path::parent).map(|p| p.join("themes"));
        ThemeStore { dir, state }
    }

    fn dir(&self) -> Result<&Path, ThemesError> {
        self.dir.as_deref().ok_or(ThemesError::NoLocation)
    }

    fn path(&self, id: &str) -> Result<PathBuf, ThemesError> {
        if !is_valid_id(id) {
            return Err(invalid(format!(
                "theme '{id}': use lower-case letters, digits, - and _ (max {MAX_NAME})"
            )));
        }
        Ok(self.dir()?.join(format!("{id}.json")))
    }

    /// Reads one user file; a file that is not a usable theme is the `Err` with the reason.
    fn read_user(&self, id: &str) -> Result<Theme, ThemesError> {
        let path = self.path(id)?;
        let bytes = match fsx::read_limited(&path, MAX_THEME_FILE) {
            Ok(b) => b,
            Err(e) if e.is_not_found() => {
                return Err(invalid(format!("no theme '{id}' (truesense theme list)")));
            }
            Err(source) => return Err(ThemesError::File { op: "read theme", source }),
        };
        let theme: Theme = serde_json::from_slice(&bytes)
            .map_err(|source| ThemesError::Parse { what: format!("theme '{id}'"), source })?;
        if theme.id != id {
            return Err(invalid(format!("theme file '{id}.json' holds the theme '{}'", theme.id)));
        }
        let problems = theme.problems();
        if let Some(first) = problems.first() {
            return Err(invalid(format!("theme '{id}': {first}")));
        }
        Ok(theme)
    }

    /// The built-in themes, then the user's valid ones; a file that is not a theme is listed in
    /// `skipped` with the reason (the Health page shows these) and never stops the listing.
    pub fn list(&self) -> Themes {
        let mut themes: Vec<ThemeEntry> = builtin()
            .into_iter()
            .map(|t| ThemeEntry {
                id: t.id.clone(),
                name: t.name.clone(),
                mood: t.mood.clone(),
                builtin: true,
                top: t.top,
                runnable: !flash_unmeasured(t),
            })
            .collect();
        let mut skipped = Vec::new();
        let mut files: Vec<String> = match &self.dir {
            Some(dir) => std::fs::read_dir(dir)
                .map(|entries| {
                    entries
                        .filter_map(Result::ok)
                        .filter_map(|e| e.file_name().into_string().ok())
                        .filter(|f| f.ends_with(".json"))
                        .collect()
                })
                .unwrap_or_default(),
            None => Vec::new(),
        };
        files.sort();
        files.truncate(MAX_USER_THEMES);
        for file in files {
            let id = file.trim_end_matches(".json");
            if find(id).is_some() {
                skipped.push(Skipped { file, reason: "the name is a built-in theme's".to_owned() });
                continue;
            }
            match self.read_user(id) {
                Ok(t) => themes.push(ThemeEntry {
                    id: t.id.clone(),
                    name: t.name.clone(),
                    mood: t.mood.clone(),
                    builtin: false,
                    top: 0,
                    runnable: !flash_unmeasured(&t),
                }),
                Err(e) => skipped.push(Skipped { file, reason: e.to_string() }),
            }
        }
        Themes { themes, skipped }
    }

    /// A built-in or user theme by id.
    pub fn get(&self, id: &str) -> Result<Theme, ThemesError> {
        if let Some(t) = find(id) {
            return Ok(t.clone());
        }
        self.read_user(id)
    }

    /// Whether `id` is a built-in theme (read-only).
    pub fn is_builtin(id: &str) -> bool {
        find(id).is_some()
    }

    /// Stores one of the user's themes as `ID.json`. A built-in id is refused, and so is a theme
    /// that breaks a rule of the catalogue ([`Theme::problems`]), listing the first reason.
    pub fn save(&self, theme: &Theme) -> Result<(), ThemesError> {
        if Self::is_builtin(&theme.id) {
            return Err(invalid(format!(
                "'{}' is a built-in theme and read-only; save a copy under another name",
                theme.id
            )));
        }
        if let Some(first) = theme.problems().first() {
            return Err(invalid(format!("theme '{}': {first}", theme.id)));
        }
        let path = self.path(&theme.id)?;
        let bytes = serde_json::to_vec_pretty(theme)
            .map_err(|source| ThemesError::Parse { what: "theme".to_owned(), source })?;
        fsx::write_atomic_mkdir(&path, &bytes, 0o644).map_err(file_error("save theme"))
    }

    /// Reads a theme from JSON text of the catalogue's schema and stores it (what `theme save`
    /// and the bus do with a theme that came from outside). Answers the theme.
    pub fn save_json(&self, text: &str) -> Result<Theme, ThemesError> {
        let theme: Theme = serde_json::from_str(text)
            .map_err(|source| ThemesError::Parse { what: "theme".to_owned(), source })?;
        self.save(&theme)?;
        Ok(theme)
    }

    /// Removes one of the user's themes. A theme that is running stays running (it is already
    /// loaded); stop it first to be rid of it.
    pub fn delete(&self, id: &str) -> Result<(), ThemesError> {
        if Self::is_builtin(id) {
            return Err(invalid(format!("'{id}' is a built-in theme and cannot be deleted")));
        }
        let path = self.path(id)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(invalid(format!("no theme '{id}'"))),
            Err(source) => Err(ThemesError::File {
                op: "delete theme",
                source: FsError::Io { op: "remove", path, source },
            }),
        }
    }

    /// Stores a theme under a new id (`ID-copy` by default, numbered until it is free) and answers
    /// that id. Any theme can be copied, a built-in one included: that is how a user starts a
    /// theme of their own.
    pub fn copy(&self, id: &str, new_id: Option<&str>) -> Result<String, ThemesError> {
        let source = self.get(id)?;
        let base = new_id.filter(|n| !n.is_empty()).map_or_else(|| format!("{id}-copy"), str::to_owned);
        let taken: Vec<String> = self.list().themes.into_iter().map(|t| t.id).collect();
        let mut target = base.clone();
        let mut n = 2;
        while taken.contains(&target) || Self::is_builtin(&target) {
            target = format!("{base}-{n}");
            n += 1;
        }
        let mut theme = source;
        theme.id = target.clone();
        theme.top = 0;
        let name = format!("{} (copy)", theme.name);
        theme.name = name.chars().take(MAX_NAME).collect();
        self.save(&theme)?;
        Ok(target)
    }

    /// The theme `spec` names with its choices applied, checked, and refused when its keyboard
    /// look has no flash measurement. The one gate every way of starting a theme goes through.
    pub fn resolve(&self, spec: &ThemeSpec) -> Result<Resolved, ThemesError> {
        spec.check_shape()?;
        let theme = self.get(&spec.name)?;
        if flash_unmeasured(&theme) {
            return Err(invalid(format!(
                "theme '{}' is not available yet: its flashing has not been measured on this keyboard",
                spec.name
            )));
        }
        let values = spec.values(&theme)?;
        let resolved = Resolved { theme, values, speed: spec.speed_factor(), brightness: spec.brightness };
        if resolved.effect().is_none() {
            return Err(invalid(format!("theme '{}' names an effect this build does not have", spec.name)));
        }
        Ok(resolved)
    }

    // ---- the running theme (shared with the key service through the user state) -----------------

    /// The theme that is running, if any.
    pub fn active(&self) -> Option<Active> {
        self.state.get_as(ACTIVE_KEY)
    }

    /// Starts a theme as the current look: checks it ([`ThemeStore::resolve`]), forgets why an earlier one
    /// stopped, and records it with the time `now` for the key service to draw.
    pub fn start(&self, spec: &ThemeSpec, now: f64) -> Result<Active, ThemesError> {
        self.resolve(spec)?;
        let active = Active { spec: spec.clone(), started: now };
        self.state.set_as(ACTIVE_KEY, &active).map_err(file_error("start theme"))?;
        // Best effort: the note is a convenience.
        let _ = self.state.set(HALT_KEY, serde_json::Value::Null);
        Ok(active)
    }

    /// Stops the running theme (the key service puts the static look back). Answers whether one
    /// was running.
    pub fn stop(&self) -> Result<bool, ThemesError> {
        let was = self.active().is_some();
        if was {
            self.state.set(ACTIVE_KEY, serde_json::Value::Null).map_err(file_error("stop theme"))?;
        }
        Ok(was)
    }

    /// What the key service does when it cannot go on drawing: forgets the running theme and
    /// keeps the reason where `theme show` can say it.
    pub fn halt(&self, reason: &str) {
        let _ = self.state.set(ACTIVE_KEY, serde_json::Value::Null);
        let _ = self.state.set(HALT_KEY, serde_json::Value::String(reason.to_owned()));
    }

    /// Why the last theme stopped by itself, if it did and nothing started since.
    pub fn halt_reason(&self) -> Option<String> {
        self.state.get_as::<String>(HALT_KEY)
    }
}
