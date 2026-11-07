//! designs: key designs (a still picture on the keyboard) and image painting.
//!
//! A design is plain data: `{"base": "rrggbb", "keys": {"w": "ff6600", "arrows": "00aaff"},
//! "cells": {"12": "ffffff"}}`. Keys and groups use the names of `keyboard list-keys`. Designs live
//! in `~/.config/truesense/designs/NAME.json` (yours win over the built-in ones of the same
//! name). No code is loaded, so a design file cannot do anything but colour keys, and a broken
//! file is skipped, never fatal.
//!
//! The order of the entries of `keys` is the order they are painted in: when two names cover the
//! same key (a group and one of its keys) the later one wins, exactly as the order of a Python
//! dict decided it, so the order of the file is kept.
#![forbid(unsafe_code)]

mod image;
mod templates;

use std::fmt;
use std::path::{Path, PathBuf};

use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use ts_foundation::fsx::{self, FsError};

pub use image::{ImageDecoder, MAX_PIXELS, Raster, image_cells, image_cells_from_raster};

use crate::keymap::{self, Color, KeyMap, KeymapError, LIT_CELLS, NUM_CELLS};

/// A failure of a design operation.
#[derive(Debug)]
pub enum DesignError {
    /// A design or name that is not usable; the text says why (the Python edition's text).
    Invalid(String),
    /// A key, group or colour in a design is not usable.
    Keymap(KeymapError),
    /// A design file could not be written.
    File {
        /// What was being done.
        op: &'static str,
        /// The underlying failure.
        source: FsError,
    },
    /// A design is not valid JSON of the right shape.
    Parse(serde_json::Error),
    /// A picture could not be read or is not usable; the text follows "cannot read the image: ".
    Image(String),
}

impl fmt::Display for DesignError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::Keymap(e) => e.fmt(f),
            Self::File { op, source } => write!(f, "{op}: {source}"),
            Self::Parse(e) => write!(f, "parse design: {e}"),
            Self::Image(why) => write!(f, "cannot read the image: {why}"),
        }
    }
}

impl std::error::Error for DesignError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Keymap(e) => Some(e),
            Self::File { source, .. } => Some(source),
            Self::Parse(e) => Some(e),
            Self::Invalid(_) | Self::Image(_) => None,
        }
    }
}

impl From<KeymapError> for DesignError {
    fn from(e: KeymapError) -> Self {
        Self::Keymap(e)
    }
}

fn invalid(text: impl Into<String>) -> DesignError {
    DesignError::Invalid(text.into())
}

/// An ordered JSON object of name to `RRGGBB`. A name that repeats keeps its first position with
/// the last value (the way a Python dict loaded the same file); `null` is an empty object.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Colors(pub Vec<(String, String)>);

impl Serialize for Colors {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (k, v) in &self.0 {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Colors {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Ordered;

        impl<'de> Visitor<'de> for Ordered {
            type Value = Colors;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an object of colours")
            }

            fn visit_unit<E>(self) -> Result<Colors, E> {
                Ok(Colors::default())
            }

            fn visit_none<E>(self) -> Result<Colors, E> {
                Ok(Colors::default())
            }

            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Colors, A::Error> {
                let mut out: Vec<(String, String)> = Vec::new();
                while let Some((key, value)) = access.next_entry::<String, String>()? {
                    match out.iter_mut().find(|(k, _)| *k == key) {
                        Some(entry) => entry.1 = value,
                        None => out.push((key, value)),
                    }
                }
                Ok(Colors(out))
            }
        }

        deserializer.deserialize_any(Ordered)
    }
}

impl Colors {
    fn of(pairs: &[(&str, &str)]) -> Colors {
        Colors(pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect())
    }
}

/// Where a design came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// One of the designs that ship with TrueSense.
    BuiltIn,
    /// A file in the user's design folder.
    Yours,
}

impl Source {
    /// `built-in` or `yours`, as shown to the user and written in the JSON.
    pub fn name(self) -> &'static str {
        match self {
            Self::BuiltIn => "built-in",
            Self::Yours => "yours",
        }
    }
}

impl Serialize for Source {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

/// A key design.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Design {
    /// A line of description (at most 80 characters).
    pub info: String,
    /// The colour of every lit key before the keys and cells below are painted, as it was written
    /// (`RRGGBB`, with or without leading `#`).
    pub base: String,
    /// Key and group names with colours, painted in this order.
    pub keys: Colors,
    /// Single cells (`"0"` to `"127"`) with colours, painted last.
    pub cells: Colors,
    /// Whether it is a built-in design or your file.
    pub source: Source,
}

