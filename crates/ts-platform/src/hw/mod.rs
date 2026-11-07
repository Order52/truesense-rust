//! hw: every firmware feature of the Predator Helios 16 as a typed function.
//!
//! Everything goes through a [`Device`](crate::firmware), so the same code drives the
//! kernel module and the simulator. Every setter writes, reads the value back (retrying for
//! about a second, because the firmware can answer `-1` or the old value right after a write)
//! and returns the verified new value or a typed [`HwError`]: a value that cannot be verified is
//! never reported as set. A value the firmware would refuse is refused first, by
//! the typed values (`PowerMode`, `FanDuty`, `Rgb`, ...) or by the setter, before anything is sent.

mod fans;
mod features;
mod linux;
mod probe;
mod rear;
mod types;

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use crate::firmware::{Device, FirmwareError, Reply};

pub use fans::{Duty, FanState, Sensors};
pub use features::Capabilities;
pub use linux::{CommandRunner, LinuxSensors, ProcessRunner};
pub use probe::{ProbeEntry, READ_ONLY, probe_firmware};
pub use rear::{REAR_ZONES, Rear, RearSettings};
pub use types::{
    Coded, FAN_AUTO_BEHAVIOR, FAN_MANUAL_BEHAVIOR, FAN_MAX_BEHAVIOR, FanDuty, FanMode, GpuMode, PowerMode,
    Reading, RearMode, Rgb, UsbLevel,
};

/// The levels USB charging while off can be limited to, as the window offers them.
pub const USB_LEVELS: [UsbLevel; 4] =
    [UsbLevel::Off, UsbLevel::Percent10, UsbLevel::Percent20, UsbLevel::Percent30];

/// A failure of a hardware feature.
#[derive(Debug)]
pub enum HwError {
    /// The firmware, or the way to it, failed or refused the command.
    Firmware(FirmwareError),
    /// A value was refused before anything was sent; the text says what is allowed.
    Invalid(String),
    /// The firmware took the command but never reported the new value.
    Readback {
        /// The setting, for example `power mode`.
        what: &'static str,
        /// The value that was written.
        wanted: String,
        /// The last value read, or `None` when the last read itself failed.
        last: Option<String>,
    },
    /// A fan stop was written but the duty registers never showed it: the firmware did not take
    /// it. The fans were handed back to the firmware's own control.
    FanStopRefused {
        /// Why the stop could not be verified.
        cause: Box<HwError>,
        /// Whether handing the fans back worked. When it did not, the fans may still be at a
        /// stopped or manual duty and the caller must retry `fan_auto` at once.
        returned_to_auto: bool,
    },
}

impl fmt::Display for HwError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Firmware(e) => e.fmt(f),
            Self::Invalid(message) => f.write_str(message),
            Self::Readback { what, wanted, last } => write!(
                f,
                "{what} readback mismatch (wanted {wanted}, last read {})",
                last.as_deref().unwrap_or("nothing")
            ),
            Self::FanStopRefused { cause, returned_to_auto: true } => {
                write!(f, "the firmware did not accept the stop ({cause}); the fans were returned to Auto")
            }
            Self::FanStopRefused { cause, returned_to_auto: false } => write!(
                f,
                "the firmware did not accept the stop ({cause}) and the fans could NOT be returned to Auto: run `truesense fan auto`"
            ),
        }
    }
}

impl std::error::Error for HwError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Firmware(e) => Some(e),
            Self::FanStopRefused { cause, .. } => Some(cause.as_ref()),
            Self::Invalid(_) | Self::Readback { .. } => None,
        }
    }
}

impl From<FirmwareError> for HwError {
    fn from(e: FirmwareError) -> Self {
        Self::Firmware(e)
    }
}

/// How long a setter waits for the firmware to report the value it was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retry {
    /// How many times the value is read.
    pub tries: u32,
    /// The pause between two reads.
    pub delay: Duration,
}

