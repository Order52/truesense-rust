//! `truesense gui`: bring the window that is already open forward, else start one.
//!
//! A second launch (the PREDATOR key, a menu entry, the terminal) must never pile up windows.
//! The order is: ask the open window to show the page and raise it; failing that, ask the running
//! server for a fresh launch address and open that; failing that, start the server (which opens
//! the window and serves it as this user). Two launches racing for the server end with the loser
//! bringing the winner's window forward.
//!
//! Every effect is behind [`WindowEffects`] so the order is tested without a browser or a server.
#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use super::GuiError;
use super::open::{Launcher, Opener};
use super::run::{Options, resume, resume_in, run, with_page};
use super::show::{Raiser, show, show_in};

/// What showing the window does to the machine.
pub trait WindowEffects {
    /// Asks the open window to go to `page`; whether a page heard it.
    fn show(&self, page: &str) -> bool;
    /// Brings the open window to the front; whether a desktop tool found it.
    fn raise(&self) -> bool;
    /// The address of the running window server with a fresh one-time code.
    fn resume(&self) -> Option<String>;
    /// Shows an address as the window.
    fn open(&self, url: &str) -> Result<(), String>;
    /// Starts the window server and keeps it up (see [`run`]).
    fn run(&self, options: Options, stop: &AtomicBool) -> Result<(), GuiError>;
}

/// The real machine: the running server found through the runtime directory, the desktop's tools
/// and a browser.
#[derive(Default)]
pub struct SystemWindow {
    /// The runtime directory to look for a running server in; `None` is the user's.
    pub runtime_dir: Option<PathBuf>,
    /// Raises the window.
    pub raiser: Raiser,
    /// Opens an address as a window.
    pub launcher: Launcher,
}

impl WindowEffects for SystemWindow {
    fn show(&self, page: &str) -> bool {
        match &self.runtime_dir {
            Some(dir) => show_in(dir, page),
            None => show(page),
        }
    }

    fn raise(&self) -> bool {
        self.raiser.raise()
    }

    fn resume(&self) -> Option<String> {
        match &self.runtime_dir {
            Some(dir) => resume_in(dir),
            None => resume(),
        }
    }

    fn open(&self, url: &str) -> Result<(), String> {
        self.launcher.open(url)
    }

    fn run(&self, options: Options, stop: &AtomicBool) -> Result<(), GuiError> {
        run(options, stop)
    }
}

/// Uses the window that is already open: it is told to show the page and brought to the front.
/// When the desktop cannot raise a window, a new one is opened instead (it is at least visible).
fn forward(effects: &dyn WindowEffects, page: &str) -> bool {
    effects.show(page) && effects.raise()
}

/// Opens the running server's page in a new window with a fresh code, when a server runs.
fn reopen(effects: &dyn WindowEffects, page: &str) -> Option<Result<(), String>> {
    effects.resume().map(|url| effects.open(&with_page(&url, page)))
}

