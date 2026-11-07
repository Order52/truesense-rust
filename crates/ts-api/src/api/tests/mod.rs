//! Tests of the application layer: every operation against the firmware simulator and a scripted
//! daemon, the daemon-down fallbacks, and the refusal rules.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod alerts;
mod audio;
mod battery;
mod bindings;
mod cpu_gpu;
mod errors;
mod fan;
mod keyboard;
mod keys;
mod power;
mod profile;
mod rear;
mod smart;
mod status;
mod system;
mod themes;
mod update;
mod window;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use super::testing::{Rig, daemon_state};
use super::{Api, Services};

/// A fresh directory under the system temp dir, removed when dropped.
pub(super) struct TempDir(pub PathBuf);

impl TempDir {
    fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ts-api-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An application layer over simulated services, with its doubles.
pub(super) struct Fx {
    pub api: Api,
    pub rig: Rig,
    pub dir: TempDir,
}

impl Fx {
    /// A running daemon.
    pub(super) fn new() -> Self {
        Self::build(|s| s)
    }

    /// No daemon: its socket does not exist.
    pub(super) fn offline() -> Self {
        let fx = Self::new();
        fx.rig.daemon.set_available(false);
        fx
    }

    /// Services adjusted by `adjust` before the layer is built.
    pub(super) fn build(adjust: impl FnOnce(Services) -> Services) -> Self {
        let dir = TempDir::new();
        let (services, rig) = Services::simulated(&dir.0).expect("simulated services");
        Self { api: Api::new(adjust(services)), rig, dir }
    }

    /// The text of the user's configuration file.
    pub(super) fn user_config(&self) -> String {
        std::fs::read_to_string(self.dir.0.join("home/config.toml")).unwrap_or_default()
    }
}
