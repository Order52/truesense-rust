//! Where the keys are on the keyboard: one rectangle per key, in key units.
//!
//! The cell numbering says which LED belongs to which key, not where the key is (inside a column
//! the cells are not in physical order), so anything that needs a position (a sweep around the
//! middle of the keyboard, a picture of the keyboard) uses these rectangles. One unit is a normal
//! key; the picture is [`WIDTH`] by [`HEIGHT`] units. The names are the key map's.
//!
//! The positions are measured from a top-down photo of a PH16-71 and checked against the cell
//! capture of the owner's machine: low function keys in four groups, Print Screen, Insert and
//! Delete at the end of that row and the media keys and Power above the numpad; a short left Shift
//! with `\` beside it; an Enter two rows tall, wider in its lower row; full-size arrows with the
//! right arrow under the numpad's first column; the PREDATOR key at the numpad's top left.
#![forbid(unsafe_code)]

/// Where the numpad starts, in key units (a small gap after the main block).
const PAD_X: f64 = 14.48;
/// One numpad column: its keys are narrower than the main block's.
const PAD_PITCH: f64 = 0.83;
/// Width of the keyboard picture in key units (main block 14.25, a gap, the numpad).
pub const WIDTH: f64 = PAD_X + 4.0 * PAD_PITCH;
/// Top edge of each row: the low function row, then a row every unit.
const ROW_Y: [f64; 6] = [0.0, 0.65, 1.65, 2.65, 3.65, 4.65];
/// Height of the function row: its keys are about half a normal key.
const FN_H: f64 = 0.6;
/// Height of the keyboard picture in key units.
pub const HEIGHT: f64 = 5.65;

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
    /// How far the key's lowest row reaches further left than the rest (Enter); zero for a plain
    /// rectangle.
    pub foot: f64,
}

