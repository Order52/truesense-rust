//! The window's server logic: who may ask, and what each request means.
//!
//! This module is pure with respect to the network: it turns a request head and body into a reply
//! (or a stream), so every security rule can be tested without a socket. `http` is the thin layer
//! that feeds it from `tiny_http`.
//!
//! The server can change firmware settings, so it is built to be hard to abuse:
//!
//! * it listens on the loopback address only and refuses any `Host` but its own address, which
//!   defeats DNS rebinding (a page on another name that resolves to 127.0.0.1 still sends its own
//!   name);
//! * it refuses requests a browser labels cross-site (`Sec-Fetch-Site`) or from another `Origin`;
//! * every command needs the session token, compared in constant time; the token is handed to
//!   the page in exchange for a one-time launch code that travels in the URL fragment, so no
//!   URL, command line or log ever holds the token itself;
//! * a command is a `POST` of `application/json`, a type a cross-origin page cannot send without a
//!   preflight that is never answered;
//! * a strict Content-Security-Policy (no inline code, no external request), bounded bodies, a
//!   bounded number of streams, and a timeout for every command.
#![forbid(unsafe_code)]

use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::GuiError;
use super::assets::Assets;
use super::bus::{CommandError, Commands};
use super::hub::{Hub, Message, Subscription};
use super::token::{Entropy, new_token, same_secret};
use ts_foundation::sync::locked;

/// The biggest request body: the largest legitimate one is 128 key colours (a few KB).
pub(crate) const MAX_BODY: usize = 1 << 20;

/// How many live streams may be open: a page needs one, two windows two.
pub(crate) const MAX_STREAMS: usize = 8;

/// How long a window URL stays usable. The URL is on the browser's command line, where other users
/// of the computer can read it, so it carries a code that works once (and soon expires) and not the
/// session token itself.
pub(crate) const LAUNCH_CODE_LIFE: Duration = Duration::from_secs(5 * 60);

/// How many unused launch codes are kept.
const MAX_CODES: usize = 16;

/// Lets the page load only its own scripts and styles and talk only to this server: no inline
/// code, no external request of any kind.
pub(crate) const CONTENT_SECURITY_POLICY: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// The headers every answer carries, whatever it is.
pub(crate) const SECURITY_HEADERS: [(&str, &str); 7] = [
    ("Content-Security-Policy", CONTENT_SECURITY_POLICY),
    ("X-Content-Type-Options", "nosniff"),
    ("Referrer-Policy", "no-referrer"),
    ("Cross-Origin-Opener-Policy", "same-origin"),
    ("Cross-Origin-Resource-Policy", "same-origin"),
    ("Permissions-Policy", "camera=(), microphone=(), geolocation=(), payment=(), usb=()"),
    ("X-Frame-Options", "DENY"),
];

/// Receives start, stop and per-request problems (never the token).
pub type Logger = Arc<dyn Fn(&str) + Send + Sync>;

/// The time source of the server (launch-code expiry, idle time); a test advances it by hand.
pub type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// What the server needs besides its commands.
#[derive(Clone)]
pub(crate) struct Settings {
    /// Shown by `gui ping` and stored in the lock file.
    pub version: String,
    /// The longest a command may take before the page is told it timed out.
    pub command_timeout: Duration,
    /// How often the live reading is taken.
    pub tick: Duration,
    /// How often an idle stream sends a keep-alive.
    pub ping_every: Duration,
    /// Where problems are written.
    pub log: Logger,
    /// The random source for tokens and codes.
    pub entropy: Entropy,
    /// The time source.
    pub clock: Clock,
}

/// The parts of a request the server looks at before it reads a body.
#[derive(Debug, Clone, Default)]
pub(crate) struct Head {
    /// `GET`, `POST`, ...
    pub method: String,
    /// The path without the query string.
    pub path: String,
    /// Header names and values as received.
    pub headers: Vec<(String, String)>,
}

