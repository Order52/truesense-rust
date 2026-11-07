//! The simple firmware settings: the 80 % battery limit, calibration, USB charging while off,
//! "charge to full once", LCD overdrive, the boot sound, the backlight timeout and the touchpad.
//!
//! Every setter returns the value read back from the firmware, never the value that was asked
//! for: a setting that could not be verified is an error (see `ts_platform::hw`).
#![forbid(unsafe_code)]

use std::fmt;

use serde::{Deserialize, Serialize};
use ts_ipc::wire::Args;
use ts_platform::hw::UsbLevel;

use super::status::usb_percent;
use super::{Api, ApiError, ApiResult};

/// The firmware's three battery settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BatteryStatus {
    /// Charging stops at 80 %.
    pub limit_80: bool,
    /// The calibration cycle runs (it needs AC and takes hours).
    pub calibration: bool,
    /// The battery level down to which USB ports charge while the laptop is off (0 = off).
    pub usb_charge_stop: i64,
}

/// What can be asked of "charge to 100 % once".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChargeFullAction {
    /// Only ask.
    Status,
    /// Lift the 80 % limit once.
    Start,
    /// Put the limit back now.
    Cancel,
}

impl std::str::FromStr for ChargeFullAction {
    type Err = ApiError;

    fn from_str(text: &str) -> Result<Self, ApiError> {
        match text {
            "status" | "get" | "" => Ok(Self::Status),
            "start" => Ok(Self::Start),
            "cancel" => Ok(Self::Cancel),
            other => Err(ApiError::invalid(format!("battery full: start | cancel | status (got '{other}')"))),
        }
    }
}

/// "Charge to 100 % once": whether it runs and, for a status question, the limit, the level and
/// the mains state the service sees.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChargeFull {
    /// The one-time full charge runs.
    pub active: bool,
    /// The 80 % limit is on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<bool>,
    /// The battery level in percent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capacity: Option<f64>,
    /// The charger is in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ac: Option<bool>,
}

impl ChargeFull {
    /// The sentence the command prints for what was asked.
    pub fn message(&self, action: ChargeFullAction) -> String {
        match action {
            ChargeFullAction::Start => "charging to 100 % once; the 80 % limit comes back when you unplug, when it has been full for 5 minutes, or after 24 hours".to_owned(),
            ChargeFullAction::Cancel => "80 % limit back on".to_owned(),
            ChargeFullAction::Status => self.to_string(),
        }
    }
}

impl fmt::Display for ChargeFull {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let battery = self.capacity.map_or_else(|| "?".to_owned(), |c| format!("{c}"));
        write!(
            f,
            "80 % limit {}, battery {battery} %, charging to full once: {}",
            super::on_off_opt(self.limit),
            if self.active { "yes" } else { "no" }
        )
    }
}

impl Api {
    /// The 80 % limit, the calibration cycle and the USB charging level.
    pub fn battery_status(&self) -> ApiResult<BatteryStatus> {
        Ok(BatteryStatus {
            limit_80: self.s.hw.get_battery_limit()?,
            calibration: self.s.hw.get_calibration()?,
            usb_charge_stop: usb_percent(self.s.hw.get_usb_charge()?),
        })
    }

    /// Whether charging stops at 80 %.
    pub fn battery_limit(&self) -> ApiResult<bool> {
        Ok(self.s.hw.get_battery_limit()?)
    }

    /// Turns the 80 % limit on or off (verified by read-back).
    pub fn set_battery_limit(&self, on: bool) -> ApiResult<bool> {
        Ok(self.s.hw.set_battery_limit(on)?)
    }

    /// Whether the calibration cycle runs.
    pub fn calibration(&self) -> ApiResult<bool> {
        Ok(self.s.hw.get_calibration()?)
    }

    /// Starts or stops the calibration cycle (verified by read-back; it needs AC power).
    pub fn set_calibration(&self, on: bool) -> ApiResult<bool> {
        Ok(self.s.hw.set_calibration(on)?)
    }

    /// The battery level down to which USB ports charge while the laptop is off (0 = off).
    pub fn usb_charge(&self) -> ApiResult<i64> {
        Ok(usb_percent(self.s.hw.get_usb_charge()?))
    }

    /// Sets that level: 0, 10, 20 or 30, anything else is refused before the firmware is asked.
    pub fn set_usb_charge(&self, level: i64) -> ApiResult<i64> {
        let level = UsbLevel::from_percent(level)?;
        Ok(i64::from(self.s.hw.set_usb_charge(level)?.percent()))
    }

    /// Asks the service about, starts or cancels "charge to 100 % once": the 80 % limit comes back
    /// when you unplug, when the battery has been full for 5 minutes, or after 24 hours. Starting
    /// needs the charger in and the limit on; the service checks.
    pub fn charge_full(&self, action: ChargeFullAction) -> ApiResult<ChargeFull> {
        let verb = match action {
            ChargeFullAction::Status => "get",
            ChargeFullAction::Start => "start",
            ChargeFullAction::Cancel => "cancel",
        };
        self.ask("battery_full", Args::new().with("action", verb))
    }

    /// The LCD overdrive; `None` when the firmware cannot say.
    pub fn overdrive(&self) -> ApiResult<Option<bool>> {
        Ok(self.s.hw.get_overdrive()?)
    }

    /// Switches the LCD overdrive (verified by read-back).
    pub fn set_overdrive(&self, on: bool) -> ApiResult<bool> {
        Ok(self.s.hw.set_overdrive(on)?)
    }

    /// The firmware's start-up chime.
    pub fn boot_sound(&self) -> ApiResult<bool> {
        Ok(self.s.hw.get_boot_sound()?)
    }

    /// Switches the start-up chime (verified by read-back).
    pub fn set_boot_sound(&self, on: bool) -> ApiResult<bool> {
        Ok(self.s.hw.set_boot_sound(on)?)
    }

    /// "Backlight off after 30 s of no typing"; `None` when neither the keyboard controller nor
    /// the firmware can say.
    pub fn keyboard_timeout(&self) -> ApiResult<Option<bool>> {
        Ok(self.s.hw.get_kb_timeout()?)
    }

    /// Sets that timeout (verified by read-back).
    pub fn set_keyboard_timeout(&self, on: bool) -> ApiResult<bool> {
        Ok(self.s.hw.set_kb_timeout(on)?)
    }

    /// Whether the touchpad is on; `None` when it cannot be told.
    pub fn touchpad(&self) -> Option<bool> {
        self.s.touchpad.enabled()
    }

    /// Switches the touchpad. Writing the kernel's switch needs root, so the service does it for
    /// users; root with no service writes it directly.
    pub fn set_touchpad(&self, on: bool) -> ApiResult<bool> {
        let action = if on { "on" } else { "off" };
        match self.ask::<bool>("touchpad", Args::new().with("action", action)) {
            Ok(now) => Ok(now),
            Err(_) if self.s.euid == 0 => Ok(self.s.touchpad.set_enabled(on)?),
            Err(e) => Err(e),
        }
    }
}
