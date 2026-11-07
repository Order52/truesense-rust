//! Where the keys are on the keyboard: one rectangle per key, in key units.
//!
//! The cell numbering says which LED belongs to which key, not where the key is (inside a column
//! the cells are not in physical order), so anything that needs a position (a sweep around the
//! middle of the keyboard, a picture of the keyboard) uses these rectangles. One unit is a normal
//! key; the picture is [`WIDTH`] by [`HEIGHT`] units. The names are the key map's.
#![forbid(unsafe_code)]

/// Width of the keyboard picture in key units (main block 15, a gap, the numpad).
pub const WIDTH: f64 = 20.5;
/// Height of the keyboard picture in key units (six rows and the gap under the function row).
pub const HEIGHT: f64 = 6.4;

/// One key's rectangle: the top left corner and the size, in key units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyRect {
    /// The key map's name for the key.
    pub name: &'static str,
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
}

impl KeyRect {
    /// The middle of the key.
    pub fn centre(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

/// The main block, rows top to bottom: key name and width. The function row is slightly narrower
/// (0.93) so that it ends flush with the 15 unit wide rows below it.
const MAIN: [&[(&str, f64)]; 6] = [
    &[
        ("esc", 1.0),
        ("f1", 0.93),
        ("f2", 0.93),
        ("f3", 0.93),
        ("f4", 0.93),
        ("f5", 0.93),
        ("f6", 0.93),
        ("f7", 0.93),
        ("f8", 0.93),
        ("f9", 0.93),
        ("f10", 0.93),
        ("f11", 0.93),
        ("f12", 0.93),
        ("prtsc", 0.93),
        ("ins", 0.93),
        ("del", 0.93),
    ],
    &[
        ("`", 1.0),
        ("1", 1.0),
        ("2", 1.0),
        ("3", 1.0),
        ("4", 1.0),
        ("5", 1.0),
        ("6", 1.0),
        ("7", 1.0),
        ("8", 1.0),
        ("9", 1.0),
        ("0", 1.0),
        ("minus", 1.0),
        ("=", 1.0),
        ("backspace", 2.0),
    ],
    &[
        ("tab", 1.5),
        ("q", 1.0),
        ("w", 1.0),
        ("e", 1.0),
        ("r", 1.0),
        ("t", 1.0),
        ("y", 1.0),
        ("u", 1.0),
        ("i", 1.0),
        ("o", 1.0),
        ("p", 1.0),
        ("[", 1.0),
        ("]", 1.0),
        ("\\", 1.5),
    ],
    &[
        ("caps", 1.75),
        ("a", 1.0),
        ("s", 1.0),
        ("d", 1.0),
        ("f", 1.0),
        ("g", 1.0),
        ("h", 1.0),
        ("j", 1.0),
        ("k", 1.0),
        ("l", 1.0),
        (";", 1.0),
        ("'", 1.0),
        ("return", 2.25),
    ],
    &[
        ("lshift", 2.25),
        ("z", 1.0),
        ("x", 1.0),
        ("c", 1.0),
        ("v", 1.0),
        ("b", 1.0),
        ("n", 1.0),
        ("m", 1.0),
        (",", 1.0),
        (".", 1.0),
        ("/", 1.0),
        ("rshift", 1.75),
        ("up", 1.0),
    ],
    &[
        ("ctrl", 1.25),
        ("fn", 1.25),
        ("windows", 1.25),
        ("lalt", 1.25),
        ("space", 4.75),
        ("ralt", 1.0),
        ("options", 1.0),
        ("rctrl", 1.25),
        ("left", 1.0),
        ("down", 1.0),
        ("right", 1.0),
    ],
];

/// The numpad block: name, column 0 to 3, width, height.
const PAD: [&[(&str, f64, f64, f64)]; 6] = [
    &[
        ("backward", 0.0, 1.0, 1.0),
        ("playpause", 1.0, 1.0, 1.0),
        ("forward", 2.0, 1.0, 1.0),
        ("power", 3.0, 1.0, 1.0),
    ],
    &[("predator", 0.0, 1.0, 1.0), ("numlock", 1.0, 1.0, 1.0), ("num/", 2.0, 1.0, 1.0), ("*", 3.0, 1.0, 1.0)],
    &[("num7", 0.0, 1.0, 1.0), ("num8", 1.0, 1.0, 1.0), ("num9", 2.0, 1.0, 1.0), ("-", 3.0, 1.0, 1.0)],
    &[("num4", 0.0, 1.0, 1.0), ("num5", 1.0, 1.0, 1.0), ("num6", 2.0, 1.0, 1.0), ("+", 3.0, 1.0, 1.0)],
    &[("num1", 0.0, 1.0, 1.0), ("num2", 1.0, 1.0, 1.0), ("num3", 2.0, 1.0, 1.0), ("numenter", 3.0, 1.0, 2.0)],
    &[("num0", 0.0, 2.0, 1.0), ("num.", 2.0, 1.0, 1.0)],
];

/// Where the numpad starts, in key units.
const PAD_X: f64 = 16.5;
/// Top edge of each key row (the function row sits close above the number row).
const ROW_Y: [f64; 6] = [0.0, 1.3, 2.3, 3.3, 4.3, 5.3];

/// Every key of the picture, main block first and then the numpad, each row left to right.
pub fn key_rects() -> Vec<KeyRect> {
    let mut out = Vec::new();
    for (row, keys) in MAIN.iter().enumerate() {
        let mut x = 0.0;
        for (name, w) in *keys {
            out.push(KeyRect { name, x, y: ROW_Y[row], w: *w, h: 1.0 });
            x += w;
        }
    }
    for (row, keys) in PAD.iter().enumerate() {
        for (name, col, w, h) in *keys {
            out.push(KeyRect { name, x: PAD_X + col, y: ROW_Y[row], w: *w, h: *h });
        }
    }
    out
}
