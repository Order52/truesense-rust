//! One window per user, and "raise the open window" for everything that wants to show a page
//! (a second `truesense-qt --page cooling`, the PREDATOR key, a tray menu entry).
//!
//! # Protocol
//!
//! The first instance takes an exclusive `flock` on `truesense-qt.lock` in the runtime directory
//! and listens on the unix socket `truesense-qt.sock` next to it (mode `0600`, in a directory that
//! is private to the user). The lock belongs to the process, so the kernel releases it when the
//! instance dies, however it dies: **a socket file whose owner holds no lock is stale** and the next
//! start removes it. A later start that cannot get the lock connects to the socket and sends one
//! line, `show [PAGE]`, to which the first instance answers `ok` (or `err <why>`); the later
//! start then exits 0. `ping` answers `ok` and is how tests and scripts ask "is it running".
//!
//! The socket path must fit `sun_path` (108 bytes); the runtime directory (`/run/user/<uid>`) does.
#![forbid(unsafe_code)]

use std::fmt;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use ts_foundation::fsx::{FileLock, FsError};

/// The socket's file name in the runtime directory.
pub const SOCKET_NAME: &str = "truesense-qt.sock";
/// The lock file's name in the runtime directory.
pub const LOCK_NAME: &str = "truesense-qt.lock";
/// The longest request line accepted; anything longer is a mistake or an attack.
const MAX_LINE: u64 = 256;
/// How long a connection may take to say its one line.
const IO_TIMEOUT: Duration = Duration::from_secs(2);
/// A first instance takes the lock a moment before it listens: a second start retries this long.
const CONNECT_RETRIES: u32 = 40;
const CONNECT_PAUSE: Duration = Duration::from_millis(50);

/// What went wrong with the single-instance machinery.
#[derive(Debug)]
pub enum SingleError {
    /// The lock file could not be opened or locked.
    Lock(FsError),
    /// A socket operation failed.
    Socket {
        /// What was being done.
        op: &'static str,
        /// The socket's path.
        path: PathBuf,
        /// The cause.
        source: io::Error,
    },
    /// The running instance answered something other than `ok`.
    Refused(String),
}

impl fmt::Display for SingleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(e) => write!(f, "single instance lock: {e}"),
            Self::Socket { op, path, source } => {
                write!(f, "single instance socket {op} {}: {source}", path.display())
            }
            Self::Refused(why) => write!(f, "the running TrueSense window refused: {why}"),
        }
    }
}

impl std::error::Error for SingleError {}

fn sock_err(op: &'static str, path: &Path) -> impl FnOnce(io::Error) -> SingleError {
    let path = path.to_path_buf();
    move |source| SingleError::Socket { op, path, source }
}

/// The directory for lock and socket: `$XDG_RUNTIME_DIR`, else a private directory of ours in the
/// temporary directory (the same rule as the web window's, `ts_gui::gui::default_runtime_dir`).
///
/// # Errors
/// The text of the runtime-directory problem.
pub fn runtime_dir() -> Result<PathBuf, String> {
    ts_gui::gui::default_runtime_dir().map_err(|e| e.to_string())
}

/// A page name is safe to pass along when it is short and made of letters, digits, `-` and `_`.
pub fn valid_page(name: &str) -> bool {
    (1..=40).contains(&name.len()) && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The outcome of starting: this process is the window, or one already exists.
pub enum Acquired {
    /// This process owns the socket and must serve it ([`Primary::serve`]).
    Primary(Primary),
    /// Another instance is running: forward with [`send_show`] and exit.
    Existing,
}

/// The first instance: holds the lock and the listening socket.
pub struct Primary {
    listener: UnixListener,
    path: PathBuf,
    _lock: FileLock,
}

/// Becomes the first instance, or finds that there already is one.
///
/// # Errors
/// [`SingleError`] when the lock or the socket cannot be set up (an unusable runtime directory).
pub fn acquire(dir: &Path) -> Result<Acquired, SingleError> {
    let socket = dir.join(SOCKET_NAME);
    let Some(lock) = FileLock::try_exclusive(&dir.join(LOCK_NAME)).map_err(SingleError::Lock)? else {
        return Ok(Acquired::Existing);
    };
    // We hold the lock, so whoever left this socket file is gone, unless some other program answers
    // on it (an older build without the lock): then it is not ours to remove.
    if socket.exists() {
        if UnixStream::connect(&socket).is_ok() {
            return Ok(Acquired::Existing);
        }
        fs::remove_file(&socket).map_err(sock_err("remove stale", &socket))?;
    }
    let listener = UnixListener::bind(&socket).map_err(sock_err("bind", &socket))?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).map_err(sock_err("chmod", &socket))?;
    Ok(Acquired::Primary(Primary { listener, path: socket, _lock: lock }))
}

