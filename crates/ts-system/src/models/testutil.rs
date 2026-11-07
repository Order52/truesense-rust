//! Test helpers shared by the modules of this crate.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A unique directory below the system temp directory, removed when dropped.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn new(label: &str) -> Self {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("ts-system-{label}-{}-{n}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    /// Writes `content` to `rel` below the directory, creating the parents.
    pub(crate) fn write(&self, rel: &str, content: &str) {
        let path = self.0.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    pub(crate) fn mkdir(&self, rel: &str) {
        fs::create_dir_all(self.0.join(rel)).unwrap();
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
