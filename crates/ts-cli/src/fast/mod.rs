//! fast: the instant client behind `truesense-fast`, for status-bar buttons.
//!
//! `truesense-fast fan smart|auto|max|<20-100>`, `power <mode> [hold-minutes]` and
//! `auto on|off|native|smart` do one thing and print nothing. With the daemon running it is a
//! single request over the control socket (no settle delay: the daemon changes its own fan control
//! and writes the hardware under one lock); without one it does what the hardware allows without
//! a daemon, as `control.py` of the Python edition did.
//!
//! It exists so that a button press costs a few milliseconds: nothing here loads the
//! configuration, opens a store or builds the application layer. It needs only the two things it
//! talks to, the daemon ([`Daemon`]) and the firmware ([`FastHardware`]), and a way to pause.
//!
//! # Safety
//!
//! A fan is never stopped from here: duty 0 needs the daemon's confirmation and its temperature
//! watchdog, which a status-bar button does not give, so it is refused with the same words as
//! `truesense fan set 0` without `--confirm`. A duty is 0 or 20..100 and is checked before
//! anything is sent.
#![forbid(unsafe_code)]

use std::io::Write;
use std::time::Duration;

use serde_json::Value;
use ts_api::api::{Daemon, Refusal};
use ts_ipc::client::ClientError;
use ts_ipc::wire::Args;
use ts_platform::hw::{Coded, FanDuty, Hw, HwError, PowerMode};

/// How long an old daemon is given to notice that its smart curve was switched off before the
/// firmware is written (one tick of its loop).
const LEGACY_SETTLE: Duration = Duration::from_millis(1200);

/// The minutes a mode picked from a button is held against the automatic profile.
const DEFAULT_HOLD_MINUTES: f64 = 30.0;

const USAGE: &str = "usage: truesense-fast fan|power|auto ...";
const FAN_USAGE: &str = "usage: truesense-fast fan smart|auto|max|<20-100>";
const POWER_USAGE: &str = "usage: truesense-fast power <mode> [hold-minutes]";
const AUTO_USAGE: &str = "usage: truesense-fast auto off|native|smart";

/// The firmware, as far as the fast path writes it directly.
pub trait FastHardware {
    /// Hands the fans to the firmware.
    ///
    /// # Errors
    /// The firmware's own refusal or a failed read-back.
    fn fan_auto(&self) -> Result<(), HwError>;

    /// Runs both fans at full speed.
    ///
    /// # Errors
    /// The firmware's own refusal or a failed read-back.
    fn fan_max(&self) -> Result<(), HwError>;

    /// Sets a manual duty for both fans (never 0 here).
    ///
    /// # Errors
    /// The firmware's own refusal or a failed read-back.
    fn fan_manual(&self, cpu: FanDuty, gpu: FanDuty) -> Result<(), HwError>;

    /// Sets the power mode.
    ///
    /// # Errors
    /// The firmware's own refusal or a failed read-back.
    fn set_mode(&self, mode: PowerMode) -> Result<(), HwError>;
}

impl FastHardware for Hw {
    fn fan_auto(&self) -> Result<(), HwError> {
        Hw::fan_auto(self)
    }

    fn fan_max(&self) -> Result<(), HwError> {
        Hw::fan_max(self)
    }

    fn fan_manual(&self, cpu: FanDuty, gpu: FanDuty) -> Result<(), HwError> {
        Hw::fan_manual(self, cpu, gpu)
    }

    fn set_mode(&self, mode: PowerMode) -> Result<(), HwError> {
        Hw::set_mode(self, mode).map(drop)
    }
}

/// What the fast path talks to.
pub struct Fast<'a> {
    /// The control socket.
    pub daemon: &'a dyn Daemon,
    /// The firmware, for when there is no daemon.
    pub hardware: &'a dyn FastHardware,
    /// Pauses; tests pass a recorder.
    pub sleep: &'a dyn Fn(Duration),
}

/// Why a command did not complete.
enum Stop {
    /// The command line is wrong: the text, exit code 2.
    Usage(String),
    /// The operation failed: `error: text`, exit code 1.
    Failed(String),
}

impl From<ClientError> for Stop {
    fn from(e: ClientError) -> Self {
        Self::Failed(e.to_string())
    }
}

impl From<HwError> for Stop {
    fn from(e: HwError) -> Self {
        Self::Failed(e.to_string())
    }
}

impl From<Refusal> for Stop {
    fn from(r: Refusal) -> Self {
        Self::Failed(r.to_string())
    }
}

/// A fan request as the buttons spell it.
enum FanRequest {
    Smart,
    Auto,
    Max,
    Manual(i64),
}

impl FanRequest {
    fn parse(word: &str) -> Option<Self> {
        match word {
            "smart" => Some(Self::Smart),
            "auto" => Some(Self::Auto),
            "max" => Some(Self::Max),
            w if w.starts_with(|c: char| c.is_ascii_digit()) => w.parse().ok().map(Self::Manual),
            _ => None,
        }
    }

    fn mode(&self) -> &'static str {
        match self {
            Self::Smart => "smart",
            Self::Auto => "auto",
            Self::Max => "max",
            Self::Manual(_) => "manual",
        }
    }
}

