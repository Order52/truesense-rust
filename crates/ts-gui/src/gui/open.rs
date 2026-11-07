//! Showing the window: a browser as an application window.
//!
//! Qt had a window of its own; here the page is shown by whatever browser the user has, as a
//! window with no tabs and no address bar so it looks like an application:
//!
//! 1. a Chromium-family browser with `--app=URL` (with the window class `truesense`);
//! 2. Firefox, which has no application mode, in a profile of its own whose `userChrome.css` hides
//!    the tabs and the navigation bar (see [`FIREFOX_USER_CHROME`]), started with `--name truesense`;
//! 3. `xdg-open`;
//! 4. otherwise the address is printed for the user to open.
//!
//! The address carries a one-time launch code, never the session token, so a copy of it taken from
//! the browser's command line is worthless once the window has opened. Every external effect is
//! behind [`LauncherHost`], so the choice of browser is tested without starting one.
#![forbid(unsafe_code)]

use std::fmt;
use std::fs;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;

use ts_foundation::fsx;

use super::show::{WINDOW_CLASS, which};

/// The browsers that can show a page as an application window without tabs or an address bar.
pub const CHROMIUM_FAMILY: [&str; 11] = [
    "chromium",
    "chromium-browser",
    "google-chrome-stable",
    "google-chrome",
    "brave",
    "brave-browser",
    "microsoft-edge-stable",
    "microsoft-edge",
    "vivaldi",
    "vivaldi-stable",
    "helium-browser",
];

/// The window's size when the browser opens it.
const WINDOW_SIZE: (u32, u32) = (1180, 820);

/// The name of Firefox's application profile under `<data dir>/truesense/`.
pub const FIREFOX_PROFILE: &str = "firefox-app";

/// Quiets everything a first run would show: this is an application window, not a browser.
pub const FIREFOX_USER_JS: &str = r#"// Written by TrueSense. This profile is only for the TrueSense window.
user_pref("toolkit.legacyUserProfileCustomizations.stylesheets", true);
user_pref("browser.shell.checkDefaultBrowser", false);
user_pref("browser.aboutwelcome.enabled", false);
user_pref("browser.startup.homepage_override.mstone", "ignore");
user_pref("startup.homepage_welcome_url", "");
user_pref("startup.homepage_welcome_url.additional", "");
user_pref("browser.tabs.warnOnClose", false);
user_pref("browser.sessionstore.resume_from_crash", false);
user_pref("browser.sessionstore.max_resumed_crashes", 0);
user_pref("browser.toolbars.bookmarks.visibility", "never");
user_pref("browser.newtabpage.enabled", false);
user_pref("browser.disableResetPrompt", true);
user_pref("browser.urlbar.suggest.searches", false);
user_pref("datareporting.policy.dataSubmissionEnabled", false);
user_pref("datareporting.healthreport.uploadEnabled", false);
user_pref("toolkit.telemetry.reportingpolicy.firstRun", false);
user_pref("app.update.auto", false);
"#;

/// Hides the browser's own controls. The title bar's window buttons stay.
pub const FIREFOX_USER_CHROME: &str = r#"/* Written by TrueSense: the page is the application, so no tabs, address bar or bookmarks. */
@namespace url("http://www.mozilla.org/keymaster/gatekeeper/there.is.only.xul");
#nav-bar, #PersonalToolbar, #sidebar-main, #sidebar-box { visibility: collapse !important; }
#tabbrowser-tabs, #alltabs-button { visibility: collapse !important; }
"#;

/// Shows an address as the window. The server calls it once at start; a test passes a recorder.
pub trait Opener: Send + Sync {
    /// Shows `url`. An error is reported by the caller and never stops the server (the window can
    /// still be opened by hand).
    fn open(&self, url: &str) -> Result<(), String>;
}

/// What the launcher needs from the machine.
pub trait LauncherHost: Send + Sync {
    /// A variable of the environment (a desktop is there when `DISPLAY` or `WAYLAND_DISPLAY` is set).
    fn env(&self, key: &str) -> Option<String>;
    /// The path of a program on `PATH`.
    fn look_path(&self, program: &str) -> Option<PathBuf>;
    /// Starts a program detached from this one and returns without waiting.
    fn start(&self, program: &Path, args: &[String]) -> io::Result<()>;
    /// The user's data directory (`XDG_DATA_HOME`); Firefox's application profile lives under it.
    fn data_dir(&self) -> PathBuf;
    /// Whether a Firefox is already running on the profile directory.
    fn firefox_running(&self, profile: &Path) -> bool;
    /// Where the address is printed when no browser can be started.
    fn print(&self, text: &str);
}

/// The real machine: the process environment, `PATH`, processes and the terminal.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemHost;

