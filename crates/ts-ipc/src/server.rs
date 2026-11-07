//! server: the daemon's side of the control socket.
//!
//! The daemon (root) owns the privileged and persistent settings; the CLI, the window and the
//! key service (running as the user) talk to it here instead of sharing files. The socket
//! file is world-connectable (mode 0666) but every request is authorised by the caller's uid,
//! read from the kernel with `SO_PEERCRED`: root, or the configured desktop user.
//!
//! The server never lets a client or a handler take it down:
//!
//! * one thread per connection, at most `max_connections` at a time (the next client is told
//!   "too many connections" instead of queueing without bound);
//! * the whole request must arrive within `io_timeout` (a peer that trickles bytes is cut
//!   off, see `wire::DeadlineReader`), and is at most [`MAX_LINE`] bytes;
//! * a handler that panics becomes an error reply (`catch_unwind`), the connection thread
//!   ends cleanly and the next request is served as usual;
//! * `stop` wakes the accept loop, lets requests in flight finish (for a bounded time) and
//!   removes the socket file.

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::wire::{self, Args, DeadlineReader, MAX_LINE, Request, Response};

/// Runs the commands. Implemented for any `Fn(&Request, u32) -> Result<Value, String>`, and by
/// [`Router`], which dispatches on the command name.
///
/// `Err(text)` becomes the error reply and is shown to the user as it is. A panic is caught
/// and also becomes an error reply, but a handler that panics while holding a lock leaves it
/// poisoned: handlers should not rely on that.
pub trait Handler: Send + Sync + 'static {
    /// Runs one request from the process with user id `caller_uid`, already authorised.
    fn handle(&self, request: &Request, caller_uid: u32) -> Result<Value, String>;
}

impl<F> Handler for F
where
    F: Fn(&Request, u32) -> Result<Value, String> + Send + Sync + 'static,
{
    fn handle(&self, request: &Request, caller_uid: u32) -> Result<Value, String> {
        self(request, caller_uid)
    }
}

type Route = Box<dyn Fn(&Args) -> Result<Value, String> + Send + Sync>;

/// A [`Handler`] that looks the command name up in a table and answers an unknown one with the
/// protocol's "unknown command" error, which is what makes an old daemon recognisable to a
/// newer client.
#[derive(Default)]
pub struct Router {
    routes: HashMap<String, Route>,
}

impl Router {
    /// An empty router: every command is unknown.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a command, builder style. A second route with the same name replaces the first.
    #[must_use]
    pub fn route(
        mut self,
        cmd: &str,
        f: impl Fn(&Args) -> Result<Value, String> + Send + Sync + 'static,
    ) -> Self {
        self.insert(cmd, f);
        self
    }

    /// Adds a command.
    pub fn insert(&mut self, cmd: &str, f: impl Fn(&Args) -> Result<Value, String> + Send + Sync + 'static) {
        self.routes.insert(cmd.to_owned(), Box::new(f));
    }

    /// The command names, sorted (for diagnostics and tests).
    pub fn commands(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.routes.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }
}

impl Handler for Router {
    fn handle(&self, request: &Request, _caller_uid: u32) -> Result<Value, String> {
        match self.routes.get(&request.cmd) {
            Some(route) => route(&request.args),
            None => Err(wire::unknown_command_message(&request.cmd)),
        }
    }
}

/// Runs one request line for an already authorised caller and returns the reply. It never
/// panics and always yields a reply that encodes to one valid line: this is the function that
/// faces untrusted bytes, so it is the one tests and fuzz targets drive without a socket.
pub fn dispatch(handler: &dyn Handler, caller_uid: u32, line: &[u8]) -> Response {
    let request = match Request::parse(line) {
        Ok(r) => r,
        Err(e) => return Response::Err(format!("bad request: {e}")),
    };
    match catch_unwind(AssertUnwindSafe(|| handler.handle(&request, caller_uid))) {
        Ok(Ok(value)) => Response::Ok(value),
        Ok(Err(text)) => Response::Err(text),
        Err(payload) => Response::Err(format!("internal error: {}", panic_text(payload.as_ref()))),
    }
}

/// The message of a panic payload (`panic!("text")` and `panic!("{x}")` carry a string).
fn panic_text(payload: &(dyn std::any::Any + Send)) -> &str {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s
    } else {
        "the handler panicked"
    }
}

