//! The machine around an update: who we are, which helper programs exist, whether a graphical
//! password prompt is available, and how to run the installer with administrator rights.
//!
//! The installer is the one place an update runs downloaded code as root, so everything that
//! decides how it is started is a pure function (tested without a desktop) and the one real
//! effect, running a command with the terminal attached, is the [`Host`] trait that tests
//! replace with a recorder.
#![forbid(unsafe_code)]

use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use ts_platform::hw::CommandRunner;

use super::UpdateError;
use crate::osd::effective_uid;

/// Programs that show pkexec's password prompt. `polkitd` alone is not enough: `pkexec` needs an
/// AGENT to ask for the password, and without one it fails silently.
pub(crate) const POLKIT_AGENTS: [&str; 13] = [
    "polkit-gnome-authentication-agent",
    "polkit-kde-authentication-agent",
    "polkit-mate-authentication-agent",
    "lxqt-policykit-agent",
    "xfce-polkit",
    "hyprpolkitagent",
    "soteria",
    "lxpolkit",
    "pantheon-agent-polkit",
    "ukui-polkit",
    "cosmic-osd",
    "polkit-ukui-authentication-agent",
    "gnome-shell",
];

/// Terminal emulators and how each runs `sh -c COMMAND` and waits until it ends.
pub(crate) const TERMINALS: [(&str, &[&str]); 9] = [
    ("kitty", &["kitty", "sh", "-c"]),
    ("foot", &["foot", "sh", "-c"]),
    ("alacritty", &["alacritty", "-e", "sh", "-c"]),
    ("ghostty", &["ghostty", "-e", "sh", "-c"]),
    ("wezterm", &["wezterm", "start", "--always-new-process", "--", "sh", "-c"]),
    ("konsole", &["konsole", "-e", "sh", "-c"]),
    ("gnome-terminal", &["gnome-terminal", "--wait", "--", "sh", "-c"]),
    ("xfce4-terminal", &["xfce4-terminal", "--disable-server", "-x", "sh", "-c"]),
    ("xterm", &["xterm", "-e", "sh", "-c"]),
];

/// How many bytes of a process's command line are looked at: the program is the first word.
const CMDLINE_BYTES: u64 = 512;

/// Everything about the machine the hand-over depends on.
pub trait Host: Send + Sync {
    /// The effective user id (0 is root, which needs no elevation).
    fn euid(&self) -> u32;
    /// An environment variable, empty counting as unset.
    fn env(&self, key: &str) -> Option<String>;
    /// Whether an executable `name` is found on `PATH`.
    fn has_program(&self, name: &str) -> bool;
    /// Whether some graphical password prompt for `pkexec` is running.
    fn polkit_agent_running(&self) -> bool;
    /// The version of the installed program, `None` when it is not installed or does not answer.
    fn installed_version(&self) -> Option<String>;
    /// Runs a command to completion with the terminal attached and returns its exit code (a
    /// negative number is the signal that ended it). This is how the installer is started.
    fn run_attached(&self, argv: &[String]) -> io::Result<i32>;
}

/// How a variable of the environment is looked up.
type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The real machine. Every place it looks is a field, so its pure parts are tested against a
/// temporary tree; only [`Host::run_attached`] is not exercised by tests.
pub struct SystemHost {
    proc_root: PathBuf,
    installed_program: PathBuf,
    runner: Arc<dyn CommandRunner>,
    env: EnvLookup,
}

impl SystemHost {
    /// The real machine: `/proc`, the installed program `/usr/local/bin/truesense` (where
    /// `tools/install.sh` puts it), the real environment, and `runner` to ask the installed
    /// program for its version.
    pub fn new(runner: Arc<dyn CommandRunner>) -> Self {
        Self {
            proc_root: PathBuf::from("/proc"),
            installed_program: PathBuf::from("/usr/local/bin/truesense"),
            runner,
            env: Arc::new(ts_foundation::fsx::env_nonempty),
        }
    }

