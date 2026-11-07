//! The live feed: one reader for every open page.
//!
//! While at least one page listens, a single thread asks the commands for a reading about once a
//! second and hands the newest one to every page. A page that cannot keep up only ever misses old
//! readings (the newest wins), so a stalled window never slows the others or grows a queue.
//! Each page also receives "show this page" requests (`gui show`) through the same mailbox, so one
//! connection carries both.
#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use serde_json::Value;

use super::bus::Commands;
use ts_foundation::sync::locked;

/// How many "show" requests a page may have waiting; a stalled page misses further ones rather
/// than holding up the others.
const MAX_PENDING_SHOWS: usize = 4;

/// What a page hears next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Message {
    /// The newest live reading, as compact JSON.
    State(String),
    /// A page id to go to, as a JSON string.
    Show(String),
}

#[derive(Debug, Default)]
struct InboxState {
    latest: Option<String>,
    shows: VecDeque<String>,
}

/// One page's mailbox: the newest reading and a few pending show requests.
#[derive(Debug, Default)]
pub(crate) struct Inbox {
    state: Mutex<InboxState>,
    ready: Condvar,
}

impl Inbox {
    /// Replaces the waiting reading by a newer one.
    fn put_state(&self, json: &str) {
        locked(&self.state).latest = Some(json.to_owned());
        self.ready.notify_all();
    }

    /// Queues a show request; `false` when the page already has too many waiting.
    pub(crate) fn put_show(&self, page_json: &str) -> bool {
        let mut state = locked(&self.state);
        if state.shows.len() >= MAX_PENDING_SHOWS {
            return false;
        }
        state.shows.push_back(page_json.to_owned());
        drop(state);
        self.ready.notify_all();
        true
    }

    fn take(state: &mut InboxState) -> Option<Message> {
        if let Some(page) = state.shows.pop_front() {
            return Some(Message::Show(page));
        }
        state.latest.take().map(Message::State)
    }

    /// The next message, waiting at most `timeout` (a request to show a page comes before a reading).
    pub(crate) fn next(&self, timeout: Duration) -> Option<Message> {
        let mut state = locked(&self.state);
        if let Some(m) = Self::take(&mut state) {
            return Some(m);
        }
        let (mut state, _) = self.ready.wait_timeout(state, timeout).unwrap_or_else(PoisonError::into_inner);
        Self::take(&mut state)
    }
}

/// A flag that can be waited on, so the reader thread sleeps between readings and still stops at once.
#[derive(Debug, Default)]
struct Stopper {
    stopped: Mutex<bool>,
    changed: Condvar,
}

impl Stopper {
    fn stop(&self) {
        *locked(&self.stopped) = true;
        self.changed.notify_all();
    }

    /// Sleeps up to `d`; `true` when stopped meanwhile.
    fn sleep(&self, d: Duration) -> bool {
        let stopped = locked(&self.stopped);
        if *stopped {
            return true;
        }
        let (stopped, _) =
            self.changed.wait_timeout_while(stopped, d, |s| !*s).unwrap_or_else(PoisonError::into_inner);
        *stopped
    }
}

struct Reader {
    stopper: Arc<Stopper>,
}

#[derive(Default)]
struct HubState {
    inboxes: Vec<Arc<Inbox>>,
    reader: Option<Reader>,
    last: Option<String>,
    /// Counts reader generations, so a late reading of a stopped reader is dropped.
    generation: u64,
}

/// Runs the live reading while pages listen and fans it out.
pub(crate) struct Hub {
    commands: Arc<dyn Commands>,
    tick: Duration,
    state: Arc<Mutex<HubState>>,
    closed: Arc<AtomicBool>,
}

impl Hub {
    /// A hub reading `commands` every `tick` while somebody listens.
    pub(crate) fn new(commands: Arc<dyn Commands>, tick: Duration) -> Self {
        Self { commands, tick, state: Arc::default(), closed: Arc::default() }
    }