/// The user id of the process on the other end of `stream`, as the kernel recorded it when the
/// peer connected (`SO_PEERCRED`). It cannot be forged by the peer, which is why access is
/// decided on it and on nothing the peer sends.
///
/// std has `UnixStream::peer_cred` but it is still unstable, so this is the one place in the
/// crate that needs `unsafe`.
pub fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let want = std::mem::size_of::<libc::ucred>();
    let mut len =
        libc::socklen_t::try_from(want).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: `fd` is open for the whole call because `stream` is borrowed; `cred` is a live,
    // properly aligned `ucred` and `len` holds its exact size, which is all getsockopt writes
    // for SO_PEERCRED; both pointers come from exclusive references that outlive the call.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut cred).cast::<libc::c_void>(),
            &raw mut len,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    if usize::try_from(len).ok() != Some(want) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "SO_PEERCRED returned a short answer"));
    }
    Ok(cred.uid)
}

/// How the server listens and how much it tolerates.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ServerConfig {
    /// The socket file to create (replacing a stale socket left by a crash).
    pub path: PathBuf,
    /// User ids allowed to send commands. Root (0) is always allowed, whatever is passed.
    pub allowed_uids: BTreeSet<u32>,
    /// Most connections served at once; the others get a "too many connections" error.
    pub max_connections: usize,
    /// How long a request may take to arrive and a reply to be written. A handler's own run
    /// time is not bounded by it.
    pub io_timeout: Duration,
    /// Permission bits of the socket file. 0666 lets every local user connect; the uid check
    /// then decides who may do anything.
    pub socket_mode: u32,
    /// How long `stop` waits for requests in flight before giving up on them.
    pub shutdown_grace: Duration,
}

impl ServerConfig {
    /// The defaults: at most 32 connections, 3 s to read a request (as in the Python edition),
    /// mode 0666, 5 s of grace on stop. `allowed_uids` is typically the desktop user, root is
    /// added.
    pub fn new(path: impl Into<PathBuf>, allowed_uids: impl IntoIterator<Item = u32>) -> Self {
        let mut allowed: BTreeSet<u32> = allowed_uids.into_iter().collect();
        allowed.insert(0);
        Self {
            path: path.into(),
            allowed_uids: allowed,
            max_connections: 32,
            io_timeout: Duration::from_secs(3),
            socket_mode: 0o666,
            shutdown_grace: Duration::from_secs(5),
        }
    }
}

/// What went wrong starting the server.
#[derive(Debug)]
pub enum ServerError {
    /// A system call failed; `op` says which and `path` where.
    Io {
        /// The operation that failed ("creating the socket directory", ...).
        op: &'static str,
        /// The path it was working on.
        path: PathBuf,
        /// The cause.
        source: io::Error,
    },
    /// Something that is not a socket sits where the socket should go. The server will not
    /// delete it: a mistyped path must not cost anybody a file.
    NotASocket {
        /// The path that is occupied.
        path: PathBuf,
    },
}

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { op, path, source } => write!(f, "{op} {}: {source}", path.display()),
            Self::NotASocket { path } => {
                write!(f, "{} exists and is not a socket; not replacing it", path.display())
            }
        }
    }
}

impl std::error::Error for ServerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::NotASocket { .. } => None,
        }
    }
}

struct Shared {
    handler: Box<dyn Handler>,
    allowed: BTreeSet<u32>,
    max_connections: usize,
    io_timeout: Duration,
    stop: AtomicBool,
    /// Connections being served, with a condition to wait for it to reach zero.
    active: Mutex<usize>,
    idle: Condvar,
}

