//! wire: the control socket protocol, shared by the daemon (server) and every client.
//!
//! One connection carries one request and one reply, each a single line of JSON terminated by
//! `\n`:
//!
//! * request: `{"cmd": "<name>", "args": {...}}` (`args` may be absent or `null`);
//! * reply: `{"ok": true, "result": <any>}` or `{"ok": false, "error": "<text>"}`.
//!
//! This is the format the Python and Go editions already speak, so an old client can talk to a
//! new daemon and the other way round. Everything that parses bytes from the socket lives
//! here and returns an error instead of panicking: the socket is world-connectable, and the
//! daemon runs as root, so a request is untrusted input until the caller's uid has been
//! checked and even then it is only as trustworthy as the process that sent it.
//!
//! Small differences from the Python bytes, none of which a parser can see: the JSON is
//! compact (no space after `,` and `:`, as the Go edition writes it), non-ASCII text is sent
//! as UTF-8 instead of `\uXXXX` escapes, and the keys of an object inside a result come out
//! in sorted order. The order `ok`, then `result` or `error`, is the same as Python's.

use std::ffi::OsStr;
use std::fmt;
use std::io::{self, BufRead, BufReader, Read};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

/// Where the daemon listens unless `TRUESENSE_SOCKET` says otherwise.
pub(crate) const DEFAULT_SOCKET: &str = "/run/truesense/truesense.sock";

/// Environment variable that moves the socket (used by tests and for running a second daemon).
pub const SOCKET_ENV: &str = "TRUESENSE_SOCKET";

/// Largest request or reply in bytes, terminator included. It matches the Python and Go
/// editions (64 KiB): one bound that keeps a hostile or broken peer from making the daemon
/// buffer an unbounded line, and that every edition enforces the same way.
pub const MAX_LINE: usize = 65536;

/// The socket to use given the value of [`SOCKET_ENV`]. Kept apart from the environment read
/// so the rule (an unset or empty value means the default) is testable without touching the
/// process environment.
pub fn socket_path_from(env: Option<&OsStr>) -> PathBuf {
    match env {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => PathBuf::from(DEFAULT_SOCKET),
    }
}

/// The socket of a simulated run (`TRUESENSE_SIM`) when `TRUESENSE_SOCKET` does not name one:
/// `truesense.sock` in the run's throwaway directory. A simulated client must never reach the real
/// daemon (it would change the real fans and power mode), and a simulated daemon must never take
/// the real daemon's place.
pub fn sim_socket_path(temp_dir: &Path) -> PathBuf {
    ts_foundation::userstate::sim_dir(temp_dir).join("truesense.sock")
}

/// The socket for these settings: `TRUESENSE_SOCKET` if set and non-empty; else, under
/// `TRUESENSE_SIM`, [`sim_socket_path`]; else `DEFAULT_SOCKET`.
pub fn socket_path_for(socket: Option<&OsStr>, sim: bool, temp_dir: &Path) -> PathBuf {
    match socket {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ if sim => sim_socket_path(temp_dir),
        _ => PathBuf::from(DEFAULT_SOCKET),
    }
}

/// Whether the process runs on the simulator (`TRUESENSE_SIM` set and non-empty).
pub fn simulated() -> bool {
    std::env::var_os("TRUESENSE_SIM").is_some_and(|v| !v.is_empty())
}

/// The socket to use: `TRUESENSE_SOCKET` if set and non-empty; else the simulated run's own socket
/// under `TRUESENSE_SIM`; else `DEFAULT_SOCKET`.
pub fn socket_path() -> PathBuf {
    socket_path_for(std::env::var_os(SOCKET_ENV).as_deref(), simulated(), &std::env::temp_dir())
}

/// The text of the "no such command" reply. A client recognises an older daemon by this
/// prefix (see `client::ClientError::UnknownCommand`), so the wording is part of the
/// protocol, not a courtesy.
pub(crate) fn unknown_command_message(cmd: &str) -> String {
    format!("{UNKNOWN_COMMAND_PREFIX} '{cmd}'")
}

/// First words of every "no such command" reply, see [`unknown_command_message`].
pub(crate) const UNKNOWN_COMMAND_PREFIX: &str = "unknown command";

