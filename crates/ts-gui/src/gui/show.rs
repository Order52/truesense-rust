//! A second `truesense gui` shows the window that is already open.
//!
//! [`show`] asks the running window server to take its page to a given page (the page hears a
//! `show` event on its stream); when a page heard it, no second window is needed and [`Raiser`]
//! brings the existing one forward. Desktops differ and there is no portable way to raise a
//! window, so the raiser tries what the running desktop has (Hyprland, Sway, an X11 window
//! manager) and says whether one of them found the window.
//!
//! Every external effect (the environment, the programs on `PATH`, running them) is behind
//! [`DesktopTools`] so the tests start nothing.
#![forbid(unsafe_code)]

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::client;
use super::instance::{LockInfo, default_runtime_dir, read_live};
use std::path::Path;

/// The window class (Wayland app id) of the TrueSense window: Firefox is started with it by name,
/// Chromium-family browsers with `--class`. A desktop can group the window under the TrueSense
/// icon and apply window rules to it, and [`Raiser`] finds it by it.
pub const WINDOW_CLASS: &str = "truesense";

/// How long a desktop tool may take before it is given up on.
const TOOL_TIMEOUT: Duration = Duration::from_secs(3);

/// How long the running server may take to answer.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(3);

/// Asks the window this user already has open to go to `page`, and reports whether a page heard
/// it. When it did, bring the existing window forward ([`Raiser::raise`]).
pub fn show(page: &str) -> bool {
    default_runtime_dir().is_ok_and(|dir| show_in(&dir, page))
}

/// [`show`] for a given runtime directory.
pub fn show_in(runtime_dir: &Path, page: &str) -> bool {
    let Some(info) = read_live(runtime_dir) else { return false };
    asked(&info, page)
}

fn asked(info: &LockInfo, page: &str) -> bool {
    let body = serde_json::json!({ "page": page }).to_string();
    let Ok(answer) = client::post_json(info.port, "/api/gui/show", Some(&info.token), &body, ANSWER_TIMEOUT)
    else {
        return false;
    };
    answer.status == 200
        && answer.json().is_some_and(|v| {
            v.get("ok") == Some(&Value::Bool(true))
                && v.get("result").and_then(|r| r.get("shown")) == Some(&Value::Bool(true))
        })
}

/// What the raiser needs from the desktop.
pub trait DesktopTools: Send + Sync {
    /// A variable of the environment.
    fn env(&self, key: &str) -> Option<String>;
    /// Whether a program is on `PATH`.
    fn have(&self, program: &str) -> bool;
    /// Runs a program and answers its standard output, or an error when it could not run or
    /// exited with a failure.
    fn run(&self, program: &str, args: &[&str]) -> Result<String, String>;
}

/// The real desktop: the process environment, `PATH` and processes.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemDesktop;

impl DesktopTools for SystemDesktop {
    fn env(&self, key: &str) -> Option<String> {
        ts_foundation::fsx::env_nonempty(key)
    }

    fn have(&self, program: &str) -> bool {
        which(program, self.env("PATH").as_deref().unwrap_or("")).is_some()
    }

    fn run(&self, program: &str, args: &[&str]) -> Result<String, String> {
        run_with_timeout(program, args, TOOL_TIMEOUT)
    }
}

/// Finds a program in a `PATH` string (the first executable file).
pub(crate) fn which(program: &str, path: &str) -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    path.split(':')
        .filter(|d| !d.is_empty())
        .map(|d| Path::new(d).join(program))
        .find(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
}

/// Runs a program, waits at most `timeout`, and answers its standard output. A program that does
/// not finish in time is killed, so a stuck compositor tool cannot hold up the launch.
fn run_with_timeout(program: &str, args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    let reader = child.stdout.take().map(|mut out| {
        thread::spawn(move || {
            let mut text = String::new();
            let _ = out.by_ref().take(1 << 20).read_to_string(&mut text);
            text
        })
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!("{program}: did not finish in {} s", timeout.as_secs()));
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(e) => break Err(format!("{program}: {e}")),
        }
    };
    let text = reader.and_then(|r| r.join().ok()).unwrap_or_default();
    match status {
        Ok(s) if s.success() => Ok(text),
        Ok(s) => Err(format!("{program}: {s}")),
        Err(e) => Err(e),
    }
}

/// Brings the TrueSense window to the front.
#[derive(Clone)]
pub struct Raiser {
    tools: Arc<dyn DesktopTools>,
}

impl Default for Raiser {
    fn default() -> Self {
        Self::new(Arc::new(SystemDesktop))
    }
}

impl Raiser {
    /// A raiser over these desktop tools.
    pub fn new(tools: Arc<dyn DesktopTools>) -> Self {
        Self { tools }
    }

