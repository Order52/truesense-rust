//! What the Lighting page decides, without Qt: the key picture and its geometry, hit testing and
//! paint strokes, colour maths, the effect catalogue with the rules of its options, the keyboard
//! preview model, the rear bar rules and the per-power-mode binding edit state (Changed, Undo).
//!
//! Everything is a pure function of its arguments (no I/O, no clock), reached from QML as
//! `Bus.vm("lighting", verb, args)`; the verbs are listed in [`call`]. The page holds the state
//! objects (selection, binding edit state) and passes them in and out, so the logic stays testable
//! and the page can be destroyed and rebuilt freely.
//!
//! Cells: the keyboard takes 128 colours, `cell = column * 6 + row` (22 columns, 6 rows); only
//! cells 0..125 are keys, 126 and 127 stay black (`docs/PROTOCOL.md`, `keymap`).
#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use ts_domain::keyboard;

use crate::json::{at, f64_at, i64_at, list_at, str_at};

/// Colours the keyboard takes in one upload.
pub(crate) const CELLS: usize = 128;
/// Cells that are keys.
pub(crate) const KEY_CELLS: usize = 126;
/// Rows of the keyboard picture (the cell numbering).
const ROWS: i64 = 6;
/// The stock cyan of the keyboard and the default effect colour.
pub(crate) const STOCK: &str = "00aec7";
/// The colours offered as swatches (the web window's palette).
pub const PALETTE: [&str; 16] = [
    "ff3b30", "ff9500", "ffcc00", "34c759", "00c7be", "32ade6", "007aff", "5856d6", "af52de", "ff2d55", "ffffff",
    "c7c7cc", "8e8e93", "3a3a3c", "00aec7", "000000",
];

// ---- colour -------------------------------------------------------------------------------------

type Rgb = [u8; 3];

/// Normalises "#FF3B30", "ff3b30" or "f30" to "ff3b30"; anything else is `None`.
pub fn norm_hex(text: &str) -> Option<String> {
    let t = text.trim().trim_start_matches('#').to_ascii_lowercase();
    if !t.is_ascii() || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    match t.len() {
        3 => Some(t.chars().flat_map(|c| [c, c]).collect()),
        6 => Some(t),
        _ => None,
    }
}

