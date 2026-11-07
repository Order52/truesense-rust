//! Events from the sound system, so the audio watcher sleeps until something changes.
//!
//! The watcher used to ask the sound system "has anything changed?" every second, which started
//! two to four helper programs (`pactl`, `amixer`) per second for ever. `pactl subscribe` prints one
//! line per change of a card, an output, an input or the server, so one resident helper replaces
//! all of those questions: the watcher waits, and runs its (unchanged) check when a line arrives.
//!
//! What stays as it was: the check itself, and a slow fallback pass every `FALLBACK` in case an
//! event never comes (a jack that the sound server does not announce). If the event stream cannot
//! be started, or ends (the sound server restarted), the watcher goes back to its old one-second
//! pace until the stream is up again.
//!
//! Everything outside the process (the helper, the clock of the pauses) is behind [`EventSource`],
//! so the tests drive the whole thing with a scripted stream.
#![forbid(unsafe_code)]

use std::io::{self, BufRead};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use ts_foundation::sync::locked;
use ts_foundation::wake;

/// How long the watcher sleeps between two checks while events arrive. The check runs sooner
/// when an event comes; this is only the safety net for a change nobody announced.
pub(crate) const FALLBACK: Duration = Duration::from_secs(10);
/// The quiet time that must follow the last event of a burst before the watcher looks: a volume
/// slider or a profile switch produces dozens of lines at once.
pub(crate) const QUIET: Duration = Duration::from_millis(400);
/// The pause before the event stream is started again after it ended or failed to start.
const RECONNECT: Duration = Duration::from_secs(5);

/// What a wait for events ended with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wake {
    /// Something changed (and has been quiet for `QUIET`).
    Changed,
    /// The time was up with no event.
    Timeout,
    /// The stop flag is set.
    Stopped,
    /// The event stream started or ended: the caller re-reads whether it can rely on events.
    Health,
}

/// The source of change notices the watcher waits on.
pub trait Events: Send + Sync {
    /// Whether events are flowing right now. Without them the watcher checks every second.
    fn healthy(&self) -> bool;
    /// Waits up to `timeout` for a change, then for `QUIET` after the last one.
    fn wait(&self, timeout: Duration, stop: &AtomicBool) -> Wake;
}

/// An opened event stream: its lines, and a way to end it from another thread.
pub struct Stream {
    /// The lines of text the helper prints.
    pub lines: Box<dyn BufRead + Send>,
    /// Ends the stream: the reader then sees the end of its input.
    pub close: Box<dyn Fn() + Send + Sync>,
}

/// Starts event streams.
pub trait EventSource: Send + Sync + 'static {
    /// Opens a new stream.
    ///
    /// # Errors
    /// The helper could not be started.
    fn open(&self) -> io::Result<Stream>;
}

/// `pactl subscribe`: one line per change in the sound server.
#[derive(Debug, Clone, Copy, Default)]
pub struct PactlSubscribe;

impl EventSource for PactlSubscribe {
    fn open(&self) -> io::Result<Stream> {
        let mut child = Command::new("pactl")
            .arg("subscribe")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdout = child.stdout.take().ok_or_else(|| io::Error::other("pactl subscribe has no output"))?;
        let child: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(Some(child)));
        let killer = child.clone();
        // The helper is reaped when the stream ends, whoever ended it.
        let reaper = child;
        Ok(Stream {
            lines: Box::new(ReapOnEnd { inner: io::BufReader::new(stdout), child: reaper }),
            close: Box::new(move || {
                if let Some(c) = locked(&killer).as_mut() {
                    let _ = c.kill();
                }
            }),
        })
    }
}

/// A reader that waits for its helper process when the stream is over, so none is left a zombie.
struct ReapOnEnd {
    inner: io::BufReader<std::process::ChildStdout>,
    child: Arc<Mutex<Option<Child>>>,
}

impl io::Read for ReapOnEnd {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        io::Read::read(&mut self.inner, buf)
    }
}

impl BufRead for ReapOnEnd {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.inner.fill_buf()
    }

    fn consume(&mut self, amount: usize) {
        self.inner.consume(amount);
    }
}