impl Shared {
    fn lock_active(&self) -> MutexGuard<'_, usize> {
        // The counter is a plain integer: a poisoned lock holds a valid value.
        self.active.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Counts one connection while it lives; dropping it (also when its thread unwinds) frees the
/// slot and wakes `stop` when the last one ends.
struct Slot(Arc<Shared>);

impl Slot {
    fn acquire(shared: &Arc<Shared>) -> Option<Slot> {
        let mut active = shared.lock_active();
        if *active >= shared.max_connections {
            return None;
        }
        *active += 1;
        Some(Slot(Arc::clone(shared)))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut active = self.0.lock_active();
        *active = active.saturating_sub(1);
        if *active == 0 {
            self.0.idle.notify_all();
        }
    }
}

/// The running control socket. Dropping it stops it.
pub struct Server {
    shared: Arc<Shared>,
    path: PathBuf,
    shutdown_grace: Duration,
    accept: Option<JoinHandle<()>>,
    stopped: bool,
}

impl Server {
    /// Creates the socket and starts accepting on a background thread.
    ///
    /// The directory is created if missing, a stale socket file is replaced, and the file
    /// gets `config.socket_mode`. The mode is set with `chmod` after `bind` instead of by
    /// clearing the process umask around it, since the umask is process-wide and other
    /// threads create files meanwhile; the short window before the `chmod` is only ever
    /// stricter than the final mode.
    pub fn start(config: ServerConfig, handler: impl Handler) -> Result<Server, ServerError> {
        let listener = bind_socket(&config)?;
        let mut allowed = config.allowed_uids;
        allowed.insert(0);
        let shared = Arc::new(Shared {
            handler: Box::new(handler),
            allowed,
            max_connections: config.max_connections.max(1),
            io_timeout: config.io_timeout,
            stop: AtomicBool::new(false),
            active: Mutex::new(0),
            idle: Condvar::new(),
        });
        let loop_shared = Arc::clone(&shared);
        let spawned = thread::Builder::new()
            .name("ipc-accept".to_owned())
            .spawn(move || accept_loop(&listener, &loop_shared));
        let accept = match spawned {
            Ok(handle) => handle,
            Err(source) => {
                let _ = fs::remove_file(&config.path);
                return Err(ServerError::Io {
                    op: "starting the accept thread for",
                    path: config.path,
                    source,
                });
            }
        };
        Ok(Server {
            shared,
            path: config.path,
            shutdown_grace: config.shutdown_grace,
            accept: Some(accept),
            stopped: false,
        })
    }

    /// The socket file's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// How many connections are being served right now.
    pub fn active_connections(&self) -> usize {
        *self.shared.lock_active()
    }

    /// Stops accepting, waits up to `shutdown_grace` for requests in flight and removes the
    /// socket file. Safe to call twice. A request that is still running after the grace
    /// period is left to finish on its own thread: the daemon is about to exit and must not
    /// hang on a stuck handler (the fans go back to Auto on exit regardless).
    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        self.shared.stop.store(true, Ordering::SeqCst);
        // `accept` blocks until a connection arrives, so make one. If the file is gone (someone
        // removed it) the connect fails and the accept thread is left to end with the process;
        // joining it would hang.
        let woke = UnixStream::connect(&self.path).is_ok();
        if let Some(handle) = self.accept.take() {
            if woke {
                let _ = handle.join();
            }
        }
        // Only a socket may be removed: if the path now names something else, leave it.
        if fs::symlink_metadata(&self.path).is_ok_and(|m| m.file_type().is_socket()) {
            let _ = fs::remove_file(&self.path);
        }
        let deadline = Instant::now() + self.shutdown_grace;
        let mut active = self.shared.lock_active();
        while *active > 0 {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            active = self.shared.idle.wait_timeout(active, left).unwrap_or_else(PoisonError::into_inner).0;
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

fn io_err(op: &'static str, path: &Path) -> impl FnOnce(io::Error) -> ServerError {
    let path = path.to_path_buf();
    move |source| ServerError::Io { op, path, source }
}

fn bind_socket(config: &ServerConfig) -> Result<UnixListener, ServerError> {
    let path = config.path.as_path();
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o755)
            .create(dir)
            .map_err(io_err("creating the socket directory", dir))?;
    }
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_socket() => {
            fs::remove_file(path).map_err(io_err("removing the stale socket", path))?
        }
        Ok(_) => return Err(ServerError::NotASocket { path: path.to_path_buf() }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_err("inspecting", path)(e)),
    }
    let listener = UnixListener::bind(path).map_err(io_err("binding the socket", path))?;
    if let Err(e) = fs::set_permissions(path, fs::Permissions::from_mode(config.socket_mode)) {
        let _ = fs::remove_file(path);
        return Err(io_err("setting the mode of", path)(e));
    }
    Ok(listener)
}

