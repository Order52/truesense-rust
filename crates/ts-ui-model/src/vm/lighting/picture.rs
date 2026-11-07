//! The key picture: the layout of the keyboard, cells, geometry and hit testing.
//!
//! A part of [`super`] (the Lighting page's view-model); it shares the parent's names.
#![forbid(unsafe_code)]

use ts_api::api::key_rects;

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
    /// The short text on the key (empty for a wide key like the space bar).
    #[serde(default)]
    pub label: String,
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

/// Text on special keys; single characters show themselves in capitals.
pub(super) fn key_label(name: &str) -> String {
    let fixed = match name {
        "backspace" => "⌫",
        "return" | "numenter" => "↵",
        "lshift" | "rshift" => "Shift",
        "caps" => "Caps",
        "windows" => "Win",
        "options" => "Opt",
        "lalt" | "ralt" => "Alt",
        "ctrl" | "rctrl" => "Ctrl",
        "space" => "",
        "up" => "↑",
        "down" => "↓",
        "left" => "←",
        "right" => "→",
        "prtsc" => "PrtSc",
        "predator" => "P",
        "power" => "⏻",
        "playpause" => "▶",
        "backward" => "⏮",
        "forward" => "⏭",
        "numlock" => "Num",
        "num/" => "/",
        "num." => ".",
        "minus" => "-",
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
    if !fixed.is_empty() || name == "space" {
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
    let mut add = |name: &str, x: f64, y: f64, w: f64, h: f64, keys: &mut Vec<Key>| {
        if let Some(cells) = names.get(name)
            && !cells.iter().any(|c| seen.contains(c))
        {
            seen.extend(cells.iter().copied());
            keys.push(Key { name: name.to_owned(), cells: cells.clone(), x, y, w, h, label: key_label(name) });
        }
    };
    for r in key_rects() {
        add(r.name, r.x, r.y, r.w, r.h, &mut keys);
    }
    let extra: Vec<(&String, &Vec<usize>)> = names
        .iter()
        .filter(|(n, cs)| !cs.iter().any(|c| seen.contains(c)) && n.as_str() != "enter" && n.as_str() != "win")
        .collect();
    let mut height = 6.4;
    for (i, (name, cells)) in extra.into_iter().enumerate() {
        if cells.iter().any(|c| seen.contains(c)) {
            continue;
        }
        seen.extend(cells.iter().copied());
        keys.push(Key {
            name: name.clone(),
            cells: cells.clone(),
            x: i as f64,
            y: 6.4,
            w: 1.0,
            h: 1.0,
            label: key_label(name),
        });
        height = 7.5;
    }
    Layout { keys, width: 20.5, height }
}

/// Keys are inset so that neighbours do not touch; a hit must land inside the inset rectangle.
pub(super) const INSET: f64 = 0.04;

/// The key under a point (layout units).
pub(crate) fn key_at(keys: &[Key], x: f64, y: f64) -> Option<&Key> {
    keys.iter().find(|k| {
        x >= k.x + INSET && x <= k.x + k.w - INSET && y >= k.y + INSET && y <= k.y + k.h - INSET
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
            let min = (k.x + INSET, k.y + INSET);
            let max = (k.x + k.w - INSET, k.y + k.h - INSET);
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