/// Why a message could not be read or understood.
#[derive(Debug)]
pub enum WireError {
    /// The line is longer than the limit.
    TooLarge {
        /// The limit that was exceeded, in bytes.
        limit: usize,
    },
    /// The transport failed (or timed out) while reading.
    Io(io::Error),
    /// Nothing but whitespace arrived.
    Empty,
    /// The bytes are not one JSON value (the text is the parser's explanation).
    Json(String),
    /// Valid JSON of the wrong shape (the text says what was expected).
    Shape(String),
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { limit } => write!(f, "message too large (the limit is {limit} bytes)"),
            Self::Io(e) => write!(f, "reading the message: {}", io_message(e)),
            Self::Empty => f.write_str("empty message"),
            Self::Json(e) => write!(f, "not valid JSON: {e}"),
            Self::Shape(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for WireError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

/// A short human text for an I/O error: a timeout reads "timed out" (the OS says "Resource
/// temporarily unavailable" for a socket timeout, which means nothing to a user).
pub(crate) fn io_message(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => "timed out".to_owned(),
        _ => e.to_string(),
    }
}

/// Reads one line of at most `limit` bytes (terminator included) and returns it without the
/// terminator. End of input also ends a line, so a client that sends its request and shuts
/// down its writing side without a newline still works, as it does with the Python server. An
/// empty input returns an empty line.
///
/// Reads at most `limit + 1` bytes from `reader`, so a peer cannot make this allocate more.
pub fn read_line<R: Read>(reader: R, limit: usize) -> Result<Vec<u8>, WireError> {
    let bound = u64::try_from(limit).map_or(u64::MAX, |l| l.saturating_add(1));
    let mut buf = Vec::new();
    BufReader::with_capacity(4096, reader.take(bound)).read_until(b'\n', &mut buf).map_err(WireError::Io)?;
    if buf.len() > limit {
        return Err(WireError::TooLarge { limit });
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
    }
    Ok(buf)
}

/// A reader over a socket whose read timeout covers the whole message, not each `read` call.
///
/// A plain `set_read_timeout` restarts on every byte, so a peer that trickles one byte every
/// few seconds would hold a connection (and its thread) for 64 KiB times the timeout. This
/// recomputes the remaining time before every read.
pub(crate) struct DeadlineReader<'a> {
    stream: &'a UnixStream,
    deadline: Instant,
}

impl<'a> DeadlineReader<'a> {
    pub(crate) fn new(stream: &'a UnixStream, deadline: Instant) -> Self {
        Self { stream, deadline }
    }
}

impl Read for DeadlineReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "deadline reached"));
        }
        self.stream.set_read_timeout(Some(left.max(Duration::from_millis(1))))?;
        (&mut &*self.stream).read(buf)
    }
}

/// Why an argument was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum ArgProblem {
    /// A required argument is absent (or `null`).
    Missing,
    /// Present but not a string.
    NotText,
    /// Present but not an integer (a fractional number is not one; `5.0` is).
    NotWhole,
    /// Present but not a number.
    NotNumber,
    /// Present but not `true` or `false`.
    NotBool,
    /// An integer outside the allowed range.
    OutOfRange {
        /// Smallest allowed value.
        min: i64,
        /// Largest allowed value.
        max: i64,
    },
    /// Any other reason, in words (for example a structure that does not decode).
    Invalid(String),
}

/// A request argument that is missing or of the wrong kind. The message is what the caller
/// sees as the daemon's error, so it names the argument.
#[derive(Debug, Clone, PartialEq)]
pub struct ArgError {
    /// Name of the offending argument.
    pub key: String,
    /// What is wrong with it.
    pub problem: ArgProblem,
}

impl fmt::Display for ArgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let k = &self.key;
        match &self.problem {
            ArgProblem::Missing => write!(f, "argument \"{k}\" is required"),
            ArgProblem::NotText => write!(f, "argument \"{k}\" must be text"),
            ArgProblem::NotWhole => write!(f, "argument \"{k}\" must be a whole number"),
            ArgProblem::NotNumber => write!(f, "argument \"{k}\" must be a number"),
            ArgProblem::NotBool => write!(f, "argument \"{k}\" must be true or false"),
            ArgProblem::OutOfRange { min, max } => {
                write!(f, "argument \"{k}\" must be between {min} and {max}")
            }
            ArgProblem::Invalid(why) => write!(f, "argument \"{k}\": {why}"),
        }
    }
}

impl std::error::Error for ArgError {}

/// Handlers report errors as text; this lets them write `args.require_str("x")?`.
impl From<ArgError> for String {
    fn from(e: ArgError) -> Self {
        e.to_string()
    }
}

/// The `args` object of a request, with typed accessors that report a wrong kind instead of
/// panicking or guessing (the daemon parses untrusted input).
///
/// An argument that is absent and one that is `null` are the same thing: the Python clients
/// send `null` for "not given" (`fan(mode, cpu=None)`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Args(Map<String, Value>);

impl From<Map<String, Value>> for Args {
    fn from(map: Map<String, Value>) -> Self {
        Self(map)
    }
}