impl LauncherHost for SystemHost {
    fn env(&self, key: &str) -> Option<String> {
        fsx::env_nonempty(key)
    }

    fn look_path(&self, program: &str) -> Option<PathBuf> {
        which(program, self.env("PATH").as_deref().unwrap_or(""))
    }

    fn start(&self, program: &Path, args: &[String]) -> io::Result<()> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // Its own process group: closing the terminal that started us does not close the window.
            .process_group(0)
            .spawn()?;
        // Reap it when it ends; the browser outlives this call.
        thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }

    fn data_dir(&self) -> PathBuf {
        if let Some(dir) = self.env("XDG_DATA_HOME") {
            return PathBuf::from(dir);
        }
        match self.env("HOME") {
            Some(home) => Path::new(&home).join(".local").join("share"),
            None => std::env::temp_dir(),
        }
    }

    fn firefox_running(&self, profile: &Path) -> bool {
        firefox_profile_running(profile, &|pid| Path::new("/proc").join(pid.to_string()).exists())
    }

    fn print(&self, text: &str) {
        println!("{text}");
    }
}

/// Opens the window with a browser.
pub struct Launcher {
    host: Arc<dyn LauncherHost>,
}

impl fmt::Debug for Launcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Launcher")
    }
}

impl Default for Launcher {
    fn default() -> Self {
        Self::new(Arc::new(SystemHost))
    }
}

impl Launcher {
    /// A launcher over this machine.
    pub fn new(host: Arc<dyn LauncherHost>) -> Self {
        Self { host }
    }

    /// Shows `url` in the best browser there is (see the module documentation for the order).
    pub fn show(&self, url: &str) -> Result<(), String> {
        let h = &*self.host;
        if h.env("DISPLAY").is_none() && h.env("WAYLAND_DISPLAY").is_none() {
            return self.print(url, "no desktop session found");
        }
        for name in CHROMIUM_FAMILY {
            if let Some(path) = h.look_path(name) {
                let (w, hgt) = WINDOW_SIZE;
                let args = [
                    format!("--app={url}"),
                    format!("--class={WINDOW_CLASS}"),
                    format!("--window-size={w},{hgt}"),
                    "--no-first-run".to_owned(),
                ];
                return self.started(url, h.start(&path, &args));
            }
        }
        if let Some(path) = h.look_path("firefox") {
            return self.firefox(&path, url);
        }
        if let Some(path) = h.look_path("xdg-open") {
            return self.started(url, h.start(&path, &[url.to_owned()]));
        }
        self.print(url, "no browser found")
    }

    /// Shows `url` in Firefox as an application window: a profile of its own with the tabs and the
    /// address bar hidden. If that profile cannot be prepared the page still opens, in an ordinary
    /// Firefox window.
    fn firefox(&self, path: &Path, url: &str) -> Result<(), String> {
        let h = &*self.host;
        let profile = h.data_dir().join("truesense").join(FIREFOX_PROFILE);
        if write_firefox_profile(&profile).is_err() {
            return self.started(url, h.start(path, &["--new-window".to_owned(), url.to_owned()]));
        }
        let running = h.firefox_running(&profile);
        self.started(url, h.start(path, &firefox_args(&profile, url, running)))
    }

    fn started(&self, url: &str, result: io::Result<()>) -> Result<(), String> {
        match result {
            Ok(()) => Ok(()),
            Err(e) => self.print(url, &format!("could not start the browser: {e}")),
        }
    }

    /// The address is only printed when no browser was started: it carries the one-time code.
    fn print(&self, url: &str, why: &str) -> Result<(), String> {
        self.host.print(&format!("{why}: open this address in a browser:\n{url}"));
        Ok(())
    }
}

impl Opener for Launcher {
    fn open(&self, url: &str) -> Result<(), String> {
        self.show(url)
    }
}

/// The command line that shows `url` in the TrueSense Firefox profile.
///
/// A launch on a profile that is running opens a window in that process; a launch on one that is
/// not must start its own process (`--no-remote`), or it would join the user's browsing session
/// instead.
pub fn firefox_args(profile: &Path, url: &str, running: bool) -> Vec<String> {
    let mut args = vec![
        "--profile".to_owned(),
        profile.display().to_string(),
        "--name".to_owned(),
        WINDOW_CLASS.to_owned(),
    ];
    if !running {
        args.insert(0, "--no-remote".to_owned());
        let (w, h) = WINDOW_SIZE;
        args.extend(["-width".to_owned(), w.to_string(), "-height".to_owned(), h.to_string()]);
    }
    args.extend(["--new-window".to_owned(), url.to_owned()]);
    args
}

