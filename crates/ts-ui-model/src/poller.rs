//! The ticker behind the live values: calls a function about once a second **while the window is
//! visible** and does nothing at all while it is hidden or minimised (APP-DESIGN 4.10: a tray-only
//! app wakes no thread and reads no hardware).
//!
//! The function runs on the poller's own thread, so it may block on hardware reads.
#![forbid(unsafe_code)]

use std::io;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[derive(Default)]
struct Switch {
    active: bool,
    stop: bool,
}

struct Shared {
    state: Mutex<Switch>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Switch> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
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
            loop {
                let mut state = worker.lock();
                while !state.active && !state.stop {
                    state = worker.wake.wait(state).unwrap_or_else(PoisonError::into_inner);
                }
                if state.stop {
                    return;
                }
                drop(state);
                tick();
                let state = worker.lock();
                // Sleep for the interval, but wake early when told to stop or pause; a pause that
                // is lifted again within the interval starts a fresh tick via the outer loop.
                let (state, _) = worker
                    .wake
                    .wait_timeout_while(state, interval, |s| s.active && !s.stop)
                    .unwrap_or_else(PoisonError::into_inner);
                if state.stop {
                    return;
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