impl Args {
    /// No arguments.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an argument, builder style: `Args::new().with("mode", "auto").with("confirm", true)`.
    #[must_use]
    pub fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.0.insert(key.to_owned(), value.into());
        self
    }

    /// Adds or replaces an argument.
    pub fn insert(&mut self, key: &str, value: impl Into<Value>) {
        self.0.insert(key.to_owned(), value.into());
    }

    /// Whether the key is present (even as `null`).
    pub fn has(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    /// The raw value of a key, `None` when absent.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key)
    }

    /// The underlying JSON object.
    pub fn as_map(&self) -> &Map<String, Value> {
        &self.0
    }

    /// The value of a key unless it is absent or `null`.
    fn present(&self, key: &str) -> Option<&Value> {
        self.0.get(key).filter(|v| !v.is_null())
    }

    fn err(key: &str, problem: ArgProblem) -> ArgError {
        ArgError { key: key.to_owned(), problem }
    }

    /// A string argument, `None` when absent or `null`.
    pub fn opt_str(&self, key: &str) -> Result<Option<&str>, ArgError> {
        match self.present(key) {
            None => Ok(None),
            Some(Value::String(s)) => Ok(Some(s)),
            Some(_) => Err(Self::err(key, ArgProblem::NotText)),
        }
    }

    /// An integer argument, `None` when absent or `null`. Accepts `5` and `5.0` (some clients
    /// serialise whole numbers as floats) and refuses `5.5`, a number beyond 64 bits and any
    /// non-number, in particular `true` (a Python `bool` is an `int`, JSON's is not).
    pub fn opt_int(&self, key: &str) -> Result<Option<i64>, ArgError> {
        let Some(v) = self.present(key) else { return Ok(None) };
        let Value::Number(n) = v else { return Err(Self::err(key, ArgProblem::NotWhole)) };
        if let Some(i) = n.as_i64() {
            return Ok(Some(i));
        }
        match n.as_f64() {
            // 2^63 as f64 is exact, so the range test is exact too; `as` then cannot saturate.
            Some(f)
                if n.is_f64()
                    && f.fract() == 0.0
                    && (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&f) =>
            {
                Ok(Some(f as i64))
            }
            _ => Err(Self::err(key, ArgProblem::NotWhole)),
        }
    }

    /// An integer argument that must lie in `min..=max`.
    pub fn opt_int_in(&self, key: &str, min: i64, max: i64) -> Result<Option<i64>, ArgError> {
        match self.opt_int(key)? {
            Some(v) if !(min..=max).contains(&v) => Err(Self::err(key, ArgProblem::OutOfRange { min, max })),
            other => Ok(other),
        }
    }

    /// A number argument (integer or fractional), `None` when absent or `null`.
    pub fn opt_float(&self, key: &str) -> Result<Option<f64>, ArgError> {
        match self.present(key) {
            None => Ok(None),
            Some(Value::Number(n)) => {
                n.as_f64().map(Some).ok_or_else(|| Self::err(key, ArgProblem::NotNumber))
            }
            Some(_) => Err(Self::err(key, ArgProblem::NotNumber)),
        }
    }

    /// A boolean argument, `None` when absent or `null`. Only `true` and `false` qualify: not
    /// `1`, not `"yes"`, because a setting must not flip on a typo.
    pub fn opt_bool(&self, key: &str) -> Result<Option<bool>, ArgError> {
        match self.present(key) {
            None => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(Self::err(key, ArgProblem::NotBool)),
        }
    }

    /// A string argument with a default for absent or `null`.
    pub fn str_or<'a>(&'a self, key: &str, default: &'a str) -> Result<&'a str, ArgError> {
        Ok(self.opt_str(key)?.unwrap_or(default))
    }

    /// An integer argument with a default for absent or `null`.
    pub fn int_or(&self, key: &str, default: i64) -> Result<i64, ArgError> {
        Ok(self.opt_int(key)?.unwrap_or(default))
    }

    /// A number argument with a default for absent or `null`.
    pub fn float_or(&self, key: &str, default: f64) -> Result<f64, ArgError> {
        Ok(self.opt_float(key)?.unwrap_or(default))
    }

    /// A boolean argument with a default for absent or `null`.
    pub fn bool_or(&self, key: &str, default: bool) -> Result<bool, ArgError> {
        Ok(self.opt_bool(key)?.unwrap_or(default))
    }

    /// A string argument that must be given.
    pub fn require_str(&self, key: &str) -> Result<&str, ArgError> {
        self.opt_str(key)?.ok_or_else(|| Self::err(key, ArgProblem::Missing))
    }

    /// An integer argument that must be given.
    pub fn require_int(&self, key: &str) -> Result<i64, ArgError> {
        self.opt_int(key)?.ok_or_else(|| Self::err(key, ArgProblem::Missing))
    }

    /// A number argument that must be given.
    pub fn require_float(&self, key: &str) -> Result<f64, ArgError> {
        self.opt_float(key)?.ok_or_else(|| Self::err(key, ArgProblem::Missing))
    }

    /// A boolean argument that must be given.
    pub fn require_bool(&self, key: &str) -> Result<bool, ArgError> {
        self.opt_bool(key)?.ok_or_else(|| Self::err(key, ArgProblem::Missing))
    }

    /// Decodes a structured argument (a list, an object such as a fan curve) into `T`. The
    /// type's own validation (`serde` rejecting unknown or mistyped fields) is the check.
    pub fn decode<T: DeserializeOwned>(&self, key: &str) -> Result<T, ArgError> {
        let v = self.present(key).ok_or_else(|| Self::err(key, ArgProblem::Missing))?;
        serde_json::from_value(v.clone()).map_err(|e| Self::err(key, ArgProblem::Invalid(e.to_string())))
    }
}

