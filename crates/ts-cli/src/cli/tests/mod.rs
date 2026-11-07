//! Tests of the command line: the argument parser against the reference help pages, every command
//! against the simulated services, and the generated completions.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod alerts;
mod commands;
mod golden;
mod help;
mod modebind;
mod parse;

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use ts_api::api::testing::{FakeDaemon, Rig};
use ts_api::api::{Api, Services};
use ts_system::selftest::Clock;
use ts_system::services::Step;

use super::{Io, Launcher, run};

/// A fresh directory under the system temp dir, removed when dropped.
pub(super) struct TempDir(pub(super) PathBuf);

impl TempDir {
    pub(super) fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ts-cli-{}-{}",
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

/// A launcher that records what it was asked to start.
#[derive(Default)]
pub(super) struct FakeLauncher {
    pub(super) calls: Mutex<Vec<String>>,
    pub(super) sudo_steps: Mutex<Option<Vec<Step>>>,
    pub(super) interrupt: Arc<AtomicBool>,
}

impl FakeLauncher {
    fn record(&self, what: &str) {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).push(what.to_owned());
    }

    pub(super) fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// A clock that never waits.
struct NoWait;

impl Clock for NoWait {
    fn now(&self) -> Duration {
        Duration::ZERO
    }

    fn sleep(&self, _duration: Duration) {}
}

impl Launcher for FakeLauncher {
    fn service_status(&self) -> Result<(), String> {
        self.record("service status");
        Ok(())
    }

    fn service_restart(&self) -> Result<(), String> {
        self.record("service restart");
        Ok(())
    }

    fn service_logs(&self) -> Result<(), String> {
        self.record("service logs");
        Ok(())
    }

    fn repair_system_with_sudo(&self) -> Option<Vec<Step>> {
        self.record("repair system");
        self.sudo_steps.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn startup_apply(&self) -> Result<Vec<String>, String> {
        self.record("startup apply");
        Ok(vec!["profile: applied".to_owned()])
    }

    fn gui(&self, page: Option<&str>) -> i32 {
        self.record(&format!("gui {}", page.unwrap_or("-")));
        0
    }

    fn tui(&self, page: Option<&str>) -> i32 {
        self.record(&format!("tui {}", page.unwrap_or("-")));
        0
    }

    fn daemon(&self) -> i32 {
        self.record("daemon");
        0
    }

    fn key_service(&self) -> i32 {
        self.record("keys run");
        0
    }

    fn interrupt(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.interrupt)
    }

    fn clock(&self) -> Arc<dyn Clock> {
        Arc::new(NoWait)
    }
}

/// What a command printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Ran {
    pub(super) code: i32,
    pub(super) out: String,
    pub(super) err: String,
}

/// The command line over simulated services, with the doubles to steer and inspect.
pub(super) struct Fx {
    pub(super) api: Api,
    pub(super) rig: Rig,
    pub(super) launcher: FakeLauncher,
    pub(super) dir: TempDir,
}

impl Fx {
    /// A running fake daemon that knows no command.
    pub(super) fn new() -> Self {
        Self::build(|s| s)
    }

    /// No daemon: the socket does not exist (what the Python reference runs saw).
    pub(super) fn offline() -> Self {
        let fx = Self::new();
        fx.rig.daemon.set_available(false);
        fx
    }

    pub(super) fn build(adjust: impl FnOnce(Services) -> Services) -> Self {
        let dir = TempDir::new();
        let (services, rig) = Services::simulated(&dir.0).expect("simulated services");
        Self { api: Api::new(adjust(services)), rig, launcher: FakeLauncher::default(), dir }
    }

    pub(super) fn daemon(&self) -> &Arc<FakeDaemon> {
        &self.rig.daemon
    }

    /// Runs a command line (words separated by spaces), answering `input` to a question.
    pub(super) fn run_with(&self, line: &str, input: &str) -> Ran {
        let args: Vec<String> = line.split_whitespace().map(str::to_owned).collect();
        self.run_args(&args, input)
    }

    /// Runs a command line given as separate words.
    pub(super) fn run_args(&self, args: &[String], input: &str) -> Ran {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut stdin = Cursor::new(input.as_bytes().to_vec());
        let code = {
            let mut io = Io::new(&mut out, &mut err, &mut stdin);
            run(args, &self.api, &self.launcher, &mut io)
        };
        Ran {
            code,
            out: String::from_utf8_lossy(&out).into_owned(),
            err: String::from_utf8_lossy(&err).into_owned(),
        }
    }

    pub(super) fn run(&self, line: &str) -> Ran {
        self.run_with(line, "")
    }

    /// The text of the user's configuration file.
    pub(super) fn user_config(&self) -> String {
        std::fs::read_to_string(self.dir.0.join("home/config.toml")).unwrap_or_default()
    }
}