/// Creates or refreshes the profile directory with the two files that make it look like an
/// application. The profile is separate from the user's browsing profile (nothing is shared,
/// nothing of the user's is touched) and a file is written again only when it differs.
pub fn write_firefox_profile(dir: &Path) -> Result<(), fsx::FsError> {
    let files = [("user.js", FIREFOX_USER_JS), ("chrome/userChrome.css", FIREFOX_USER_CHROME)];
    for (name, content) in files {
        let path = dir.join(name);
        if fs::read_to_string(&path).is_ok_and(|have| have == content) {
            continue;
        }
        fsx::write_atomic_mkdir(&path, content.as_bytes(), 0o644)?;
    }
    Ok(())
}

/// Whether a Firefox is running on that profile: its lock is a symlink whose target ends in the
/// process id (`127.0.0.1:+1234`). `alive` says whether a process exists.
pub(crate) fn firefox_profile_running(profile: &Path, alive: &dyn Fn(u32) -> bool) -> bool {
    let Ok(target) = fs::read_link(profile.join("lock")) else { return false };
    let target = target.to_string_lossy();
    let Some((_, pid)) = target.rsplit_once('+') else { return false };
    pid.parse::<u32>().is_ok_and(|pid| pid > 0 && alive(pid))
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::gui::testutil::Temp;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// A machine with chosen variables and programs that records every start.
    struct FakeHost {
        env: BTreeMap<&'static str, &'static str>,
        programs: Vec<&'static str>,
        data: PathBuf,
        firefox_running: bool,
        fail_start: bool,
        started: Mutex<Vec<(String, Vec<String>)>>,
        printed: Mutex<Vec<String>>,
    }

    impl FakeHost {
        fn new(data: &Path) -> Self {
            Self {
                env: BTreeMap::from([("WAYLAND_DISPLAY", "wayland-1")]),
                programs: Vec::new(),
                data: data.to_owned(),
                firefox_running: false,
                fail_start: false,
                started: Mutex::default(),
                printed: Mutex::default(),
            }
        }
    }

    impl LauncherHost for FakeHost {
        fn env(&self, key: &str) -> Option<String> {
            self.env.get(key).map(|v| (*v).to_owned())
        }
        fn look_path(&self, program: &str) -> Option<PathBuf> {
            self.programs.contains(&program).then(|| PathBuf::from(format!("/usr/bin/{program}")))
        }
        fn start(&self, program: &Path, args: &[String]) -> io::Result<()> {
            self.started.lock().expect("lock").push((program.display().to_string(), args.to_vec()));
            if self.fail_start { Err(io::Error::other("boom")) } else { Ok(()) }
        }
        fn data_dir(&self) -> PathBuf {
            self.data.clone()
        }
        fn firefox_running(&self, _profile: &Path) -> bool {
            self.firefox_running
        }
        fn print(&self, text: &str) {
            self.printed.lock().expect("lock").push(text.to_owned());
        }
    }

    const URL: &str = "http://127.0.0.1:4000/#code=abc";

    fn open(host: FakeHost) -> (Result<(), String>, Arc<FakeHost>) {
        let host = Arc::new(host);
        (Launcher::new(host.clone()).show(URL), host)
    }

    fn started(h: &FakeHost) -> Vec<(String, Vec<String>)> {
        h.started.lock().expect("lock").clone()
    }

    fn printed(h: &FakeHost) -> Vec<String> {
        h.printed.lock().expect("lock").clone()
    }

    #[test]
    fn without_a_desktop_only_the_address_is_printed() {
        let mut host = FakeHost::new(Path::new("/nonexistent"));
        host.env.clear();
        host.programs = vec!["chromium", "firefox"];
        let (result, h) = open(host);
        assert!(result.is_ok());
        assert!(started(&h).is_empty(), "no browser is started without a desktop session");
        assert_eq!(
            printed(&h),
            [format!("no desktop session found: open this address in a browser:\n{URL}")]
        );
    }

    #[test]
    fn chromium_family_gets_an_app_window_with_the_class() {
        let mut host = FakeHost::new(Path::new("/nonexistent"));
        host.programs = vec!["firefox", "brave", "chromium-browser"];
        let (_, h) = open(host);
        assert_eq!(
            started(&h),
            [(
                "/usr/bin/chromium-browser".to_owned(),
                vec![
                    format!("--app={URL}"),
                    "--class=truesense".to_owned(),
                    "--window-size=1180,820".to_owned(),
                    "--no-first-run".to_owned()
                ]
            )],
            "the first of the family in the list wins over Firefox"
        );
        assert!(printed(&h).is_empty(), "the address carries the code: not printed when a browser started");
    }

    #[test]
    fn firefox_runs_in_its_own_tabless_profile() {
        let t = Temp::new("ff");
        let mut host = FakeHost::new(&t.0);
        host.programs = vec!["firefox", "xdg-open"];
        let (_, h) = open(host);
        let profile = t.0.join("truesense").join("firefox-app");
        assert_eq!(started(&h), [("/usr/bin/firefox".to_owned(), firefox_args(&profile, URL, false))]);
        assert_eq!(fs::read_to_string(profile.join("user.js")).expect("user.js"), FIREFOX_USER_JS);
        let chrome = fs::read_to_string(profile.join("chrome/userChrome.css")).expect("userChrome.css");
        assert_eq!(chrome, FIREFOX_USER_CHROME);
        assert!(chrome.contains("#nav-bar") && chrome.contains("#tabbrowser-tabs"));
        assert!(FIREFOX_USER_JS.contains("toolkit.legacyUserProfileCustomizations.stylesheets\", true"));
    }

    #[test]
    fn firefox_args_start_a_process_of_its_own_unless_the_profile_runs() {
        let p = Path::new("/data/truesense/firefox-app");
        assert_eq!(
            firefox_args(p, URL, false),
            [
                "--no-remote",
                "--profile",
                "/data/truesense/firefox-app",
                "--name",
                "truesense",
                "-width",
                "1180",
                "-height",
                "820",
                "--new-window",
                URL
            ]
        );
        assert_eq!(
            firefox_args(p, URL, true),
            ["--profile", "/data/truesense/firefox-app", "--name", "truesense", "--new-window", URL],
            "a running profile gets the window in its process, with no size to apply"
        );
    }

    #[test]
    fn a_profile_that_cannot_be_written_still_opens_an_ordinary_firefox_window() {
        let t = Temp::new("ffro");
        // The data directory is a file: the profile cannot be created below it.
        let blocker = t.0.join("data");
        fs::write(&blocker, "x").expect("file");
        let mut host = FakeHost::new(&blocker);
        host.programs = vec!["firefox"];
        let (_, h) = open(host);
        assert_eq!(
            started(&h),
            [("/usr/bin/firefox".to_owned(), vec!["--new-window".to_owned(), URL.to_owned()])]
        );
    }

    #[test]
    fn the_profile_files_are_rewritten_only_when_they_differ() {
        let t = Temp::new("ffrw");
        write_firefox_profile(&t.0).expect("first");
        let stamp = fs::metadata(t.0.join("user.js")).expect("meta").modified().expect("mtime");
        write_firefox_profile(&t.0).expect("second");
        assert_eq!(fs::metadata(t.0.join("user.js")).expect("meta").modified().expect("mtime"), stamp);
        fs::write(t.0.join("user.js"), "changed").expect("edit");
        write_firefox_profile(&t.0).expect("third");
        assert_eq!(fs::read_to_string(t.0.join("user.js")).expect("read"), FIREFOX_USER_JS);
    }

    #[test]
    fn xdg_open_is_the_next_choice_and_nothing_found_prints() {
        let mut host = FakeHost::new(Path::new("/nonexistent"));
        host.programs = vec!["xdg-open"];
        let (_, h) = open(host);
        assert_eq!(started(&h), [("/usr/bin/xdg-open".to_owned(), vec![URL.to_owned()])]);

        let (result, h) = open(FakeHost::new(Path::new("/nonexistent")));
        assert!(result.is_ok());
        assert_eq!(printed(&h), [format!("no browser found: open this address in a browser:\n{URL}")]);
    }

    #[test]
    fn a_browser_that_fails_to_start_falls_back_to_printing() {
        let mut host = FakeHost::new(Path::new("/nonexistent"));
        host.programs = vec!["chromium"];
        host.fail_start = true;
        let (result, h) = open(host);
        assert!(result.is_ok());
        assert_eq!(printed(&h).len(), 1);
        assert!(printed(&h)[0].starts_with("could not start the browser: boom"));
    }

    #[test]
    fn a_running_firefox_profile_is_told_from_its_lock_symlink() {
        use std::os::unix::fs::symlink;
        let t = Temp::new("lock");
        let alive = |pid: u32| pid == 1234;
        assert!(!firefox_profile_running(&t.0, &alive), "no lock");
        for (target, running) in [
            ("127.0.0.1:+1234", true),
            ("127.0.0.1:+999", false),
            ("127.0.0.1:+0", false),
            ("127.0.0.1:+abc", false),
            ("nothing", false),
        ] {
            let _ = fs::remove_file(t.0.join("lock"));
            symlink(target, t.0.join("lock")).expect("symlink");
            assert_eq!(firefox_profile_running(&t.0, &alive), running, "{target}");
        }
    }
}