    /// Registers a page. It gets the latest reading at once, so a new window does not wait a second
    /// for its first values. The first page starts the reader.
    pub(crate) fn subscribe(&self) -> Subscription {
        let inbox = Arc::new(Inbox::default());
        let mut state = locked(&self.state);
        if let Some(last) = &state.last {
            inbox.put_state(last);
        }
        state.inboxes.push(Arc::clone(&inbox));
        if state.reader.is_none() && !self.closed.load(Ordering::SeqCst) {
            state.generation += 1;
            let stopper = Arc::new(Stopper::default());
            state.reader = Some(Reader { stopper: Arc::clone(&stopper) });
            self.spawn_reader(stopper, state.generation);
        }
        Subscription { inbox, state: Arc::clone(&self.state) }
    }

    fn spawn_reader(&self, stopper: Arc<Stopper>, generation: u64) {
        let (commands, state, tick) = (Arc::clone(&self.commands), Arc::clone(&self.state), self.tick);
        // A thread that fails to start only means no live values; the pages stay usable.
        let _ = thread::Builder::new().name("gui-live".to_owned()).spawn(move || {
            loop {
                if let Some(reading) = commands.live() {
                    publish(&state, generation, &reading);
                }
                if stopper.sleep(tick) {
                    return;
                }
            }
        });
    }

    /// Tells every page to go to `page_json` (already a JSON string); how many pages heard it.
    pub(crate) fn show(&self, page_json: &str) -> usize {
        let state = locked(&self.state);
        state.inboxes.iter().filter(|inbox| inbox.put_show(page_json)).count()
    }

    /// The number of pages listening.
    #[cfg(test)]
    pub(crate) fn listeners(&self) -> usize {
        locked(&self.state).inboxes.len()
    }

    /// Stops the reader whatever the pages do (the server is leaving).
    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let mut state = locked(&self.state);
        if let Some(reader) = state.reader.take() {
            reader.stopper.stop();
        }
        state.last = None;
    }
}

fn publish(state: &Mutex<HubState>, generation: u64, reading: &Value) {
    let Ok(json) = serde_json::to_string(reading) else { return };
    let mut state = locked(state);
    if state.generation != generation || state.reader.is_none() {
        return; // a late reading of a reader that was stopped
    }
    for inbox in &state.inboxes {
        inbox.put_state(&json);
    }
    state.last = Some(json);
}

/// A page's registration; dropping it leaves the hub, and the last one stops the reader.
pub(crate) struct Subscription {
    inbox: Arc<Inbox>,
    state: Arc<Mutex<HubState>>,
}

