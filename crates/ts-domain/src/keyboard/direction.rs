//! The directions of the keyboard's hardware Wave.
//!
//! The effect packet carries one direction byte (`08 02 <effect> <speed> <brightness> 01 <byte>`).
//! TrueSense stores the *logical* direction (0 = the first) and sends `logical + 1`, so the byte
//! is 1 for the default. On a PH16-71 the Wave was seen taking six values (the reference project
//! `predator-sense`, tested on the same laptop on 2026-10-15; its `wire_direction` sends the byte
//! as it is, clamped to 1..=6, and its `directions_keep_their_hardware_confirmed_values` test pins
//! the order):
//!
//! | wire byte | logical | direction |
//! |---|---|---|
//! | 1 | 0 | left to right |
//! | 2 | 1 | right to left |
//! | 3 | 2 | bottom to top |
//! | 4 | 3 | top to bottom |
//! | 5 | 4 | clockwise |
//! | 6 | 5 | counter-clockwise |
//!
//! Only the Wave takes them; no other effect of the 14 was seen to honour a direction.
#![forbid(unsafe_code)]

use std::fmt;

/// One of the six directions of the Wave.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Left to right (the default).
    LeftToRight,
    /// Right to left.
    RightToLeft,
    /// Bottom to top.
    BottomToTop,
    /// Top to bottom.
    TopToBottom,
    /// Clockwise around the keyboard.
    Clockwise,
    /// Counter-clockwise around the keyboard.
    CounterClockwise,
}

/// A direction text or number that is not one of the six.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectionError(pub String);

impl fmt::Display for DirectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DirectionError {}

impl Direction {
    /// The six, in the order users see them (and in logical order).
    pub const ALL: [Direction; 6] = [
        Self::LeftToRight,
        Self::RightToLeft,
        Self::BottomToTop,
        Self::TopToBottom,
        Self::Clockwise,
        Self::CounterClockwise,
    ];

    /// The stored number, 0 to 5.
    pub const fn logical(self) -> i64 {
        self as i64
    }

    /// The byte the keyboard receives: the logical number plus one (1 to 6).
    pub const fn wire_byte(self) -> u8 {
        self as u8 + 1
    }

    /// The direction for a stored number, `None` outside 0 to 5.
    pub fn from_logical(n: i64) -> Option<Self> {
        usize::try_from(n).ok().and_then(|i| Self::ALL.get(i).copied())
    }

    /// The name the command line, the bus and the window use.
    pub const fn id(self) -> &'static str {
        match self {
            Self::LeftToRight => "left-to-right",
            Self::RightToLeft => "right-to-left",
            Self::BottomToTop => "bottom-to-top",
            Self::TopToBottom => "top-to-bottom",
            Self::Clockwise => "clockwise",
            Self::CounterClockwise => "counter-clockwise",
        }
    }

    /// Reads a name (`clockwise`, `counter-clockwise`, ...; case, `_`, spaces and the short forms
    /// `cw` and `ccw` are accepted) or a number 0 to 5.
    pub fn parse(text: &str) -> Result<Self, DirectionError> {
        let t = text.trim().to_lowercase().replace(['_', ' '], "-");
        if let Ok(n) = t.parse::<i64>() {
            return Self::from_logical(n).ok_or_else(|| bad(text));
        }
        match t.as_str() {
            "cw" | "clockwise" => Ok(Self::Clockwise),
            "ccw" | "counterclockwise" | "counter-clockwise" | "anticlockwise" | "anti-clockwise" => {
                Ok(Self::CounterClockwise)
            }
            other => Self::ALL.into_iter().find(|d| d.id() == other).ok_or_else(|| bad(text)),
        }
    }
}

fn bad(text: &str) -> DirectionError {
    DirectionError(format!("direction '{text}': {}", allowed()))
}

/// What is accepted, for an error message: `0..5 or left-to-right, ...`.
pub fn allowed() -> String {
    let names: Vec<&str> = Direction::ALL.iter().map(|d| d.id()).collect();
    format!("0..5 or {}", names.join(", "))
}

/// Whether the hardware effect honours a direction: only the Wave does.
pub fn takes_direction(effect: &str) -> bool {
    effect == "wave"
}

/// A direction number for the effect, as `Keyboard::set_effect` takes it. The Wave takes the six
/// logical values 0 to 5 and nothing else; every other effect keeps the old rule (any byte,
/// 0 to 254, that the keyboard ignores).
pub fn check(effect: &str, logical: i64) -> Result<(), DirectionError> {
    if takes_direction(effect) && Direction::from_logical(logical).is_none() {
        return Err(DirectionError(format!("wave direction {logical}: {}", allowed())));
    }
    Ok(())
}