impl Head {
    /// A header by name, ignoring case; the first one if repeated.
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// A complete answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reply {
    /// The HTTP status.
    pub status: u16,
    /// Headers besides the security ones.
    pub headers: Vec<(&'static str, String)>,
    /// The body.
    pub body: Vec<u8>,
}

impl Reply {
    fn json(status: u16, value: &Value) -> Reply {
        let body = serde_json::to_vec(value).unwrap_or_else(|_| b"{\"ok\":false}".to_vec());
        Reply {
            status,
            headers: vec![
                ("Content-Type", "application/json; charset=utf-8".to_owned()),
                ("Cache-Control", "no-store".to_owned()),
            ],
            body,
        }
    }

    fn ok(result: Value) -> Reply {
        Reply::json(200, &json!({"ok": true, "result": result}))
    }

    fn fail(status: u16, message: &str, code: &str) -> Reply {
        let mut error = json!({"message": message});
        if !code.is_empty() {
            error["code"] = json!(code);
        }
        Reply::json(status, &json!({"ok": false, "result": null, "error": error}))
    }

    fn with(mut self, name: &'static str, value: impl Into<String>) -> Reply {
        self.headers.push((name, value.into()));
        self
    }
}

/// What a request turned into.
pub(crate) enum Outcome {
    /// An answer to send.
    Reply(Reply),
    /// The live stream: the caller sends the stream headers and calls [`Server::pump`].
    Stream(StreamTicket),
}

/// The right to hold one stream open; dropping it frees the slot and starts the idle clock when
/// it was the last.
pub(crate) struct StreamTicket {
    server: Arc<Server>,
    sub: Subscription,
}

impl Drop for StreamTicket {
    fn drop(&mut self) {
        if self.server.streams.fetch_sub(1, Ordering::SeqCst) == 1 {
            *locked(&self.server.last_seen) = (self.server.settings.clock)();
        }
    }
}

/// The server state shared by every request.
pub(crate) struct Server {
    addr: String,
    token: String,
    settings: Settings,
    commands: Arc<dyn Commands>,
    assets: Assets,
    hub: Hub,
    streams: AtomicUsize,
    /// When the number of open streams last became zero.
    last_seen: Mutex<Instant>,
    /// Unused launch codes and when they expire.
    codes: Mutex<Vec<(String, Instant)>>,
    quit: AtomicBool,
    closing: AtomicBool,
}

/// `^[a-z][a-z0-9-]{0,31}$`: what an area or a verb may be called.
fn is_command_name(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && s.len() <= 32
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// `^[a-z][a-z-]{0,23}$`: what a page id looks like.
fn is_page_name(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && s.len() <= 24
        && chars.all(|c| c.is_ascii_lowercase() || c == '-')
}

/// Whether a `Content-Type` is `application/json` (parameters such as the charset are allowed).
fn is_json_type(value: Option<&str>) -> bool {
    value.is_some_and(|v| v.split(';').next().unwrap_or("").trim().eq_ignore_ascii_case("application/json"))
}

impl Server {
    /// A server answering at `addr` (`127.0.0.1:port`, the only acceptable `Host`) to whoever holds `token`.
    pub(crate) fn new(
        addr: String,
        token: String,
        commands: Arc<dyn Commands>,
        settings: Settings,
    ) -> Arc<Self> {
        let now = (settings.clock)();
        let hub = Hub::new(Arc::clone(&commands), settings.tick);
        Arc::new(Self {
            addr,
            token,
            commands,
            assets: Assets::embedded(),
            hub,
            streams: AtomicUsize::new(0),
            last_seen: Mutex::new(now),
            codes: Mutex::new(Vec::new()),
            quit: AtomicBool::new(false),
            closing: AtomicBool::new(false),
            settings,
        })
    }

    /// How long no page has had a stream open (zero while one is).
    pub(crate) fn idle_for(&self) -> Duration {
        if self.streams.load(Ordering::SeqCst) > 0 {
            return Duration::ZERO;
        }
        let since = *locked(&self.last_seen);
        (self.settings.clock)().saturating_duration_since(since)
    }

    /// Whether a page asked the server to stop.
    pub(crate) fn quit_requested(&self) -> bool {
        self.quit.load(Ordering::SeqCst)
    }

    /// Whether the server is shutting down (open streams end).
    pub(crate) fn closing(&self) -> bool {
        self.closing.load(Ordering::SeqCst)
    }

    /// Ends the live reading and tells open streams to finish.
    pub(crate) fn close(&self) {
        self.closing.store(true, Ordering::SeqCst);
        self.hub.close();
    }

    fn log(&self, text: &str) {
        (self.settings.log)(text);
    }

    // ---- launch codes

    /// Makes a one-time code that a page exchanges for the session token.
    pub(crate) fn mint_code(&self) -> Result<String, GuiError> {
        let code = new_token(&self.settings.entropy)?;
        let now = (self.settings.clock)();
        let mut codes = locked(&self.codes);
        codes.retain(|(_, expiry)| now <= *expiry);
        while codes.len() >= MAX_CODES {
            codes.remove(0); // the oldest
        }
        codes.push((code.clone(), now + LAUNCH_CODE_LIFE));
        Ok(code)
    }

    /// Uses a code up (whatever happens next) and says whether it was valid. Every stored code is
    /// compared, so the time taken does not tell how close a guess was.
    fn redeem(&self, offered: &str) -> bool {
        let now = (self.settings.clock)();
        let mut codes = locked(&self.codes);
        let mut found = None;
        for (i, (code, expiry)) in codes.iter().enumerate() {
            if same_secret(code, offered) && found.is_none() {
                found = Some((i, *expiry));
            }
        }
        match found {
            Some((i, expiry)) => {
                codes.remove(i);
                now <= expiry
            }
            None => false,
        }
    }

    // ---- requests

    /// Answers one request. The body is read only when the request has passed every check that
    /// does not need it.
    pub(crate) fn handle(self: &Arc<Self>, head: &Head, body: &mut dyn Read) -> Outcome {
        // DNS rebinding: a page on another name that resolves to 127.0.0.1 still sends its own
        // name as Host, so anything but the exact listening address is refused.
        if head.header("Host") != Some(self.addr.as_str()) {
            return Outcome::Reply(Reply::fail(403, "unexpected Host header", "bad-host"));
        }
        if head.method != "GET" && head.method != "POST" {
            return Outcome::Reply(
                Reply::fail(405, "only GET and POST are served", "").with("Allow", "GET, POST"),
            );
        }
        // Cross-site requests: browsers label every request; the page itself is same-origin
        // ("none" is the user typing the address). Clients that send neither header (curl) are
        // not a CSRF vector and still need the token for anything that acts.
        if let Some(site) = head.header("Sec-Fetch-Site") {
            if !site.is_empty() && site != "same-origin" && site != "none" {
                return Outcome::Reply(Reply::fail(403, "cross-site request refused", "cross-site"));
            }
        }
        if let Some(origin) = head.header("Origin") {
            if !origin.is_empty() && origin != format!("http://{}", self.addr) {
                return Outcome::Reply(Reply::fail(403, "cross-origin request refused", "cross-site"));
            }
        }

        if !head.path.starts_with("/api/") {
            return Outcome::Reply(self.asset(head));
        }
        if head.path == "/api/gui/session" {
            return Outcome::Reply(self.exchange(head, body)); // the only call without a token
        }
        if !self.authorised(head) {
            return Outcome::Reply(Reply::fail(401, "missing or wrong token", "unauthorised"));
        }
        self.api(head, body)
    }

    /// An embedded file.
    fn asset(&self, head: &Head) -> Reply {
        if head.method != "GET" {
            return Reply::fail(405, "pages are read with GET", "").with("Allow", "GET");
        }
        let Some(asset) = self.assets.find(&head.path) else {
            return Reply {
                status: 404,
                headers: vec![("Content-Type", "text/plain; charset=utf-8".to_owned())],
                body: b"not found".to_vec(),
            };
        };
        let mut reply = Reply {
            status: 200,
            headers: vec![
                ("Content-Type", asset.content_type.to_owned()),
                ("ETag", asset.etag.clone()),
                // Revalidated with the ETag: a new binary shows its new page.
                ("Cache-Control", "no-cache".to_owned()),
            ],
            body: asset.body.to_vec(),
        };
        if head.header("If-None-Match") == Some(asset.etag.as_str()) {
            reply.status = 304;
            reply.body.clear();
        }
        reply
    }

    fn authorised(&self, head: &Head) -> bool {
        head.header("Authorization")
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|got| same_secret(got, &self.token))
    }

    /// Trades a valid one-time launch code for the session token. The code is used up whatever
    /// happens next, so a copy of the URL taken from the command line is worthless once the real
    /// window has opened.
    fn exchange(&self, head: &Head, body: &mut dyn Read) -> Reply {
        if head.method != "POST" {
            return Reply::fail(405, "send the launch code with POST", "").with("Allow", "POST");
        }
        if !is_json_type(head.header("Content-Type")) {
            return Reply::fail(415, "send application/json", "");
        }
        let mut raw = Vec::new();
        if body.take(4097).read_to_end(&mut raw).is_err() || raw.len() > 4096 {
            return Reply::fail(400, "the body must be {\"code\": ...}", "");
        }
        let code = serde_json::from_slice::<Value>(&raw)
            .ok()
            .and_then(|v| v.get("code").and_then(Value::as_str).map(str::to_owned));
        let Some(code) = code else {
            return Reply::fail(400, "the body must be {\"code\": ...}", "");
        };
        if !self.redeem(&code) {
            return Reply::fail(401, "this window link has been used already or has expired", "unauthorised");
        }
        Reply::ok(json!({"token": self.token}))
    }

    fn api(self: &Arc<Self>, head: &Head, body: &mut dyn Read) -> Outcome {
        let rest = head.path.strip_prefix("/api/").unwrap_or_default();
        if rest == "stream" {
            if head.method != "GET" {
                return Outcome::Reply(
                    Reply::fail(405, "the stream is read with GET", "").with("Allow", "GET"),
                );
            }
            return self.open_stream();
        }
        let Some((area, verb)) = rest.split_once('/') else {
            return Outcome::Reply(Reply::fail(404, "no such command", "unknown-command"));
        };
        if !is_command_name(area) || !is_command_name(verb) {
            return Outcome::Reply(Reply::fail(404, "no such command", "unknown-command"));
        }
        if head.method != "POST" {
            return Outcome::Reply(Reply::fail(405, "commands are sent with POST", "").with("Allow", "POST"));
        }
        // A cross-origin page cannot send application/json without a preflight we never answer.
        if !is_json_type(head.header("Content-Type")) {
            return Outcome::Reply(Reply::fail(415, "send application/json", ""));
        }
        let args = match read_args(body) {
            Ok(args) => args,
            Err(reply) => return Outcome::Reply(reply),
        };
        Outcome::Reply(self.command(area, verb, args))
    }

    /// Runs one command: the few the server answers itself, then the application's.
    fn command(self: &Arc<Self>, area: &str, verb: &str, args: Value) -> Reply {
        match (area, verb) {
            ("gui", "ping") => return Reply::ok(json!({"version": self.settings.version})),
            ("gui", "quit") => {
                self.quit.store(true, Ordering::SeqCst);
                return Reply::ok(json!(true));
            }
            ("gui", "show") => {
                // A second `truesense gui` asks the window that is open to go to a page.
                let page = args.get("page").and_then(Value::as_str).unwrap_or("");
                let heard = self.broadcast_show(page);
                return Reply::ok(json!({"shown": heard > 0, "windows": heard}));
            }
            ("gui", "launch") => {
                return match self.mint_code() {
                    Ok(code) => Reply::ok(json!({"code": code})),
                    Err(_) => Reply::fail(500, "could not make a launch code", ""),
                };
            }
            _ => {}
        }
        match self.run_command(area, verb, args) {
            Ok(result) => Reply::ok(result),
            Err(e) if e.is_unknown() => Reply::fail(404, e.message(), "unknown-command"),
            Err(e) if e.is_timeout() => Reply::fail(504, e.message(), "timeout"),
            Err(e) => {
                self.log(&format!("command {area} {verb} failed: {e}"));
                Reply::fail(422, e.message(), e.code().unwrap_or(""))
            }
        }
    }

    /// Runs the command on its own thread and waits at most the command timeout: a hung hardware
    /// call then ends the request, not the server (the thread cannot be killed and is left to
    /// finish). A panic inside a command is the same as a failure.
    fn run_command(&self, area: &str, verb: &str, args: Value) -> Result<Value, CommandError> {
        let (tx, rx) = mpsc::channel();
        let commands = Arc::clone(&self.commands);
        let (a, v) = (area.to_owned(), verb.to_owned());
        thread::Builder::new()
            .name(format!("gui-cmd-{area}"))
            .spawn(move || {
                let _ = tx.send(commands.run(&a, &v, &args));
            })
            .map_err(|e| CommandError::failed(format!("cannot start the command: {e}")))?;
        match rx.recv_timeout(self.settings.command_timeout) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => Err(CommandError::timeout(area, verb)),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(CommandError::failed("internal error: the command stopped unexpectedly"))
            }
        }
    }

    // ---- show

    /// Tells every open page to go to `page` (home when the name is not a page id) and counts how
    /// many heard it. The name is sent as a JSON string, so it cannot carry markup.
    fn broadcast_show(&self, page: &str) -> usize {
        let page = if is_page_name(page) { page } else { "home" };
        let json = serde_json::to_string(page).unwrap_or_else(|_| "\"home\"".to_owned());
        self.hub.show(&json)
    }

    // ---- stream

    fn open_stream(self: &Arc<Self>) -> Outcome {
        if self.streams.fetch_add(1, Ordering::SeqCst) >= MAX_STREAMS {
            self.streams.fetch_sub(1, Ordering::SeqCst);
            return Outcome::Reply(Reply::fail(503, "too many open windows", ""));
        }
        let sub = self.hub.subscribe();
        Outcome::Stream(StreamTicket { server: Arc::clone(self), sub })
    }

    /// Writes the event stream to `out` until the page goes away (a write fails), the server closes,
    /// or `out` fails. The page reads it with `fetch` and not `EventSource`, which cannot send the
    /// Authorization header.
    pub(crate) fn pump(&self, ticket: &StreamTicket, out: &mut dyn Write) -> io::Result<()> {
        let mut last_write = Instant::now();
        out.write_all(b"retry: 2000\n\n")?;
        out.flush()?;
        loop {
            if self.closing() {
                return Ok(());
            }
            let wait = Duration::from_millis(200).min(self.settings.ping_every);
            match ticket.sub.inbox().next(wait) {
                Some(Message::State(json)) => write!(out, "event: state\ndata: {json}\n\n")?,
                Some(Message::Show(json)) => write!(out, "event: show\ndata: {json}\n\n")?,
                None if last_write.elapsed() >= self.settings.ping_every => out.write_all(b": ping\n\n")?,
                None => continue,
            }
            out.flush()?;
            last_write = Instant::now();
        }
    }
}

/// Reads and checks a command body: a JSON object, `{}` when empty.
fn read_args(body: &mut dyn Read) -> Result<Value, Reply> {
    let mut raw = Vec::new();
    match body.take(MAX_BODY as u64 + 1).read_to_end(&mut raw) {
        Ok(_) if raw.len() > MAX_BODY => return Err(Reply::fail(413, "request body too large", "")),
        Ok(_) => {}
        Err(_) => return Err(Reply::fail(400, "could not read the request body", "")),
    }
    let text = raw.trim_ascii();
    if text.is_empty() {
        return Ok(json!({}));
    }
    match serde_json::from_slice::<Value>(text) {
        Ok(v) if v.is_object() => Ok(v),
        _ => Err(Reply::fail(400, "the body must be a JSON object", "")),
    }
}

/// The headers of the live stream.
pub(crate) fn stream_headers() -> Vec<(&'static str, String)> {
    vec![
        ("Content-Type", "text/event-stream".to_owned()),
        ("Cache-Control", "no-store".to_owned()),
        ("X-Accel-Buffering", "no".to_owned()),
    ]
}
