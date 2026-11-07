//! What this tool last put on the keyboard, in the form the earlier editions stored it.
#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

use crate::keymap::{self, Color, KeymapError};

/// Look modes stored in [`State::mode`].
pub mod mode {
    /// The backlight effect "off" was sent.
    pub const OFF: &str = "off";
    /// A hardware effect.
    pub const EFFECT: &str = "effect";
    /// The colour follows the power mode.
    pub const OPERATING: &str = "operating";
    /// A per-key picture.
    pub const PERKEY: &str = "perkey";
}

/// The look this tool last put on the keyboard. The hardware cannot be asked which effect is
/// running, so every look is remembered (in the user state under `keyboard`, and inside saved
/// looks) and brightness, speed and colour changes are re-sent on top of it.
///
/// The JSON field names are those the Python and Go editions wrote, and a field that is absent
/// stays absent when written again: "no speed stored" (the default of 5 applies) is not the same
/// as a speed of 0, and a brightness of 0 is a real, dark setting. Fields this edition does not
/// know are not kept.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// One of [`mode`]; empty when nothing is remembered.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mode: String,
    /// The effect name (`effect` mode).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// The colour as lower-case `rrggbb`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub color: String,
    /// The animation speed, 1 (slowest) to 12 (fastest).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<i64>,
    /// The brightness, 0 to 50 (0 to 255 in the operating mode, where the controller takes 0x32).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness: Option<i64>,
    /// The effect direction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<i64>,
    /// The per-key colours as `rrggbb`, normally 128 of them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cells: Vec<String>,
}

impl State {
    /// Whether nothing is remembered (an empty object was falsy in the Python edition).
    pub fn is_empty(&self) -> bool {
        *self == State::default()
    }

    /// The stored speed, or `default` when none was stored.
    pub fn speed_or(&self, default: i64) -> i64 {
        self.speed.unwrap_or(default)
    }

    /// The stored brightness, or `default` when none was stored.
    pub fn brightness_or(&self, default: i64) -> i64 {
        self.brightness.unwrap_or(default)
    }

    /// The stored direction, or `default` when none was stored.
    pub fn direction_or(&self, default: i64) -> i64 {
        self.direction.unwrap_or(default)
    }

    /// The stored colour, or `default` when none was stored.
    pub fn color_or<'a>(&'a self, default: &'a str) -> &'a str {
        if self.color.is_empty() { default } else { &self.color }
    }

    /// The remembered per-key colours; a malformed entry makes the whole list an error rather
    /// than a silently wrong picture.
    pub fn parse_cells(&self) -> Result<Vec<Color>, KeymapError> {
        self.cells.iter().map(|c| keymap::parse_hex(c)).collect()
    }
}

/// The first 128 colours in the stored form, padded with black.
pub(super) fn cells_hex(cells: &[Color]) -> Vec<String> {
    (0..keymap::NUM_CELLS).map(|i| keymap::hex(cells.get(i).copied().unwrap_or([0; 3]))).collect()
}

/// The colour as the state stores it: no leading `#`, lower case.
pub(super) fn normalise_color(color: &str) -> String {
    color.trim_start_matches('#').to_lowercase()
}
