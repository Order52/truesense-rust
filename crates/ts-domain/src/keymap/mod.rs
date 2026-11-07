//! keymap: key names to lighting cells for the PH16-71 keyboard, and whole-keyboard colour pictures.
//!
//! A cell is numbered `column * 6 + row` (22 columns, 6 rows, row 0 on top). The table comes from
//! an owner-verified capture (each cell was lit and the key the owner pressed was recorded) and is
//! embedded in the binary, so a laptop without the data directory still knows its keys.
//! Corrections against the community map, verified by key presses: cell 2 is Esc, cell 5 is
//! CapsLock, cell 70 is the main-row minus.
//!
//! Everything here is pure: no file, clock or device is touched.
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::fmt;
use std::sync::OnceLock;

use serde::Deserialize;

/// How many colours the keyboard takes in one upload: 4 bytes per cell, 8 reports of 64 bytes.
pub const NUM_CELLS: usize = 128;
/// The cells the vendor's Windows application lights (0..125); the last two stay black.
pub const LIT_CELLS: usize = 126;
/// The number of key columns.
pub const COLUMNS: usize = 22;
/// The number of key rows.
pub const ROWS: usize = 6;

/// One colour, red green blue.
pub type Color = [u8; 3];

/// A key name, group name or colour that is not usable. The text is what the Python edition
/// said, so it reaches the user unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeymapError {
    /// A colour that is not `RRGGBB` (the text is the colour after the leading `#`s were removed).
    BadColour(String),
    /// A key, group or `cellN` name the map does not know (the text is the name as given).
    UnknownKey(String),
    /// The key map data is damaged (only possible for a map parsed from outside the binary).
    Corrupt(String),
}

impl fmt::Display for KeymapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadColour(c) => write!(f, "bad colour '{c}', use RRGGBB"),
            Self::UnknownKey(k) => write!(f, "unknown key '{k}' (see: truesense keyboard list-keys)"),
            Self::Corrupt(why) => write!(f, "key map: {why}"),
        }
    }
}

impl std::error::Error for KeymapError {}

