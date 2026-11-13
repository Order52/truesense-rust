//! hwkeys: the keyboard's own lighting keys, as the lighting loops need to know about them.
//!
//! Fn+F11 and Fn+F12 (and the other keys the keyboard handles by itself) change the backlight
//! inside the keyboard; the key service only sees a vendor report go by. The theme renderer and
//! the per-key animations send a brightness with every frame, so without a word from the key
//! service the next frame (66 ms later) would put the old level back, and the next read of the
//! keyboard (seconds later) would only find that old level again: the keys would never work while
//! a picture is moving. The key service calls [`pressed`]; the loops compare [`generation`] with
//! the one they saw last and then follow the keyboard's own brightness for [`FOLLOW_S`].
#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicU64, Ordering};

use ts_foundation::wake;

/// How long after such a key the loops read the keyboard's brightness before every frame (the
/// keyboard applies the change around the report, and a key may be held to repeat).
pub const FOLLOW_S: f64 = 1.5;

static GENERATION: AtomicU64 = AtomicU64::new(0);

/// A key the keyboard handles by itself was pressed: the loops re-read the brightness now.
pub fn pressed() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    // The loops may be sleeping between frames or idle: wake them so the first frame after the
    // key already follows it.
    wake::ring();
}

/// How many such keys have been pressed since the process started.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_press_moves_the_generation_on() {
        let before = generation();
        pressed();
        pressed();
        assert!(generation() >= before + 2);
    }
}
