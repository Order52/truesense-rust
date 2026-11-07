//! The live store: the newest reading of the machine, plus a short history of a few series for
//! sparklines.
//!
//! Readings are taken on worker threads (a hardware read can take tens of milliseconds, and a page
//! may also ask for an immediate refresh), so two reads can finish out of order. Every read takes a
//! [`Ticket`] **before** it starts; a finished read is accepted only when its ticket is newer than
//! the newest one accepted so far. "The newest reading wins", and an old slow read can neither
//! overwrite a newer frame nor add a stale point to a history.
#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde_json::{Map, Value, json};

use crate::json::f64_at;

/// Samples kept per series: two minutes at the one-second tick.
pub const HISTORY_LEN: usize = 120;

/// The series every page may rely on: dotted paths into a live frame (`COMMANDS.md`, "The live
/// stream"). Pages that need another one call [`LiveStore::track`].
pub const DEFAULT_SERIES: &[&str] = &[
    "sensors.cpu_temp",
    "sensors.gpu_temp",
    "sensors.sys_temp",
    "sensors.cpu_fan_rpm",
    "sensors.gpu_fan_rpm",
    "load.cpu_pct",
    "load.gpu_pct",
    "power.cpu_w",
    "power.gpu_w",
    "battery_pct",
];

/// The right to deliver one reading; taken before the read starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ticket(u64);

struct Inner {
    newest: u64,
    accepted: u64,
    frame: Option<Value>,
    cap: usize,
    series: Vec<(String, VecDeque<Option<f64>>)>,
}

/// Newest frame and the history ring buffers; shared between the poller and the readers.
pub struct LiveStore {
    next: AtomicU64,
    inner: Mutex<Inner>,
}

