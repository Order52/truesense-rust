//! The real device: `/dev/truesense`.

use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::Mutex;

use super::{Device, FirmwareError, length_reply};

/// The size of the read buffer: the kernel keeps at most 255 bytes of reply per open file.
const REPLY_BUFFER: usize = 512;

/// Something a command can be written to and its reply read from: an open `/dev/truesense`, or a
/// scripted stand-in in tests.
pub trait Channel: Read + Write + Send {}

impl<T: Read + Write + Send> Channel for T {}

/// Opens the device channel; called again when the kept channel goes stale.
pub type Opener = Box<dyn Fn() -> io::Result<Box<dyn Channel>> + Send + Sync>;

/// The kernel module's device. The kernel gives every open file its own reply buffer, so
/// several processes (daemon, CLI, windows) can use the device at the same time. This client
/// keeps ONE open file per process, which saves an open and a close per command (the daemon
/// polls the sensors every second), and reopens it once if it turns out to be stale. A lock
/// serialises the write-then-read pair, because the two calls together are one command.
pub struct KernelDevice {
    path: PathBuf,
    opener: Opener,
    channel: Mutex<Option<Box<dyn Channel>>>,
}

/// How one write-then-read exchange ended.
enum Exchange {
    /// The raw reply line (an `err ...` reply is a reply too).
    Reply(String),
    /// The file did not behave like a live device file: reopen and try once more.
    Stale(io::Error),
}

impl KernelDevice {
    /// A device at `path` (normally [`super::DEVICE_PATH`]). Nothing is opened until the first command.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let target = path.clone();
        Self::with_opener(
            path,
            Box::new(move || {
                let file = OpenOptions::new().read(true).write(true).open(&target)?;
                Ok(Box::new(file) as Box<dyn Channel>)
            }),
        )
    }

    /// A device whose channel comes from `opener`; `path` is only used in error messages.
    /// Used by tests to script the device without touching `/dev`.
    pub fn with_opener(path: impl Into<PathBuf>, opener: Opener) -> Self {
        Self { path: path.into(), opener, channel: Mutex::new(None) }
    }

    fn open(&self) -> Result<Box<dyn Channel>, FirmwareError> {
        (self.opener)().map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => FirmwareError::NotLoaded { path: self.path.clone() },
            io::ErrorKind::PermissionDenied => FirmwareError::PermissionDenied { path: self.path.clone() },
            _ => FirmwareError::Io { operation: format!("open {}", self.path.display()), source: e },
        })
    }
}

impl Device for KernelDevice {
    fn run(&self, command: &str) -> Result<String, FirmwareError> {
        if let Some(refusal) = length_reply(command) {
            return Ok(refusal);
        }
        // A poisoned lock only means another thread panicked between a write and its read; the
        // channel is then in an unknown state, so drop it and start from a fresh open.
        let mut guard = self.channel.lock().unwrap_or_else(|poisoned| {
            let mut guard = poisoned.into_inner();
            *guard = None;
            guard
        });
        for attempt in 0..2 {
            let fresh = guard.is_none();
            if fresh {
                *guard = Some(self.open()?);
            }
            let Some(channel) = guard.as_mut() else { continue };
            match exchange(channel.as_mut(), command) {
                Exchange::Reply(reply) => return Ok(reply),
                Exchange::Stale(source) => {
                    *guard = None;
                    // A file that was just opened and is still not usable is not "stale": the
                    // device is broken, and retrying would only repeat the command.
                    if fresh || attempt == 1 {
                        return Err(FirmwareError::Io { operation: command.to_string(), source });
                    }
                }
            }
        }
        Err(FirmwareError::Io {
            operation: command.to_string(),
            source: io::Error::other("no usable channel to the device"),
        })
    }
}

/// Writes `command` and reads the reply that the kernel keeps for this open file.
fn exchange(channel: &mut dyn Channel, command: &str) -> Exchange {
    match write_once(channel, command.as_bytes()) {
        Ok(()) => match read_reply(channel) {
            Ok(Some(reply)) => Exchange::Reply(reply),
            Ok(None) => Exchange::Stale(io::Error::new(io::ErrorKind::UnexpectedEof, "empty reply")),
            Err(e) => Exchange::Stale(e),
        },
        // The module rejected the command and left its reason in the reply buffer (the same
        // reply a rejected command gets from the simulator).
        Err(write_error) => match read_reply(channel) {
            Ok(Some(reply)) => Exchange::Reply(reply),
            _ => Exchange::Stale(write_error),
        },
    }
}

fn write_once(channel: &mut dyn Channel, bytes: &[u8]) -> io::Result<()> {
    loop {
        match channel.write(bytes) {
            Ok(n) if n == bytes.len() => return Ok(()),
            Ok(n) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    format!("short write ({n} of {} bytes)", bytes.len()),
                ));
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
}

/// One read: the kernel hands the whole reply over in one piece and a second read is EOF.
/// `Ok(None)` is an empty read.
fn read_reply(channel: &mut dyn Channel) -> io::Result<Option<String>> {
    let mut buffer = [0u8; REPLY_BUFFER];
    loop {
        match channel.read(&mut buffer) {
            Ok(0) => return Ok(None),
            Ok(n) => return Ok(Some(String::from_utf8_lossy(&buffer[..n]).into_owned())),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
}