    /// Whether a desktop tool found a TrueSense window and focused it.
    pub fn raise(&self) -> bool {
        let t = &*self.tools;
        let set = |key: &str| t.env(key).is_some();
        if set("HYPRLAND_INSTANCE_SIGNATURE") && t.have("hyprctl") {
            return self.raise_hyprland();
        }
        if set("SWAYSOCK") && t.have("swaymsg") {
            let command = format!("[app_id=\"{WINDOW_CLASS}\"] focus");
            return t.run("swaymsg", &[&command]).is_ok_and(|out| out.contains("\"success\": true"));
        }
        if set("DISPLAY") && t.have("wmctrl") {
            return t.run("wmctrl", &["-x", "-a", WINDOW_CLASS]).is_ok();
        }
        if set("DISPLAY") && t.have("xdotool") {
            let Ok(out) = t.run("xdotool", &["search", "--class", WINDOW_CLASS]) else { return false };
            let Some(id) = out.split_whitespace().next() else { return false };
            return t.run("xdotool", &["windowactivate", id]).is_ok();
        }
        false
    }

    fn raise_hyprland(&self) -> bool {
        let t = &*self.tools;
        let Ok(clients) = t.run("hyprctl", &["clients", "-j"]) else { return false };
        if !has_window(&clients) {
            return false;
        }
        // Hyprland 0.5x and newer take a Lua dispatcher; older ones the classic "focuswindow class:..." form.
        let lua = format!("hl.dsp.focus({{ window = \"class:^{WINDOW_CLASS}$\" }})");
        if t.run("hyprctl", &["dispatch", &lua]).is_ok_and(|out| dispatched(&out)) {
            return true;
        }
        let class = format!("class:^{WINDOW_CLASS}$");
        t.run("hyprctl", &["dispatch", "focuswindow", &class]).is_ok_and(|out| dispatched(&out))
    }
}

/// Whether `hyprctl clients -j` lists a window of our class.
fn has_window(clients_json: &str) -> bool {
    match serde_json::from_str::<Value>(clients_json) {
        Ok(Value::Array(list)) => {
            list.iter().any(|c| c.get("class").and_then(Value::as_str) == Some(WINDOW_CLASS))
        }
        _ => false,
    }
}

/// `hyprctl dispatch` exits 0 even for a dispatcher it does not know and says so in its output,
/// which is how the two forms are told apart.
fn dispatched(output: &str) -> bool {
    let out = output.to_ascii_lowercase();
    !(out.contains("invalid") || out.contains("error") || out.contains("unknown"))
}

