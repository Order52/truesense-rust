//! gui: the TrueSense window, a small local web server and the page it serves.
//!
//! Qt has no maintained Rust binding that keeps the binary static, and a web page works in any
//! browser, so `truesense gui` starts a server on the loopback interface and shows it as an
//! application window (a Chromium `--app` window, or Firefox in a profile of its own without tabs
//! or address bar). The front end is the Go edition's finished one, embedded unchanged in the
//! binary (`crates/ts-gui/web`); the contract between the page and this code is `COMMANDS.md`.
//!
//! # Layers
//!
//! * [`Commands`] is the seam between the page and the machine. [`ApiCommands`] implements it
//!   over `ts_api::api::Api` only, so every operation and every safety rule is the one the command
//!   line and the terminal UI use.
//! * `server` decides who may ask and what each request means (Host and Origin checks, session
//!   token, one-time launch code, strict CSP, the command bus, the live stream); it never touches
//!   a socket. `http` feeds it from `tiny_http`.
//! * [`run`] starts everything, shows the window through an [`Opener`] and stops after the last
//!   page closed. [`resume`], [`show`] and [`Raiser`] let a second `truesense gui` forward to the
//!   window that is already open instead of starting another.
//!
//! # What the binary has to provide
//!
//! * `Arc<ts_api::api::Api>` built from the real `Services` (`ApiCommands::new`).
//! * [`Hooks`]: the self-test ([`SelfTest`]), the daemon journal ([`ServiceLogs`]) and the start-up
//!   settings ([`StartupControl`]), which need pieces only the assembly layer builds. A missing hook
//!   makes its command say so.
//! * A stop flag for [`run`] (set it from the signal handler).
#![forbid(unsafe_code)]

mod assets;
mod bus;
mod client;
mod http;
mod hub;
mod instance;
mod open;
mod run;
mod server;
mod show;
mod token;
mod window;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod testutil;

use std::fmt;
use std::path::Path;

pub use bus::{ApiCommands, CommandError, Commands, Hooks, SelfTest, ServiceLogs, StartupControl};
pub use instance::{default_runtime_dir, runtime_dir_with};
pub use open::{
    CHROMIUM_FAMILY, FIREFOX_PROFILE, FIREFOX_USER_CHROME, FIREFOX_USER_JS, Launcher, LauncherHost, Opener,
    SystemHost, firefox_args, write_firefox_profile,
};
pub use run::{
    DEFAULT_COMMAND_TIMEOUT, DEFAULT_IDLE_TIMEOUT, DEFAULT_PING, DEFAULT_TICK, Options, resume, resume_in,
    run, with_page,
};
pub use server::{Clock, Logger};
pub use show::{DesktopTools, Raiser, SystemDesktop, WINDOW_CLASS, raise, show, show_in};
pub use token::{Entropy, system_entropy};
pub use window::{SystemWindow, WindowEffects, show_window};

/// Why the window server could not start or stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuiError {
    /// This user already has a window server: use [`resume`] and open its address.
    AlreadyRunning,
    /// The random source failed; no token is made from a weak source.
    Entropy(String),
    /// The directory for the single-instance lock is not usable.
    RuntimeDir {
        /// The directory.
        dir: String,
        /// Why not.
        why: String,
    },
    /// The listen address is not usable (or not on the loopback interface).
    Listen(String),
    /// A file or thread could not be made.
    Io(String),
}

impl GuiError {
    pub(crate) fn runtime_dir(dir: &Path, e: &std::io::Error) -> Self {
        Self::RuntimeDir { dir: dir.display().to_string(), why: e.to_string() }
    }
}

impl fmt::Display for GuiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRunning => f.write_str("the TrueSense window is already running"),
            Self::Entropy(e) => write!(f, "gui: no random numbers for the window's token: {e}"),
            Self::RuntimeDir { dir, why } => write!(f, "gui: runtime directory {dir}: {why}"),
            Self::Listen(e) => write!(f, "gui: {e}"),
            Self::Io(e) => write!(f, "gui: {e}"),
        }
    }
}

impl std::error::Error for GuiError {}
