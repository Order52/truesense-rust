//! battery: everything the kernel says about the laptop's battery, for the Battery page.
//!
//! [`sensors::Sampler`](crate::sensors::Sampler) answers the two questions the control loop asks
//! (how full, charging or not). A person looking at the Battery page wants the rest: voltage,
//! current, power draw, how worn the cells are, how many cycles they have had, the temperature
//! and whether a charge limit is active. This module reads those from
//! `/sys/class/power_supply/BAT*` and from the two out-of-tree Acer battery interfaces when they
//! are present.
//!
//! # Rules of the reading
//!
//! * A file that is missing, unreadable, empty or not a number is `None`, never an error: the
//!   kernel drivers differ in what they expose (this laptop gives `charge_*` in micro-amp-hours
//!   and no `energy_*`; another gives the reverse), and a page must show what exists.
//! * Nothing from `/sys` is trusted: values outside a plausible range are `None` as well (a
//!   firmware that reports 65535 V is broken, and a gauge must not draw it), text is cut and
//!   stripped of control characters, and every read is size-limited.
//! * Units are converted once, here: volts, amperes (as milliamperes), watts, watt-hours,
//!   milliampere-hours, percent, degrees Celsius.
//! * Current and power are magnitudes. ACPI batteries disagree about the sign of `current_now`
//!   (some report it negative while discharging, this one reports it positive while charging),
//!   so the direction is the [`BatteryInfo::status`], never the sign.
//! * A direct reading wins over a derived one: `power_now` over voltage times current, the
//!   energy pair over the charge pair for the health percentage.
//!
//! Every path lies below an injected root (`/` in production, a temporary tree in tests).
#![forbid(unsafe_code)]

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_foundation::fsx;

/// The kernel's battery class.
const POWER_SUPPLY: &str = "sys/class/power_supply";

/// The `acer-wmi-battery` driver's directory: the firmware's own 80 % health mode, the
/// calibration mode and the battery temperature.
const ACER_BATTERY: &str = "sys/bus/wmi/drivers/acer-wmi-battery";

/// The same health mode under the name the out-of-tree Linuwu-Sense driver gives it. It is the
/// same firmware call (WMID method 21), so it is a second way to read the one switch.
const ACER_LIMITER: &str = "sys/bus/platform/drivers/acer-wmi/acer-wmi/predator_sense";

/// A sysfs attribute is a few bytes; this bounds what a hostile tree can make us read.
const ATTRIBUTE_LIMIT: u64 = 4096;

/// The longest text kept from a vendor string such as the model name.
const TEXT_LIMIT: usize = 64;

/// Above this a voltage is not a battery voltage (laptop packs are 7 to 20 V).
const MAX_VOLTS: f64 = 100.0;

/// Above this a current is not a laptop battery's (a charger gives at most a few amperes).
const MAX_AMPS: f64 = 100.0;

/// The range, in degrees Celsius, in which a battery temperature is believed.
const TEMPERATURE_RANGE: (f64, f64) = (-40.0, 120.0);

/// Whether and how the charge is limited. Several independent mechanisms can limit it, and a
/// machine may have any of them, so each is reported and [`limited`](Self::limited) is the
/// summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChargeLimit {
    /// The kernel's generic `charge_control_end_threshold`: charging stops at this percentage.
    pub end_threshold_pct: Option<u8>,
    /// `charge_control_start_threshold`: charging resumes below this percentage.
    pub start_threshold_pct: Option<u8>,
    /// The firmware's fixed 80 % cap ("health mode") as the Acer battery driver exposes it.
    pub health_mode: Option<bool>,
    /// The firmware's calibration cycle, from the same driver.
    pub calibration_mode: Option<bool>,
    /// TrueSense's own firmware interface (`get_battery 1`). The reader cannot ask the
    /// firmware; the application layer fills it in with [`ChargeLimit::with_firmware_limit`].
    pub firmware_limit_80: Option<bool>,
    /// The summary: `Some(true)` when any mechanism is limiting, `Some(false)` when at least one
    /// reported and none is limiting, `None` when nothing could be read.
    pub limited: Option<bool>,
}

