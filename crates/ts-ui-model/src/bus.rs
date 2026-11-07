//! The command bus as the UI sees it: asynchronous calls over [`ts_gui::gui::Commands`].
//!
//! `Commands` is the one contract between a front end and the machine (`COMMANDS.md`): the web
//! window and the Qt app both go through it, so every refusal and safety rule below it stays in one
//! place. This module adds what a GUI needs on top and a library has no business knowing:
//!
//! * [`Bus::run`]: a command on its own thread, the reply delivered through a callback, so the GUI
//!   thread never waits for hardware (a `selftest run` may take minutes);
//! * [`Bus::write`]: keyed, **ordered and coalesced** writes for sliders and colour pickers (see
//!   [`crate::coalesce`]): one writer thread performs them one at a time, newest value per control;
//! * [`Reply`]: success or failure as plain data (`message` and optional `code`), failure text
//!   unchanged because the contract says it is written for a person.
//!
//! Nothing here knows Qt: the bridge turns a [`Reply`] into a signal.
#![forbid(unsafe_code)]

use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use serde_json::Value;
use ts_gui::gui::{CommandError, Commands};

use crate::coalesce::Coalescer;
use crate::json;

/// Error code of a write that a newer write for the same control replaced before it started.
/// Callers treat it as "nothing to report": the newer write will answer.
pub(crate) const CODE_SUPERSEDED: &str = "superseded";
/// Error code of a command the bus does not have (the page asked for something that is not in
/// `COMMANDS.md`).
pub(crate) const CODE_UNKNOWN: &str = "unknown-command";
/// Error code of a command that did not finish in the time the bus allows.
pub(crate) const CODE_TIMEOUT: &str = "timeout";
/// Error code of arguments that are not a JSON object.
pub const CODE_BAD_ARGS: &str = "bad-args";
/// Error code of a command that panicked (a bug below the bus; the app stays alive and says so).
pub(crate) const CODE_INTERNAL: &str = "internal";

/// One command: area and verb are the command line's names (`power set`), `args` a JSON object.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// The area (`power`, `fan`, `keyboard`, ...).
    pub area: String,
    /// The verb (`set`, `status`, ...).
    pub verb: String,
    /// The arguments, always a JSON object.
    pub args: Value,
}

impl Request {
    /// A request from the three texts QML passes.
    ///
    /// # Errors
    /// [`CmdError`] with [`CODE_BAD_ARGS`] when `args_json` is not a JSON object.
    pub fn parse(area: &str, verb: &str, args_json: &str) -> Result<Self, CmdError> {
        let args = json::parse_args(args_json).map_err(|m| CmdError::new(m, Some(CODE_BAD_ARGS)))?;
        Ok(Self { area: area.to_owned(), verb: verb.to_owned(), args })
    }
}

/// Why a command failed: the text to show as it is, and a short code when the page can do
/// something more useful than print it (`needs-daemon`, `no-permission`, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdError {
    /// Written for a person; never reworded by the UI.
    pub message: String,
    /// Machine-readable reason, if any.
    pub code: Option<String>,
}

impl CmdError {
    /// An error with `message` and an optional `code`.
    pub fn new(message: impl Into<String>, code: Option<&str>) -> Self {
        Self { message: message.into(), code: code.map(str::to_owned) }
    }

    /// Whether this is the "a newer write replaced me" answer.
    pub fn is_superseded(&self) -> bool {
        self.code.as_deref() == Some(CODE_SUPERSEDED)
    }
}

impl From<CommandError> for CmdError {
    fn from(e: CommandError) -> Self {
        let code = if e.is_unknown() {
            Some(CODE_UNKNOWN)
        } else if e.is_timeout() {
            Some(CODE_TIMEOUT)
        } else {
            e.code()
        };
        Self::new(e.message(), code)
    }
}

/// The answer to one call, delivered once.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    /// The id [`Bus::run`] or [`Bus::write`] returned for the call.
    pub id: u64,
    /// The command's JSON result, or why it failed.
    pub outcome: Result<Value, CmdError>,
}

impl Reply {
    /// `(ok, json)`: the result serialised on success, `{"message", "code"?}` on failure. This is
    /// the shape the QML side decodes.
    pub fn to_json(&self) -> (bool, String) {
        match &self.outcome {
            Ok(v) => (true, v.to_string()),
            Err(e) => (false, json::error_object(&e.message, e.code.as_deref()).to_string()),
        }
    }
}