/// Brings the window forward with the real desktop tools.
pub fn raise() -> bool {
    Raiser::default().raise()
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Mutex;

    /// A desktop with chosen variables, programs and answers; records what was run.
    #[derive(Default)]
    pub(crate) struct FakeDesktop {
        pub env: BTreeSet<&'static str>,
        pub programs: BTreeSet<&'static str>,
        /// Answers by the full command line; anything else fails.
        pub answers: BTreeMap<String, String>,
        pub ran: Mutex<Vec<String>>,
    }

    impl DesktopTools for FakeDesktop {
        fn env(&self, key: &str) -> Option<String> {
            self.env.iter().find(|k| **k == key).map(|_| "1".to_owned())
        }

        fn have(&self, program: &str) -> bool {
            self.programs.contains(program)
        }

        fn run(&self, program: &str, args: &[&str]) -> Result<String, String> {
            let line = format!("{program} {}", args.join(" "));
            self.ran.lock().map(|mut r| r.push(line.clone())).ok();
            self.answers.get(&line).cloned().ok_or_else(|| format!("{line}: failed"))
        }
    }

    fn raiser(d: FakeDesktop) -> (Raiser, Arc<FakeDesktop>) {
        let d = Arc::new(d);
        (Raiser::new(d.clone()), d)
    }

    fn ran(d: &FakeDesktop) -> Vec<String> {
        d.ran.lock().map(|r| r.clone()).unwrap_or_default()
    }

    const CLIENTS: &str = r#"[{"class": "firefox"}, {"class":   "truesense", "title": "TrueSense"}]"#;
    const LUA: &str = r#"hyprctl dispatch hl.dsp.focus({ window = "class:^truesense$" })"#;

    #[test]
    fn hyprland_focuses_a_listed_window_with_the_lua_dispatcher() {
        let (r, d) = raiser(FakeDesktop {
            env: ["HYPRLAND_INSTANCE_SIGNATURE"].into(),
            programs: ["hyprctl", "swaymsg"].into(),
            answers: [
                ("hyprctl clients -j".to_owned(), CLIENTS.to_owned()),
                (LUA.to_owned(), "ok".to_owned()),
            ]
            .into(),
            ..FakeDesktop::default()
        });
        assert!(r.raise());
        assert_eq!(ran(&d), ["hyprctl clients -j", LUA], "Hyprland wins over the other tools");
    }

    #[test]
    fn hyprland_falls_back_to_the_classic_dispatcher_when_lua_is_not_known() {
        let classic = "hyprctl dispatch focuswindow class:^truesense$";
        let (r, d) = raiser(FakeDesktop {
            env: ["HYPRLAND_INSTANCE_SIGNATURE"].into(),
            programs: ["hyprctl"].into(),
            answers: [
                ("hyprctl clients -j".to_owned(), CLIENTS.to_owned()),
                (LUA.to_owned(), "Invalid dispatcher".to_owned()),
                (classic.to_owned(), "ok".to_owned()),
            ]
            .into(),
            ..FakeDesktop::default()
        });
        assert!(r.raise());
        assert_eq!(ran(&d).last().map(String::as_str), Some(classic));
    }

    #[test]
    fn hyprland_without_our_window_raises_nothing() {
        for clients in ["[]", r#"[{"class": "firefox"}]"#, "not json", r#"{"class": "truesense"}"#, ""] {
            let (r, d) = raiser(FakeDesktop {
                env: ["HYPRLAND_INSTANCE_SIGNATURE"].into(),
                programs: ["hyprctl"].into(),
                answers: [("hyprctl clients -j".to_owned(), clients.to_owned())].into(),
                ..FakeDesktop::default()
            });
            assert!(!r.raise(), "{clients}");
            assert_eq!(ran(&d), ["hyprctl clients -j"], "no dispatch for {clients:?}");
        }
    }

    #[test]
    fn sway_needs_a_success_answer() {
        let line = r#"swaymsg [app_id="truesense"] focus"#;
        let make = |answer: &str| {
            raiser(FakeDesktop {
                env: ["SWAYSOCK"].into(),
                programs: ["swaymsg"].into(),
                answers: [(line.to_owned(), answer.to_owned())].into(),
                ..FakeDesktop::default()
            })
            .0
        };
        assert!(make(r#"[{ "success": true }]"#).raise());
        assert!(!make(r#"[{ "success": false, "error": "no match" }]"#).raise());
    }

    #[test]
    fn x11_uses_wmctrl_then_xdotool() {
        let (r, d) = raiser(FakeDesktop {
            env: ["DISPLAY"].into(),
            programs: ["wmctrl", "xdotool"].into(),
            answers: [("wmctrl -x -a truesense".to_owned(), String::new())].into(),
            ..FakeDesktop::default()
        });
        assert!(r.raise());
        assert_eq!(ran(&d), ["wmctrl -x -a truesense"]);

        let (r, d) = raiser(FakeDesktop {
            env: ["DISPLAY"].into(),
            programs: ["xdotool"].into(),
            answers: [
                ("xdotool search --class truesense".to_owned(), "9437186\n9437190\n".to_owned()),
                ("xdotool windowactivate 9437186".to_owned(), String::new()),
            ]
            .into(),
            ..FakeDesktop::default()
        });
        assert!(r.raise());
        assert_eq!(ran(&d).last().map(String::as_str), Some("xdotool windowactivate 9437186"));

        let (r, _) = raiser(FakeDesktop {
            env: ["DISPLAY"].into(),
            programs: ["xdotool"].into(),
            answers: [("xdotool search --class truesense".to_owned(), "\n".to_owned())].into(),
            ..FakeDesktop::default()
        });
        assert!(!r.raise(), "no window found");
    }

    #[test]
    fn no_desktop_tool_means_nothing_was_raised() {
        let (r, d) = raiser(FakeDesktop::default());
        assert!(!r.raise());
        assert!(ran(&d).is_empty());
        let (r, d) = raiser(FakeDesktop { env: ["DISPLAY"].into(), ..FakeDesktop::default() });
        assert!(!r.raise());
        assert!(ran(&d).is_empty(), "a desktop without any tool starts nothing");
    }

    #[test]
    fn which_finds_executables_on_a_path_string() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("ts-gui-which-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let exe = dir.join("tool");
        std::fs::write(&exe, "#!/bin/sh\n").expect("file");
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        std::fs::write(dir.join("plain"), "x").expect("plain");
        let path = format!("/nonexistent::{}", dir.display());
        assert_eq!(which("tool", &path), Some(exe));
        assert_eq!(which("plain", &path), None, "not executable");
        assert_eq!(which("tool", ""), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