impl Drop for ReapOnEnd {
    fn drop(&mut self) {
        if let Some(mut c) = locked(&self.child).take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Whether a `pactl subscribe` line is about something the watcher cares about: a card, an output,
/// an input or the server (default device). Streams (`sink-input`, `source-output`), clients and
/// modules come and go with every application and mean nothing for the microphone policy.
pub fn relevant(line: &str) -> bool {
    let Some((_, what)) = line.split_once(" on ") else { return false };
    matches!(what.split_whitespace().next(), Some("card" | "sink" | "source" | "server"))
}

/// State shared between the reader thread and the waiter.
struct Shared {
    pending: AtomicBool,
    alive: AtomicBool,
    closing: AtomicBool,
    close: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

/// The event stream of a sound server, kept running by a reader thread that restarts it.
pub struct Subscription {
    shared: Arc<Shared>,
    reader: Option<JoinHandle<()>>,
}

impl Subscription {
    /// Starts reading events from `source`. The reader thread blocks on the helper's output (it is
    /// not scheduled while nothing happens) and has a small stack.
    pub fn start(source: Arc<dyn EventSource>) -> Self {
        let shared = Arc::new(Shared {
            pending: AtomicBool::new(false),
            alive: AtomicBool::new(false),
            closing: AtomicBool::new(false),
            close: Mutex::new(None),
        });
        let thread_shared = shared.clone();
        let reader = std::thread::Builder::new()
            .name("audio-events".into())
            .stack_size(64 * 1024)
            .spawn(move || read_loop(source.as_ref(), &thread_shared))
            .ok();
        Self { shared, reader }
    }
}

fn read_loop(source: &dyn EventSource, shared: &Shared) {
    while !shared.closing.load(Ordering::SeqCst) {
        if let Ok(stream) = source.open() {
            *locked(&shared.close) = Some(stream.close);
            // `close` may have been asked for while the helper was starting.
            if shared.closing.load(Ordering::SeqCst) {
                if let Some(close) = locked(&shared.close).as_ref() {
                    close();
                }
            }
            shared.alive.store(true, Ordering::SeqCst);
            wake::ring();
            for line in stream.lines.lines() {
                let Ok(line) = line else { break };
                if relevant(&line) {
                    shared.pending.store(true, Ordering::SeqCst);
                    wake::ring();
                }
            }
            shared.alive.store(false, Ordering::SeqCst);
            wake::ring();
        }
        wake::wait_until(RECONNECT, || shared.closing.load(Ordering::SeqCst));
    }
}

impl Events for Subscription {
    fn healthy(&self) -> bool {
        self.shared.alive.load(Ordering::SeqCst)
    }

    fn wait(&self, timeout: Duration, stop: &AtomicBool) -> Wake {
        let shared = &self.shared;
        let entered = shared.alive.load(Ordering::SeqCst);
        let stopped = || stop.load(Ordering::SeqCst);
        let first = wake::wait_until(timeout, || {
            stopped()
                || shared.pending.swap(false, Ordering::SeqCst)
                || shared.alive.load(Ordering::SeqCst) != entered
        });
        if stopped() {
            return Wake::Stopped;
        }
        if !first {
            return Wake::Timeout;
        }
        if shared.alive.load(Ordering::SeqCst) != entered {
            return Wake::Health;
        }
        // An event: wait until the burst is over.
        while wake::wait_until(QUIET, || stopped() || shared.pending.swap(false, Ordering::SeqCst)) {
            if stopped() {
                return Wake::Stopped;
            }
        }
        Wake::Changed
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.shared.closing.store(true, Ordering::SeqCst);
        if let Some(close) = locked(&self.shared.close).as_ref() {
            close();
        }
        wake::ring();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::mpsc;
    use std::time::Instant;

    use super::*;

    #[test]
    fn only_changes_of_cards_outputs_inputs_and_the_server_count() {
        for line in [
            "Event 'change' on card #47",
            "Event 'new' on sink #51",
            "Event 'remove' on source #12",
            "Event 'change' on server #4294967295",
        ] {
            assert!(relevant(line), "{line}");
        }
        for line in [
            "Event 'new' on sink-input #120",
            "Event 'remove' on source-output #7",
            "Event 'new' on client #5",
            "Event 'new' on module #9",
            "",
            "Event 'change'",
            "garbage on",
        ] {
            assert!(!relevant(line), "{line}");
        }
    }

    /// A stream the test feeds line by line; it ends when the sender is dropped or `close` runs.
    struct Scripted {
        feeds: Mutex<Vec<mpsc::Receiver<String>>>,
        opened: Arc<std::sync::atomic::AtomicUsize>,
    }

    struct ChannelLines(mpsc::Receiver<String>, Vec<u8>);

    impl io::Read for ChannelLines {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            let n = {
                let available = io::BufRead::fill_buf(self)?;
                let n = available.len().min(out.len());
                out[..n].copy_from_slice(&available[..n]);
                n
            };
            io::BufRead::consume(self, n);
            Ok(n)
        }
    }

    impl BufRead for ChannelLines {
        fn fill_buf(&mut self) -> io::Result<&[u8]> {
            if self.1.is_empty() {
                if let Ok(line) = self.0.recv() {
                    self.1 = format!("{line}\n").into_bytes();
                }
            }
            Ok(&self.1)
        }

        fn consume(&mut self, amount: usize) {
            self.1.drain(..amount);
        }
    }

    impl EventSource for Scripted {
        fn open(&self) -> io::Result<Stream> {
            self.opened.fetch_add(1, Ordering::SeqCst);
            let Some(rx) = self.feeds.lock().unwrap().pop() else {
                return Err(io::Error::other("no more streams"));
            };
            Ok(Stream { lines: Box::new(ChannelLines(rx, Vec::new())), close: Box::new(|| {}) })
        }
    }

    fn scripted() -> (Subscription, mpsc::Sender<String>, Arc<std::sync::atomic::AtomicUsize>) {
        let (tx, rx) = mpsc::channel();
        let opened = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let source = Scripted { feeds: Mutex::new(vec![rx]), opened: opened.clone() };
        (Subscription::start(Arc::new(source)), tx, opened)
    }

    fn until(what: &str, ok: impl Fn() -> bool) {
        let end = Instant::now() + Duration::from_secs(5);
        while !ok() {
            assert!(Instant::now() < end, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_quiet_stream_times_out_and_is_healthy() {
        let (sub, _tx, _) = scripted();
        until("the stream", || sub.healthy());
        let stop = AtomicBool::new(false);
        assert_eq!(sub.wait(Duration::from_millis(50), &stop), Wake::Timeout);
    }

    #[test]
    fn a_burst_of_events_is_one_wake_after_it_goes_quiet() {
        let (sub, tx, _) = scripted();
        until("the stream", || sub.healthy());
        let stop = AtomicBool::new(false);
        tx.send("Event 'new' on sink-input #1".into()).unwrap(); // not relevant
        assert_eq!(sub.wait(Duration::from_millis(60), &stop), Wake::Timeout);
        for _ in 0..5 {
            tx.send("Event 'change' on source #3".into()).unwrap();
        }
        let started = Instant::now();
        assert_eq!(sub.wait(Duration::from_secs(10), &stop), Wake::Changed);
        assert!(started.elapsed() >= QUIET, "the watcher waits for the burst to end");
        assert_eq!(sub.wait(Duration::from_millis(50), &stop), Wake::Timeout, "the burst was one wake");
    }

    #[test]
    fn a_stop_ends_a_long_wait_at_once() {
        let (sub, _tx, _) = scripted();
        until("the stream", || sub.healthy());
        let stop = Arc::new(AtomicBool::new(false));
        let waiter = {
            let stop = stop.clone();
            std::thread::scope(|scope| {
                let handle = scope.spawn(|| {
                    let started = Instant::now();
                    let how = sub.wait(Duration::from_secs(60), &stop);
                    (how, started.elapsed())
                });
                std::thread::sleep(Duration::from_millis(50));
                stop.store(true, Ordering::SeqCst);
                wake::ring();
                handle.join().unwrap()
            })
        };
        assert_eq!(waiter.0, Wake::Stopped);
        assert!(waiter.1 < Duration::from_secs(5));
    }

    #[test]
    fn a_stream_that_ends_is_reported_so_the_watcher_can_poll_again() {
        let (sub, tx, _) = scripted();
        until("the stream", || sub.healthy());
        let stop = AtomicBool::new(false);
        drop(tx); // the helper exited
        assert_eq!(sub.wait(Duration::from_secs(10), &stop), Wake::Health);
        assert!(!sub.healthy());
    }

    #[test]
    fn a_source_that_cannot_start_is_never_healthy() {
        let source = Scripted {
            feeds: Mutex::new(Vec::new()),
            opened: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        let sub = Subscription::start(Arc::new(source));
        std::thread::sleep(Duration::from_millis(50));
        assert!(!sub.healthy());
    }
}
