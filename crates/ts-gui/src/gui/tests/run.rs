//! The server's life: start, show the window, serve, forward a second launch, stop.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::json;

use super::harness::Scripted;
use super::stream::Stream;
use crate::gui::client;
use crate::gui::instance::{INFO_NAME, LOCK_NAME, read_live};
use crate::gui::open::{Launcher, LauncherHost, Opener};
use crate::gui::testutil::Temp;
use crate::gui::{
    DesktopTools, GuiError, Options, Raiser, SystemWindow, resume_in, run, show_in, show_window, with_page,
};

/// Records the address the window would be opened with.
#[derive(Default)]
struct Recorder {
    urls: Mutex<Vec<String>>,
    fail: AtomicBool,
}

impl Opener for Recorder {
    fn open(&self, url: &str) -> Result<(), String> {
        self.urls.lock().expect("urls").push(url.to_owned());
        if self.fail.load(Ordering::SeqCst) { Err("no browser".to_owned()) } else { Ok(()) }
    }
}

struct Running {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<(), GuiError>>>,
    opener: Arc<Recorder>,
    log: Arc<Mutex<Vec<String>>>,
    commands: Arc<Scripted>,
    dir: PathBuf,
    _temp: Option<Temp>,
}

impl Running {
    fn start() -> Running {
        let temp = Temp::new("run");
        let dir = temp.0.clone();
        Self::start_in(dir, Some(temp), |_| {})
    }

    fn start_with(tweak: impl FnOnce(&mut Options)) -> Running {
        let temp = Temp::new("run");
        let dir = temp.0.clone();
        Self::start_in(dir, Some(temp), tweak)
    }

    fn start_in(dir: PathBuf, temp: Option<Temp>, tweak: impl FnOnce(&mut Options)) -> Running {
        let commands = Arc::new(Scripted::default());
        let opener = Arc::new(Recorder::default());
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut options = Options::new(commands.clone());
        options.opener = opener.clone();
        options.runtime_dir = Some(dir.clone());
        options.tick = Duration::from_millis(10);
        options.ping_every = Duration::from_millis(150);
        options.version = "1.2.3".to_owned();
        let sink = Arc::clone(&log);
        options.log = Arc::new(move |line| sink.lock().expect("log").push(line.to_owned()));
        tweak(&mut options);
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = thread::spawn(move || run(options, &flag));
        Running { stop, thread: Some(thread), opener, log, commands, dir, _temp: temp }
    }

