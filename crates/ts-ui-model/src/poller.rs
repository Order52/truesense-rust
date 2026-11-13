//! The ticker behind the live values: calls a function about once a second **while the window is
//! visible** and does nothing at all while it is hidden or minimised (APP-DESIGN 4.10: a tray-only
//! app wakes no thread and reads no hardware).
//!
//! The function runs on the poller's own thread, so it may block on hardware reads.
//!
//! A [`Poke`] asks for one extra tick now (after a command changed the machine, so a tile bound to
//! the live values shows the new state at once instead of up to a second later). The first poke
//! after a quiet moment ticks at once; pokes that come close together are folded, at most one
//! poked tick every [`MIN_POKE_GAP`].
#![forbid(unsafe_code)]

use std::io;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use ts_foundation::sync::locked;

/// The shortest time between two ticks that pokes cause (a slider drag pokes on every write; the
/// live values need not follow faster than this, and without the service every tick is a few
/// firmware calls).
pub const MIN_POKE_GAP: Duration = Duration::from_millis(250);

#[derive(Default)]
struct Switch {
    active: bool,
    stop: bool,
    poked: bool,
}

struct Shared {
    state: Mutex<Switch>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Switch> {
        locked(&self.state)
    }
}

/// A thread that ticks while active.
pub struct Poller {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Poller {
    /// Starts the thread, inactive. `tick` is called once at once when the poller becomes active
    /// (so a window that just appeared is not blank for a second), then every `interval`.
    ///
    /// # Errors
    /// The operating system could not start the thread.
    pub fn spawn(interval: Duration, mut tick: impl FnMut() + Send + 'static) -> io::Result<Self> {
        let shared = Arc::new(Shared { state: Mutex::new(Switch::default()), wake: Condvar::new() });
        let worker = shared.clone();
        let thread = thread::Builder::new().name("ts-live-poller".to_owned()).spawn(move || {
            // When the last tick that a poke caused started.
            let mut last_poked: Option<Instant> = None;
            let mut poked_now = false;
            loop {
                let mut state = worker.lock();
                while !state.active && !state.stop {
                    state = worker.wake.wait(state).unwrap_or_else(PoisonError::into_inner);
                }
                if state.stop {
                    return;
                }
                state.poked = false;
                drop(state);
                if poked_now {
                    last_poked = Some(Instant::now());
                    poked_now = false;
                }
                tick();
                let state = worker.lock();
                let (mut state, _) = worker
                    .wake
                    .wait_timeout_while(state, interval, |s| s.active && !s.stop && !s.poked)
                    .unwrap_or_else(PoisonError::into_inner);
                if state.stop {
                    return;
                }
                // A poke soon after the last poked tick waits for the gap (more pokes in that time
                // are one).
                if state.poked && state.active {
                    poked_now = true;
                    let gap =
                        last_poked.map_or(Duration::ZERO, |at| MIN_POKE_GAP.saturating_sub(at.elapsed()));
                    if !gap.is_zero() {
                        (state, _) = worker
                            .wake
                            .wait_timeout_while(state, gap, |s| s.active && !s.stop)
                            .unwrap_or_else(PoisonError::into_inner);
                    }
                    if state.stop {
                        return;
                    }
                }
            }
        })?;
        Ok(Self { shared, thread: Some(thread) })
    }

    /// Turns ticking on (an immediate tick follows) or off (the current tick finishes, no more start).
    pub fn set_active(&self, active: bool) {
        let mut state = self.shared.lock();
        if state.active != active {
            state.active = active;
            self.shared.wake.notify_all();
        }
    }

    /// A handle that asks for an extra tick from any thread (see [`Poke`]).
    pub fn poke_handle(&self) -> Poke {
        Poke(self.shared.clone())
    }

    /// Whether it is ticking.
    pub fn is_active(&self) -> bool {
        self.shared.lock().active
    }

    /// Stops the thread and waits for it.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.shared.lock().stop = true;
        self.shared.wake.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Asks a [`Poller`] for one tick as soon as [`MIN_POKE_GAP`] allows; cheap to clone, usable from
/// any thread. A poke while the poller is inactive (window hidden) does nothing: the first tick
/// after it becomes active comes at once anyway.
#[derive(Clone)]
pub struct Poke(Arc<Shared>);

impl Poke {
    /// Asks for the tick.
    pub fn poke(&self) {
        let mut state = self.0.lock();
        if state.active && !state.poked {
            state.poked = true;
            self.0.wake.notify_all();
        }
    }
}

impl Drop for Poller {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Instant;

