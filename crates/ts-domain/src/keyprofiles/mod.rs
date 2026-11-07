//! keyprofiles: saved keyboard lighting profiles (the PREDATOR-key and keyboard profiles of the
//! window and the command line).
//!
//! A profile is a whole-keyboard picture, 128 colours, stored as
//! `~/.config/truesense/lighting/NAME.json` in the format the Python and Go editions wrote:
//! `{"cells": [[r, g, b], ...]}`. Applying one uploads the colours through the keyboard at the
//! brightness the user has set (Fn+F11/F12) unless told otherwise.
#![forbid(unsafe_code)]

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use ts_foundation::fsx::{self, FsError};

use crate::keyboard::{Keyboard, KeyboardError};
use crate::keymap::{self, Color, KeymapError};

/// A failure of a profile operation.
#[derive(Debug)]
pub enum ProfileError {
    /// A name, action or request that is not usable; the text says why (the Python edition's text).
    Invalid(String),
    /// A profile file could not be read, written or removed.
    File {
        /// What was being done.
        op: &'static str,
        /// The underlying failure.
        source: FsError,
    },
    /// A profile file is not a profile.
    Parse {
        /// Why: the decoder's complaint or the first bad cell.
        reason: String,
    },
    /// The keyboard refused the upload.
    Keyboard(KeyboardError),
    /// A colour in a `save` request was not `RRGGBB`.
    Colour(KeymapError),
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::File { op, source } => write!(f, "{op}: {source}"),
            Self::Parse { reason } => write!(f, "parse profile: {reason}"),
            Self::Keyboard(e) => e.fmt(f),
            Self::Colour(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for ProfileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::File { source, .. } => Some(source),
            Self::Keyboard(e) => Some(e),
            Self::Colour(e) => Some(e),
            Self::Invalid(_) | Self::Parse { .. } => None,
        }
    }
}

fn invalid(text: impl Into<String>) -> ProfileError {
    ProfileError::Invalid(text.into())
}

/// What applying a profile needs from the keyboard.
pub trait PerKeyUpload: Send + Sync {
    /// Uploads the colours; `None` keeps the brightness the keyboard has now.
    fn per_key(&self, cells: &[Color], brightness: Option<i64>) -> Result<(), KeyboardError>;
}

impl PerKeyUpload for Keyboard {
    fn per_key(&self, cells: &[Color], brightness: Option<i64>) -> Result<(), KeyboardError> {
        Keyboard::per_key(self, cells, brightness)
    }
}

/// `~/.config/truesense/lighting` under `home`. (The Python edition did not honour
/// `XDG_CONFIG_HOME` here, unlike for designs; the files must stay where it put them.)
pub fn default_dir(home: &Path) -> PathBuf {
    home.join(".config/truesense/lighting")
}

/// Whether `name` is usable as a profile name: letters, digits, `-` and `_`, and at least one
/// letter or digit. It is never a path.
pub fn valid_name(name: &str) -> bool {
    let rest: Vec<char> = name.chars().filter(|c| !matches!(c, '-' | '_')).collect();
    !rest.is_empty() && rest.iter().all(|c| c.is_alphanumeric())
}

/// The profile file text for these colours, written the way Python's `json.dumps` writes it.
pub fn encode(cells: &[Color]) -> String {
    let cells: Vec<String> = cells.iter().map(|c| format!("[{}, {}, {}]", c[0], c[1], c[2])).collect();
    format!("{{\"cells\": [{}]}}", cells.join(", "))
}

#[derive(Deserialize)]
struct File {
    cells: Option<Vec<Vec<i64>>>,
}

/// Decodes a profile file. A profile with a cell that is not three numbers 0 to 255 is refused
/// whole rather than loaded with wrong colours; at most 128 cells are kept.
pub fn parse(text: &str) -> Result<Vec<Color>, ProfileError> {
    let file: File = serde_json::from_str(text).map_err(|e| ProfileError::Parse { reason: e.to_string() })?;
    let cells = file.cells.ok_or_else(|| ProfileError::Parse { reason: "no cells".to_owned() })?;
    let mut out = Vec::with_capacity(cells.len().min(keymap::NUM_CELLS));
    for (i, cell) in cells.iter().enumerate() {
        let [r, g, b] = cell[..] else {
            return Err(ProfileError::Parse { reason: format!("cell {i} is not r, g, b") });
        };
        let byte = |v: i64| {
            u8::try_from(v)
                .map_err(|_| ProfileError::Parse { reason: format!("cell {i}: {v} is not a colour value") })
        };
        out.push([byte(r)?, byte(g)?, byte(b)?]);
    }
    out.truncate(keymap::NUM_CELLS);
    Ok(out)
}

/// The answer to [`ProfileStore::command`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutput {
    /// The profile names (`list`).
    Names(Vec<String>),
    /// A line for the user (`load`, `save`, `delete`).
    Message(String),
}