/// What `str()` of a JSON value looked like in Python, for the free-text `info` of a design file.
fn py_str(v: &Value) -> String {
    match v {
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// A field that can be told apart three ways: absent (`None`), `null` (`Some(Value::Null)`) or a
/// value. Python's `d.get("info", "")` made that difference (`str(None)` is "None").
fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
struct Raw {
    #[serde(default, deserialize_with = "present")]
    info: Option<Value>,
    #[serde(default, deserialize_with = "present")]
    base: Option<Value>,
    #[serde(default)]
    keys: Option<Colors>,
    #[serde(default)]
    cells: Option<Colors>,
}

impl Design {
    /// Checks and cleans a design, as the Python edition did: a bad colour, an unknown key or
    /// group name, or a cell outside 0..127 makes the whole design invalid.
    fn checked(
        map: &KeyMap,
        info: &str,
        base: &str,
        keys: Colors,
        cells: Colors,
        source: Source,
    ) -> Result<Design, DesignError> {
        keymap::parse_hex(base)?;
        for (name, colour) in &keys.0 {
            keymap::parse_hex(colour)?;
            map.cells_for(name)?;
        }
        for (cell, colour) in &cells.0 {
            keymap::parse_hex(colour)?;
            cell_index(cell)?;
        }
        Ok(Design { info: info.chars().take(80).collect(), base: base.to_owned(), keys, cells, source })
    }

    /// Reads a design from JSON text. `source` says where the text came from.
    pub fn parse(map: &KeyMap, text: &str, source: Source) -> Result<Design, DesignError> {
        if !text.trim_start().starts_with('{') {
            // Valid JSON of another shape is "not an object"; invalid JSON is reported as such.
            return Err(match serde_json::from_str::<Value>(text) {
                Ok(_) => invalid("a design is a JSON object"),
                Err(e) => DesignError::Parse(e),
            });
        }
        let raw: Raw = serde_json::from_str(text).map_err(DesignError::Parse)?;
        let info = raw.info.as_ref().map_or_else(String::new, py_str);
        let base = match raw.base {
            None => "000000".to_owned(),
            Some(Value::String(s)) => s,
            Some(_) => return Err(invalid("a design's base colour is RRGGBB")),
        };
        Design::checked(
            map,
            &info,
            &base,
            raw.keys.unwrap_or_default(),
            raw.cells.unwrap_or_default(),
            source,
        )
    }

    /// The 128 cell colours of the design: the base, then the keys and groups in order, then the
    /// single cells. The last two cells stay black unless a cell entry paints them.
    pub fn cells_of(&self, map: &KeyMap) -> Result<Vec<Color>, DesignError> {
        let mut cv = vec![[0u8; 3]; NUM_CELLS];
        cv[..LIT_CELLS].fill(keymap::parse_hex(&self.base)?);
        for (name, colour) in &self.keys.0 {
            let rgb = keymap::parse_hex(colour)?;
            for c in map.cells_for(name)? {
                cv[c] = rgb;
            }
        }
        for (cell, colour) in &self.cells.0 {
            cv[cell_index(cell)?] = keymap::parse_hex(colour)?;
        }
        Ok(cv)
    }
}

/// A cell number `0` to `127`, written in ASCII digits (Python accepted `"03"` too).
fn cell_index(cell: &str) -> Result<usize, DesignError> {
    let bad = || invalid(format!("cell '{cell}': 0..{}", NUM_CELLS - 1));
    if cell.is_empty() || !cell.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad());
    }
    cell.parse::<usize>().ok().filter(|c| *c < NUM_CELLS).ok_or_else(bad)
}

/// The designs of the earlier editions that ship with TrueSense (the templates follow them), in display order: name, description, base colour,
/// keys and groups in painting order.
type BuiltinRow = (&'static str, &'static str, &'static str, &'static [(&'static str, &'static str)]);

const BUILTIN: [BuiltinRow; 6] = [
    (
        "gaming",
        "WASD and the keys around it lit for play",
        "101018",
        &[
            ("wasd", "ff6a00"),
            ("lshift", "ff3300"),
            ("space", "ff3300"),
            ("ctrl", "ff3300"),
            ("q", "00aaff"),
            ("e", "00aaff"),
            ("r", "00aaff"),
            ("f", "00aaff"),
            ("numrow", "22dd44"),
            ("tab", "ff0066"),
            ("esc", "ff0066"),
            ("arrows", "ffd000"),
        ],
    ),
    (
        "coding",
        "dark board, function keys and Esc picked out",
        "08080f",
        &[
            ("letters", "1d6f8a"),
            ("numrow", "2a8fb0"),
            ("fkeys", "ff9900"),
            ("esc", "ff2244"),
            ("enter", "55ee66"),
            ("backspace", "ff5566"),
            ("arrows", "7a7aff"),
        ],
    ),
    ("arrows", "only the arrow keys and WASD", "000000", &[("wasd", "ffffff"), ("arrows", "ffffff")]),
    (
        "ice",
        "cool blue to white",
        "0a2a50",
        &[("letters", "3ab0ff"), ("numrow", "7fd0ff"), ("fkeys", "c8ecff"), ("esc", "ffffff")],
    ),
    (
        "ember",
        "warm red and orange",
        "3a0600",
        &[("letters", "ff4a00"), ("numrow", "ff7a10"), ("fkeys", "ffb030"), ("esc", "ffe080")],
    ),
    ("stealth", "almost off, just the letters", "000000", &[("letters", "14323c")]),
];