impl ChargeLimit {
    /// This state with the firmware interface's answer added and the summary recomputed.
    #[must_use]
    pub fn with_firmware_limit(mut self, on: Option<bool>) -> Self {
        self.firmware_limit_80 = on;
        self.limited = self.summary();
        self
    }

    fn summary(&self) -> Option<bool> {
        let votes = [self.end_threshold_pct.map(|t| t < 100), self.health_mode, self.firmware_limit_80];
        let known: Vec<bool> = votes.into_iter().flatten().collect();
        if known.is_empty() { None } else { Some(known.contains(&true)) }
    }
}

/// One battery. Every field is `None` when the kernel does not give it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BatteryInfo {
    /// The directory name, `BAT1`.
    pub name: String,
    /// The battery is in its bay (`present`); `None` when the driver has no such file.
    pub present: Option<bool>,
    /// Charge in percent (`capacity`).
    pub percent: Option<u8>,
    /// The kernel's coarse level word: `Normal`, `Low`, `Critical`, `Full`.
    pub capacity_level: Option<String>,
    /// The kernel's state word: `Charging`, `Discharging`, `Full`, `Not charging`, `Unknown`.
    pub status: Option<String>,
    /// Chemistry, `Li-ion`.
    pub technology: Option<String>,
    /// The cell maker's code, `LGC`.
    pub manufacturer: Option<String>,
    /// The pack's model, `AP21A8T`. The serial number is deliberately not read: it identifies a
    /// person's laptop and nothing on the page needs it.
    pub model: Option<String>,
    /// The present voltage in volts.
    pub voltage_v: Option<f64>,
    /// The design (minimum) voltage in volts.
    pub voltage_min_design_v: Option<f64>,
    /// The present current in milliamperes, as a magnitude.
    pub current_ma: Option<f64>,
    /// The power flowing in or out in watts, as a magnitude: `power_now` when the driver has it,
    /// else voltage times current.
    pub power_w: Option<f64>,
    /// The charge held now, in milliampere-hours (`charge_now`).
    pub charge_now_mah: Option<f64>,
    /// The charge held when full today.
    pub charge_full_mah: Option<f64>,
    /// The charge the pack held when new.
    pub charge_full_design_mah: Option<f64>,
    /// The energy held now, in watt-hours (`energy_now`).
    pub energy_now_wh: Option<f64>,
    /// The energy held when full today.
    pub energy_full_wh: Option<f64>,
    /// The energy the pack held when new.
    pub energy_full_design_wh: Option<f64>,
    /// Charge cycles (`cycle_count`). Some drivers always say 0; that is passed on as read.
    pub cycle_count: Option<u32>,
    /// How much of its original capacity the pack still holds: full over design, in percent.
    /// Not capped at 100: a new pack can hold a little more than its label.
    pub health_pct: Option<f64>,
    /// The pack's temperature in degrees Celsius, from the power-supply file, the battery's
    /// hwmon chip or the Acer driver, whichever exists first.
    pub temperature_c: Option<f64>,
}

impl BatteryInfo {
    /// A reading with nothing in it, which is what a directory without readable attributes gives.
    fn blank(name: &str) -> Self {
        Self { name: name.to_owned(), ..Self::default() }
    }

    fn has_data(&self) -> bool {
        *self != Self::blank(&self.name)
    }
}

/// Everything read about the machine's batteries at one moment.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BatteryReport {
    /// There is a battery in the machine: `battery` is `Some`.
    pub present: bool,
    /// The mains adapter is plugged in; `None` when the machine has no adapter entry to read.
    pub ac_online: Option<bool>,
    /// The first battery that is in its bay and gave at least one reading.
    pub battery: Option<BatteryInfo>,
    /// Any further batteries, in name order.
    pub others: Vec<BatteryInfo>,
    /// The charge-limit mechanisms.
    pub charge_limit: ChargeLimit,
}

