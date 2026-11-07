//! File helpers every other crate needs: reads that always close the file and cannot be made
//! to allocate without bound, atomic writes, directory creation, sysfs-style number reads and
//! an advisory lock.
//!
//! Why atomic writes matter here: the service state, the user state, the config and the live
//! state file that status bars poll are all read by other processes while we rewrite them. A
//! plain `open(..., "w")` leaves a window with an empty or half-written file, and a power loss
//! in that window (this is a laptop that freezes when an undervolt goes wrong) would destroy
//! the settings. The temporary file therefore lives in the **same directory** (a rename across
//! file systems is not atomic), has an unpredictable name and is created with `O_EXCL` (the
//! Python edition used `path + ".tmp"`, which in a world-writable `/tmp` lets another user
//! plant a symlink there), is synced, and only then renamed over the target.
#![deny(unsafe_code)]

use std::ffi::OsString;
use std::fmt;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The largest file [`read_limited`] (and so every settings reader) accepts. Settings files are
/// a few kilobytes; a file past this size is damaged or hostile, and refusing it keeps a
/// root daemon from allocating whatever a user-writable path points it at.
pub const MAX_SETTINGS_FILE: u64 = 1 << 20;

/// How many unique temp names [`write_atomic`] tries before giving up (only a collision with
/// an existing file makes it try another).
const TEMP_ATTEMPTS: u32 = 32;

/// A failed file operation, carrying the operation and the path so the message is useful
/// without the caller adding context.
#[derive(Debug)]
pub enum FsError {
    /// A system call failed.
    Io {
        /// What was being done, e.g. `"read"`, `"rename"`.
        op: &'static str,
        /// The file or directory involved.
        path: PathBuf,
        /// The underlying error.
        source: io::Error,
    },
    /// The file was readable but its text is not the number a sysfs-style read expected.
    NotANumber {
        /// The file that was read.
        path: PathBuf,
        /// The (trimmed, possibly truncated) text found there.
        text: String,
    },
    /// The file is larger than the limit given to a bounded read.
    TooLarge {
        /// The file that was read.
        path: PathBuf,
        /// The limit in bytes.
        limit: u64,
    },
    /// A directory was wanted and something else is there.
    NotADirectory {
        /// The offending path.
        path: PathBuf,
    },
}

impl FsError {
    fn io(op: &'static str, path: &Path, source: io::Error) -> Self {
        FsError::Io { op, path: path.to_path_buf(), source }
    }

    /// The `io::ErrorKind` when the failure came from a system call.
    pub fn kind(&self) -> Option<io::ErrorKind> {
        match self {
            FsError::Io { source, .. } => Some(source.kind()),
            _ => None,
        }
    }

    /// True when the file or directory does not exist. Callers use this to tell "never
    /// written" (fine, use the default) from "unreadable" (worth reporting).
    pub fn is_not_found(&self) -> bool {
        self.kind() == Some(io::ErrorKind::NotFound)
    }
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FsError::Io { op, path, source } => write!(f, "{op} {}: {source}", path.display()),
            FsError::NotANumber { path, text } => write!(f, "{}: not a number: {text:?}", path.display()),
            FsError::TooLarge { path, limit } => write!(f, "{}: larger than {limit} bytes", path.display()),
            FsError::NotADirectory { path } => write!(f, "{}: exists and is not a directory", path.display()),
        }
    }
}

impl std::error::Error for FsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FsError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// The real-process environment lookup, with an empty value counted as unset. Every function
/// in this crate that depends on the environment takes a lookup closure so tests can inject
/// one; this is what production code passes.
pub fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// Whether `path` exists (following symlinks). Any error other than "not found", such as
/// a permission problem on a parent, counts as existing: the caller asked "is something there
/// that I must not clobber".
pub fn exists(path: &Path) -> bool {
    !matches!(fs::metadata(path), Err(e) if e.kind() == io::ErrorKind::NotFound)
}

/// Reads a whole file as bytes, refusing one larger than `limit` bytes.
pub fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>, FsError> {
    let file = File::open(path).map_err(|e| FsError::io("open", path, e))?;
    let mut buf = Vec::new();
    // One byte past the limit is read so "exactly limit" is accepted and "more" is detected
    // without trusting a size the file can change after we asked.
    file.take(limit.saturating_add(1)).read_to_end(&mut buf).map_err(|e| FsError::io("read", path, e))?;
    if buf.len() as u64 > limit {
        return Err(FsError::TooLarge { path: path.to_path_buf(), limit });
    }
    Ok(buf)
}

