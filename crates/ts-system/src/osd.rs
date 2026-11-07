//! osd: short on-screen messages ("Audio output: Headphones") for the command line, the key
//! service and the daemon, through one implementation.
//!
//! The style (config `[osd] style`, overridden by the environment variable `TRUESENSE_OSD`) is
//! `native` (the compositor's own notification, then the desktop notification daemon),
//! `noctalia` (Noctalia's OSD popup) or `none` (silent). A missing helper program is never an
//! error: showing a message is a courtesy and must not break the action that wanted it.
//!
//! The daemon runs as root but the desktop session belongs to the user, so as root every helper
//! is run as that user (`runuser`) with the session's environment rebuilt from the runtime
//! directory: there is no inherited `WAYLAND_DISPLAY` or session bus address to rely on.
//!
//! Every effect is injected: the program runner ([`SessionRunner`]), the user database
//! ([`UserDirectory`]), the environment, the runtime directory, the sleeper and whether we are
//! root. Tests use fakes and never start a process.
#![forbid(unsafe_code)]

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use ts_foundation::sync::locked;
use ts_platform::hw::Sleeper;

/// How long one helper may take. A wedged compositor must not stall the daemon's tick or a key
/// press.
const HELPER_TIMEOUT: Duration = Duration::from_secs(4);

/// How long a message stays on screen, in milliseconds (`hyprctl notify` and `notify-send`).
const SHOW_MS: &str = "1800";

/// How long Noctalia's popups stay silenced after the last [`Quiet`] section by default: the
/// time PipeWire needs to finish recreating devices after a profile flip.
pub const DEFAULT_SETTLE: Duration = Duration::from_millis(800);

/// The effective user id of this process, read from `<proc_root>/self/status` (`Uid:` lists the
/// real, effective, saved and filesystem ids). `None` when it cannot be read.
///
/// Read from `/proc` rather than through `geteuid(2)` so this crate stays free of `unsafe`
/// and the answer can be faked in a test.
pub fn effective_uid(proc_root: &Path) -> Option<u32> {
    let status = fs::read(proc_root.join("self/status")).ok()?;
    let status = String::from_utf8_lossy(&status);
    let line = status.lines().find(|l| l.starts_with("Uid:"))?;
    line.split_whitespace().nth(2)?.parse().ok()
}

/// One helper program to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// The program, found through `PATH`.
    pub program: String,
    /// Its arguments.
    pub args: Vec<String>,
    /// The complete environment to run it with, or `None` to inherit ours.
    pub env: Option<Vec<(String, String)>>,
    /// Run it as this user (through `runuser`); only ever set while we are root.
    pub as_user: Option<String>,
}

/// What a helper did: whether it exited with status 0, and what it printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The exit status was 0.
    pub success: bool,
    /// Standard output, lossily decoded.
    pub stdout: String,
}

/// Runs helper programs. `None` means the program could not be run at all or ran out of time;
/// the caller treats that like a refusal.
pub trait SessionRunner: Send + Sync {
    /// Runs `call` and waits for it, giving up after a few seconds.
    fn run(&self, call: &Invocation) -> Option<Outcome>;
}

/// Runs real processes, killing one that overruns `HELPER_TIMEOUT`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessSessionRunner;

impl SessionRunner for ProcessSessionRunner {
    fn run(&self, call: &Invocation) -> Option<Outcome> {
        let mut command = match &call.as_user {
            Some(user) => {
                let mut c = Command::new("runuser");
                c.args(["-u", user, "--", &call.program]);
                c
            }
            None => Command::new(&call.program),
        };
        command.args(&call.args);
        if let Some(env) = &call.env {
            command.env_clear().envs(env.iter().map(|(k, v)| (k, v)));
        }
        let mut child =
            command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
        let mut stdout = child.stdout.take()?;
        let reader = thread::spawn(move || {
            let mut out = Vec::new();
            let _ = stdout.read_to_end(&mut out);
            out
        });
        let deadline = Instant::now() + HELPER_TIMEOUT;
        let mut backoff = ts_foundation::backoff::Backoff::new();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => backoff.wait(),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = reader.join();
                    return None;
                }
            }
        };
        let out = reader.join().ok()?;
        Some(Outcome { success: status.success(), stdout: String::from_utf8_lossy(&out).into_owned() })
    }
}

