//! The commands that start something outside this library: a service manager, a window, the
//! terminal UI, the daemon.
//!
//! The command line parses and prints; everything that executes another program, takes over the
//! terminal or runs for as long as the machine is up goes through this trait, so the library
//! never spawns a process itself and the tests replace it with a recorder.
#![forbid(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use ts_system::selftest::Clock;
use ts_system::services::Step;

/// What only the program that embeds the command line can do.
pub trait Launcher {
    /// `systemctl --no-pager status truesense.service`, run with the terminal as its output.
    ///
    /// # Errors
    /// The text to show after `error:` when the program could not be started.
    fn service_status(&self) -> Result<(), String>;

    /// `sudo systemctl restart truesense.service`.
    ///
    /// # Errors
    /// The text to show after `error:` when the program could not be started.
    fn service_restart(&self) -> Result<(), String>;

    /// `journalctl -u truesense.service -b --no-pager -n 60`.
    ///
    /// # Errors
    /// The text to show after `error:` when the program could not be started.
    fn service_logs(&self) -> Result<(), String>;

    /// The part of `service repair` that needs root, run through sudo as
    /// `truesense service repair --system`; the steps it answered, or `None` when sudo was
    /// refused or the answer could not be read.
    fn repair_system_with_sudo(&self) -> Option<Vec<Step>>;

    /// `truesense startup apply`: what the lights and the profile do at login. It lives in the
    /// key-service crate, which this library does not depend on. The lines to print, or the text
    /// of the error.
    ///
    /// # Errors
    /// The text to show after `error:`.
    fn startup_apply(&self) -> Result<Vec<String>, String>;

    /// Runs the small window, opened on `page` (one of [`GUI_PAGES`](super::GUI_PAGES)); the exit code.
    fn gui(&self, page: Option<&str>) -> i32;

    /// Runs the terminal UI, opened on `page` (one of [`TUI_PAGES`](super::TUI_PAGES)); the exit code.
    fn tui(&self, page: Option<&str>) -> i32;

    /// Runs the daemon in the foreground (what systemd does); the exit code.
    fn daemon(&self) -> i32;

    /// Runs the key service in the foreground (`keys run`); the exit code.
    fn key_service(&self) -> i32;

    /// A flag the process sets on SIGINT, so that `audio voice watch` (which runs until it is
    /// interrupted) can end. A launcher without signal handling returns a flag that is never set.
    fn interrupt(&self) -> Arc<AtomicBool>;

    /// The clock the self-test waits on (22 seconds of fan watch on a real one).
    fn clock(&self) -> Arc<dyn Clock>;
}
