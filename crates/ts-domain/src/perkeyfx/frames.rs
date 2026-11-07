//! The frames of the per-key animations: pure functions of time, nothing else.
//!
//! The keyboard's hardware effects cover the whole keyboard; a different animation on chosen
//! keys has to be drawn frame by frame. [`render`] gives the 128 colours of one frame from the
//! time since the animation started, the base colours and the chosen keys. It keeps no state
//! between frames (even the "random" numbers are a hash of the cell and the time step), so a
//! frame is reproducible, can be tested without a keyboard, and the runner can skip a frame that
//! equals the last one.
//!
//! Every expression keeps the order of operations of the Python edition: its frames are the
//! reference, and a different order of additions would change the last bit of a colour now and
//! then. The functions of `f64` are those of the system's C library, as in Python.
#![forbid(unsafe_code)]

use std::f64::consts::PI;

use super::sweep;
use crate::keymap::{self, COLUMNS, Color, LIT_CELLS, NUM_CELLS, ROWS};

/// The animations in display order: the name the settings carry and the label shown to the user.
/// The first 18 are the earlier editions'; the sweeps after them are this edition's.
pub const MODES: [(&str, &str); 20] = [
    ("pulse", "Pulse"),
    ("wave", "Wave"),
    ("cycle", "Colour cycle"),
    ("twinkle", "Twinkle"),
    ("rainbow", "Rainbow sweep"),
    ("breathing", "Breathing (base colours)"),
    ("scanner", "Scanner"),
    ("comet", "Comet"),
    ("ripple", "Ripples"),
    ("rain", "Rain"),
    ("matrix", "Matrix rain"),
    ("fire", "Fire"),
    ("plasma", "Plasma"),
    ("aurora", "Aurora"),
    ("lightning", "Lightning"),
    ("confetti", "Confetti"),
    ("starlight", "Starlight"),
    ("mood", "Mood lamp"),
    ("wave-clockwise", "Clockwise sweep"),
    ("wave-counter-clockwise", "Counter-clockwise sweep"),
];

/// The slowest animation speed.
pub const SPEED_MIN: i64 = 1;
/// The fastest animation speed.
pub const SPEED_MAX: i64 = 10;

/// The animation names in display order (without `off`).
pub fn mode_names() -> Vec<&'static str> {
    MODES.iter().map(|(n, _)| *n).collect()
}

/// Whether `name` is an animation (`off` is not one).
pub fn is_mode(name: &str) -> bool {
    MODES.iter().any(|(n, _)| *n == name)
}

const WHITE: Color = [255, 255, 255];

/// A stateless pseudo-random number in `0..1` from two or three integers. The arithmetic wraps
/// modulo 2^32 exactly as the Python edition masked it, for negative inputs too.
pub fn hash(i: i64, k: i64, z: i64) -> f64 {
    let sum = i
        .wrapping_mul(2_654_435_761)
        .wrapping_add(k.wrapping_mul(40_503))
        .wrapping_add(z.wrapping_mul(7_919))
        .wrapping_add(97);
    let mut x = sum as u32; // the low 32 bits: the same residue as Python's `& 0xFFFFFFFF`
    x ^= x >> 15;
    x = x.wrapping_mul(2_246_822_519);
    x ^= x >> 13;
    f64::from(x & 0xFFFF) / 65536.0
}

fn clamp01(f: f64) -> f64 {
    f.clamp(0.0, 1.0)
}

/// A byte from a value that is at most 255 and not negative by construction; anything else is
/// clamped by the conversion instead of wrapping.
fn byte(v: f64) -> u8 {
    v as u8
}

fn scale(c: Color, f: f64) -> Color {
    let f = clamp01(f);
    [byte(f64::from(c[0]) * f), byte(f64::from(c[1]) * f), byte(f64::from(c[2]) * f)]
}

fn mix(a: Color, b: Color, f: f64) -> Color {
    let f = clamp01(f);
    let one = |i: usize| byte(f64::from(a[i]) + (f64::from(b[i]) - f64::from(a[i])) * f);
    [one(0), one(1), one(2)]
}

