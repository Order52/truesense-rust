//! Knowing that a file changed without asking: inotify on the directories of the key service's
//! files (the user's configuration and the user state).
//!
//! Before this the key service looked at the configuration's modification time every 1.5 s and the
//! two lighting loops read the user state twice a second each, about five wake-ups a second for a
//! program that has nothing to do. Now one thread blocks in `read` on an inotify descriptor (it is
//! not scheduled while nothing is written) and, when something in a watched directory changes,
//!
//! * rings the process bell (`ts_foundation::wake`), which ends the sleeps of the lighting loops, and
//! * writes a byte into a pipe whose read end the key loop polls together with the keyboard.
//!
//! An event only means "look now": every reader still compares what it reads with what it had, so a
//! change in the same directory that is not ours (a lock file, a neighbour) costs one extra look.
//!
//! This is the one place of the crate besides `hid.rs` that needs `unsafe`: the system calls are
//! `inotify_init1`, `inotify_add_watch`, `inotify_rm_watch`, `read`, `pipe2`, `write` and `poll`,
//! each on a descriptor this file owns.
#![allow(unsafe_code)]

use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use ts_foundation::wake;

/// What counts as a change: a file written and closed, created, moved into place (the atomic
/// write every store of this program uses), deleted or moved away.
const MASK: u32 = libc::IN_CLOSE_WRITE
    | libc::IN_MOVED_TO
    | libc::IN_MOVED_FROM
    | libc::IN_CREATE
    | libc::IN_DELETE
    | libc::IN_DELETE_SELF
    | libc::IN_MOVE_SELF;

/// Watches directories and tells the rest of the process when something in them changed.
pub struct Watcher {
    inotify: Arc<OwnedFd>,
    watches: Vec<libc::c_int>,
    /// Whether every directory asked for could be watched.
    complete: bool,
    /// Read end of the pipe the key loop polls; the thread writes to the other end.
    signal_read: OwnedFd,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// Starts watching `dir`; `None` when it cannot be (it does not exist, or the system is out of watches).
fn add_watch(inotify: &OwnedFd, dir: &Path) -> Option<libc::c_int> {
    let path = CString::new(dir.as_os_str().as_bytes()).ok()?;
    // SAFETY: `path` is a valid NUL-terminated string that outlives the call; the descriptor is an
    // inotify instance owned by the caller.
    let wd = unsafe { libc::inotify_add_watch(inotify.as_raw_fd(), path.as_ptr(), MASK) };
    (wd >= 0).then_some(wd)
}

fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: `fds` is a valid array of two ints for `pipe2` to fill in; both ends are made
    // non-blocking so neither a full pipe nor an empty one can ever hang a caller.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both descriptors were just opened by `pipe2` and are owned from here on.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

impl Watcher {
    /// Watches the directory of every file in `files` (a directory that does not exist is skipped:
    /// there is nothing to change in it, and the first file written creates it). Starts the thread.
    ///
    /// # Errors
    /// The inotify instance, the pipe or the thread could not be created.
    pub fn start(files: &[PathBuf]) -> io::Result<Self> {
        // SAFETY: no arguments are pointers; the result is checked and wrapped in an owner at once.
        let raw = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `raw` was just returned by `inotify_init1` and nothing else owns it.
        let inotify = Arc::new(unsafe { OwnedFd::from_raw_fd(raw) });
        let mut dirs: Vec<&Path> = files.iter().filter_map(|f| f.parent()).collect();
        dirs.sort();
        dirs.dedup();
        let mut watches = Vec::new();
        let mut complete = true;
        // A directory that does not exist yet (a user who never saved a setting) is waited for
        // through its nearest existing parent: the creation of the directory is an event there,
        // and the thread then watches the directory itself.
        let mut missing: Vec<PathBuf> = Vec::new();
        for dir in dirs {
            if add_watch(&inotify, dir).map(|wd| watches.push(wd)).is_some() {
                continue;
            }
            missing.push(dir.to_path_buf());
            match dir.ancestors().skip(1).find_map(|up| add_watch(&inotify, up)) {
                Some(wd) => watches.push(wd),
                None => complete = false,
            }
        }
        let (signal_read, signal_write) = pipe()?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (inotify, stop) = (inotify.clone(), stop.clone());
            std::thread::Builder::new().name("ts-watch".into()).stack_size(64 * 1024).spawn(move || {
                // Room for a burst of events; each is 16 bytes plus the name.
                let mut buffer = [0u8; 4096];
                loop {
                    // SAFETY: `buffer` is valid for `len` bytes and the descriptor is owned by
                    // `inotify`, kept alive by this closure's clone of the Arc.
                    let n =
                        unsafe { libc::read(inotify.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
                    if stop.load(Ordering::SeqCst) {
                        return;
                    }
                    if n < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                        return;
                    }
                    if n > 0 {
                        missing.retain(|dir| add_watch(&inotify, dir).is_none());
                        let byte = 1u8;
                        // SAFETY: `signal_write` is open for the life of this closure and `byte`
                        // is a valid one-byte buffer; a full pipe already holds a wake-up, so a
                        // failed write is fine.
                        unsafe {
                            libc::write(signal_write.as_raw_fd(), (&raw const byte).cast(), 1);
                        }
                        wake::ring();
                    }
                }
            })?
        };
        Ok(Self { inotify, watches, complete, signal_read, stop, thread: Some(thread) })
    }