impl fmt::Display for BatteryReport {
    /// One line for a person: `97 % Charging, 17.09 V, 12.3 W, health 73.8 %, 666 cycles`; the
    /// parts the kernel did not give are left out.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some(b) = &self.battery else { return f.write_str("no battery") };
        let mut parts = Vec::new();
        parts.push(match (b.percent, &b.status) {
            (Some(p), Some(s)) => format!("{p} % {s}"),
            (Some(p), None) => format!("{p} %"),
            (None, Some(s)) => s.clone(),
            (None, None) => b.name.clone(),
        });
        if let Some(v) = b.voltage_v {
            parts.push(format!("{v:.2} V"));
        }
        if let Some(w) = b.power_w {
            parts.push(format!("{w:.1} W"));
        }
        if let Some(h) = b.health_pct {
            parts.push(format!("health {h:.1} %"));
        }
        if let Some(c) = b.cycle_count {
            parts.push(format!("{c} cycles"));
        }
        f.write_str(&parts.join(", "))
    }
}

/// Reads batteries below a root.
#[derive(Debug, Clone)]
pub struct BatteryReader {
    root: PathBuf,
}

impl BatteryReader {
    /// A reader of the tree at `root` (`/` in production).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The real machine.
    pub fn system() -> Self {
        Self::new("/")
    }

    /// Reads everything now. It never fails: a machine without a battery gives a report with
    /// `present` false.
    pub fn read(&self) -> BatteryReport {
        let class = self.root.join(POWER_SUPPLY);
        let mut names: Vec<String> = fs::read_dir(&class)
            .map(|entries| {
                entries.filter_map(Result::ok).filter_map(|e| e.file_name().into_string().ok()).collect()
            })
            .unwrap_or_default();
        // Name order, so the answer does not depend on directory order.
        names.sort();

        let mut batteries = Vec::new();
        let mut mains = Vec::new();
        for name in &names {
            let dir = class.join(name);
            match raw(&dir, "type").as_deref() {
                Some("Mains") => mains.push(dir),
                // A game controller or a mouse lists itself with the type Battery too, with the
                // scope `Device`; the laptop's own packs are named BAT*.
                Some("Battery") | None if name.starts_with("BAT") => {
                    let info = self.read_battery(&dir, name);
                    if info.present != Some(false) && info.has_data() {
                        batteries.push(info);
                    }
                }
                _ => {}
            }
        }

        let ac_online = mains.iter().filter_map(|d| flag(d, "online")).reduce(|a, b| a || b);
        let mut batteries = batteries.into_iter();
        let battery = batteries.next();
        let charge_limit = self.charge_limit(battery.as_ref().map(|b| class.join(&b.name)));
        BatteryReport {
            present: battery.is_some(),
            ac_online,
            battery,
            others: batteries.collect(),
            charge_limit,
        }
    }

    fn read_battery(&self, dir: &Path, name: &str) -> BatteryInfo {
        let voltage_v = number(dir, "voltage_now").map(|uv| uv / 1e6).filter(|v| *v > 0.0 && *v < MAX_VOLTS);
        let current_a = number(dir, "current_now").map(|ua| (ua / 1e6).abs()).filter(|a| *a <= MAX_AMPS);
        let power_w = number(dir, "power_now")
            .map(|uw| uw.abs() / 1e6)
            .filter(|w| *w <= MAX_VOLTS * MAX_AMPS)
            .or_else(|| Some(voltage_v? * current_a?));
        let wh = |file: &str| number(dir, file).filter(|v| *v >= 0.0).map(|uwh| uwh / 1e6);
        let mah = |file: &str| number(dir, file).filter(|v| *v >= 0.0).map(|uah| uah / 1e3);
        let (energy_full_wh, energy_design_wh) = (wh("energy_full"), wh("energy_full_design"));
        let (charge_full_mah, charge_design_mah) = (mah("charge_full"), mah("charge_full_design"));
        let health = |full: Option<f64>, design: Option<f64>| match (full, design) {
            (Some(f), Some(d)) if d > 0.0 => Some(f / d * 100.0),
            _ => None,
        };
        BatteryInfo {
            name: name.to_owned(),
            present: flag(dir, "present"),
            percent: number(dir, "capacity").filter(|c| (0.0..=100.0).contains(c)).map(|c| c as u8),
            capacity_level: text(dir, "capacity_level"),
            status: text(dir, "status"),
            technology: text(dir, "technology"),
            manufacturer: text(dir, "manufacturer"),
            model: text(dir, "model_name"),
            voltage_v,
            voltage_min_design_v: number(dir, "voltage_min_design")
                .map(|uv| uv / 1e6)
                .filter(|v| *v > 0.0 && *v < MAX_VOLTS),
            current_ma: current_a.map(|a| a * 1e3),
            power_w,
            charge_now_mah: mah("charge_now"),
            charge_full_mah,
            charge_full_design_mah: charge_design_mah,
            energy_now_wh: wh("energy_now"),
            energy_full_wh,
            energy_full_design_wh: energy_design_wh,
            cycle_count: number(dir, "cycle_count")
                .filter(|c| *c >= 0.0 && *c <= f64::from(u32::MAX))
                .map(|c| c as u32),
            health_pct: health(energy_full_wh, energy_design_wh)
                .or_else(|| health(charge_full_mah, charge_design_mah)),
            temperature_c: self.temperature(dir),
        }
    }

