//! The key picture: the layout of the keyboard, cells, geometry and hit testing.
//!
//! A part of [`super`] (the Lighting page's view-model); it shares the parent's names.
#![forbid(unsafe_code)]

use ts_api::api::{KEYBOARD_HEIGHT, KEYBOARD_WIDTH, key_rects};

use super::*;

// ---- the key picture ----------------------------------------------------------------------------

/// One key of the picture, in layout units (one unit is a normal key).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Key {
    /// The name from `keyboard layout`.
    pub name: String,
    /// The cells it lights.
    pub cells: Vec<usize>,
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
    /// The main legend printed on the cap (empty for the space bar and the keys with a symbol).
    #[serde(default)]
    pub label: String,
    /// The legend above it, typed with Shift (`!` over `1`); empty for most keys.
    #[serde(default)]
    pub shift: String,
    /// A small second legend under it (the numpad's `Home`, `PgUp`, ...); empty for most keys.
    #[serde(default)]
    pub sub: String,
    /// A symbol the picture draws instead of text: `windows`, `menu` or `predator`; empty otherwise.
    #[serde(default)]
    pub icon: String,
}

impl Key {
    /// A key of the picture with the legends printed on the PH16-71's cap (US layout).
    fn new(name: &str, cells: Vec<usize>, (x, y, w, h): (f64, f64, f64, f64)) -> Self {
        let (shift, sub, icon) = legends(name);
        Key {
            name: name.to_owned(),
            cells,
            x,
            y,
            w,
            h,
            label: key_label(name),
            shift: shift.to_owned(),
            sub: sub.to_owned(),
            icon: icon.to_owned(),
        }
    }
}

/// The legends of a key besides its main one: (above, under, symbol). The main legend is
/// [`key_label`].
pub(super) fn legends(name: &str) -> (&'static str, &'static str, &'static str) {
    match name {
        "`" => ("~", "", ""),
        "1" => ("!", "", ""),
        "2" => ("@", "", ""),
        "3" => ("#", "", ""),
        "4" => ("$", "", ""),
        "5" => ("%", "", ""),
        "6" => ("^", "", ""),
        "7" => ("&", "", ""),
        "8" => ("*", "", ""),
        "9" => ("(", "", ""),
        "0" => (")", "", ""),
        "minus" => ("_", "", ""),
        "=" => ("+", "", ""),
        "[" => ("{", "", ""),
        "]" => ("}", "", ""),
        "\\" => ("|", "", ""),
        ";" => (":", "", ""),
        "'" => ("\"", "", ""),
        "," => ("<", "", ""),
        "." => (">", "", ""),
        "/" => ("?", "", ""),
        "num7" => ("", "Home", ""),
        "num8" => ("", "\u{25b2}", ""),
        "num9" => ("", "PgUp", ""),
        "num4" => ("", "\u{25c0}", ""),
        "num6" => ("", "\u{25b6}", ""),
        "num1" => ("", "End", ""),
        "num2" => ("", "\u{25bc}", ""),
        "num3" => ("", "PgDn", ""),
        "num0" => ("", "Ins", ""),
        "num." => ("", "Del", ""),
        "windows" => ("", "", "windows"),
        "options" => ("", "", "menu"),
        "predator" => ("", "", "predator"),
        _ => ("", "", ""),
    }
}

/// The picture: keys and its overall size.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    /// Keys in drawing order.
    pub keys: Vec<Key>,
    /// Overall width in units.
    pub width: f64,
    /// Overall height in units.
    pub height: f64,
}

/// The main legend of a key, as printed on the PH16-71's caps (US layout); single characters show
/// themselves in capitals. Keys with a symbol (`windows`, `options`, `predator`, see [`legends`])
/// have none.
pub(super) fn key_label(name: &str) -> String {
    let fixed = match name {
        "backspace" => "Backspace",
        "return" | "numenter" => "Enter",
        "lshift" | "rshift" => "Shift",
        "caps" => "Caps Lock",
        "windows" | "options" | "predator" | "space" => "",
        "lalt" | "ralt" => "Alt",
        "ctrl" | "rctrl" => "Ctrl",
        "up" => "\u{25b2}",
        "down" => "\u{25bc}",
        "left" => "\u{25c0}",
        "right" => "\u{25b6}",
        "prtsc" => "PrtSc",
        "power" => "\u{23fb}",
        "playpause" => "\u{23ef}",
        "backward" => "\u{23ee}",
        "forward" => "\u{23ed}",
        "numlock" => "NumLk",
        "num/" => "/",
        "num." => ".",
        "minus" => "-",
        "-" => "\u{2212}",
        "tab" => "Tab",
        "esc" => "Esc",
        "ins" => "Ins",
        "del" => "Del",
        "fn" => "Fn",
        n if n.len() == 4 && n.starts_with("num") && n.as_bytes().get(3).is_some_and(u8::is_ascii_digit) => {
            return n.chars().skip(3).collect();
        }
        _ => "",
    };
    if !fixed.is_empty() || matches!(name, "space" | "windows" | "options" | "predator") {
        return fixed.to_owned();
    }
    if name.chars().count() == 1 {
        return name.to_uppercase();
    }
    let mut short: String = name.chars().take(3).collect();
    if let Some(first) = short.get(0..1).map(str::to_uppercase) {
        short.replace_range(0..1, &first);
    }
    short
}