fn parse_rgb(hex: &str) -> Option<Rgb> {
    let h = norm_hex(hex)?;
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

fn to_hex(c: Rgb) -> String {
    format!("{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

fn clamp_byte(v: f64) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

/// Hue (degrees), saturation and value (0..1) of a colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hsv {
    /// 0..360.
    pub h: f64,
    /// 0..1.
    pub s: f64,
    /// 0..1.
    pub v: f64,
}

/// RGB to HSV; grey has hue 0.
pub fn rgb_to_hsv(c: Rgb) -> Hsv {
    let [r, g, b] = c.map(|v| f64::from(v) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    Hsv { h, s: if max == 0.0 { 0.0 } else { d / max }, v: max }
}

/// HSV to RGB; hue wraps, saturation and value are clamped.
pub(crate) fn hsv_to_rgb(hsv: Hsv) -> Rgb {
    let h = hsv.h.rem_euclid(360.0) / 60.0;
    let (s, v) = (hsv.s.clamp(0.0, 1.0), hsv.v.clamp(0.0, 1.0));
    let c = v * s;
    let x = c * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match h as u8 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [clamp_byte((r + m) * 255.0), clamp_byte((g + m) * 255.0), clamp_byte((b + m) * 255.0)]
}

/// The full-saturation hue ramp for `t` in 0..1 (wraps), the same ramp as `keymap.hsv`.
fn hue_ramp(t: f64) -> Rgb {
    hsv_to_rgb(Hsv { h: t.rem_euclid(1.0) * 360.0, s: 1.0, v: 1.0 })
}

fn lerp_rgb(a: Rgb, b: Rgb, t: f64) -> Rgb {
    [0, 1, 2].map(|i| clamp_byte(f64::from(a[i]) + (f64::from(b[i]) - f64::from(a[i])) * t))
}

/// Perceived brightness 0..255 (the weights of the web window).
fn luminance(c: Rgb) -> f64 {
    0.3 * f64::from(c[0]) + 0.59 * f64::from(c[1]) + 0.11 * f64::from(c[2])
}

/// Whether a key of this colour wants dark text (`true`) or light text.
pub(crate) fn wants_dark_text(hex: &str) -> bool {
    parse_rgb(hex).is_some_and(|c| luminance(c) > 150.0)
}

/// Gradient over keys by their x position: the colour at each x between two ends (all equal x get
/// the start colour).
fn gradient_at(xs: &[f64], from: Rgb, to: Rgb) -> Vec<String> {
    let lo = xs.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = xs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    xs.iter().map(|&x| to_hex(lerp_rgb(from, to, if hi > lo { (x - lo) / (hi - lo) } else { 0.0 }))).collect()
}

/// Rainbow over keys by x, over 85 % of the hue circle so that both ends differ.
fn rainbow_at(xs: &[f64]) -> Vec<String> {
    let lo = xs.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = xs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    xs.iter().map(|&x| to_hex(hue_ramp(0.85 * if hi > lo { (x - lo) / (hi - lo) } else { 0.0 }))).collect()
}

/// The stock look: 126 cyan keys and two dark spare cells.
pub fn stock_cells() -> Vec<String> {
    let mut cells = vec![STOCK.to_owned(); KEY_CELLS];
    cells.extend(["000000".to_owned(), "000000".to_owned()]);
    cells
}

/// 128 colours from whatever the page holds: missing, short or malformed entries become black, so
/// a half-read answer never leaves the picture with holes.
fn cells_from(value: Option<&Value>) -> Vec<String> {
    let list = value.and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    (0..CELLS)
        .map(|i| list.get(i).and_then(Value::as_str).and_then(norm_hex).unwrap_or_else(|| "000000".to_owned()))
        .collect()
}

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

/// Main block rows, top to bottom: key name and width. The function row is slightly narrower
/// (0.93) so that it ends flush with the 15 unit wide rows below it.
fn main_rows() -> Vec<Vec<(String, f64)>> {
    let one = |names: &str| -> Vec<(String, f64)> { names.chars().map(|c| (c.to_string(), 1.0)).collect() };
    let mut f_row = vec![("esc".to_owned(), 1.0)];
    f_row.extend((1..=12).map(|i| (format!("f{i}"), 0.93)));
    f_row.extend(["prtsc", "ins", "del"].map(|n| (n.to_owned(), 0.93)));
    let named = |list: &[(&str, f64)]| -> Vec<(String, f64)> {
        list.iter().map(|(n, w)| ((*n).to_owned(), *w)).collect()
    };
    let cat = |parts: Vec<Vec<(String, f64)>>| parts.concat();
    vec![
        f_row,
        cat(vec![one("`1234567890"), named(&[("minus", 1.0), ("=", 1.0), ("backspace", 2.0)])]),
        cat(vec![
            named(&[("tab", 1.5)]),
            one("qwertyuiop"),
            named(&[("[", 1.0), ("]", 1.0), ("\\", 1.5)]),
        ]),
        cat(vec![
            named(&[("caps", 1.75)]),
            one("asdfghjkl"),
            named(&[(";", 1.0), ("'", 1.0), ("return", 2.25)]),
        ]),
        cat(vec![
            named(&[("lshift", 2.25)]),
            one("zxcvbnm"),
            named(&[(",", 1.0), (".", 1.0), ("/", 1.0), ("rshift", 1.75), ("up", 1.0)]),
        ]),
        named(&[
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
        ]),
    ]
}

/// The numpad: name, column 0..3, width, height.
const PAD: [&[(&str, f64, f64, f64)]; 6] = [
    &[("backward", 0.0, 1.0, 1.0), ("playpause", 1.0, 1.0, 1.0), ("forward", 2.0, 1.0, 1.0), ("power", 3.0, 1.0, 1.0)],
    &[("predator", 0.0, 1.0, 1.0), ("numlock", 1.0, 1.0, 1.0), ("num/", 2.0, 1.0, 1.0), ("*", 3.0, 1.0, 1.0)],
    &[("num7", 0.0, 1.0, 1.0), ("num8", 1.0, 1.0, 1.0), ("num9", 2.0, 1.0, 1.0), ("-", 3.0, 1.0, 1.0)],
    &[("num4", 0.0, 1.0, 1.0), ("num5", 1.0, 1.0, 1.0), ("num6", 2.0, 1.0, 1.0), ("+", 3.0, 1.0, 1.0)],
    &[("num1", 0.0, 1.0, 1.0), ("num2", 1.0, 1.0, 1.0), ("num3", 2.0, 1.0, 1.0), ("numenter", 3.0, 1.0, 2.0)],
    &[("num0", 0.0, 2.0, 1.0), ("num.", 2.0, 1.0, 1.0)],
];
/// Where the numpad starts, in units.
const PAD_X: f64 = 16.5;
/// Top edge of each key row (the function row sits close above the number row).
const ROW_Y: [f64; 6] = [0.0, 1.3, 2.3, 3.3, 4.3, 5.3];

/// Text on special keys; single characters show themselves in capitals.
fn key_label(name: &str) -> String {
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
    for (ri, row) in main_rows().iter().enumerate() {
        let mut x = 0.0;
        for (name, w) in row {
            add(name, x, ROW_Y.get(ri).copied().unwrap_or(0.0), *w, 1.0, &mut keys);
            x += w;
        }
    }
    for (ri, row) in PAD.iter().enumerate() {
        for (name, col, w, h) in *row {
            add(name, PAD_X + col, ROW_Y.get(ri).copied().unwrap_or(0.0), *w, *h, &mut keys);
        }
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
const INSET: f64 = 0.04;

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
fn segment_entry(a: (f64, f64), b: (f64, f64), min: (f64, f64), max: (f64, f64)) -> Option<f64> {
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
fn x_of_cell(keys: &[Key], cell: usize) -> f64 {
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
fn tidy(cells: impl IntoIterator<Item = usize>) -> Vec<usize> {
    cells.into_iter().filter(|c| *c < KEY_CELLS).collect::<BTreeSet<_>>().into_iter().collect()
}

// ---- painting -----------------------------------------------------------------------------------

/// Cells ordered left to right by their key's centre (ties by cell number), for gradients.
fn by_x(keys: &[Key], cells: &[usize]) -> Vec<usize> {
    let mut v = tidy(cells.iter().copied());
    v.sort_by(|a, b| x_of_cell(keys, *a).total_cmp(&x_of_cell(keys, *b)).then(a.cmp(b)));
    v
}

/// The tools that rewrite colours of several cells at once.
fn paint_tool(tool: &str, cells: &mut [String], keys: &[Key], selection: &[usize], args: &Value) -> Result<(), String> {
    let colour = str_at(args, "colour").and_then(norm_hex);
    // No selection means every key.
    let targets: Vec<usize> = if selection.is_empty() { (0..KEY_CELLS).collect() } else { tidy(selection.iter().copied()) };
    let set = |cells: &mut [String], list: &[usize], colours: &[String]| {
        for (c, hex) in list.iter().zip(colours) {
            if let Some(slot) = cells.get_mut(*c) {
                slot.clone_from(hex);
            }
        }
    };
    match tool {
        "fill" => {
            let hex = colour.ok_or("fill needs a colour")?;
            set(cells, &targets, &vec![hex; targets.len()]);
        }
        "clear" => set(cells, &targets, &vec!["000000".to_owned(); targets.len()]),
        "gradient" => {
            let from = str_at(args, "from").and_then(parse_rgb).ok_or("gradient needs a start colour")?;
            let to = str_at(args, "to").and_then(parse_rgb).ok_or("gradient needs an end colour")?;
            let ordered = by_x(keys, &targets);
            let xs: Vec<f64> = ordered.iter().map(|c| x_of_cell(keys, *c)).collect();
            set(cells, &ordered, &gradient_at(&xs, from, to));
        }
        "rainbow" => {
            let ordered = by_x(keys, &targets);
            let xs: Vec<f64> = ordered.iter().map(|c| x_of_cell(keys, *c)).collect();
            set(cells, &ordered, &rainbow_at(&xs));
        }
        other => return Err(format!("unknown lighting tool: {other}")),
    }
    Ok(())
}

// ---- effects ------------------------------------------------------------------------------------

/// One hardware effect of the keyboard and what its options mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effect {
    /// The name `keyboard effect` takes.
    pub id: &'static str,
    /// Icon file under `qml/icons`.
    pub icon: &'static str,
    /// The firmware uses the colour (wave and neon ignore it: the command sends black).
    pub colour: bool,
    /// The effect has a speed (a static colour has none).
    pub speed: bool,
    /// The effect travels, so a direction makes sense. This only enables the control: the byte is
    /// always sent, and the keyboard ignores it where it means nothing. [U: which effects honour
    /// it was never measured on the laptop.]
    pub direction: bool,
    /// Lowest brightness at which picking this effect shows anything (neon was invisible at 25 and
    /// shows at 50: `docs/verification-log.md`; the others need only a visible level).
    pub pick_floor: i64,
}

const fn fx(id: &'static str, icon: &'static str, colour: bool, speed: bool, direction: bool, pick_floor: i64) -> Effect {
    Effect { id, icon, colour, speed, direction, pick_floor }
}

/// The 13 effects in display order (`off` is the backlight switch, not a tile).
pub const EFFECTS: [Effect; 13] = [
    fx("static", "lighting/static", true, false, false, 10),
    fx("breathing", "lighting/breathing", true, true, false, 10),
    fx("wave", "lighting/wave", false, true, true, 10),
    fx("snake", "lighting/snake", true, true, true, 10),
    fx("ripple", "lighting/ripple", true, true, true, 10),
    fx("neon", "lighting/rainbow", false, true, true, 50),
    fx("raindrop", "lighting/rain", true, true, true, 10),
    fx("lightning", "lighting/lightning", true, true, false, 10),
    fx("spot", "lighting/spot", true, true, false, 10),
    fx("star", "lighting/stars", true, true, false, 10),
    fx("fireball", "lighting/fireball", true, true, true, 10),
    fx("snow", "lighting/snow", true, true, true, 10),
    fx("heartbeat", "lighting/heartbeat", true, true, false, 10),
];

/// Lowest and highest effect speed (1 slowest .. 12 fastest, the UI scale of `keyboard`).
const SPEED_RANGE: (i64, i64) = (keyboard::SPEED_MIN, keyboard::SPEED_MAX);
/// The keyboard's brightness range.
const KB_MAX: i64 = keyboard::MAX_BRIGHT;
/// The effect speed used when none is known.
const DEFAULT_SPEED: i64 = keyboard::DEFAULT_SPEED;

fn effect(id: &str) -> Option<&'static Effect> {
    EFFECTS.iter().find(|e| e.id == id)
}

fn effect_json(e: &Effect) -> Value {
    json!({"id": e.id, "icon": e.icon, "colour": e.colour, "speed": e.speed, "direction": e.direction})
}

/// The arguments of `keyboard effect` for picking `name` while the lights are as `current` says
/// (`colour`, `speed`, `brightness`, `direction`, all optional). Picking an effect while the
/// backlight is dark or dim raises the brightness to the effect's floor so that the pick is
/// visible; the brightness slider itself never does this (0 stays 0).
pub fn pick_effect(name: &str, current: &Value) -> Result<Value, String> {
    let e = effect(name).ok_or_else(|| format!("unknown keyboard effect: {name}"))?;
    let colour = str_at(current, "colour").and_then(norm_hex).unwrap_or_else(|| STOCK.to_owned());
    let speed = i64_at(current, "speed").filter(|s| (SPEED_RANGE.0..=SPEED_RANGE.1).contains(s)).unwrap_or(DEFAULT_SPEED);
    let level = i64_at(current, "brightness").unwrap_or(0).clamp(0, KB_MAX);
    let direction = i64_at(current, "direction").filter(|d| matches!(d, 0 | 1)).unwrap_or(0);
    Ok(json!({
        "name": e.id, "color": colour, "speed": speed,
        "brightness": level.max(e.pick_floor), "direction": direction,
    }))
}

/// The software animations of the key service (`keyboard fx`): id and whether it needs the colours
/// under it. Speeds are 1..10.
pub(crate) const FX_MODES: [&str; 18] = [
    "pulse", "wave", "cycle", "twinkle", "rainbow", "breathing", "scanner", "comet", "ripple", "rain", "matrix", "fire",
    "plasma", "aurora", "lightning", "confetti", "starlight", "mood",
];

/// The rear bar's effects: id, icon, whether it has a speed and whether it travels (a direction
/// only makes sense for the moving ones; [U] not measured, like the keyboard's table).
const REAR_EFFECTS: [(&str, &str, bool, bool); 5] = [
    ("static", "lighting/static", false, false),
    ("breathing", "lighting/breathing", true, false),
    ("neon", "lighting/rainbow", true, true),
    ("wave", "lighting/wave", true, true),
    ("twinkling", "lighting/stars", true, false),
];

/// The arguments of `rear set` for what the controls show. The static effect colours three parts
/// (`zones`), every other effect one colour. A brightness of 0 is "off" for the bar, so an effect
/// picked while it is off turns it back on at the last brightness (or 100); `turned_on` tells the
/// page to move its switch.
pub fn rear_request(state: &Value) -> Result<Value, String> {
    let name = str_at(state, "effect").unwrap_or("static");
    if !REAR_EFFECTS.iter().any(|(id, ..)| *id == name) {
        return Err(format!("unknown rear light effect: {name}"));
    }
    let colour = str_at(state, "colour").and_then(norm_hex).unwrap_or_else(|| STOCK.to_owned());
    let zones: Vec<String> = (0..3)
        .map(|i| {
            list_at(state, "zones")
                .and_then(|z| z.get(i))
                .and_then(Value::as_str)
                .and_then(norm_hex)
                .unwrap_or_else(|| colour.clone())
        })
        .collect();
    let speed = i64_at(state, "speed").unwrap_or(3).clamp(1, 5);
    let direction = i64_at(state, "direction").filter(|d| matches!(d, 0 | 1)).unwrap_or(1);
    let mut brightness = i64_at(state, "brightness").unwrap_or(100).clamp(0, 100);
    let turned_on = brightness == 0;
    if turned_on {
        brightness = i64_at(state, "last_brightness").filter(|b| *b > 0).unwrap_or(100).clamp(1, 100);
    }
    let mut args = json!({"effect": name, "speed": speed, "brightness": brightness, "direction": direction});
    if name == "static" {
        args["zones"] = json!(zones);
    } else {
        args["color"] = json!(colour);
    }
    Ok(json!({"args": args, "brightness": brightness, "turned_on": turned_on}))
}

// ---- the preview --------------------------------------------------------------------------------

/// Colours of the 128 cells that stand for the lights described by `look` (the `look` member of
/// `keyboard status`), with `cells` the per-key colours when known. `exact` is true when the
/// picture is the real thing (a still colour or the per-key state) and false for an
/// approximation of a moving effect, which the page says so.
pub fn preview(look: &Value, cells: Option<&Value>) -> Value {
    let mode = str_at(look, "mode").unwrap_or("");
    let level = i64_at(look, "brightness").unwrap_or(KB_MAX).clamp(0, KB_MAX);
    let dim = |hex: &str| -> String {
        if level == 0 {
            return "000000".to_owned();
        }
        let f = 0.35 + 0.65 * (level as f64 / KB_MAX as f64);
        parse_rgb(hex).map_or_else(|| "000000".to_owned(), |c| to_hex(c.map(|v| clamp_byte(f64::from(v) * f))))
    };
    let (kind, exact, base): (&str, bool, Vec<String>) = match mode {
        "perkey" => match cells.filter(|c| c.is_array()) {
            Some(c) => ("perkey", true, cells_from(Some(c))),
            None => ("unknown", false, vec!["000000".to_owned(); CELLS]),
        },
        "off" => ("off", true, vec!["000000".to_owned(); CELLS]),
        "effect" => {
            let name = str_at(look, "name").unwrap_or("static");
            let colour = str_at(look, "color").and_then(norm_hex).unwrap_or_else(|| STOCK.to_owned());
            let uses_colour = effect(name).is_none_or(|e| e.colour);
            let still = name == "static";
            let pattern: Vec<String> = (0..CELLS)
                .map(|cell| {
                    if !uses_colour {
                        // The firmware ignores the colour: a hue ramp across the columns.
                        return to_hex(hue_ramp((cell as f64 / ROWS as f64) / 22.0));
                    }
                    if still || name == "breathing" {
                        return colour.clone();
                    }
                    // Sparse bright keys on a dim ground: a still frame of a moving effect.
                    if (cell * 7) % 5 == 0 { colour.clone() } else { dim_to(&colour, 0.3) }
                })
                .collect();
            ("effect", still, pattern)
        }
        _ => ("unknown", false, vec!["000000".to_owned(); CELLS]),
    };
    let shown: Vec<String> = base
        .iter()
        .enumerate()
        .map(|(i, hex)| if i >= KEY_CELLS { "000000".to_owned() } else { dim(hex) })
        .collect();
    json!({"cells": shown, "kind": kind, "exact": exact})
}

fn dim_to(hex: &str, f: f64) -> String {
    parse_rgb(hex).map_or_else(|| "000000".to_owned(), |c| to_hex(c.map(|v| clamp_byte(f64::from(v) * f))))
}

// ---- names and look summaries -------------------------------------------------------------------

/// Whether `name` is a usable look or design name (the rule of the web window and the stores).
pub fn valid_name(name: &str) -> bool {
    ts_foundation::names::is_valid(name)
}

/// Whether `name` is a usable per-key set name (no dots).
pub(crate) fn valid_set_name(name: &str) -> bool {
    ts_foundation::names::is_valid(name) && !name.contains('.')
}

/// A look split into the facts the page words in its own language: the keyboard part (`effect`
/// name, `perkey`, or a theme) and the rear part.
pub fn look_summary(look: &Value) -> Value {
    let keyboard = at(look, "keyboard").map(|k| {
        let mode = str_at(k, "mode").unwrap_or("");
        json!({
            "kind": if mode == "perkey" { "perkey" } else if mode == "off" { "off" } else { "effect" },
            "name": str_at(k, "name"), "brightness": i64_at(k, "brightness"), "speed": i64_at(k, "speed"),
        })
    });
    let rear = at(look, "rear").map(|r| {
        json!({"mode": str_at(r, "mode").or_else(|| str_at(r, "effect")), "brightness": i64_at(r, "brightness")})
    });
    let theme = at(look, "theme").map(|t| json!({"id": str_at(t, "id")}));
    json!({"keyboard": keyboard, "rear": rear, "theme": theme})
}

// ---- per power mode bindings --------------------------------------------------------------------

/// The five power modes, in the order of the strip.
pub const MODES: [&str; 5] = crate::limits::POWER_MODES;

/// What one power mode has: the saved look and curve (read-only here) and the look being edited.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ModeEntry {
    /// The look the daemon holds for the mode.
    pub saved: Option<String>,
    /// The fan curve bound to the mode (shown, never edited here: the Cooling page owns it).
    pub curve: Option<String>,
    /// The look as the person has chosen it, saved or not.
    pub draft: Option<String>,
    /// Earlier drafts, newest last, for Undo.
    pub steps: Vec<Option<String>>,
}

impl ModeEntry {
    /// The draft differs from what is saved.
    pub fn changed(&self) -> bool {
        self.draft != self.saved
    }
}

/// The edit state of the "Lighting for \[mode\]" strip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bindings {
    /// The power mode being edited.
    pub selected: String,
    /// The power mode the laptop is in now.
    pub current: Option<String>,
    /// One entry per mode.
    pub modes: BTreeMap<String, ModeEntry>,
}

impl Bindings {
    /// Reads the answer of `mode bindings`. Drafts that the person changed survive a reload
    /// (`previous`); drafts nobody touched follow what the daemon says now.
    pub fn load(result: &Value, previous: Option<&Self>) -> Self {
        let current = str_at(result, "mode").filter(|m| MODES.contains(m)).map(str::to_owned);
        let selected = previous
            .map(|p| p.selected.clone())
            .or_else(|| current.clone())
            .unwrap_or_else(|| "balanced".to_owned());
        let mut modes = BTreeMap::new();
        for mode in MODES {
            let saved = str_at(result, &format!("bindings.{mode}.look.name")).map(str::to_owned);
            let curve = str_at(result, &format!("bindings.{mode}.fan_curve.name")).map(str::to_owned);
            let old = previous.and_then(|p| p.modes.get(mode));
            let (draft, steps) = match old {
                Some(o) if o.changed() && o.draft != saved => (o.draft.clone(), o.steps.clone()),
                _ => (saved.clone(), Vec::new()),
            };
            modes.insert(mode.to_owned(), ModeEntry { saved, curve, draft, steps });
        }
        Self { selected, current, modes }
    }

    fn entry_mut(&mut self, mode: &str) -> Result<&mut ModeEntry, String> {
        self.modes.get_mut(mode).ok_or_else(|| format!("unknown power mode: {mode}"))
    }

    /// Chooses the look of `mode` (`None`: no look binding). Choosing what is already the draft is
    /// not a step.
    pub fn choose(&mut self, mode: &str, look: Option<String>) -> Result<(), String> {
        let entry = self.entry_mut(mode)?;
        if entry.draft != look {
            let before = entry.draft.take();
            entry.steps.push(before);
            entry.steps.truncate(40);
            entry.draft = look;
        }
        Ok(())
    }

    /// Takes back the last choice for `mode`; returns whether there was one.
    pub fn undo(&mut self, mode: &str) -> Result<bool, String> {
        let entry = self.entry_mut(mode)?;
        match entry.steps.pop() {
            Some(before) => {
                entry.draft = before;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Throws away every unsaved choice for `mode`.
    pub fn discard(&mut self, mode: &str) -> Result<(), String> {
        let entry = self.entry_mut(mode)?;
        entry.draft.clone_from(&entry.saved);
        entry.steps.clear();
        Ok(())
    }

    /// The bus commands that make the daemon match the draft of `mode`: bind a look, drop the look
    /// binding, or nothing when unchanged. The fan curve is never touched.
    pub fn plan(&self, mode: &str) -> Result<Vec<Value>, String> {
        let entry = self.modes.get(mode).ok_or_else(|| format!("unknown power mode: {mode}"))?;
        Ok(match (&entry.draft, &entry.saved) {
            (d, s) if d == s => Vec::new(),
            (Some(look), _) => vec![json!({"area": "mode", "verb": "bind", "args": {"mode": mode, "look": look}})],
            (None, Some(_)) => {
                vec![json!({"area": "mode", "verb": "unbind", "args": {"mode": mode, "what": "look"}})]
            }
            (None, None) => Vec::new(),
        })
    }

    /// For QML: the selection and one row per mode.
    pub fn view(&self) -> Value {
        let rows: Vec<Value> = MODES
            .iter()
            .filter_map(|mode| {
                let e = self.modes.get(*mode)?;
                Some(json!({
                    "mode": mode, "look": e.draft, "saved": e.saved, "curve": e.curve,
                    "changed": e.changed(), "can_undo": !e.steps.is_empty(), "current": self.current.as_deref() == Some(mode),
                }))
            })
            .collect();
        json!({"selected": self.selected, "current": self.current, "modes": rows})
    }
}

// ---- themes -------------------------------------------------------------------------------------

/// `#RRGGBB` (upper case, the catalogue's form) from user input, or `None`.
fn theme_colour(text: &str) -> Option<String> {
    norm_hex(text).map(|h| format!("#{}", h.to_ascii_uppercase()))
}

/// The look member a setting target (`keyboard.params.anchor`, `bar.speed`, `keyboard.dir`) writes:
/// the path inside the theme object.
fn target_path(target: &str) -> Option<Vec<String>> {
    let parts: Vec<&str> = target.split('.').collect();
    match parts.as_slice() {
        [light @ ("keyboard" | "bar" | "logo"), field @ ("speed" | "dir")] => Some(vec![(*light).to_owned(), (*field).to_owned()]),
        [light @ ("keyboard" | "bar" | "logo"), "params", id] if !id.is_empty() => {
            Some(vec![(*light).to_owned(), "params".to_owned(), (*id).to_owned()])
        }
        _ => None,
    }
}

fn path_get<'a>(theme: &'a Value, path: &[String]) -> Option<&'a Value> {
    path.iter().try_fold(theme, |v, key| v.get(key))
}

/// Writes `value` at `path` when the parent object exists (a target that does not fit its look is
/// ignored, as the engine does).
fn path_set(theme: &mut Value, path: &[String], value: Value) {
    let Some((last, parents)) = path.split_last() else { return };
    let mut node = theme;
    for key in parents {
        match node.get_mut(key) {
            Some(next) => node = next,
            None => return,
        }
    }
    if let Some(map) = node.as_object_mut() {
        map.insert(last.clone(), value);
    }
}

/// The controls of a theme's settings with the value each has now (read from the theme's looks):
/// `[{id, label, kind: number|choice|toggle|keys|key, min, max, step, unit, clock, options, value}]`.
/// A choice's value is the index of the option (what `theme apply` takes), a toggle's a boolean,
/// and a number in hours (unit `h`) is shown as a clock time (`clock`).
pub fn theme_settings(theme: &Value) -> Value {
    let list = list_at(theme, "settings").unwrap_or_default();
    let out: Vec<Value> = list
        .iter()
        .filter_map(|s| {
            let id = str_at(s, "id")?;
            let kind = str_at(s, "type")?;
            let current = str_at(s, "target").and_then(target_path).and_then(|p| path_get(theme, &p).cloned());
            let num = |v: &Option<Value>| v.as_ref().and_then(Value::as_f64);
            let mut entry = json!({"id": id, "label": str_at(s, "label").unwrap_or(id)});
            let (kind_out, value) = match kind {
                "choice" => {
                    let values: Vec<f64> =
                        list_at(s, "values").unwrap_or_default().iter().filter_map(Value::as_f64).collect();
                    entry["options"] = json!(list_at(s, "options").unwrap_or_default());
                    let idx = num(&current).and_then(|c| values.iter().position(|v| (*v - c).abs() < 1e-9)).unwrap_or(0);
                    ("choice", json!(idx))
                }
                "toggle" => {
                    let on = f64_at(s, "on").unwrap_or(1.0);
                    ("toggle", json!(num(&current).is_some_and(|c| (c - on).abs() < 1e-9)))
                }
                "keys" => ("keys", current.filter(Value::is_array).unwrap_or_else(|| json!([]))),
                "key" => {
                    let first = current.as_ref().and_then(|c| c.as_array().and_then(|a| a.first()).or(Some(c))).and_then(Value::as_str);
                    ("key", json!(first.unwrap_or("")))
                }
                "number" | "time" => {
                    let (min, max) = (f64_at(s, "min").unwrap_or(0.0), f64_at(s, "max").unwrap_or(100.0));
                    entry["min"] = json!(min);
                    entry["max"] = json!(max);
                    entry["step"] = json!(f64_at(s, "step").filter(|st| *st > 0.0).unwrap_or(1.0));
                    let unit = str_at(s, "unit").unwrap_or("");
                    entry["unit"] = json!(unit);
                    entry["clock"] = json!(kind == "time" || unit == "h");
                    ("number", json!(num(&current).unwrap_or(min).clamp(min, max.max(min))))
                }
                _ => return None,
            };
            entry["kind"] = json!(kind_out);
            entry["value"] = value;
            Some(entry)
        })
        .collect();
    json!(out)
}

/// Clock text for hours of the day (`6.5` is `06:30`).
pub fn clock_text(hours: f64) -> String {
    let minutes = (hours.rem_euclid(24.0) * 60.0).round() as i64 % (24 * 60);
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// The values a page holds for a theme: the colourway (`"custom"` or an index), the custom colours
/// and one value per setting id.
fn colourway_palette(theme: &Value, choice: &Value, custom: &[String]) -> Vec<String> {
    let ways = list_at(theme, "colourways").unwrap_or_default();
    let first: Vec<String> = ways
        .first()
        .and_then(|w| list_at(w, "palette"))
        .unwrap_or_default()
        .iter()
        .filter_map(|c| c.as_str().map(str::to_owned))
        .collect();
    if choice.as_str() == Some("custom") {
        // A slot without a valid colour keeps the first colourway's (the engine's rule).
        return first.iter().enumerate().map(|(i, c)| custom.get(i).cloned().unwrap_or_else(|| c.clone())).collect();
    }
    let idx = choice.as_u64().and_then(|i| usize::try_from(i).ok()).unwrap_or(0);
    ways.get(idx.min(ways.len().saturating_sub(1)))
        .and_then(|w| list_at(w, "palette"))
        .map(|p| p.iter().filter_map(|c| c.as_str().map(str::to_owned)).collect())
        .unwrap_or(first)
}

fn custom_colours(args: &Value) -> Vec<String> {
    list_at(args, "custom").unwrap_or_default().iter().map(|c| c.as_str().and_then(theme_colour).unwrap_or_default()).collect()
}

/// The arguments of `theme apply`: the colourway (an index or `custom` with its colours), the
/// settings by id, the speed (10..100 %) and the brightness (1..100 %). Values out of range are
/// clamped here so that the service never has to refuse a slider.
pub fn theme_request(args: &Value) -> Result<Value, String> {
    let name = str_at(args, "name").filter(|n| !n.is_empty()).ok_or("a theme needs a name")?;
    let mut out = json!({"name": name});
    match at(args, "colourway") {
        Some(Value::String(s)) if s == "custom" => {
            out["colourway"] = json!("custom");
            out["custom"] = json!(custom_colours(args).into_iter().filter(|c| !c.is_empty()).collect::<Vec<_>>());
        }
        Some(v) => {
            if let Some(i) = v.as_u64() {
                out["colourway"] = json!(i);
            }
        }
        None => {}
    }
    if let Some(params) = at(args, "values").and_then(Value::as_object) {
        out["params"] = Value::Object(params.clone());
    }
    if let Some(s) = i64_at(args, "speed") {
        out["speed"] = json!(s.clamp(10, 100));
    }
    if let Some(b) = i64_at(args, "brightness") {
        out["brightness"] = json!(b.clamp(1, 100));
    }
    Ok(out)
}

/// An id for a user theme from the name typed: lower case letters, digits, `-` and `_` (spaces
/// become `-`); `None` when nothing is left.
pub(crate) fn theme_id(name: &str) -> Option<String> {
    let id: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c == ' ' { '-' } else { c })
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'))
        .collect();
    (!id.is_empty() && id.len() <= 40).then_some(id)
}

/// The theme object `theme save` takes for "keep what I chose": a copy of `theme` (as `theme show`
/// gives it) under a new `id` and `name`, whose palette and first colourway are the colours chosen
/// and whose settings hold the chosen values. `top` and `suits` are dropped (a user theme has no
/// rank), and ids of `taken` (the built-in themes) are refused here with a message.
pub fn theme_for_save(args: &Value, taken: &[String]) -> Result<Value, String> {
    let mut theme = at(args, "theme").cloned().filter(Value::is_object).ok_or("no theme to save")?;
    let name = str_at(args, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or("give the theme a name")?;
    let id = theme_id(name).ok_or("the name needs letters or digits")?;
    if taken.contains(&id) {
        return Err(format!("'{id}' is the id of a built-in theme: choose another name"));
    }
    let palette = colourway_palette(&theme, at(args, "colourway").unwrap_or(&Value::Null), &custom_colours(args));
    let way_name = if at(args, "colourway").and_then(Value::as_str) == Some("custom") {
        "Mine".to_owned()
    } else {
        let idx = at(args, "colourway").and_then(Value::as_u64).and_then(|i| usize::try_from(i).ok()).unwrap_or(0);
        list_at(&theme, "colourways").and_then(|w| w.get(idx)).and_then(|w| str_at(w, "name")).unwrap_or("Mine").to_owned()
    };
    if let Some(values) = at(args, "values").and_then(Value::as_object) {
        let defs: Vec<Value> = list_at(&theme, "settings").unwrap_or_default().to_vec();
        for def in &defs {
            let (Some(sid), Some(path)) = (str_at(def, "id"), str_at(def, "target").and_then(target_path)) else { continue };
            let Some(v) = values.get(sid) else { continue };
            let written = match (str_at(def, "type"), v) {
                (Some("choice"), v) => {
                    let values_list: Vec<f64> = list_at(def, "values").unwrap_or_default().iter().filter_map(Value::as_f64).collect();
                    let i = v.as_f64().map_or(0, |x| x as i64).clamp(0, values_list.len() as i64 - 1);
                    usize::try_from(i).ok().and_then(|i| values_list.get(i).copied()).map(|x| json!(x))
                }
                (Some("toggle"), v) => {
                    let (on, off) = (f64_at(def, "on").unwrap_or(1.0), f64_at(def, "off").unwrap_or(0.0));
                    Some(json!(if v.as_bool().unwrap_or(false) { on } else { off }))
                }
                (Some("keys"), Value::Array(a)) => Some(Value::Array(a.clone())),
                (Some("key"), Value::String(k)) => Some(json!([k])),
                (Some("number" | "time"), v) => v.as_f64().map(|x| json!(x)),
                _ => None,
            };
            if let Some(w) = written {
                path_set(&mut theme, &path, w);
            }
        }
    }
    theme["id"] = json!(id);
    theme["name"] = json!(name);
    theme["palette"] = json!(palette);
    let mut ways: Vec<Value> = list_at(&theme, "colourways").unwrap_or_default().to_vec();
    ways.retain(|w| str_at(w, "name") != Some(way_name.as_str()));
    ways.insert(0, json!({"name": way_name, "palette": palette}));
    theme["colourways"] = json!(ways);
    if let Some(map) = theme.as_object_mut() {
        map.remove("top");
        map.remove("suits");
    }
    Ok(theme)
}

// ---- the verbs ----------------------------------------------------------------------------------

fn keys_arg(args: &Value) -> Vec<Key> {
    at(args, "keys").and_then(|k| serde_json::from_value(k.clone()).ok()).unwrap_or_default()
}

fn cells_arg(args: &Value, name: &str) -> Vec<usize> {
    list_at(args, name)
        .map(|l| l.iter().filter_map(Value::as_u64).filter_map(|c| usize::try_from(c).ok()).collect())
        .unwrap_or_default()
}

fn point(args: &Value, name: &str) -> Option<(f64, f64)> {
    Some((f64_at(args, &format!("{name}.x"))?, f64_at(args, &format!("{name}.y"))?))
}

fn bindings_arg(args: &Value) -> Result<Bindings, String> {
    at(args, "state")
        .and_then(|s| serde_json::from_value(s.clone()).ok())
        .ok_or_else(|| "the binding state is missing".to_owned())
}

fn state_out(b: &Bindings) -> Value {
    json!({"state": b, "view": b.view()})
}

/// The verbs of the page:
/// `effects`, `rear_effects`, `fx_modes`, `palette`, `stock_cells`, `norm_hex`, `hsv_to_hex`,
/// `hex_to_hsv`, `wants_dark_text`, `inks`, `neighbour`, `layout`, `hit`, `select`, `stroke`, `paint`, `tool`,
/// `pick_effect`, `rear_request`, `preview`, `look_summary`, `valid_name`, `theme_settings`, `theme_request`, `theme_for_save`, `clock_text`, and for the strip
/// `bindings_load`, `bindings_select`, `bindings_choose`, `bindings_undo`, `bindings_discard`,
/// `bindings_plan`.
///
/// # Errors
/// The text for an unknown verb or arguments that cannot be used.
pub fn call(verb: &str, args: &Value) -> Result<Value, String> {
    match verb {
        "effects" => Ok(json!(EFFECTS.iter().map(effect_json).collect::<Vec<_>>())),
        "rear_effects" => Ok(json!(
            REAR_EFFECTS
                .iter()
                .map(|(id, icon, speed, direction)| json!({"id": id, "icon": icon, "speed": speed, "direction": direction}))
                .collect::<Vec<_>>()
        )),
        "fx_modes" => Ok(json!(FX_MODES)),
        "palette" => Ok(json!(PALETTE)),
        "stock_cells" => Ok(json!(stock_cells())),
        "norm_hex" => Ok(json!(norm_hex(str_at(args, "s").unwrap_or("")))),
        "hsv_to_hex" => {
            let hsv = Hsv {
                h: f64_at(args, "h").unwrap_or(0.0),
                s: f64_at(args, "s").unwrap_or(0.0),
                v: f64_at(args, "v").unwrap_or(0.0),
            };
            Ok(json!(to_hex(hsv_to_rgb(hsv))))
        }
        "hex_to_hsv" => {
            let rgb = str_at(args, "hex").and_then(parse_rgb).ok_or("not a colour")?;
            let h = rgb_to_hsv(rgb);
            Ok(json!({"h": h.h, "s": h.s, "v": h.v}))
        }
        "inks" => Ok(json!(cells_from(at(args, "cells")).iter().map(|c| wants_dark_text(c)).collect::<Vec<_>>())),
        "neighbour" => {
            let cell = usize::try_from(i64_at(args, "cell").ok_or("neighbour needs a cell")?).map_err(|e| e.to_string())?;
            Ok(json!(neighbour(&keys_arg(args), cell, str_at(args, "dir").unwrap_or(""))))
        }
        "wants_dark_text" => Ok(json!(wants_dark_text(str_at(args, "hex").unwrap_or("")))),
        "layout" => {
            let layout = build_layout(&name_cells(args));
            serde_json::to_value(layout).map_err(|e| e.to_string())
        }
        "hit" => {
            let keys = keys_arg(args);
            let (x, y) = point(args, "at").ok_or("hit needs at.x and at.y")?;
            Ok(json!(key_at(&keys, x, y).map(|k| &k.cells)))
        }
        "select" => select(args),
        "stroke" => {
            let keys = keys_arg(args);
            let a = point(args, "from").ok_or("stroke needs from")?;
            let b = point(args, "to").unwrap_or(a);
            let touched: Vec<usize> = keys_on_segment(&keys, a, b).into_iter().flat_map(|k| k.cells.iter().copied()).collect();
            let mut cells = cells_from(at(args, "cells"));
            let mut changed = false;
            if let Some(hex) = str_at(args, "colour").and_then(norm_hex) {
                for c in &touched {
                    if let Some(slot) = cells.get_mut(*c)
                        && *slot != hex
                    {
                        slot.clone_from(&hex);
                        changed = true;
                    }
                }
            }
            Ok(json!({"touched": touched, "cells": cells, "changed": changed}))
        }
        "paint" => {
            // Paint a list of cells one colour.
            let hex = str_at(args, "colour").and_then(norm_hex).ok_or("paint needs a colour")?;
            let mut cells = cells_from(at(args, "cells"));
            for c in cells_arg(args, "targets") {
                if let Some(slot) = cells.get_mut(c) {
                    slot.clone_from(&hex);
                }
            }
            Ok(json!(cells))
        }
        "tool" => {
            let tool = str_at(args, "tool").ok_or("tool needs a name")?;
            let keys = keys_arg(args);
            let mut cells = cells_from(at(args, "cells"));
            paint_tool(tool, &mut cells, &keys, &cells_arg(args, "selection"), args)?;
            Ok(json!(cells))
        }
        "pick_effect" => pick_effect(str_at(args, "name").unwrap_or(""), at(args, "current").unwrap_or(&Value::Null)),
        "rear_request" => rear_request(args),
        "preview" => Ok(preview(at(args, "look").unwrap_or(&Value::Null), at(args, "cells"))),
        "theme_settings" => Ok(theme_settings(at(args, "theme").unwrap_or(&Value::Null))),
        "theme_request" => theme_request(args),
        "theme_for_save" => {
            let taken: Vec<String> = list_at(args, "taken").unwrap_or_default().iter().filter_map(|t| t.as_str().map(str::to_owned)).collect();
            theme_for_save(args, &taken)
        }
        "clock_text" => Ok(json!(clock_text(f64_at(args, "hours").unwrap_or(0.0)))),
        "look_summary" => Ok(look_summary(args)),
        "valid_name" => {
            let name = str_at(args, "name").unwrap_or("");
            Ok(json!(if str_at(args, "kind") == Some("set") { valid_set_name(name) } else { valid_name(name) }))
        }
        "bindings_load" => {
            let previous: Option<Bindings> = at(args, "state").and_then(|s| serde_json::from_value(s.clone()).ok());
            let result = at(args, "result").unwrap_or(&Value::Null);
            Ok(state_out(&Bindings::load(result, previous.as_ref())))
        }
        "bindings_select" => {
            let mut b = bindings_arg(args)?;
            let mode = str_at(args, "mode").ok_or("select needs a mode")?;
            if !MODES.contains(&mode) {
                return Err(format!("unknown power mode: {mode}"));
            }
            mode.clone_into(&mut b.selected);
            Ok(state_out(&b))
        }
        "bindings_choose" => {
            let mut b = bindings_arg(args)?;
            let mode = str_at(args, "mode").unwrap_or(&b.selected.clone()).to_owned();
            b.choose(&mode, str_at(args, "look").map(str::to_owned))?;
            Ok(state_out(&b))
        }
        "bindings_undo" => {
            let mut b = bindings_arg(args)?;
            let mode = str_at(args, "mode").unwrap_or(&b.selected.clone()).to_owned();
            b.undo(&mode)?;
            Ok(state_out(&b))
        }
        "bindings_discard" => {
            let mut b = bindings_arg(args)?;
            let mode = str_at(args, "mode").unwrap_or(&b.selected.clone()).to_owned();
            b.discard(&mode)?;
            Ok(state_out(&b))
        }
        "bindings_plan" => {
            let b = bindings_arg(args)?;
            let mode = str_at(args, "mode").unwrap_or(&b.selected.clone()).to_owned();
            Ok(json!(b.plan(&mode)?))
        }
        _ => Err(format!("unknown lighting verb: {verb}")),
    }
}

/// `select {op, keys, selection, ...}`: the selection after `op` = `box` (`from`, `to`, `add`),
/// `group` (`group`), `toggle` (`cell`), `invert`, `all`, `none`.
fn select(args: &Value) -> Result<Value, String> {
    let keys = keys_arg(args);
    let current = tidy(cells_arg(args, "selection"));
    let out = match str_at(args, "op").unwrap_or("") {
        "none" => Vec::new(),
        "all" => (0..KEY_CELLS).collect(),
        "invert" => (0..KEY_CELLS).filter(|c| !current.contains(c)).collect(),
        "group" => tidy(group_cells(str_at(args, "group").unwrap_or(""), &keys)),
        "toggle" => {
            let cell = usize::try_from(i64_at(args, "cell").ok_or("toggle needs a cell")?).map_err(|e| e.to_string())?;
            // A key can own several cells: toggle them all together.
            let owner: Vec<usize> = keys.iter().find(|k| k.cells.contains(&cell)).map_or_else(|| vec![cell], |k| k.cells.clone());
            if owner.iter().all(|c| current.contains(c)) {
                current.into_iter().filter(|c| !owner.contains(c)).collect()
            } else {
                tidy(current.into_iter().chain(owner))
            }
        }
        "box" => {
            let a = point(args, "from").ok_or("box needs from")?;
            let b = point(args, "to").unwrap_or(a);
            let inside = cells_in_box(&keys, a, b);
            if args.get("add").and_then(Value::as_bool).unwrap_or(false) {
                tidy(current.into_iter().chain(inside))
            } else {
                tidy(inside)
            }
        }
        other => return Err(format!("unknown selection operation: {other}")),
    };
    Ok(json!(out))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn real_layout() -> Layout {
        let v: Value = serde_json::from_str(include_str!("../../tests/lighting-keys.json")).unwrap();
        build_layout(&name_cells(&v))
    }

    fn cells_all(hex: &str) -> Vec<String> {
        vec![hex.to_owned(); CELLS]
    }

    // ---- colour ----

    #[test]
    fn hex_input_is_normalised_and_junk_refused() {
        assert_eq!(norm_hex("#FF3B30").as_deref(), Some("ff3b30"));
        assert_eq!(norm_hex("  f30 ").as_deref(), Some("ff3300"));
        for bad in ["", "#", "12345", "1234567", "gggggg", "ff 3b30", "ünï", "#12345g", "-12345"] {
            assert_eq!(norm_hex(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn hsv_round_trips_primary_and_grey_colours() {
        for (hex, h, s, v) in [("ff0000", 0.0, 1.0, 1.0), ("00ff00", 120.0, 1.0, 1.0), ("0000ff", 240.0, 1.0, 1.0), ("808080", 0.0, 0.0, 128.0 / 255.0)] {
            let got = rgb_to_hsv(parse_rgb(hex).unwrap());
            assert!((got.h - h).abs() < 1e-9 && (got.s - s).abs() < 1e-9 && (got.v - v).abs() < 1e-9, "{hex}: {got:?}");
            assert_eq!(to_hex(hsv_to_rgb(got)), hex);
        }
        // Every hue survives a round trip within one step of rounding.
        for deg in (0..360).step_by(7) {
            let c = hsv_to_rgb(Hsv { h: f64::from(deg), s: 0.8, v: 0.9 });
            let back = hsv_to_rgb(rgb_to_hsv(c));
            for i in 0..3 {
                assert!(c[i].abs_diff(back[i]) <= 1, "hue {deg}");
            }
        }
    }

    #[test]
    fn hsv_wraps_hue_and_clamps_the_rest() {
        assert_eq!(hsv_to_rgb(Hsv { h: 360.0, s: 1.0, v: 1.0 }), [255, 0, 0]);
        assert_eq!(hsv_to_rgb(Hsv { h: -120.0, s: 1.0, v: 1.0 }), [0, 0, 255]);
        assert_eq!(hsv_to_rgb(Hsv { h: 10.0, s: 5.0, v: -1.0 }), [0, 0, 0]);
        assert_eq!(hsv_to_rgb(Hsv { h: 10.0, s: -5.0, v: 2.0 }), [255, 255, 255]);
    }

    #[test]
    fn hue_ramp_matches_the_keymap_ramp_at_the_ends_and_wraps() {
        assert_eq!(hue_ramp(0.0), [255, 0, 0]);
        assert_eq!(hue_ramp(1.0), [255, 0, 0]);
        assert_eq!(hue_ramp(1.0 / 3.0), [0, 255, 0]);
        assert_eq!(hue_ramp(2.0 / 3.0), [0, 0, 255]);
        assert_eq!(hue_ramp(-1.0 / 3.0), [0, 0, 255]);
    }

    #[test]
    fn text_colour_follows_luminance() {
        assert!(wants_dark_text("ffffff"));
        assert!(wants_dark_text("ffcc00"));
        assert!(!wants_dark_text("000000"));
        assert!(!wants_dark_text("007aff"));
        assert!(!wants_dark_text("nonsense"));
    }

    #[test]
    fn gradient_and_rainbow_by_x() {
        let xs = [0.0, 5.0, 10.0];
        assert_eq!(gradient_at(&xs, [0, 0, 0], [200, 100, 50]), ["000000", "643219", "c86432"]);
        assert_eq!(gradient_at(&[3.0, 3.0], [9, 9, 9], [200, 0, 0]), ["090909", "090909"], "equal x: the start colour");
        let r = rainbow_at(&xs);
        assert_eq!(r[0], "ff0000");
        assert_ne!(r[2], r[0], "85 % of the circle: the ends differ");
        assert!(gradient_at(&[], [0; 3], [0; 3]).is_empty());
    }

    #[test]
    fn stock_cells_are_cyan_with_two_dark_spares() {
        let s = stock_cells();
        assert_eq!((s.len(), s[0].as_str(), s[125].as_str(), s[126].as_str(), s[127].as_str()), (128, "00aec7", "00aec7", "000000", "000000"));
    }

    #[test]
    fn cells_from_pads_and_sanitises() {
        let c = cells_from(Some(&json!(["FF0000", "zz", 5, "#00ff00"])));
        assert_eq!((c.len(), c[0].as_str(), c[1].as_str(), c[2].as_str(), c[3].as_str(), c[127].as_str()), (128, "ff0000", "000000", "000000", "00ff00", "000000"));
        assert_eq!(cells_from(None).len(), 128);
        assert_eq!(cells_from(Some(&json!({"not": "a list"}))).len(), 128);
    }

    // ---- the picture ----

    #[test]
    fn the_real_layout_draws_every_cell_once() {
        let l = real_layout();
        let mut seen = BTreeSet::new();
        for k in &l.keys {
            for c in &k.cells {
                assert!(seen.insert(*c), "cell {c} drawn twice ({})", k.name);
            }
        }
        assert_eq!(seen.len(), 103, "105 names, two of them aliases: 103 distinct cells");
        assert_eq!(l.height, 6.4, "no extra row: the picture knows every key");
        for name in ["esc", "space", "return", "windows", "numenter", "predator", "up"] {
            assert!(l.keys.iter().any(|k| k.name == name), "{name}");
        }
        assert!(!l.keys.iter().any(|k| k.name == "enter" || k.name == "win"), "aliases are drawn once");
    }

    #[test]
    fn rows_end_flush_and_nothing_overlaps() {
        let l = real_layout();
        for (i, a) in l.keys.iter().enumerate() {
            for b in &l.keys[i + 1..] {
                let overlap = a.x + a.w - 1e-9 > b.x && b.x + b.w - 1e-9 > a.x && a.y + a.h - 1e-9 > b.y && b.y + b.h - 1e-9 > a.y;
                assert!(!overlap, "{} overlaps {}", a.name, b.name);
            }
        }
        let right = |n: &str| l.keys.iter().find(|k| k.name == n).map(|k| k.x + k.w).unwrap();
        assert!((right("backspace") - 15.0).abs() < 1e-9);
        assert!((right("return") - 15.0).abs() < 1e-9);
        assert!((right("up") - 15.0).abs() < 1e-9);
        assert!((right("right") - 16.0).abs() < 1e-9);
        assert!((right("num.") - 20.5 + 1.0).abs() < 1e-9 || right("numenter") <= l.width + 1e-9);
        assert!(l.keys.iter().all(|k| k.x + k.w <= l.width + 1e-9));
    }

    #[test]
    fn unknown_keys_get_an_extra_row_and_junk_is_skipped() {
        let v = json!({"keys": {"zzz": [{"cell": 100}], "a": [20], "bad": "x", "far": [999], "mix": [1, {"cell": 2}]}});
        let names = name_cells(&v);
        assert_eq!(names.get("zzz"), Some(&vec![100]));
        assert!(!names.contains_key("bad") && !names.contains_key("far"));
        let l = build_layout(&names);
        let z = l.keys.iter().find(|k| k.name == "zzz").unwrap();
        assert_eq!((z.y, l.height), (6.4, 7.5));
        assert!(l.keys.iter().any(|k| k.name == "a" && k.y == 3.3));
        assert_eq!(build_layout(&BTreeMap::new()).keys.len(), 0);
    }

    #[test]
    fn labels() {
        assert_eq!(key_label("a"), "A");
        assert_eq!(key_label("lshift"), "Shift");
        assert_eq!(key_label("num7"), "7");
        assert_eq!(key_label("space"), "");
        assert_eq!(key_label("f5"), "F5");
        assert_eq!(key_label("zzzzz"), "Zzz");
    }

    #[test]
    fn hit_testing_uses_the_inset_rectangles() {
        let l = real_layout();
        let a = l.keys.iter().find(|k| k.name == "a").unwrap();
        assert_eq!(key_at(&l.keys, a.x + 0.5, a.y + 0.5).map(|k| k.name.as_str()), Some("a"));
        assert!(key_at(&l.keys, a.x + 0.01, a.y + 0.5).is_none(), "inside the gap between keys");
        assert!(key_at(&l.keys, -3.0, -3.0).is_none());
        assert!(key_at(&l.keys, f64::NAN, 1.0).is_none());
        let space = l.keys.iter().find(|k| k.name == "space").unwrap();
        assert_eq!(key_at(&l.keys, space.x + 2.0, space.y + 0.5).map(|k| k.name.as_str()), Some("space"));
    }

    #[test]
    fn a_box_selects_every_key_it_touches_in_any_corner_order() {
        let l = real_layout();
        let cells = cells_in_box(&l.keys, (2.0, 3.5), (0.2, 2.4));
        let names: BTreeSet<&str> = l.keys.iter().filter(|k| k.cells.iter().any(|c| cells.contains(c))).map(|k| k.name.as_str()).collect();
        assert!(["tab", "q", "caps", "a"].iter().all(|n| names.contains(n)), "{names:?}");
        assert!(!names.contains("w") || l.keys.iter().find(|k| k.name == "w").is_some_and(|w| w.x < 1.5));
        assert_eq!(cells_in_box(&l.keys, (50.0, 50.0), (60.0, 60.0)), Vec::<usize>::new());
    }

    #[test]
    fn a_fast_drag_paints_every_key_on_the_line_in_order() {
        let l = real_layout();
        let row = l.keys.iter().filter(|k| k.y == 3.3 && k.x < 15.0).collect::<Vec<_>>();
        let y = 3.3 + 0.5;
        // From the left edge of the home row to the right edge in one jump.
        let hits = keys_on_segment(&l.keys, (0.1, y), (14.9, y));
        assert_eq!(hits.len(), row.len(), "every home row key, none skipped");
        assert!(hits.windows(2).all(|w| w[0].x < w[1].x), "ordered along the drag");
        // The same, dragged right to left, reverses the order.
        let back = keys_on_segment(&l.keys, (14.9, y), (0.1, y));
        assert!(back.windows(2).all(|w| w[0].x > w[1].x));
        // A click (no movement) is one key; a click in a gap is none.
        let a = l.keys.iter().find(|k| k.name == "a").unwrap();
        assert_eq!(keys_on_segment(&l.keys, (a.x + 0.5, y), (a.x + 0.5, y)).len(), 1);
        assert!(keys_on_segment(&l.keys, (a.x + 0.01, y), (a.x + 0.01, y)).is_empty());
        // A diagonal crossing three rows.
        let diag = keys_on_segment(&l.keys, (3.8, 1.8), (3.8, 5.8));
        assert_eq!(diag.len(), 5, "{:?}", diag.iter().map(|k| &k.name).collect::<Vec<_>>());
        assert!(keys_on_segment(&l.keys, (f64::NAN, 0.0), (1.0, 1.0)).is_empty());
    }

    #[test]
    fn arrow_navigation_moves_to_the_nearest_key_in_that_direction() {
        let l = real_layout();
        let cell = |n: &str| l.keys.iter().find(|k| k.name == n).unwrap().cells[0];
        assert_eq!(neighbour(&l.keys, cell("g"), "right"), Some(cell("h")));
        assert_eq!(neighbour(&l.keys, cell("g"), "left"), Some(cell("f")));
        assert_eq!(neighbour(&l.keys, cell("g"), "up"), Some(cell("t")));
        assert!(["b", "v"].iter().any(|n| neighbour(&l.keys, cell("g"), "down") == Some(cell(n))));
        assert_eq!(neighbour(&l.keys, cell("esc"), "left"), None, "no key to the left of the corner");
        assert_eq!(neighbour(&l.keys, cell("g"), "sideways"), None);
        assert_eq!(neighbour(&l.keys, 500, "left"), None);
        assert_eq!(call("neighbour", &json!({"keys": serde_json::to_value(&l.keys).unwrap(), "cell": cell("g"), "dir": "right"})).unwrap(), json!(cell("h")));
        assert_eq!(call("inks", &json!({"cells": ["ffffff", "000000"]})).unwrap()[0], json!(true));
    }

    #[test]
    fn groups_pick_the_documented_keys() {
        let l = real_layout();
        assert_eq!(group_cells("all", &l.keys).len(), 126);
        assert_eq!(group_cells("wasd", &l.keys).len(), 4);
        assert_eq!(group_cells("arrows", &l.keys).len(), 4);
        assert_eq!(group_cells("fkeys", &l.keys).len(), 12);
        assert_eq!(group_cells("numrow", &l.keys).len(), 10);
        assert_eq!(group_cells("letters", &l.keys).len(), 26);
        assert_eq!(group_cells("numpad", &l.keys).len(), 14, "num0..9, num/, num., numenter, numlock");
        assert!(group_cells("nope", &l.keys).is_empty());
    }

    // ---- painting and selection through the verbs ----

    fn layout_json() -> Value {
        serde_json::to_value(real_layout()).unwrap()
    }

    #[test]
    fn stroke_paints_the_crossed_keys_and_reports_whether_anything_changed() {
        let layout = layout_json();
        let args = json!({"keys": layout["keys"], "from": {"x": 0.1, "y": 3.8}, "to": {"x": 14.9, "y": 3.8}, "colour": "FF0000", "cells": cells_all("000000")});
        let r = call("stroke", &args).unwrap();
        assert_eq!(r["touched"].as_array().unwrap().len(), 13, "caps a s d f g h j k l ; ' return");
        assert_eq!(r["changed"], true);
        let painted = r["cells"].as_array().unwrap().iter().filter(|c| *c == "ff0000").count();
        assert_eq!(painted, 13);
        // Painting the same colour again changes nothing: no upload needed.
        let again = call("stroke", &json!({"keys": layout["keys"], "from": {"x": 0.1, "y": 3.8}, "to": {"x": 14.9, "y": 3.8}, "colour": "ff0000", "cells": r["cells"]})).unwrap();
        assert_eq!(again["changed"], false);
        // No colour: only the keys are reported (the eyedropper and hover).
        let dry = call("stroke", &json!({"keys": layout["keys"], "from": {"x": 0.1, "y": 3.8}, "to": {"x": 0.1, "y": 3.8}})).unwrap();
        assert_eq!((dry["changed"].clone(), dry["touched"].as_array().unwrap().len()), (json!(false), 1));
    }

    #[test]
    fn tools_fill_gradient_rainbow_clear_and_their_selections() {
        let layout = layout_json();
        let base = |extra: Value| {
            let mut a = json!({"keys": layout["keys"], "cells": cells_all("111111")});
            a.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            a
        };
        let all_fill = call("tool", &base(json!({"tool": "fill", "colour": "abcdef"}))).unwrap();
        let v = all_fill.as_array().unwrap();
        assert_eq!((v[0].as_str(), v[125].as_str(), v[126].as_str(), v[127].as_str()), (Some("abcdef"), Some("abcdef"), Some("111111"), Some("111111")), "spare cells untouched");
        let sel = json!([0, 1, 2]);
        let part = call("tool", &base(json!({"tool": "clear", "selection": sel}))).unwrap();
        assert_eq!(part.as_array().unwrap().iter().filter(|c| *c == "000000").count(), 3);
        // A gradient runs across the selected keys by x.
        let wasd: Vec<usize> = call("select", &json!({"op": "group", "group": "wasd", "keys": layout["keys"]})).unwrap().as_array().unwrap().iter().map(|c| c.as_u64().unwrap() as usize).collect();
        let g = call("tool", &base(json!({"tool": "gradient", "from": "000000", "to": "ffffff", "selection": wasd}))).unwrap();
        let g = g.as_array().unwrap();
        let shade = |name: &str| {
            let cell = real_layout().keys.iter().find(|k| k.name == name).unwrap().cells[0];
            u8::from_str_radix(&g[cell].as_str().unwrap()[0..2], 16).unwrap()
        };
        assert_eq!(shade("a"), 0, "leftmost key gets the start");
        assert!(shade("s") < shade("d"), "left to right");
        assert_eq!(shade("d"), 255, "wasd: a is leftmost and d rightmost");
        let rb = call("tool", &base(json!({"tool": "rainbow", "selection": [0, 6, 12]}))).unwrap();
        assert_eq!(rb.as_array().unwrap().iter().filter(|c| *c != "111111").count(), 3);
        for bad in [json!({"tool": "nope"}), json!({"tool": "fill"}), json!({"tool": "gradient"}), json!({})] {
            assert!(call("tool", &base(bad)).is_err());
        }
    }

    #[test]
    fn selection_operations() {
        let layout = layout_json();
        let sel = |args: Value| -> Vec<u64> {
            let mut a = json!({"keys": layout["keys"]});
            a.as_object_mut().unwrap().extend(args.as_object().unwrap().clone());
            call("select", &a).unwrap().as_array().unwrap().iter().map(|c| c.as_u64().unwrap()).collect()
        };
        assert_eq!(sel(json!({"op": "none"})), Vec::<u64>::new());
        assert_eq!(sel(json!({"op": "all"})).len(), 126);
        assert_eq!(sel(json!({"op": "invert", "selection": [0, 1]})).len(), 124);
        assert_eq!(sel(json!({"op": "toggle", "cell": 5, "selection": [1, 2]})), vec![1, 2, 5]);
        assert_eq!(sel(json!({"op": "toggle", "cell": 5, "selection": [1, 5]})), vec![1]);
        assert_eq!(sel(json!({"op": "toggle", "cell": 500})), Vec::<u64>::new(), "an out of range cell is no key");
        let boxed = sel(json!({"op": "box", "from": {"x": 0.1, "y": 3.5}, "to": {"x": 0.4, "y": 3.9}}));
        assert_eq!(boxed.len(), 1);
        let added = sel(json!({"op": "box", "add": true, "selection": [100], "from": {"x": 0.1, "y": 3.5}, "to": {"x": 0.4, "y": 3.9}}));
        assert_eq!(added.len(), 2);
        assert!(call("select", &json!({"op": "wat"})).is_err());
    }

    // ---- effects ----

    #[test]
    fn the_catalogue_has_thirteen_effects_with_the_hardware_rules() {
        assert_eq!(EFFECTS.len(), 13);
        let ids: BTreeSet<&str> = EFFECTS.iter().map(|e| e.id).collect();
        assert_eq!(ids.len(), 13);
        for e in EFFECTS {
            assert!(e.icon.starts_with("lighting/"));
        }
        // keyboard.py NO_COLOR = off, wave, neon; static has no speed.
        let no_colour: Vec<&str> = EFFECTS.iter().filter(|e| !e.colour).map(|e| e.id).collect();
        assert_eq!(no_colour, ["wave", "neon"]);
        assert!(!effect("static").unwrap().speed);
        assert!(effect("breathing").unwrap().speed);
        assert_eq!(effect("neon").unwrap().pick_floor, 50);
        assert!(!effect("static").unwrap().direction && effect("wave").unwrap().direction);
        assert_eq!(call("effects", &json!({})).unwrap().as_array().unwrap().len(), 13);
    }

    #[test]
    fn picking_an_effect_keeps_the_options_and_makes_it_visible() {
        let r = pick_effect("wave", &json!({"colour": "FF0000", "speed": 9, "brightness": 30, "direction": 1})).unwrap();
        assert_eq!(r, json!({"name": "wave", "color": "ff0000", "speed": 9, "brightness": 30, "direction": 1}));
        let dark = pick_effect("snake", &json!({"brightness": 0})).unwrap();
        assert_eq!((dark["brightness"].clone(), dark["color"].clone(), dark["speed"].clone()), (json!(10), json!("00aec7"), json!(5)));
        assert_eq!(pick_effect("neon", &json!({"brightness": 25})).unwrap()["brightness"], 50, "neon needs 50 to show");
        assert_eq!(pick_effect("neon", &json!({"brightness": 50})).unwrap()["brightness"], 50);
        // Junk in the current state falls back to defaults and stays in the hardware's ranges.
        let junk = pick_effect("static", &json!({"colour": "zz", "speed": 99, "brightness": 999, "direction": 7})).unwrap();
        assert_eq!(junk, json!({"name": "static", "color": "00aec7", "speed": 5, "brightness": 50, "direction": 0}));
        assert!(pick_effect("operatingmode", &json!({})).is_err());
        assert!(call("pick_effect", &json!({"name": "nope"})).is_err());
    }

    #[test]
    fn rear_requests_follow_the_zone_and_brightness_rules() {
        let s = rear_request(&json!({"effect": "static", "colour": "112233", "zones": ["ff0000", "00ff00", "0000ff"], "speed": 3, "brightness": 70})).unwrap();
        assert_eq!(s["args"], json!({"effect": "static", "speed": 3, "brightness": 70, "direction": 1, "zones": ["ff0000", "00ff00", "0000ff"]}));
        assert_eq!(s["turned_on"], false);
        let w = rear_request(&json!({"effect": "wave", "colour": "#AABBCC", "speed": 9, "brightness": 400, "direction": 0})).unwrap();
        assert_eq!(w["args"], json!({"effect": "wave", "speed": 5, "brightness": 100, "direction": 0, "color": "aabbcc"}));
        // Missing zones copy the single colour.
        let z = rear_request(&json!({"effect": "static", "colour": "010203", "zones": [null, "ffffff"]})).unwrap();
        assert_eq!(z["args"]["zones"], json!(["010203", "ffffff", "010203"]));
        // Brightness 0 means off: picking an effect turns the bar back on.
        let on = rear_request(&json!({"effect": "breathing", "brightness": 0, "last_brightness": 40})).unwrap();
        assert_eq!((on["brightness"].clone(), on["turned_on"].clone()), (json!(40), json!(true)));
        assert_eq!(rear_request(&json!({"brightness": 0})).unwrap()["brightness"], json!(100));
        assert!(rear_request(&json!({"effect": "strobe"})).is_err());
    }

    // ---- preview ----

    #[test]
    fn preview_shows_the_current_look() {
        let still = preview(&json!({"mode": "effect", "name": "static", "color": "ff0000", "brightness": 50}), None);
        assert_eq!((still["kind"].clone(), still["exact"].clone()), (json!("effect"), json!(true)));
        let c = still["cells"].as_array().unwrap();
        assert_eq!((c.len(), c[0].as_str(), c[126].as_str()), (128, Some("ff0000"), Some("000000")));
        // Dim at low brightness but not black; black at 0.
        let low = preview(&json!({"mode": "effect", "name": "static", "color": "ffffff", "brightness": 5}), None);
        let first = low["cells"][0].as_str().unwrap();
        assert!(first != "ffffff" && first != "000000", "{first}");
        let zero = preview(&json!({"mode": "effect", "name": "static", "color": "ffffff", "brightness": 0}), None);
        assert!(zero["cells"].as_array().unwrap().iter().all(|c| c == "000000"));
        // A colour-less effect draws the hue ramp, approximately.
        let neon = preview(&json!({"mode": "effect", "name": "neon", "color": "ff0000", "brightness": 50}), None);
        assert_eq!(neon["exact"], false);
        assert_ne!(neon["cells"][0], neon["cells"][60]);
        let spot = preview(&json!({"mode": "effect", "name": "spot", "color": "00ff00", "brightness": 50}), None);
        assert_eq!(spot["exact"], false);
        // The per-key state is exact when the cells are known and unknown otherwise.
        let mut known = cells_all("123456");
        known[3] = "ff00ff".into();
        let pk = preview(&json!({"mode": "perkey", "brightness": 50}), Some(&json!(known)));
        assert_eq!((pk["kind"].clone(), pk["exact"].clone(), pk["cells"][3].clone()), (json!("perkey"), json!(true), json!("ff00ff")));
        assert_eq!(preview(&json!({"mode": "perkey"}), None)["kind"], "unknown");
        for look in [json!({}), json!(null), json!({"mode": 5}), json!({"mode": "mystery"})] {
            assert_eq!(preview(&look, None)["kind"], "unknown");
        }
        assert_eq!(preview(&json!({"mode": "off"}), None)["kind"], "off");
    }

    // ---- names and summaries ----

    #[test]
    fn name_rules() {
        for ok in ["a", "late-night", "my.look_2", &"x".repeat(40)] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", " ", "a b", "ä", "../x", "a/b", &"x".repeat(41)] {
            assert!(!valid_name(bad), "{bad:?}");
        }
        assert!(valid_set_name("my_set-1") && !valid_set_name("my.set"));
        assert_eq!(call("valid_name", &json!({"name": "a.b", "kind": "set"})).unwrap(), json!(false));
    }

    #[test]
    fn look_summary_splits_the_parts() {
        let look = json!({"keyboard": {"mode": "effect", "name": "wave", "brightness": 20, "speed": 4}, "rear": {"mode": "static", "brightness": 80}});
        let s = look_summary(&look);
        assert_eq!(s["keyboard"]["name"], "wave");
        assert_eq!(s["rear"]["mode"], "static");
        assert!(s["theme"].is_null());
        let pk = look_summary(&json!({"keyboard": {"mode": "perkey"}, "rear": null}));
        assert_eq!((pk["keyboard"]["kind"].clone(), pk["rear"].clone()), (json!("perkey"), Value::Null));
        assert!(look_summary(&json!({}))["keyboard"].is_null());
    }

    // ---- bindings ----

    fn answer(binds: Value) -> Value {
        json!({"mode": "turbo", "bindings": binds})
    }

    fn loaded() -> Bindings {
        Bindings::load(
            &answer(json!({
                "turbo": {"look": {"name": "red", "data": {}}, "fan_curve": {"name": "loud", "data": {}}},
                "eco": {"look": {"name": "calm", "data": {}}, "fan_curve": null},
            })),
            None,
        )
    }

    #[test]
    fn loading_reads_looks_curves_and_selects_the_current_mode() {
        let b = loaded();
        assert_eq!((b.selected.as_str(), b.current.as_deref()), ("turbo", Some("turbo")));
        assert_eq!((b.modes["turbo"].saved.as_deref(), b.modes["turbo"].curve.as_deref()), (Some("red"), Some("loud")));
        assert_eq!((b.modes["eco"].saved.as_deref(), b.modes["eco"].curve.as_deref()), (Some("calm"), None));
        assert_eq!(b.modes["quiet"], ModeEntry::default());
        assert!(b.modes.values().all(|e| !e.changed()));
        assert_eq!(Bindings::load(&json!({"mode": "warp"}), None).selected, "balanced");
        assert_eq!(Bindings::load(&Value::Null, None).modes.len(), 5);
    }

    #[test]
    fn choosing_marks_changed_and_undo_walks_back() {
        let mut b = loaded();
        b.choose("eco", Some("night".into())).unwrap();
        assert!(b.modes["eco"].changed());
        b.choose("eco", Some("night".into())).unwrap();
        assert_eq!(b.modes["eco"].steps.len(), 1, "choosing the same thing is not a step");
        b.choose("eco", None).unwrap();
        assert!(b.undo("eco").unwrap());
        assert_eq!(b.modes["eco"].draft.as_deref(), Some("night"));
        assert!(b.undo("eco").unwrap());
        assert_eq!(b.modes["eco"].draft.as_deref(), Some("calm"));
        assert!(!b.modes["eco"].changed(), "back at the saved look: not changed");
        assert!(!b.undo("eco").unwrap());
        assert!(b.choose("warp", None).is_err() && b.undo("warp").is_err() && b.discard("warp").is_err());
        // The history is bounded.
        for i in 0..100 {
            b.choose("quiet", Some(format!("l{i}"))).unwrap();
        }
        assert_eq!(b.modes["quiet"].steps.len(), 40);
    }

    #[test]
    fn discard_restores_the_saved_look() {
        let mut b = loaded();
        b.choose("turbo", None).unwrap();
        b.choose("turbo", Some("x".into())).unwrap();
        b.discard("turbo").unwrap();
        assert_eq!((b.modes["turbo"].draft.as_deref(), b.modes["turbo"].steps.len()), (Some("red"), 0));
    }

    #[test]
    fn the_save_plan_binds_unbinds_or_does_nothing() {
        let mut b = loaded();
        assert!(b.plan("turbo").unwrap().is_empty());
        b.choose("turbo", Some("blue".into())).unwrap();
        assert_eq!(b.plan("turbo").unwrap(), vec![json!({"area": "mode", "verb": "bind", "args": {"mode": "turbo", "look": "blue"}})]);
        b.choose("eco", None).unwrap();
        assert_eq!(b.plan("eco").unwrap(), vec![json!({"area": "mode", "verb": "unbind", "args": {"mode": "eco", "what": "look"}})]);
        b.choose("quiet", Some("a".into())).unwrap();
        b.choose("quiet", None).unwrap();
        assert!(b.plan("quiet").unwrap().is_empty(), "nothing bound and nothing chosen");
        assert!(b.plan("warp").is_err());
    }

    #[test]
    fn a_reload_keeps_unsaved_choices_and_follows_saved_ones() {
        let mut b = loaded();
        b.choose("eco", Some("night".into())).unwrap();
        // The daemon now says turbo has another look (set from the CLI); eco's draft is still being edited.
        let fresh = answer(json!({"turbo": {"look": {"name": "green"}}, "eco": {"look": {"name": "calm"}}}));
        let r = Bindings::load(&fresh, Some(&b));
        assert_eq!(r.modes["eco"].draft.as_deref(), Some("night"));
        assert_eq!(r.modes["eco"].steps.len(), 1);
        assert_eq!(r.modes["turbo"].draft.as_deref(), Some("green"), "untouched drafts follow the daemon");
        assert_eq!(r.selected, "turbo");
        // After saving, the daemon answers with the new look and the badge goes away.
        let saved = Bindings::load(&answer(json!({"eco": {"look": {"name": "night"}}})), Some(&r));
        assert!(!saved.modes["eco"].changed() && saved.modes["eco"].steps.is_empty());
    }

    #[test]
    fn the_bindings_verbs_round_trip_the_state_through_json() {
        let first = call("bindings_load", &json!({"result": answer(json!({"eco": {"look": {"name": "calm"}, "fan_curve": {"name": "quiet"}}}))})).unwrap();
        let state = first["state"].clone();
        let sel = call("bindings_select", &json!({"state": state, "mode": "eco"})).unwrap();
        assert_eq!(sel["view"]["selected"], "eco");
        let ch = call("bindings_choose", &json!({"state": sel["state"], "look": "night"})).unwrap();
        let eco = &ch["view"]["modes"][0];
        assert_eq!((eco["mode"].clone(), eco["changed"].clone(), eco["can_undo"].clone(), eco["curve"].clone()), (json!("eco"), json!(true), json!(true), json!("quiet")));
        let plan = call("bindings_plan", &json!({"state": ch["state"]})).unwrap();
        assert_eq!(plan[0]["args"]["look"], "night");
        let undone = call("bindings_undo", &json!({"state": ch["state"], "mode": "eco"})).unwrap();
        assert_eq!(undone["view"]["modes"][0]["changed"], false);
        let discarded = call("bindings_discard", &json!({"state": ch["state"]})).unwrap();
        assert_eq!(discarded["view"]["modes"][0]["look"], "calm");
        assert!(call("bindings_choose", &json!({"look": "x"})).is_err(), "no state");
        assert!(call("bindings_select", &json!({"state": ch["state"], "mode": "warp"})).is_err());
    }

    // ---- themes ----

    fn sample_theme() -> Value {
        json!({
            "id": "predator", "name": "Predator", "top": 1, "mood": "m", "suits": ["x"],
            "colourways": [{"name": "Predator", "palette": ["#00E5FF", "#0090FF"]}, {"name": "Crimson", "palette": ["#FF1A3C", "#FF0080"]}],
            "custom": ["a", "b"], "palette": ["#00E5FF", "#0090FF"],
            "keyboard": {"effect": "beacon", "params": {"anchor": 2, "interval": 4, "wide": 1}},
            "bar": {"hw": "ripple", "colors": [0, 1, 0], "speed": 2},
            "logo": {"hw": "static", "colors": [0]},
            "settings": [
                {"id": "anchor", "label": "Start", "type": "choice", "target": "keyboard.params.anchor", "options": ["WASD", "Arrows", "Centre"], "values": [0, 1, 2]},
                {"id": "interval", "label": "Every", "type": "number", "target": "keyboard.params.interval", "min": 1, "max": 9, "step": 1, "unit": "s"},
                {"id": "wide", "label": "Wide", "type": "toggle", "target": "keyboard.params.wide", "on": 1, "off": 0},
                {"id": "barspeed", "label": "Bar", "type": "number", "target": "bar.speed", "min": 0, "max": 5, "step": 1},
                {"id": "wake", "label": "Wake", "type": "number", "target": "keyboard.params.wake", "min": 0, "max": 24, "step": 0.5, "unit": "h"},
                {"id": "keys", "label": "Keys", "type": "keys", "target": "keyboard.params.keys"},
                {"id": "nope", "label": "Odd", "type": "mystery", "target": "keyboard.params.x"}
            ]
        })
    }

    #[test]
    fn theme_settings_read_the_values_the_looks_hold() {
        let s = theme_settings(&sample_theme());
        let s = s.as_array().unwrap();
        assert_eq!(s.len(), 6, "the mystery type is skipped");
        assert_eq!((s[0]["kind"].clone(), s[0]["value"].clone()), (json!("choice"), json!(2)), "the index of 2 in values");
        assert_eq!((s[1]["value"].clone(), s[1]["max"].clone(), s[1]["clock"].clone()), (json!(4.0), json!(9.0), json!(false)));
        assert_eq!((s[2]["kind"].clone(), s[2]["value"].clone()), (json!("toggle"), json!(true)));
        assert_eq!(s[3]["value"], json!(2.0), "a bar speed target");
        assert_eq!((s[4]["clock"].clone(), s[4]["value"].clone()), (json!(true), json!(0.0)), "missing: the minimum");
        assert_eq!((s[5]["kind"].clone(), s[5]["value"].clone()), (json!("keys"), json!([])));
        assert!(theme_settings(&Value::Null).as_array().unwrap().is_empty());
        assert_eq!(clock_text(6.5), "06:30");
        assert_eq!(clock_text(24.0), "00:00");
        assert_eq!(clock_text(-1.0), "23:00");
    }

    #[test]
    fn theme_requests_clamp_and_carry_the_colours() {
        let r = theme_request(&json!({"name": "predator", "colourway": 1, "values": {"anchor": 1}, "speed": 500, "brightness": 0})).unwrap();
        assert_eq!(r, json!({"name": "predator", "colourway": 1, "params": {"anchor": 1}, "speed": 100, "brightness": 1}));
        let c = theme_request(&json!({"name": "x", "colourway": "custom", "custom": ["abc", "zz", "#00ff00"]})).unwrap();
        assert_eq!(c["custom"], json!(["#AABBCC", "#00FF00"]));
        assert_eq!(c["colourway"], "custom");
        assert_eq!(theme_request(&json!({"name": "x", "speed": 3})).unwrap()["speed"], json!(10));
        assert!(theme_request(&json!({})).is_err());
    }

    #[test]
    fn theme_ids_come_from_names() {
        assert_eq!(theme_id("My Night 2").as_deref(), Some("my-night-2"));
        assert_eq!(theme_id("  ÄÖ  "), None);
        assert_eq!(theme_id("a_b-c").as_deref(), Some("a_b-c"));
        assert_eq!(theme_id(&"x".repeat(41)), None);
    }

    #[test]
    fn saving_a_theme_keeps_the_colours_and_values_chosen() {
        let args = json!({"theme": sample_theme(), "name": "Night Wave", "colourway": 1, "values": {"anchor": 0, "interval": 7, "wide": false, "barspeed": 4, "keys": ["w", "a"]}});
        let t = theme_for_save(&args, &["predator".to_owned()]).unwrap();
        assert_eq!((t["id"].clone(), t["name"].clone()), (json!("night-wave"), json!("Night Wave")));
        assert_eq!(t["palette"], json!(["#FF1A3C", "#FF0080"]));
        assert_eq!(t["colourways"][0], json!({"name": "Crimson", "palette": ["#FF1A3C", "#FF0080"]}));
        assert_eq!(t["colourways"].as_array().unwrap().len(), 2, "no duplicate of the chosen way");
        assert_eq!(t["keyboard"]["params"]["anchor"], json!(0.0));
        assert_eq!(t["keyboard"]["params"]["interval"], json!(7.0));
        assert_eq!(t["keyboard"]["params"]["wide"], json!(0.0));
        assert_eq!(t["bar"]["speed"], json!(4.0));
        assert_eq!(t["keyboard"]["params"]["keys"], json!(["w", "a"]), "a target the look lacks is not invented");
        assert!(t.get("top").is_none() && t.get("suits").is_none());
        // Custom colours: the unset slot keeps the first colourway's.
        let c = theme_for_save(&json!({"theme": sample_theme(), "name": "Mine2", "colourway": "custom", "custom": ["#112233"]}), &[]).unwrap();
        assert_eq!(c["palette"], json!(["#112233", "#0090FF"]));
        assert_eq!(c["colourways"][0]["name"], "Mine");
        // Refusals say why.
        assert!(theme_for_save(&json!({"theme": sample_theme(), "name": "Predator"}), &["predator".to_owned()]).unwrap_err().contains("built-in"));
        assert!(theme_for_save(&json!({"theme": sample_theme(), "name": ""}), &[]).is_err());
        assert!(theme_for_save(&json!({"name": "x"}), &[]).is_err());
        assert!(theme_for_save(&json!({"theme": sample_theme(), "name": "!!"}), &[]).is_err());
    }

    // ---- the dispatcher ----

    #[test]
    fn dispatch_reaches_the_page_and_unknown_verbs_say_so() {
        assert_eq!(crate::vm::dispatch("lighting", "norm_hex", &json!({"s": "#ABC"})), Ok(json!("aabbcc")));
        assert!(crate::vm::dispatch("lighting", "nope", &json!({})).unwrap_err().contains("nope"));
        assert_eq!(call("hsv_to_hex", &json!({"h": 120, "s": 1, "v": 1})).unwrap(), json!("00ff00"));
        assert!(call("hex_to_hsv", &json!({"hex": "x"})).is_err());
        assert_eq!(call("palette", &json!({})).unwrap().as_array().unwrap().len(), 16);
        assert_eq!(call("fx_modes", &json!({})).unwrap().as_array().unwrap().len(), 18);
        assert_eq!(call("rear_effects", &json!({})).unwrap().as_array().unwrap().len(), 5);
        assert_eq!(call("stock_cells", &json!({})).unwrap().as_array().unwrap().len(), 128);
        assert!(call("layout", &json!({"keys": 5})).unwrap()["keys"].as_array().unwrap().is_empty());
    }

    #[test]
    fn hostile_arguments_never_panic() {
        let hostile = [json!(null), json!([]), json!("x"), json!({"keys": [{"cells": "x"}], "from": {"x": "a"}, "cells": 5, "state": 3, "selection": "x", "cell": -4, "targets": [-1, 1e30]})];
        for verb in ["layout", "hit", "select", "stroke", "paint", "tool", "pick_effect", "rear_request", "preview", "look_summary", "valid_name", "bindings_load", "bindings_select", "bindings_choose", "bindings_undo", "bindings_discard", "bindings_plan", "theme_settings", "theme_request", "theme_for_save", "clock_text", "hsv_to_hex", "hex_to_hsv", "norm_hex"] {
            for args in &hostile {
                let _ = call(verb, args);
            }
        }
    }
}