/// What the notifier needs to know about a desktop user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserEntry {
    /// The numeric user id (names the runtime directory `/run/user/<uid>`).
    pub uid: u32,
    /// The home directory.
    pub home: String,
}

/// Finds a user by name.
pub trait UserDirectory: Send + Sync {
    /// The entry of `name`, or `None` when there is no such user.
    fn lookup(&self, name: &str) -> Option<UserEntry>;
}

/// Looks users up in a passwd-format file (`/etc/passwd`). Users that only exist in a network
/// directory are not seen; the desktop user of a laptop is a local one.
#[derive(Debug, Clone)]
pub struct PasswdFile {
    path: PathBuf,
}

impl PasswdFile {
    /// Reads the passwd-format file at `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl UserDirectory for PasswdFile {
    fn lookup(&self, name: &str) -> Option<UserEntry> {
        let text = fs::read(&self.path).ok()?;
        parse_passwd(&String::from_utf8_lossy(&text), name)
    }
}

/// Pure part of [`PasswdFile`]: `name:x:uid:gid:gecos:home:shell`. Lines that do not have those
/// fields are skipped, never an error.
fn parse_passwd(text: &str, name: &str) -> Option<UserEntry> {
    text.lines().find_map(|line| {
        let f: Vec<&str> = line.split(':').collect();
        if f.len() < 6 || f[0] != name {
            return None;
        }
        Some(UserEntry { uid: f[2].parse().ok()?, home: f[5].to_owned() })
    })
}

/// How a program gets the process environment.
type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Shows on-screen messages. Cheap to clone: clones share the quiet-section counter, so a
/// [`Quiet`] section started through one clone is seen by all.
#[derive(Clone)]
pub struct Osd {
    style: String,
    user: String,
    is_root: bool,
    runtime_dir: PathBuf,
    runner: Arc<dyn SessionRunner>,
    users: Arc<dyn UserDirectory>,
    env: EnvLookup,
    sleeper: Sleeper,
    quiet_depth: Arc<Mutex<usize>>,
}

impl Osd {
    /// A notifier that runs helpers with `runner` and finds users with `users`. Defaults:
    /// style `native`, no target user, not root, runtime directories under `/run/user`, the
    /// real process environment and `thread::sleep`.
    pub fn new(runner: Arc<dyn SessionRunner>, users: Arc<dyn UserDirectory>) -> Self {
        Self {
            style: "native".to_owned(),
            user: String::new(),
            is_root: false,
            runtime_dir: PathBuf::from("/run/user"),
            runner,
            users,
            env: Arc::new(ts_foundation::fsx::env_nonempty),
            sleeper: Arc::new(thread::sleep),
            quiet_depth: Arc::new(Mutex::new(0)),
        }
    }

    /// The real machine: real processes, `/etc/passwd`, and root detected from `/proc`.
    /// `style` is the configured style and `user` the desktop user to show messages to when
    /// running as root (the config's `user`).
    pub fn system(style: &str, user: &str) -> Self {
        let root = effective_uid(Path::new("/proc")) == Some(0);
        Self::new(Arc::new(ProcessSessionRunner), Arc::new(PasswdFile::new("/etc/passwd")))
            .with_style(style)
            .with_user(user)
            .with_root(root)
    }

    /// Sets the configured style (`native`, `noctalia`, `none`); empty means `native`. The
    /// notifier is immutable, so code that reloads the configuration builds a new one.
    #[must_use]
    pub fn with_style(mut self, style: &str) -> Self {
        self.style = style.to_owned();
        self
    }

    /// Sets the desktop user to show messages to when running as root.
    #[must_use]
    pub fn with_user(mut self, user: &str) -> Self {
        self.user = user.to_owned();
        self
    }