/// Reads a UTF-8 text file (at most [`MAX_SETTINGS_FILE`] bytes). Invalid UTF-8 is an error,
/// never a panic or a lossy guess.
pub fn read_to_string(path: &Path) -> Result<String, FsError> {
    let bytes = read_limited(path, MAX_SETTINGS_FILE)?;
    String::from_utf8(bytes)
        .map_err(|e| FsError::io("read", path, io::Error::new(io::ErrorKind::InvalidData, e.to_string())))
}

/// Reads a text file and strips surrounding whitespace: the sysfs convention (values end in a newline).
pub fn read_trim(path: &Path) -> Result<String, FsError> {
    read_to_string(path).map(|s| s.trim().to_owned())
}

/// Like [`read_trim`] but any failure is `None`: for optional kernel files that may not exist
/// on this model.
pub fn read_trim_opt(path: &Path) -> Option<String> {
    read_trim(path).ok()
}

/// Reads a file that holds one integer (the sysfs convention).
pub fn read_int(path: &Path) -> Result<i64, FsError> {
    let text = read_trim(path)?;
    text.parse().map_err(|_| not_a_number(path, &text))
}

/// Reads a file that holds one decimal number. `NaN` and infinities are rejected: a sensor
/// file that says so is broken, and they would poison every comparison downstream.
pub fn read_float(path: &Path) -> Result<f64, FsError> {
    let text = read_trim(path)?;
    match text.parse::<f64>() {
        Ok(v) if v.is_finite() => Ok(v),
        _ => Err(not_a_number(path, &text)),
    }
}

fn not_a_number(path: &Path, text: &str) -> FsError {
    FsError::NotANumber { path: path.to_path_buf(), text: text.chars().take(64).collect() }
}

/// Writes `data` to an **existing** file without creating or replacing it: the way to write a
/// kernel pseudo-file (sysfs, procfs) where a missing file must stay an error and the write
/// must reach the same inode. Not atomic; do not use it for regular files.
pub fn write_existing(path: &Path, data: &[u8]) -> Result<(), FsError> {
    let mut file = OpenOptions::new().write(true).open(path).map_err(|e| FsError::io("open", path, e))?;
    file.write_all(data).map_err(|e| FsError::io("write", path, e))
}

/// Creates `path` and any missing parents, giving each directory it creates `mode` (before
/// the umask). An existing directory is left alone, its permissions untouched. Something
/// that exists and is not a directory is an error rather than being replaced.
pub fn ensure_dir(path: &Path, mode: u32) -> Result<(), FsError> {
    match fs::metadata(path) {
        Ok(m) if m.is_dir() => return Ok(()),
        Ok(_) => return Err(FsError::NotADirectory { path: path.to_path_buf() }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(FsError::io("stat", path, e)),
    }
    DirBuilder::new().recursive(true).mode(mode).create(path).map_err(|e| FsError::io("mkdir", path, e))
}

/// Replaces the file at `path` with `data`, atomically and durably: a reader sees the old
/// content or the new, never a mixture, and after a crash the new content has reached the
/// disk. The file ends up with exactly `mode` (the umask is overridden, since a status file
/// that must be world-readable should not depend on the caller's umask). The parent
/// directory must exist; see [`write_atomic_mkdir`].
pub(crate) fn write_atomic(path: &Path, data: &[u8], mode: u32) -> Result<(), FsError> {
    write_atomic_impl(path, data, mode, true)
}

/// [`write_atomic`] without the `fsync`s: still all-or-nothing for readers, but a power loss
/// may leave the old content. For files rewritten every second and meaningless after a
/// reboot, such as the live state for status bars, where a disk sync per tick would be waste.
pub(crate) fn write_atomic_nosync(path: &Path, data: &[u8], mode: u32) -> Result<(), FsError> {
    write_atomic_impl(path, data, mode, false)
}

/// `write_atomic` after creating the parent directory (mode `0o755`) when it is missing.
pub fn write_atomic_mkdir(path: &Path, data: &[u8], mode: u32) -> Result<(), FsError> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        ensure_dir(dir, 0o755)?;
    }
    write_atomic(path, data, mode)
}