/// The colour as lower-case `rrggbb`, the form every saved file and the user state use.
pub fn hex(c: Color) -> String {
    format!("{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

/// Reads `RRGGBB` in either case. Any number of leading `#` is accepted because the Python
/// edition stripped them all (`lstrip("#")`); anything else, including spaces, is refused.
pub fn parse_hex(text: &str) -> Result<Color, KeymapError> {
    let t = text.trim_start_matches('#');
    let b = t.as_bytes();
    if b.len() != 6 || !b.iter().all(u8::is_ascii_hexdigit) {
        return Err(KeymapError::BadColour(t.to_owned()));
    }
    let nibble = |c: u8| match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        _ => c - b'A' + 10,
    };
    let byte = |i: usize| nibble(b[i]) << 4 | nibble(b[i + 1]);
    Ok([byte(0), byte(2), byte(4)])
}

/// The embedded capture of the PH16-71.
const VERIFIED_JSON: &str = include_str!("keymap_ph16-71_verified.json");

/// Cells the capture labelled wrongly (see the module comment).
const OVERRIDE: [(usize, &str); 3] = [(2, "esc"), (5, "caps"), (70, "minus")];

/// The capture's labels are cut to five letters for some keys; these are the names users type.
const ALIAS: [(&str, &str); 16] = [
    ("oemminus", "minus"),
    ("capsl", "caps"),
    ("lshif", "lshift"),
    ("rshif", "rshift"),
    ("windo", "win"),
    ("optio", "menu"),
    ("retur", "enter"),
    ("backs", "backspace"),
    ("preda", "predator"),
    ("forwa", "next"),
    ("backw", "prev"),
    ("playp", "playpause"),
    ("numlk", "numlock"),
    ("numen", "numenter"),
    ("num.", "num."),
    ("oemplus", "="),
];

#[derive(Deserialize)]
struct Capture {
    cells: std::collections::BTreeMap<String, CaptureCell>,
}

#[derive(Deserialize)]
struct CaptureCell {
    label: Option<String>,
}

/// The key-name table: single keys and named groups, each a list of cells.
#[derive(Debug, Clone)]
pub struct KeyMap {
    /// Names in the order the capture first mentions them (the order the Python edition listed them in).
    names: Vec<(String, Vec<usize>)>,
    index: HashMap<String, usize>,
    groups: Vec<(&'static str, Vec<usize>)>,
}

impl KeyMap {
    /// The PH16-71 map, parsed once.
    pub fn verified() -> &'static KeyMap {
        static MAP: OnceLock<KeyMap> = OnceLock::new();
        // The data is a constant of the binary and a test parses it; the empty map is only a
        // way to avoid a panic path, and every key lookup then reports "unknown key".
        MAP.get_or_init(|| KeyMap::parse(VERIFIED_JSON).unwrap_or_else(|_| KeyMap::build(Vec::new())))
    }

    /// Builds a map from a capture file (`{"cells": {"<cell>": {"label": "..."}}}`). Hostile
    /// input is an error, never a panic.
    pub fn parse(json: &str) -> Result<KeyMap, KeymapError> {
        let capture: Capture = serde_json::from_str(json).map_err(|e| KeymapError::Corrupt(e.to_string()))?;
        let mut labelled = Vec::with_capacity(capture.cells.len());
        for (id, cell) in capture.cells {
            let n: usize = id.parse().map_err(|_| KeymapError::Corrupt(format!("bad cell {id:?}")))?;
            if n >= NUM_CELLS {
                return Err(KeymapError::Corrupt(format!("bad cell {id:?}")));
            }
            labelled.push((n, cell.label.unwrap_or_default()));
        }
        // The capture lists the cells in ascending order and the order of cells inside a name
        // follows it (a BTreeMap of strings would sort "10" before "2").
        labelled.sort_unstable_by_key(|(n, _)| *n);
        Ok(KeyMap::build(labelled))
    }

    fn build(labelled: Vec<(usize, String)>) -> KeyMap {
        let mut names: Vec<(String, Vec<usize>)> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        for (cell, label) in labelled {
            let name = match OVERRIDE.iter().find(|(c, _)| *c == cell) {
                Some((_, n)) => (*n).to_owned(),
                None => normalise(&label),
            };
            if name.is_empty() {
                continue;
            }
            match index.get(&name) {
                Some(&i) => names[i].1.push(cell),
                None => {
                    index.insert(name.clone(), names.len());
                    names.push((name, vec![cell]));
                }
            }
        }
        // Friendlier names for the capture's own: they share the cells and never replace a name.
        for (friendly, original) in [("enter", "return"), ("win", "windows")] {
            if !index.contains_key(friendly)
                && let Some(&i) = index.get(original)
            {
                let cells = names[i].1.clone();
                index.insert(friendly.to_owned(), names.len());
                names.push((friendly.to_owned(), cells));
            }
        }
        let join = |list: &[&str]| -> Vec<usize> {
            list.iter().filter_map(|n| index.get(*n)).flat_map(|&i| names[i].1.iter().copied()).collect()
        };
        let letters: Vec<String> = ('a'..='z').map(String::from).collect();
        let fkeys: Vec<String> = (1..=12).map(|i| format!("f{i}")).collect();
        // Python's list("1234567890"): the digits 1 to 9, then 0, as on the keyboard.
        let numrow: Vec<String> = "1234567890".chars().map(String::from).collect();
        let refs = |v: &[String]| -> Vec<usize> { join(&v.iter().map(String::as_str).collect::<Vec<_>>()) };
        let numpad: Vec<usize> =
            names.iter().filter(|(n, _)| n.starts_with("num")).flat_map(|(_, c)| c.iter().copied()).collect();
        let groups = vec![
            ("wasd", join(&["w", "a", "s", "d"])),
            ("arrows", join(&["up", "down", "left", "right"])),
            ("fkeys", refs(&fkeys)),
            ("numrow", refs(&numrow)),
            ("letters", refs(&letters)),
            ("numpad", numpad),
            ("all", (0..LIT_CELLS).collect()),
        ];
        KeyMap { names, index, groups }
    }

    /// Every single-key name with its cells, in the order the capture first mentions them.
    pub fn names(&self) -> &[(String, Vec<usize>)] {
        &self.names
    }

    /// The named groups (`wasd`, `arrows`, `fkeys`, `numrow`, `letters`, `numpad`, `all`).
    pub fn groups(&self) -> &[(&'static str, Vec<usize>)] {
        &self.groups
    }

    /// The cells of one key name (not a group, no `cellN`), or `None`. The name is matched in lower case.
    pub fn key(&self, name: &str) -> Option<&[usize]> {
        self.index.get(&name.to_lowercase()).map(|&i| self.names[i].1.as_slice())
    }

    /// Resolves a group, a key name or `cellN` (N below 128) to cells. A group wins over a key of
    /// the same name, as in the Python edition.
    pub fn cells_for(&self, name: &str) -> Result<Vec<usize>, KeymapError> {
        let n = name.to_lowercase();
        if let Some((_, cells)) = self.groups.iter().find(|(g, _)| *g == n) {
            return Ok(cells.clone());
        }
        if let Some(&i) = self.index.get(&n) {
            return Ok(self.names[i].1.clone());
        }
        // Only ASCII digits count: Python's `\d` also matched other scripts, which no one types.
        if let Some(digits) = n.strip_prefix("cell")
            && !digits.is_empty()
            && digits.bytes().all(|b| b.is_ascii_digit())
            && let Ok(c) = digits.parse::<usize>()
            && c < NUM_CELLS
        {
            return Ok(vec![c]);
        }
        Err(KeymapError::UnknownKey(name.to_owned()))
    }
}

fn normalise(label: &str) -> String {
    let l = label.to_lowercase();
    ALIAS.iter().find(|(from, _)| *from == l).map_or(l, |(_, to)| (*to).to_owned())
}

/// One colour channel rounded the way Python's `round()` rounds (halves to the even neighbour,
/// which the saved frames of the earlier editions were computed with) and clamped to a byte.
fn channel(f: f64) -> u8 {
    let r = f.round_ties_even();
    if r.is_nan() {
        0
    } else {
        // Clamped to 0..=255 first, so the conversion cannot truncate.
        r.clamp(0.0, 255.0) as u8
    }
}

/// Blends two colours: `t` = 0 gives `a`, `t` = 1 gives `b`.
pub(crate) fn lerp(a: Color, b: Color, t: f64) -> Color {
    let one = |i: usize| channel(f64::from(a[i]) + (f64::from(b[i]) - f64::from(a[i])) * t);
    [one(0), one(1), one(2)]
}

/// Python's float modulo: the result takes the sign of the divisor.
pub(crate) fn py_mod(x: f64, y: f64) -> f64 {
    let r = x % y;
    if r != 0.0 && (r < 0.0) != (y < 0.0) { r + y } else { r }
}

/// The fully saturated colour at hue `h` (0..1, wraps around; negative values wrap too).
/// A hue that is not a number is taken as 0.
pub fn hsv(h: f64) -> Color {
    let h = if h.is_finite() { py_mod(h, 1.0) } else { 0.0 };
    let sector = (h * 6.0) as i64; // 0..=6: Python's `h % 1.0` can round up to 1.0 for tiny negative h
    let f = h * 6.0 - sector as f64;
    let q = 1.0 - f;
    let (r, g, b) = match sector.rem_euclid(6) {
        0 => (1.0, f, 0.0),
        1 => (q, 1.0, 0.0),
        2 => (0.0, 1.0, f),
        3 => (0.0, q, 1.0),
        4 => (f, 0.0, 1.0),
        _ => (1.0, 0.0, q),
    };
    [channel(r * 255.0), channel(g * 255.0), channel(b * 255.0)]
}

/// A full colour list (128 cells) with every lit cell set to `base` and the two unused cells black.
pub fn canvas(base: Color) -> Vec<Color> {
    let mut cv = vec![[0; 3]; NUM_CELLS];
    cv[..LIT_CELLS].fill(base);
    cv
}

/// A left-to-right blend over the 22 columns (cell / 6).
pub fn gradient(c1: Color, c2: Color) -> Vec<Color> {
    let mut cv = vec![[0; 3]; NUM_CELLS];
    for (i, cell) in cv.iter_mut().enumerate().take(LIT_CELLS) {
        *cell = lerp(c1, c2, (i / ROWS) as f64 / (COLUMNS - 1) as f64);
    }
    cv
}

/// The hues spread over the 22 columns.
pub fn rainbow() -> Vec<Color> {
    let mut cv = vec![[0; 3]; NUM_CELLS];
    for (i, cell) in cv.iter_mut().enumerate().take(LIT_CELLS) {
        *cell = hsv((i / ROWS) as f64 / COLUMNS as f64);
    }
    cv
}

#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) mod testutil;