impl LiveStore {
    /// A store keeping `cap` samples of each of [`DEFAULT_SERIES`].
    pub fn new(cap: usize) -> Self {
        let cap = cap.max(1);
        let series = DEFAULT_SERIES.iter().map(|p| ((*p).to_owned(), VecDeque::with_capacity(cap))).collect();
        Self {
            next: AtomicU64::new(1),
            inner: Mutex::new(Inner { newest: 0, accepted: 0, frame: None, cap, series }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The ticket for a read that is about to start.
    pub fn ticket(&self) -> Ticket {
        Ticket(self.next.fetch_add(1, Ordering::SeqCst))
    }

    /// Offers a finished reading. Returns whether it was accepted (newer than every accepted one).
    /// A missing value (`null`, absent) in a tracked series is stored as a gap so the series stay
    /// aligned in time.
    pub fn push(&self, ticket: Ticket, frame: Value) -> bool {
        let mut inner = self.lock();
        if ticket.0 <= inner.newest {
            return false;
        }
        inner.newest = ticket.0;
        inner.accepted += 1;
        let cap = inner.cap;
        for (path, ring) in &mut inner.series {
            if ring.len() == cap {
                ring.pop_front();
            }
            ring.push_back(f64_at(&frame, path));
        }
        inner.frame = Some(frame);
        true
    }

    /// Starts keeping a history for `path` (no effect when it is kept already). The new series
    /// starts empty.
    pub fn track(&self, path: &str) {
        let mut inner = self.lock();
        if !inner.series.iter().any(|(p, _)| p == path) {
            let cap = inner.cap;
            inner.series.push((path.to_owned(), VecDeque::with_capacity(cap)));
        }
    }

    /// The newest accepted frame.
    pub fn frame(&self) -> Option<Value> {
        self.lock().frame.clone()
    }

    /// How many readings were accepted so far.
    pub fn accepted(&self) -> u64 {
        self.lock().accepted
    }

    /// The history of `path`, oldest first, gaps as `None`; empty for an untracked path.
    pub fn history(&self, path: &str) -> Vec<Option<f64>> {
        self.lock()
            .series
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, r)| r.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Everything a page needs for one repaint as one JSON object:
    /// `{"n": accepted, "frame": {...}, "history": {"sensors.cpu_temp": [54, null, 55], ...}}`.
    /// Pages parse it once per tick instead of calling back for each series.
    pub fn snapshot(&self) -> Value {
        let inner = self.lock();
        let history: Map<String, Value> =
            inner.series.iter().map(|(p, r)| (p.clone(), json!(r.iter().collect::<Vec<_>>()))).collect();
        json!({ "n": inner.accepted, "frame": inner.frame, "history": history })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn frame(cpu: i64) -> Value {
        json!({"sensors": {"cpu_temp": cpu, "gpu_temp": null}, "battery_pct": 80})
    }

    #[test]
    fn newest_reading_wins_even_when_an_older_one_finishes_last() {
        let s = LiveStore::new(10);
        let slow = s.ticket();
        let fast = s.ticket();
        assert!(s.push(fast, frame(60)));
        assert!(!s.push(slow, frame(50)), "the older read finished later and must be dropped");
        assert_eq!(s.frame(), Some(frame(60)));
        assert_eq!(s.history("sensors.cpu_temp"), vec![Some(60.0)]);
        assert_eq!(s.accepted(), 1);
    }

    #[test]
    fn a_ticket_cannot_be_accepted_twice() {
        let s = LiveStore::new(10);
        let t = s.ticket();
        assert!(s.push(t, frame(1)));
        assert!(!s.push(t, frame(2)));
        assert_eq!(s.frame(), Some(frame(1)));
    }

    #[test]
    fn history_is_a_ring_of_the_given_capacity() {
        let s = LiveStore::new(3);
        for v in 1..=5 {
            assert!(s.push(s.ticket(), frame(v)));
        }
        assert_eq!(s.history("sensors.cpu_temp"), vec![Some(3.0), Some(4.0), Some(5.0)]);
        assert_eq!(s.accepted(), 5);
    }

    #[test]
    fn missing_values_are_gaps_and_series_stay_aligned() {
        let s = LiveStore::new(10);
        s.push(s.ticket(), frame(50));
        s.push(s.ticket(), json!({"sensors": {"gpu_temp": 40}}));
        assert_eq!(s.history("sensors.cpu_temp"), vec![Some(50.0), None]);
        assert_eq!(s.history("sensors.gpu_temp"), vec![None, Some(40.0)]);
        assert_eq!(s.history("battery_pct"), vec![Some(80.0), None]);
    }

    #[test]
    fn tracking_a_new_series_starts_empty_and_is_idempotent() {
        let s = LiveStore::new(5);
        s.push(s.ticket(), json!({"extra": {"x": 1}}));
        assert!(s.history("extra.x").is_empty());
        s.track("extra.x");
        s.track("extra.x");
        s.push(s.ticket(), json!({"extra": {"x": 2}}));
        assert_eq!(s.history("extra.x"), vec![Some(2.0)]);
        assert!(s.history("never.tracked").is_empty());
    }

    #[test]
    fn snapshot_is_one_json_object_with_frame_and_histories() {
        let s = LiveStore::new(5);
        assert_eq!(s.snapshot()["frame"], Value::Null);
        s.push(s.ticket(), frame(54));
        let snap = s.snapshot();
        assert_eq!(snap["n"], json!(1));
        assert_eq!(snap["frame"]["sensors"]["cpu_temp"], json!(54));
        assert_eq!(snap["history"]["sensors.cpu_temp"], json!([54.0]));
        assert_eq!(snap["history"]["sensors.gpu_temp"], json!([null]));
    }

    #[test]
    fn zero_capacity_is_treated_as_one() {
        let s = LiveStore::new(0);
        s.push(s.ticket(), frame(1));
        s.push(s.ticket(), frame(2));
        assert_eq!(s.history("sensors.cpu_temp"), vec![Some(2.0)]);
    }
}
