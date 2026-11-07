//! One window server per user.
//!
//! The running server holds an exclusive `flock` on a lock file and publishes its port and
//! session token in a second file that only the user can read. The lock is tied to the process, so
//! it vanishes with it even after a crash: a leftover info file is never mistaken for a live
//! server. A second `truesense gui` finds the server through these two files and forwards to it
//! instead of starting another.
//!
//! Two files and not one (as the Go edition had): taking the lock then needs no `unsafe` here,
//! because [`FileLock`] of the foundation does it.
#![forbid(unsafe_code)]

use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_foundation::fsx::{self, FileLock};

use super::GuiError;

/// The lock file's name.
pub(crate) const LOCK_NAME: &str = "truesense-gui.lock";
/// The file the running server describes itself in.
pub(crate) const INFO_NAME: &str = "truesense-gui.json";

/// What the running server publishes so that a second launch can reach it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockInfo {
    /// The loopback port.
    pub port: u16,
    /// The session token (the file is private to the user).
    pub token: String,
    /// The server's process id.
    pub pid: u32,
    /// The server's version.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
}

/// Where the lock and info files live: `$XDG_RUNTIME_DIR` (private to the user, a tmpfs, emptied
/// at logout), or else a private directory of ours in the temporary directory.
///
/// `env` looks a variable up (the process environment in production) and `temp` is the temporary
/// directory, so a test needs neither.
pub fn runtime_dir_with(env: &dyn Fn(&str) -> Option<String>, temp: &Path) -> Result<PathBuf, GuiError> {
    if let Some(dir) = env("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    // /proc/self belongs to the user this process runs as: its owner is the uid, without a libc call.
    let uid = fs::metadata("/proc/self").map_err(|e| GuiError::runtime_dir(temp, &e))?.uid();
    let dir = temp.join(format!("truesense-{uid}"));
    private_dir(&dir).map_err(|e| GuiError::runtime_dir(&dir, &e))?;
    // Somebody else may have made the directory first: it is only ours if it is private and ours.
    let meta = fs::symlink_metadata(&dir).map_err(|e| GuiError::runtime_dir(&dir, &e))?;
    if !meta.is_dir() || meta.permissions().mode() & 0o777 != 0o700 || meta.uid() != uid {
        return Err(GuiError::RuntimeDir {
            dir: dir.display().to_string(),
            why: "it is not a private directory of this user".to_owned(),
        });
    }
    Ok(dir)
}

/// [`runtime_dir_with`] for this process.
pub fn default_runtime_dir() -> Result<PathBuf, GuiError> {
    runtime_dir_with(&fsx::env_nonempty, &std::env::temp_dir())
}

/// Creates the directory (and its parents) readable by this user only, the mode the lock and
/// token files below it rely on. An existing directory keeps its mode: `runtime_dir_with` judges it.
fn private_dir(dir: &Path) -> std::io::Result<()> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
}

/// The held lock. Dropping it removes the published token first, then releases the lock.
#[derive(Debug)]
pub(crate) struct Instance {
    dir: PathBuf,
    _lock: FileLock,
}

impl Instance {
    /// Takes the lock for this user; `AlreadyRunning` when another server holds it.
    pub(crate) fn acquire(dir: &Path) -> Result<Instance, GuiError> {
        private_dir(dir).map_err(|e| GuiError::runtime_dir(dir, &e))?;
        match FileLock::try_exclusive(&dir.join(LOCK_NAME)) {
            Ok(Some(lock)) => Ok(Instance { dir: dir.to_owned(), _lock: lock }),
            Ok(None) => Err(GuiError::AlreadyRunning),
            Err(e) => Err(GuiError::Io(format!("window lock: {e}"))),
        }
    }

