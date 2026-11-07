//! Starting the window server and keeping it up for as long as a page is open.
#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use super::GuiError;
use super::bus::Commands;
use super::client;
use super::http::{bind_loopback, serve};
use super::instance::{Instance, LockInfo, default_runtime_dir, read_live};
use super::open::{Launcher, Opener};
use super::server::{Clock, Logger, Server, Settings};
use super::token::{Entropy, new_token, system_entropy};

use std::net::SocketAddr;
use url::window_url;

/// How long the server stays up after the last page was closed. A page that never connects counts
/// as closed from the start.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// The longest one command may take (a self-test or an update takes minutes).
pub const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// How often the live state is read while a page is open.
pub const DEFAULT_TICK: Duration = Duration::from_secs(1);

/// How often an idle stream sends a keep-alive so a dead connection is noticed.
pub const DEFAULT_PING: Duration = Duration::from_secs(15);

/// How long a second launch waits for the running server to answer.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(3);

/// What [`run`] needs. [`Options::new`] fills in the production defaults; a test replaces the
/// opener, the runtime directory, the random source and the clock.
pub struct Options {
    /// Runs the page's actions and provides the live state.
    pub commands: Arc<dyn Commands>,
    /// Shows the window (a browser by default).
    pub opener: Arc<dyn Opener>,
    /// Holds the single-instance lock; `None` means the user's runtime directory.
    pub runtime_dir: Option<PathBuf>,
    /// The listen address, which must be on the loopback interface; `None` means `127.0.0.1:0`
    /// (a random port).
    pub addr: Option<String>,
    /// The page the window opens on (`home`, `lighting`, ...); `None` opens home.
    pub page: Option<String>,
    /// How long the server stays up after the last page closed.
    pub idle_timeout: Duration,
    /// The longest one command may take.
    pub command_timeout: Duration,
    /// How often the live state is read.
    pub tick: Duration,
    /// How often an idle stream sends a keep-alive.
    pub ping_every: Duration,
    /// The version shown by `gui ping` and stored in the lock file.
    pub version: String,
    /// Receives start, stop and per-request problems (never the token).
    pub log: Logger,
    /// The random source for the session token and launch codes.
    pub entropy: Entropy,
    /// The time source.
    pub clock: Clock,
}

impl Options {
    /// Production defaults over these commands: a browser app window, the user's runtime
    /// directory, a random loopback port, one minute of idle time.
    pub fn new(commands: Arc<dyn Commands>) -> Self {
        Self {
            commands,
            opener: Arc::new(Launcher::default()),
            runtime_dir: None,
            addr: None,
            page: None,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            command_timeout: DEFAULT_COMMAND_TIMEOUT,
            tick: DEFAULT_TICK,
            ping_every: DEFAULT_PING,
            version: ts_foundation::buildinfo::VERSION.to_owned(),
            log: Arc::new(|_| {}),
            entropy: system_entropy(),
            clock: Arc::new(Instant::now),
        }
    }
}

/// Starts the window server and shows the window. Returns `Ok` when `stop` is set, when the page
/// asks to quit, or when no page has been connected for [`Options::idle_timeout`]; returns
/// [`GuiError::AlreadyRunning`] when this user already has a server (see [`resume`]).
pub fn run(options: Options, stop: &AtomicBool) -> Result<(), GuiError> {
    let dir = match &options.runtime_dir {
        Some(d) => d.clone(),
        None => default_runtime_dir()?,
    };
    let instance = Instance::acquire(&dir)?;
    let listener = bind_loopback(options.addr.as_deref().unwrap_or("127.0.0.1:0"))?;
    let local = listener.local_addr().map_err(|e| GuiError::Listen(e.to_string()))?;
    let token = new_token(&options.entropy)?;
    let settings = Settings {
        version: options.version.clone(),
        command_timeout: options.command_timeout,
        tick: options.tick,
        ping_every: options.ping_every,
        log: Arc::clone(&options.log),
        entropy: Arc::clone(&options.entropy),
        clock: Arc::clone(&options.clock),
    };
    let server = Server::new(local.to_string(), token.clone(), Arc::clone(&options.commands), settings);
    let serving = serve(listener, Arc::clone(&server))?;

    let info =
        LockInfo { port: local.port(), token, pid: std::process::id(), version: options.version.clone() };
    if let Err(e) = instance.publish(&info) {
        serving.shutdown();
        return Err(e);
    }
    (options.log)(&format!("window server listening on {local}"));
    let first_url =
        server.mint_code().map(|code| window_url(local, &code, options.page.as_deref().unwrap_or("")));
    match first_url {
        Ok(url) => {
            if let Err(e) = options.opener.open(&url) {
                (options.log)(&format!("could not open the window: {e}"));
            }
        }
        Err(e) => {
            server.close();
            serving.shutdown();
            return Err(e);
        }
    }

    let poll = (options.idle_timeout / 4).max(Duration::from_millis(10));
    let reason = loop {
        if stop.load(Ordering::SeqCst) || server.quit_requested() {
            break Ok(());
        }
        if !serving.is_running() {
            break Err(GuiError::Io("the server stopped unexpectedly".to_owned()));
        }
        if server.idle_for() >= options.idle_timeout {
            (options.log)("no page for a while: window server stops");
            break Ok(());
        }
        thread::sleep(poll.min(Duration::from_millis(100)));
    };
    server.close();
    serving.shutdown();
    drop(instance);
    reason
}

