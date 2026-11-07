//! The control socket as the application layer sees it.
//!
//! The daemon (root) owns the privileged and persistent settings; every client asks it through
//! one [`Daemon`] value. It is a trait, not the socket client itself, so that the same code runs
//! against the real socket, a test double, or a daemon that is "not running".
#![forbid(unsafe_code)]

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use ts_ipc::client::{Client, ClientError};
use ts_ipc::wire::Args;

use super::{Api, ApiError, ApiResult};

/// The daemon's control socket. `Client` is the real one.
pub trait Daemon: Send + Sync {
    /// Whether the socket exists. It says nothing about whether anything listens; it is the cheap
    /// test the fast path starts with (the Python edition's `ipc.available()`).
    fn available(&self) -> bool;

    /// Sends one command and returns its result.
    fn call(&self, cmd: &str, args: Args) -> Result<Value, ClientError>;
}

impl Daemon for Client {
    fn available(&self) -> bool {
        Client::available(self)
    }

    fn call(&self, cmd: &str, args: Args) -> Result<Value, ClientError> {
        Client::call(self, cmd, args)
    }
}

/// The service's answer to `state`: the live readings, the settings that survive reboots and how
/// long a power-mode pick is still held. The maps are kept as the daemon sent them, so `--json`
/// prints exactly what Python printed and a newer daemon's extra keys are not lost.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DaemonState {
    /// What the daemon measured last second (`tcpu`, `rpm1`, `power_mode`, `ac`, ...).
    pub live: Map<String, Value>,
    /// The switches it runs by (`smart_fan`, `auto_mode`, `fan_curve`, ...).
    pub settings: Map<String, Value>,
    /// Seconds left of the hold that keeps a hand-picked power mode against the automatic one.
    pub hold_left_s: i64,
}

impl DaemonState {
    /// A boolean setting; `None` when absent or of another type.
    pub fn setting_bool(&self, key: &str) -> Option<bool> {
        self.settings.get(key).and_then(Value::as_bool)
    }

    /// A text setting; `None` when absent or of another type.
    pub fn setting_str(&self, key: &str) -> Option<&str> {
        self.settings.get(key).and_then(Value::as_str)
    }

    /// A live number; `None` when absent or not a number.
    pub fn live_f64(&self, key: &str) -> Option<f64> {
        self.live.get(key).and_then(Value::as_f64)
    }

    /// A live text; `None` when absent or of another type.
    pub fn live_str(&self, key: &str) -> Option<&str> {
        self.live.get(key).and_then(Value::as_str)
    }

    /// A live flag; `None` when absent or of another type.
    pub fn live_bool(&self, key: &str) -> Option<bool> {
        self.live.get(key).and_then(Value::as_bool)
    }
}

impl Api {
    /// Whether the service's socket exists.
    pub fn daemon_available(&self) -> bool {
        self.s.daemon.available()
    }

    /// Whether the service answers `ping` with `pong` (the check behind "the service runs").
    pub fn daemon_responds(&self) -> bool {
        matches!(self.s.daemon.call("ping", Args::new()), Ok(Value::String(s)) if s == "pong")
    }

    /// Asks the daemon and returns the raw result.
    pub(crate) fn ask_value(&self, cmd: &str, args: Args) -> ApiResult<Value> {
        Ok(self.s.daemon.call(cmd, args)?)
    }

    /// Asks the daemon and decodes the result; a result of another shape is
    /// [`ClientError::UnexpectedReply`], not a panic.
    pub(crate) fn ask<T: DeserializeOwned>(&self, cmd: &str, args: Args) -> ApiResult<T> {
        let value = self.ask_value(cmd, args)?;
        serde_json::from_value(value).map_err(|e| {
            ApiError::Daemon(ClientError::UnexpectedReply { cmd: cmd.to_owned(), reason: e.to_string() })
        })
    }

    /// The service's state. Without a service the error says how to start it.
    pub fn daemon_state(&self) -> ApiResult<DaemonState> {
        self.ask("state", Args::new())
    }

    /// Changes one of the service's switches (`smart_fan`, `auto_profile`, `operating_follow`,
    /// `auto_mode`) and returns the settings the service now holds.
    pub fn daemon_set(&self, key: &str, value: impl Into<Value>) -> ApiResult<Map<String, Value>> {
        self.ask("set", Args::new().with("key", key).with("value", value))
    }

    /// A switch of the service as a boolean (false when the service does not list it).
    pub(crate) fn daemon_bool(&self, key: &str) -> ApiResult<bool> {
        Ok(self.daemon_state()?.setting_bool(key).unwrap_or(false))
    }

    /// Sets a switch of the service and answers the value the service reports back.
    pub(crate) fn daemon_set_bool(&self, key: &str, on: bool) -> ApiResult<bool> {
        let settings = self.daemon_set(key, on)?;
        settings.get(key).and_then(Value::as_bool).ok_or_else(|| {
            ApiError::Daemon(ClientError::UnexpectedReply {
                cmd: "set".to_owned(),
                reason: format!("the service did not report {key}"),
            })
        })
    }
}