/// Where a [`Reply`] goes. Called exactly once, on a worker thread (or on the calling thread when
/// the call was refused before it started).
pub type Done = Box<dyn FnOnce(Reply) + Send + 'static>;

struct WriteJob {
    id: u64,
    request: Request,
    done: Done,
}

/// The asynchronous front of a [`Commands`] implementation.
pub struct Bus {
    commands: Arc<dyn Commands>,
    ids: AtomicU64,
    writes: Arc<Coalescer<WriteJob>>,
}

impl Bus {
    /// A bus over `commands`; starts the writer thread.
    ///
    /// # Errors
    /// The operating system could not start the writer thread.
    pub fn new(commands: Arc<dyn Commands>) -> io::Result<Self> {
        let writes: Arc<Coalescer<WriteJob>> = Arc::new(Coalescer::new());
        let worker_writes = writes.clone();
        let worker_commands = commands.clone();
        thread::Builder::new().name("ts-bus-writer".to_owned()).spawn(move || {
            while let Some((_key, job)) = worker_writes.next() {
                let outcome = execute(worker_commands.as_ref(), &job.request);
                (job.done)(Reply { id: job.id, outcome });
            }
        })?;
        Ok(Self { commands, ids: AtomicU64::new(1), writes })
    }

    fn next_id(&self) -> u64 {
        self.ids.fetch_add(1, Ordering::Relaxed)
    }

    /// Runs `request` on a thread of its own and calls `done` with the answer. Use it for reads
    /// and for one-shot actions (buttons, tiles, dialogs). Returns the call's id.
    ///
    /// Commands for different controls may overlap; use [`Self::write`] for anything that a drag
    /// or a key repeat can issue many times.
    pub fn run(&self, request: Request, done: Done) -> u64 {
        let id = self.next_id();
        let commands = self.commands.clone();
        let (slot, thread_slot) = shared_once(done);
        let spawned = thread::Builder::new().name("ts-bus-cmd".to_owned()).spawn(move || {
            if let Some(done) = thread_slot.take() {
                let outcome = execute(commands.as_ref(), &request);
                done(Reply { id, outcome });
            }
        });
        if let Err(e) = spawned {
            if let Some(done) = slot.take() {
                let error = CmdError::new(format!("cannot start a worker thread: {e}"), Some(CODE_INTERNAL));
                done(Reply { id, outcome: Err(error) });
            }
        }
        id
    }

    /// Answers a call that was refused before it could start (arguments that are not an object):
    /// it still gets an id and `done` is called, so the caller handles one path for every outcome.
    pub fn refuse(&self, error: CmdError, done: Done) -> u64 {
        let id = self.next_id();
        done(Reply { id, outcome: Err(error) });
        id
    }

    /// Queues `request` as the write for control `key`. A write for the same key that has not
    /// started is replaced (its `done` gets [`CODE_SUPERSEDED`] at once); writes run one at a
    /// time in the order of the person's latest change per key. Returns the call's id.
    pub fn write(&self, key: &str, request: Request, done: Done) -> u64 {
        let id = self.next_id();
        if let Some(replaced) = self.writes.submit(key, WriteJob { id, request, done }) {
            let error = CmdError::new("replaced by a newer value", Some(CODE_SUPERSEDED));
            (replaced.done)(Reply { id: replaced.id, outcome: Err(error) });
        }
        id
    }

    /// One reading of the live state (`None`: could not be read this time). Blocks for as long as
    /// the hardware reads take, so call it from a worker thread, never the GUI thread.
    pub fn live(&self) -> Option<Value> {
        catch_unwind(AssertUnwindSafe(|| self.commands.live())).unwrap_or(None)
    }

    /// How many writes are waiting (for tests and the status line).
    pub fn pending_writes(&self) -> usize {
        self.writes.len()
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        self.writes.close();
    }
}

/// Runs one command, turning a panic below the bus into an error reply: a bug must not take the
/// window down, and the person should be told something failed.
fn execute(commands: &dyn Commands, request: &Request) -> Result<Value, CmdError> {
    match catch_unwind(AssertUnwindSafe(|| commands.run(&request.area, &request.verb, &request.args))) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(e.into()),
        Err(_) => Err(CmdError::new(
            format!("internal error while running {} {}", request.area, request.verb),
            Some(CODE_INTERNAL),
        )),
    }
}

