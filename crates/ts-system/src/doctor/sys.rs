//! The real operating system behind [`Host`]: the only place of the doctor that needs `unsafe`,
//! two libc calls that have no safe equivalent in the standard library.
#![allow(unsafe_code)]

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ts_foundation::fsx;

use super::Host;

/// The effective user id of this process.
pub(super) fn effective_uid() -> u32 {
    // SAFETY: `geteuid` takes no arguments, has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// `access(2)`: may this process read, and with `write` also write, `path`? The kernel answers
/// with the process's effective ids and the file's ACLs, which a check of the mode bits would
/// miss (udev gives the seat's user access to `/dev/truesense` through an ACL). A path with an
/// interior NUL cannot name a file, so it is not accessible.
pub(super) fn access(path: &Path, write: bool) -> bool {
    let Ok(c_path) = CString::new(path.as_os_str().as_bytes()) else { return false };
    let mode = libc::R_OK | if write { libc::W_OK } else { 0 };
    // SAFETY: `c_path` is a valid NUL-terminated string that outlives the call, and `access`
    // only reads it.
    unsafe { libc::access(c_path.as_ptr(), mode) == 0 }
}

/// The user id of `user` in the text of a passwd file (`name:password:uid:gid:...`), if present.
/// Malformed lines, comments and a uid that is not a number are skipped, never an error: the
/// caller then inspects the current session instead.
pub(super) fn passwd_uid(passwd: &str, user: &str) -> Option<u32> {
    passwd.lines().filter(|line| !line.starts_with('#')).find_map(|line| {
        let mut fields = line.split(':');
        let (name, _password, uid) = (fields.next()?, fields.next()?, fields.next()?);
        if name == user { uid.parse().ok() } else { None }
    })
}

/// Whether `dir/name` is a regular file with an execute bit.
fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The real machine: the process environment, `/etc/passwd`, `PATH`, `access(2)` and the clock.
#[derive(Debug, Clone)]
pub struct SystemHost {
    passwd: PathBuf,
}

impl SystemHost {
    /// The real machine.
    pub fn new() -> Self {
        Self { passwd: PathBuf::from("/etc/passwd") }
    }

    /// Reads the user database from `passwd` instead of `/etc/passwd` (tests).
    #[must_use]
    pub fn with_passwd(mut self, passwd: impl Into<PathBuf>) -> Self {
        self.passwd = passwd.into();
        self
    }
}

impl Default for SystemHost {
    fn default() -> Self {
        Self::new()
    }
}

impl Host for SystemHost {
    fn env_var(&self, name: &str) -> Option<String> {
        fsx::env_nonempty(name)
    }

    fn euid(&self) -> u32 {
        effective_uid()
    }

    fn lookup_uid(&self, user: &str) -> Option<u32> {
        passwd_uid(&fsx::read_to_string(&self.passwd).ok()?, user)
    }

    fn program_on_path(&self, name: &str) -> bool {
        if name.is_empty() || name.contains('/') {
            return false;
        }
        let Some(path) = fsx::env_nonempty("PATH") else { return false };
        std::env::split_paths(&path).any(|dir| is_executable(&dir.join(name)))
    }

    fn can_access(&self, path: &Path, write: bool) -> bool {
        access(path, write)
    }

    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}
