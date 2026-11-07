//! client: how the command line, the terminal UI, the window and the user services ask the
//! daemon for things.
//!
//! One call is one connection: connect, send one request line, read one reply line, close,
//! all within one timeout. The errors tell apart "there is no daemon" (the caller may fall back
//! to the hardware for what is safe without it) from "the daemon said no" (show its message).
//!
//! The status-bar fast path (`truesense-fast`: fan and power through the daemon in one request,
//! hardware directly without it) lives in `ts-cli`, which can see the hardware layer; this
//! module gives it [`Client::try_call`], the part that decides between the two.

use std::fmt;
use std::io::{self, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::wire::{self, Args, DeadlineReader, MAX_LINE, Request, Response, WireError};

/// How long a call may take unless told otherwise (the Python edition's 3 s).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(3);

/// Why a call failed. `Display` is the message to show the user.
#[derive(Debug)]
pub enum ClientError {
    /// Nothing is at the socket path: the daemon is not running.
    NotRunning,
    /// The socket exists but this user may not open it.
    PermissionDenied,
    /// The socket is there but the exchange failed: refused, timed out, closed without a
    /// reply, or the reply was not understood. The text says which.
    Unreachable(String),
    /// The daemon does not know the command: it is older than this program.
    UnknownCommand {
        /// The command that was refused.
        cmd: String,
    },
    /// The daemon understood the request and reported an error; the text is its own.
    Daemon(String),
    /// The request could not be encoded (for example it is larger than the protocol allows).
    BadRequest(String),
    /// The daemon answered, but the result is not the shape the caller expected.
    UnexpectedReply {
        /// The command that was sent.
        cmd: String,
        /// What did not match.
        reason: String,
    },
}

impl ClientError {
    /// Whether the daemon is simply not there (the caller may fall back to the hardware).
    pub fn is_not_running(&self) -> bool {
        matches!(self, Self::NotRunning)
    }

    /// Whether the daemon is too old to know the command (the caller may use an older way).
    pub fn is_unknown_command(&self) -> bool {
        matches!(self, Self::UnknownCommand { .. })
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRunning => {
                f.write_str("the TrueSense daemon is not running (sudo systemctl start truesense)")
            }
            Self::PermissionDenied => f.write_str("no permission to reach the TrueSense daemon socket"),
            Self::Unreachable(why) => write!(f, "daemon not answering: {why}"),
            Self::UnknownCommand { cmd } => write!(
                f,
                "unknown command '{cmd}': the running TrueSense service is older than this app. \
                 Update it with: sudo tools/install.sh (it restarts the service)"
            ),
            Self::Daemon(text) | Self::BadRequest(text) => f.write_str(text),
            Self::UnexpectedReply { cmd, reason } => write!(f, "{cmd}: unexpected reply: {reason}"),
        }
    }
}

impl std::error::Error for ClientError {}

/// A handle on the daemon's socket. Cheap to clone; holds no connection between calls.
#[derive(Debug, Clone)]
pub struct Client {
    path: PathBuf,
    timeout: Duration,
}

