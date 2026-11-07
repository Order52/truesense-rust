//! One bell for the whole process: a sleep that something else can cut short.
//!
//! The long-running services (the key service, the audio watcher) used to look at their stop flag
//! every 50 to 500 milliseconds, dozens of wake-ups a second doing nothing. A thread that sleeps
//! on this bell instead sleeps for as long as it likes and is woken the moment somebody rings it:
//! the signal bridge rings it when SIGTERM or SIGINT arrives, and an event source (a sound-system
//! event, a changed file) rings it when there is something to look at. Nothing here polls.
//!
//! Waiters pass a predicate ("is my flag set?"), which is evaluated while the bell's lock is held,
//! and ringers set their flag *before* they ring, so a ring can never fall between a waiter's check
//! and its sleep. A ring wakes every waiter; each one looks at its own predicate and goes back to
//! sleep if it is not for them, which is cheap because rings are rare (a stop, a device event).
#![forbid(unsafe_code)]

use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Counts the rings, so that a waiter can tell "woken by a ring" from "woken by nothing".
static RINGS: Mutex<u64> = Mutex::new(0);
static BELL: Condvar = Condvar::new();

/// Wakes every thread waiting on the bell. Set whatever flag the waiters look at first.
pub fn ring() {
    let mut rings = RINGS.lock().unwrap_or_else(PoisonError::into_inner);
    *rings = rings.wrapping_add(1);
    drop(rings);
    BELL.notify_all();
}

/// Waits until `ready()` is true or `timeout` has passed, and answers `ready()`'s last value.
/// `ready` is called with the bell's lock held: it must be quick and must not ring.
pub fn wait_until(timeout: Duration, ready: impl Fn() -> bool) -> bool {
    wait_inner(timeout, |_| ready())
}

/// A sleep of up to `timeout` that any ring cuts short. For loops that re-read their own state
/// after every pause, where an early return only means looking sooner.
pub fn sleep(timeout: Duration) {
    let before = *RINGS.lock().unwrap_or_else(PoisonError::into_inner);
    wait_inner(timeout, |rings| rings != before);
}

fn wait_inner(timeout: Duration, ready: impl Fn(u64) -> bool) -> bool {
    let deadline = Instant::now().checked_add(timeout);
    let mut rings = RINGS.lock().unwrap_or_else(PoisonError::into_inner);
    loop {
        if ready(*rings) {
            return true;
        }
        let left = match deadline {
            Some(deadline) => {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return false;
                }
                left
            }
            // A timeout too long to add to the clock is "for ever".
            None => Duration::from_secs(3600),
        };
        rings = BELL.wait_timeout(rings, left).unwrap_or_else(PoisonError::into_inner).0;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    #[test]
    fn a_wait_with_nothing_ready_lasts_the_timeout() {
        let started = Instant::now();
        assert!(!wait_until(Duration::from_millis(60), || false));
        assert!(started.elapsed() >= Duration::from_millis(55), "{:?}", started.elapsed());
    }

    #[test]
    fn a_flag_that_is_already_set_returns_at_once() {
        let started = Instant::now();
        assert!(wait_until(Duration::from_secs(30), || true));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn a_ring_after_the_flag_wakes_a_sleeper_long_before_its_timeout() {
        let flag = Arc::new(AtomicBool::new(false));
        let sleeper = {
            let flag = flag.clone();
            std::thread::spawn(move || {
                let started = Instant::now();
                let ready = wait_until(Duration::from_secs(30), || flag.load(Ordering::SeqCst));
                (ready, started.elapsed())
            })
        };
        std::thread::sleep(Duration::from_millis(50));
        flag.store(true, Ordering::SeqCst);
        ring();
        let (ready, took) = sleeper.join().unwrap();
        assert!(ready);
        assert!(took < Duration::from_secs(5), "{took:?}");
    }

    #[test]
    fn a_ring_for_somebody_else_does_not_end_the_wait_early() {
        let started = Instant::now();
        let ringer = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(20));
            ring();
        });
        assert!(!wait_until(Duration::from_millis(120), || false));
        ringer.join().unwrap();
        assert!(started.elapsed() >= Duration::from_millis(115));
    }

    #[test]
    fn a_ring_cuts_a_plain_sleep_short() {
        let sleeper = std::thread::spawn(|| {
            let started = Instant::now();
            sleep(Duration::from_secs(30));
            started.elapsed()
        });
        // The sleeper must be waiting before the ring comes: ring until it is done.
        while !sleeper.is_finished() {
            std::thread::sleep(Duration::from_millis(20));
            ring();
        }
        assert!(sleeper.join().unwrap() < Duration::from_secs(5));
    }

    #[test]
    fn an_absurd_timeout_does_not_overflow() {
        assert!(wait_until(Duration::MAX, || true));
    }
}