fn accept_loop(listener: &UnixListener, shared: &Arc<Shared>) {
    loop {
        let stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(_) if shared.stop.load(Ordering::SeqCst) => return,
            Err(e) => {
                // Out of file descriptors, a client that gave up, a signal: none is a reason to
                // stop serving. Pause so that a persistent failure cannot spin a core.
                if e.kind() != io::ErrorKind::Interrupted {
                    thread::sleep(Duration::from_millis(50));
                }
                continue;
            }
        };
        if shared.stop.load(Ordering::SeqCst) {
            return; // the connection `stop` made to wake this loop
        }
        let Some(slot) = Slot::acquire(shared) else {
            reject(&stream, shared.io_timeout, "too many connections");
            continue;
        };
        let conn_shared = Arc::clone(shared);
        // If the thread cannot be started the closure is dropped with the stream: the client
        // sees the connection close and the slot is released.
        let _ = thread::Builder::new().name("ipc-conn".to_owned()).spawn(move || {
            let _slot = slot;
            serve(&stream, &conn_shared);
        });
    }
}

/// Answers a connection that will not be served. The reply is a few bytes into a fresh socket
/// buffer, so this does not block the accept loop (the timeout is a belt and braces).
fn reject(stream: &UnixStream, timeout: Duration, why: &str) {
    let _ = stream.set_write_timeout(Some(timeout));
    let _ = (&mut &*stream).write_all(&Response::Err(why.to_owned()).encode());
}

/// Serves the one request of a connection. The authorisation happens after the request is read
/// (as in the Python edition) so that an unauthorised peer still gets its answer instead of a
/// reset, and before it is parsed or handed to anything.
fn serve(stream: &UnixStream, shared: &Shared) {
    let deadline = Instant::now() + shared.io_timeout;
    let _ = stream.set_write_timeout(Some(shared.io_timeout));
    let response = answer(stream, shared, deadline);
    let _ = (&mut &*stream).write_all(&response.encode());
}