/// Shows the window on `page` (empty: home). Returns when the user's window server stops, or at
/// once when an existing window was used. `options` is used only when no server runs yet; its
/// [`Options::page`] is set to `page`.
pub fn show_window(
    effects: &dyn WindowEffects,
    mut options: Options,
    page: &str,
    stop: &AtomicBool,
) -> Result<(), GuiError> {
    if forward(effects, page) {
        return Ok(());
    }
    if let Some(opened) = reopen(effects, page) {
        return opened.map_err(GuiError::Io);
    }
    options.page = Some(page.to_owned()).filter(|p| !p.is_empty());
    match effects.run(options, stop) {
        Err(GuiError::AlreadyRunning) => {
            // Another launch won the race to start the server: use its window.
            if forward(effects, page) {
                return Ok(());
            }
            match reopen(effects, page) {
                Some(opened) => opened.map_err(GuiError::Io),
                None => Err(GuiError::AlreadyRunning),
            }
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use crate::gui::bus::{CommandError, Commands};

    struct NoCommands;

    impl Commands for NoCommands {
        fn run(
            &self,
            area: &str,
            verb: &str,
            _args: &serde_json::Value,
        ) -> Result<serde_json::Value, CommandError> {
            Err(CommandError::unknown(area, verb))
        }

        fn live(&self) -> Option<serde_json::Value> {
            None
        }
    }

    /// A scripted machine that records the order of what was done.
    struct Fake {
        shows: Mutex<Vec<bool>>,
        raises: Mutex<Vec<bool>>,
        resumes: Mutex<Vec<Option<String>>>,
        runs: Mutex<Vec<Result<(), GuiError>>>,
        did: Mutex<Vec<String>>,
        open_fails: bool,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                shows: Mutex::default(),
                raises: Mutex::default(),
                resumes: Mutex::default(),
                runs: Mutex::default(),
                did: Mutex::default(),
                open_fails: false,
            }
        }

        fn next<T: Clone>(queue: &Mutex<Vec<T>>, default: T) -> T {
            let mut q = queue.lock().unwrap_or_else(|e| e.into_inner());
            if q.is_empty() { default } else { q.remove(0) }
        }

        fn note(&self, what: String) {
            if let Ok(mut d) = self.did.lock() {
                d.push(what);
            }
        }

        fn did(&self) -> Vec<String> {
            self.did.lock().map(|d| d.clone()).unwrap_or_default()
        }
    }

    impl WindowEffects for Fake {
        fn show(&self, page: &str) -> bool {
            self.note(format!("show {page}"));
            Self::next(&self.shows, false)
        }

        fn raise(&self) -> bool {
            self.note("raise".to_owned());
            Self::next(&self.raises, false)
        }

        fn resume(&self) -> Option<String> {
            self.note("resume".to_owned());
            Self::next(&self.resumes, None)
        }

        fn open(&self, url: &str) -> Result<(), String> {
            self.note(format!("open {url}"));
            if self.open_fails { Err("no browser".to_owned()) } else { Ok(()) }
        }

        fn run(&self, options: Options, _stop: &AtomicBool) -> Result<(), GuiError> {
            self.note(format!("run page={:?}", options.page));
            Self::next(&self.runs, Ok(()))
        }
    }

    fn options() -> Options {
        Options::new(Arc::new(NoCommands))
    }

    fn go(fake: &Fake, page: &str) -> Result<(), GuiError> {
        show_window(fake, options(), page, &AtomicBool::new(false))
    }

    #[test]
    fn an_open_window_is_told_the_page_and_raised_and_nothing_else_happens() {
        let fake = Fake::new();
        fake.shows.lock().map(|mut s| s.push(true)).ok();
        fake.raises.lock().map(|mut s| s.push(true)).ok();
        assert_eq!(go(&fake, "curves"), Ok(()));
        assert_eq!(fake.did(), ["show curves", "raise"]);
    }

    #[test]
    fn a_window_that_cannot_be_raised_is_replaced_by_a_new_one_on_the_same_server() {
        let fake = Fake::new();
        fake.shows.lock().map(|mut s| s.push(true)).ok();
        fake.resumes.lock().map(|mut s| s.push(Some("http://127.0.0.1:9/#code=abc".to_owned()))).ok();
        assert_eq!(go(&fake, "keys"), Ok(()));
        assert_eq!(
            fake.did(),
            ["show keys", "raise", "resume", "open http://127.0.0.1:9/#code=abc&page=keys"]
        );
    }

    #[test]
    fn a_server_without_a_page_gets_a_new_window_not_a_second_server() {
        let fake = Fake::new();
        fake.resumes.lock().map(|mut s| s.push(Some("http://127.0.0.1:9/#code=abc".to_owned()))).ok();
        assert_eq!(go(&fake, ""), Ok(()));
        assert_eq!(
            fake.did(),
            ["show ", "resume", "open http://127.0.0.1:9/#code=abc"],
            "no page: home, no page member"
        );
    }

    #[test]
    fn with_nothing_running_the_server_starts_on_the_page() {
        let fake = Fake::new();
        assert_eq!(go(&fake, "lighting"), Ok(()));
        assert_eq!(fake.did(), ["show lighting", "resume", "run page=Some(\"lighting\")"]);
        let fake = Fake::new();
        assert_eq!(go(&fake, ""), Ok(()));
        assert_eq!(fake.did().last().map(String::as_str), Some("run page=None"));
    }

    #[test]
    fn losing_the_race_to_start_the_server_means_using_the_winners_window() {
        let fake = Fake::new();
        fake.runs.lock().map(|mut s| s.push(Err(GuiError::AlreadyRunning))).ok();
        fake.shows.lock().map(|mut s| s.extend([false, true])).ok();
        fake.raises.lock().map(|mut s| s.push(true)).ok();
        assert_eq!(go(&fake, "home"), Ok(()));
        assert_eq!(fake.did(), ["show home", "resume", "run page=Some(\"home\")", "show home", "raise"]);

        let fake = Fake::new();
        fake.runs.lock().map(|mut s| s.push(Err(GuiError::AlreadyRunning))).ok();
        fake.resumes.lock().map(|mut s| s.extend([None, Some("http://127.0.0.1:9/#code=z".to_owned())])).ok();
        assert_eq!(go(&fake, "home"), Ok(()));
        assert!(
            fake.did().last().is_some_and(|l| l.starts_with("open http://127.0.0.1:9/#code=z&page=home"))
        );

        let fake = Fake::new();
        fake.runs.lock().map(|mut s| s.push(Err(GuiError::AlreadyRunning))).ok();
        assert_eq!(go(&fake, "home"), Err(GuiError::AlreadyRunning), "nothing could be reached");
    }

    #[test]
    fn other_failures_of_the_server_are_reported_as_they_are() {
        let fake = Fake::new();
        fake.runs.lock().map(|mut s| s.push(Err(GuiError::Listen("busy".to_owned())))).ok();
        assert_eq!(go(&fake, "home"), Err(GuiError::Listen("busy".to_owned())));
        let mut fake = Fake::new();
        fake.open_fails = true;
        fake.resumes.lock().map(|mut s| s.push(Some("http://127.0.0.1:9/#code=a".to_owned()))).ok();
        assert_eq!(go(&fake, "home"), Err(GuiError::Io("no browser".to_owned())));
    }
}