/// One command: `{"cmd": "...", "args": {...}}`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Request {
    /// The command name the daemon dispatches on.
    pub cmd: String,
    /// Its arguments (always sent, as an empty object when there are none, like Python).
    pub args: Args,
}

impl Request {
    /// A request for `cmd` with `args`.
    pub fn new(cmd: &str, args: Args) -> Self {
        Self { cmd: cmd.to_owned(), args }
    }

    /// Parses one request line (without its terminator). Never panics: any byte string gives
    /// a request or an error. `cmd` must be text; `args` must be an object, absent or `null`.
    /// Unknown extra fields are ignored so a newer client can add one.
    pub fn parse(line: &[u8]) -> Result<Self, WireError> {
        let Value::Object(mut obj) = parse_value(line)? else {
            return Err(WireError::Shape("a request must be a JSON object".to_owned()));
        };
        let cmd = match obj.remove("cmd") {
            Some(Value::String(s)) => s,
            _ => return Err(WireError::Shape("\"cmd\" must be text".to_owned())),
        };
        let args = match obj.remove("args") {
            None | Some(Value::Null) => Args::new(),
            Some(Value::Object(map)) => Args(map),
            Some(_) => return Err(WireError::Shape("\"args\" must be an object".to_owned())),
        };
        Ok(Self { cmd, args })
    }

    /// The request as one line with its terminator, refusing what the server would refuse.
    pub fn encode(&self) -> Result<Vec<u8>, WireError> {
        let mut out = serde_json::to_vec(self).map_err(|e| WireError::Json(e.to_string()))?;
        out.push(b'\n');
        if out.len() > MAX_LINE {
            return Err(WireError::TooLarge { limit: MAX_LINE });
        }
        Ok(out)
    }
}

/// Parses `line` as exactly one JSON value (serde_json limits the nesting depth, so a hostile
/// `[[[[...` cannot overflow the stack).
fn parse_value(line: &[u8]) -> Result<Value, WireError> {
    if line.iter().all(u8::is_ascii_whitespace) {
        return Err(WireError::Empty);
    }
    serde_json::from_slice(line).map_err(|e| WireError::Json(e.to_string()))
}

/// The reply to a request.
#[derive(Debug, Clone, PartialEq)]
pub enum Response {
    /// The command worked; the value is its result (`null` when it has none).
    Ok(Value),
    /// The command failed; the text is shown to the user as it is.
    Err(String),
}

#[derive(Serialize)]
struct OkReply<'a> {
    ok: bool,
    result: &'a Value,
}

#[derive(Serialize)]
struct ErrReply<'a> {
    ok: bool,
    error: &'a str,
}

impl Response {
    /// The reply as one line with its terminator. Always a valid reply within [`MAX_LINE`]: a
    /// result that is too large to send becomes an error reply, which tells the caller what
    /// happened (the Python client would otherwise read half a line and report garbage).
    pub fn encode(&self) -> Vec<u8> {
        let encoded = match self {
            Self::Ok(result) => serde_json::to_vec(&OkReply { ok: true, result }),
            Self::Err(error) => serde_json::to_vec(&ErrReply { ok: false, error }),
        };
        match encoded {
            Ok(mut out) if out.len() < MAX_LINE => {
                out.push(b'\n');
                out
            }
            Ok(_) => error_line("internal error: the reply is larger than the 64 KiB limit"),
            Err(e) => error_line(&format!("internal error: reply is not encodable: {e}")),
        }
    }

    /// Parses a reply line (without its terminator). `ok` must be exactly `true` for success;
    /// anything else is a failure whose text is `error` (or "daemon error" when there is none,
    /// as the Python client says).
    pub fn parse(line: &[u8]) -> Result<Self, WireError> {
        let Value::Object(mut obj) = parse_value(line)? else {
            return Err(WireError::Shape("a reply must be a JSON object".to_owned()));
        };
        if obj.get("ok") == Some(&Value::Bool(true)) {
            return Ok(Self::Ok(obj.remove("result").unwrap_or(Value::Null)));
        }
        Ok(Self::Err(match obj.remove("error") {
            Some(Value::String(s)) => s,
            _ => "daemon error".to_owned(),
        }))
    }
}