    /// Looks for processes and the effective user id below `root` instead of `/proc`.
    #[must_use]
    pub fn with_proc_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.proc_root = root.into();
        self
    }

    /// Asks `program` instead of `/usr/local/bin/truesense` for the installed version.
    #[must_use]
    pub fn with_installed_program(mut self, program: impl Into<PathBuf>) -> Self {
        self.installed_program = program.into();
        self
    }

    /// Reads variables with `env` instead of the process environment.
    #[must_use]
    pub fn with_env(mut self, env: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> Self {
        self.env = Arc::new(env);
        self
    }
}

impl Host for SystemHost {
    fn euid(&self) -> u32 {
        // When it cannot be read, "not root" is the safe answer: the installer is then started
        // through sudo or pkexec, which ask for a password.
        effective_uid(&self.proc_root).unwrap_or(u32::MAX)
    }

    fn env(&self, key: &str) -> Option<String> {
        (self.env)(key)
    }

    fn has_program(&self, name: &str) -> bool {
        let Some(path) = (self.env)("PATH") else { return false };
        path.split(':').filter(|d| !d.is_empty()).any(|dir| is_executable(&Path::new(dir).join(name)))
    }

    fn polkit_agent_running(&self) -> bool {
        polkit_agent_running(&self.proc_root)
    }

    fn installed_version(&self) -> Option<String> {
        let program = self.installed_program.to_str()?;
        let out = self.runner.output(program, &["--version"], Duration::from_secs(5)).ok()?;
        parse_version_line(&String::from_utf8_lossy(&out))
    }

    fn run_attached(&self, argv: &[String]) -> io::Result<i32> {
        let (program, args) =
            argv.split_first().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty command"))?;
        let status = Command::new(program).args(args).status()?;
        Ok(status.code().or_else(|| status.signal().map(|s| -s)).unwrap_or(-1))
    }
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The version out of `truesense --version`, which prints `truesense 0.5.0`.
fn parse_version_line(text: &str) -> Option<String> {
    let mut words = text.split_whitespace();
    (words.next()? == "truesense").then(|| words.next().map(str::to_owned)).flatten()
}

/// Whether some graphical password prompt for `pkexec` runs, by looking at the first word of the
/// command line of every process below `proc_root`.
pub fn polkit_agent_running(proc_root: &Path) -> bool {
    let Ok(entries) = fs::read_dir(proc_root) else { return false };
    entries.filter_map(Result::ok).any(|entry| {
        let name = entry.file_name();
        let Some(pid) = name.to_str().filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        else {
            return false;
        };
        let Ok(file) = fs::File::open(proc_root.join(pid).join("cmdline")) else { return false };
        let mut cmdline = Vec::new();
        if file.take(CMDLINE_BYTES).read_to_end(&mut cmdline).is_err() {
            return false;
        }
        // The program is the first NUL-separated word; arguments that merely mention an agent
        // (a shell sourcing a file called polkit-gnome...) do not count.
        let program =
            String::from_utf8_lossy(cmdline.split(|b| *b == 0).next().unwrap_or_default()).to_lowercase();
        POLKIT_AGENTS.iter().any(|agent| program.contains(agent))
    })
}

/// A command that opens a terminal window, runs the shell command `inner` in it and waits:
/// `$TERMINAL` first (by base name), then the usual ones. `None` when there is no terminal.
pub fn terminal_command(
    inner: &str,
    terminal_env: Option<&str>,
    has_program: &dyn Fn(&str) -> bool,
) -> Option<Vec<String>> {
    let preferred = terminal_env.and_then(|t| t.rsplit('/').next()).filter(|t| !t.is_empty());
    let candidates = preferred.into_iter().chain(TERMINALS.iter().map(|(name, _)| *name));
    for name in candidates {
        let Some((_, argv)) = TERMINALS.iter().find(|(n, _)| *n == name) else { continue };
        if has_program(name) {
            let mut command: Vec<String> = argv.iter().map(|a| (*a).to_owned()).collect();
            command.push(inner.to_owned());
            return Some(command);
        }
    }
    None
}

/// What decides how the installer is elevated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Elevation {
    /// Our effective user id.
    pub euid: u32,
    /// Asked from the window (so a graphical prompt is possible).
    pub gui: bool,
    /// A polkit agent is running.
    pub agent: bool,
    /// There is a desktop session (`WAYLAND_DISPLAY` or `DISPLAY`).
    pub desktop: bool,
    /// `pkexec` is installed.
    pub has_pkexec: bool,
    /// `sudo` is installed.
    pub has_sudo: bool,
}

