//! Lighting themes: the built-in catalogue and the user's own, and the one that runs on the
//! keyboard.
//!
//! A theme is drawn frame by frame by the key service (its renderer loop reads the running-theme
//! record in the user state); this module only chooses, validates and records.
//! Starting a theme is the same as applying a look with a theme part: the same record, the same
//! checks, and the same rule that anything else written to the keyboard ends it.
//!
//! User themes are files (`~/.config/truesense/themes/NAME.json`) in the catalogue's own schema, so
//! the interfaces list, show, copy and delete them the way they do looks.
#![forbid(unsafe_code)]

use serde::Serialize;
use serde_json::Value;
use ts_domain::themes::{Active, Skipped, ThemeEntry, ThemeSpec, ThemeStore, ThemesError};

use super::{Api, ApiError, ApiResult};

impl From<ThemesError> for ApiError {
    fn from(e: ThemesError) -> Self {
        match e {
            ThemesError::Invalid(text) => ApiError::invalid(text),
            ThemesError::NoLocation => ApiError::invalid(ThemesError::NoLocation.to_string()),
            other => ApiError::domain("theme", other),
        }
    }
}

/// Which theme runs and whether the key service is drawing it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ThemeStatus {
    /// What was asked for; `None` when no theme is the current look.
    pub active: Option<Active>,
    /// Why the key service is not drawing it (it stopped by itself: the keyboard refused a frame,
    /// or another command set the keyboard); `None` when it is drawing, or nothing runs.
    pub halted: Option<String>,
}

impl ThemeStatus {
    /// One line for a terminal.
    pub fn describe(&self) -> String {
        match (&self.active, &self.halted) {
            (None, _) => "no theme is running".to_owned(),
            (Some(a), None) => format!("theme {} is running", a.spec.name),
            (Some(a), Some(why)) => format!("theme {} stopped: {why}", a.spec.name),
        }
    }
}

/// The themes on offer and the one that runs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ThemeList {
    /// Built-in themes in the gallery order, then the user's.
    pub themes: Vec<ThemeEntry>,
    /// User files that are not themes, with the reason (`Health` shows them).
    pub skipped: Vec<Skipped>,
    /// The theme that runs.
    pub status: ThemeStatus,
}

/// One theme in full.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ThemeDetail {
    /// What the list says about it.
    #[serde(flatten)]
    pub entry: ThemeEntry,
    /// The theme as the catalogue's schema writes it: colourways, Custom labels, looks, settings.
    pub theme: Value,
}

impl Api {
    /// The theme store: the user's directory beside the configuration file, over the user state.
    pub(super) fn theme_store(&self) -> ThemeStore {
        ThemeStore::beside(self.s.config.user.as_deref(), self.s.user.clone())
    }

    /// The running theme and whether it is being drawn.
    pub fn theme_status(&self) -> ThemeStatus {
        let store = self.theme_store();
        ThemeStatus { active: store.active(), halted: store.halt_reason() }
    }

    /// The built-in themes, the user's, and what runs.
    pub fn themes(&self) -> ThemeList {
        let store = self.theme_store();
        let listed = store.list();
        ThemeList {
            themes: listed.themes,
            skipped: listed.skipped,
            status: ThemeStatus { active: store.active(), halted: store.halt_reason() },
        }
    }

    /// One theme in full: a built-in or one of the user's.
    pub fn theme(&self, name: &str) -> ApiResult<ThemeDetail> {
        let store = self.theme_store();
        let theme = store.get(name)?;
        let entry = store
            .list()
            .themes
            .into_iter()
            .find(|t| t.id == name)
            .ok_or_else(|| ApiError::invalid(format!("no theme '{name}' (truesense theme list)")))?;
        Ok(ThemeDetail { entry, theme: serde_json::to_value(&theme)? })
    }

    /// Starts a theme as the current look, with the user's choices. The theme is checked first (a
    /// theme that is unknown, has a setting it does not have, or was never flash-measured is
    /// refused and nothing changes); a per-key animation that was running is switched off, because
    /// two loops cannot draw on one keyboard.
    pub fn apply_theme(&self, spec: &ThemeSpec) -> ApiResult<ThemeStatus> {
        let store = self.theme_store();
        store.resolve(spec)?;
        if self.s.fx.get().mode != "off" {
            self.set_keyboard_fx_cells("off", 0, &[], None)?;
        }
        store.start(spec, (self.s.now)())?;
        Ok(ThemeStatus { active: store.active(), halted: None })
    }

    /// Ends the running theme: the key service puts the static look back. Answers whether one ran.
    pub fn stop_theme(&self) -> ApiResult<bool> {
        Ok(self.theme_store().stop()?)
    }

    /// Stores a theme of the user's from JSON of the catalogue's schema.
    pub fn save_theme(&self, json: &str) -> ApiResult<ThemeEntry> {
        let store = self.theme_store();
        let theme = store.save_json(json)?;
        store.list().themes.into_iter().find(|t| t.id == theme.id).ok_or_else(|| {
            ApiError::invalid(format!("theme '{}' was saved but cannot be read back", theme.id))
        })
    }

    /// Removes one of the user's themes (a built-in one is read-only).
    pub fn delete_theme(&self, name: &str) -> ApiResult<()> {
        Ok(self.theme_store().delete(name)?)
    }

    /// Copies any theme to a new one of the user's (`NAME-copy` by default, numbered until free)
    /// and answers the new id.
    pub fn copy_theme(&self, name: &str, new_name: Option<&str>) -> ApiResult<String> {
        Ok(self.theme_store().copy(name, new_name)?)
    }

    /// Whether a theme is read-only.
    pub fn theme_is_builtin(name: &str) -> bool {
        ThemeStore::is_builtin(name)
    }
}