fn answer(stream: &UnixStream, shared: &Shared, deadline: Instant) -> Response {
    let uid = match peer_uid(stream) {
        Ok(uid) => uid,
        Err(e) => return Response::Err(format!("cannot identify the caller: {e}")),
    };
    let line = match wire::read_line(DeadlineReader::new(stream, deadline), MAX_LINE) {
        Ok(line) => line,
        Err(e) => return Response::Err(e.to_string()),
    };
    if !shared.allowed.contains(&uid) {
        return Response::Err(format!("uid {uid} is not allowed to control TrueSense"));
    }
    dispatch(shared.handler.as_ref(), uid, &line)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::client::{Client, ClientError};
    use serde_json::json;
    use std::io::Read;
    use std::os::unix::fs::MetadataExt;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;

    /// A unique temp directory, removed on drop. Paths stay short: a socket path is limited to
    /// about 108 bytes.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static N: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "tsipc-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn sock(&self) -> PathBuf {
            self.0.join("s.sock")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn me() -> u32 {
        let probe = TempDir::new();
        fs::metadata(&probe.0).unwrap().uid()
    }

    fn standard_router() -> Router {
        Router::new()
            .route("ping", |_| Ok(json!("pong")))
            .route("echo", |a| Ok(json!({"got": a})))
            .route("none", |_| Ok(Value::Null))
            .route("fail", |_| Err("firmware said no".to_owned()))
            .route("boom", |_| -> Result<Value, String> { panic!("kaboom") })
            .route("boom_string", |a| -> Result<Value, String> { panic!("bad {}", a.as_map().len()) })
            .route("need_int", |a| Ok(json!(a.require_int("n")? + 1)))
    }

    fn start(dir: &TempDir, allowed: &[u32], handler: impl Handler) -> (Server, Client) {
        let server = Server::start(ServerConfig::new(dir.sock(), allowed.iter().copied()), handler).unwrap();
        let client = Client::new(dir.sock()).with_timeout(Duration::from_secs(2));
        (server, client)
    }

    fn remote(e: ClientError) -> String {
        e.to_string()
    }

    #[test]
    fn peer_uid_is_the_callers() {
        let (a, _b) = UnixStream::pair().unwrap();
        assert_eq!(peer_uid(&a).unwrap(), me());
    }

    #[test]
    fn peer_uid_fails_cleanly_on_a_non_socket() {
        let dir = TempDir::new();
        let file = fs::File::create(dir.0.join("f")).unwrap();
        let not_a_socket = UnixStream::from(std::os::fd::OwnedFd::from(file));
        let e = peer_uid(&not_a_socket).unwrap_err();
        assert_eq!(e.raw_os_error(), Some(libc::ENOTSOCK));
    }

    #[test]
    fn round_trip_like_the_python_test() {
        let dir = TempDir::new();
        let (mut server, client) = start(&dir, &[me()], standard_router());
        assert_eq!(client.call("ping", Args::new()).unwrap(), json!("pong"));
        let got = client.call("echo", Args::new().with("n", 3).with("s", "x")).unwrap();
        assert_eq!(got, json!({"got": {"n": 3, "s": "x"}}));
        assert_eq!(client.call("none", Args::new()).unwrap(), Value::Null);
        assert_eq!(client.call("need_int", Args::new().with("n", 41)).unwrap(), json!(42));
        assert_eq!(
            remote(client.call("need_int", Args::new().with("n", "x")).unwrap_err()),
            "argument \"n\" must be a whole number"
        );
        assert_eq!(remote(client.call("fail", Args::new()).unwrap_err()), "firmware said no");
        match client.call("nope", Args::new()).unwrap_err() {
            ClientError::UnknownCommand { cmd } => assert_eq!(cmd, "nope"),
            other => panic!("{other:?}"),
        }
        server.stop();
        assert!(
            matches!(client.call("ping", Args::new()), Err(ClientError::NotRunning)),
            "daemon gone: a clear error"
        );
    }

    #[test]
    fn a_panicking_handler_is_an_error_reply_and_the_server_lives() {
        let dir = TempDir::new();
        let (_server, client) = start(&dir, &[me()], standard_router());
        assert_eq!(remote(client.call("boom", Args::new()).unwrap_err()), "internal error: kaboom");
        assert_eq!(remote(client.call("boom_string", Args::new()).unwrap_err()), "internal error: bad 0");
        assert_eq!(client.call("ping", Args::new()).unwrap(), json!("pong"));
        assert_eq!(client.call("ping", Args::new()).unwrap(), json!("pong"));
    }

    #[test]
    fn only_allowed_uids_may_control() {
        if me() == 0 {
            return; // root is always allowed
        }
        let dir = TempDir::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&calls);
        let handler = move |_: &Request, _: u32| {
            seen.fetch_add(1, Ordering::SeqCst);
            Ok(json!("ran"))
        };
        let (_server, client) = start(&dir, &[424_242], handler);
        let msg = remote(client.call("ping", Args::new()).unwrap_err());
        assert_eq!(msg, format!("uid {} is not allowed to control TrueSense", me()));
        assert_eq!(calls.load(Ordering::SeqCst), 0, "an unauthorised request must never reach the handler");
    }

    #[test]
    fn handler_sees_the_callers_uid() {
        let dir = TempDir::new();
        let (_server, client) = start(&dir, &[me()], |_: &Request, uid: u32| Ok(json!(uid)));
        assert_eq!(client.call("whoami", Args::new()).unwrap(), json!(me()));
    }

    #[test]
    fn root_is_always_allowed() {
        let cfg = ServerConfig::new("/x/y.sock", [1000]);
        assert_eq!(cfg.allowed_uids.iter().copied().collect::<Vec<_>>(), vec![0, 1000]);
        assert!(ServerConfig::new("/x/y.sock", []).allowed_uids.contains(&0));
    }

    #[test]
    fn socket_file_mode_creation_and_removal() {
        let dir = TempDir::new();
        let nested = dir.0.join("run").join("ts").join("s.sock");
        let mut server = Server::start(ServerConfig::new(&nested, [me()]), standard_router()).unwrap();
        let meta = fs::symlink_metadata(&nested).unwrap();
        assert!(meta.file_type().is_socket());
        assert_eq!(
            meta.permissions().mode() & 0o777,
            0o666,
            "any local user may connect, the uid check decides"
        );
        assert_eq!(fs::metadata(nested.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(server.path(), nested);
        server.stop();
        assert!(!nested.exists(), "the socket file is removed on stop");
        server.stop();
    }

    #[test]
    fn configured_mode_is_applied_and_drop_cleans_up() {
        let dir = TempDir::new();
        let mut cfg = ServerConfig::new(dir.sock(), [me()]);
        cfg.socket_mode = 0o660;
        let server = Server::start(cfg, standard_router()).unwrap();
        assert_eq!(fs::metadata(dir.sock()).unwrap().permissions().mode() & 0o777, 0o660);
        drop(server);
        assert!(!dir.sock().exists());
    }

    #[test]
    fn a_stale_socket_is_replaced() {
        let dir = TempDir::new();
        let stale = UnixListener::bind(dir.sock()).unwrap();
        drop(stale); // closing a listener leaves the file behind, like a crashed daemon
        assert!(dir.sock().exists());
        let (_server, client) = start(&dir, &[me()], standard_router());
        assert_eq!(client.call("ping", Args::new()).unwrap(), json!("pong"));
    }

    #[test]
    fn a_regular_file_is_never_deleted() {
        let dir = TempDir::new();
        fs::write(dir.sock(), b"precious").unwrap();
        let e = Server::start(ServerConfig::new(dir.sock(), []), standard_router()).err().unwrap();
        assert!(matches!(e, ServerError::NotASocket { .. }), "{e}");
        assert!(e.to_string().contains("not a socket"));
        assert_eq!(fs::read(dir.sock()).unwrap(), b"precious");
    }

    #[test]
    fn start_reports_an_unusable_directory() {
        let dir = TempDir::new();
        fs::write(dir.0.join("file"), b"x").unwrap();
        let e = Server::start(ServerConfig::new(dir.0.join("file").join("s.sock"), []), standard_router())
            .err()
            .unwrap();
        assert!(matches!(e, ServerError::Io { op: "creating the socket directory", .. }), "{e}");
        assert!(std::error::Error::source(&e).is_some());
    }

    /// Sends raw bytes, closes the writing side (a request without a newline ends at end of
    /// input, as with the Python server) and reads the raw reply: what a Python client sees.
    fn raw(path: &Path, request: &[u8]) -> Vec<u8> {
        let mut s = UnixStream::connect(path).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let _ = s.write_all(request);
        s.shutdown(std::net::Shutdown::Write).unwrap();
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        out
    }

    #[test]
    fn python_client_bytes_get_the_documented_reply_bytes() {
        let dir = TempDir::new();
        let (_server, _c) = start(&dir, &[me()], standard_router());
        assert_eq!(
            raw(&dir.sock(), b"{\"cmd\": \"ping\", \"args\": {}}\n"),
            b"{\"ok\":true,\"result\":\"pong\"}\n"
        );
        assert_eq!(
            raw(&dir.sock(), b"{\"cmd\": \"fail\", \"args\": {}}\n"),
            b"{\"ok\":false,\"error\":\"firmware said no\"}\n"
        );
        assert_eq!(
            raw(&dir.sock(), b"{\"cmd\": \"ping\"}"),
            b"{\"ok\":true,\"result\":\"pong\"}\n",
            "a request without a newline still works"
        );
        assert_eq!(
            raw(&dir.sock(), b"{\"cmd\": \"nope\", \"args\": {}}\n"),
            b"{\"ok\":false,\"error\":\"unknown command 'nope'\"}\n"
        );
    }

    #[test]
    fn hostile_requests_get_an_error_reply_each() {
        let dir = TempDir::new();
        let (_server, _c) = start(&dir, &[me()], standard_router());
        let table: Vec<(&str, Vec<u8>)> = vec![
            ("empty", b"\n".to_vec()),
            ("closed without data", Vec::new()),
            ("garbage", b"\x00\xff\xfe garbage\n".to_vec()),
            ("array", b"[]\n".to_vec()),
            ("cmd number", b"{\"cmd\":5}\n".to_vec()),
            ("args array", b"{\"cmd\":\"echo\",\"args\":[1]}\n".to_vec()),
            ("deep", [b"[".repeat(50_000), b"\n".to_vec()].concat()),
            ("too large", vec![b'a'; MAX_LINE + 10]),
            ("too large with newline", [vec![b'a'; MAX_LINE + 10], b"\n".to_vec()].concat()),
        ];
        for (name, req) in table {
            let out = raw(&dir.sock(), &req);
            assert!(
                out.ends_with(b"\n") && out.iter().filter(|b| **b == b'\n').count() == 1,
                "{name}: one line, got {out:?}"
            );
            assert!(
                out.starts_with(b"{\"ok\":false,\"error\":\""),
                "{name}: {}",
                String::from_utf8_lossy(&out)
            );
        }
        let out = raw(&dir.sock(), &[vec![b'a'; MAX_LINE + 10], b"\n".to_vec()].concat());
        assert!(String::from_utf8_lossy(&out).contains("too large"));
        // after all that the server is fine
        let c = Client::new(dir.sock());
        assert_eq!(c.call("ping", Args::new()).unwrap(), json!("pong"));
    }

    #[test]
    fn dispatch_table() {
        let h = standard_router();
        let reply = |line: &str| dispatch(&h, 0, line.as_bytes());
        assert_eq!(reply(r#"{"cmd":"ping"}"#), Response::Ok(json!("pong")));
        assert_eq!(reply(r#"{"cmd":"ping","args":null}"#), Response::Ok(json!("pong")));
        assert_eq!(reply(r#"{"cmd":"x"}"#), Response::Err("unknown command 'x'".into()));
        assert_eq!(reply(r#"{"cmd":"boom"}"#), Response::Err("internal error: kaboom".into()));
        for bad in ["", "not json", "{\"cmd\": 5}", "[]", "{", "\u{0}", "{\"cmd\":\"echo\",\"args\":[1]}"] {
            assert!(matches!(reply(bad), Response::Err(_)), "{bad:?}");
            let enc = reply(bad).encode();
            assert!(enc.ends_with(b"\n") && Response::parse(&enc[..enc.len() - 1]).is_ok());
        }
        assert!(matches!(reply(""), Response::Err(m) if m.starts_with("bad request: ")));
    }

    #[test]
    fn a_handler_with_a_non_string_panic_payload() {
        let h = |_: &Request, _: u32| -> Result<Value, String> { std::panic::panic_any(42_u8) };
        assert_eq!(
            dispatch(&h, 0, br#"{"cmd":"a"}"#),
            Response::Err("internal error: the handler panicked".into())
        );
    }

    #[test]
    fn an_oversized_result_becomes_an_error() {
        let dir = TempDir::new();
        let big = |_: &Request, _: u32| Ok(json!("x".repeat(MAX_LINE)));
        let (_server, client) = start(&dir, &[me()], big);
        assert!(
            remote(client.call("big", Args::new()).unwrap_err()).contains("larger than the 64 KiB limit")
        );
    }

    #[test]
    fn a_silent_client_is_cut_off_by_the_read_timeout() {
        let dir = TempDir::new();
        let mut cfg = ServerConfig::new(dir.sock(), [me()]);
        cfg.io_timeout = Duration::from_millis(100);
        let _server = Server::start(cfg, standard_router()).unwrap();
        let mut idle = UnixStream::connect(dir.sock()).unwrap();
        idle.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let started = Instant::now();
        let mut out = Vec::new();
        let _ = idle.read_to_end(&mut out);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(String::from_utf8_lossy(&out).contains("timed out"), "{}", String::from_utf8_lossy(&out));
        assert_eq!(Client::new(dir.sock()).call("ping", Args::new()).unwrap(), json!("pong"));
    }

    #[test]
    fn a_trickling_client_cannot_hold_a_connection_past_the_timeout() {
        let dir = TempDir::new();
        let mut cfg = ServerConfig::new(dir.sock(), [me()]);
        cfg.io_timeout = Duration::from_millis(200);
        let _server = Server::start(cfg, standard_router()).unwrap();
        let mut slow = UnixStream::connect(dir.sock()).unwrap();
        slow.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let started = Instant::now();
        for _ in 0..30 {
            if slow.write_all(b" ").is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(30));
            if started.elapsed() > Duration::from_millis(400) {
                break;
            }
        }
        let mut out = Vec::new();
        let _ = slow.read_to_end(&mut out);
        assert!(started.elapsed() < Duration::from_secs(2), "cut off by the whole-request deadline");
        assert!(String::from_utf8_lossy(&out).contains("timed out"));
    }

    /// A handler that blocks until released, for tests of the connection cap and of shutdown.
    fn gate() -> (impl Handler, mpsc::Sender<()>, mpsc::Receiver<()>) {
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let (entered_tx, entered_rx) = mpsc::channel::<()>();
        let release_rx = Mutex::new(release_rx);
        let entered_tx = Mutex::new(entered_tx);
        let handler = move |req: &Request, _: u32| {
            if req.cmd == "wait" {
                let _ = entered_tx.lock().unwrap().send(());
                let _ = release_rx.lock().unwrap().recv_timeout(Duration::from_secs(10));
                return Ok(json!("released"));
            }
            Ok(json!("pong"))
        };
        (handler, release_tx, entered_rx)
    }

    #[test]
    fn the_connection_cap_refuses_the_extra_client_and_recovers() {
        let dir = TempDir::new();
        let (handler, release, entered) = gate();
        let mut cfg = ServerConfig::new(dir.sock(), [me()]);
        cfg.max_connections = 1;
        let server = Server::start(cfg, handler).unwrap();
        let client = Client::new(dir.sock()).with_timeout(Duration::from_secs(5));
        let first = {
            let c = client.clone();
            thread::spawn(move || c.call("wait", Args::new()))
        };
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(server.active_connections(), 1);
        assert_eq!(remote(client.call("ping", Args::new()).unwrap_err()), "too many connections");
        release.send(()).unwrap();
        assert_eq!(first.join().unwrap().unwrap(), json!("released"));
        // the slot is free again (the worker may need a moment to drop its guard)
        let deadline = Instant::now() + Duration::from_secs(3);
        while server.active_connections() > 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(client.call("ping", Args::new()).unwrap(), json!("pong"));
    }

    #[test]
    fn stop_lets_a_request_in_flight_finish() {
        let dir = TempDir::new();
        let (handler, release, entered) = gate();
        let mut server = Server::start(ServerConfig::new(dir.sock(), [me()]), handler).unwrap();
        let client = Client::new(dir.sock()).with_timeout(Duration::from_secs(5));
        let inflight = thread::spawn(move || client.call("wait", Args::new()));
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        let releaser = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            let _ = release.send(());
        });
        server.stop();
        assert_eq!(server.active_connections(), 0, "stop waited for the request");
        assert_eq!(inflight.join().unwrap().unwrap(), json!("released"));
        releaser.join().unwrap();
        assert!(!dir.sock().exists());
    }

    #[test]
    fn stop_does_not_hang_on_a_stuck_handler() {
        let dir = TempDir::new();
        let (handler, release, entered) = gate();
        let mut cfg = ServerConfig::new(dir.sock(), [me()]);
        cfg.shutdown_grace = Duration::from_millis(100);
        let mut server = Server::start(cfg, handler).unwrap();
        let client = Client::new(dir.sock()).with_timeout(Duration::from_millis(300));
        let stuck = thread::spawn(move || client.call("wait", Args::new()));
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        let started = Instant::now();
        server.stop();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!dir.sock().exists());
        assert!(stuck.join().unwrap().is_err(), "the client timed out");
        let _ = release.send(());
    }

    #[test]
    fn many_clients_at_once() {
        let dir = TempDir::new();
        let (_server, client) = start(&dir, &[me()], standard_router());
        let threads: Vec<_> = (0..16)
            .map(|i| {
                let c = client.clone();
                thread::spawn(move || c.call("need_int", Args::new().with("n", i)).map(|v| v == json!(i + 1)))
            })
            .collect();
        for t in threads {
            assert!(t.join().unwrap().unwrap());
        }
    }

    #[test]
    fn router_lists_and_replaces_commands() {
        let mut r = Router::new().route("b", |_| Ok(json!(1))).route("a", |_| Ok(json!(1)));
        r.insert("b", |_| Ok(json!(2)));
        assert_eq!(r.commands(), vec!["a", "b"]);
        assert_eq!(dispatch(&r, 0, br#"{"cmd":"b"}"#), Response::Ok(json!(2)));
        assert_eq!(
            dispatch(&Router::new(), 0, br#"{"cmd":"a"}"#),
            Response::Err("unknown command 'a'".into())
        );
    }
}