impl Client {
    /// A client for the socket at `path` with the default timeout.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into(), timeout: DEFAULT_TIMEOUT }
    }

    /// A client for the socket named by `TRUESENSE_SOCKET`, or the default one.
    pub fn from_env() -> Self {
        Self::new(wire::socket_path())
    }

    /// Sets the time one call may take, connecting and waiting for the reply included.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The socket path this client talks to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the daemon's socket file exists. It says nothing about whether anything listens
    /// on it (a crashed daemon leaves the file); it is the cheap test the fast path starts
    /// with.
    pub fn available(&self) -> bool {
        self.path.exists()
    }

    /// Sends `cmd` and returns its result.
    pub fn call(&self, cmd: &str, args: Args) -> Result<Value, ClientError> {
        let line = Request::new(cmd, args)
            .encode()
            .map_err(|e| ClientError::BadRequest(format!("cannot send {cmd}: {e}")))?;
        let deadline = Instant::now() + self.timeout;
        let stream = connect(&self.path, self.timeout).map_err(connect_error)?;
        // A daemon that refuses the connection ("too many connections") replies and closes
        // without reading the request, so this write can fail although a reply is waiting.
        // Read first, and only report the write error if there is no reply to show instead.
        let sent = stream
            .set_write_timeout(Some(self.timeout.max(Duration::from_millis(1))))
            .and_then(|()| (&mut &stream).write_all(&line));
        let reply = match wire::read_line(DeadlineReader::new(&stream, deadline), MAX_LINE) {
            Ok(reply) if !reply.is_empty() => reply,
            Ok(_) | Err(WireError::Io(_)) if sent.is_err() => {
                let e = sent.err().map_or_else(String::new, |e| wire::io_message(&e));
                return Err(ClientError::Unreachable(e));
            }
            Ok(_) => {
                return Err(ClientError::Unreachable(
                    "the daemon closed the connection without a reply".to_owned(),
                ));
            }
            Err(WireError::Io(e)) => return Err(ClientError::Unreachable(wire::io_message(&e))),
            Err(other) => return Err(ClientError::Unreachable(other.to_string())),
        };
        if reply.is_empty() {
            return Err(ClientError::Unreachable(
                "the daemon closed the connection without a reply".to_owned(),
            ));
        }
        match Response::parse(&reply) {
            Ok(Response::Ok(result)) => Ok(result),
            Ok(Response::Err(text)) if text.starts_with(wire::UNKNOWN_COMMAND_PREFIX) => {
                Err(ClientError::UnknownCommand { cmd: cmd.to_owned() })
            }
            Ok(Response::Err(text)) => Err(ClientError::Daemon(text)),
            Err(e) => Err(ClientError::Unreachable(format!("unreadable reply: {e}"))),
        }
    }

    /// Sends `cmd` and decodes its result into `T` (use `serde_json::Value` or `()`-like
    /// types such as `Option<Value>` for commands whose result is not needed).
    pub fn call_as<T: DeserializeOwned>(&self, cmd: &str, args: Args) -> Result<T, ClientError> {
        let value = self.call(cmd, args)?;
        serde_json::from_value(value)
            .map_err(|e| ClientError::UnexpectedReply { cmd: cmd.to_owned(), reason: e.to_string() })
    }

    /// The fast path's decision in one call: `Ok(None)` when there is no daemon to ask (no
    /// socket file, or it vanished between the check and the connect), so the caller can act
    /// on the hardware instead; `Ok(Some(result))` when the daemon did it; an error when the
    /// daemon is there but failed or refused. Check [`ClientError::is_unknown_command`] to
    /// fall back to an older way of asking.
    pub fn try_call(&self, cmd: &str, args: Args) -> Result<Option<Value>, ClientError> {
        if !self.available() {
            return Ok(None);
        }
        match self.call(cmd, args) {
            Ok(v) => Ok(Some(v)),
            Err(ClientError::NotRunning) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Whether the daemon answers `ping` with `pong` (the service check of the doctor).
    pub fn ping(&self) -> bool {
        matches!(self.call("ping", Args::new()), Ok(Value::String(s)) if s == "pong")
    }
}

fn connect_error(e: io::Error) -> ClientError {
    match e.kind() {
        io::ErrorKind::NotFound => ClientError::NotRunning,
        io::ErrorKind::PermissionDenied => ClientError::PermissionDenied,
        _ => ClientError::Unreachable(wire::io_message(&e)),
    }
}

/// Connects within `timeout`. std has no connect timeout for Unix sockets, and a blocking
/// `connect` waits forever when the daemon's accept queue is full (a stopped or wedged
/// daemon), which would hang a status bar. The connect runs on a helper thread that is
/// abandoned on timeout; it ends by itself when the kernel finally answers.
fn connect(path: &Path, timeout: Duration) -> io::Result<UnixStream> {
    let (tx, rx) = mpsc::channel();
    let target = path.to_path_buf();
    thread::Builder::new().name("ipc-connect".to_owned()).spawn(move || {
        let _ = tx.send(UnixStream::connect(target));
    })?;
    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(io::Error::from(io::ErrorKind::TimedOut)),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(io::Error::other("the connect thread ended without a result"))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static N: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "tscl-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn sock(&self) -> PathBuf {
            self.0.join("c.sock")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A fake daemon: reads one request line, records it, writes `reply` and closes.
    fn fake_daemon(dir: &TempDir, reply: &'static [u8]) -> thread::JoinHandle<Vec<u8>> {
        let listener = UnixListener::bind(dir.sock()).unwrap();
        thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let line = wire::read_line(&s, MAX_LINE).unwrap();
            let _ = s.write_all(reply);
            line
        })
    }

    fn call_fake(reply: &'static [u8]) -> (Result<Value, ClientError>, Vec<u8>) {
        let dir = TempDir::new();
        let daemon = fake_daemon(&dir, reply);
        let result = Client::new(dir.sock())
            .with_timeout(Duration::from_secs(2))
            .call("fan", Args::new().with("mode", "auto"));
        (result, daemon.join().unwrap())
    }

    #[test]
    fn sends_the_request_the_python_daemon_expects() {
        let (r, seen) = call_fake(b"{\"ok\": true, \"result\": {\"mode\": \"auto\"}}\n");
        assert_eq!(r.unwrap(), json!({"mode": "auto"}));
        assert_eq!(seen, br#"{"cmd":"fan","args":{"mode":"auto"}}"#);
    }

    #[test]
    fn reply_table() {
        let text = |r: Result<Value, ClientError>| r.map_err(|e| e.to_string());
        assert_eq!(text(call_fake(b"{\"ok\": true}\n").0), Ok(Value::Null));
        assert_eq!(
            text(call_fake(b"{\"ok\": true, \"result\": \"pong\"}").0),
            Ok(json!("pong")),
            "no newline, closed"
        );
        assert_eq!(
            text(call_fake(b"{\"ok\": false, \"error\": \"firmware said no\"}\n").0),
            Err("firmware said no".into())
        );
        assert_eq!(text(call_fake(b"{\"ok\": false}\n").0), Err("daemon error".into()));
        let old = call_fake(b"{\"ok\": false, \"error\": \"unknown command 'fan'\"}\n").0.unwrap_err();
        assert!(old.is_unknown_command() && !old.is_not_running());
        assert_eq!(
            old.to_string(),
            "unknown command 'fan': the running TrueSense service is older than this app. Update it with: sudo tools/install.sh (it restarts the service)"
        );
        for (name, reply) in [
            ("closed without a reply", &b""[..]),
            ("garbage", b"not json\n"),
            ("array", b"[1,2]\n"),
            ("truncated", b"{\"ok\": tr"),
            ("binary", b"\x00\xff\xfe\n"),
            ("deep", &[b'['; 70_000][..]),
        ] {
            let e = call_fake(reply).0.unwrap_err();
            assert!(matches!(e, ClientError::Unreachable(_)), "{name}: {e:?}");
            assert!(e.to_string().starts_with("daemon not answering: "), "{name}: {e}");
        }
    }

    #[test]
    fn an_oversized_reply_is_refused() {
        let dir = TempDir::new();
        let listener = UnixListener::bind(dir.sock()).unwrap();
        let daemon = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let _ = wire::read_line(&s, MAX_LINE);
            let _ = s.write_all(&vec![b'a'; MAX_LINE + 100]);
        });
        let e = Client::new(dir.sock()).call("x", Args::new()).unwrap_err();
        assert!(e.to_string().contains("too large"), "{e}");
        daemon.join().unwrap();
    }

    #[test]
    fn no_daemon_is_not_running_and_try_call_is_none() {
        let dir = TempDir::new();
        let c = Client::new(dir.0.join("missing").join("x.sock"));
        assert!(matches!(c.call("ping", Args::new()), Err(ClientError::NotRunning)));
        assert!(!c.available());
        assert!(matches!(c.try_call("ping", Args::new()), Ok(None)));
        assert!(!c.ping());
        assert_eq!(
            c.call("ping", Args::new()).unwrap_err().to_string(),
            "the TrueSense daemon is not running (sudo systemctl start truesense)"
        );
    }

    #[test]
    fn a_stale_socket_is_unreachable_not_not_running() {
        let dir = TempDir::new();
        drop(UnixListener::bind(dir.sock()).unwrap()); // the file stays, nobody listens
        let c = Client::new(dir.sock());
        assert!(c.available());
        let e = c.call("ping", Args::new()).unwrap_err();
        assert!(matches!(&e, ClientError::Unreachable(m) if m.contains("onnection refused")), "{e}");
        assert!(
            c.try_call("ping", Args::new()).is_err(),
            "available but broken is an error, not a silent fallback"
        );
    }

    #[test]
    fn permission_denied_is_its_own_error() {
        let dir = TempDir::new();
        let listener = UnixListener::bind(dir.sock()).unwrap();
        std::fs::set_permissions(dir.sock(), std::fs::Permissions::from_mode(0o000)).unwrap();
        let c = Client::new(dir.sock());
        if std::os::unix::fs::MetadataExt::uid(&std::fs::metadata(&dir.0).unwrap()) == 0 {
            return; // root ignores the mode
        }
        let e = c.call("ping", Args::new()).unwrap_err();
        assert!(matches!(e, ClientError::PermissionDenied), "{e:?}");
        assert_eq!(e.to_string(), "no permission to reach the TrueSense daemon socket");
        drop(listener);
    }

    #[test]
    fn a_daemon_that_never_answers_times_out() {
        let dir = TempDir::new();
        let listener = UnixListener::bind(dir.sock()).unwrap();
        let (hold_tx, hold_rx) = mpsc::channel::<()>();
        let daemon = thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let _ = hold_rx.recv_timeout(Duration::from_secs(5));
            drop(s);
        });
        let started = Instant::now();
        let e = Client::new(dir.sock())
            .with_timeout(Duration::from_millis(150))
            .call("ping", Args::new())
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(e.to_string(), "daemon not answering: timed out");
        let _ = hold_tx.send(());
        daemon.join().unwrap();
    }

    #[test]
    fn try_call_and_typed_results() {
        let dir = TempDir::new();
        let daemon =
            fake_daemon(&dir, b"{\"ok\": true, \"result\": {\"settings\": {\"smart_fan\": true}}}\n");
        let c = Client::new(dir.sock());
        let got = c.try_call("state", Args::new()).unwrap();
        assert_eq!(got, Some(json!({"settings": {"smart_fan": true}})));
        daemon.join().unwrap();

        let dir = TempDir::new();
        let daemon = fake_daemon(&dir, b"{\"ok\": true, \"result\": [1, 2, 3]}\n");
        let c = Client::new(dir.sock());
        assert_eq!(c.call_as::<Vec<u8>>("list", Args::new()).unwrap(), vec![1, 2, 3]);
        daemon.join().unwrap();

        let dir = TempDir::new();
        let daemon = fake_daemon(&dir, b"{\"ok\": true, \"result\": \"x\"}\n");
        let e = Client::new(dir.sock()).call_as::<Vec<u8>>("list", Args::new()).unwrap_err();
        assert!(matches!(&e, ClientError::UnexpectedReply { cmd, .. } if cmd == "list"), "{e:?}");
        assert!(e.to_string().starts_with("list: unexpected reply: "));
        daemon.join().unwrap();
    }

    #[test]
    fn ping_means_pong() {
        let dir = TempDir::new();
        let daemon = fake_daemon(&dir, b"{\"ok\": true, \"result\": \"pong\"}\n");
        assert!(Client::new(dir.sock()).ping());
        daemon.join().unwrap();
        let dir = TempDir::new();
        let daemon = fake_daemon(&dir, b"{\"ok\": true, \"result\": \"nope\"}\n");
        assert!(!Client::new(dir.sock()).ping());
        daemon.join().unwrap();
    }

    #[test]
    fn an_oversized_request_is_refused_before_connecting() {
        let c = Client::new("/nonexistent/x.sock");
        let e = c.call("x", Args::new().with("s", "a".repeat(MAX_LINE))).unwrap_err();
        assert!(matches!(e, ClientError::BadRequest(_)), "{e:?}");
        assert!(e.to_string().starts_with("cannot send x: message too large"));
    }

    #[test]
    fn from_env_and_accessors() {
        let c = Client::new("/a/b.sock").with_timeout(Duration::from_millis(5));
        assert_eq!(c.path(), Path::new("/a/b.sock"));
        assert_eq!(Client::from_env().path(), wire::socket_path());
    }

    #[test]
    fn the_connect_helper_reports_a_missing_socket() {
        let e = connect(Path::new("/nonexistent/x.sock"), Duration::from_secs(1)).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
    }
}
