//! The on/off and choice settings: power mode, panel overdrive, boot sound, backlight timeout,
//! USB charging, battery limit and calibration, GPU mode.

use serde::Serialize;

use super::{Coded, GpuMode, Hw, HwError, PowerMode, Reading, UsbLevel};

/// Which of the firmware interfaces the kernel module found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Capabilities {
    /// The gaming interface: modes, fans, sensors, rear light.
    pub game: bool,
    /// The APGe interface: USB charging while off, backlight timeout flag.
    pub apge: bool,
    /// The battery interface: charge limit and calibration.
    pub battery: bool,
}

fn bit(on: bool) -> u8 {
    u8::from(on)
}

impl Hw {
    /// The kernel module's version.
    pub fn version(&self) -> Result<String, HwError> {
        let reply = self.exec("version")?;
        reply.get("version").map(str::to_string).ok_or_else(|| {
            HwError::Firmware(crate::firmware::FirmwareError::BadReply {
                detail: "missing/invalid 'version' in reply".to_string(),
            })
        })
    }

    /// Which firmware interfaces the module found; an interface the reply does not mention counts as absent.
    pub fn capabilities(&self) -> Result<Capabilities, HwError> {
        let reply = self.exec("caps")?;
        let on = |key: &str| reply.get(key) == Some("1");
        Ok(Capabilities { game: on("game"), apge: on("apge"), battery: on("battery") })
    }

    // ---- power mode ----

    /// The current power mode; a code this BIOS has and we do not is [`Reading::Unknown`].
    pub fn get_mode(&self) -> Result<Reading<PowerMode>, HwError> {
        Ok(Reading::from_code(self.read_int("get_mode", "mode")?))
    }

    /// Sets the power mode and returns it once the firmware reports it.
    pub fn set_mode(&self, mode: PowerMode) -> Result<PowerMode, HwError> {
        self.exec(&format!("set_mode {}", mode.code()))?;
        self.settle("power mode", &Reading::Known(mode), || self.get_mode(), Reading::to_string)?;
        Ok(mode)
    }

    /// The modes this laptop's firmware offers, from the firmware's bit mask (bit N for code N).
    pub fn supported_modes(&self) -> Result<Vec<PowerMode>, HwError> {
        let mask = self.read_int("get_modes", "mask")?;
        Ok(PowerMode::ALL.iter().copied().filter(|m| mask >> m.code() & 1 == 1).collect())
    }

    // ---- on/off firmware settings ----

    /// Panel overdrive; `None` when the firmware answers "unknown" (`-1`).
    pub fn get_overdrive(&self) -> Result<Option<bool>, HwError> {
        let value = self.read_int("get_overdrive", "overdrive")?;
        Ok((value >= 0).then_some(value != 0))
    }

    /// Switches panel overdrive and returns the verified state.
    pub fn set_overdrive(&self, on: bool) -> Result<bool, HwError> {
        self.exec(&format!("set_overdrive {}", bit(on)))?;
        self.settle("overdrive", &Some(on), || self.get_overdrive(), |v| format!("{v:?}"))?;
        Ok(on)
    }

    /// The firmware's start-up chime.
    pub fn get_boot_sound(&self) -> Result<bool, HwError> {
        Ok(self.read_int("get_boot", "boot")? != 0)
    }

    /// Switches the start-up chime and returns the verified state.
    pub fn set_boot_sound(&self, on: bool) -> Result<bool, HwError> {
        self.exec(&format!("set_boot {}", bit(on)))?;
        self.settle("boot sound", &on, || self.get_boot_sound(), bool::to_string)?;
        Ok(on)
    }

    fn wmi_kb_timeout(&self) -> Result<Option<bool>, HwError> {
        let value = self.read_int("get_kbtimeout", "kbtimeout")?;
        Ok((value >= 0).then_some(value != 0))
    }

    /// "Backlight off after 30 s of no typing". The keyboard controller's own flag is the one
    /// that acts, so it is asked first; the firmware's flag is kept in step and is the fallback
    /// when the controller cannot say. `None` when neither can.
    pub fn get_kb_timeout(&self) -> Result<Option<bool>, HwError> {
        if let Some(flag) = &self.kb_timeout {
            if let Ok(Some(value)) = flag.get() {
                return Ok(Some(value));
            }
        }
        self.wmi_kb_timeout()
    }

    /// Sets both flags and verifies the one that acts. The controller's flag is best effort:
    /// the firmware flag, written first, already took.
    pub fn set_kb_timeout(&self, on: bool) -> Result<bool, HwError> {
        self.exec(&format!("set_kbtimeout {}", bit(on)))?;
        if let Some(controller) = &self.kb_timeout {
            let _ = controller.set(on);
        }
        self.settle("backlight timeout", &Some(on), || self.get_kb_timeout(), |v| format!("{v:?}"))?;
        Ok(on)
    }

    /// The battery level down to which USB ports charge while the laptop is off.
    pub fn get_usb_charge(&self) -> Result<Reading<UsbLevel>, HwError> {
        Ok(Reading::from_code(self.read_int("get_usb", "usb")?))
    }

    /// Sets that level and returns it once the firmware reports it.
    pub fn set_usb_charge(&self, level: UsbLevel) -> Result<UsbLevel, HwError> {
        self.exec(&format!("set_usb {}", level.code()))?;
        self.settle("usb charging", &Reading::Known(level), || self.get_usb_charge(), Reading::to_string)?;
        Ok(level)
    }

    /// Whether charging stops at 80 %.
    pub fn get_battery_limit(&self) -> Result<bool, HwError> {
        Ok(self.read_int("get_battery 1", "battery")? != 0)
    }

    /// Turns the 80 % charge limit on or off and returns the verified state.
    pub fn set_battery_limit(&self, on: bool) -> Result<bool, HwError> {
        self.exec(&format!("set_battery 1 {}", bit(on)))?;
        self.settle("battery limit", &on, || self.get_battery_limit(), bool::to_string)?;
        Ok(on)
    }

    /// Whether the firmware's battery calibration cycle is running.
    pub fn get_calibration(&self) -> Result<bool, HwError> {
        Ok(self.read_int("get_battery 2", "battery")? != 0)
    }

    /// Starts or stops the calibration cycle (needs AC power; a full cycle takes hours) and
    /// returns the verified state.
    pub fn set_calibration(&self, on: bool) -> Result<bool, HwError> {
        self.exec(&format!("set_battery 2 {}", bit(on)))?;
        self.settle("battery calibration", &on, || self.get_calibration(), bool::to_string)?;
        Ok(on)
    }

    // ---- GPU mode (MUX; takes effect after a reboot) ----

    /// The GPU mode.
    pub fn get_gpu_mode(&self) -> Result<Reading<GpuMode>, HwError> {
        Ok(Reading::from_code(self.read_int("get_gpu", "gpu")?))
    }

    /// Sets the GPU mode and returns it once the firmware reports it. It applies after a reboot.
    pub fn set_gpu_mode(&self, mode: GpuMode) -> Result<GpuMode, HwError> {
        self.exec(&format!("set_gpu {}", mode.code()))?;
        self.settle("gpu mode", &Reading::Known(mode), || self.get_gpu_mode(), Reading::to_string)?;
        Ok(mode)
    }
}
