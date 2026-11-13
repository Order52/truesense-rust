//! The things only the program itself can do for the command line: start other programs, run
//! the long-lived services in the foreground and watch for Ctrl-C.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use ts_api::api::Api;
use ts_cli::cli::Launcher;
use ts_foundation::session;
use ts_gui::gui::{ApiCommands, Options, SystemWindow, show_window};
use ts_system::selftest::{Clock, SystemClock};
use ts_system::services::Step;

use crate::{hooks, signals};

/// The launcher of the real program: it holds the application layer the foreground modes use.
pub(crate) struct SystemLauncher {
    api: Arc<Api>,
}

impl SystemLauncher {
    /// A launcher whose window, terminal UI and key service run over `api`.
    pub(crate) fn new(api: Arc<Api>) -> Self {
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
    // Started from a key press, a login script or a service, the process may have been born
    // before the desktop: find the live session (a window without one aborts inside Qt and
    // leaves a core dump behind).
    let session = session::Plan::system(session::Prefer::Process);
    if !session.has_display {
        let _ = writeln!(
            std::io::stderr(),
            "error: no graphical session found (no Wayland or X11 display): run this from your desktop"
        );
        return 1;
    }
    focus_open_window(&session);
    let mut command = Command::new(app);
    session.apply(&mut command);
    if let Some(page) = page {
        command.args(["--page", page]);
    }
    let error = command.exec();
    let _ = writeln!(std::io::stderr(), "error: cannot start {}: {error}", app.display());
    1
}

/// How long a compositor's command-line tool gets to answer before it is given up on.
const COMPOSITOR_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(800);

/// One way of asking a compositor to focus the app's window.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FocusAttempt {
    argv: Vec<String>,
    /// Whether success is the tool printing `ok` (hyprctl always exits 0) and not its exit status.
    says_ok: bool,
}

/// The ways to focus the window of the app whose Wayland app id is `class`, for the compositor the
/// session is running, in the order they are tried. Hyprland 0.55 and later take Lua dispatchers
/// and refuse the older text form; older ones the other way round.
fn focus_attempts(class: &str, hyprland: bool, sway: bool) -> Vec<FocusAttempt> {
    let mut attempts = Vec::new();
    if hyprland {
        let argv = |parts: &[&str]| parts.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>();
        attempts.push(FocusAttempt {
            argv: argv(&[
                "hyprctl",
                "dispatch",
                &format!("hl.dsp.focus({{ window = \"class:^{class}$\" }})"),
            ]),
            says_ok: true,
        });
        attempts.push(FocusAttempt {
            argv: argv(&["hyprctl", "dispatch", "focuswindow", &format!("class:^({class})$")]),
            says_ok: true,
        });
    }
    if sway {
        attempts.push(FocusAttempt {
            argv: vec!["swaymsg".to_owned(), format!("[app_id=\"{class}\"] focus")],
            says_ok: false,
        });
    }
    attempts
}

/// Runs a short command and returns its output, or `None` when it cannot be started, does not
/// finish within `COMPOSITOR_TIMEOUT` (it is killed) or fails.
fn run_briefly(argv: &[String], session: &session::Plan) -> Option<String> {
    use std::io::Read;
    let (program, args) = argv.split_first()?;
    let mut command = Command::new(program);
    session.apply(&mut command);
    let mut child =
        command.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let deadline = std::time::Instant::now() + COMPOSITOR_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = String::new();
                if let Some(mut pipe) = child.stdout.take() {
                    let _ = pipe.read_to_string(&mut out);
                }
                return status.success().then_some(out);
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Asks the compositor to bring the app's window to the front. Wayland does not let a program
/// raise its own window, and not every compositor honours an activation request from one that was
/// started by a key press, so the launcher asks the compositor itself. With no such window open
/// this does nothing, and the window that is about to start gets the focus by being new. Best
/// effort: the answer is not needed.
fn focus_open_window(session: &session::Plan) {
    // What the compositor tools will see: the plan's variables over the process's own.
    let env = |name: &str| {
        session
            .set
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .or_else(|| std::env::var(name).ok())
            .filter(|v| !v.is_empty())
    };
    let class = env("TRUESENSE_APP_ID").unwrap_or_else(|| "truesense".to_owned());
    let present = |name: &str| env(name).is_some();
    for attempt in focus_attempts(&class, present("HYPRLAND_INSTANCE_SIGNATURE"), present("SWAYSOCK")) {
        if let Some(out) = run_briefly(&attempt.argv, session)
            && (!attempt.says_ok || out.trim() == "ok")
        {
            return;
        }
    }
}

/// Starts `truesense gui` detached from this terminal (the terminal UI's "open the window" button).
pub(crate) fn open_window_detached() -> Result<(), String> {
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
    fn hyprland_is_asked_in_the_new_form_first_then_the_old_and_sway_by_its_own_command() {
        let attempts = focus_attempts("truesense", true, false);
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].argv[..2], ["hyprctl", "dispatch"]);
        assert_eq!(attempts[0].argv[2], "hl.dsp.focus({ window = \"class:^truesense$\" })");
        assert_eq!(attempts[1].argv, ["hyprctl", "dispatch", "focuswindow", "class:^(truesense)$"]);
        assert!(attempts.iter().all(|a| a.says_ok), "hyprctl exits 0 whatever it thinks: its words decide");
        let sway = focus_attempts("truesense", false, true);
        assert_eq!(
            sway,
            [FocusAttempt {
                argv: vec!["swaymsg".into(), "[app_id=\"truesense\"] focus".into()],
                says_ok: false
            }]
        );
        assert!(focus_attempts("truesense", false, false).is_empty(), "other desktops: nothing to ask");
        // A test window class is used as given.
        assert!(focus_attempts("tstest", true, false)[0].argv[2].contains("class:^tstest$"));
    }

    #[test]
    fn a_compositor_tool_that_hangs_is_given_up_on_and_one_that_answers_is_heard() {
        let plan = session::Plan::default();
        let started = std::time::Instant::now();
        assert_eq!(run_briefly(&["sleep".into(), "5".into()], &plan), None);
        assert!(started.elapsed() < std::time::Duration::from_secs(3), "it was killed, not waited for");
        assert_eq!(run_briefly(&["echo".into(), "ok".into()], &plan).as_deref().map(str::trim), Some("ok"));
        assert_eq!(run_briefly(&["false".into()], &plan), None);
        assert_eq!(run_briefly(&["/nonexistent/tool".into()], &plan), None);
        assert_eq!(run_briefly(&[], &plan), None);
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