    /// Says whether this process is root (then helpers are run as the desktop user).
    #[must_use]
    pub fn with_root(mut self, is_root: bool) -> Self {
        self.is_root = is_root;
        self
    }

    /// Sets the base of the per-user runtime directories (`/run/user`).
    #[must_use]
    pub fn with_runtime_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.runtime_dir = dir.into();
        self
    }

    /// Sets how environment variables (`TRUESENSE_OSD`) are looked up.
    #[must_use]
    pub fn with_env(mut self, env: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> Self {
        self.env = Arc::new(env);
        self
    }

    /// Sets the sleeper used for the settle time of [`Osd::quiet`].
    #[must_use]
    pub fn with_sleeper(mut self, sleeper: Sleeper) -> Self {
        self.sleeper = sleeper;
        self
    }

    /// The style in force: `TRUESENSE_OSD` wins over the configured style, which defaults to
    /// `native`.
    fn style(&self) -> String {
        (self.env)("TRUESENSE_OSD")
            .filter(|s| !s.is_empty())
            .or_else(|| Some(self.style.clone()).filter(|s| !s.is_empty()))
            .unwrap_or_else(|| "native".to_owned())
    }

    /// The environment of the user's desktop session. As root the daemon has none of its own,
    /// and the bus and compositor sockets are found by name in the user's runtime directory.
    /// `None` when the user does not exist.
    fn session_env(&self) -> Option<Vec<(String, String)>> {
        let entry = self.users.lookup(&self.user)?;
        let runtime = self.runtime_dir.join(entry.uid.to_string());
        let mut env = vec![
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ("HOME".to_owned(), entry.home),
            ("XDG_RUNTIME_DIR".to_owned(), runtime.display().to_string()),
            ("DBUS_SESSION_BUS_ADDRESS".to_owned(), format!("unix:path={}/bus", runtime.display())),
        ];
        let sockets = newest_first_sorted(&runtime, "wayland-");
        if !sockets.is_empty() {
            // The newest socket that is not a lock file; when only lock files exist the usual
            // first compositor socket name is the best guess.
            let name =
                sockets.iter().rev().find(|n| !n.ends_with(".lock")).map_or("wayland-1", String::as_str);
            env.push(("WAYLAND_DISPLAY".to_owned(), name.to_owned()));
        }
        // Hyprland names a directory per instance; its newest one is the live compositor.
        if let Some(sig) = newest_first_sorted(&runtime.join("hypr"), "").pop() {
            env.push(("HYPRLAND_INSTANCE_SIGNATURE".to_owned(), sig));
        }
        Some(env)
    }

    fn run(
        &self,
        env: &Option<Vec<(String, String)>>,
        as_user: &Option<String>,
        program: &str,
        args: &[&str],
    ) -> Option<Outcome> {
        self.runner.run(&Invocation {
            program: program.to_owned(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            env: env.clone(),
            as_user: as_user.clone(),
        })
    }

    /// Shows `text` with an icon name (`audio`, `speaker`, `power`, `fan`, `mic`, ...; Noctalia
    /// translates the ones it knows). Returns whether a helper accepted the message: `false`
    /// for the `none` style, an unknown user, or when no helper took it.
    pub fn show(&self, text: &str, icon: &str) -> bool {
        let style = self.style();
        if style == "none" {
            return false;
        }
        let (env, as_user) = if self.is_root && !self.user.is_empty() {
            match self.session_env() {
                Some(env) => (Some(env), Some(self.user.clone())),
                None => return false,
            }
        } else {
            (None, None)
        };
        if style == "noctalia" {
            let icon = noctalia_icon(icon);
            return self
                .run(&env, &as_user, "noctalia", &["msg", "osd-show", icon, text])
                .is_some_and(|o| o.success);
        }
        // `hyprctl notify` answers "ok" on stdout whatever its exit status; anything else (not
        // Hyprland, an error text) falls through to the desktop's notification daemon.
        let hypr = self.run(&env, &as_user, "hyprctl", &["notify", "1", SHOW_MS, "rgb(00aec7)", text]);
        if hypr.is_some_and(|o| o.stdout.trim() == "ok") {
            return true;
        }
        self.run(&env, &as_user, "notify-send", &["-a", "TrueSense", "-t", SHOW_MS, "TrueSense", text])
            .is_some_and(|o| o.success)
    }

    /// Silences Noctalia's own popups while a profile flip makes PipeWire recreate audio
    /// devices (it would announce every transient device). They come back `settle` after the
    /// returned guard is dropped, also on an error path; overlapping sections count, and only
    /// the outermost one disables and re-enables.
    pub fn quiet(&self, settle: Duration) -> Quiet {
        let first = {
            let mut depth = locked(&self.quiet_depth);
            *depth += 1;
            *depth == 1
        };
        if first {
            self.run(&None, &None, "noctalia", &["msg", "osd-disable"]);
        }
        Quiet { osd: self.clone(), settle }
    }
}

/// A silenced-popups section; see [`Osd::quiet`]. Dropping it ends the section.
#[must_use = "the popups come back when the guard is dropped"]
pub struct Quiet {
    osd: Osd,
    settle: Duration,
}

impl Drop for Quiet {
    fn drop(&mut self) {
        let last = *locked(&self.osd.quiet_depth) == 1;
        if last {
            (self.osd.sleeper)(self.settle);
            self.osd.run(&None, &None, "noctalia", &["msg", "osd-enable"]);
        }
        let mut depth = locked(&self.osd.quiet_depth);
        *depth = depth.saturating_sub(1);
    }
}

/// The name Noctalia's OSD knows for one of our icon names; others pass through.
fn noctalia_icon(icon: &str) -> &str {
    match icon {
        "audio" => "headphones",
        "speaker" => "device-speaker",
        "power" | "fan" => "gauge",
        "mic" => "microphone",
        other => other,
    }
}

/// The entries of `dir` whose name starts with `prefix` (hidden entries are never matched),
/// ordered oldest first by modification time, then by name. Empty when `dir` cannot be read.
fn newest_first_sorted(dir: &Path, prefix: &str) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut found: Vec<(SystemTime, String)> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            if name.starts_with('.') || !name.starts_with(prefix) {
                return None;
            }
            let mtime = fs::metadata(e.path()).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            Some((mtime, name))
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, n)| n).collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Records every call and answers by program name.
    #[derive(Default)]
    struct FakeRunner {
        calls: Mutex<Vec<Invocation>>,
        answers: Mutex<Vec<(String, Outcome)>>,
    }

    impl FakeRunner {
        fn answer(&self, program: &str, success: bool, stdout: &str) {
            self.answers
                .lock()
                .unwrap()
                .push((program.to_owned(), Outcome { success, stdout: stdout.to_owned() }));
        }
        fn programs(&self) -> Vec<String> {
            self.calls.lock().unwrap().iter().map(|c| c.program.clone()).collect()
        }
        fn calls(&self) -> Vec<Invocation> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl SessionRunner for FakeRunner {
        fn run(&self, call: &Invocation) -> Option<Outcome> {
            self.calls.lock().unwrap().push(call.clone());
            self.answers.lock().unwrap().iter().find(|(p, _)| *p == call.program).map(|(_, o)| o.clone())
        }
    }

    struct FakeUsers;
    impl UserDirectory for FakeUsers {
        fn lookup(&self, name: &str) -> Option<UserEntry> {
            (name == "alice").then(|| UserEntry { uid: 1000, home: "/home/alice".into() })
        }
    }

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let p = std::env::temp_dir().join(format!(
                "ts-system-osd-{tag}-{}-{}",
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

    fn osd(runner: &Arc<FakeRunner>) -> Osd {
        Osd::new(runner.clone(), Arc::new(FakeUsers)).with_env(|_| None)
    }

    #[test]
    fn native_prefers_the_compositor_when_it_says_ok() {
        let r = Arc::new(FakeRunner::default());
        r.answer("hyprctl", true, "ok\n");
        assert!(osd(&r).show("Power mode: turbo", "power"));
        assert_eq!(r.programs(), ["hyprctl"]);
        let c = &r.calls()[0];
        assert_eq!(c.args, ["notify", "1", "1800", "rgb(00aec7)", "Power mode: turbo"]);
        assert_eq!((c.env.clone(), c.as_user.clone()), (None, None));
    }

    #[test]
    fn native_falls_back_to_notify_send_and_reports_its_status() {
        let r = Arc::new(FakeRunner::default());
        r.answer("hyprctl", false, "not hyprland");
        r.answer("notify-send", true, "");
        assert!(osd(&r).show("hi", "audio"));
        assert_eq!(r.programs(), ["hyprctl", "notify-send"]);
        assert_eq!(r.calls()[1].args, ["-a", "TrueSense", "-t", "1800", "TrueSense", "hi"]);

        let r = Arc::new(FakeRunner::default());
        r.answer("notify-send", false, "");
        assert!(!osd(&r).show("hi", "audio"), "notify-send failing means nobody showed it");

        let r = Arc::new(FakeRunner::default());
        assert!(!osd(&r).show("hi", "audio"), "no helper installed is not an error, just false");
        assert_eq!(r.programs(), ["hyprctl", "notify-send"]);
    }

    #[test]
    fn hyprctl_only_counts_when_it_prints_ok() {
        let r = Arc::new(FakeRunner::default());
        r.answer("hyprctl", true, "error: no such thing");
        r.answer("notify-send", true, "");
        assert!(osd(&r).show("x", "power"));
        assert_eq!(r.programs(), ["hyprctl", "notify-send"]);
    }

    #[test]
    fn none_is_silent_and_never_runs_anything() {
        let r = Arc::new(FakeRunner::default());
        assert!(!osd(&r).with_style("none").show("x", "power"));
        assert!(r.programs().is_empty());
    }

    #[test]
    fn noctalia_translates_icons_and_follows_the_exit_status() {
        for (icon, shown) in [
            ("audio", "headphones"),
            ("speaker", "device-speaker"),
            ("power", "gauge"),
            ("fan", "gauge"),
            ("mic", "microphone"),
            ("update", "update"),
        ] {
            let r = Arc::new(FakeRunner::default());
            r.answer("noctalia", true, "");
            assert!(osd(&r).with_style("noctalia").show("t", icon));
            assert_eq!(r.calls()[0].args, ["msg", "osd-show", shown, "t"]);
            assert_eq!(r.programs(), ["noctalia"], "no fallback to the native helpers");
        }
        let r = Arc::new(FakeRunner::default());
        r.answer("noctalia", false, "");
        assert!(!osd(&r).with_style("noctalia").show("t", "audio"));
        assert!(!osd(&Arc::new(FakeRunner::default())).with_style("noctalia").show("t", "audio"));
    }

    #[test]
    fn the_environment_overrides_the_configured_style() {
        let r = Arc::new(FakeRunner::default());
        let o = osd(&r).with_style("native").with_env(|k| (k == "TRUESENSE_OSD").then(|| "none".to_owned()));
        assert!(!o.show("x", "power"));
        assert!(r.programs().is_empty());
        let r = Arc::new(FakeRunner::default());
        r.answer("noctalia", true, "");
        let o =
            osd(&r).with_style("none").with_env(|k| (k == "TRUESENSE_OSD").then(|| "noctalia".to_owned()));
        assert!(o.show("x", "power"));
        // An empty configured style means native; an unknown one behaves like native too.
        let r = Arc::new(FakeRunner::default());
        r.answer("hyprctl", true, "ok");
        assert!(osd(&r).with_style("").show("x", "power"));
        assert!(osd(&r).with_style("fancy").show("x", "power"));
    }

    #[test]
    fn as_root_helpers_run_as_the_user_in_the_session_environment() {
        let tmp = TempDir::new("session");
        let rt = tmp.0.join("1000");
        fs::create_dir_all(rt.join("hypr/aaa_old")).unwrap();
        fs::create_dir_all(rt.join("hypr/bbb_new")).unwrap();
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let new = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000);
        fs::File::open(rt.join("hypr/aaa_old")).unwrap().set_modified(old).unwrap();
        fs::File::open(rt.join("hypr/bbb_new")).unwrap().set_modified(new).unwrap();
        for (name, t) in [
            ("wayland-0", old),
            ("wayland-1", new),
            ("wayland-1.lock", SystemTime::UNIX_EPOCH + Duration::from_secs(3_000_000)),
        ] {
            let f = fs::File::create(rt.join(name)).unwrap();
            f.set_modified(t).unwrap();
        }
        let r = Arc::new(FakeRunner::default());
        r.answer("hyprctl", true, "ok");
        let o = osd(&r).with_root(true).with_user("alice").with_runtime_dir(&tmp.0);
        assert!(o.show("hello", "power"));
        let call = &r.calls()[0];
        assert_eq!(call.as_user.as_deref(), Some("alice"));
        let env = call.env.clone().unwrap();
        let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("PATH").as_deref(), Some("/usr/bin:/bin"));
        assert_eq!(get("HOME").as_deref(), Some("/home/alice"));
        assert_eq!(get("XDG_RUNTIME_DIR"), Some(rt.display().to_string()));
        assert_eq!(get("DBUS_SESSION_BUS_ADDRESS"), Some(format!("unix:path={}/bus", rt.display())));
        assert_eq!(
            get("WAYLAND_DISPLAY").as_deref(),
            Some("wayland-1"),
            "the newest socket that is not a lock file"
        );
        assert_eq!(get("HYPRLAND_INSTANCE_SIGNATURE").as_deref(), Some("bbb_new"));
    }

    #[test]
    fn session_environment_without_sockets_has_only_the_basics() {
        let tmp = TempDir::new("nosock");
        fs::create_dir_all(tmp.0.join("1000")).unwrap();
        fs::File::create(tmp.0.join("1000/wayland-0.lock")).unwrap();
        let r = Arc::new(FakeRunner::default());
        let o = osd(&r).with_root(true).with_user("alice").with_runtime_dir(&tmp.0);
        let env = o.session_env().unwrap();
        // Only a lock file: the usual first socket name is assumed (like the Python edition).
        assert!(env.contains(&("WAYLAND_DISPLAY".to_owned(), "wayland-1".to_owned())));
        assert!(!env.iter().any(|(k, _)| k == "HYPRLAND_INSTANCE_SIGNATURE"));
        let o = osd(&r).with_root(true).with_user("alice").with_runtime_dir(tmp.0.join("missing"));
        let env = o.session_env().unwrap();
        assert!(!env.iter().any(|(k, _)| k == "WAYLAND_DISPLAY"));
    }

    #[test]
    fn as_root_an_unknown_user_shows_nothing() {
        let r = Arc::new(FakeRunner::default());
        assert!(!osd(&r).with_root(true).with_user("mallory").show("x", "power"));
        assert!(r.programs().is_empty());
        // Root without a configured user cannot reach any session either, but Python still
        // runs the helpers (as root, with the daemon's own environment).
        r.answer("hyprctl", true, "ok");
        assert!(osd(&r).with_root(true).show("x", "power"));
        assert_eq!(r.calls()[0].as_user, None);
    }

    #[test]
    fn a_normal_user_runs_helpers_directly_even_with_a_configured_user() {
        let r = Arc::new(FakeRunner::default());
        r.answer("hyprctl", true, "ok");
        assert!(osd(&r).with_user("alice").show("x", "power"));
        assert_eq!(r.calls()[0].as_user, None);
        assert_eq!(r.calls()[0].env, None);
    }

    #[test]
    fn quiet_sections_nest_and_only_the_outermost_toggles_noctalia() {
        let r = Arc::new(FakeRunner::default());
        let slept = Arc::new(Mutex::new(Vec::new()));
        let s = slept.clone();
        let o = osd(&r).with_sleeper(Arc::new(move |d| s.lock().unwrap().push(d)));
        {
            let outer = o.quiet(Duration::from_millis(5));
            {
                let _inner = o.clone().quiet(Duration::from_millis(7));
                assert_eq!(r.programs(), ["noctalia"]);
            }
            assert_eq!(r.programs(), ["noctalia"], "an inner section ending changes nothing");
            assert!(slept.lock().unwrap().is_empty());
            drop(outer);
        }
        let calls = r.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].args, ["msg", "osd-disable"]);
        assert_eq!(calls[1].args, ["msg", "osd-enable"]);
        assert_eq!(
            *slept.lock().unwrap(),
            [Duration::from_millis(5)],
            "the popups return after the settle time"
        );
        // A new section after the last ended starts over.
        drop(o.quiet(Duration::ZERO));
        assert_eq!(r.programs().len(), 4);
    }

    #[test]
    fn quiet_re_enables_the_popups_when_the_section_unwinds() {
        let r = Arc::new(FakeRunner::default());
        let o = osd(&r).with_sleeper(Arc::new(|_| {}));
        let o2 = o.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _q = o2.quiet(Duration::ZERO);
            std::panic::resume_unwind(Box::new("boom"));
        }));
        assert!(result.is_err());
        assert_eq!(r.calls().last().unwrap().args, ["msg", "osd-enable"]);
        drop(o);
    }

    #[test]
    fn passwd_lines_are_parsed_defensively() {
        let text = "root:x:0:0:root:/root:/bin/bash\n# comment\nbroken\nalice:x:1000:1000:Alice,,,:/home/alice:/bin/fish\nbob:x:nan:1:x:/h:/s\n::::::\n";
        assert_eq!(parse_passwd(text, "alice"), Some(UserEntry { uid: 1000, home: "/home/alice".into() }));
        assert_eq!(parse_passwd(text, "root").map(|u| u.uid), Some(0));
        assert_eq!(parse_passwd(text, "bob"), None, "a non-numeric uid is a broken line");
        assert_eq!(parse_passwd(text, "carol"), None);
        assert_eq!(parse_passwd("", "alice"), None);
        assert_eq!(parse_passwd("alice:x:1000", "alice"), None);
    }

    #[test]
    fn passwd_file_reads_a_real_file_and_a_missing_one_is_no_user() {
        let tmp = TempDir::new("passwd");
        fs::write(tmp.0.join("passwd"), "alice:x:1000:1000::/home/alice:/bin/sh\n").unwrap();
        assert_eq!(PasswdFile::new(tmp.0.join("passwd")).lookup("alice").map(|u| u.uid), Some(1000));
        assert_eq!(PasswdFile::new(tmp.0.join("none")).lookup("alice"), None);
    }

    #[test]
    fn effective_uid_is_the_second_field_of_the_uid_line() {
        let tmp = TempDir::new("status");
        fs::create_dir_all(tmp.0.join("self")).unwrap();
        fs::write(tmp.0.join("self/status"), "Name:\tx\nUid:\t1000\t0\t1000\t1000\nGid:\t1\t1\t1\t1\n")
            .unwrap();
        assert_eq!(effective_uid(&tmp.0), Some(0));
        fs::write(tmp.0.join("self/status"), "Uid:\t1000\n").unwrap();
        assert_eq!(effective_uid(&tmp.0), None);
        fs::write(tmp.0.join("self/status"), "Uid:\ta\tb\n").unwrap();
        assert_eq!(effective_uid(&tmp.0), None);
        assert_eq!(effective_uid(&tmp.0.join("nope")), None);
    }

    #[test]
    fn the_real_runner_reports_a_missing_program_as_none() {
        // Only the failure path runs a (non-existent) program: the tests start no real process.
        let call = Invocation {
            program: "/nonexistent/truesense-osd-helper".into(),
            args: vec![],
            env: Some(vec![]),
            as_user: None,
        };
        assert_eq!(ProcessSessionRunner.run(&call), None);
    }
}
