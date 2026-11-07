//! The desktop alerts' settings: reading and changing the `[alerts]` section of the configuration.
//!
//! The alerts themselves are raised by the root daemon (`ts_decision::alerts` rules, evaluated on
//! its tick), so they work with every window closed. This layer only edits the settings: the
//! change is written to the user's configuration file, validated first, and the daemon notices
//! the changed file within about ten seconds. There is no request to the daemon, so the settings
//! can be read and changed while it is not running (they apply when it starts).
#![forbid(unsafe_code)]

use serde::Deserialize;
use ts_foundation::config::{AlertsConfig, Value, section_table};

use super::{Api, ApiError, ApiResult};

/// A change to the settings: only the members that are given are written, so a key the user
/// never touched keeps following the built-in default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AlertsEdit {
    /// Raise alerts at all.
    pub enabled: Option<bool>,
    /// CPU limit in degrees C (70..100).
    pub cpu_limit_c: Option<i64>,
    /// GPU limit in degrees C (70..100).
    pub gpu_limit_c: Option<i64>,
    /// Least minutes between two messages of the same alert (1..240).
    pub repeat_minutes: Option<i64>,
    /// Consecutive samples a condition must hold (1..30).
    pub debounce_samples: Option<i64>,
    /// Battery alert level in percent (5..50, 0 = off).
    pub battery_low_pct: Option<i64>,
    /// Tell when the thermal governor holds the CPU clock down.
    pub throttling: Option<bool>,
    /// Tell when the CPU temperature cannot be read.
    pub sensor_lost: Option<bool>,
}

impl AlertsEdit {
    /// Whether nothing is asked for.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The members that are given, as the keys of the section.
    fn pairs(&self) -> Vec<(&'static str, Value)> {
        let mut out = Vec::new();
        let ints = [
            ("cpu_limit_c", self.cpu_limit_c),
            ("gpu_limit_c", self.gpu_limit_c),
            ("repeat_minutes", self.repeat_minutes),
            ("debounce_samples", self.debounce_samples),
            ("battery_low_pct", self.battery_low_pct),
        ];
        let flags =
            [("enabled", self.enabled), ("throttling", self.throttling), ("sensor_lost", self.sensor_lost)];
        out.extend(flags.into_iter().filter_map(|(k, v)| v.map(|b| (k, Value::Boolean(b)))));
        out.extend(ints.into_iter().filter_map(|(k, v)| v.map(|n| (k, Value::Integer(n)))));
        out
    }

    /// `current` with this change applied.
    fn applied_to(&self, current: &AlertsConfig) -> AlertsConfig {
        AlertsConfig {
            enabled: self.enabled.unwrap_or(current.enabled),
            cpu_limit_c: self.cpu_limit_c.unwrap_or(current.cpu_limit_c),
            gpu_limit_c: self.gpu_limit_c.unwrap_or(current.gpu_limit_c),
            repeat_minutes: self.repeat_minutes.unwrap_or(current.repeat_minutes),
            debounce_samples: self.debounce_samples.unwrap_or(current.debounce_samples),
            battery_low_pct: self.battery_low_pct.unwrap_or(current.battery_low_pct),
            throttling: self.throttling.unwrap_or(current.throttling),
            sensor_lost: self.sensor_lost.unwrap_or(current.sensor_lost),
        }
    }
}

/// The settings as the lines `truesense alerts show` prints.
pub fn describe_alerts(a: &AlertsConfig) -> String {
    let on = |b: bool| if b { "on" } else { "off" };
    let battery = if a.battery_low_pct == 0 { "off".to_owned() } else { format!("{} %", a.battery_low_pct) };
    [
        format!("alerts: {}", on(a.enabled)),
        format!("  CPU limit      {} C (again after it drops 5 C)", a.cpu_limit_c),
        format!("  GPU limit      {} C (again after it drops 5 C)", a.gpu_limit_c),
        format!("  repeat         at most every {} min per alert", a.repeat_minutes),
        format!("  debounce       {} samples in a row (one or two seconds each)", a.debounce_samples),
        format!("  battery low    {battery}"),
        format!("  throttling     {}", on(a.throttling)),
        format!("  sensor lost    {}", on(a.sensor_lost)),
    ]
    .join("\n")
}

impl Api {
    /// The alert settings in force: the files over the defaults. A configuration file that does
    /// not parse is an error, because the daemon would be running on the defaults.
    pub fn alerts(&self) -> ApiResult<AlertsConfig> {
        Ok(self.load_config()?.alerts)
    }

    /// Changes the alert settings and answers the settings now in force. The result is checked
    /// before anything is written (a limit outside 70..100 C is refused with the allowed range),
    /// the file is rewritten atomically with its other keys kept, and the daemon picks the change
    /// up by itself within about ten seconds.
    pub fn set_alerts(&self, edit: &AlertsEdit) -> ApiResult<AlertsConfig> {
        if edit.is_empty() {
            return Err(ApiError::invalid("alerts set: nothing to change"));
        }
        let wanted = edit.applied_to(&self.alerts()?);
        let problems = wanted.validate();
        if !problems.is_empty() {
            return Err(ApiError::invalid(problems.join("; ")));
        }
        self.s.config.write_user(section_table("alerts", edit.pairs()))?;
        self.alerts()
    }
}