impl Default for Retry {
    /// Ten reads 100 ms apart: about a second, which the firmware has always been enough for.
    fn default() -> Self {
        Self { tries: 10, delay: Duration::from_millis(100) }
    }
}

/// The keyboard controller's own "backlight off after 30 s" flag. It sits behind a trait because
/// the keyboard (a USB HID device) belongs to a layer above this one.
pub trait KbTimeoutFlag: Send + Sync {
    /// The flag, or `Ok(None)` when the controller cannot say.
    fn get(&self) -> Result<Option<bool>, HwError>;
    /// Sets the flag.
    fn set(&self, on: bool) -> Result<(), HwError>;
}

/// Where the rear light remembers what it was last set to (a convenience file in the user's
/// state). Writing is best effort: a failure there must never fail the light, so the method
/// cannot fail.
pub trait RearMemory: Send + Sync {
    /// Stores `value` under `key` (`rear_zones`, `rear_state`).
    fn remember(&self, key: &str, value: serde_json::Value);
}

/// Called to wait between two read-backs; replaced in tests so that they do not wait.
pub type Sleeper = Arc<dyn Fn(Duration) + Send + Sync>;

/// The hardware, as one value that carries its dependencies.
#[derive(Clone)]
pub struct Hw {
    device: Arc<dyn Device>,
    retry: Retry,
    sleep: Sleeper,
    kb_timeout: Option<Arc<dyn KbTimeoutFlag>>,
    memory: Option<Arc<dyn RearMemory>>,
}

impl Hw {
    /// The hardware behind `device`, with production timing (see [`Retry::default`]).
    pub fn new(device: Arc<dyn Device>) -> Self {
        Self {
            device,
            retry: Retry::default(),
            sleep: Arc::new(std::thread::sleep),
            kb_timeout: None,
            memory: None,
        }
    }

    /// Changes how long setters wait for a read-back.
    #[must_use]
    pub fn with_retry(mut self, retry: Retry) -> Self {
        self.retry = retry;
        self
    }

    /// Replaces the function that waits between read-backs (tests pass a fake clock).
    #[must_use]
    pub fn with_sleeper(mut self, sleep: Sleeper) -> Self {
        self.sleep = sleep;
        self
    }

    /// Adds the keyboard controller's backlight timeout flag.
    #[must_use]
    pub fn with_kb_timeout(mut self, flag: Arc<dyn KbTimeoutFlag>) -> Self {
        self.kb_timeout = Some(flag);
        self
    }

    /// Adds the place where the rear light remembers its colours.
    #[must_use]
    pub fn with_rear_memory(mut self, memory: Arc<dyn RearMemory>) -> Self {
        self.memory = Some(memory);
        self
    }

    /// The device, for the few callers that need a raw command (the probe, the self-test).
    pub fn device(&self) -> &Arc<dyn Device> {
        &self.device
    }

    fn exec(&self, command: &str) -> Result<Reply, HwError> {
        Ok(self.device.execute(command)?)
    }

    fn read_int(&self, command: &str, key: &str) -> Result<i64, HwError> {
        Ok(self.exec(command)?.int(key)?)
    }

    /// Reads back until the firmware reports `want`; fails only after the configured number of
    /// reads. A read that fails counts like a read of the wrong value: right after a write the
    /// firmware may answer with an error for a moment.
    fn settle<T: PartialEq>(
        &self,
        what: &'static str,
        want: &T,
        mut read: impl FnMut() -> Result<T, HwError>,
        show: impl Fn(&T) -> String,
    ) -> Result<(), HwError> {
        let mut last = None;
        for attempt in 0..self.retry.tries {
            match read() {
                Ok(got) if got == *want => return Ok(()),
                Ok(got) => last = Some(show(&got)),
                Err(_) => last = None,
            }
            if attempt + 1 < self.retry.tries {
                (self.sleep)(self.retry.delay);
            }
        }
        Err(HwError::Readback { what, wanted: show(want), last })
    }
}

#[cfg(test)]
mod tests;