/// The address of the window server this user already runs, with a fresh one-time code, so a second
/// `truesense gui` can bring the existing window forward (open that address) instead of starting
/// another server. `None` when no server runs.
pub fn resume() -> Option<String> {
    resume_in(&default_runtime_dir().ok()?)
}

/// [`resume`] for a given runtime directory.
pub fn resume_in(runtime_dir: &std::path::Path) -> Option<String> {
    let info = read_live(runtime_dir)?;
    // The session token never leaves the lock file: the running server mints a code for the new window.
    let answer =
        client::post_json(info.port, "/api/gui/launch", Some(&info.token), "{}", ANSWER_TIMEOUT).ok()?;
    if answer.status != 200 {
        return None;
    }
    let value = answer.json()?;
    let code = value.get("result")?.get("code")?.as_str().filter(|c| !c.is_empty())?.to_owned();
    Some(window_url(SocketAddr::from(([127, 0, 0, 1], info.port)), &code, ""))
}

/// Adds the page to a window address that [`run`] or [`resume`] produced (the page names are those of
/// `truesense gui --page`: home, lighting, keys, ...). An unknown name opens home.
pub fn with_page(url: &str, page: &str) -> String {
    url::with_page(url, page)
}

/// The window address: the code is in the fragment, which browsers never send to a server.
mod url {
    use std::net::SocketAddr;

    /// `http://127.0.0.1:port/#code=...&page=...`. The code is URL-safe already; the page is a page
    /// id (letters and dashes), escaped anyway.
    pub(super) fn window_url(addr: SocketAddr, code: &str, page: &str) -> String {
        let mut fragment = format!("code={}", escape(code));
        if !page.is_empty() {
            fragment.push_str(&format!("&page={}", escape(page)));
        }
        format!("http://{addr}/#{fragment}")
    }

    /// Sets (or replaces) the `page` member of the fragment.
    pub(super) fn with_page(url: &str, page: &str) -> String {
        if page.is_empty() {
            return url.to_owned();
        }
        let (base, fragment) = url.split_once('#').unwrap_or((url, ""));
        let mut members: Vec<&str> =
            fragment.split('&').filter(|m| !m.is_empty() && !m.starts_with("page=")).collect();
        let page = format!("page={}", escape(page));
        members.push(&page);
        format!("{base}#{}", members.join("&"))
    }

    /// Percent-escapes everything but unreserved characters.
    fn escape(text: &str) -> String {
        text.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    char::from(b).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_code_travels_in_the_fragment_only() {
            let addr: SocketAddr = ([127, 0, 0, 1], 4242).into();
            assert_eq!(window_url(addr, "abc-_", ""), "http://127.0.0.1:4242/#code=abc-_");
            assert_eq!(window_url(addr, "abc", "curves"), "http://127.0.0.1:4242/#code=abc&page=curves");
            assert!(!window_url(addr, "abc", "x").split('#').next().unwrap_or("").contains("abc"));
        }

        #[test]
        fn a_page_is_added_or_replaced() {
            let url = "http://127.0.0.1:1/#code=abc";
            assert_eq!(with_page(url, "keys"), "http://127.0.0.1:1/#code=abc&page=keys");
            assert_eq!(
                with_page("http://127.0.0.1:1/#code=abc&page=home", "keys"),
                "http://127.0.0.1:1/#code=abc&page=keys"
            );
            assert_eq!(with_page(url, ""), url);
            assert_eq!(with_page("http://127.0.0.1:1/", "keys"), "http://127.0.0.1:1/#page=keys");
            assert_eq!(with_page(url, "a&b=c"), "http://127.0.0.1:1/#code=abc&page=a%26b%3Dc");
        }
    }
}
