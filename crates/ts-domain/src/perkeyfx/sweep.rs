//! The rotating sweeps: a colour wave that turns around the middle of the keyboard.
//!
//! The keyboard's own Wave can also run clockwise and counter-clockwise (see
//! [`crate::keyboard::Direction`]), but it is a fixed rainbow that ignores every colour. These two
//! per-key animations do the same turn in software, so they work with any colours:
//!
//! * a key that has a colour of its own (the brightest channel above 60, the rule of the other
//!   animations) keeps it, and a soft bright band travels around the keyboard across the user's
//!   picture: a gradient design stays a gradient;
//! * a dark key takes the rainbow, so on a black base the sweep is the hardware wave's look.
//!
//! Where a key is comes from the real key rectangles ([`crate::keymap::geometry`]) joined to the
//! verified key map: a cell takes the middle of the rectangle of the key it lights, so a key has
//! one angle and one colour. (The cell numbering is not physical order: inside a column the cells
//! are scrambled, which is why the rectangles are needed.) A cell that carries no key name, such
//! as the unlabelled rest of the space bar or the second cell of a wide key, takes the position of
//! the nearest cell that has one. The angle is measured from the middle of the keyboard, grows
//! clockwise as the person typing sees it, and is 0 pointing right.
//!
//! The turn is at most half a revolution per second (speed 10), so any one key brightens or changes
//! hue at most once every two seconds: far below the three flashes a second of WCAG 2.3.1, and
//! frames are drawn only five times a second anyway. Everything is a pure function of the time.
#![forbid(unsafe_code)]

use std::f64::consts::{PI, TAU};
use std::sync::OnceLock;

use crate::keymap::geometry::{HEIGHT as KEYBOARD_HEIGHT, WIDTH as KEYBOARD_WIDTH, key_rects};
use crate::keymap::{self, Color, KeyMap, NUM_CELLS, ROWS};

/// Revolutions per second at speed 10; the sweep never turns faster.
const MAX_REVOLUTIONS_PER_SECOND: f64 = 0.5;

/// The dimmest a coloured key gets while the band is away.
const DIM: f64 = 0.3;

/// The middle of each cell's key in key units from the top left.
fn centres() -> [(f64, f64); NUM_CELLS] {
    let rects = key_rects();
    let mut known: [Option<(f64, f64)>; NUM_CELLS] = [None; NUM_CELLS];
    for (name, cells) in KeyMap::verified().names() {
        if let Some(rect) = rects.iter().find(|r| r.name == name) {
            for cell in cells.iter().copied().filter(|c| *c < NUM_CELLS) {
                // An alias (`enter` for `return`) shares cells with the name before it: the first counts.
                known[cell].get_or_insert(rect.centre());
            }
        }
    }
    let lattice = |cell: usize| ((cell / ROWS) as i64, (cell % ROWS) as i64);
    let mut out = [(KEYBOARD_WIDTH / 2.0, KEYBOARD_HEIGHT / 2.0); NUM_CELLS];
    for cell in 0..NUM_CELLS {
        out[cell] = known[cell].unwrap_or_else(|| {
            let (col, row) = lattice(cell);
            (0..NUM_CELLS)
                .filter_map(|other| known[other].map(|p| (other, p)))
                .min_by_key(|(other, _)| {
                    let (c, r) = lattice(*other);
                    ((c - col).pow(2) + (r - row).pow(2), *other)
                })
                .map_or((KEYBOARD_WIDTH / 2.0, KEYBOARD_HEIGHT / 2.0), |(_, p)| p)
        });
    }
    out
}

/// The angle of each cell's key around the middle of the keyboard in turns, 0 up to (not
/// including) 1, 0 pointing right and growing clockwise (downwards on the screen).
fn angles() -> &'static [f64; NUM_CELLS] {
    static ANGLES: OnceLock<[f64; NUM_CELLS]> = OnceLock::new();
    ANGLES.get_or_init(|| {
        let mut out = [0.0; NUM_CELLS];
        for (slot, (x, y)) in out.iter_mut().zip(centres()) {
            *slot = ((y - KEYBOARD_HEIGHT / 2.0).atan2(x - KEYBOARD_WIDTH / 2.0) / TAU).rem_euclid(1.0);
        }
        out
    })
}

/// The angle of a cell in turns (see `angles`); 0 for a cell that does not exist.
pub fn cell_angle(cell: usize) -> f64 {
    angles().get(cell).copied().unwrap_or(0.0)
}

/// The phase of the pattern at an angle: the pattern is a function of `angle - turn` for the
/// clockwise sweep and of `-angle - turn` for the counter-clockwise one, so its features move
/// towards larger angles (clockwise) or smaller ones (counter-clockwise) as `turn` grows.
pub fn phase(angle: f64, turn: f64, clockwise: bool) -> f64 {
    if clockwise { angle - turn } else { -angle - turn }
}

/// How far the sweep has turned after `t` seconds at the animation `rate` (0.1 to 1.0).
fn turn(t: f64, rate: f64) -> f64 {
    t * rate * MAX_REVOLUTIONS_PER_SECOND
}

/// The colour of one cell at time `t`. `lit` tells whether the key has a colour of its own.
pub(super) fn colour(cell: usize, base: Color, lit: bool, t: f64, rate: f64, clockwise: bool) -> Color {
    let phase = phase(cell_angle(cell), turn(t, rate), clockwise);
    if lit {
        let band = 0.5 + 0.5 * (2.0 * PI * phase).cos();
        let f = (DIM + (1.0 - DIM) * band).clamp(0.0, 1.0);
        let one = |v: u8| (f64::from(v) * f) as u8;
        [one(base[0]), one(base[1]), one(base[2])]
    } else {
        keymap::hsv(phase)
    }
}