/// Reads the answer of `keyboard layout` (`{"keys": {"name": [{"cell"}|number, ...]}}`) into
/// name to cells. Entries of the wrong shape and cells beyond the keyboard are skipped.
pub(crate) fn name_cells(layout: &Value) -> BTreeMap<String, Vec<usize>> {
    let keys = at(layout, "keys").and_then(Value::as_object);
    keys.into_iter()
        .flatten()
        .filter_map(|(name, cells)| {
            let cells: Vec<usize> = cells
                .as_array()?
                .iter()
                .filter_map(|c| c.as_u64().or_else(|| c.get("cell").and_then(Value::as_u64)))
                .filter_map(|c| usize::try_from(c).ok())
                .filter(|c| *c < CELLS)
                .collect();
            (!cells.is_empty()).then(|| (name.clone(), cells))
        })
        .collect()
}

/// Builds the picture from the key names the machine reports. A key the picture does not know still
/// gets a cap, in an extra row below; names that share cells with an earlier key (aliases such as
/// `enter` for `return`) are drawn once.
pub(crate) fn build_layout(names: &BTreeMap<String, Vec<usize>>) -> Layout {
    let mut seen: BTreeSet<usize> = BTreeSet::new();
    let mut keys = Vec::new();
    let mut add = |name: &str, (x, y, w, h): (f64, f64, f64, f64), keys: &mut Vec<Key>| {
        if let Some(cells) = names.get(name)
            && !cells.iter().any(|c| seen.contains(c))
        {
            seen.extend(cells.iter().copied());
            keys.push(Key::new(name, cells.clone(), (x, y, w, h)));
        }
    };
    for r in key_rects() {
        add(r.name, (r.x, r.y, r.w, r.h), &mut keys);
    }
    let extra: Vec<(&String, &Vec<usize>)> = names
        .iter()
        .filter(|(n, cs)| !cs.iter().any(|c| seen.contains(c)) && n.as_str() != "enter" && n.as_str() != "win")
        .collect();
    let mut height = KEYBOARD_HEIGHT;
    for (i, (name, cells)) in extra.into_iter().enumerate() {
        if cells.iter().any(|c| seen.contains(c)) {
            continue;
        }
        seen.extend(cells.iter().copied());
        keys.push(Key::new(name, cells.clone(), (i as f64, KEYBOARD_HEIGHT + 0.2, 1.0, 1.0)));
        height = KEYBOARD_HEIGHT + 1.2;
    }
    Layout { keys, width: KEYBOARD_WIDTH, height }
}

/// Keys are inset so that neighbours do not touch; a hit must land inside the inset rectangle.
pub(super) const INSET: f64 = 0.04;

impl Key {
    /// The key's rectangle (min corner, max corner), inset by [`INSET`].
    fn inset(&self) -> ((f64, f64), (f64, f64)) {
        ((self.x + INSET, self.y + INSET), (self.x + self.w - INSET, self.y + self.h - INSET))
    }
}

/// The key under a point (layout units).
pub(crate) fn key_at(keys: &[Key], x: f64, y: f64) -> Option<&Key> {
    keys.iter().find(|k| {
        let (min, max) = k.inset();
        x >= min.0 && x <= max.0 && y >= min.1 && y <= max.1
    })
}

/// Cells of every key a box touches (any corner order).
pub(crate) fn cells_in_box(keys: &[Key], a: (f64, f64), b: (f64, f64)) -> Vec<usize> {
    let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
    let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
    keys.iter()
        .filter(|k| k.x + k.w > x0 && k.x < x1 && k.y + k.h > y0 && k.y < y1)
        .flat_map(|k| k.cells.iter().copied())
        .collect()
}