    /// Whether every directory, or at least an existing parent of it, could be watched. When one could not (it does not exist yet), a
    /// change in it is not announced, and the callers keep a short fallback pause.
    pub fn complete(&self) -> bool {
        self.complete
    }

    /// The descriptor that becomes readable when something changed, for `poll`.
    pub fn fd(&self) -> RawFd {
        self.signal_read.as_raw_fd()
    }

    /// Empties the pipe after a wake-up, so the next `poll` blocks again.
    pub fn drain(&self) {
        let mut buffer = [0u8; 64];
        loop {
            // SAFETY: `buffer` is valid for its length and the descriptor is owned by `self`;
            // the pipe is non-blocking, so this returns at once when it is empty.
            let n =
                unsafe { libc::read(self.signal_read.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
            if n <= 0 {
                return;
            }
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Removing a watch makes the kernel queue an IN_IGNORED event, which ends the thread's
        // blocking read.
        let mut removed = false;
        for wd in &self.watches {
            // SAFETY: the descriptor is the owned inotify instance and `wd` came from it.
            removed |= unsafe { libc::inotify_rm_watch(self.inotify.as_raw_fd(), *wd) } == 0;
        }
        match self.thread.take() {
            Some(thread) if removed => {
                let _ = thread.join();
            }
            // Nothing could be removed (nothing watched, or the directories are gone): nothing
            // will wake the read, so the thread is left to end with the process.
            _ => {}
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ts-watch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn readable(fd: RawFd, timeout_ms: i32) -> bool {
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        // SAFETY: one initialised `pollfd`, count 1.
        unsafe { libc::poll(&mut pfd, 1, timeout_ms) > 0 }
    }

    #[test]
    fn nothing_changes_nothing_wakes() {
        let dir = temp_dir("quiet");
        let watcher = Watcher::start(&[dir.join("user.json")]).unwrap();
        assert!(!readable(watcher.fd(), 100));
        drop(watcher);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_written_in_the_directory_wakes_the_poll_and_the_bell() {
        let dir = temp_dir("write");
        let watcher = Watcher::start(&[dir.join("user.json")]).unwrap();
        let rung = Arc::new(AtomicBool::new(false));
        let waiter = {
            let rung = rung.clone();
            std::thread::spawn(move || {
                // The bell is shared by the whole process: wait for a ring after this point.
                let before = Instant::now();
                wake::sleep(Duration::from_secs(10));
                rung.store(before.elapsed() < Duration::from_secs(9), Ordering::SeqCst);
            })
        };
        std::thread::sleep(Duration::from_millis(100));
        // The atomic write every store here uses: a temporary file moved over the name.
        std::fs::write(dir.join("user.json.tmp"), b"{}").unwrap();
        std::fs::rename(dir.join("user.json.tmp"), dir.join("user.json")).unwrap();
        assert!(readable(watcher.fd(), 5000), "the event reaches the pipe");
        // The create, the write and the rename are separate kernel events and the reader thread
        // may deliver them in more than one batch: drain until it has gone quiet.
        let quiet = (0..50).any(|_| {
            watcher.drain();
            !readable(watcher.fd(), 100)
        });
        assert!(quiet, "drained");
        waiter.join().unwrap();
        assert!(rung.load(Ordering::SeqCst), "the bell cut the sleeper short");
        drop(watcher);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_directory_is_skipped_and_dropping_never_hangs() {
        let watcher = Watcher::start(&[PathBuf::from("/nonexistent-ts/dir/file.json")]).unwrap();
        assert!(watcher.complete(), "the nearest existing parent stands in until the directory exists");
        assert!(!readable(watcher.fd(), 50));
        drop(watcher);
    }

    #[test]
    fn a_directory_created_later_is_watched_from_then_on() {
        let base = temp_dir("late");
        let file = base.join("sub").join("config.toml");
        let watcher = Watcher::start(std::slice::from_ref(&file)).unwrap();
        assert!(watcher.complete(), "the parent is watched until the directory exists");
        std::fs::create_dir(base.join("sub")).unwrap();
        assert!(readable(watcher.fd(), 5000), "the creation is an event");
        watcher.drain();
        std::thread::sleep(Duration::from_millis(100));
        std::fs::write(&file, b"x = 1").unwrap();
        assert!(readable(watcher.fd(), 5000), "and the file written in it is seen");
        drop(watcher);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn dropping_a_watcher_ends_its_thread() {
        let dir = temp_dir("drop");
        let watcher = Watcher::start(&[dir.join("a")]).unwrap();
        let started = Instant::now();
        drop(watcher);
        assert!(started.elapsed() < Duration::from_secs(5));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
