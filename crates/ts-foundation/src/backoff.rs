//! A growing pause for loops that wait on something that has no way to wake them (a child
//! process that has not exited yet).
//!
//! A fixed pause of 10 ms makes every short helper (`pactl`, `amixer`, `systemctl`, which finish
//! in a few milliseconds) cost up to 10 ms more than it needs, which added up to a third of the
//! time of `truesense status`. Starting at 100 microseconds and doubling up to 5 ms keeps the
//! added wait near zero for quick programs and the wake-ups rare for slow ones.
#![forbid(unsafe_code)]

use std::time::Duration;

/// The first pause.
const FIRST: Duration = Duration::from_micros(100);
/// The longest pause: the cost of noticing a slow program's exit late.
const LONGEST: Duration = Duration::from_millis(5);

/// A pause that doubles each time it is used, from 100 microseconds up to 5 milliseconds.
#[derive(Debug, Clone, Copy)]
pub struct Backoff {
    next: Duration,
}

impl Backoff {
    /// A pause starting at its shortest.
    pub const fn new() -> Self {
        Self { next: FIRST }
    }

    /// The length of the next pause, and grows the one after it.
    pub fn next_pause(&mut self) -> Duration {
        let now = self.next;
        self.next = (self.next * 2).min(LONGEST);
        now
    }

    /// Sleeps for [`next_pause`](Self::next_pause).
    pub fn wait(&mut self) {
        std::thread::sleep(self.next_pause());
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pauses_double_from_100_microseconds_and_stop_at_5_milliseconds() {
        let mut b = Backoff::new();
        let pauses: Vec<u128> = (0..9).map(|_| b.next_pause().as_micros()).collect();
        assert_eq!(pauses, [100, 200, 400, 800, 1600, 3200, 5000, 5000, 5000]);
    }

    #[test]
    fn a_quick_wait_costs_far_less_than_the_old_fixed_pause() {
        let mut b = Backoff::new();
        let started = std::time::Instant::now();
        b.wait();
        assert!(started.elapsed() < Duration::from_millis(10));
    }
}
