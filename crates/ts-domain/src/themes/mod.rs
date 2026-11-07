//! themes: lighting themes for the keyboard, drawn frame by frame by the key service.
//!
//! The keyboard's own effects cover the whole keyboard with one pattern. A theme is a designed
//! look (a colour palette, an effect and its settings) that the `ts-fx` engine draws, 15 or so
//! frames a second at most, on every key. This module is everything about themes that is not
//! drawing and not hardware:
//!
//! * [`ThemeSpec`]: the part of a [`looks::Look`](crate::looks::Look) that names a theme with the
//!   user's choices (colourway, custom colours, settings, speed, brightness). Older programs
//!   ignore it, because a look's JSON may carry members they do not know.
//! * [`ThemeStore`]: the 32 built-in themes (read-only), the user's own in
//!   `~/.config/truesense/themes/NAME.json` (the catalogue's own schema, validated strictly, a
//!   broken file skipped and reported), and the one theme that is running now, kept in the user
//!   state where the key service reads it.
//! * [`KEY_NAMES`]: how the engine's key names map to this laptop's cells (see `cells.rs`).
//!
//! Pure with respect to hardware: nothing here touches a device, a clock or the real home
//! directory; the directory, the user state and the time are passed in.
#![forbid(unsafe_code)]

mod cells;
mod preview;
mod spec;
mod store;

use std::fmt;

use ts_foundation::fsx::FsError;

pub use cells::KEY_NAMES;
pub use preview::{DEFAULT_T as PREVIEW_T, MAX_T as PREVIEW_MAX_T, STEP as PREVIEW_STEP, preview};
pub use spec::{
    BRIGHTNESS_MAX, BRIGHTNESS_MIN, ColourwayPick, DEFAULT_SPEED, SPEED_MAX, SPEED_MIN, ThemeSpec,
};
pub use store::{
    ACTIVE_KEY, Active, ColourwayInfo, HALT_KEY, MAX_THEME_FILE, Resolved, Skipped, ThemeEntry, ThemeStore,
    Themes,
};

/// Why a theme operation failed.
#[derive(Debug)]
pub enum ThemesError {
    /// A theme name, choice or file that is not usable; the text says why.
    Invalid(String),
    /// There is no place to keep user themes (no home directory).
    NoLocation,
    /// A theme file could not be read, written or removed.
    File {
        /// What was being done.
        op: &'static str,
        /// The underlying failure.
        source: FsError,
    },
    /// Something that should be a theme is not JSON of the theme schema.
    Parse {
        /// What the text was (a file or `theme`).
        what: String,
        /// The decoder's complaint.
        source: serde_json::Error,
    },
}

impl fmt::Display for ThemesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::NoLocation => f.write_str("there is no place for your themes (HOME is not set)"),
            Self::File { op, source } => write!(f, "{op}: {source}"),
            Self::Parse { what, source } => write!(f, "{what}: {source}"),
        }
    }
}

impl std::error::Error for ThemesError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::File { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            Self::Invalid(_) | Self::NoLocation => None,
        }
    }
}

pub(crate) fn invalid(text: impl Into<String>) -> ThemesError {
    ThemesError::Invalid(text.into())
}

#[cfg(test)]
mod cells_tests;
#[cfg(test)]
mod preview_tests;
#[cfg(test)]
mod tests;
