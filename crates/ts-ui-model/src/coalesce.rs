//! Write coalescing: a slider that is dragged sends dozens of values a second, the hardware can
//! take a few. Writes are keyed by the control they belong to (`"fan.cpu"`, `"keyboard.brightness"`);
//! a newer write for a key replaces an older one that has **not started yet**, so the machine ends
//! on the value the person stopped at and never works through the backlog.
//!
//! [`Pending`] is the pure data structure (testable without threads); [`Coalescer`] wraps it with a
//! mutex and a condition variable so one worker thread can wait for the next write.
#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

/// Unstarted writes in the order the person made their latest change to each key.
///
/// A replacement moves to the **back**: with writes A1, B1, A2 the machine sees B1 then A2, so the
/// last thing the person touched is also the last thing written (a fan slider must not be applied
/// after a later power-mode change that the person made after it).
#[derive(Debug)]
pub struct Pending<T> {
    queue: VecDeque<(String, T)>,
}

impl<T> Default for Pending<T> {
    fn default() -> Self {
        Self { queue: VecDeque::new() }
    }
}

impl<T> Pending<T> {
    /// An empty queue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `item` for `key`. Returns the older, unstarted item it replaced, so the caller can tell
    /// whoever waited for it that it was superseded.
    pub fn push(&mut self, key: &str, item: T) -> Option<T> {
        let replaced = self.queue.iter().position(|(k, _)| k == key).and_then(|i| self.queue.remove(i));
        self.queue.push_back((key.to_owned(), item));
        replaced.map(|(_, old)| old)
    }

    /// The oldest unstarted write.
    pub fn pop(&mut self) -> Option<(String, T)> {
        self.queue.pop_front()
    }

    /// How many writes are waiting.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Whether nothing is waiting.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

/// [`Pending`] shared between the threads that submit writes and the one that performs them.
pub struct Coalescer<T> {
    state: Mutex<State<T>>,
    wake: Condvar,
}

struct State<T> {
    pending: Pending<T>,
    closed: bool,
}

impl<T> Default for Coalescer<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Coalescer<T> {
    /// An open, empty queue.
    pub fn new() -> Self {
        Self { state: Mutex::new(State { pending: Pending::new(), closed: false }), wake: Condvar::new() }
    }

    /// A poisoned lock only means another thread panicked while holding it; the queue itself is
    /// always consistent (every operation is a single `VecDeque` call), so carry on.
    fn lock(&self) -> MutexGuard<'_, State<T>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queues a write; returns what it replaced. After [`Self::close`] the item is handed back as
    /// the "replaced" value, so its owner is still told it never ran.
    pub fn submit(&self, key: &str, item: T) -> Option<T> {
        let mut state = self.lock();
        if state.closed {
            return Some(item);
        }
        let replaced = state.pending.push(key, item);
        self.wake.notify_one();
        replaced
    }

    /// Blocks until a write is waiting and returns it; `None` once the queue is closed and empty.
    pub fn next(&self) -> Option<(String, T)> {
        let mut state = self.lock();
        loop {
            if let Some(job) = state.pending.pop() {
                return Some(job);
            }
            if state.closed {
                return None;
            }
            state = self.wake.wait(state).unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Stops accepting writes and wakes the worker; writes already queued are still handed out.
    pub fn close(&self) {
        self.lock().closed = true;
        self.wake.notify_all();
    }

    /// How many writes are waiting.
    pub fn len(&self) -> usize {
        self.lock().pending.len()
    }

    /// Whether nothing is waiting.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn newer_write_for_the_same_key_replaces_an_unstarted_one() {
        let mut p = Pending::new();
        assert_eq!(p.push("fan", 20), None);
        assert_eq!(p.push("fan", 30), Some(20));
        assert_eq!(p.push("fan", 40), Some(30));
        assert_eq!(p.len(), 1);
        assert_eq!(p.pop(), Some(("fan".to_owned(), 40)));
        assert!(p.is_empty());
    }

    #[test]
    fn different_keys_do_not_interfere_and_keep_latest_intent_order() {
        let mut p = Pending::new();
        p.push("a", 1);
        p.push("b", 1);
        assert_eq!(p.push("a", 2), Some(1));
        assert_eq!(p.pop(), Some(("b".to_owned(), 1)));
        assert_eq!(p.pop(), Some(("a".to_owned(), 2)));
        assert_eq!(p.pop(), None);
    }

    #[test]
    fn a_started_write_is_not_replaced_the_next_one_waits() {
        let mut p = Pending::new();
        p.push("fan", 1);
        let started = p.pop();
        assert_eq!(started, Some(("fan".to_owned(), 1)));
        assert_eq!(p.push("fan", 2), None, "the running write is no longer pending");
        assert_eq!(p.pop(), Some(("fan".to_owned(), 2)));
    }

    #[test]
    fn worker_blocks_until_a_write_arrives_and_stops_on_close() {
        let c = Arc::new(Coalescer::new());
        let worker = {
            let c = c.clone();
            thread::spawn(move || {
                let mut seen = Vec::new();
                while let Some((k, v)) = c.next() {
                    seen.push((k, v));
                }
                seen
            })
        };
        thread::sleep(Duration::from_millis(20));
        c.submit("k", 1);
        thread::sleep(Duration::from_millis(20));
        c.submit("k", 2);
        c.close();
        let seen = worker.join().unwrap();
        assert!(!seen.is_empty() && seen.last() == Some(&("k".to_owned(), 2)), "{seen:?}");
    }

    #[test]
    fn submit_after_close_hands_the_item_back() {
        let c = Coalescer::new();
        c.close();
        assert_eq!(c.submit("k", 7), Some(7));
        assert!(c.is_empty());
        assert_eq!(c.next(), None);
    }

    #[test]
    fn queued_writes_are_delivered_after_close() {
        let c = Coalescer::new();
        c.submit("a", 1);
        c.close();
        assert_eq!(c.next(), Some(("a".to_owned(), 1)));
        assert_eq!(c.next(), None);
    }
}
