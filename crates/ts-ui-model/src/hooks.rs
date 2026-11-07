//! What the command bus needs from the program around it: the self-test, the daemon's journal and
//! the login settings (the three [`Hooks`] of `ts_gui::gui`).
//!
//! Adapted from `crates/truesense/src/hooks.rs` (the same three adapters the web window gets), so
//! the Qt app and `truesense gui` run exactly the same code behind `selftest run`, `service logs`
//! and `startup set`.
#![forbid(unsafe_code)]

use std::process::Command;
use std::sync::Arc;

use ts_api::adapt::selftest::{SelftestAudio, SelftestDaemon, SelftestKeyboard};
use ts_api::api::Api;
use ts_gui::gui::{Hooks, SelfTest, ServiceLogs, StartupControl};
use ts_system::selftest::{self, Deps, Options, SystemClock};

/// The self-test run for the window: the same adapters and clock as `truesense selftest`.
struct WindowSelfTest(Arc<Api>);

impl SelfTest for WindowSelfTest {
    fn run(&self, skip_fans: bool) -> (bool, String) {
        let api = &*self.0;
        let clock = SystemClock::new();
        let (daemon, keyboard, audio) = (SelftestDaemon(api), SelftestKeyboard(api), SelftestAudio(api));
        let mut deps = Deps::new(&clock);
        deps.daemon = api.daemon_available().then_some(&daemon as &dyn selftest::Daemon);
        deps.keyboard = Some(&keyboard);
        deps.audio = Some(&audio);
        let mut out = Vec::new();
        let report = selftest::run(&api.services().hw, &deps, Options { skip_fans }, &mut out);
        (report.ok(), String::from_utf8_lossy(&out).into_owned())
    }
}

/// The last 60 lines of the daemon's journal.
struct Journal;

impl ServiceLogs for Journal {
    fn text(&self) -> Result<String, String> {
        let out = Command::new("journalctl")
            .args(["-u", "truesense.service", "-b", "--no-pager", "-n", "60"])
            .output()
            .map_err(|e| format!("cannot run journalctl: {e}"))?;
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// The login settings, through the key-service crate that owns them.
struct Login(Arc<Api>);

impl StartupControl for Login {
    fn set(&self, target: &str, value: &str) -> Result<String, String> {
        ts_keys::startup::set_option(&self.0, target, value).map_err(|e| e.to_string())
    }

    fn apply(&self) -> Result<Vec<String>, String> {
        ts_keys::startup::apply(&self.0).map_err(|e| e.to_string())
    }
}

/// The hooks of the window's command bus over `api`.
pub fn window_hooks(api: &Arc<Api>) -> Hooks {
    Hooks {
        self_test: Some(Arc::new(WindowSelfTest(api.clone()))),
        service_logs: Some(Arc::new(Journal)),
        startup: Some(Arc::new(Login(api.clone()))),
    }
}