/// 0..1 to black, red, orange, yellow-white.
fn heat(v: f64) -> Color {
    let v = clamp01(v);
    if v < 0.4 {
        return [byte(255.0 * v / 0.4), 0, 0];
    }
    if v < 0.75 {
        return [255, byte(160.0 * (v - 0.4) / 0.35), 0];
    }
    [255, byte(160.0 + (95.0 * (v - 0.75) / 0.25).trunc()), byte(200.0 * (v - 0.75) / 0.25)]
}

/// Python's `int()` of a float: truncation toward zero. Values this large do not occur (they would
/// mean an animation running for thousands of years).
fn int(f: f64) -> i64 {
    f as i64
}

/// The colour of one cell. Cells are numbered `column * 6 + row` (22 columns, 6 rows); row 0 is
/// the top row.
fn cell_fx(mode: &str, c: usize, base: Color, t: f64, rate: f64) -> Color {
    let (col, row) = (c / ROWS, c % ROWS);
    let (colf, rowf, cell) = (col as f64, row as f64, c as i64);
    let x = colf / (COLUMNS - 1) as f64;
    // A dark key still shows the effect.
    let lit = if base.iter().copied().max().unwrap_or(0) > 60 { base } else { WHITE };
    match mode {
        "pulse" => scale(lit, 0.15 + 0.85 * (0.5 + 0.5 * (2.0 * PI * rate * t).sin())),
        "breathing" => scale(lit, 0.5 - 0.5 * (2.0 * PI * rate * 0.5 * t).cos()),
        "wave" => keymap::hsv(x - t * rate),
        "rainbow" => keymap::hsv(x * 0.8 + t * rate * 0.3 + rowf * 0.02),
        "cycle" => keymap::hsv(t * rate * 0.5),
        "wave-clockwise" | "wave-counter-clockwise" => {
            let lit_key = base.iter().copied().max().unwrap_or(0) > 60;
            sweep::colour(c, base, lit_key, t, rate, mode == "wave-clockwise")
        }
        "mood" => mix(WHITE, keymap::hsv(t / 60.0 * (rate * 10.0).max(1.0) / 5.0), 0.55),
        "twinkle" => {
            let s = (2.0 * PI * (t * rate * 0.6 + hash(cell, 1, 0))).sin().max(0.0).powf(12.0);
            mix(base, WHITE, s)
        }
        "starlight" => {
            // Each key gets a new chance every cycle.
            let u = t * rate * 0.35 + hash(cell, 2, 0);
            let phase = keymap::py_mod(u, 1.0);
            let on = hash(cell, 3, int(u)) < 0.25;
            mix(scale(base, 0.12), WHITE, if on { 1.0 - (phase * 2.0 - 1.0).abs() } else { 0.0 })
        }
        "scanner" => {
            let pos = (COLUMNS - 1) as f64 * (1.0 - (keymap::py_mod(t * rate * 0.5, 1.0) * 2.0 - 1.0).abs());
            mix(scale(base, 0.08), lit, 1.0 - (colf - pos).abs() / 3.0)
        }
        "comet" => {
            let head = keymap::py_mod(t * rate * 8.0, (COLUMNS + 8) as f64);
            let d = head - colf;
            mix(
                scale(base, 0.05),
                lit,
                if (0.0..=8.0).contains(&d) { (1.0 - d / 8.0).powf(2.0) } else { 0.0 },
            )
        }
        "ripple" => {
            let dx = (colf - (COLUMNS - 1) as f64 / 2.0) / 3.0;
            let dy = rowf - 2.5;
            let d = dx.hypot(dy) * 1.5;
            let rad = keymap::py_mod(t * rate * 6.0, 12.0);
            mix(scale(base, 0.06), lit, 1.0 - (d - rad).abs() / 1.6)
        }
        "rain" => {
            let top = (5 - row) as f64;
            let head = keymap::py_mod(hash(col as i64, 7, 0) * 6.0 + t * rate * 4.0, 9.0);
            let d = head - top;
            mix(scale(base, 0.05), lit, if (0.0..3.0).contains(&d) { 1.0 - d / 3.0 } else { 0.0 })
        }
        "matrix" => {
            let top = (5 - row) as f64;
            let head = keymap::py_mod(t * rate * 5.5 + hash(col as i64, 8, 0) * 12.0, 12.0);
            let d = head - top;
            if !(0.0..=4.0).contains(&d) {
                [0, 6, 0]
            } else if d < 1.0 {
                [180, 255, 180]
            } else {
                [0, byte(255.0 * (1.0 - d / 4.0)), 0]
            }
        }
        "fire" => {
            let top = (5 - row) as f64;
            let k = int(t * rate * 4.0);
            let f = keymap::py_mod(t * rate * 4.0, 1.0);
            let n = hash(cell, k, 0) * (1.0 - f) + hash(cell, k.wrapping_add(1), 0) * f;
            heat(n * 0.55 + (0.5 - top * 0.09))
        }
        "plasma" => {
            let v = (x * 9.0 + t * rate * 2.0).sin()
                + (rowf * 1.7 + t * rate * 1.3).sin()
                + ((x * 6.0 + rowf) + t * rate).sin();
            keymap::hsv(v / 6.0 + 0.5 + t * rate * 0.05)
        }
        "aurora" => {
            let v =
                0.5 + 0.5 * (x * 6.0 + t * rate * 0.8 + rowf * 0.7).sin() * (x * 2.5 - t * rate * 0.4).sin();
            if v > 0.2 { mix([0, 200, 90], [110, 40, 210], v) } else { [0, 10, 25] }
        }
        "lightning" => {
            let k = int(t * rate * 6.0);
            let on = hash(k, 9, 0) < 0.18;
            let strike = int(hash(k, 10, 0) * COLUMNS as f64);
            let width = 2 + int(hash(k, 11, 0) * 4.0);
            if on && (col as i64 - strike).abs() <= width { [220, 230, 255] } else { [5, 5, 18] }
        }
        "confetti" => {
            let k = int(t * rate * 4.0);
            let f = keymap::py_mod(t * rate * 4.0, 1.0);
            for j in 0..2i64 {
                let step = k.wrapping_sub(j);
                if hash(cell, step, 12) < 0.07 {
                    return scale(keymap::hsv(hash(cell, step, 13)), 1.0 - (j as f64 + f) / 2.0);
                }
            }
            scale(base, 0.1)
        }
        _ => base,
    }
}