impl Fast<'_> {
    /// Runs one command line (the words after `truesense-fast`) and returns the exit code: 0 done,
    /// 1 failed, 2 the command line was wrong. Errors go to `err`; nothing is printed on success.
    pub fn run(&self, args: &[String], err: &mut dyn Write) -> i32 {
        let words: Vec<&str> = args.iter().map(String::as_str).collect();
        match self.dispatch(&words) {
            Ok(()) => 0,
            Err(Stop::Usage(text)) => {
                let _ = writeln!(err, "{text}");
                2
            }
            Err(Stop::Failed(text)) => {
                let _ = writeln!(err, "error: {text}");
                1
            }
        }
    }

    fn dispatch(&self, words: &[&str]) -> Result<(), Stop> {
        match words {
            ["fan", word] => {
                let request = FanRequest::parse(word).ok_or_else(|| Stop::Usage(FAN_USAGE.to_owned()))?;
                self.fan(&request)
            }
            ["fan", ..] => Err(Stop::Usage(FAN_USAGE.to_owned())),
            ["power", mode] => self.power(mode, DEFAULT_HOLD_MINUTES),
            ["power", mode, minutes] => {
                let hold = minutes
                    .parse::<f64>()
                    .ok()
                    .filter(|m| m.is_finite() && *m >= 0.0)
                    .ok_or_else(|| Stop::Usage(POWER_USAGE.to_owned()))?;
                self.power(mode, hold)
            }
            ["power", ..] => Err(Stop::Usage(POWER_USAGE.to_owned())),
            ["auto", word] => {
                let value = match *word {
                    "on" => "smart",
                    "off" => "off",
                    "native" => "native",
                    "smart" => "smart",
                    _ => return Err(Stop::Usage(AUTO_USAGE.to_owned())),
                };
                self.daemon.call("set", Args::new().with("key", "auto_mode").with("value", value))?;
                Ok(())
            }
            ["auto", ..] => Err(Stop::Usage(AUTO_USAGE.to_owned())),
            _ => Err(Stop::Usage(USAGE.to_owned())),
        }
    }

    /// `control.fan`: one request to the daemon, or the firmware when there is none.
    fn fan(&self, request: &FanRequest) -> Result<(), Stop> {
        let duties = match request {
            FanRequest::Manual(percent) => {
                let duty = FanDuty::new(*percent).map_err(|_| Refusal::FanDuty)?;
                if duty.is_stop() {
                    // A stop needs the daemon's watchdog and the explicit confirmation of
                    // `truesense fan set 0 --confirm`; a button gives neither.
                    return Err(if self.daemon.available() {
                        Refusal::StopNeedsConfirmation
                    } else {
                        Refusal::StopNeedsService
                    }
                    .into());
                }
                Some(duty)
            }
            _ => None,
        };
        if !self.daemon.available() {
            return self.fan_direct(request, duties);
        }
        let mut args = Args::new().with("mode", request.mode());
        if let FanRequest::Manual(percent) = request {
            args = args.with("cpu", *percent).with("gpu", Value::Null);
        }
        match self.daemon.call("fan", args) {
            Ok(_) => Ok(()),
            Err(e) if e.is_unknown_command() => self.fan_legacy(request, duties),
            Err(e) => Err(e.into()),
        }
    }

    fn fan_direct(&self, request: &FanRequest, duty: Option<FanDuty>) -> Result<(), Stop> {
        match (request, duty) {
            (FanRequest::Auto, _) => self.hardware.fan_auto()?,
            (FanRequest::Max, _) => self.hardware.fan_max()?,
            (FanRequest::Manual(_), Some(d)) => self.hardware.fan_manual(d, d)?,
            (FanRequest::Manual(_), None) | (FanRequest::Smart, _) => {
                return Err(Refusal::SmartNeedsService.into());
            }
        }
        Ok(())
    }

    /// A daemon too old to know `fan`: its smart curve is switched off, one tick is waited, then the
    /// firmware is written.
    fn fan_legacy(&self, request: &FanRequest, duty: Option<FanDuty>) -> Result<(), Stop> {
        let set = |on: bool| self.daemon.call("set", Args::new().with("key", "smart_fan").with("value", on));
        if matches!(request, FanRequest::Smart) {
            set(true)?;
            return Ok(());
        }
        if set(false).is_ok() {
            (self.sleep)(LEGACY_SETTLE);
        }
        self.fan_direct(request, duty)
    }

    /// `control.power`: the daemon sets the mode and holds it; without one the firmware is written
    /// and the hold is asked for on a best-effort basis.
    fn power(&self, word: &str, hold_minutes: f64) -> Result<(), Stop> {
        let mode: PowerMode = word.parse().map_err(|_| Stop::Usage(Self::bad_mode(word)))?;
        if self.daemon.available() {
            let args = Args::new().with("mode", mode.name()).with("hold_minutes", hold_minutes);
            match self.daemon.call("power", args) {
                Ok(_) => return Ok(()),
                Err(e) if e.is_unknown_command() => {}
                Err(e) => return Err(e.into()),
            }
        }
        self.hardware.set_mode(mode)?;
        if hold_minutes != 0.0 && self.daemon.available() {
            // The hold is a convenience: the mode is already set.
            let _ = self.daemon.call("hold", Args::new().with("minutes", hold_minutes));
        }
        Ok(())
    }

    /// The `argparse` text of `truesense power set` for a mode that does not exist, which is what
    /// the status-bar script printed (it ran that command).
    fn bad_mode(word: &str) -> String {
        // Python's repr of the word: double quotes when it holds a single quote and no double one.
        let shown = if word.contains('\'') && !word.contains('"') {
            format!("\"{word}\"")
        } else {
            format!("'{}'", word.replace('\'', "\\'"))
        };
        format!(
            "usage: truesense power set [-h] {{quiet,balanced,performance,turbo,eco}}\ntruesense power set: error: argument mode: invalid choice: {shown} (choose from 'quiet', 'balanced', 'performance', 'turbo', 'eco')"
        )
    }
}

#[cfg(test)]
mod tests;
