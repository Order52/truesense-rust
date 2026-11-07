//! Colour maths: parsing and normalising colours, HSV, blending.
//!
//! A part of [`super`] (the Lighting page's view-model); it shares the parent's names.
#![forbid(unsafe_code)]

use super::*;

// ---- colour -------------------------------------------------------------------------------------

pub(super) type Rgb = [u8; 3];

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

pub(super) fn parse_rgb(hex: &str) -> Option<Rgb> {
    let h = norm_hex(hex)?;
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

pub(super) fn to_hex(c: Rgb) -> String {
    format!("{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

pub(super) fn clamp_byte(v: f64) -> u8 {
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
pub(super) fn hue_ramp(t: f64) -> Rgb {
    hsv_to_rgb(Hsv { h: t.rem_euclid(1.0) * 360.0, s: 1.0, v: 1.0 })
}

pub(super) fn lerp_rgb(a: Rgb, b: Rgb, t: f64) -> Rgb {
    [0, 1, 2].map(|i| clamp_byte(f64::from(a[i]) + (f64::from(b[i]) - f64::from(a[i])) * t))
}

/// Perceived brightness 0..255 (the weights of the web window).
pub(super) fn luminance(c: Rgb) -> f64 {
    0.3 * f64::from(c[0]) + 0.59 * f64::from(c[1]) + 0.11 * f64::from(c[2])
}

/// Whether a key of this colour wants dark text (`true`) or light text.
pub(crate) fn wants_dark_text(hex: &str) -> bool {
    parse_rgb(hex).is_some_and(|c| luminance(c) > 150.0)
}

/// Gradient over keys by their x position: the colour at each x between two ends (all equal x get
/// the start colour).
pub(super) fn gradient_at(xs: &[f64], from: Rgb, to: Rgb) -> Vec<String> {
    let lo = xs.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = xs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    xs.iter().map(|&x| to_hex(lerp_rgb(from, to, if hi > lo { (x - lo) / (hi - lo) } else { 0.0 }))).collect()
}

/// Rainbow over keys by x, over 85 % of the hue circle so that both ends differ.
pub(super) fn rainbow_at(xs: &[f64]) -> Vec<String> {
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
pub(super) fn cells_from(value: Option<&Value>) -> Vec<String> {
    let list = value.and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    (0..CELLS)
        .map(|i| list.get(i).and_then(Value::as_str).and_then(norm_hex).unwrap_or_else(|| "000000".to_owned()))
        .collect()
}