/// Where a segment enters a rectangle, as a fraction of the segment (Liang-Barsky); `None` when it
/// misses. A segment that is a point hits when it is inside.
pub(super) fn segment_entry(a: (f64, f64), b: (f64, f64), min: (f64, f64), max: (f64, f64)) -> Option<f64> {
    if ![a.0, a.1, b.0, b.1].iter().all(|v| v.is_finite()) {
        return None;
    }
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    for (p, q) in [(-dx, a.0 - min.0), (dx, max.0 - a.0), (-dy, a.1 - min.1), (dy, max.1 - a.1)] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                t0 = t0.max(r);
            } else {
                t1 = t1.min(r);
            }
            if t0 > t1 {
                return None;
            }
        }
    }
    Some(t0)
}

/// Keys crossed by the straight line a drag made between two pointer positions, in the order the
/// line meets them. A fast drag reports far-apart positions; interpolating exactly means no key in
/// between is skipped.
pub(crate) fn keys_on_segment(keys: &[Key], a: (f64, f64), b: (f64, f64)) -> Vec<&Key> {
    let mut hits: Vec<(f64, &Key)> = keys
        .iter()
        .filter_map(|k| {
            let (min, max) = k.inset();
            segment_entry(a, b, min, max).map(|t| (t, k))
        })
        .collect();
    hits.sort_by(|l, r| l.0.total_cmp(&r.0));
    hits.into_iter().map(|(_, k)| k).collect()
}

/// Horizontal centre of the key that has `cell` (gradients and rainbows run across the board).
pub(super) fn x_of_cell(keys: &[Key], cell: usize) -> f64 {
    keys.iter().find(|k| k.cells.contains(&cell)).map_or(0.0, |k| k.x + k.w / 2.0)
}

/// The key nearest to the one that owns `cell` in a direction (`left`, `right`, `up`, `down`), for
/// arrow-key navigation of the picture. Keys far off the line of travel do not count.
pub fn neighbour(keys: &[Key], cell: usize, dir: &str) -> Option<usize> {
    let (dx, dy) = match dir {
        "left" => (-1.0, 0.0),
        "right" => (1.0, 0.0),
        "up" => (0.0, -1.0),
        "down" => (0.0, 1.0),
        _ => return None,
    };
    let from = keys.iter().find(|k| k.cells.contains(&cell))?;
    let (cx, cy) = (from.x + from.w / 2.0, from.y + from.h / 2.0);
    keys.iter()
        .filter(|k| !std::ptr::eq(*k, from))
        .filter_map(|k| {
            let (ex, ey) = (k.x + k.w / 2.0 - cx, k.y + k.h / 2.0 - cy);
            let along = ex * dx + ey * dy;
            let across = (ex * dy - ey * dx).abs();
            (along >= 0.3 && across <= along * 1.6 + 0.5).then_some((along + across * 2.0, k))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .and_then(|(_, k)| k.cells.first().copied())
}

/// The cells of a named group ("wasd", "arrows", ...); `all` is the 126 key cells.
pub(crate) fn group_cells(group: &str, keys: &[Key]) -> Vec<usize> {
    let by_names = |pred: &dyn Fn(&str) -> bool| -> Vec<usize> {
        keys.iter().filter(|k| pred(&k.name)).flat_map(|k| k.cells.iter().copied()).collect()
    };
    match group {
        "all" => (0..KEY_CELLS).collect(),
        "wasd" => by_names(&|n| matches!(n, "w" | "a" | "s" | "d")),
        "arrows" => by_names(&|n| matches!(n, "up" | "down" | "left" | "right")),
        "fkeys" => by_names(&|n| n.strip_prefix('f').and_then(|d| d.parse::<u8>().ok()).is_some_and(|d| (1..=12).contains(&d))),
        "numrow" => by_names(&|n| n.len() == 1 && n.as_bytes().first().is_some_and(u8::is_ascii_digit)),
        "letters" => by_names(&|n| n.len() == 1 && n.as_bytes().first().is_some_and(u8::is_ascii_lowercase)),
        "numpad" => by_names(&|n| n.starts_with("num")),
        _ => Vec::new(),
    }
}

/// A selection as the page keeps it: sorted, unique, key cells only.
pub(super) fn tidy(cells: impl IntoIterator<Item = usize>) -> Vec<usize> {
    cells.into_iter().filter(|c| *c < KEY_CELLS).collect::<BTreeSet<_>>().into_iter().collect()
}