    /// The address the window was opened with, once it has been.
    fn url(&self) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(u) = self.opener.urls.lock().expect("urls").first() {
                return u.clone();
            }
            assert!(
                Instant::now() < deadline,
                "the window was never opened; log: {:?}",
                self.log.lock().expect("log")
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn port(&self) -> u16 {
        let url = self.url();
        let host = url.strip_prefix("http://127.0.0.1:").expect("loopback address");
        host.split('/').next().expect("port").parse().expect("number")
    }

    fn code(&self) -> String {
        let url = self.url();
        let fragment = url.split_once('#').expect("fragment").1.to_owned();
        let code = fragment.split('&').find_map(|m| m.strip_prefix("code=")).expect("code");
        code.to_owned()
    }

    fn token(&self) -> String {
        let answer = client::post_json(
            self.port(),
            "/api/gui/session",
            None,
            &json!({ "code": self.code() }).to_string(),
            Duration::from_secs(5),
        )
        .expect("exchange");
        assert_eq!(answer.status, 200, "{}", answer.text());
        answer.json().expect("json")["result"]["token"].as_str().expect("token").to_owned()
    }

    fn finish(mut self) -> Result<(), GuiError> {
        self.stop.store(true, Ordering::SeqCst);
        self.join()
    }

    fn join(&mut self) -> Result<(), GuiError> {
        let t = self.thread.take().expect("running");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !t.is_finished() {
            assert!(Instant::now() < deadline, "the server did not stop");
            thread::sleep(Duration::from_millis(5));
        }
        t.join().expect("the server thread did not panic")
    }

    fn logged(&self) -> String {
        self.log.lock().expect("log").join("\n")
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[test]
fn the_window_is_opened_with_a_one_time_code_that_buys_the_session_token() {
    let r = Running::start();
    let url = r.url();
    assert!(url.starts_with("http://127.0.0.1:"), "{url}");
    let (base, fragment) = url.split_once('#').expect("the code is in the fragment");
    assert!(base.ends_with('/') && !base.contains("code"), "nothing secret before the #: {base}");
    assert!(fragment.starts_with("code="));
    assert!(!fragment.contains("page="), "no page asked for");

    let page = client::send(
        r.port(),
        &client::Request { method: "GET", path: "/", ..client::Request::default() },
        Duration::from_secs(5),
    )
    .expect("page");
    assert_eq!(page.status, 200);

    let token = r.token();
    assert_ne!(token, r.code(), "the code is not the token");
    assert_eq!(token.len(), 43);
    let echo =
        client::post_json(r.port(), "/api/test/echo", Some(&token), "{\"a\":1}", Duration::from_secs(5))
            .expect("echo");
    assert_eq!(echo.json().expect("json")["result"], json!({"a": 1}));
    let again = client::post_json(
        r.port(),
        "/api/gui/session",
        None,
        &json!({ "code": r.code() }).to_string(),
        Duration::from_secs(5),
    )
    .expect("again");
    assert_eq!(again.status, 401, "the code was used up by the window");
    let ping = client::post_json(r.port(), "/api/gui/ping", Some(&token), "{}", Duration::from_secs(5))
        .expect("ping");
    assert_eq!(ping.json().expect("json")["result"]["version"], json!("1.2.3"));

    let (dir, log) = (r.dir.clone(), r.logged());
    assert!(!log.contains(&token) && !log.contains(&r.code()), "neither secret is logged: {log}");
    assert!(log.contains("listening on 127.0.0.1:"), "{log}");
    assert!(r.finish().is_ok());
    assert_eq!(read_live(&dir), None, "the token is removed when the server stops");
    assert!(!Path::new(&dir).join(INFO_NAME).exists());
}

#[test]
fn the_page_to_open_on_is_part_of_the_address() {
    let r = Running::start_with(|o| o.page = Some("curves".to_owned()));
    let fragment = r.url().split_once('#').expect("fragment").1.to_owned();
    assert!(fragment.split('&').any(|m| m == "page=curves"), "{fragment}");
    assert!(fragment.split('&').any(|m| m.starts_with("code=")));
    assert_eq!(with_page(&r.url(), "keys"), r.url().replace("page=curves", "page=keys"));
}

#[test]
fn a_second_server_for_the_same_user_is_refused_and_the_first_keeps_running() {
    let r = Running::start();
    let _ = r.url();
    let commands = Arc::new(Scripted::default());
    let mut options = Options::new(commands);
    let opener = Arc::new(Recorder::default());
    options.opener = opener.clone();
    options.runtime_dir = Some(r.dir.clone());
    let stop = AtomicBool::new(false);
    assert_eq!(run(options, &stop), Err(GuiError::AlreadyRunning));
    assert!(opener.urls.lock().expect("urls").is_empty(), "no second window");
    assert_eq!(
        client::post_json(r.port(), "/api/gui/ping", Some(&r.token()), "{}", Duration::from_secs(5))
            .expect("ping")
            .status,
        200
    );
    assert_eq!(GuiError::AlreadyRunning.to_string(), "the TrueSense window is already running");
}

#[test]
fn a_second_launch_resumes_the_running_window_with_a_fresh_code() {
    let r = Running::start();
    let first = r.url();
    assert!(resume_in(&r.dir).is_some());
    let resumed = resume_in(&r.dir).expect("a window runs");
    assert!(resumed.starts_with(first.split_once('#').expect("fragment").0), "the same server: {resumed}");
    assert_ne!(resumed, first, "each launch has its own code");
    let code = resumed.split_once("#code=").expect("code").1.to_owned();
    assert!(!code.contains('&'));
    let exchanged = client::post_json(
        r.port(),
        "/api/gui/session",
        None,
        &json!({ "code": code }).to_string(),
        Duration::from_secs(5),
    )
    .expect("exchange");
    assert_eq!(exchanged.status, 200);
    // The code the first window was opened with is still its own.
    assert_eq!(r.token().len(), 43);
}

#[test]
fn nothing_to_resume_when_no_server_runs_or_only_a_leftover_file_remains() {
    let t = Temp::new("noresume");
    assert_eq!(resume_in(&t.0), None);
    assert!(!show_in(&t.0, "home"));
    std::fs::write(t.0.join(LOCK_NAME), b"").expect("lock");
    std::fs::write(t.0.join(INFO_NAME), json!({"port": 1, "token": "x", "pid": 1}).to_string())
        .expect("info");
    assert_eq!(resume_in(&t.0), None, "a lock file nobody holds is not a server");
    assert!(!show_in(&t.0, "home"));

    // The server is gone but its files are claimed: the port refuses the connection.
    let r = Running::start();
    let (dir, port) = (r.dir.clone(), r.port());
    let held = crate::gui::instance::Instance::acquire(&t.0).expect("lock");
    held.publish(&crate::gui::instance::LockInfo {
        port: 1,
        token: "x".to_owned(),
        pid: 1,
        version: String::new(),
    })
    .expect("publish");
    assert_eq!(resume_in(&t.0), None, "nothing listens on the port");
    assert!(!show_in(&t.0, "home"));
    drop((held, port, dir));
}

#[test]
fn show_asks_the_open_window_to_go_to_a_page() {
    let r = Running::start();
    assert!(!show_in(&r.dir, "curves"), "a server without a page open: the caller must open a window");
    let token = r.token();
    let mut stream = Stream::open_port(r.port(), Some(&token));
    assert_eq!(stream.status(), 200);
    assert!(stream.event().is_some());
    assert!(show_in(&r.dir, "curves"));
    loop {
        let (event, data) = stream.event().expect("event");
        if event == "show" {
            assert_eq!(data, "\"curves\"");
            break;
        }
    }
}

#[test]
fn the_server_stops_a_while_after_the_last_page_closed() {
    let r = Running::start_with(|o| o.idle_timeout = Duration::from_millis(300));
    let mut r = r;
    let token = r.token();
    let mut stream = Stream::open_port(r.port(), Some(&token));
    assert!(stream.event().is_some());
    thread::sleep(Duration::from_millis(700));
    assert!(
        r.thread.as_ref().is_some_and(|t| !t.is_finished()),
        "a page is open: it stays up past the idle time"
    );
    drop(stream);
    assert!(r.join().is_ok());
    assert!(r.logged().contains("no page for a while"), "{}", r.logged());
}

#[test]
fn a_window_that_never_connects_counts_as_closed_from_the_start() {
    let mut r = Running::start_with(|o| o.idle_timeout = Duration::from_millis(250));
    let started = Instant::now();
    assert!(r.join().is_ok());
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn the_page_can_stop_the_server_and_a_stop_flag_does_too() {
    let mut r = Running::start();
    let token = r.token();
    let quit = client::post_json(r.port(), "/api/gui/quit", Some(&token), "{}", Duration::from_secs(5))
        .expect("quit");
    assert_eq!(quit.json().expect("json")["result"], json!(true));
    assert!(r.join().is_ok());
    assert_eq!(read_live(&r.dir), None);

    let r = Running::start();
    let _ = r.url();
    assert!(r.finish().is_ok(), "a signal handler sets the flag");
}

#[test]
fn a_failed_open_does_not_stop_the_server() {
    let r = Running::start_with(|_| {});
    let _ = r.url();
    drop(r);
    let t = Temp::new("openfail");
    let commands = Arc::new(Scripted::default());
    let opener = Arc::new(Recorder::default());
    opener.fail.store(true, Ordering::SeqCst);
    let log = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut options = Options::new(commands);
    options.opener = opener.clone();
    options.runtime_dir = Some(t.0.clone());
    options.idle_timeout = Duration::from_millis(200);
    let sink = Arc::clone(&log);
    options.log = Arc::new(move |l| sink.lock().expect("log").push(l.to_owned()));
    let stop = AtomicBool::new(false);
    assert!(run(options, &stop).is_ok());
    let log = log.lock().expect("log").join("\n");
    assert!(log.contains("could not open the window: no browser"), "{log}");
}

#[test]
fn the_server_never_listens_beyond_this_computer() {
    for addr in
        ["0.0.0.0:0", "192.168.1.5:8080", "[::]:0", "8.8.8.8:80", "localhost:0", "127.0.0.1", "nonsense", ""]
    {
        let t = Temp::new("listen");
        let mut options = Options::new(Arc::new(Scripted::default()));
        options.opener = Arc::new(Recorder::default());
        options.runtime_dir = Some(t.0.clone());
        options.addr = Some(addr.to_owned());
        let stop = AtomicBool::new(false);
        assert!(matches!(run(options, &stop), Err(GuiError::Listen(_))), "{addr:?}");
        // The lock was given back: a failed start leaves nothing behind.
        assert_eq!(read_live(&t.0), None);
        assert!(crate::gui::instance::Instance::acquire(&t.0).is_ok(), "{addr:?} left the lock held");
    }
}

#[test]
fn the_ipv6_loopback_address_is_acceptable_to_the_bind_check() {
    assert!(crate::gui::http::bind_loopback("127.0.0.1:0").is_ok());
    assert!(crate::gui::http::bind_loopback("127.0.0.5:0").is_ok(), "all of 127/8 is this computer");
    assert!(
        crate::gui::http::bind_loopback("[::1]:0").is_ok() || std::net::TcpListener::bind("[::1]:0").is_err()
    );
}

#[test]
fn live_readings_flow_only_while_a_page_listens() {
    let r = Running::start();
    let token = r.token();
    thread::sleep(Duration::from_millis(100));
    assert_eq!(r.commands.reads.load(Ordering::SeqCst), 0, "nobody listens: the hardware is not read");
    let mut stream = Stream::open_port(r.port(), Some(&token));
    assert!(stream.event().is_some());
    assert!(r.commands.reads.load(Ordering::SeqCst) >= 1);
}

#[test]
fn the_defaults_are_the_production_ones() {
    let o = Options::new(Arc::new(Scripted::default()));
    assert_eq!(o.idle_timeout, Duration::from_secs(60), "one minute after the last page closed");
    assert_eq!(o.command_timeout, Duration::from_secs(600));
    assert_eq!(o.tick, Duration::from_secs(1));
    assert_eq!(o.version, ts_foundation::buildinfo::VERSION);
    assert!(o.addr.is_none() && o.page.is_none() && o.runtime_dir.is_none());
}

// ---- a second `truesense gui` against the real server

/// A desktop whose compositor always finds and focuses the window.
struct FindsTheWindow;

impl DesktopTools for FindsTheWindow {
    fn env(&self, key: &str) -> Option<String> {
        (key == "SWAYSOCK").then(|| "/run/sway".to_owned())
    }

    fn have(&self, program: &str) -> bool {
        program == "swaymsg"
    }

    fn run(&self, _program: &str, _args: &[&str]) -> Result<String, String> {
        Ok("[{ \"success\": true }]".to_owned())
    }
}

/// A machine with one browser that records how it is started.
struct OneBrowser {
    started: Mutex<Vec<Vec<String>>>,
}

impl LauncherHost for OneBrowser {
    fn env(&self, key: &str) -> Option<String> {
        (key == "WAYLAND_DISPLAY").then(|| "wayland-1".to_owned())
    }

    fn look_path(&self, program: &str) -> Option<PathBuf> {
        (program == "chromium").then(|| PathBuf::from("/usr/bin/chromium"))
    }

    fn start(&self, _program: &Path, args: &[String]) -> std::io::Result<()> {
        self.started.lock().expect("started").push(args.to_vec());
        Ok(())
    }

    fn data_dir(&self) -> PathBuf {
        PathBuf::from("/nonexistent")
    }

    fn firefox_running(&self, _profile: &Path) -> bool {
        false
    }

    fn print(&self, _text: &str) {}
}

fn second_launch(dir: &Path, raises: bool) -> (SystemWindow, Arc<OneBrowser>) {
    let browser = Arc::new(OneBrowser { started: Mutex::default() });
    let raiser =
        if raises { Raiser::new(Arc::new(FindsTheWindow)) } else { Raiser::new(Arc::new(NoDesktop)) };
    let window =
        SystemWindow { runtime_dir: Some(dir.to_owned()), raiser, launcher: Launcher::new(browser.clone()) };
    (window, browser)
}

/// A desktop with no tool that could raise anything.
struct NoDesktop;

impl DesktopTools for NoDesktop {
    fn env(&self, _key: &str) -> Option<String> {
        None
    }

    fn have(&self, _program: &str) -> bool {
        false
    }

    fn run(&self, _program: &str, _args: &[&str]) -> Result<String, String> {
        Err("nothing to run".to_owned())
    }
}

#[test]
fn a_second_launch_shows_the_page_in_the_open_window_and_raises_it() {
    let r = Running::start();
    let token = r.token();
    let mut stream = Stream::open_port(r.port(), Some(&token));
    assert!(stream.event().is_some());
    let (window, browser) = second_launch(&r.dir, true);
    let options = Options::new(Arc::new(Scripted::default()));
    show_window(&window, options, "curves", &AtomicBool::new(false)).expect("forwarded");
    assert!(browser.started.lock().expect("started").is_empty(), "no second window");
    loop {
        let (event, data) = stream.event().expect("event");
        if event == "show" {
            assert_eq!(data, "\"curves\"");
            break;
        }
    }
}

#[test]
fn a_second_launch_that_cannot_raise_opens_one_more_window_on_the_same_server() {
    let r = Running::start();
    let token = r.token();
    let mut stream = Stream::open_port(r.port(), Some(&token));
    assert!(stream.event().is_some());
    let (window, browser) = second_launch(&r.dir, false);
    let options = Options::new(Arc::new(Scripted::default()));
    show_window(&window, options, "keys", &AtomicBool::new(false)).expect("reopened");
    let started = browser.started.lock().expect("started").clone();
    assert_eq!(started.len(), 1);
    let app = started[0][0].strip_prefix("--app=").expect("an app window").to_owned();
    assert!(app.starts_with(&format!("http://127.0.0.1:{}/#code=", r.port())), "{app}");
    assert!(app.ends_with("&page=keys"), "{app}");
    // The address works: its code buys the session token of the running server.
    let code = app.split_once("#code=").expect("code").1.split('&').next().expect("code").to_owned();
    let answer = client::post_json(
        r.port(),
        "/api/gui/session",
        None,
        &json!({ "code": code }).to_string(),
        Duration::from_secs(5),
    )
    .expect("exchange");
    assert_eq!(answer.status, 200);
}