/// An error reply as a line, built by hand so that it cannot itself fail to encode.
fn error_line(message: &str) -> Vec<u8> {
    let mut out = serde_json::to_vec(&ErrReply { ok: false, error: message })
        .unwrap_or_else(|_| br#"{"ok":false,"error":"internal error"}"#.to_vec());
    out.push(b'\n');
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;

    fn args(v: Value) -> Args {
        match v {
            Value::Object(m) => Args(m),
            _ => Args::new(),
        }
    }

    #[test]
    fn socket_path_rules() {
        assert_eq!(socket_path_from(None), PathBuf::from(DEFAULT_SOCKET));
        assert_eq!(socket_path_from(Some(OsStr::new(""))), PathBuf::from(DEFAULT_SOCKET));
        assert_eq!(socket_path_from(Some(OsStr::new("/tmp/x.sock"))), PathBuf::from("/tmp/x.sock"));
    }

    #[test]
    fn a_simulated_run_never_falls_back_to_the_real_socket() {
        let tmp = Path::new("/t");
        assert_eq!(socket_path_for(None, false, tmp), PathBuf::from(DEFAULT_SOCKET));
        let sim = socket_path_for(None, true, tmp);
        assert_ne!(sim, PathBuf::from(DEFAULT_SOCKET));
        assert!(sim.starts_with("/t"), "{}", sim.display());
        assert_eq!(socket_path_for(Some(OsStr::new("")), true, tmp), sim);
        // An explicit socket still wins (a test daemon of its own).
        assert_eq!(socket_path_for(Some(OsStr::new("/x.sock")), true, tmp), PathBuf::from("/x.sock"));
    }

    #[test]
    fn request_round_trip_and_bytes() {
        let r =
            Request::new("fan", Args::new().with("mode", "manual").with("cpu", 50).with("gpu", Value::Null));
        let line = r.encode().unwrap_or_default();
        assert_eq!(line, b"{\"cmd\":\"fan\",\"args\":{\"mode\":\"manual\",\"cpu\":50,\"gpu\":null}}\n");
        assert_eq!(Request::parse(&line).ok(), Some(r));
        let no_args = Request::new("ping", Args::new()).encode().unwrap_or_default();
        assert_eq!(no_args, b"{\"cmd\":\"ping\",\"args\":{}}\n");
    }

    #[test]
    fn python_formatted_request_parses() {
        // json.dumps({"cmd": "set", "args": {"key": "smart_fan", "value": False}}) from the Python client
        let line = br#"{"cmd": "set", "args": {"key": "smart_fan", "value": false}}"#;
        let r = Request::parse(line).unwrap_or_else(|e| Request::new(&e.to_string(), Args::new()));
        assert_eq!(r.cmd, "set");
        assert_eq!(r.args.require_str("key"), Ok("smart_fan"));
        assert_eq!(r.args.require_bool("value"), Ok(false));
        // non-ASCII escaped the way Python's ensure_ascii does it, including a surrogate pair
        let r = Request::parse(br#"{"cmd": "x\u00e9\ud83d\ude00"}"#).ok();
        assert_eq!(r.map(|r| r.cmd), Some("x\u{e9}\u{1F600}".to_owned()));
    }

    #[test]
    fn request_parse_table() {
        let ok_cmd = |line: &str| Request::parse(line.as_bytes()).ok().map(|r| r.cmd);
        assert_eq!(ok_cmd(r#"{"cmd":"a"}"#), Some("a".to_owned()));
        assert_eq!(ok_cmd(r#"{"cmd":"a","args":null}"#), Some("a".to_owned()));
        assert_eq!(ok_cmd(r#"  {"cmd":"a","extra":[1,2]}  "#), Some("a".to_owned()));
        assert_eq!(ok_cmd("{\"cmd\":\"a\"}\r"), Some("a".to_owned()));
        assert_eq!(
            ok_cmd(r#"{"cmd":"a","cmd":"b"}"#),
            Some("b".to_owned()),
            "last duplicate wins, as in Python"
        );
        // hostile or broken, each must be an error
        let deep_array = "[".repeat(100_000);
        let deep_object = r#"{"a":"#.repeat(100_000);
        let deep_args = format!(r#"{{"cmd":"a","args":{}{}}}"#, r#"{"a":"#.repeat(5000), "1");
        let bad: Vec<(&str, &[u8])> = vec![
            ("empty", b""),
            ("spaces", b"   \t "),
            ("not json", b"not json"),
            ("truncated", br#"{"cmd":"a""#),
            ("trailing garbage", br#"{"cmd":"a"} x"#),
            ("two values", br#"{"cmd":"a"}{"cmd":"b"}"#),
            ("array", b"[]"),
            ("string", br#""cmd""#),
            ("number", b"5"),
            ("null", b"null"),
            ("no cmd", br#"{"args":{}}"#),
            ("cmd number", br#"{"cmd":5}"#),
            ("cmd null", br#"{"cmd":null}"#),
            ("cmd list", br#"{"cmd":["a"]}"#),
            ("args array", br#"{"cmd":"a","args":[1]}"#),
            ("args string", br#"{"cmd":"a","args":"x"}"#),
            ("args number", br#"{"cmd":"a","args":1}"#),
            ("invalid utf8", b"{\"cmd\":\"\xff\xfe\"}"),
            ("lone surrogate", br#"{"cmd":"\ud800"}"#),
            ("nul bytes", b"\0\0\0"),
            ("NaN", br#"{"cmd":"a","args":{"x":NaN}}"#),
            ("deep array", deep_array.as_bytes()),
            ("deep object", deep_object.as_bytes()),
            ("deep args", deep_args.as_bytes()),
            ("bad escape", br#"{"cmd":"\q"}"#),
            ("control char in string", b"{\"cmd\":\"a\nb\"}"),
        ];
        for (name, line) in bad {
            assert!(Request::parse(line).is_err(), "{name} must be refused");
        }
    }

    #[test]
    fn request_parse_never_panics_on_mutations() {
        // every prefix and every single-byte corruption of valid requests, plus pseudo-random bytes
        let seeds: [&[u8]; 3] = [
            br#"{"cmd":"fan_curve","args":{"action":"use","name":"x","data":{"cpu":[[40,20],[95,100]],"s":"\u00e9"}}}"#,
            br#"{"cmd":"a","args":{"n":-1.5e300,"b":[true,false,null,[[]]]}}"#,
            b"{\"cmd\":\"\xe2\x82\xac\"}",
        ];
        let mut count = 0u32;
        for seed in seeds {
            for end in 0..=seed.len() {
                let _ = Request::parse(&seed[..end]);
                count += 1;
            }
            for i in 0..seed.len() {
                for b in [0u8, b'"', b'{', b'}', b'\\', 0xff, b'[', b'\n'] {
                    let mut m = seed.to_vec();
                    m[i] = b;
                    let _ = Request::parse(&m);
                    let _ = Response::parse(&m);
                    count += 1;
                }
            }
        }
        let mut x = 0x2545_F491_4F6C_DD1Du64;
        for _ in 0..3000 {
            let mut next = || {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x
            };
            let len = usize::try_from(next() % 200).unwrap_or(0);
            let bytes: Vec<u8> = (0..len).map(|_| u8::try_from(next() % 256).unwrap_or(0)).collect();
            let _ = Request::parse(&bytes);
            let _ = Response::parse(&bytes);
            count += 1;
        }
        assert!(count > 3000);
    }

    #[test]
    fn response_encode_bytes_and_parse() {
        assert_eq!(Response::Ok(json!("pong")).encode(), b"{\"ok\":true,\"result\":\"pong\"}\n");
        assert_eq!(Response::Ok(Value::Null).encode(), b"{\"ok\":true,\"result\":null}\n");
        assert_eq!(Response::Err("no".into()).encode(), b"{\"ok\":false,\"error\":\"no\"}\n");
        for r in [Response::Ok(json!({"a": [1, 2.5, null]})), Response::Err("bad \"quote\"\n".into())] {
            let mut line = r.encode();
            assert_eq!(line.pop(), Some(b'\n'));
            assert!(!line.contains(&b'\n'), "one line");
            assert_eq!(Response::parse(&line).ok(), Some(r));
        }
        let parse = |s: &str| Response::parse(s.as_bytes()).ok();
        // what the Python daemon writes
        assert_eq!(
            parse(r#"{"ok": true, "result": {"live": {"ac": true}}}"#),
            Some(Response::Ok(json!({"live": {"ac": true}})))
        );
        assert_eq!(parse(r#"{"ok": true}"#), Some(Response::Ok(Value::Null)));
        assert_eq!(
            parse(r#"{"ok": false, "error": "uid 1 is not allowed"}"#),
            Some(Response::Err("uid 1 is not allowed".into()))
        );
        assert_eq!(parse(r#"{"ok": false}"#), Some(Response::Err("daemon error".into())));
        assert_eq!(parse(r#"{"ok": false, "error": 5}"#), Some(Response::Err("daemon error".into())));
        assert_eq!(parse(r#"{}"#), Some(Response::Err("daemon error".into())));
        assert_eq!(
            parse(r#"{"ok": 1, "result": 2}"#),
            Some(Response::Err("daemon error".into())),
            "only true is success"
        );
        for bad in ["", "[]", "nope", "{\"ok\":true", "\"ok\""] {
            assert!(Response::parse(bad.as_bytes()).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn oversized_reply_becomes_an_error_reply() {
        let big = Response::Ok(json!("x".repeat(MAX_LINE))).encode();
        assert!(big.len() < 200 && big.ends_with(b"\n"));
        assert!(
            matches!(Response::parse(&big[..big.len() - 1]), Ok(Response::Err(m)) if m.contains("64 KiB"))
        );
        let huge = Request::new("a", Args::new().with("s", "x".repeat(MAX_LINE)));
        assert!(matches!(huge.encode(), Err(WireError::TooLarge { .. })));
        // just inside the limit
        let fits = Response::Ok(json!("x".repeat(MAX_LINE - 40))).encode();
        assert!(fits.len() <= MAX_LINE && fits.starts_with(b"{\"ok\":true"));
    }

    #[test]
    fn read_line_limits() {
        let line = |input: &[u8], limit| read_line(Cursor::new(input.to_vec()), limit).ok();
        assert_eq!(line(b"abc\n", 10), Some(b"abc".to_vec()));
        assert_eq!(line(b"abc", 10), Some(b"abc".to_vec()), "end of input ends the line");
        assert_eq!(line(b"", 10), Some(Vec::new()));
        assert_eq!(line(b"abc\ndef\n", 10), Some(b"abc".to_vec()), "one message per connection");
        assert_eq!(line(b"abcdefghi\n", 10), Some(b"abcdefghi".to_vec()), "terminator counts: 10 bytes fit");
        assert_eq!(line(b"abcdefghij\n", 10), None, "11 bytes do not");
        assert_eq!(line(b"abcdefghijk", 10), None);
        let endless = std::io::repeat(b'a');
        assert!(
            matches!(read_line(endless, MAX_LINE), Err(WireError::TooLarge { limit: MAX_LINE })),
            "an endless stream is cut off"
        );
        let big = vec![b'a'; MAX_LINE + 10];
        assert!(matches!(read_line(Cursor::new(big), MAX_LINE), Err(WireError::TooLarge { .. })));
    }

    #[test]
    fn read_line_reports_transport_errors() {
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::WouldBlock))
            }
        }
        let e = read_line(Broken, 10).err().map(|e| e.to_string());
        assert_eq!(e.as_deref(), Some("reading the message: timed out"));
    }

    #[test]
    fn typed_getters() {
        let a = args(
            json!({"s": "x", "n": 5, "nf": 5.0, "frac": 1.5, "big": 1e300, "huge": 18446744073709551615u64,
            "neg": -3, "b": true, "list": [1, 2], "nul": null, "t": 2.5}),
        );
        assert_eq!(a.opt_str("s"), Ok(Some("x")));
        assert_eq!(a.opt_str("nul"), Ok(None));
        assert_eq!(a.opt_str("missing"), Ok(None));
        assert_eq!(a.str_or("missing", "dflt"), Ok("dflt"));
        assert_eq!(a.str_or("nul", "dflt"), Ok("dflt"));
        assert_eq!(a.opt_int("n"), Ok(Some(5)));
        assert_eq!(a.opt_int("nf"), Ok(Some(5)));
        assert_eq!(a.opt_int("neg"), Ok(Some(-3)));
        assert_eq!(a.int_or("missing", 7), Ok(7));
        assert_eq!(a.opt_float("t"), Ok(Some(2.5)));
        assert_eq!(a.opt_float("n"), Ok(Some(5.0)));
        assert_eq!(a.float_or("missing", 1.5), Ok(1.5));
        assert_eq!(a.opt_bool("b"), Ok(Some(true)));
        assert_eq!(a.bool_or("missing", false), Ok(false));
        assert_eq!(a.opt_int_in("n", 0, 10), Ok(Some(5)));
        assert_eq!(a.opt_int_in("missing", 0, 10), Ok(None));
        // refusals: (result, message)
        let msg = |e: ArgError| e.to_string();
        assert_eq!(a.opt_str("n").map_err(msg), Err("argument \"n\" must be text".into()));
        assert_eq!(a.opt_int("s").map_err(msg), Err("argument \"s\" must be a whole number".into()));
        assert_eq!(a.opt_int("frac").map_err(msg), Err("argument \"frac\" must be a whole number".into()));
        assert_eq!(a.opt_int("big").map_err(msg), Err("argument \"big\" must be a whole number".into()));
        assert_eq!(a.opt_int("huge").map_err(msg), Err("argument \"huge\" must be a whole number".into()));
        assert_eq!(
            a.opt_int("b").map_err(msg),
            Err("argument \"b\" must be a whole number".into()),
            "true is not 1"
        );
        assert_eq!(a.opt_int("list").map_err(msg), Err("argument \"list\" must be a whole number".into()));
        assert_eq!(a.opt_float("s").map_err(msg), Err("argument \"s\" must be a number".into()));
        assert_eq!(a.opt_float("b").map_err(msg), Err("argument \"b\" must be a number".into()));
        assert_eq!(a.opt_bool("s").map_err(msg), Err("argument \"s\" must be true or false".into()));
        assert_eq!(
            a.opt_bool("n").map_err(msg),
            Err("argument \"n\" must be true or false".into()),
            "1 is not true"
        );
        assert_eq!(
            a.opt_int_in("n", 6, 10).map_err(msg),
            Err("argument \"n\" must be between 6 and 10".into())
        );
        assert_eq!(a.require_str("missing").map_err(msg), Err("argument \"missing\" is required".into()));
        assert_eq!(a.require_int("nul").map_err(msg), Err("argument \"nul\" is required".into()));
        assert_eq!(a.require_float("missing").map_err(msg), Err("argument \"missing\" is required".into()));
        assert_eq!(a.require_bool("missing").map_err(msg), Err("argument \"missing\" is required".into()));
        assert_eq!(a.require_int("n"), Ok(5));
        assert_eq!(a.require_float("t"), Ok(2.5));
        assert_eq!(a.require_bool("b"), Ok(true));
        assert!(a.has("nul") && !a.has("missing"));
    }

    #[test]
    fn integer_edges() {
        let a = args(
            json!({"max": i64::MAX, "min": i64::MIN, "f_min": -9223372036854775808.0, "f_over": 9223372036854775808.0}),
        );
        assert_eq!(a.opt_int("max"), Ok(Some(i64::MAX)));
        assert_eq!(a.opt_int("min"), Ok(Some(i64::MIN)));
        assert_eq!(a.opt_int("f_min"), Ok(Some(i64::MIN)));
        assert!(a.opt_int("f_over").is_err());
    }

    #[test]
    fn decode_structured_argument() {
        let a = args(json!({"list": [1, 2], "curve": {"cpu": [[40, 20]]}, "bad": "x"}));
        assert_eq!(a.decode::<Vec<u8>>("list"), Ok(vec![1, 2]));
        assert_eq!(
            a.decode::<std::collections::BTreeMap<String, Vec<(u8, u8)>>>("curve").map(|m| m.len()),
            Ok(1)
        );
        assert_eq!(
            a.decode::<Vec<u8>>("missing").map_err(|e| e.to_string()),
            Err("argument \"missing\" is required".into())
        );
        let e = a.decode::<Vec<u8>>("bad").map_err(|e| e.to_string());
        assert!(matches!(e, Err(m) if m.starts_with("argument \"bad\": ")));
        let e = a.decode::<Vec<u8>>("list").and_then(|_| a.decode::<Vec<u8>>("bad"));
        assert!(e.is_err());
    }

    #[test]
    fn arg_errors_convert_to_handler_errors() {
        fn handler(a: &Args) -> Result<Value, String> {
            let n = a.require_int("n")?;
            Ok(json!(n))
        }
        assert_eq!(handler(&Args::new().with("n", 2)), Ok(json!(2)));
        assert_eq!(handler(&Args::new()), Err("argument \"n\" is required".to_owned()));
    }

    #[test]
    fn deadline_reader_times_out_on_a_silent_peer() {
        let (a, _b) = UnixStream::pair().unwrap();
        let started = Instant::now();
        let r = read_line(DeadlineReader::new(&a, Instant::now() + Duration::from_millis(50)), 100);
        assert!(matches!(r, Err(WireError::Io(_))));
        assert!(started.elapsed() < Duration::from_secs(2));
        let r = read_line(DeadlineReader::new(&a, Instant::now()), 100);
        assert!(matches!(r, Err(WireError::Io(_))), "an expired deadline reads nothing");
    }

    #[test]
    fn deadline_covers_a_trickling_peer() {
        use std::io::Write;
        let (a, mut b) = UnixStream::pair().unwrap();
        // The peer trickles a byte every 20 ms for as long as the reader keeps listening (a limit of
        // 100 bytes would end it after 2 s, so the line limit is never what stops the reader).
        let writer = std::thread::spawn(move || {
            for _ in 0..1000 {
                if b.write_all(b"a").is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let started = Instant::now();
        let r = read_line(DeadlineReader::new(&a, Instant::now() + Duration::from_millis(150)), 10_000);
        assert!(matches!(r, Err(WireError::Io(_))), "a byte every 20 ms must not keep the reader alive");
        // Cut off by the deadline, long before the trickle (20 s) or any limit could end it.
        assert!(started.elapsed() < Duration::from_secs(10));
        drop(a);
        assert!(writer.join().is_ok());
    }
}
