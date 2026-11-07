//! Bounded readers for the kernel's text files, shared by the monitor modules.
//!
//! Everything under `/proc` and `/sys` is untrusted input as far as a parser is concerned: a file
//! can be missing, unreadable, changing while it is read, or (in a test, or on a broken kernel
//! module) arbitrarily large and not UTF-8. Every reader here is bounded and total: it returns
//! `None` instead of failing, because a monitor page must keep showing the other readings when one
//! file is gone.
#![forbid(unsafe_code)]

use std::cmp::Ordering;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

/// The largest text file read whole (`/proc/cpuinfo` is about 40 KB on a 22-thread laptop).
pub(crate) const MAX_TEXT: u64 = 4 * 1024 * 1024;

/// The path of `absolute` (`/proc/stat`) below `root` (`/` on a real machine, a temporary tree in
/// tests), so no module ever reads outside the tree it was given.
pub(crate) fn below(root: &Path, absolute: &str) -> PathBuf {
    root.join(absolute.trim_start_matches('/'))
}

/// At most `limit` bytes of a file, decoded leniently (a process name in `/proc` is not always
/// UTF-8). `None` when the file cannot be opened or read.
pub(crate) fn read_prefix(path: &Path, limit: u64) -> Option<String> {
    let mut bytes = Vec::new();
    File::open(path).ok()?.take(limit).read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// A whole text file up to [`MAX_TEXT`] bytes; longer files are cut, not refused, because the
/// parsers skip a truncated last line anyway.
pub(crate) fn read_text(path: &Path) -> Option<String> {
    read_prefix(path, MAX_TEXT)
}

/// The first line of a small file, trimmed (the sysfs convention for single values).
pub(crate) fn read_line(path: &Path) -> Option<String> {
    let text = read_prefix(path, 4096)?;
    Some(text.lines().next().unwrap_or("").trim().to_owned())
}

/// A file holding one unsigned integer.
pub(crate) fn read_u64(path: &Path) -> Option<u64> {
    read_line(path)?.parse().ok()
}

/// A file holding one signed integer.
pub(crate) fn read_i64(path: &Path) -> Option<i64> {
    read_line(path)?.parse().ok()
}

/// Splits a name into its non-numeric stem and trailing number: `hwmon10` is `("hwmon", 10)`.
fn natural_key(name: &str) -> (&str, Option<u64>) {
    let stem = name.trim_end_matches(|c: char| c.is_ascii_digit());
    (stem, name[stem.len()..].parse().ok())
}

/// Orders names the way a person counts: `cpu2` before `cpu10`.
pub(crate) fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (sa, na) = natural_key(a);
    let (sb, nb) = natural_key(b);
    sa.cmp(sb).then(na.cmp(&nb)).then_with(|| a.cmp(b))
}

/// The entry names of a directory in natural order; empty when it cannot be read. Names that are
/// not UTF-8 are skipped (no kernel name is).
pub(crate) fn dir_names(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> =
        entries.filter_map(Result::ok).filter_map(|e| e.file_name().into_string().ok()).collect();
    names.sort_by(|a, b| natural_cmp(a, b));
    names
}

/// `cpu12` as `12`: the number of an entry with the given stem, `None` for anything else.
pub(crate) fn indexed(name: &str, stem: &str) -> Option<usize> {
    let digits = name.strip_prefix(stem)?;
    if digits.is_empty() || digits.len() > 9 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod testutil {
    //! A temporary tree for the monitor modules' tests.
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A directory under the system temp dir, removed when dropped.
    pub(crate) struct Tree(PathBuf);

    impl Tree {
        pub(crate) fn new() -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "ts-system-monitor-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        pub(crate) fn root(&self) -> &Path {
            &self.0
        }

        /// Writes a file (and its directories) below the root.
        pub(crate) fn put(&self, rel: &str, text: &str) {
            self.put_bytes(rel, text.as_bytes());
        }

        pub(crate) fn put_bytes(&self, rel: &str, bytes: &[u8]) {
            let path = self.0.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }

        pub(crate) fn remove(&self, rel: &str) {
            let path = self.0.join(rel);
            if path.is_dir() {
                fs::remove_dir_all(path).unwrap();
            } else {
                fs::remove_file(path).unwrap();
            }
        }

        pub(crate) fn symlink(&self, rel: &str, target: &str) {
            let path = self.0.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(target, path).unwrap();
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::testutil::Tree;
    use super::*;

    #[test]
    fn natural_order_counts_like_a_person() {
        let mut names = vec!["cpu10", "cpu2", "cpu1", "cpufreq", "cpu", "hwmon9", "hwmon10"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, ["cpu", "cpu1", "cpu2", "cpu10", "cpufreq", "hwmon9", "hwmon10"]);
    }

    #[test]
    fn indexed_accepts_only_stem_and_digits() {
        assert_eq!(indexed("cpu12", "cpu"), Some(12));
        assert_eq!(indexed("cpu", "cpu"), None);
        assert_eq!(indexed("cpufreq", "cpu"), None);
        assert_eq!(indexed("cpu1x", "cpu"), None);
        assert_eq!(
            indexed("cpu99999999999999999999", "cpu"),
            None,
            "a number that does not fit is not a core"
        );
        assert_eq!(indexed("hwmon3", "cpu"), None);
    }

    #[test]
    fn readers_are_total_and_bounded() {
        let t = Tree::new();
        t.put("a", "  42 \nsecond");
        t.put("big", &"x".repeat(100));
        t.put_bytes("bin", &[0xff, 0xfe, b'o', b'k']);
        assert_eq!(read_line(&t.root().join("a")).as_deref(), Some("42"));
        assert_eq!(read_u64(&t.root().join("a")), Some(42));
        assert_eq!(read_i64(&t.root().join("big")), None);
        assert_eq!(read_prefix(&t.root().join("big"), 10).map(|s| s.len()), Some(10));
        assert!(read_text(&t.root().join("bin")).unwrap().ends_with("ok"), "lossy, not an error");
        assert_eq!(read_text(&t.root().join("missing")), None);
        assert!(dir_names(&t.root().join("missing")).is_empty());
    }
}
