//! The window's command bus.
//!
//! Everything the web page does is one command, `POST /api/{area}/{verb}` with a JSON object, and
//! its live values arrive on a stream. [`Commands`] is the seam: the server forwards each request
//! to it and knows nothing of firmware. [`ApiCommands`] is the real implementation, written on
//! `ts_api::api::Api` only, so the window, the command line and the terminal UI share every
//! operation and every safety rule (a fan stop, an undervolt or a power limit beyond the danger
//! line is refused by the application layer unless the arguments carry the confirmation, however
//! the page asked the user first: the page is not trusted to be the only guard). `COMMANDS.md`
//! next to this module is the specification, and a test fails when it, the front end and this
//! table disagree.
#![forbid(unsafe_code)]

mod alerts;
mod args;
mod favourites;
mod hardware;
mod info;
mod keys;
mod light;
mod modebind;
mod power;
mod system;

use std::fmt;
use std::sync::Arc;

use serde::Serialize;
use serde_json::{Map, Value, json};
use ts_api::api::{Api, ApiError};
use ts_ipc::client::ClientError;

pub use system::{Hooks, SelfTest, ServiceLogs, StartupControl};

use args::Args;

/// Everything the window needs from the application. The page never talks to the hardware or the
/// daemon itself: each of its actions is one [`Commands::run`] (area and verb are the names of the
/// command line, see `COMMANDS.md`), and the live values come from [`Commands::live`].
/// Implementations run as the user who started the window, so the daemon's own uid check still
/// decides what is allowed.
pub trait Commands: Send + Sync + 'static {
    /// Executes one command. `args` is the JSON object the page sent (`{}` when it sent nothing).
    /// The result must be JSON. A failure whose [`CommandError::is_unknown`] is true answers 404,
    /// any other failure 422 with its message, which is shown to the person as it is.
    fn run(&self, area: &str, verb: &str, args: &Value) -> Result<Value, CommandError>;

    /// One reading of the live state, about once a second while a page is connected, so it may
    /// do real hardware reads but must stay cheap. `None` when it could not be read this time.
    fn live(&self) -> Option<Value>;
}

/// Why a command failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandError {
    kind: Kind,
    message: String,
    code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Unknown,
    Failed,
    Timeout,
}

impl CommandError {
    /// A failure with a message fit to show a person (write it like the command line's `error:` line).
    pub fn failed(message: impl Into<String>) -> Self {
        Self { kind: Kind::Failed, message: message.into(), code: None }
    }

    /// The (area, verb) pair does not exist.
    pub fn unknown(area: &str, verb: &str) -> Self {
        Self { kind: Kind::Unknown, message: format!("unknown command: {area} {verb}"), code: None }
    }

    /// The command did not finish in time.
    pub(crate) fn timeout(area: &str, verb: &str) -> Self {
        Self { kind: Kind::Timeout, message: format!("{area} {verb} took too long"), code: None }
    }

    /// Adds a short machine-readable code (`needs-daemon`) the page can use to explain what to do.
    #[must_use]
    pub fn with_code(mut self, code: &str) -> Self {
        self.code = Some(code.to_owned());
        self
    }

    /// The text to show.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The machine-readable code, if any.
    pub fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }

    /// Whether the command does not exist.
    pub fn is_unknown(&self) -> bool {
        self.kind == Kind::Unknown
    }

    /// Whether the command timed out.
    pub fn is_timeout(&self) -> bool {
        self.kind == Kind::Timeout
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CommandError {}

impl From<ApiError> for CommandError {
    /// The application layer's wording is kept; the service being absent or refusing the user gets
    /// a code so the page can say what to do.
    fn from(e: ApiError) -> Self {
        let code = match &e {
            ApiError::Daemon(ClientError::NotRunning) => Some("needs-daemon"),
            ApiError::Daemon(ClientError::PermissionDenied) => Some("no-permission"),
            _ => None,
        };
        let error = Self::failed(e.to_string());
        match code {
            Some(c) => error.with_code(c),
            None => error,
        }
    }
}

type Outcome = Result<Value, CommandError>;

/// Turns "not mine" (`Ok(None)`) into `None`, so the next area gets its turn.
pub(crate) fn lift(r: Result<Option<Value>, CommandError>) -> Option<Outcome> {
    r.transpose()
}

/// The result of a handler as JSON.
pub(crate) fn val<T: Serialize>(v: T) -> Outcome {
    serde_json::to_value(v).map_err(|e| CommandError::failed(format!("the result cannot be encoded: {e}")))
}

/// A profile as JSON with `"builtin"` added (the page shows built-in profiles read-only).
pub(crate) fn with_builtin<T: Serialize>(profile: T, builtin: bool) -> Outcome {
    let mut object = match val(profile)? {
        Value::Object(o) => o,
        other => {
            let mut o = Map::new();
            o.insert("value".to_owned(), other);
            o
        }
    };
    object.insert("builtin".to_owned(), json!(builtin));
    Ok(Value::Object(object))
}

/// A list of names as a JSON array (never `null`).
pub(crate) fn names(list: impl IntoIterator<Item = String>) -> Value {
    Value::Array(list.into_iter().map(Value::String).collect())
}

/// The shared inputs of every handler.
pub(crate) struct Ctx<'a> {
    pub api: &'a Api,
    pub hooks: &'a Hooks,
}

/// [`Commands`] over the application layer.
pub struct ApiCommands {
    api: Arc<Api>,
    hooks: Hooks,
}

impl ApiCommands {
    /// The window's commands over this application layer, with the few things only the binary can
    /// provide (self-test, journal, start-up apply).
    pub fn new(api: Arc<Api>, hooks: Hooks) -> Self {
        Self { api, hooks }
    }
}

impl Commands for ApiCommands {
    fn run(&self, area: &str, verb: &str, args: &Value) -> Result<Value, CommandError> {
        let ctx = Ctx { api: &self.api, hooks: &self.hooks };
        let a = Args::new(args)?;
        let key = format!("{area}/{verb}");
        let done = power::dispatch(&ctx, &key, &a)
            .or_else(|| hardware::dispatch(&ctx, &key, &a))
            .or_else(|| light::dispatch(&ctx, &key, &a))
            .or_else(|| keys::dispatch(&ctx, &key, &a))
            .or_else(|| modebind::dispatch(&ctx, &key, &a))
            .or_else(|| system::dispatch(&ctx, &key, &a))
            .or_else(|| info::dispatch(&ctx, &key, &a))
            .or_else(|| alerts::dispatch(&ctx, &key, &a))
            .or_else(|| favourites::dispatch(&ctx, &key, &a));
        done.unwrap_or_else(|| Err(CommandError::unknown(area, verb)))
    }

    fn live(&self) -> Option<Value> {
        serde_json::to_value(self.api.live()).ok()
    }
}
