//! The PREDATOR key's press patterns: tap, double, triple and long.
//!
//! [`Gestures`] is a pure state machine. It never reads a clock and starts no timer: every method
//! takes the time of the event, first fires whatever deadline has passed by then (in the order the
//! deadlines fell), and answers the gestures that resolved. The service asks [`Gestures::deadline`]
//! for the next moment something can happen and calls [`Gestures::advance`] then. That keeps the
//! timing deterministic in tests and leaves the event loop in charge of waiting (the Python
//! edition used one `threading.Timer` per press, which raced with the release).
//!
//! Timing, as in 0.3.x:
//!
//! * a long press fires while the key is still held, `long_after` after the press, so it feels
//!   instant; the release that follows is not also a tap;
//! * taps are counted; the third resolves at once, otherwise the count resolves `multi_window`
//!   after the last release unless another press comes first;
//! * a press cancels the pending resolution but keeps the count, and a long press discards the
//!   taps that came before it.
#![forbid(unsafe_code)]

use std::time::Duration;

use ts_domain::keybinds::Gesture;

/// Turns PREDATOR key presses and releases into gestures. Times are any monotonic offsets (the
/// service uses the time since it started); only their differences matter. It answers only
/// [`Gesture::Tap`], [`Gesture::Double`], [`Gesture::Triple`] and [`Gesture::Long`]: Fn+Esc is a
/// key combination of its own, not a gesture of this key.
#[derive(Debug, Clone)]
pub struct Gestures {
    long_after: Duration,
    multi_window: Duration,
    taps: u32,
    down: bool,
    long_done: bool,
    long_at: Option<Duration>,
    multi_at: Option<Duration>,
}

impl Gestures {
    /// A machine with the given long-press and multi-tap timings.
    pub fn new(long_after: Duration, multi_window: Duration) -> Self {
        Self {
            long_after,
            multi_window,
            taps: 0,
            down: false,
            long_done: false,
            long_at: None,
            multi_at: None,
        }
    }

    /// Changes the timings for presses and releases from now on; a deadline already running keeps
    /// its time, as the Python edition's timers did.
    pub fn set_timing(&mut self, long_after: Duration, multi_window: Duration) {
        self.long_after = long_after;
        self.multi_window = multi_window;
    }

    /// The PREDATOR key went down at `now`. A press while the key is already down is a repeated
    /// report, not a new press, and is ignored (the Python edition armed a second long timer for
    /// it).
    pub fn press(&mut self, now: Duration) -> Vec<Gesture> {
        let out = self.advance(now);
        if self.down {
            return out;
        }
        self.multi_at = None;
        self.long_done = false;
        self.down = true;
        self.long_at = Some(now.saturating_add(self.long_after));
        out
    }

    /// The key came up at `now`. A release that does not belong to a PREDATOR press is ignored:
    /// Fn+F10, Fn+F11, Fn+F12 and Fn+Esc all end with the same `04 00 00` report and none of them
    /// may count as a tap.
    pub fn release(&mut self, now: Duration) -> Vec<Gesture> {
        let mut out = self.advance(now);
        if !self.down {
            return out;
        }
        self.down = false;
        self.long_at = None;
        if self.long_done {
            self.taps = 0;
            return out;
        }
        self.taps += 1;
        if self.taps >= 3 {
            out.extend(self.flush());
        } else {
            self.multi_at = Some(now.saturating_add(self.multi_window));
        }
        out
    }

    /// Fires every deadline that has passed by `now` and answers what resolved.
    pub fn advance(&mut self, now: Duration) -> Vec<Gesture> {
        let mut out = Vec::new();
        if self.long_at.is_some_and(|at| now >= at) {
            self.long_at = None;
            self.long_done = true;
            self.taps = 0;
            out.push(Gesture::Long);
        }
        if self.multi_at.is_some_and(|at| now >= at) {
            out.extend(self.flush());
        }
        out
    }

    /// The next time [`Gestures::advance`] has something to do, if any.
    pub fn deadline(&self) -> Option<Duration> {
        match (self.long_at, self.multi_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// The keyboard went away at `now`. A press that is still down will never be released: it is
    /// dropped (nobody can say whether it was a tap), while taps that were already counted still
    /// resolve in their window.
    pub fn abandon_press(&mut self, now: Duration) {
        if !self.down {
            return;
        }
        self.down = false;
        self.long_at = None;
        if self.long_done {
            self.taps = 0;
        } else if self.taps > 0 {
            self.multi_at = Some(now.saturating_add(self.multi_window));
        }
    }

    /// Resolves the counted taps: one is a tap, two a double, three or more a triple.
    fn flush(&mut self) -> Option<Gesture> {
        let taps = std::mem::take(&mut self.taps);
        self.multi_at = None;
        Gesture::from_taps(taps)
    }
}