/// A value two owners can race for; exactly one `take` gets it. Needed because a thread that
/// failed to start has already moved the callback into a closure that never runs.
fn shared_once<T>(value: T) -> (Once<T>, Once<T>) {
    let cell = Arc::new(std::sync::Mutex::new(Some(value)));
    (Once(cell.clone()), Once(cell))
}

struct Once<T>(Arc<std::sync::Mutex<Option<T>>>);

impl<T> Once<T> {
    fn take(&self) -> Option<T> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::time::Duration;

    /// A scripted machine: records every command; `fan set` blocks until the test releases it.
    struct Fake {
        log: Mutex<Vec<String>>,
        gate: Mutex<Option<Receiver<()>>>,
        entered: Mutex<Option<Sender<()>>>,
    }

    impl Fake {
        fn new() -> Arc<Self> {
            Arc::new(Self { log: Mutex::new(Vec::new()), gate: Mutex::new(None), entered: Mutex::new(None) })
        }
        fn log(&self) -> Vec<String> {
            self.log.lock().map(|l| l.clone()).unwrap_or_default()
        }
    }

    impl Commands for Fake {
        fn run(&self, area: &str, verb: &str, args: &Value) -> Result<Value, CommandError> {
            self.log.lock().map(|mut l| l.push(format!("{area} {verb} {args}"))).ok();
            match (area, verb) {
                ("boom", _) => std::panic::panic_any("test panic"),
                ("fan", "set") => {
                    if let Some(tx) = self.entered.lock().ok().and_then(|mut e| e.take()) {
                        tx.send(()).ok();
                    }
                    if let Some(rx) = self.gate.lock().ok().and_then(|mut g| g.take()) {
                        rx.recv_timeout(Duration::from_secs(5)).ok();
                    }
                    Ok(json!("done"))
                }
                ("status", "get") => Ok(json!({"mode": "balanced"})),
                ("daemon", "needed") => {
                    Err(CommandError::failed("service not running").with_code("needs-daemon"))
                }
                ("nope", _) => Err(CommandError::unknown(area, verb)),
                _ => Err(CommandError::failed(format!("refused: {area} {verb}"))),
            }
        }
        fn live(&self) -> Option<Value> {
            Some(json!({"t": 1}))
        }
    }

    fn collect() -> (Done, Receiver<Reply>) {
        let (tx, rx) = mpsc::channel();
        (Box::new(move |r| drop(tx.send(r))), rx)
    }

    fn req(area: &str, verb: &str, args: Value) -> Request {
        Request { area: area.to_owned(), verb: verb.to_owned(), args }
    }

    fn wait(rx: &Receiver<Reply>) -> Reply {
        rx.recv_timeout(Duration::from_secs(5)).expect("a reply")
    }

