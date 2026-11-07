//! Sensors and fans.

use serde::Serialize;

use std::sync::atomic::Ordering;

use super::{FanDuty, FanMode, Hw, HwError};
use crate::firmware::FirmwareError;

/// One reading of every temperature (degrees C) and both fan speeds (rpm). The field names are
/// the ones the daemon, the CLI and the window exchange as JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Sensors {
    /// CPU temperature.
    pub cpu_temp: i64,
    /// Board temperature.
    pub sys_temp: i64,
    /// GPU temperature.
    pub gpu_temp: i64,
    /// CPU fan speed.
    pub cpu_fan_rpm: i64,
    /// GPU fan speed.
    pub gpu_fan_rpm: i64,
}

/// The fan duty registers of both fans, in percent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Duty {
    /// CPU fan.
    pub cpu: i64,
    /// GPU fan.
    pub gpu: i64,
}

/// What the firmware reports about fan control.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FanState {
    /// The behaviour word as `0x41`, kept verbatim for unknown values.
    pub behavior_raw: String,
    /// The decoded behaviour.
    pub mode: FanMode,
    /// The duty registers (what the firmware last accepted; not the measured speed).
    pub duty_register_pct: Duty,
}

impl Hw {
    /// Every temperature and both fan speeds, in one firmware call.
    pub fn sensors(&self) -> Result<Sensors, HwError> {
        let reply = self.exec("sensors")?;
        Ok(Sensors {
            cpu_temp: reply.int("cpu")?,
            sys_temp: reply.int("sys")?,
            gpu_temp: reply.int("gpu")?,
            cpu_fan_rpm: reply.int("cpu_rpm")?,
            gpu_fan_rpm: reply.int("gpu_rpm")?,
        })
    }

    /// The temperatures with three firmware calls instead of five: the module's `sensors_temps`
    /// leaves the two fan-speed reads out (each firmware call is an SMI that stalls every core
    /// for milliseconds). The fan speeds in the result are `last_rpm`, what the caller read
    /// before; they are only for display, no decision uses them. A module that predates the
    /// command refuses it once ("unknown command", or "invalid argument" from the oldest handler)
    /// and is asked for the full reading from then on.
    pub fn sensor_temps(&self, last_rpm: (i64, i64)) -> Result<Sensors, HwError> {
        if self.temps_command.load(Ordering::Relaxed) {
            match self.exec("sensors_temps") {
                Ok(reply) => {
                    return Ok(Sensors {
                        cpu_temp: reply.int("cpu")?,
                        sys_temp: reply.int("sys")?,
                        gpu_temp: reply.int("gpu")?,
                        cpu_fan_rpm: last_rpm.0,
                        gpu_fan_rpm: last_rpm.1,
                    });
                }
                // `sensors_temps` takes no argument, so any refusal means the loaded module does
                // not have it. An older module says "unknown command" in its reply but then
                // overwrites that reply with "invalid argument" (its write handler leaves
                // -EINVAL set), so both words mean the same thing here.
                Err(HwError::Firmware(FirmwareError::Rejected { reason, .. }))
                    if reason == "unknown command" || reason == "invalid argument" =>
                {
                    self.temps_command.store(false, Ordering::Relaxed);
                }
                Err(e) => return Err(e),
            }
        }
        self.sensors()
    }

    /// The fan behaviour and duty registers.
    pub fn fan_state(&self) -> Result<FanState, HwError> {
        let reply = self.exec("get_fan")?;
        let behavior = reply.int("behavior")?;
        Ok(FanState {
            behavior_raw: format!("0x{behavior:x}"),
            mode: FanMode::from_behavior(behavior),
            duty_register_pct: Duty { cpu: reply.int("cpu")?, gpu: reply.int("gpu")? },
        })
    }

    fn fan_mode(&self) -> Result<FanMode, HwError> {
        Ok(self.fan_state()?.mode)
    }

    fn fan_duty(&self) -> Result<Duty, HwError> {
        Ok(self.fan_state()?.duty_register_pct)
    }

    /// Hands the fans back to the firmware's own control, and waits until it says so.
    pub fn fan_auto(&self) -> Result<(), HwError> {
        self.exec("set_fan_auto")?;
        self.settle("fan auto", &FanMode::Auto, || self.fan_mode(), FanMode::to_string)
    }

    /// Runs both fans at full speed.
    pub fn fan_max(&self) -> Result<(), HwError> {
        self.exec("set_fan_max")?;
        self.settle("fan max", &FanMode::Max, || self.fan_mode(), FanMode::to_string)
    }

    /// Sets both fans' duty. A duty of 0 stops that fan: the daemon alone may ask for it, after
    /// its confirmation and temperature checks, and watches the temperatures afterwards. A stop
    /// is proven by reading the duty registers back; if the firmware did not take it, the fans are
    /// handed back to Auto and [`HwError::FanStopRefused`] says whether that worked. (On the
    /// tested PH16-71 the firmware takes the command but keeps a stopped fan at its minimum
    /// speed.)
    pub fn fan_manual(&self, cpu: FanDuty, gpu: FanDuty) -> Result<(), HwError> {
        self.exec(&format!("set_fan_manual {cpu} {gpu}"))?;
        self.settle("fan manual", &FanMode::Manual, || self.fan_mode(), FanMode::to_string)?;
        if cpu.is_stop() || gpu.is_stop() {
            // A stopped fan was never seen working before: prove the firmware took it, or hand the fans back.
            let want = Duty { cpu: i64::from(cpu.percent()), gpu: i64::from(gpu.percent()) };
            if let Err(cause) =
                self.settle("fan stop", &want, || self.fan_duty(), |d| format!("({}, {})", d.cpu, d.gpu))
            {
                let returned_to_auto = self.fan_auto().is_ok();
                return Err(HwError::FanStopRefused { cause: Box::new(cause), returned_to_auto });
            }
        }
        Ok(())
    }
}
