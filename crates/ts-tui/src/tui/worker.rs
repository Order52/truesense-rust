//! The threads behind the screen: one that writes to the hardware, and the ones that read.
//!
//! The screen's own thread never waits for the laptop. Writes go to one worker that runs them one
//! at a time in the order they were made (two writes to the firmware must never interleave), and a
//! newer write for the same setting replaces an older one that has not started: dragging a slider
//! sends the last position, not every step. Reads of the dashboard happen every second; the
//! settings of the open page are read when the page opens and after every write, never two batches
//! at once (the newest request waits for the one running).

use std::any::Any;
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use super::backend::Backend;
use super::data::ReadKey;
use super::model::{Msg, Ticket, Write};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The writes waiting for their turn.
#[derive(Debug, Default)]
pub(crate) struct Queue {
    jobs: VecDeque<Write>,
}

impl Queue {
    /// Adds a write behind the others. An older write with the same key that has not started is
    /// replaced; its ticket is answered so the screen can settle its row. A write with an empty key
    /// never replaces anything.
    pub(crate) fn submit(&mut self, write: Write) -> Vec<Ticket> {
        let mut dropped = Vec::new();
        if !write.key.is_empty() {
            self.jobs.retain(|j| {
                if j.key == write.key {
                    dropped.push(j.ticket);
                    false
                } else {
                    true
                }
            });
        }
        self.jobs.push_back(write);
        dropped
    }

    pub(crate) fn next(&mut self) -> Option<Write> {
        self.jobs.pop_front()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.jobs.len()
    }
}

#[derive(Default)]
struct WorkerState {
    queue: Queue,
    /// A thread is draining the queue or a write is executing.
    running: bool,
}

/// Runs the writes. A worker that is idle holds no thread.
pub(crate) struct Worker {
    shared: Arc<(Mutex<WorkerState>, Condvar)>,
    backend: Arc<dyn Backend>,
    results: Sender<Msg>,
}

impl Worker {
    pub(crate) fn new(backend: Arc<dyn Backend>, results: Sender<Msg>) -> Self {
        Self { shared: Arc::new((Mutex::new(WorkerState::default()), Condvar::new())), backend, results }
    }

    /// Queues a write; answers the tickets of the writes it replaced.
    pub(crate) fn submit(&self, write: Write) -> Vec<Ticket> {
        let (state, _) = &*self.shared;
        let mut s = lock(state);
        let dropped = s.queue.submit(write);
        let start = !s.running;
        s.running = true;
        drop(s);
        if start {
            let shared = Arc::clone(&self.shared);
            let backend = Arc::clone(&self.backend);
            let results = self.results.clone();
            thread::spawn(move || drain_queue(&shared, &*backend, &results));
        }
        dropped
    }

    /// True when nothing is queued or running.
    #[cfg(test)]
    pub(crate) fn idle(&self) -> bool {
        !lock(&self.shared.0).running
    }

    /// Waits until every queued write is done (on quit, so a change made a moment ago is not
    /// lost); answers whether the worker is idle.
    pub(crate) fn drain(&self, timeout: Duration) -> bool {
        let (state, idle) = &*self.shared;
        let deadline = Instant::now() + timeout;
        let mut s = lock(state);
        while s.running {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            s = idle.wait_timeout(s, left).unwrap_or_else(PoisonError::into_inner).0;
        }
        true
    }
}