    #[test]
    fn run_delivers_the_result_with_the_calls_id() {
        let bus = Bus::new(Fake::new()).expect("bus");
        let (done, rx) = collect();
        let id = bus.run(req("status", "get", json!({})), done);
        let reply = wait(&rx);
        assert_eq!(reply.id, id);
        assert_eq!(reply.outcome, Ok(json!({"mode": "balanced"})));
        assert_eq!(reply.to_json(), (true, r#"{"mode":"balanced"}"#.to_owned()));
    }

    #[test]
    fn refusals_keep_their_text_verbatim_and_codes_survive() {
        let bus = Bus::new(Fake::new()).expect("bus");
        let (done, rx) = collect();
        bus.run(req("fan", "off", json!({})), done);
        assert_eq!(wait(&rx).outcome, Err(CmdError::new("refused: fan off", None)));

        let (done, rx) = collect();
        bus.run(req("daemon", "needed", json!({})), done);
        let reply = wait(&rx);
        assert_eq!(reply.outcome, Err(CmdError::new("service not running", Some("needs-daemon"))));
        assert_eq!(
            reply.to_json(),
            (false, r#"{"message":"service not running","code":"needs-daemon"}"#.to_owned())
        );

        let (done, rx) = collect();
        bus.run(req("nope", "x", json!({})), done);
        assert_eq!(wait(&rx).outcome.err().and_then(|e| e.code), Some(CODE_UNKNOWN.to_owned()));
    }

    #[test]
    fn a_panic_below_the_bus_becomes_an_internal_error() {
        let bus = Bus::new(Fake::new()).expect("bus");
        let (done, rx) = collect();
        bus.run(req("boom", "x", json!({})), done);
        let err = wait(&rx).outcome.err();
        assert_eq!(err.as_ref().and_then(|e| e.code.as_deref()), Some(CODE_INTERNAL));
        // The bus is still usable afterwards.
        let (done, rx) = collect();
        bus.run(req("status", "get", json!({})), done);
        assert!(wait(&rx).outcome.is_ok());
    }

    #[test]
    fn writes_for_one_key_collapse_to_the_newest_unstarted_value() {
        let fake = Fake::new();
        let (gate_tx, gate_rx) = mpsc::channel();
        let (entered_tx, entered_rx) = mpsc::channel();
        if let (Ok(mut g), Ok(mut e)) = (fake.gate.lock(), fake.entered.lock()) {
            *g = Some(gate_rx);
            *e = Some(entered_tx);
        }
        let bus = Bus::new(fake.clone()).expect("bus");

        let (d1, r1) = collect();
        bus.write("fan", req("fan", "set", json!({"cpu": 30})), d1);
        // The first write is now running (blocked inside the machine), so it cannot be replaced.
        assert!(entered_rx.recv_timeout(Duration::from_secs(5)).is_ok());
        let (d2, r2) = collect();
        let (d3, r3) = collect();
        let (d4, r4) = collect();
        bus.write("fan", req("fan", "set", json!({"cpu": 40})), d2);
        bus.write("fan", req("fan", "set", json!({"cpu": 50})), d3);
        bus.write("fan", req("fan", "set", json!({"cpu": 60})), d4);
        // The two replaced values were answered at once as superseded.
        assert!(wait(&r2).outcome.err().is_some_and(|e| e.is_superseded()));
        assert!(wait(&r3).outcome.err().is_some_and(|e| e.is_superseded()));
        assert_eq!(bus.pending_writes(), 1);

        gate_tx.send(()).ok();
        assert_eq!(wait(&r1).outcome, Ok(json!("done")));
        assert_eq!(wait(&r4).outcome, Ok(json!("done")));
        assert_eq!(fake.log(), vec![r#"fan set {"cpu":30}"#.to_owned(), r#"fan set {"cpu":60}"#.to_owned()]);
    }

    #[test]
    fn writes_for_different_keys_all_run_in_order() {
        let fake = Fake::new();
        let bus = Bus::new(fake.clone()).expect("bus");
        let (da, ra) = collect();
        let (db, rb) = collect();
        bus.write("a", req("status", "get", json!({"n": 1})), da);
        bus.write("b", req("status", "get", json!({"n": 2})), db);
        assert!(wait(&ra).outcome.is_ok());
        assert!(wait(&rb).outcome.is_ok());
        assert_eq!(fake.log(), vec![r#"status get {"n":1}"#.to_owned(), r#"status get {"n":2}"#.to_owned()]);
    }

    #[test]
    fn live_passes_through_and_swallows_panics() {
        struct Bad;
        impl Commands for Bad {
            fn run(&self, _: &str, _: &str, _: &Value) -> Result<Value, CommandError> {
                Ok(Value::Null)
            }
            fn live(&self) -> Option<Value> {
                std::panic::panic_any("live exploded")
            }
        }
        let bus = Bus::new(Fake::new()).expect("bus");
        assert_eq!(bus.live(), Some(json!({"t": 1})));
        let bad = Bus::new(Arc::new(Bad)).expect("bus");
        assert_eq!(bad.live(), None);
    }

    #[test]
    fn request_parse_refuses_non_object_arguments() {
        assert!(Request::parse("power", "set", r#"{"mode":"turbo"}"#).is_ok());
        assert_eq!(Request::parse("power", "status", "").map(|r| r.args), Ok(json!({})));
        let err = Request::parse("power", "set", "[1]").err();
        assert_eq!(err.and_then(|e| e.code), Some(CODE_BAD_ARGS.to_owned()));
    }

    #[test]
    fn dropping_the_bus_stops_the_writer() {
        let fake = Fake::new();
        let bus = Bus::new(fake.clone()).expect("bus");
        let writes = bus.writes.clone();
        drop(bus);
        // Closed: a late write is handed back instead of queued forever.
        assert!(
            writes
                .submit("k", WriteJob { id: 1, request: req("a", "b", json!({})), done: Box::new(|_| ()) })
                .is_some()
        );
    }
}