impl KeyRect {
    /// The middle of the key.
    pub fn centre(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

/// The main block, rows top to bottom: key name, left edge and width. Gaps between groups are
/// where the photo has them (the function row's groups, the arrows' notch under right Shift).
// Measured positions, not constants: 6.28 is where the Y key starts, not an approximation of tau.
#[allow(clippy::approx_constant)]
const MAIN: [&[(&str, f64, f64)]; 6] = [
    &[
        ("esc", 0.0, 1.05),
        ("f1", 1.37, 0.855),
        ("f2", 2.225, 0.855),
        ("f3", 3.08, 0.855),
        ("f4", 3.935, 0.855),
        ("f5", 4.99, 0.855),
        ("f6", 5.845, 0.855),
        ("f7", 6.7, 0.855),
        ("f8", 7.555, 0.855),
        ("f9", 8.51, 0.855),
        ("f10", 9.365, 0.855),
        ("f11", 10.22, 0.855),
        ("f12", 11.075, 0.855),
        ("prtsc", 12.0, 0.75),
        ("ins", 12.75, 0.75),
        ("del", 13.5, 0.75),
    ],
    &[
        ("`", 0.0, 0.8),
        ("1", 0.8, 1.0),
        ("2", 1.8, 1.0),
        ("3", 2.8, 1.0),
        ("4", 3.8, 1.0),
        ("5", 4.8, 1.0),
        ("6", 5.8, 1.0),
        ("7", 6.8, 1.0),
        ("8", 7.8, 1.0),
        ("9", 8.8, 1.0),
        ("0", 9.8, 1.0),
        ("minus", 10.8, 1.0),
        ("=", 11.8, 1.0),
        ("backspace", 12.8, 1.45),
    ],
    &[
        ("tab", 0.0, 1.28),
        ("q", 1.28, 1.0),
        ("w", 2.28, 1.0),
        ("e", 3.28, 1.0),
        ("r", 4.28, 1.0),
        ("t", 5.28, 1.0),
        ("y", 6.28, 1.0),
        ("u", 7.28, 1.0),
        ("i", 8.28, 1.0),
        ("o", 9.28, 1.0),
        ("p", 10.28, 1.0),
        ("[", 11.28, 1.0),
        ("]", 12.28, 1.0),
        // Enter: two rows tall, see ENTER_FOOT.
        ("return", 13.28, 0.97),
    ],
    &[
        ("caps", 0.0, 1.48),
        ("a", 1.48, 1.0),
        ("s", 2.48, 1.0),
        ("d", 3.48, 1.0),
        ("f", 4.48, 1.0),
        ("g", 5.48, 1.0),
        ("h", 6.48, 1.0),
        ("j", 7.48, 1.0),
        ("k", 8.48, 1.0),
        ("l", 9.48, 1.0),
        (";", 10.48, 1.0),
        ("'", 11.48, 1.0),
    ],
    &[
        ("lshift", 0.0, 0.95),
        ("\\", 0.95, 1.0),
        ("z", 1.95, 1.0),
        ("x", 2.95, 1.0),
        ("c", 3.95, 1.0),
        ("v", 4.95, 1.0),
        ("b", 5.95, 1.0),
        ("n", 6.95, 1.0),
        ("m", 7.95, 1.0),
        (",", 8.95, 1.0),
        (".", 9.95, 1.0),
        ("/", 10.95, 1.0),
        ("rshift", 11.95, 1.35),
        ("up", 13.3, 0.95),
    ],
    &[
        ("ctrl", 0.0, 1.0),
        ("fn", 1.0, 1.0),
        ("windows", 2.0, 1.0),
        ("lalt", 3.0, 1.0),
        ("space", 4.0, 4.9),
        ("ralt", 8.9, 1.0),
        ("options", 9.9, 1.0),
        ("rctrl", 10.9, 1.5),
        ("left", 12.4, 0.9),
        ("down", 13.3, 0.95),
    ],
];

/// How far Enter's lower half reaches left of its upper half (to the right edge of `'`).
const ENTER_FOOT: f64 = 0.8;

/// The numpad block: name, row, column 0 to 3, height in rows. The media keys and Power sit above
/// it in the function row; its bottom row starts with the right arrow.
const PAD: [(&str, usize, f64, usize); 23] = [
    ("backward", 0, 0.0, 1),
    ("playpause", 0, 1.0, 1),
    ("forward", 0, 2.0, 1),
    ("power", 0, 3.0, 1),
    ("predator", 1, 0.0, 1),
    ("numlock", 1, 1.0, 1),
    ("num/", 1, 2.0, 1),
    ("*", 1, 3.0, 1),
    ("num7", 2, 0.0, 1),
    ("num8", 2, 1.0, 1),
    ("num9", 2, 2.0, 1),
    ("-", 2, 3.0, 1),
    ("num4", 3, 0.0, 1),
    ("num5", 3, 1.0, 1),
    ("num6", 3, 2.0, 1),
    ("+", 3, 3.0, 1),
    ("num1", 4, 0.0, 1),
    ("num2", 4, 1.0, 1),
    ("num3", 4, 2.0, 1),
    ("numenter", 4, 3.0, 2),
    ("right", 5, 0.0, 1),
    ("num0", 5, 1.0, 1),
    ("num.", 5, 2.0, 1),
];

/// Height of one row's keys.
fn row_h(row: usize) -> f64 {
    if row == 0 { FN_H } else { 1.0 }
}

/// Every key of the picture, main block first and then the numpad, each row left to right.
pub fn key_rects() -> Vec<KeyRect> {
    let mut out = Vec::new();
    for (row, keys) in MAIN.iter().enumerate() {
        for (name, x, w) in *keys {
            let tall = *name == "return";
            let h = if tall { ROW_Y[row + 1] - ROW_Y[row] + 1.0 } else { row_h(row) };
            out.push(KeyRect {
                name,
                x: *x,
                y: ROW_Y[row],
                w: *w,
                h,
                foot: if tall { ENTER_FOOT } else { 0.0 },
            });
        }
    }
    for (name, row, col, rows) in PAD {
        let h = ROW_Y[row + rows - 1] - ROW_Y[row] + row_h(row + rows - 1);
        out.push(KeyRect { name, x: PAD_X + col * PAD_PITCH, y: ROW_Y[row], w: PAD_PITCH, h, foot: 0.0 });
    }
    out
}