/// The built-in designs by name. A built-in that does not check out against `map` (a map of
/// another laptop) is left out rather than failing everything.
pub fn builtin(map: &KeyMap) -> Vec<(String, Design)> {
    let earlier = BUILTIN.iter().map(|(name, info, base, keys)| (*name, *info, *base, Colors::of(keys)));
    earlier
        .chain(templates::all())
        .filter_map(|(name, info, base, keys)| {
            Design::checked(map, info, base, keys, Colors::default(), Source::BuiltIn)
                .ok()
                .map(|d| (name.to_owned(), d))
        })
        .collect()
}

/// How many of the built-in designs come from the earlier editions (the first ones, unchanged);
/// the ones after them are the templates.
pub const EARLIER_BUILTIN: usize = BUILTIN.len();

/// The folder of your designs: `$XDG_CONFIG_HOME/truesense/designs` (an empty value counts as
/// unset), else `~/.config/truesense/designs` under `home`.
pub fn user_dir(xdg_config_home: Option<&str>, home: &Path) -> PathBuf {
    let base = match xdg_config_home.filter(|x| !x.is_empty()) {
        Some(x) => PathBuf::from(x),
        None => home.join(".config"),
    };
    base.join("truesense/designs")
}

/// The designs: the built-in ones and the files of one folder.
pub struct DesignStore {
    dir: PathBuf,
    map: &'static KeyMap,
}

impl DesignStore {
    /// A store over the folder `dir` (see [`user_dir`]), for the PH16-71 key map.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into(), map: KeyMap::verified() }
    }

    /// The key map the designs are checked against.
    pub fn key_map(&self) -> &'static KeyMap {
        self.map
    }

    /// Every design by name: the built-in ones, then the files of the folder in file-name order.
    /// A file with the name of a built-in design replaces it in place; a file that is unreadable,
    /// too large, not JSON or not a valid design is skipped, never fatal.
    pub fn load_all(&self) -> Vec<(String, Design)> {
        let mut all = builtin(self.map);
        let mut files: Vec<String> = std::fs::read_dir(&self.dir)
            .map(|entries| {
                entries.filter_map(Result::ok).filter_map(|e| e.file_name().into_string().ok()).collect()
            })
            .unwrap_or_default();
        files.sort();
        for file in files {
            let Some(name) = file.strip_suffix(".json") else { continue };
            let Ok(bytes) = fsx::read_limited(&self.dir.join(&file), fsx::MAX_SETTINGS_FILE) else {
                continue;
            };
            let Ok(text) = String::from_utf8(bytes) else { continue };
            let Ok(design) = Design::parse(self.map, &text, Source::Yours) else { continue };
            match all.iter_mut().find(|(n, _)| n == name) {
                Some(entry) => entry.1 = design,
                None => all.push((name.to_owned(), design)),
            }
        }
        all
    }

    /// One design by name.
    pub fn get(&self, name: &str) -> Result<Design, DesignError> {
        self.load_all()
            .into_iter()
            .find(|(n, _)| n == name)
            .map(|(_, d)| d)
            .ok_or_else(|| invalid(format!("no design '{name}' (see: truesense keyboard design list)")))
    }

    /// Stores the picture now on the keyboard as one of your designs (its first 126 cells) and
    /// returns the file. The name is at most 40 characters and has no `/`, `\` or `.`.
    pub fn save(&self, name: &str, cells: &[Color]) -> Result<PathBuf, DesignError> {
        if name.is_empty() || name.chars().any(|c| matches!(c, '/' | '\\' | '.')) || name.chars().count() > 40
        {
            return Err(invalid("design name: letters, digits, - and _"));
        }
        #[derive(Serialize)]
        struct Saved {
            info: &'static str,
            base: &'static str,
            cells: Colors,
        }
        let saved = Saved {
            info: "saved from the keyboard",
            base: "000000",
            cells: Colors(
                cells
                    .iter()
                    .take(LIT_CELLS)
                    .enumerate()
                    .map(|(i, c)| (i.to_string(), keymap::hex(*c)))
                    .collect(),
            ),
        };
        let mut text = Vec::new();
        let formatter = serde_json::ser::PrettyFormatter::with_indent(b" ");
        saved
            .serialize(&mut serde_json::Serializer::with_formatter(&mut text, formatter))
            .map_err(DesignError::Parse)?;
        let path = self.dir.join(format!("{name}.json"));
        fsx::write_atomic_mkdir(&path, &text, 0o644)
            .map_err(|source| DesignError::File { op: "save design", source })?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests;