    fn counter() -> (Arc<AtomicU32>, impl FnMut() + Send + 'static) {
        let n = Arc::new(AtomicU32::new(0));
        let c = n.clone();
        (n, move || {
            c.fetch_add(1, Ordering::SeqCst);
        })
    }

    fn wait_until(what: &str, cond: impl Fn() -> bool) {
        let start = Instant::now();
        while !cond() {
            assert!(start.elapsed() < Duration::from_secs(5), "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn nothing_ticks_while_inactive() {
        let (n, tick) = counter();
        let p = Poller::spawn(Duration::from_millis(5), tick).unwrap();
        thread::sleep(Duration::from_millis(60));
        assert_eq!(n.load(Ordering::SeqCst), 0);
        assert!(!p.is_active());
        p.stop();
    }

    #[test]
    fn activating_ticks_at_once_and_then_repeatedly() {
        let (n, tick) = counter();
        let p = Poller::spawn(Duration::from_millis(10), tick).unwrap();
        p.set_active(true);
        wait_until("first tick", || n.load(Ordering::SeqCst) >= 1);
        wait_until("more ticks", || n.load(Ordering::SeqCst) >= 4);
        p.stop();
    }

    #[test]
    fn deactivating_stops_the_ticks_and_reactivating_resumes_them() {
        let (n, tick) = counter();
        let p = Poller::spawn(Duration::from_millis(5), tick).unwrap();
        p.set_active(true);
        wait_until("ticks", || n.load(Ordering::SeqCst) >= 2);
        p.set_active(false);
        thread::sleep(Duration::from_millis(40));
        let frozen = n.load(Ordering::SeqCst);
        thread::sleep(Duration::from_millis(60));
        assert_eq!(n.load(Ordering::SeqCst), frozen, "no ticks while hidden");
        p.set_active(true);
        wait_until("resume", || n.load(Ordering::SeqCst) > frozen);
        p.stop();
    }

    #[test]
    fn a_poke_ticks_at_once_instead_of_after_the_interval() {
        let (n, tick) = counter();
        let p = Poller::spawn(Duration::from_secs(60), tick).unwrap();
        p.set_active(true);
        wait_until("first tick", || n.load(Ordering::SeqCst) == 1);
        let poke = p.poke_handle();
        let start = Instant::now();
        poke.poke();
        wait_until("poked tick", || n.load(Ordering::SeqCst) == 2);
        assert!(start.elapsed() < Duration::from_secs(2), "the poke did not wait for the 60 s interval");
        p.stop();
    }

    #[test]
    fn pokes_close_together_are_one_tick_and_keep_the_gap() {
        let (n, tick) = counter();
        let p = Poller::spawn(Duration::from_secs(60), tick).unwrap();
        p.set_active(true);
        wait_until("first tick", || n.load(Ordering::SeqCst) == 1);
        let poke = p.poke_handle();
        let start = Instant::now();
        for _ in 0..20 {
            poke.poke();
            thread::sleep(Duration::from_millis(2));
        }
        wait_until("poked tick", || n.load(Ordering::SeqCst) >= 2);
        thread::sleep(MIN_POKE_GAP * 2);
        let ticks = n.load(Ordering::SeqCst);
        assert!(ticks <= 3, "20 pokes in 40 ms made {ticks} ticks");
        assert!(start.elapsed() >= MIN_POKE_GAP.saturating_sub(Duration::from_millis(5)));
        p.stop();
    }

    #[test]
    fn a_poke_while_hidden_does_nothing() {
        let (n, tick) = counter();
        let p = Poller::spawn(Duration::from_millis(5), tick).unwrap();
        p.poke_handle().poke();
        thread::sleep(Duration::from_millis(50));
        assert_eq!(n.load(Ordering::SeqCst), 0);
        p.stop();
    }

    #[test]
    fn a_long_interval_does_not_delay_stop() {
        let (_n, tick) = counter();
        let p = Poller::spawn(Duration::from_secs(60), tick).unwrap();
        p.set_active(true);
        thread::sleep(Duration::from_millis(20));
        let start = Instant::now();
        p.stop();
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
