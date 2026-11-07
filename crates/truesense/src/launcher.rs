//! The things only the program itself can do for the command line: start other programs, run
//! the long-lived services in the foreground and watch for Ctrl-C.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ts_api::api::Api;
use ts_cli::cli::Launcher;
use ts_gui::gui::{ApiCommands, Options, SystemWindow, show_window};
use ts_system::selftest::{Clock, SystemClock};
use ts_system::services::Step;

use crate::{hooks, signals};

/// The launcher of the real program: it holds the application layer the foreground modes use.
pub struct SystemLauncher {
    api: Arc<Api>,
}

impl SystemLauncher {
    /// A launcher whose window, terminal UI and key service run over `api`.
    pub fn new(api: Arc<Api>) -> Self {
        Self { api }
    }
}

/// Runs `program` with the terminal as its output; only a failure to start it is an error (like
/// the Python edition, which ignored the exit status of `systemctl status`).
fn run_visible(program: &str, args: &[&str]) -> Result<(), String> {
    Command::new(program).args(args).status().map(|_| ()).map_err(|e| format!("cannot run {program}: {e}"))
}

/// The path of this program, for the commands that start another copy of it.
fn current_exe() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("truesense"))
}

/// The program name of the native (Qt) window.
const NATIVE_APP: &str = "truesense-qt";

/// Where the native window is installed when it is not beside this program.
const NATIVE_APP_INSTALLED: [&str; 2] = ["/usr/local/bin/truesense-qt", "/usr/bin/truesense-qt"];

/// The first existing candidate for the native window: next to this program first (a build tree or
/// an unpacked release), then the usual install places. `is_file` is injected so the search is
/// tested without touching the real file system.
fn find_native_app(
    beside: Option<&std::path::Path>,
    installed: &[&str],
    is_file: impl Fn(&std::path::Path) -> bool,
) -> Option<PathBuf> {
    beside
        .map(|dir| dir.join(NATIVE_APP))
        .into_iter()
        .chain(installed.iter().map(PathBuf::from))
        .find(|p| is_file(p))
}

/// The native window's program, found next to this one or in the usual install places.
fn native_app() -> Option<PathBuf> {
    let exe = current_exe();
    find_native_app(exe.parent(), &NATIVE_APP_INSTALLED, |p| p.is_file())
}

/// Replaces this process with the native window (a second launch then just forwards the page to
/// the open window and exits, which is the app's own single-instance logic).
fn exec_native_app(app: &std::path::Path, page: Option<&str>) -> i32 {
    use std::os::unix::process::CommandExt;
    let mut command = Command::new(app);
    if let Some(page) = page {
        command.args(["--page", page]);
    }
    let error = command.exec();
    let _ = writeln!(std::io::stderr(), "error: cannot start {}: {error}", app.display());
    1
}

/// Starts `truesense gui` detached from this terminal (the terminal UI's "open the window" button).
pub fn open_window_detached() -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let mut child = Command::new(current_exe())
        .arg("gui")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("cannot start the window: {e}"))?;
    // The window outlives this process; a thread reaps it if it ends first.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

impl Launcher for SystemLauncher {
    fn service_status(&self) -> Result<(), String> {
        run_visible("systemctl", &["--no-pager", "status", "truesense.service"])
    }

    fn service_restart(&self) -> Result<(), String> {
        run_visible("sudo", &["systemctl", "restart", "truesense.service"])
    }

    fn service_logs(&self) -> Result<(), String> {
        run_visible("journalctl", &["-u", "truesense.service", "-b", "--no-pager", "-n", "60"])
    }

    fn repair_system_with_sudo(&self) -> Option<Vec<Step>> {
        let out = Command::new("sudo")
            .arg(current_exe())
            .args(["service", "repair", "--system", "--json"])
            .stdin(Stdio::inherit())
            .stderr(Stdio::inherit())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let last = text.lines().rev().find(|l| l.trim_start().starts_with('['))?;
        serde_json::from_str(last).ok()
    }

