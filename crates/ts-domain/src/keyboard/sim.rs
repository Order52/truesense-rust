//! An in-memory keyboard controller for the simulator (`TRUESENSE_SIM=1`) and for the tests of
//! everything above this module.
//!
//! It records every report, tracks the brightness and the backlight-timeout flag the way the
//! controller does, and answers the two queries (`0x88` brightness, `0xB0` timeout flag), so
//! the command line, the key service and the window run without hardware.
#![forbid(unsafe_code)]

use std::io;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use super::{HidDevice, HidOpener, KeyboardError, OUTPUT_LEN, PACKET_LEN};
use crate::keymap::{self, Color};

#[derive(Debug)]
pub(super) struct Inner {
    packets: Vec<[u8; PACKET_LEN]>,
    reports: Vec<[u8; OUTPUT_LEN]>,
    brightness: u8,
    timeout: bool,
    /// The query the next feature read answers.
    pending: u8,
}

/// A simulated controller. Clones share one keyboard, like several parts of one process share
/// the one real device.
#[derive(Debug, Clone)]
pub struct SimHid {
    pub(super) inner: Arc<Mutex<Inner>>,
}

impl Default for SimHid {
    fn default() -> Self {
        Self::new()
    }
}

impl SimHid {
    /// A controller in its factory state: brightness 25, timeout flag on.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                packets: Vec::new(),
                reports: Vec::new(),
                brightness: 25,
                timeout: true,
                pending: 0,
            })),
        }
    }

    /// The process-wide simulated keyboard: the daemon's handlers, the key service and a command
    /// must all see the same one, exactly as they would see the one real device.
    pub fn shared() -> SimHid {
        static SHARED: OnceLock<SimHid> = OnceLock::new();
        SHARED.get_or_init(SimHid::new).clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Every feature report sent so far.
    pub fn packets(&self) -> Vec<[u8; PACKET_LEN]> {
        self.lock().packets.clone()
    }

    /// Forgets the recorded feature reports and per-key output reports (the state stays).
    pub fn clear_log(&self) {
        let mut s = self.lock();
        s.packets.clear();
        s.reports.clear();
    }

    /// How many per-key output reports were written (8 per upload).
    pub fn frames(&self) -> usize {
        self.lock().reports.len()
    }

    /// The colours of the last complete per-key upload, or `None` before the first.
    pub fn last_cells(&self) -> Option<Vec<Color>> {
        let s = self.lock();
        let per_upload = keymap::NUM_CELLS * 4 / OUTPUT_LEN;
        let start = s.reports.len().checked_sub(per_upload)?;
        Some(s.reports[start..].iter().flat_map(|r| r.chunks_exact(4).map(|c| [c[1], c[2], c[3]])).collect())
    }

    /// The simulated brightness.
    pub fn brightness(&self) -> u8 {
        self.lock().brightness
    }

    /// The simulated controller's own backlight-timeout flag.
    pub fn timeout(&self) -> bool {
        self.lock().timeout
    }
}

impl HidOpener for SimHid {
    fn open(&self) -> Result<Box<dyn HidDevice>, KeyboardError> {
        Ok(Box::new(self.clone()))
    }
}

impl HidDevice for SimHid {
    /// Records a feature report and applies its effect to the simulated controller.
    fn set_feature(&mut self, payload: &[u8; PACKET_LEN]) -> io::Result<()> {
        let mut s = self.lock();
        s.packets.push(*payload);
        match payload[0] {
            // Effect and brightness packets carry the brightness in byte 4.
            0x08 => s.brightness = payload[4],
            0x30 => s.timeout = payload[2] != 0,
            0x88 | 0xB0 => s.pending = payload[0],
            _ => {}
        }
        Ok(())
    }

    /// Answers the query sent last; the report echoes its command byte.
    fn get_feature(&mut self) -> io::Result<[u8; PACKET_LEN]> {
        let s = self.lock();
        let mut r = [0u8; PACKET_LEN];
        r[0] = s.pending;
        match s.pending {
            0x88 => r[4] = s.brightness,
            0xB0 => r[2] = u8::from(s.timeout),
            _ => {}
        }
        r[PACKET_LEN - 1] = super::checksum(&r[..PACKET_LEN - 1]);
        Ok(r)
    }

    fn write_output(&mut self, data: &[u8; OUTPUT_LEN]) -> io::Result<()> {
        self.lock().reports.push(*data);
        Ok(())
    }
}
