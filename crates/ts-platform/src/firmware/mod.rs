//! firmware: the only door to the laptop's firmware.
//!
//! It speaks the text protocol of the `truesense_wmi` kernel module (`/dev/truesense`, see
//! `docs/PROTOCOL.md` and `kernel/truesense-wmi/truesense_wmi.c`): one command line is written,
//! one reply line (`ok key=value ...` or `err message`) is read back from the same open file.
//! Nothing above this module knows whether it talks to hardware: [`KernelDevice`] is the real
//! device, [`Simulator`] is an in-memory firmware (the `TRUESENSE_SIM=1` semantics) that rejects
//! exactly what the kernel rejects, and [`ReadOnly`] is a guard that lets only reads through (the
//! probe path for laptops nobody has verified).

mod kernel;
mod reply;
mod sim;

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

pub use kernel::{Channel, KernelDevice, Opener};
pub use reply::Reply;
pub use sim::{SimFan, Simulator, Snapshot};

/// Where the kernel module's character device lives (`TRUESENSE_DEVICE` overrides it).
pub const DEVICE_PATH: &str = "/dev/truesense";

/// The longest command the kernel accepts: its write buffer is 96 bytes and a command of 96 bytes
/// or more is refused with `EINVAL` before any reply is produced.
pub const MAX_COMMAND_LEN: usize = 95;

/// Every command of the module that only reads. The module whitelists its commands; this is the
/// subset that can never change a setting, and the only thing [`ReadOnly`] lets through.
pub const READ_ONLY_COMMANDS: [&str; 16] = [
    "version",
    "caps",
    "sensors",
    "sensors_temps",
    "get_mode",
    "get_modes",
    "get_gpu",
    "get_boot",
    "get_misc",
    "get_overdrive",
    "get_fan",
    "get_rear",
    "get_slot",
    "get_usb",
    "get_kbtimeout",
    "get_battery",
];

/// A failure reported by, or on the way to, the firmware. The text of every variant is what the
/// Python edition showed, so messages reach the user unchanged.
#[derive(Debug)]
pub enum FirmwareError {
    /// The device node does not exist: the kernel module is not loaded.
    NotLoaded {
        /// The device that was opened.
        path: PathBuf,
    },
    /// The device exists but this user may not open it.
    PermissionDenied {
        /// The device that was opened.
        path: PathBuf,
    },
    /// The module answered `err ...`: the command or its argument was refused.
    Rejected {
        /// The command that was sent (context for logs; the display text is the reason alone).
        command: String,
        /// The module's own message.
        reason: String,
    },
    /// The reply was not understood (neither `ok` nor `err`), or a field was missing or invalid.
    BadReply {
        /// What was wrong, with the offending text.
        detail: String,
    },
    /// Talking to the device failed at the operating-system level.
    Io {
        /// The operation that failed, for example the command being sent.
        operation: String,
        /// The operating-system error.
        source: io::Error,
    },
    /// The command was not sent: the guard is read-only and this command may change a setting, or
    /// the command is not a well-formed single line.
    Refused {
        /// The command that was refused.
        command: String,
    },
}

impl fmt::Display for FirmwareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotLoaded { path } => write!(
                f,
                "{} not found: the truesense_wmi kernel module is not loaded (sudo modprobe truesense_wmi, or run: truesense doctor)",
                path.display()
            ),
            Self::PermissionDenied { path } => write!(
                f,
                "no permission on {} (log in on the laptop's seat, or use sudo; see: truesense doctor)",
                path.display()
            ),
            Self::Rejected { reason, .. } => f.write_str(reason),
            Self::BadReply { detail } => f.write_str(detail),
            Self::Io { operation, source } => write!(f, "{operation}: {source}"),
            Self::Refused { command } => write!(f, "'{command}' was not sent: not a read-only command"),
        }
    }
}

