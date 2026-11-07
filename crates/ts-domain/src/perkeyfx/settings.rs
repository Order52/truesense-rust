//! The settings of the animation that is switched on, as clients and the key service share them.
//!
//! They live in the user state (`perkey_fx`: mode, speed, keys, base colours, start time): the
//! command line, the TUI and the window write them, and the runner inside the key service reads
//! them twice a second. The JSON field names are those the Python and Go editions wrote.
#![forbid(unsafe_code)]

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_foundation::userstate::{self, Store};

use super::frames::{MODES, SPEED_MAX, SPEED_MIN, is_mode};
use super::{FrameSink, FxError};
use crate::keymap::{self, Color, NUM_CELLS};

/// The speed an animation gets when none is stored.
pub const DEFAULT_SPEED: i64 = 5;

/// The animation switched on, or `off`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Settings {
    /// `off` or one of the animation names.
    pub mode: String,
    /// 1 (slowest) to 10 (fastest).
    pub speed: i64,
    /// The cells it applies to; `None` is every lit key.
    pub keys: Option<Vec<i64>>,
    /// The colour of each cell without the animation (`rrggbb`), what the keys go back to.
    pub base: Vec<String>,
    /// When it was started, seconds since the epoch: the animation's clock runs from here.
    /// `None` runs the clock at 0.
    pub t0: Option<f64>,
}

impl Settings {
    /// No animation.
    pub fn off() -> Settings {
        Settings { mode: "off".to_owned(), speed: DEFAULT_SPEED, keys: None, base: Vec::new(), t0: None }
    }

    /// The base colours; `None` when one entry is not `rrggbb`, which makes the whole list unusable.
    pub fn base_colors(&self) -> Option<Vec<Color>> {
        self.base.iter().map(|h| keymap::parse_hex(h).ok()).collect()
    }

    /// The chosen cells that exist on the keyboard (an empty list means every lit key).
    pub fn cells(&self) -> Vec<usize> {
        self.keys
            .iter()
            .flatten()
            .filter_map(|k| usize::try_from(*k).ok())
            .filter(|k| *k < NUM_CELLS)
            .collect()
    }
}

/// How the state holds the settings: every field optional and loosely typed, because the file is
/// written by other editions and edited by hand. A field of the wrong type is read as absent.
#[derive(Deserialize)]
struct Stored {
    mode: Option<Value>,
    speed: Option<Value>,
    keys: Option<Value>,
    base: Option<Value>,
    t0: Option<Value>,
}

fn from_stored(stored: Stored) -> Settings {
    let mode = match stored.mode {
        Some(Value::String(m)) if !m.is_empty() => m,
        _ => return Settings::off(),
    };
    let speed = stored.speed.and_then(|v| v.as_i64()).unwrap_or(DEFAULT_SPEED);
    let keys = stored.keys.and_then(|v| v.as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()));
    let base = match stored.base {
        Some(Value::Array(a)) => a.into_iter().filter_map(|v| v.as_str().map(str::to_owned)).collect(),
        _ => Vec::new(),
    };
    let t0 = stored.t0.and_then(|v| v.as_f64()).filter(|t| t.is_finite());
    Settings { mode, speed, keys, base, t0 }
}

/// The seconds since the epoch, for [`FxStore`]'s clock.
pub fn epoch_seconds() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

/// Reads and writes the settings in the user state.
pub struct FxStore {
    state: Arc<Store>,
    canvas: Arc<dyn FrameSink>,
    now: Arc<dyn Fn() -> f64 + Send + Sync>,
}

impl FxStore {
    /// A store over `state`. `canvas` gives the colours now on the keyboard (the default base of an
    /// animation) and `now` the wall clock in seconds since the epoch ([`epoch_seconds`]).
    pub fn new(
        state: Arc<Store>,
        canvas: Arc<dyn FrameSink>,
        now: Arc<dyn Fn() -> f64 + Send + Sync>,
    ) -> Self {
        Self { state, canvas, now }
    }

    /// The current settings; `off` if none, or if the stored ones are not settings.
    pub fn get(&self) -> Settings {
        self.state.get_as::<Stored>(userstate::keys::PERKEY_FX).map_or_else(Settings::off, from_stored)
    }

    /// Switches an animation on, or `off`. `speed` is 1 to 10; `keys` are the cells it applies to
    /// (none: every lit key); `base` is the colour of each cell without the animation (default,
    /// and for an empty list: the colours now on the keyboard). Returns the settings as stored.
    pub fn set(&self, mode: &str, speed: i64, keys: &[i64], base: &[Color]) -> Result<Settings, FxError> {
        if mode != "off" && !is_mode(mode) {
            let names: Vec<&str> = MODES.iter().map(|(n, _)| *n).collect();
            return Err(FxError::Invalid(format!("animation: off, {}", names.join(", "))));
        }
        if !(SPEED_MIN..=SPEED_MAX).contains(&speed) {
            return Err(FxError::Invalid(format!("speed {SPEED_MIN}..{SPEED_MAX}")));
        }
        if let Some(bad) = keys.iter().find(|k| !(0..NUM_CELLS as i64).contains(*k)) {
            return Err(FxError::Invalid(format!("cell {bad}: 0..{}", NUM_CELLS - 1)));
        }
        let base: Vec<String> = if base.is_empty() {
            self.canvas.current_cells().iter().map(|c| keymap::hex(*c)).collect()
        } else {
            base.iter().map(|c| keymap::hex(*c)).collect()
        };
        let mut chosen = keys.to_vec();
        chosen.sort_unstable();
        chosen.dedup();
        let stored = Settings {
            mode: mode.to_owned(),
            speed,
            keys: (!chosen.is_empty()).then_some(chosen),
            base,
            t0: Some((self.now)()),
        };
        self.state.set_as(userstate::keys::PERKEY_FX, &stored).map_err(FxError::Save)?;
        Ok(self.get())
    }
}