/// Removes the temporary files of [`write_atomic`] that a crash left beside `path`
/// (`.<name>.<pid>.<nanos>.<n>.tmp`), returning how many it removed. A process killed between
/// creating its temp file and renaming it leaves one behind each time; a daemon that crash-loops
/// would otherwise fill the directory. Call it only when no other process writes `path`
/// (the daemon, after it took its single-instance lock): it cannot tell a dead writer's file
/// from a live one's.
pub(crate) fn remove_stale_temps(path: &Path) -> usize {
    let (Some(dir), Some(name)) = (path.parent().filter(|d| !d.as_os_str().is_empty()), path.file_name())
    else {
        return 0;
    };
    let prefix = format!(".{}.", name.to_string_lossy());
    let Ok(entries) = fs::read_dir(dir) else { return 0 };
    entries
        .filter_map(Result::ok)
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.starts_with(&prefix) && n.ends_with(".tmp") && e.file_type().is_ok_and(|t| t.is_file())
        })
        .filter(|e| fs::remove_file(e.path()).is_ok())
        .count()
}

fn write_atomic_impl(path: &Path, data: &[u8], mode: u32, sync: bool) -> Result<(), FsError> {
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    let name = path.file_name().ok_or_else(|| {
        FsError::io("write", path, io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))
    })?;
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    let mut collision = None;
    for attempt in 0..TEMP_ATTEMPTS {
        let mut tmp_name = OsString::from(".");
        tmp_name.push(name);
        tmp_name.push(format!(".{}.{nanos:x}.{attempt}.tmp", std::process::id()));
        let tmp = dir.join(tmp_name);
        // create_new is O_CREAT|O_EXCL: it never follows a planted symlink and never reuses a file.
        match OpenOptions::new().write(true).create_new(true).mode(mode).open(&tmp) {
            Ok(file) => return fill_and_rename(file, &tmp, path, data, mode, sync),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => collision = Some(e),
            Err(e) => return Err(FsError::io("create temp file for", path, e)),
        }
    }
    let err = collision.unwrap_or_else(|| io::Error::from(io::ErrorKind::AlreadyExists));
    Err(FsError::io("create temp file for", path, err))
}

fn fill_and_rename(
    mut file: File,
    tmp: &Path,
    target: &Path,
    data: &[u8],
    mode: u32,
    sync: bool,
) -> Result<(), FsError> {
    let result = (|| {
        file.write_all(data).map_err(|e| FsError::io("write", tmp, e))?;
        file.set_permissions(fs::Permissions::from_mode(mode)).map_err(|e| FsError::io("chmod", tmp, e))?;
        if sync {
            file.sync_all().map_err(|e| FsError::io("fsync", tmp, e))?;
        }
        drop(file);
        fs::rename(tmp, target).map_err(|e| FsError::io("rename to", target, e))
    })();
    match result {
        Ok(()) => {
            if sync {
                sync_parent(target);
            }
            Ok(())
        }
        Err(e) => {
            // Leave no stray temp file behind; the original error is the one worth reporting.
            let _ = fs::remove_file(tmp);
            Err(e)
        }
    }
}

/// Syncs the directory so the rename itself survives a crash. Best effort: the data is
/// already in place, and some file systems refuse to open a directory for this.
fn sync_parent(path: &Path) {
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
}

/// An exclusive advisory lock (`flock(2)`) on a file, released when the value is dropped (or
/// the process dies, so a crashed holder never blocks anyone). It serialises read-modify-write
/// cycles between *processes*: the window, the key service and the command line all update the
/// same user state.
#[derive(Debug)]
pub struct FileLock {
    _file: File,
}

impl FileLock {
    /// Opens (creating it, mode `0o644`) the lock file and waits until the lock is ours.
    pub fn exclusive(path: &Path) -> Result<FileLock, FsError> {
        Self::acquire(path, false).and_then(|l| {
            l.ok_or_else(|| FsError::io("lock", path, io::Error::from(io::ErrorKind::WouldBlock)))
        })
    }

    /// Like [`FileLock::exclusive`] but returns `Ok(None)` at once when another holder has the lock.
    pub fn try_exclusive(path: &Path) -> Result<Option<FileLock>, FsError> {
        Self::acquire(path, true)
    }

    fn acquire(path: &Path, nonblocking: bool) -> Result<Option<FileLock>, FsError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o644)
            .open(path)
            .map_err(|e| FsError::io("open lock file", path, e))?;
        let op = libc::LOCK_EX | if nonblocking { libc::LOCK_NB } else { 0 };
        loop {
            match flock(&file, op) {
                Ok(()) => return Ok(Some(FileLock { _file: file })),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if nonblocking && e.kind() == io::ErrorKind::WouldBlock => return Ok(None),
                Err(e) => return Err(FsError::io("lock", path, e)),
            }
        }
    }
}