/// A directory of profiles, and the keyboard they are applied to.
pub struct ProfileStore {
    dir: PathBuf,
    keyboard: Arc<dyn PerKeyUpload>,
}

impl ProfileStore {
    /// A store over `dir` (see [`default_dir`]) that applies profiles through `keyboard`.
    pub fn new(dir: impl Into<PathBuf>, keyboard: Arc<dyn PerKeyUpload>) -> Self {
        Self { dir: dir.into(), keyboard }
    }

    fn path(&self, name: &str) -> Result<PathBuf, ProfileError> {
        if !valid_name(name) {
            return Err(invalid("profile names: letters, digits, - and _"));
        }
        Ok(self.dir.join(format!("{name}.json")))
    }

    /// Stores a profile.
    pub fn save(&self, name: &str, cells: &[Color]) -> Result<(), ProfileError> {
        let path = self.path(name)?;
        fsx::write_atomic_mkdir(&path, encode(cells).as_bytes(), 0o644)
            .map_err(|source| ProfileError::File { op: "save lighting profile", source })
    }

    /// Whether a profile of that name is stored.
    pub fn exists(&self, name: &str) -> bool {
        self.path(name).is_ok_and(|p| fsx::exists(&p))
    }

    /// The stored profiles, sorted. Only `*.json` files count. (The Python edition listed every
    /// file in the directory with its last five characters cut off.)
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|e| e.file_type().is_ok_and(|t| !t.is_dir()))
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter_map(|f| f.strip_suffix(".json").map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    /// Reads a profile.
    pub fn load(&self, name: &str) -> Result<Vec<Color>, ProfileError> {
        let path = self.path(name)?;
        let bytes = match fsx::read_limited(&path, fsx::MAX_SETTINGS_FILE) {
            Ok(b) => b,
            Err(e) if e.is_not_found() => {
                return Err(invalid(format!("no profile '{name}' (truesense keyboard profile list)")));
            }
            Err(source) => return Err(ProfileError::File { op: "read lighting profile", source }),
        };
        let text = String::from_utf8(bytes).map_err(|e| ProfileError::Parse { reason: e.to_string() })?;
        parse(&text).map_err(|e| invalid(format!("lighting profile '{name}': {e}")))
    }

    /// Puts a profile on the keyboard; `None` keeps the brightness the keyboard has now.
    pub fn apply(&self, name: &str, brightness: Option<i64>) -> Result<(), ProfileError> {
        let cells = self.load(name)?;
        self.keyboard.per_key(&cells, brightness).map_err(ProfileError::Keyboard)
    }

    /// Removes a profile.
    pub fn delete(&self, name: &str) -> Result<(), ProfileError> {
        let path = self.path(name)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Err(invalid(format!("no lighting profile '{name}'")))
            }
            Err(source) => Err(ProfileError::File {
                op: "delete lighting profile",
                source: FsError::Io { op: "remove", path, source },
            }),
        }
    }

    /// `truesense keyboard profile list|load|save|delete NAME [save: fill COLOR | gradient C1 C2 |
    /// rainbow]`. `list` answers the names, the others a message. `save` without a spec fills
    /// the keyboard with the stock cyan.
    pub fn command(
        &self,
        action: &str,
        name: Option<&str>,
        spec: &[&str],
    ) -> Result<CommandOutput, ProfileError> {
        if action == "list" {
            return Ok(CommandOutput::Names(self.names()));
        }
        let Some(name) = name.filter(|n| !n.is_empty()) else {
            return Err(invalid(format!("profile {action} needs a name")));
        };
        match action {
            "load" => {
                self.apply(name, None)?;
                Ok(CommandOutput::Message(format!("loaded lighting profile '{name}'")))
            }
            "delete" => {
                self.delete(name)?;
                Ok(CommandOutput::Message(format!("deleted lighting profile '{name}'")))
            }
            "save" => {
                self.save(name, &preset(spec)?)?;
                Ok(CommandOutput::Message(format!("saved lighting profile '{name}'")))
            }
            _ => Err(invalid("profile list | load NAME | save NAME ... | delete NAME")),
        }
    }
}

/// `fill COLOR`, `gradient C1 C2` or `rainbow` as a picture (default: the stock cyan fill).
fn preset(spec: &[&str]) -> Result<Vec<Color>, ProfileError> {
    let colour = |c: &str| keymap::parse_hex(c).map_err(ProfileError::Colour);
    match spec {
        [] => Ok(keymap::canvas([0x00, 0xAE, 0xC7])),
        ["rainbow", ..] => Ok(keymap::rainbow()),
        ["gradient", c1, c2] => Ok(keymap::gradient(colour(c1)?, colour(c2)?)),
        ["fill", c] => Ok(keymap::canvas(colour(c)?)),
        _ => Err(invalid("save NAME fill COLOR | gradient C1 C2 | rainbow")),
    }
}

#[cfg(test)]
mod tests;