impl Subscription {
    /// The page's mailbox.
    pub(crate) fn inbox(&self) -> &Inbox {
        &self.inbox
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let mut state = locked(&self.state);
        state.inboxes.retain(|i| !Arc::ptr_eq(i, &self.inbox));
        if state.inboxes.is_empty() {
            if let Some(reader) = state.reader.take() {
                reader.stopper.stop();
            }
            state.last = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::bus::{CommandError, Commands};
    use std::sync::atomic::AtomicUsize;

    /// Counts how often it is read and answers an increasing number.
    struct Counting {
        reads: AtomicUsize,
        silent: bool,
    }

    impl Commands for Counting {
        fn run(&self, area: &str, verb: &str, _args: &Value) -> Result<Value, CommandError> {
            Err(CommandError::unknown(area, verb))
        }

        fn live(&self) -> Option<Value> {
            let n = self.reads.fetch_add(1, Ordering::SeqCst);
            (!self.silent).then(|| serde_json::json!({ "n": n }))
        }
    }

    fn hub(silent: bool) -> (Hub, Arc<Counting>) {
        let c = Arc::new(Counting { reads: AtomicUsize::new(0), silent });
        (Hub::new(c.clone(), Duration::from_millis(5)), c)
    }

    fn state_n(m: Option<Message>) -> Option<i64> {
        match m {
            Some(Message::State(json)) => serde_json::from_str::<Value>(&json).ok()?.get("n")?.as_i64(),
            _ => None,
        }
    }

    #[test]
    fn nothing_is_read_while_nobody_listens_and_reading_stops_with_the_last_page() {
        let (hub, counting) = hub(false);
        thread::sleep(Duration::from_millis(30));
        assert_eq!(counting.reads.load(Ordering::SeqCst), 0);
        let sub = hub.subscribe();
        assert!(state_n(sub.inbox().next(Duration::from_secs(2))).is_some());
        assert_eq!(hub.listeners(), 1);
        drop(sub);
        assert_eq!(hub.listeners(), 0);
        thread::sleep(Duration::from_millis(40));
        let after = counting.reads.load(Ordering::SeqCst);
        thread::sleep(Duration::from_millis(60));
        assert_eq!(counting.reads.load(Ordering::SeqCst), after, "the reader stopped");
    }

    #[test]
    fn a_slow_page_only_misses_old_readings() {
        let (hub, _) = hub(false);
        let sub = hub.subscribe();
        thread::sleep(Duration::from_millis(80));
        let first = state_n(sub.inbox().next(Duration::from_secs(2)));
        assert!(first.is_some_and(|n| n >= 3), "got the newest reading, not the oldest: {first:?}");
    }

    #[test]
    fn a_new_page_gets_the_latest_reading_at_once() {
        let (hub, counting) = hub(false);
        let first = hub.subscribe();
        assert!(first.inbox().next(Duration::from_secs(2)).is_some());
        let second = hub.subscribe();
        assert!(second.inbox().next(Duration::from_millis(1)).is_some(), "no wait for a tick");
        assert_eq!(hub.listeners(), 2);
        assert!(counting.reads.load(Ordering::SeqCst) >= 1);
    }

    #[test]
    fn a_commands_that_has_no_reading_leaves_the_page_waiting_not_failing() {
        let (hub, counting) = hub(true);
        let sub = hub.subscribe();
        assert_eq!(sub.inbox().next(Duration::from_millis(40)), None);
        assert!(counting.reads.load(Ordering::SeqCst) >= 2, "it keeps trying");
    }

    #[test]
    fn show_requests_reach_every_page_and_come_before_readings() {
        let (hub, _) = hub(false);
        let (a, b) = (hub.subscribe(), hub.subscribe());
        thread::sleep(Duration::from_millis(30));
        assert_eq!(hub.show("\"curves\""), 2);
        assert_eq!(a.inbox().next(Duration::from_secs(1)), Some(Message::Show("\"curves\"".to_owned())));
        assert_eq!(b.inbox().next(Duration::from_secs(1)), Some(Message::Show("\"curves\"".to_owned())));
        assert!(matches!(a.inbox().next(Duration::from_secs(1)), Some(Message::State(_))));
    }

    #[test]
    fn a_stalled_page_misses_show_requests_instead_of_blocking() {
        let (hub, _) = hub(true);
        let sub = hub.subscribe();
        for _ in 0..MAX_PENDING_SHOWS {
            assert_eq!(hub.show("\"home\""), 1);
        }
        assert_eq!(hub.show("\"home\""), 0);
        drop(sub);
    }

    #[test]
    fn closing_stops_the_reader_and_refuses_a_new_one() {
        let (hub, counting) = hub(false);
        let sub = hub.subscribe();
        assert!(sub.inbox().next(Duration::from_secs(2)).is_some());
        hub.close();
        thread::sleep(Duration::from_millis(30));
        let after = counting.reads.load(Ordering::SeqCst);
        let late = hub.subscribe();
        thread::sleep(Duration::from_millis(40));
        assert_eq!(counting.reads.load(Ordering::SeqCst), after);
        drop((sub, late));
    }
}