/// The only `unsafe` in this module: `std::fs::File::lock` would do this safely, but it needs
/// Rust 1.89 and the workspace promises 1.85.
#[allow(unsafe_code)]
fn flock(file: &File, op: libc::c_int) -> io::Result<()> {
    // SAFETY: `flock` only reads its two integer arguments. The descriptor is valid because
    // `file` is borrowed for the whole call and owns it; no memory is passed to the kernel.
    let rc = unsafe { libc::flock(file.as_raw_fd(), op) };
    if rc == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
pub(crate) mod testing {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A fresh temp directory, removed on drop.
    pub(crate) struct TempDir(pub PathBuf);
    impl TempDir {
        pub(crate) fn new(tag: &str) -> TempDir {
            static N: AtomicU32 = AtomicU32::new(0);
            let p = std::env::temp_dir().join(format!(
                "ts-foundation-{tag}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&p).expect("create temp dir");
            TempDir(p)
        }
        pub(crate) fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .expect("read_dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn atomic_write_creates_replaces_and_leaves_no_temp_file() {
        let d = TempDir::new("fsx-atomic");
        let p = d.join("state.json");
        write_atomic(&p, b"one", 0o644).expect("first write");
        assert_eq!(fs::read(&p).expect("read"), b"one");
        write_atomic(&p, b"two, longer", 0o644).expect("second write");
        write_atomic(&p, b"3", 0o644).expect("third write");
        assert_eq!(fs::read(&p).expect("read"), b"3", "a shorter write must not keep the old tail");
        assert_eq!(entries(&d.0), vec!["state.json"]);
    }

    #[test]
    fn atomic_write_sets_the_exact_mode_whatever_the_umask() {
        let d = TempDir::new("fsx-mode");
        for (name, mode) in [("a", 0o644u32), ("b", 0o600), ("c", 0o666)] {
            let p = d.join(name);
            write_atomic(&p, b"x", mode).expect("write");
            assert_eq!(fs::metadata(&p).expect("stat").permissions().mode() & 0o7777, mode, "mode of {name}");
        }
        // Replacing an existing file takes the new mode too.
        let p = d.join("a");
        write_atomic_nosync(&p, b"y", 0o600).expect("rewrite");
        assert_eq!(fs::metadata(&p).expect("stat").permissions().mode() & 0o7777, 0o600);
    }

    #[test]
    fn atomic_write_needs_the_directory_unless_asked_to_make_it() {
        let d = TempDir::new("fsx-mkdir");
        let p = d.join("sub/deeper/file");
        let err = write_atomic(&p, b"x", 0o644).expect_err("parent is missing");
        assert!(err.is_not_found(), "{err}");
        write_atomic_mkdir(&p, b"x", 0o644).expect("mkdir variant");
        assert_eq!(fs::read(&p).expect("read"), b"x");
    }

    #[test]
    fn atomic_write_never_follows_a_symlink_at_the_target_or_the_temp_name() {
        let d = TempDir::new("fsx-symlink");
        let victim = d.join("victim");
        fs::write(&victim, b"precious").expect("victim");
        let p = d.join("state");
        symlink(&victim, &p).expect("symlink");
        write_atomic(&p, b"new", 0o644).expect("write over a symlink");
        assert_eq!(fs::read(&victim).expect("victim"), b"precious", "the symlink target is untouched");
        assert_eq!(fs::read(&p).expect("read"), b"new");
        assert!(
            !fs::symlink_metadata(&p).expect("stat").file_type().is_symlink(),
            "the link itself was replaced"
        );
    }

    #[test]
    fn failed_write_leaves_the_old_file_and_no_temp_file() {
        let d = TempDir::new("fsx-fail");
        // Renaming a file over a directory fails after the temp file was written.
        let target = d.join("is-a-dir");
        fs::create_dir(&target).expect("dir");
        let err = write_atomic(&target, b"x", 0o644).expect_err("cannot replace a directory");
        assert!(matches!(err, FsError::Io { op: "rename to", .. }), "{err}");
        assert_eq!(entries(&d.0), vec!["is-a-dir"], "temp file was cleaned up");
    }

    #[test]
    fn atomic_write_rejects_a_path_without_a_file_name() {
        for p in ["/", ".."] {
            assert!(write_atomic(Path::new(p), b"x", 0o644).is_err(), "{p}");
        }
    }

    #[test]
    fn ensure_dir_is_idempotent_and_refuses_files() {
        let d = TempDir::new("fsx-ensure");
        let p = d.join("a/b/c");
        ensure_dir(&p, 0o750).expect("create");
        ensure_dir(&p, 0o700).expect("again");
        assert!(p.is_dir());
        let f = d.join("file");
        fs::write(&f, b"x").expect("file");
        assert!(matches!(ensure_dir(&f, 0o755), Err(FsError::NotADirectory { .. })));
        assert!(matches!(ensure_dir(&f.join("below"), 0o755), Err(FsError::Io { .. })));
    }

    #[test]
    fn number_reads_follow_the_sysfs_convention() {
        let d = TempDir::new("fsx-read");
        let cases: [(&str, &str, Option<i64>); 8] = [
            ("1", "42\n", Some(42)),
            ("2", "  -7 \n", Some(-7)),
            ("3", "", None),
            ("4", "abc", None),
            ("5", "4.5", None),
            ("6", "99999999999999999999999", None),
            ("7", "1 2", None),
            ("8", "0x10", None),
        ];
        for (name, text, want) in cases {
            let p = d.join(name);
            fs::write(&p, text).expect("write");
            assert_eq!(read_int(&p).ok(), want, "{text:?}");
        }
        let p = d.join("f");
        for (text, want) in
            [("45.5\n", Some(45.5)), ("3", Some(3.0)), ("nan", None), ("inf", None), ("", None), ("x", None)]
        {
            fs::write(&p, text).expect("write");
            assert_eq!(read_float(&p).ok(), want, "{text:?}");
        }
        assert!(read_int(&d.join("missing")).expect_err("missing").is_not_found());
        assert_eq!(read_trim_opt(&d.join("missing")), None);
        fs::write(&p, " hi \n").expect("write");
        assert_eq!(read_trim(&p).expect("trim"), "hi");
    }

    #[test]
    fn a_long_garbage_value_is_cut_in_the_error_message() {
        let d = TempDir::new("fsx-long");
        let p = d.join("n");
        fs::write(&p, "z".repeat(10_000)).expect("write");
        let msg = read_int(&p).expect_err("not a number").to_string();
        assert!(msg.len() < 300, "{}", msg.len());
    }

    #[test]
    fn reads_refuse_oversize_and_non_utf8_files() {
        let d = TempDir::new("fsx-limit");
        let p = d.join("big");
        fs::write(&p, vec![b'a'; 11]).expect("write");
        assert_eq!(read_limited(&p, 11).expect("exactly the limit").len(), 11);
        assert!(matches!(read_limited(&p, 10), Err(FsError::TooLarge { limit: 10, .. })));
        fs::write(&p, [0xff, 0xfe, 0x00]).expect("write");
        assert!(read_to_string(&p).is_err());
        // A directory is a read error, not a panic.
        assert!(read_to_string(&d.0).is_err());
    }

    #[test]
    fn write_existing_does_not_create() {
        let d = TempDir::new("fsx-existing");
        let p = d.join("knob");
        assert!(write_existing(&p, b"1").expect_err("no file").is_not_found());
        assert!(!p.exists());
        fs::write(&p, b"0").expect("create");
        write_existing(&p, b"1").expect("write");
        assert_eq!(fs::read(&p).expect("read"), b"1");
    }

    #[test]
    fn exists_reports_files_dirs_and_nothing() {
        let d = TempDir::new("fsx-exists");
        assert!(exists(&d.0));
        assert!(!exists(&d.join("nope")));
        // A dangling symlink: nothing at the end of it.
        symlink(d.join("nowhere"), d.join("dangling")).expect("symlink");
        assert!(!exists(&d.join("dangling")));
    }

    #[test]
    fn env_lookup_treats_empty_as_unset() {
        assert_eq!(env_nonempty("TS_FOUNDATION_SURELY_UNSET_VARIABLE"), None);
    }

    #[test]
    fn the_lock_excludes_a_second_holder_until_dropped() {
        let d = TempDir::new("fsx-lock");
        let p = d.join("x.lock");
        let first = FileLock::exclusive(&p).expect("first lock");
        assert!(FileLock::try_exclusive(&p).expect("try").is_none(), "second holder must be refused");
        drop(first);
        let again = FileLock::try_exclusive(&p).expect("try").expect("free after drop");
        drop(again);
        assert!(p.exists());
    }

    #[test]
    fn the_lock_file_cannot_be_created_in_a_missing_directory() {
        let d = TempDir::new("fsx-lock-missing");
        assert!(FileLock::exclusive(&d.join("no/such/dir/x.lock")).is_err());
    }
}