    /// Writes the port and token for a second launch to read; the file is private (`0600`) from the
    /// moment it exists, because it holds the session token.
    pub(crate) fn publish(&self, info: &LockInfo) -> Result<(), GuiError> {
        let path = self.dir.join(INFO_NAME);
        let json = serde_json::to_vec(info).map_err(|e| GuiError::Io(format!("window info: {e}")))?;
        let tmp = self.dir.join(format!("{INFO_NAME}.{}.tmp", std::process::id()));
        let write = || -> std::io::Result<()> {
            let mut f =
                fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
            f.write_all(&json)?;
            f.sync_all()?;
            fs::rename(&tmp, &path)
        };
        write().map_err(|e| {
            let _ = fs::remove_file(&tmp);
            GuiError::Io(format!("window info {}: {e}", path.display()))
        })
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // The token goes first: from here on nobody can be pointed at a server that is leaving.
        let _ = fs::remove_file(self.dir.join(INFO_NAME));
    }
}

/// The published info, but only when a process really holds the lock.
pub(crate) fn read_live(dir: &Path) -> Option<LockInfo> {
    let probe = dir.join(LOCK_NAME);
    if !fsx::exists(&probe) {
        return None;
    }
    match FileLock::try_exclusive(&probe) {
        // Nobody holds it: whatever the info file says is a leftover.
        Ok(Some(_free)) => return None,
        Ok(None) => {}
        Err(_) => return None,
    }
    let bytes = fsx::read_limited(&dir.join(INFO_NAME), 4096).ok()?;
    let info: LockInfo = serde_json::from_slice(&bytes).ok()?;
    (info.port > 0 && !info.token.is_empty()).then_some(info)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::gui::testutil::Temp;

    fn info() -> LockInfo {
        LockInfo { port: 4242, token: "tok".to_owned(), pid: 7, version: "0.5.0".to_owned() }
    }

    #[test]
    fn nothing_is_live_without_a_lock_file() {
        let t = Temp::new("none");
        assert_eq!(read_live(&t.0), None);
    }

    #[test]
    fn a_held_lock_publishes_and_a_second_instance_is_refused() {
        let t = Temp::new("held");
        let first = Instance::acquire(&t.0).expect("first");
        first.publish(&info()).expect("publish");
        assert_eq!(read_live(&t.0), Some(info()));
        assert!(matches!(Instance::acquire(&t.0), Err(GuiError::AlreadyRunning)));
        let mode = fs::metadata(t.0.join(INFO_NAME)).expect("info").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the info file holds the token");
        drop(first);
        assert_eq!(read_live(&t.0), None, "released: the token is gone");
        assert!(Instance::acquire(&t.0).is_ok(), "and the lock is free again");
    }

    #[test]
    fn a_leftover_info_file_without_a_holder_is_not_a_live_server() {
        let t = Temp::new("stale");
        fs::write(t.0.join(LOCK_NAME), b"").expect("lock file");
        fs::write(t.0.join(INFO_NAME), serde_json::to_vec(&info()).expect("json")).expect("info file");
        assert_eq!(read_live(&t.0), None);
    }

    #[test]
    fn hostile_info_files_are_ignored() {
        let t = Temp::new("hostile");
        let held = Instance::acquire(&t.0).expect("lock");
        for text in [
            "",
            "{",
            "null",
            "[]",
            r#"{"port":0,"token":"x","pid":1}"#,
            r#"{"port":5,"token":"","pid":1}"#,
            r#"{"port":99999,"token":"x","pid":1}"#,
        ] {
            fs::write(t.0.join(INFO_NAME), text).expect("write");
            assert_eq!(read_live(&t.0), None, "{text:?}");
        }
        drop(held);
    }

    #[test]
    fn the_runtime_dir_prefers_xdg_and_otherwise_makes_a_private_one() {
        let t = Temp::new("rt");
        let xdg = runtime_dir_with(&|k| (k == "XDG_RUNTIME_DIR").then(|| "/run/user/1234".to_owned()), &t.0);
        assert_eq!(xdg.expect("xdg"), PathBuf::from("/run/user/1234"));
        let fallback = runtime_dir_with(&|_| None, &t.0).expect("fallback");
        assert!(fallback.starts_with(&t.0));
        assert_eq!(fs::metadata(&fallback).expect("dir").permissions().mode() & 0o777, 0o700);
        // A directory somebody else made with open permissions is refused.
        fs::set_permissions(&fallback, fs::Permissions::from_mode(0o755)).expect("chmod");
        assert!(matches!(runtime_dir_with(&|_| None, &t.0), Err(GuiError::RuntimeDir { .. })));
    }
}