impl std::error::Error for FirmwareError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// One firmware, real or simulated. Implementations are safe for concurrent use: commands from
/// several threads are serialised, exactly as the kernel serialises them.
pub trait Device: Send + Sync {
    /// Sends one command line and returns the raw reply line. A command the module rejects is not
    /// an error here: its `err ...` reply is returned like any other, and [`Device::execute`]
    /// turns it into [`FirmwareError::Rejected`]. Only a failure to talk to the device is an `Err`.
    fn run(&self, command: &str) -> Result<String, FirmwareError>;

    /// Sends one command and parses the reply: the normal way to use a device.
    fn execute(&self, command: &str) -> Result<Reply, FirmwareError> {
        Reply::parse(command, &self.run(command)?)
    }
}

impl<D: Device + ?Sized> Device for Arc<D> {
    fn run(&self, command: &str) -> Result<String, FirmwareError> {
        (**self).run(command)
    }
}

/// Refuses every command whose name is not in [`READ_ONLY_COMMANDS`], before it reaches the
/// inner device. The probe of an unverified laptop runs behind this guard so that "nothing is
/// written" holds by construction, not by the care of the code above.
pub struct ReadOnly<'a> {
    inner: &'a dyn Device,
}

impl<'a> ReadOnly<'a> {
    /// Wraps `inner`; only reads will reach it.
    pub fn new(inner: &'a dyn Device) -> Self {
        Self { inner }
    }

    /// Whether `command` is a single printable line whose name is a read-only command. Control
    /// characters are refused outright: the kernel stops at a NUL and trims other whitespace, so
    /// a command that looks like a read to one parser could be a write to another.
    pub fn allows(command: &str) -> bool {
        if command.bytes().any(|b| b.is_ascii_control()) {
            return false;
        }
        command.split_whitespace().next().is_some_and(|name| READ_ONLY_COMMANDS.contains(&name))
    }
}

impl Device for ReadOnly<'_> {
    fn run(&self, command: &str) -> Result<String, FirmwareError> {
        if !Self::allows(command) {
            return Err(FirmwareError::Refused { command: command.to_string() });
        }
        self.inner.run(command)
    }
}

/// Checks the length rule the kernel enforces on every write. The kernel drops a command of 0 or
/// 96 or more bytes with `EINVAL` and produces no reply at all, which a client that keeps the
/// device open could mistake for a stale file; so both device implementations answer such a
/// command themselves, with the `err` reply a refused command would have produced.
pub(crate) fn length_reply(command: &str) -> Option<String> {
    (command.is_empty() || command.len() > MAX_COMMAND_LEN)
        .then(|| format!("err a command is 1 to {MAX_COMMAND_LEN} bytes\n"))
}

/// Which firmware a process should talk to, decided from the environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceKind {
    /// The in-memory simulator (`TRUESENSE_SIM` set to anything non-empty, as in the Python edition).
    Simulator,
    /// The kernel device at this path (`TRUESENSE_DEVICE`, default [`DEVICE_PATH`]).
    Kernel(PathBuf),
}

impl DeviceKind {
    /// Decides from `lookup`, which maps an environment variable name to its value. Taking a
    /// lookup instead of reading the process environment keeps the decision testable.
    pub fn from_env(lookup: &dyn Fn(&str) -> Option<String>) -> Self {
        if lookup("TRUESENSE_SIM").is_some_and(|v| !v.is_empty()) {
            return Self::Simulator;
        }
        let path = lookup("TRUESENSE_DEVICE").filter(|v| !v.is_empty());
        Self::Kernel(PathBuf::from(path.unwrap_or_else(|| DEVICE_PATH.to_string())))
    }

    /// Decides from the real process environment.
    pub fn from_process_env() -> Self {
        Self::from_env(&|k| std::env::var(k).ok())
    }

    /// Creates the device. Call it once per process and share the result: the simulator keeps its
    /// state inside the value, like real hardware keeps it in the firmware.
    pub fn open(self) -> Arc<dyn Device> {
        match self {
            Self::Simulator => Arc::new(Simulator::new()),
            Self::Kernel(path) => Arc::new(KernelDevice::new(path)),
        }
    }
}

#[cfg(test)]
mod tests;
