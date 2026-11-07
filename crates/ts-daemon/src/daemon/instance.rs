//! One daemon per machine.
//!
//! systemd never starts a unit twice, but nothing else stops `sudo truesense daemon` typed in a
//! terminal, the old Go or Python daemon still running during an upgrade, or the user's own fan
//! service. Two daemons would each write the fans every second, and the socket server of the
//! second deletes the first one's socket file ("a stale socket is replaced"), leaving the first
//! daemon running and unreachable. So the daemon takes an exclusive lock beside the socket
//! (released by the kernel when the process dies, so a crash never blocks the restart) and, for
//! daemons of other editions that do not take it, refuses when something answers on the socket.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use ts_foundation::fsx::{self, FileLock};

use super::DaemonError;

/// The lock file of the daemon whose control socket is `socket`: beside it, so a test that
/// puts the socket in a temporary directory gets its own lock.
pub(super) fn lock_path(socket: &Path) -> PathBuf {
    socket.with_extension("lock")
}

/// What the daemon holds while it runs.
#[derive(Debug)]
pub(super) struct Instance {
    _lock: Option<FileLock>,
}

/// Takes the lock and checks the socket; `Err(AlreadyRunning)` when another daemon is there.
/// A lock file that cannot be created (a run directory that is not writable) is not a reason
/// to leave the fans unmanaged: the caller goes on and the socket start decides, and the second
/// check still catches a daemon that answers.
pub(super) fn acquire(socket: &Path) -> Result<(Instance, Option<String>), DaemonError> {
    let lock = lock_path(socket);
    if let Some(dir) = lock.parent().filter(|d| !d.as_os_str().is_empty()) {
        // A missing run directory is the service manager's to create; creating it here is harmless.
        let _ = fsx::ensure_dir(dir, 0o755);
    }
    let (held, problem) = match FileLock::try_exclusive(&lock) {
        Ok(Some(held)) => (Some(held), None),
        Ok(None) => {
            return Err(DaemonError::AlreadyRunning(format!("it holds {}", lock.display())));
        }
        Err(e) => (None, Some(format!("no single-instance lock: {e}"))),
    };
    if UnixStream::connect(socket).is_ok() {
        return Err(DaemonError::AlreadyRunning(format!("it answers on {}", socket.display())));
    }
    Ok((Instance { _lock: held }, problem))
}