/// The text of a panic, for the line that says a write crashed.
fn panic_text(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Drains the queue and ends when it is empty. A write reports its own errors and the queue must
/// never die, so a panic inside one is turned into the write's error.
fn drain_queue(shared: &(Mutex<WorkerState>, Condvar), backend: &dyn Backend, results: &Sender<Msg>) {
    let (state, idle) = shared;
    loop {
        let next = {
            let mut s = lock(state);
            let next = s.queue.next();
            if next.is_none() {
                s.running = false;
                idle.notify_all();
            }
            next
        };
        let Some(write) = next else { return };
        let result = catch_unwind(AssertUnwindSafe(|| backend.execute(&write.op)))
            .unwrap_or_else(|p| Err(format!("internal error: {}", panic_text(&*p))));
        // The screen may be gone (quit): nobody to tell.
        let _ = results.send(Msg::Written { ticket: write.ticket, result });
    }
}

/// Which batch of slow reads runs next. Pure, so the "never overlap, the newest waits" rule can be
/// tested without threads.
#[derive(Debug, Default)]
pub(crate) struct SlowGate {
    busy: bool,
    waiting: Option<Vec<ReadKey>>,
}

impl SlowGate {
    /// A batch is wanted: it runs now (answered) unless one is running, in which case it waits and
    /// replaces any batch that was waiting.
    pub(crate) fn request(&mut self, keys: Vec<ReadKey>) -> Option<Vec<ReadKey>> {
        if self.busy {
            self.waiting = Some(keys);
            None
        } else {
            self.busy = true;
            Some(keys)
        }
    }

    /// The running batch is done: the waiting one runs next, else the gate opens.
    pub(crate) fn finished(&mut self) -> Option<Vec<ReadKey>> {
        let next = self.waiting.take();
        self.busy = next.is_some();
        next
    }
}

/// The background readers: the dashboard every second, the page's settings on request.
pub(crate) struct Readers {
    backend: Arc<dyn Backend>,
    results: Sender<Msg>,
    wake: Sender<()>,
    gate: Arc<Mutex<SlowGate>>,
    stop: Arc<AtomicBool>,
}

impl Readers {
    /// Starts the dashboard reader: it reads at once, then every second or when woken.
    pub(crate) fn start(backend: Arc<dyn Backend>, results: Sender<Msg>) -> Self {
        let (wake, woken): (Sender<()>, Receiver<()>) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let readers = Self {
            backend: Arc::clone(&backend),
            results: results.clone(),
            wake,
            gate: Arc::new(Mutex::new(SlowGate::default())),
            stop: Arc::clone(&stop),
        };
        thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                let fast = backend.fast();
                if results.send(Msg::Fast(Box::new(fast))).is_err() {
                    return;
                }
                // A wake-up (or the second running out) means read again.
                let _ = woken.recv_timeout(Duration::from_secs(1));
            }
        });
        readers
    }

    /// Reads the dashboard now instead of at the next second.
    pub(crate) fn wake(&self) {
        let _ = self.wake.send(());
    }

    /// Reads these settings in the background; never two batches at once.
    pub(crate) fn slow(&self, keys: Vec<ReadKey>) {
        let first = lock(&self.gate).request(keys);
        let Some(mut keys) = first else { return };
        let backend = Arc::clone(&self.backend);
        let results = self.results.clone();
        let gate = Arc::clone(&self.gate);
        thread::spawn(move || {
            loop {
                for key in keys {
                    let reading = catch_unwind(AssertUnwindSafe(|| backend.read(key))).unwrap_or_else(|p| {
                        super::data::SlowReading {
                            key,
                            value: Err(format!("internal error: {}", panic_text(&*p))),
                        }
                    });
                    if results.send(Msg::Slow(reading)).is_err() {
                        return;
                    }
                }
                match lock(&gate).finished() {
                    Some(next) => keys = next,
                    None => return,
                }
            }
        });
    }

    /// Ends the dashboard reader.
    pub(crate) fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.wake.send(());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::Barrier;

    use super::*;
    use crate::tui::backend::{Catalog, Done, Op};
    use crate::tui::data::{Fast, SlowReading};

    fn write(ticket: Ticket, key: &str, op: Op) -> Write {
        Write { ticket, key: key.to_owned(), op }
    }

    #[test]
    fn a_newer_write_for_a_key_replaces_an_older_one_that_has_not_started() {
        let mut q = Queue::default();
        assert!(q.submit(write(1, "kb", Op::UsbCharge(10))).is_empty());
        assert!(q.submit(write(2, "fan", Op::UsbCharge(20))).is_empty());
        assert_eq!(q.submit(write(3, "kb", Op::UsbCharge(30))), vec![1]);
        assert_eq!(q.len(), 2);
        assert_eq!(q.next().map(|w| w.ticket), Some(2), "order of the writes that remain is the order made");
        assert_eq!(q.next().map(|w| w.ticket), Some(3));
        assert!(q.next().is_none());
    }

    #[test]
    fn writes_without_a_key_are_never_coalesced() {
        let mut q = Queue::default();
        for t in 1..=3 {
            assert!(q.submit(write(t, "", Op::ReleaseHold)).is_empty());
        }
        assert_eq!(q.len(), 3);
    }

    #[test]
    fn the_slow_gate_never_overlaps_and_the_newest_request_waits() {
        let mut g = SlowGate::default();
        assert_eq!(g.request(vec![ReadKey::Limit]), Some(vec![ReadKey::Limit]));
        assert_eq!(g.request(vec![ReadKey::Calibration]), None);
        assert_eq!(g.request(vec![ReadKey::UsbCharge]), None, "the newest request replaces the waiting one");
        assert_eq!(g.finished(), Some(vec![ReadKey::UsbCharge]));
        assert_eq!(g.finished(), None);
        assert_eq!(g.request(vec![ReadKey::Rear]), Some(vec![ReadKey::Rear]), "idle again");
    }

    /// A backend that records what it ran and can hold the first write at a gate.
    struct Gated {
        ran: Mutex<Vec<Op>>,
        started: Barrier,
        release: Barrier,
        hold_first: AtomicBool,
        busy: AtomicBool,
        overlapped: AtomicBool,
    }

    impl Backend for Gated {
        fn catalog(&self) -> Catalog {
            Catalog::default()
        }

        fn fast(&self) -> Fast {
            Fast {
                service: None,
                sensors: Err("none".into()),
                fan: Err("none".into()),
                mode: Err("none".into()),
                ac: Err("none".into()),
                battery: None,
            }
        }

        fn read(&self, key: ReadKey) -> SlowReading {
            SlowReading { key, value: Err("none".into()) }
        }

        fn execute(&self, op: &Op) -> Result<Done, String> {
            if self.busy.swap(true, Ordering::SeqCst) {
                self.overlapped.store(true, Ordering::SeqCst);
            }
            if self.hold_first.swap(false, Ordering::SeqCst) {
                self.started.wait();
                self.release.wait();
            }
            if matches!(op, Op::OpenWindow) {
                self.busy.store(false, Ordering::SeqCst);
                panic_for_test();
            }
            lock(&self.ran).push(op.clone());
            self.busy.store(false, Ordering::SeqCst);
            Ok(Done::Unit)
        }
    }

    fn panic_for_test() {
        panic!("kaboom");
    }

    fn gated() -> Arc<Gated> {
        Arc::new(Gated {
            ran: Mutex::new(Vec::new()),
            started: Barrier::new(2),
            release: Barrier::new(2),
            hold_first: AtomicBool::new(true),
            busy: AtomicBool::new(false),
            overlapped: AtomicBool::new(false),
        })
    }

    #[test]
    fn the_worker_runs_writes_in_order_one_at_a_time_and_coalesces_behind_a_busy_write() {
        let backend = gated();
        let (tx, rx) = mpsc::channel();
        let worker = Worker::new(backend.clone(), tx);
        assert!(worker.submit(write(1, "blocker", Op::ReleaseHold)).is_empty());
        backend.started.wait(); // the first write is running and holds the worker
        assert!(worker.submit(write(2, "brightness", Op::UsbCharge(10))).is_empty());
        assert!(worker.submit(write(3, "other", Op::Calibration(true))).is_empty());
        assert_eq!(worker.submit(write(4, "brightness", Op::UsbCharge(30))), vec![2]);
        backend.release.wait();
        assert!(worker.drain(Duration::from_secs(5)));
        assert!(worker.idle());
        let ran = lock(&backend.ran).clone();
        assert_eq!(ran, vec![Op::ReleaseHold, Op::Calibration(true), Op::UsbCharge(30)]);
        assert!(!backend.overlapped.load(Ordering::SeqCst), "writes never overlap");
        let tickets: Vec<Ticket> = rx
            .try_iter()
            .filter_map(|m| match m {
                Msg::Written { ticket, result } => {
                    assert!(result.is_ok());
                    Some(ticket)
                }
                _ => None,
            })
            .collect();
        assert_eq!(tickets, vec![1, 3, 4]);
    }

    #[test]
    fn a_panicking_write_is_reported_and_does_not_kill_the_queue() {
        let backend = gated();
        backend.hold_first.store(false, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        let worker = Worker::new(backend.clone(), tx);
        worker.submit(write(1, "", Op::OpenWindow));
        worker.submit(write(2, "", Op::ReleaseHold));
        assert!(worker.drain(Duration::from_secs(5)));
        let results: Vec<(Ticket, Result<Done, String>)> = rx
            .try_iter()
            .filter_map(|m| match m {
                Msg::Written { ticket, result } => Some((ticket, result)),
                _ => None,
            })
            .collect();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].1, Err("internal error: kaboom".to_owned()));
        assert_eq!(results[1].1, Ok(Done::Unit));
    }

    #[test]
    fn the_worker_restarts_after_going_idle_and_drain_gives_up_at_the_timeout() {
        let backend = gated();
        backend.hold_first.store(false, Ordering::SeqCst);
        let (tx, _rx) = mpsc::channel();
        let worker = Worker::new(backend.clone(), tx);
        assert!(worker.drain(Duration::from_millis(1)), "idle: nothing to wait for");
        worker.submit(write(1, "", Op::ReleaseHold));
        assert!(worker.drain(Duration::from_secs(5)));
        worker.submit(write(2, "", Op::ReleaseHold));
        assert!(worker.drain(Duration::from_secs(5)));
        assert_eq!(lock(&backend.ran).len(), 2);
        // A write that never finishes within the timeout.
        let held = gated();
        let (tx, _rx) = mpsc::channel();
        let worker = Worker::new(held.clone(), tx);
        worker.submit(write(1, "", Op::ReleaseHold));
        held.started.wait();
        assert!(!worker.drain(Duration::from_millis(30)), "gives up while the write holds");
        held.release.wait();
        assert!(worker.drain(Duration::from_secs(5)));
    }

    #[test]
    fn the_readers_deliver_a_fast_reading_at_once_and_each_requested_setting() {
        let backend = gated();
        let (tx, rx) = mpsc::channel();
        let readers = Readers::start(backend, tx);
        let first = rx.recv_timeout(Duration::from_secs(5)).expect("a fast reading");
        assert!(matches!(first, Msg::Fast(_)));
        readers.slow(vec![ReadKey::Limit, ReadKey::Calibration]);
        let mut keys = Vec::new();
        while keys.len() < 2 {
            if let Msg::Slow(r) = rx.recv_timeout(Duration::from_secs(5)).expect("a slow reading") {
                keys.push(r.key);
            }
        }
        assert_eq!(keys, vec![ReadKey::Limit, ReadKey::Calibration]);
        readers.wake();
        let again = rx.recv_timeout(Duration::from_secs(5)).expect("woken");
        assert!(matches!(again, Msg::Fast(_) | Msg::Slow(_)));
        readers.stop();
    }
}