/// One frame: 128 colours. `keys` are the cells the animation applies to (none: every lit key,
/// cells 0 to 125); the others keep their base colour. `base` shorter than 128 is padded with black
/// and cells outside 0..127 in `keys` are ignored (Python raised an `IndexError`). `speed` is
/// clamped to 1..10, `t` is seconds since the animation started (a time that is not a number is
/// taken as 0). A mode that is not an animation (`off`) returns the base unchanged.
pub fn render(mode: &str, t: f64, base: &[Color], keys: &[usize], speed: i64) -> Vec<Color> {
    let t = if t.is_finite() { t } else { 0.0 };
    // Cycles per second.
    let rate = speed.clamp(SPEED_MIN, SPEED_MAX) as f64 * 0.1;
    let mut cells = vec![[0u8; 3]; NUM_CELLS];
    for (slot, c) in cells.iter_mut().zip(base) {
        *slot = *c;
    }
    let animate = |cells: &mut Vec<Color>, c: usize| {
        if let Some(cell) = cells.get(c).copied() {
            cells[c] = cell_fx(mode, c, cell, t, rate);
        }
    };
    if keys.is_empty() {
        (0..LIT_CELLS).for_each(|c| animate(&mut cells, c));
    } else {
        keys.iter().for_each(|c| animate(&mut cells, *c));
    }
    cells
}
