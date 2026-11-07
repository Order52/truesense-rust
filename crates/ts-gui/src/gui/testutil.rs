//! Helpers shared by the tests of this module.
#![allow(clippy::expect_used)]

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

/// A fresh temporary directory, removed when dropped.
pub(crate) struct Temp(pub PathBuf);

impl Temp {
    /// Makes `<tmp>/ts-gui-<tag>-<pid>-<n>`.
    pub(crate) fn new(tag: &str) -> Temp {
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!(
            "ts-gui-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).expect("create temp dir");
        Temp(p)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