    fn startup_apply(&self) -> Result<Vec<String>, String> {
        ts_keys::startup::apply(&self.api).map_err(|e| e.to_string())
    }

    fn gui(&self, page: Option<&str>) -> i32 {
        // The native window is the app; the web window is only the fallback for a machine where
        // the app is not installed (no graphics libraries, a headless box, a bare `cargo build`).
        if let Some(app) = native_app() {
            return exec_native_app(&app, page);
        }
        let stop = signals::interrupt_flag();
        let commands = Arc::new(ApiCommands::new(self.api.clone(), hooks::window_hooks(&self.api)));
        let mut options = Options::new(commands);
        options.log = Arc::new(|line: &str| {
            let _ = writeln!(std::io::stderr(), "{line}");
        });
        match show_window(&SystemWindow::default(), options, page.unwrap_or("home"), &stop) {
            Ok(()) => 0,
            Err(e) => {
                let _ = writeln!(std::io::stderr(), "error: {e}");
                1
            }
        }
    }

    fn tui(&self, page: Option<&str>) -> i32 {
        let options = ts_tui::tui::Options {
            page: page.map(str::to_owned),
            window: Some(Arc::new(open_window_detached)),
            ..ts_tui::tui::Options::default()
        };
        match ts_tui::tui::run(self.api.clone(), options) {
            Ok(()) => 0,
            Err(e) => {
                let _ = writeln!(std::io::stderr(), "{e}");
                1
            }
        }
    }

    fn daemon(&self) -> i32 {
        crate::app::run_daemon()
    }

    fn key_service(&self) -> i32 {
        let stop = signals::interrupt_flag();
        ts_keys::fnkeys::run(&self.api, stop);
        0
    }

    fn interrupt(&self) -> Arc<AtomicBool> {
        signals::interrupt_flag()
    }

    fn clock(&self) -> Arc<dyn Clock> {
        Arc::new(SystemClock::new())
    }
}

/// Whether a stop was requested (kept for symmetry with the services that poll it).
#[allow(dead_code)]
pub fn stop_requested(flag: &AtomicBool) -> bool {
    flag.load(Ordering::SeqCst)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn the_window_program_is_truesense_qt_not_the_old_name() {
        assert_eq!(NATIVE_APP, "truesense-qt");
        assert!(NATIVE_APP_INSTALLED.iter().all(|p| p.ends_with("/truesense-qt")));
    }

    #[test]
    fn the_program_beside_this_one_wins_over_an_installed_one() {
        let found = find_native_app(Some(Path::new("/opt/ts")), &NATIVE_APP_INSTALLED, |_| true);
        assert_eq!(found, Some(PathBuf::from("/opt/ts/truesense-qt")));
    }

    #[test]
    fn installed_places_are_tried_in_order() {
        let only = |want: &'static str| move |p: &Path| p == Path::new(want);
        assert_eq!(
            find_native_app(Some(Path::new("/opt/ts")), &NATIVE_APP_INSTALLED, only("/usr/bin/truesense-qt")),
            Some(PathBuf::from("/usr/bin/truesense-qt"))
        );
        assert_eq!(
            find_native_app(None, &NATIVE_APP_INSTALLED, |p| p.starts_with("/usr")),
            Some(PathBuf::from("/usr/local/bin/truesense-qt"))
        );
    }

    #[test]
    fn the_old_program_name_is_never_looked_for_and_nothing_found_means_the_web_window() {
        let seen = std::cell::RefCell::new(Vec::new());
        let found = find_native_app(Some(Path::new("/opt/ts")), &NATIVE_APP_INSTALLED, |p| {
            seen.borrow_mut().push(p.to_path_buf());
            false
        });
        assert_eq!(found, None);
        assert_eq!(seen.borrow().len(), 3);
        assert!(seen.borrow().iter().all(|p| !p.to_string_lossy().contains("truesense-app")));
    }
}