/// What the serving thread does when a client asks to show a page (`None`: just raise the window).
pub(crate) type OnShow = Box<dyn Fn(Option<String>) + Send + 'static>;

/// The running accept loop.
pub struct Server {
    stop: Arc<AtomicBool>,
    path: PathBuf,
    thread: Option<JoinHandle<()>>,
    _lock: FileLock,
}

impl Primary {
    /// Starts answering clients on a thread of its own; `on_show` runs on that thread.
    ///
    /// # Errors
    /// The operating system could not start the thread.
    pub fn serve(self, on_show: OnShow) -> io::Result<Server> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let Primary { listener, path, _lock } = self;
        let thread = thread::Builder::new().name("ts-single-instance".to_owned()).spawn(move || {
            for stream in listener.incoming() {
                if flag.load(Ordering::SeqCst) {
                    break;
                }
                if let Ok(stream) = stream {
                    handle(stream, &on_show);
                }
            }
        })?;
        Ok(Server { stop, path, thread: Some(thread), _lock })
    }
}

impl Server {
    /// Stops accepting, removes the socket file and releases the lock.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the blocked accept(), then take the socket away.
        let _ = UnixStream::connect(&self.path);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let _ = fs::remove_file(&self.path);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Reads one request line and answers it. Never panics and never blocks for long on a bad client.
fn handle(stream: UnixStream, on_show: &OnShow) {
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let mut line = String::new();
    let read = BufReader::new((&stream).take(MAX_LINE)).read_line(&mut line);
    let reply = match read {
        Ok(n) if n > 0 && line.ends_with('\n') => respond(line.trim(), on_show),
        Ok(0) => return, // a wake-up connection or a client that said nothing
        _ => "err bad request".to_owned(),
    };
    let _ = (&stream).write_all(format!("{reply}\n").as_bytes());
}

fn respond(line: &str, on_show: &OnShow) -> String {
    let mut words = line.split_whitespace();
    match (words.next(), words.next(), words.next()) {
        (Some("ping"), None, None) => "ok".to_owned(),
        (Some("show"), None, None) => {
            on_show(None);
            "ok".to_owned()
        }
        (Some("show"), Some(page), None) if valid_page(page) => {
            on_show(Some(page.to_owned()));
            "ok".to_owned()
        }
        (Some("show"), Some(_), _) => "err bad page name".to_owned(),
        _ => "err unknown request".to_owned(),
    }
}

/// Asks the running instance to show `page` (or just to come to the front) and waits for its answer.
///
/// # Errors
/// [`SingleError`] when nobody answers (after about two seconds of retrying, which covers a first
/// instance that is still starting) or the answer is not `ok`.
pub fn send_show(dir: &Path, page: Option<&str>) -> Result<(), SingleError> {
    request(dir, &page.map_or_else(|| "show".to_owned(), |p| format!("show {p}")))
}

/// Whether an instance is running and answering.
pub fn ping(dir: &Path) -> bool {
    request(dir, "ping").is_ok()
}

fn request(dir: &Path, line: &str) -> Result<(), SingleError> {
    let socket = dir.join(SOCKET_NAME);
    let mut last = None;
    let mut stream = None;
    for _ in 0..CONNECT_RETRIES {
        match UnixStream::connect(&socket) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => {
                last = Some(e);
                thread::sleep(CONNECT_PAUSE);
            }
        }
    }
    let Some(stream) = stream else {
        let source = last.unwrap_or_else(|| io::Error::from(io::ErrorKind::NotConnected));
        return Err(SingleError::Socket { op: "connect", path: socket, source });
    };
    stream.set_read_timeout(Some(IO_TIMEOUT)).map_err(sock_err("configure", &socket))?;
    stream.set_write_timeout(Some(IO_TIMEOUT)).map_err(sock_err("configure", &socket))?;
    (&stream).write_all(format!("{line}\n").as_bytes()).map_err(sock_err("write", &socket))?;
    let mut answer = String::new();
    BufReader::new(&stream).take(MAX_LINE).read_line(&mut answer).map_err(sock_err("read", &socket))?;
    match answer.trim() {
        "ok" => Ok(()),
        other => Err(SingleError::Refused(other.to_owned())),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicU32;

    /// A short private directory: unix socket paths are limited to about 100 bytes, which a
    /// scratch directory deep in a build tree can exceed.
    struct Dir(PathBuf);
    impl Dir {
        fn new() -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let p = PathBuf::from("/tmp").join(format!(
                "tsq-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&p).unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).unwrap();
            Self(p)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn primary(dir: &Path) -> Primary {
        match acquire(dir).unwrap() {
            Acquired::Primary(p) => p,
            Acquired::Existing => panic!("expected to become the first instance"),
        }
    }

    fn recorder() -> (OnShow, Arc<Mutex<Vec<Option<String>>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        (Box::new(move |p| s.lock().unwrap().push(p)), seen)
    }

    #[test]
    fn the_second_start_sends_show_and_the_first_receives_the_page() {
        let d = Dir::new();
        let (on_show, seen) = recorder();
        let server = primary(&d.0).serve(on_show).unwrap();
        assert!(matches!(acquire(&d.0).unwrap(), Acquired::Existing));
        send_show(&d.0, Some("cooling")).unwrap();
        send_show(&d.0, None).unwrap();
        assert_eq!(*seen.lock().unwrap(), vec![Some("cooling".to_owned()), None]);
        assert!(ping(&d.0));
        server.stop();
    }

    #[test]
    fn the_socket_is_private_and_removed_on_stop() {
        let d = Dir::new();
        let (on_show, _) = recorder();
        let server = primary(&d.0).serve(on_show).unwrap();
        let mode = fs::metadata(d.0.join(SOCKET_NAME)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        server.stop();
        assert!(!d.0.join(SOCKET_NAME).exists());
        assert!(!ping(&d.0), "nobody answers after stop");
    }

    #[test]
    fn a_stale_socket_without_a_lock_holder_is_replaced() {
        let d = Dir::new();
        // A crashed instance: the socket file is there, nobody listens, nobody holds the lock.
        drop(UnixListener::bind(d.0.join(SOCKET_NAME)).unwrap());
        assert!(d.0.join(SOCKET_NAME).exists());
        let (on_show, seen) = recorder();
        let server = primary(&d.0).serve(on_show).unwrap();
        send_show(&d.0, Some("home")).unwrap();
        assert_eq!(*seen.lock().unwrap(), vec![Some("home".to_owned())]);
        server.stop();
    }

    #[test]
    fn dropping_the_server_frees_the_lock_for_the_next_start() {
        let d = Dir::new();
        let (on_show, _) = recorder();
        drop(primary(&d.0).serve(on_show).unwrap());
        let (on_show, _) = recorder();
        primary(&d.0).serve(on_show).unwrap().stop();
    }

    #[test]
    fn hostile_requests_are_refused_without_effect() {
        let d = Dir::new();
        let (on_show, seen) = recorder();
        let server = primary(&d.0).serve(on_show).unwrap();
        for line in [
            "show ../../etc/passwd",
            "show a b",
            "frobnicate",
            "show \u{1F600}",
            &format!("show {}", "x".repeat(41)),
        ] {
            let r = request(&d.0, line);
            assert!(matches!(r, Err(SingleError::Refused(_))), "{line:?} -> {r:?}");
        }
        // A line that never ends, and a client that connects and says nothing, do not wedge it.
        let mut s = UnixStream::connect(d.0.join(SOCKET_NAME)).unwrap();
        s.write_all(&vec![b'a'; 4096]).unwrap();
        drop(s);
        drop(UnixStream::connect(d.0.join(SOCKET_NAME)).unwrap());
        assert!(ping(&d.0));
        assert!(seen.lock().unwrap().is_empty());
        server.stop();
    }

    #[test]
    fn page_names_are_validated() {
        assert!(valid_page("cooling") && valid_page("fan-curves") && valid_page("a_b9"));
        assert!(!valid_page("") && !valid_page("a/b") && !valid_page("a b") && !valid_page(&"x".repeat(41)));
    }
}
