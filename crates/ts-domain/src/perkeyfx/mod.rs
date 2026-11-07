//! perkeyfx: per-key animations (software).
//!
//! The keyboard's hardware effects cover the whole keyboard; a different animation on chosen keys
//! has to be drawn frame by frame. [`render`] makes one frame (a pure function of time, so the
//! frames are deterministic and testable), [`FxStore`] holds the settings in the user state, and
//! [`Runner`] draws about five frames a second inside the key service while an animation is on,
//! then puts the base colours back when it is switched off.
#![forbid(unsafe_code)]

mod frames;
mod runner;
mod settings;

use std::fmt;

use ts_foundation::fsx::FsError;

pub use frames::{MODES, SPEED_MAX, SPEED_MIN, hash, is_mode, mode_names, render};
pub use runner::{BRIGHTNESS_REFRESH_S, ERROR_BACKOFF, FRAME_INTERVAL, IDLE_INTERVAL, Runner};
pub use settings::{DEFAULT_SPEED, FxStore, Settings, epoch_seconds};

use crate::keyboard::{Keyboard, KeyboardError};
use crate::keymap::Color;

/// A failure of the animation settings.
#[derive(Debug)]
pub enum FxError {
    /// A mode, speed or cell that is not usable; the text says what is allowed (the Python edition's text).
    Invalid(String),
    /// The settings could not be stored.
    Save(FsError),
}

impl fmt::Display for FxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::Save(e) => write!(f, "save animation settings: {e}"),
        }
    }
}

impl std::error::Error for FxError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Save(e) => Some(e),
            Self::Invalid(_) => None,
        }
    }
}

/// The keyboard as the animation uses it.
pub trait FrameSink: Send + Sync {
    /// The keyboard's brightness as it reports it.
    fn brightness(&self) -> Result<u8, KeyboardError>;
    /// Uploads a picture without remembering it as the look, so the look the user chose survives
    /// the animation. `None` keeps the brightness the keyboard has now.
    fn per_key_frame(&self, cells: &[Color], brightness: Option<i64>) -> Result<(), KeyboardError>;
    /// The colours this tool last uploaded (128 cells).
    fn current_cells(&self) -> Vec<Color>;
}

impl FrameSink for Keyboard {
    fn brightness(&self) -> Result<u8, KeyboardError> {
        self.get_brightness()
    }

    fn per_key_frame(&self, cells: &[Color], brightness: Option<i64>) -> Result<(), KeyboardError> {
        Keyboard::per_key_frame(self, cells, brightness)
    }

    fn current_cells(&self) -> Vec<Color> {
        Keyboard::current_cells(self)
    }
}

#[cfg(test)]
mod tests;
