//! The read-only probe of the firmware, for laptops nobody has verified.

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};

use crate::firmware::{Device, FirmwareError, ReadOnly, Reply};

/// Everything the probe may ask the firmware: reads only. The probe runs behind
/// [`ReadOnly`], and a test proves that nothing outside this list is ever sent.
pub const READ_ONLY: [&str; 14] = [
    "version",
    "caps",
    "sensors",
    "get_fan",
    "get_mode",
    "get_modes",
    "get_overdrive",
    "get_boot",
    "get_kbtimeout",
    "get_usb",
    "get_battery 1",
    "get_battery 2",
    "get_gpu",
    "get_rear",
];

/// What one probe command answered.
#[derive(Debug)]
pub struct ProbeEntry {
    /// The command, exactly as sent.
    pub command: &'static str,
    /// The reply, or why there was none.
    pub outcome: Result<Reply, FirmwareError>,
}

/// The answers of a probe, in the order the commands were sent. Serialised as the Python edition
/// printed it: an object from command to `{"ok": true, "value": {...}}` or
/// `{"ok": false, "error": "..."}` (the error cut to 120 characters, as before), with the reply
/// fields in the order the module printed them.
#[derive(Debug)]
pub struct ProbeReport {
    entries: Vec<ProbeEntry>,
}

impl ProbeReport {
    /// Every command with its answer.
    pub fn entries(&self) -> &[ProbeEntry] {
        &self.entries
    }

    /// How many commands got an answer.
    pub fn answered(&self) -> usize {
        self.entries.iter().filter(|e| e.outcome.is_ok()).count()
    }
}

struct Outcome<'a>(&'a Result<Reply, FirmwareError>);

impl Serialize for Outcome<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2))?;
        match self.0 {
            Ok(reply) => {
                map.serialize_entry("ok", &true)?;
                map.serialize_entry("value", reply)?;
            }
            Err(e) => {
                map.serialize_entry("ok", &false)?;
                map.serialize_entry("error", &e.to_string().chars().take(120).collect::<String>())?;
            }
        }
        map.end()
    }
}

impl Serialize for ProbeReport {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.entries.len()))?;
        for entry in &self.entries {
            map.serialize_entry(entry.command, &Outcome(&entry.outcome))?;
        }
        map.end()
    }
}

/// Sends every command of [`READ_ONLY`], in order, and reports what each answered. A command
/// that gets no answer is an entry, not a failure of the probe. The device is wrapped in
/// [`ReadOnly`] here, so even a mistake in the list could not write a setting.
pub fn probe_firmware(device: &dyn Device) -> ProbeReport {
    let guard = ReadOnly::new(device);
    let entries =
        READ_ONLY.iter().map(|&command| ProbeEntry { command, outcome: guard.execute(command) }).collect();
    ProbeReport { entries }
}
