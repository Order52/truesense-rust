//! The templates: ready-made still looks that ship next to the six designs of the earlier editions.
//!
//! Each one is a design like any other (a base colour, then keys and groups painted in order), made
//! from the key groups of the verified key map, so it shows on the Per-key tab with a thumbnail, can
//! be applied with one click, copied to be edited and saved as one of yours. They are plain data:
//! the same checks as a design file apply, and `builtin` leaves out any that does not check out
//! against the key map in use.
//!
//! A row of the keyboard is one of seven lists below (function row, number row, the three letter
//! rows with the keys at their ends, the bottom row, and the numpad); the row looks paint them one
//! colour each.
#![forbid(unsafe_code)]

use super::Colors;

/// A template: name, description (at most 80 characters), base colour, keys in painting order.
pub(super) type Template = (&'static str, &'static str, &'static str, Colors);

/// Esc, the function keys and the three keys after them.
const FUNCTION_ROW: &[&str] = &[
    "esc", "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10", "f11", "f12", "prtsc", "ins", "del",
];
/// The number row with its punctuation and Backspace.
const NUMBER_ROW: &[&str] =
    &["`", "1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "minus", "=", "backspace"];
/// Tab, Q to P and the brackets.
const TOP_ROW: &[&str] = &["tab", "q", "w", "e", "r", "t", "y", "u", "i", "o", "p", "[", "]", "\\"];
/// Caps Lock, A to L, the punctuation and Enter.
const HOME_ROW: &[&str] = &["caps", "a", "s", "d", "f", "g", "h", "j", "k", "l", ";", "'", "return"];
/// Left Shift, Z to M, the punctuation, Right Shift and Up.
const BOTTOM_LETTERS: &[&str] = &["lshift", "z", "x", "c", "v", "b", "n", "m", ",", ".", "/", "rshift", "up"];
/// Ctrl, Fn, Win, Alt, the space bar and the keys right of it.
const SPACE_ROW: &[&str] =
    &["ctrl", "fn", "windows", "lalt", "space", "ralt", "options", "rctrl", "left", "down", "right"];
/// The numpad block with the media keys above it.
const PAD: &[&str] = &[
    "backward",
    "playpause",
    "forward",
    "power",
    "predator",
    "numlock",
    "num/",
    "*",
    "num7",
    "num8",
    "num9",
    "-",
    "num4",
    "num5",
    "num6",
    "+",
    "num1",
    "num2",
    "num3",
    "numenter",
    "num0",
    "num.",
];

fn paint(list: &[&str], colour: &str) -> Vec<(String, String)> {
    list.iter().map(|k| ((*k).to_owned(), colour.to_owned())).collect()
}

/// Rows painted one colour each, from the function row down to the numpad.
fn rows(colours: [&str; 7]) -> Colors {
    let lists = [FUNCTION_ROW, NUMBER_ROW, TOP_ROW, HOME_ROW, BOTTOM_LETTERS, SPACE_ROW, PAD];
    Colors(lists.iter().zip(colours).flat_map(|(list, c)| paint(list, c)).collect())
}

fn keys(parts: &[(&[&str], &str)]) -> Colors {
    Colors(parts.iter().flat_map(|(list, c)| paint(list, c)).collect())
}

/// The templates in display order.
pub(super) fn all() -> Vec<Template> {
    vec![
        (
            "fps",
            "shooter: WASD, ability keys and the weapon numbers stand out",
            "06060c",
            keys(&[
                (&["w", "a", "s", "d"], "ff2a2a"),
                (&["lshift", "ctrl", "space"], "ff8800"),
                (&["1", "2", "3", "4", "5"], "00e0ff"),
                (&["q", "e", "f", "g"], "ffd000"),
                (&["r"], "39ff14"),
                (&["c", "v", "x", "z"], "b86bff"),
                (&["tab", "esc"], "ff0066"),
                (&["up", "down", "left", "right"], "3a3a5a"),
            ]),
        ),
        (
            "moba",
            "Q W E R skills, D F spells, the number row for items",
            "08060f",
            keys(&[
                (&["q", "w", "e", "r"], "b44dff"),
                (&["d", "f"], "00d1ff"),
                (&["1", "2", "3", "4", "5", "6"], "ffcc00"),
                (&["a"], "ff3b3b"),
                (&["s"], "ff9900"),
                (&["b"], "3ddc84"),
                (&["tab", "space"], "ffffff"),
                (&["ctrl"], "9a9ab0"),
                (&["esc"], "ff0066"),
            ]),
        ),
        (
            "typing",
            "home-row guide: F and J bumps white, the rest of the row blue",
            "0b1220",
            keys(&[
                (
                    &["q", "w", "e", "r", "t", "y", "u", "i", "o", "p", "z", "x", "c", "v", "b", "n", "m"],
                    "27405e",
                ),
                (&["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"], "1b2c44"),
                (&["space"], "27405e"),
                (&["a", "s", "d", "g", "h", "k", "l", ";"], "3ab0ff"),
                (&["f", "j"], "ffffff"),
                (&["return", "backspace", "lshift", "rshift"], "6a8caf"),
            ]),
        ),
        (
            "media",
            "editing: arrows to scrub, space to play, J K L in the middle",
            "0d0d12",
            keys(&[
                (&["f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10", "f11", "f12"], "7c4dff"),
                (&["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"], "ffd740"),
                (&["j", "k", "l"], "ff9100"),
                (&["i", "o"], "00b0ff"),
                (&["up", "down", "left", "right"], "ffffff"),
                (&["space"], "00e676"),
                (&["ctrl", "lshift", "lalt"], "ff5252"),
                (&["backward", "playpause", "forward"], "00e676"),
            ]),
        ),
        (
            "sunset",
            "a dusk gradient from violet at the top to gold at the bottom",
            "3a0ca3",
            rows(["7b2cbf", "c9184a", "ff5d00", "ff8c00", "ffb703", "ffd60a", "ff5d00"]),
        ),
        (
            "neon-grid",
            "cyan and magenta rows over a deep purple board",
            "0a0014",
            rows(["ff00aa", "00e5ff", "ff00aa", "00e5ff", "ff00aa", "00e5ff", "7a00ff"]),
        ),
        (
            "rainbow-rows",
            "every row of the keyboard in its own colour of the rainbow",
            "000000",
            rows(["ff3b30", "ff9500", "ffcc00", "34c759", "32ade6", "af52de", "5856d6"]),
        ),
        (
            "night",
            "a dim amber board for a dark room, easy on the eyes",
            "000000",
            keys(&[
                (FUNCTION_ROW, "1a0e00"),
                (NUMBER_ROW, "1f1100"),
                (TOP_ROW, "2b1700"),
                (HOME_ROW, "2b1700"),
                (BOTTOM_LETTERS, "2b1700"),
                (SPACE_ROW, "2b1700"),
                (&["esc", "return", "backspace"], "4a1a00"),
            ]),
        ),
    ]
}