    /// The pack temperature: the power-supply file (tenths of a degree), the battery's own hwmon
    /// chip (thousandths), then the Acer driver (thousandths).
    fn temperature(&self, dir: &Path) -> Option<f64> {
        let in_range = |t: &f64| (TEMPERATURE_RANGE.0..=TEMPERATURE_RANGE.1).contains(t);
        number(dir, "temp")
            .map(|tenths| tenths / 10.0)
            .filter(in_range)
            .or_else(|| self.hwmon_temperature(dir).filter(in_range))
            .or_else(|| {
                number(&self.root.join(ACER_BATTERY), "temperature")
                    .map(|milli| milli / 1000.0)
                    .filter(in_range)
            })
    }

    fn hwmon_temperature(&self, dir: &Path) -> Option<f64> {
        let mut chips: Vec<PathBuf> = fs::read_dir(dir)
            .ok()?
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_str().is_some_and(|n| n.starts_with("hwmon")))
            .map(|e| e.path())
            .collect();
        chips.sort();
        chips.iter().find_map(|chip| number(chip, "temp1_input")).map(|milli| milli / 1000.0)
    }

    fn charge_limit(&self, battery_dir: Option<PathBuf>) -> ChargeLimit {
        let percent =
            |dir: &Path, file: &str| number(dir, file).filter(|v| (0.0..=100.0).contains(v)).map(|v| v as u8);
        let acer = self.root.join(ACER_BATTERY);
        // The driver creates `health_mode` whether or not the firmware implements it and reports
        // -1 when it does not, which `flag` turns into "unknown".
        let health_mode =
            flag(&acer, "health_mode").or_else(|| flag(&self.root.join(ACER_LIMITER), "battery_limiter"));
        let mut limit = ChargeLimit {
            end_threshold_pct: battery_dir
                .as_deref()
                .and_then(|d| percent(d, "charge_control_end_threshold")),
            start_threshold_pct: battery_dir
                .as_deref()
                .and_then(|d| percent(d, "charge_control_start_threshold")),
            health_mode,
            calibration_mode: flag(&acer, "calibration_mode"),
            firmware_limit_80: None,
            limited: None,
        };
        limit.limited = limit.summary();
        limit
    }
}

/// The trimmed text of an attribute, or `None` when it is missing, empty, not UTF-8 or too big.
fn raw(dir: &Path, name: &str) -> Option<String> {
    let bytes = fsx::read_limited(&dir.join(name), ATTRIBUTE_LIMIT).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// A vendor string: control characters dropped, cut to [`TEXT_LIMIT`] characters.
fn text(dir: &Path, name: &str) -> Option<String> {
    let cleaned: String = raw(dir, name)?.chars().filter(|c| !c.is_control()).take(TEXT_LIMIT).collect();
    let cleaned = cleaned.trim();
    (!cleaned.is_empty()).then(|| cleaned.to_owned())
}

/// A number, as the kernel prints integers; anything else (including a float that is not
/// finite) is `None`.
fn number(dir: &Path, name: &str) -> Option<f64> {
    raw(dir, name)?.parse::<i64>().ok().map(|v| v as f64)
}

/// A `0`/`1` attribute; `-1` (the Acer driver's "the firmware has no such function") and any
/// other value is `None`.
fn flag(dir: &Path, name: &str) -> Option<bool> {
    match raw(dir, name)?.as_str() {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