/// How to run the installer as root: as it is when we already are root; `pkexec` (graphical
/// prompt) only when asked from the window with a desktop AND a polkit agent to ask for the
/// password; else `sudo` (which asks in the terminal); `pkexec` as a last resort.
pub fn elevate_command(script: &str, e: &Elevation) -> Result<Vec<String>, UpdateError> {
    let with = |program: &str| vec![program.to_owned(), script.to_owned()];
    if e.euid == 0 {
        return Ok(vec![script.to_owned()]);
    }
    if e.gui && e.agent && e.has_pkexec && e.desktop {
        return Ok(with("pkexec"));
    }
    if e.has_sudo {
        return Ok(with("sudo"));
    }
    if e.agent && e.has_pkexec {
        return Ok(with("pkexec"));
    }
    Err(UpdateError::NoElevation)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let p = std::env::temp_dir().join(format!(
                "ts-system-host-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A /proc with one directory per command line.
    fn fake_proc(tmp: &TempDir, cmds: &[&str]) {
        for (i, cmd) in cmds.iter().enumerate() {
            let dir = tmp.0.join((100 + i).to_string());
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("cmdline"), format!("{}\0", cmd.replace(' ', "\0"))).unwrap();
        }
    }

    #[test]
    fn polkitd_alone_is_not_a_password_prompt() {
        let tmp = TempDir::new();
        fake_proc(
            &tmp,
            &["/usr/lib/polkit-1/polkitd --no-debug", "/usr/bin/zsh -c source snapshot polkit-gnome"],
        );
        assert!(!polkit_agent_running(&tmp.0));
        let tmp = TempDir::new();
        fake_proc(&tmp, &["/usr/bin/hyprpolkitagent"]);
        assert!(polkit_agent_running(&tmp.0));
        let tmp = TempDir::new();
        fake_proc(&tmp, &["/usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1"]);
        assert!(polkit_agent_running(&tmp.0));
    }

    #[test]
    fn every_known_agent_is_recognised_and_the_match_is_case_insensitive() {
        for agent in POLKIT_AGENTS {
            let tmp = TempDir::new();
            fake_proc(&tmp, &[&format!("/usr/lib/{agent} --x")]);
            assert!(polkit_agent_running(&tmp.0), "{agent}");
        }
        let tmp = TempDir::new();
        fake_proc(&tmp, &["/USR/BIN/HyprPolkitAgent"]);
        assert!(polkit_agent_running(&tmp.0));
    }

    #[test]
    fn process_entries_that_are_not_processes_are_skipped() {
        let tmp = TempDir::new();
        fake_proc(&tmp, &["/usr/bin/bash"]);
        fs::create_dir_all(tmp.0.join("self")).unwrap();
        fs::write(tmp.0.join("self/cmdline"), "soteria\0").unwrap();
        fs::create_dir_all(tmp.0.join("12x")).unwrap();
        fs::write(tmp.0.join("12x/cmdline"), "soteria\0").unwrap();
        fs::create_dir_all(tmp.0.join("500")).unwrap(); // no cmdline: a process that went away
        fs::write(tmp.0.join("501"), "soteria").unwrap(); // a file, not a directory
        assert!(!polkit_agent_running(&tmp.0));
        assert!(!polkit_agent_running(&tmp.0.join("nonexistent")));
        // A huge command line is only read as far as its first word needs.
        let tmp = TempDir::new();
        fs::create_dir_all(tmp.0.join("7")).unwrap();
        fs::write(tmp.0.join("7/cmdline"), format!("/usr/bin/soteria\0{}", "a\0".repeat(100_000))).unwrap();
        assert!(polkit_agent_running(&tmp.0));
        fs::write(tmp.0.join("7/cmdline"), vec![0xffu8; 4096]).unwrap();
        assert!(!polkit_agent_running(&tmp.0));
    }

    fn has_only(names: &[&'static str]) -> impl Fn(&str) -> bool {
        let set: HashSet<&'static str> = names.iter().copied().collect();
        move |n| set.contains(n)
    }

    #[test]
    fn the_terminal_comes_from_the_environment_then_the_usual_ones() {
        let have = has_only(&["foot", "xterm"]);
        assert_eq!(
            terminal_command("echo hi", Some("/usr/bin/foot"), &have).unwrap(),
            ["foot", "sh", "-c", "echo hi"]
        );
        assert_eq!(terminal_command("echo hi", None, &have).unwrap()[0], "foot");
        assert_eq!(terminal_command("x", Some("xterm"), &have).unwrap(), ["xterm", "-e", "sh", "-c", "x"]);
        assert_eq!(
            terminal_command("x", Some("not-a-terminal"), &have).unwrap()[0],
            "foot",
            "an unknown $TERMINAL is skipped"
        );
        assert_eq!(
            terminal_command("x", Some("kitty"), &have).unwrap()[0],
            "foot",
            "a $TERMINAL that is not installed is skipped"
        );
        assert_eq!(terminal_command("x", Some(""), &have).unwrap()[0], "foot");
        assert_eq!(terminal_command("x", Some("/"), &have).unwrap()[0], "foot");
        assert_eq!(terminal_command("x", None, &|_| false), None);
    }

    #[test]
    fn every_terminal_runs_the_command_through_sh_c() {
        for (name, argv) in TERMINALS {
            let have = move |n: &str| n == name;
            let command = terminal_command("the inner command", None, &have).unwrap();
            assert_eq!(command[0], name);
            assert_eq!(&command[command.len() - 3..], ["sh", "-c", "the inner command"], "{name}");
            assert_eq!(command.len(), argv.len() + 1);
        }
    }

    fn elevation() -> Elevation {
        Elevation { euid: 1000, gui: false, agent: false, desktop: false, has_pkexec: true, has_sudo: true }
    }

    #[test]
    fn root_runs_the_installer_directly() {
        let e = Elevation { euid: 0, ..elevation() };
        assert_eq!(elevate_command("/x/install.sh", &e).unwrap(), ["/x/install.sh"]);
    }

    #[test]
    fn pkexec_only_from_the_window_with_an_agent_and_a_desktop() {
        let window = Elevation { gui: true, agent: true, desktop: true, ..elevation() };
        assert_eq!(elevate_command("/x/i.sh", &window).unwrap(), ["pkexec", "/x/i.sh"]);
        for (label, e) in [
            ("no agent: pkexec would fail without a prompt", Elevation { agent: false, ..window }),
            ("not from the window", Elevation { gui: false, ..window }),
            ("no desktop", Elevation { desktop: false, ..window }),
            ("no pkexec", Elevation { has_pkexec: false, ..window }),
        ] {
            assert_eq!(elevate_command("/x/i.sh", &e).unwrap(), ["sudo", "/x/i.sh"], "{label}");
        }
    }

    #[test]
    fn without_sudo_an_agent_makes_pkexec_possible_and_otherwise_it_is_an_error() {
        let no_sudo = Elevation { has_sudo: false, ..elevation() };
        assert_eq!(
            elevate_command("/x/i.sh", &Elevation { agent: true, ..no_sudo }).unwrap(),
            ["pkexec", "/x/i.sh"]
        );
        let err = elevate_command("/x/i.sh", &no_sudo).unwrap_err();
        assert_eq!(err.to_string(), "need sudo, or pkexec with a polkit agent, to install the update");
        let err = elevate_command(
            "/x/i.sh",
            &Elevation { agent: true, has_pkexec: false, has_sudo: false, ..elevation() },
        );
        assert!(err.is_err());
    }

    struct FakeVersion(Mutex<Vec<(String, Vec<String>)>>, Option<&'static str>);
    impl CommandRunner for FakeVersion {
        fn output(&self, program: &str, args: &[&str], _t: Duration) -> io::Result<Vec<u8>> {
            self.0.lock().unwrap().push((program.to_owned(), args.iter().map(|a| (*a).to_owned()).collect()));
            self.1.map(|s| s.as_bytes().to_vec()).ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }
    }

    #[test]
    fn the_installed_version_is_asked_of_the_installed_program() {
        let runner = Arc::new(FakeVersion(Mutex::new(Vec::new()), Some("truesense 0.5.1\n")));
        let host = SystemHost::new(runner.clone());
        assert_eq!(host.installed_version().as_deref(), Some("0.5.1"));
        assert_eq!(
            runner.0.lock().unwrap()[0],
            ("/usr/local/bin/truesense".to_owned(), vec!["--version".to_owned()])
        );
        for (out, want) in [
            (Some("truesense 0.4.0"), Some("0.4.0")),
            (Some("other 1.0"), None),
            (Some(""), None),
            (Some("truesense"), None),
            (None, None),
        ] {
            let host = SystemHost::new(Arc::new(FakeVersion(Mutex::new(Vec::new()), out)))
                .with_installed_program("/opt/truesense");
            assert_eq!(host.installed_version().as_deref(), want, "{out:?}");
        }
    }

    #[test]
    fn programs_are_found_on_the_path_only_when_executable() {
        let tmp = TempDir::new();
        let bin = tmp.0.join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("sudo"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(bin.join("sudo"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(bin.join("pkexec"), "not executable").unwrap();
        fs::set_permissions(bin.join("pkexec"), fs::Permissions::from_mode(0o644)).unwrap();
        fs::create_dir_all(bin.join("kitty")).unwrap(); // a directory is not a program
        let path = format!("/nonexistent::{}", bin.display());
        let host = SystemHost::new(Arc::new(FakeVersion(Mutex::new(Vec::new()), None)))
            .with_env(move |k| (k == "PATH").then(|| path.clone()));
        assert!(host.has_program("sudo"));
        assert!(!host.has_program("pkexec"));
        assert!(!host.has_program("kitty"));
        assert!(!host.has_program("nothing"));
        assert!(host.env("PATH").is_some());
        let no_path = SystemHost::new(Arc::new(FakeVersion(Mutex::new(Vec::new()), None))).with_env(|_| None);
        assert!(!no_path.has_program("sudo"));
    }

    #[test]
    fn the_euid_comes_from_proc_and_an_unreadable_one_is_not_root() {
        let tmp = TempDir::new();
        fs::create_dir_all(tmp.0.join("self")).unwrap();
        fs::write(tmp.0.join("self/status"), "Uid:\t1000\t0\t0\t0\n").unwrap();
        let host =
            SystemHost::new(Arc::new(FakeVersion(Mutex::new(Vec::new()), None))).with_proc_root(&tmp.0);
        assert_eq!(host.euid(), 0);
        fs::write(tmp.0.join("self/status"), "Uid:\t1000\t1000\t1000\t1000\n").unwrap();
        assert_eq!(host.euid(), 1000);
        let lost = SystemHost::new(Arc::new(FakeVersion(Mutex::new(Vec::new()), None)))
            .with_proc_root(tmp.0.join("none"));
        assert_ne!(lost.euid(), 0);
        assert!(!lost.polkit_agent_running());
    }

    #[test]
    fn running_nothing_is_an_error_and_the_real_runner_reports_a_missing_program() {
        let host = SystemHost::new(Arc::new(FakeVersion(Mutex::new(Vec::new()), None)));
        assert_eq!(host.run_attached(&[]).unwrap_err().kind(), io::ErrorKind::InvalidInput);
        // Only the failure path starts a (non-existent) program: the tests run no real process.
        let err = host.run_attached(&["/nonexistent/truesense-installer".to_owned()]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
