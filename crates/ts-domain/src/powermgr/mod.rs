//! powermgr: detecting and handling the other power managers (auto-cpufreq, TLP, tuned,
//! tuned-ppd, power-profiles-daemon, system76-power).
//!
//! Those services change CPU governors, turbo and power limits behind TrueSense's back. "TrueSense
//! manages power" (off by default) stops and disables the ones that are running, remembers exactly
//! what it did, puts the CPU back to the kernel defaults (governor `powersave`, energy preference
//! `balance_performance`, turbo on: a manager that was stopped half way can leave turbo OFF, which
//! caps the CPU at its base clock), and [`PowerManagers::restore`] undoes all of it.
//!
//! Safety:
//!
//! * only the fixed list [`UNITS`] is ever stopped, started, enabled or disabled: a hand-edited
//!   record can never make this module touch another service;
//! * what was stopped, and the CPU settings before the first change, are written down **before**
//!   anything is stopped; if the record cannot be written nothing is changed;
//! * a second takeover keeps the ORIGINAL state to go back to;
//! * every CPU setting is read back, and what did not take is reported.
//!
//! Every effect is injected: `systemctl` runs through `hw::CommandRunner`, the CPU tree through
//! [`Sysfs`], and the record is a file at an injected path.
#![forbid(unsafe_code)]

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::{Map, Value, json};
use ts_foundation::fsx::{self, FsError};
use ts_platform::hw::CommandRunner;

use crate::power::Sysfs;

/// The only services this module ever stops, starts, enables or disables.
pub const UNITS: [&str; 6] =
    ["auto-cpufreq", "tlp", "tuned", "tuned-ppd", "power-profiles-daemon", "system76-power"];

/// Where a takeover remembers what it stopped, so that [`PowerManagers::restore`] can undo it
/// across restarts of the daemon.
pub(crate) const DEFAULT_RECORD_PATH: &str = "/var/lib/truesense/power_managers.json";
/// The kernel's CPU directory.
pub const DEFAULT_CPU_DIR: &str = "/sys/devices/system/cpu";

/// How long one `systemctl` call may take.
const SYSTEMCTL_TIMEOUT: Duration = Duration::from_secs(30);
/// How much of `systemctl`'s complaint goes into a report.
const MAX_FAILURE_TEXT: usize = 80;
/// The record is a few hundred bytes; a bigger file is not ours.
const MAX_RECORD: u64 = 64 * 1024;

/// The record path: `TRUESENSE_PM_RECORD` when set (tests, unusual installs), else
/// [`DEFAULT_RECORD_PATH`]. The environment is passed in so that tests can inject one.
pub fn record_path_from(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    env("TRUESENSE_PM_RECORD").map_or_else(|| PathBuf::from(DEFAULT_RECORD_PATH), PathBuf::from)
}

/// [`record_path_from`] for the running process.
pub fn record_path() -> PathBuf {
    record_path_from(&fsx::env_nonempty)
}

fn is_known_unit(name: &str) -> bool {
    UNITS.contains(&name)
}

/// A known power manager that is running or enabled. Field names are the Python dict keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnitStatus {
    /// The unit name, without `.service`.
    pub unit: String,
    /// Whether it is running.
    pub active: bool,
    /// Whether it starts at boot.
    pub enabled: bool,
}

/// The turbo switch, governor and energy preference as they are now (cpu0 stands for all cores).
/// `None` could not be read. Field names are the Python dict keys.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct CpuState {
    /// `intel_pstate/no_turbo`: `"1"` means turbo is OFF.
    pub no_turbo: Option<String>,
    /// `scaling_governor`.
    pub governor: Option<String>,
    /// `energy_performance_preference`.
    pub epp: Option<String>,
}

impl CpuState {
    /// What the CPU is put back to when a manager was in charge.
    fn defaults() -> Self {
        Self {
            no_turbo: Some("0".into()),
            governor: Some("powersave".into()),
            epp: Some("balance_performance".into()),
        }
    }

    /// Reads a remembered state, ignoring any value that is not text: a hand-edited record must
    /// not make the whole record unreadable. `None` when the value is not an object.
    fn from_value(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        let text = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_owned);
        Some(Self { no_turbo: text("no_turbo"), governor: text("governor"), epp: text("epp") })
    }
}

/// What a takeover changed about one manager, so that restore can put it back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordUnit {
    /// The unit name, without `.service`.
    pub unit: String,
    /// Whether it was running before.
    pub was_active: bool,
    /// Whether it started at boot before.
    pub was_enabled: bool,
}

/// Everything a takeover changed, kept on disk so that restore undoes exactly that.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Record {
    /// The CPU state to go back to, taken before the first change; `None` when nothing is recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<CpuState>,
    /// The managers that were stopped.
    pub units: Vec<RecordUnit>,
}

fn truthy(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        _ => false,
    }
}

impl Record {
    /// Whether nothing is recorded (a missing, unreadable or empty record).
    pub fn is_empty(&self) -> bool {
        self.cpu.is_none() && self.units.is_empty()
    }

    /// Reads a record, skipping malformed entries instead of rejecting the whole record: what is
    /// readable is still worth restoring. Anything that is not an object is an empty record.
    pub fn from_value(value: &Value) -> Record {
        let Some(object) = value.as_object() else { return Record::default() };
        let cpu = object.get("cpu").and_then(CpuState::from_value);
        let units = object
            .get("units")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(Value::as_object)
                    .filter_map(|e| {
                        let unit = e.get("unit")?.as_str().filter(|u| !u.is_empty())?;
                        Some(RecordUnit {
                            unit: unit.to_owned(),
                            was_active: truthy(e.get("was_active")),
                            was_enabled: truthy(e.get("was_enabled")),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Record { cpu, units }
    }
}

/// What [`PowerManagers::takeover`] did. Field names are the Python dict keys; the daemon adds `on`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TakeoverReport {
    /// The managers that were running or enabled.
    pub found: Vec<String>,
    /// The ones that were stopped and disabled.
    pub stopped: Vec<String>,
    /// The ones that could not be stopped, with the reason.
    pub failed: Vec<String>,
    /// CPU settings that did not take (`turbo`, `governor`, `epp`, `ceiling cpuN`).
    pub cpu_not_set: Vec<String>,
    /// The CPU now.
    pub cpu: CpuState,
    /// Whether every core's frequency ceiling is at its maximum.
    pub ceiling_fixed: bool,
}

/// What [`PowerManagers::restore`] did. Serialised like the Python edition: `restored`,
/// `cpu_not_set` and `cpu`, or `restored`, `cpu` and `note` when there was nothing to restore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreReport {
    /// The managers that were started again.
    pub restored: Vec<String>,
    /// CPU settings that did not take.
    pub cpu_not_set: Vec<String>,
    /// The CPU now.
    pub cpu: CpuState,
    /// Set when nothing was on record.
    pub note: Option<String>,
}

impl Serialize for RestoreReport {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = Map::new();
        map.insert("restored".into(), json!(self.restored));
        match &self.note {
            Some(note) => {
                map.insert("cpu".into(), json!(self.cpu));
                map.insert("note".into(), json!(note));
            }
            None => {
                map.insert("cpu_not_set".into(), json!(self.cpu_not_set));
                map.insert("cpu".into(), json!(self.cpu));
            }
        }
        Value::Object(map).serialize(serializer)
    }
}

/// Why a takeover did not start.
#[derive(Debug)]
pub enum PowerMgrError {
    /// The record could not be written, so nothing was changed: a takeover that cannot be undone
    /// is not started.
    RecordNotSaved(FsError),
}

impl fmt::Display for PowerMgrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RecordNotSaved(e) => {
                write!(f, "could not remember what was stopped ({e}); nothing was changed")
            }
        }
    }
}

impl std::error::Error for PowerMgrError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::RecordNotSaved(e) => Some(e),
        }
    }
}

/// Detection and takeover of power managers over injected dependencies.
pub struct PowerManagers {
    runner: Arc<dyn CommandRunner>,
    sys: Arc<dyn Sysfs>,
    cpu_dir: PathBuf,
    record: PathBuf,
}

impl PowerManagers {
    /// Power managers handled with `runner` (for `systemctl`), over the CPU tree at `cpu_dir`
    /// ([`DEFAULT_CPU_DIR`]) read through `sys`, remembering its work in the file `record`.
    pub fn new(
        runner: Arc<dyn CommandRunner>,
        sys: Arc<dyn Sysfs>,
        cpu_dir: impl Into<PathBuf>,
        record: impl Into<PathBuf>,
    ) -> Self {
        Self { runner, sys, cpu_dir: cpu_dir.into(), record: record.into() }
    }

    /// The text `systemctl` printed, trimmed; empty when it could not run.
    fn systemctl(&self, args: &[&str]) -> (bool, String) {
        match self.runner.output("systemctl", args, SYSTEMCTL_TIMEOUT) {
            Ok(out) => (true, String::from_utf8_lossy(&out).trim().to_owned()),
            Err(e) => (false, e.to_string()),
        }
    }

    fn is_active(&self, unit: &str) -> bool {
        self.systemctl(&["is-active", &format!("{unit}.service")]).1 == "active"
    }

    /// Every known manager that is running or enabled.
    pub fn status(&self) -> Vec<UnitStatus> {
        UNITS
            .iter()
            .filter_map(|unit| {
                let active = self.is_active(unit);
                let enabled = self.systemctl(&["is-enabled", &format!("{unit}.service")]).1 == "enabled";
                (active || enabled).then(|| UnitStatus { unit: (*unit).to_owned(), active, enabled })
            })
            .collect()
    }

    fn read(&self, path: &Path) -> Option<String> {
        self.sys.read(path).ok().map(|t| t.trim().to_owned())
    }

    fn turbo_path(&self) -> PathBuf {
        self.cpu_dir.join("intel_pstate/no_turbo")
    }

    /// Turbo switch, governor and energy preference as they are now.
    pub fn cpu_state(&self) -> CpuState {
        let cpu0 = self.cpu_dir.join("cpu0/cpufreq");
        CpuState {
            no_turbo: self.read(&self.turbo_path()),
            governor: self.read(&cpu0.join("scaling_governor")),
            epp: self.read(&cpu0.join("energy_performance_preference")),
        }
    }

    /// The `cpuN` directories, in order (`cpu[0-9]*`: `cpufreq` and `cpuidle` are not cores).
    fn cores(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .sys
            .list(&self.cpu_dir)
            .unwrap_or_default()
            .into_iter()
            .filter(|n| {
                n.strip_prefix("cpu").and_then(|rest| rest.chars().next()).is_some_and(|c| c.is_ascii_digit())
            })
            .collect();
        names.sort_by_key(|n| (n.len(), n.clone())); // cpu2 before cpu10
        names
    }

    /// The per-core files called `file` that exist.
    fn core_files(&self, file: &str) -> Vec<PathBuf> {
        self.cores()
            .iter()
            .map(|c| self.cpu_dir.join(c).join("cpufreq").join(file))
            .filter(|p| self.sys.read(p).is_ok())
            .collect()
    }

    /// Writes governor / energy preference to every core and the turbo switch, then reads each
    /// back. Returns what could not be set.
    fn set_cpu(&self, wanted: &CpuState) -> Vec<String> {
        let mut bad = Vec::new();
        let turbo = self.turbo_path();
        if let Some(want) = &wanted.no_turbo {
            if self.read(&turbo).is_some() {
                let _ = self.sys.write(&turbo, want);
                if self.read(&turbo).as_deref() != Some(want.as_str()) {
                    bad.push("turbo".to_owned());
                }
            }
        }
        for (key, file, want) in [
            ("governor", "scaling_governor", &wanted.governor),
            ("epp", "energy_performance_preference", &wanted.epp),
        ] {
            let Some(want) = want else { continue };
            let files = self.core_files(file);
            for path in &files {
                let _ = self.sys.write(path, want);
            }
            if files.iter().any(|p| self.read(p).as_deref() != Some(want.as_str())) {
                bad.push(key.to_owned());
            }
        }
        bad
    }

    /// The cores (their `cpufreq` directories) whose frequency ceiling (`scaling_max_freq`) is below
    /// what the core can do. Turning turbo off makes the kernel lower the ceiling, and turning it on
    /// again does not raise it.
    pub fn ceiling_low(&self) -> Vec<PathBuf> {
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        self.core_files("cpuinfo_max_freq")
            .into_iter()
            .filter_map(|max_file| {
                let dir = max_file.parent()?.to_path_buf();
                let hw = self.read(&max_file)?;
                let cur = self.read(&dir.join("scaling_max_freq"))?;
                // A sign or text would be accepted by a lenient parser; sysfs only has digits.
                (digits(&hw) && digits(&cur) && cur.parse::<u64>().ok()? < hw.parse::<u64>().ok()?)
                    .then_some(dir)
            })
            .collect()
    }

    /// Raises every low ceiling to the core's own maximum (what the service also does when it
    /// stops), read back. Returns the cores that did not take it.
    pub fn fix_ceiling(&self) -> Vec<String> {
        let mut bad = Vec::new();
        for dir in self.ceiling_low() {
            let want = self.read(&dir.join("cpuinfo_max_freq"));
            let current = dir.join("scaling_max_freq");
            if let Some(want) = &want {
                let _ = self.sys.write(&current, want);
            }
            if want.is_none() || self.read(&current) != want {
                bad.push(
                    dir.parent()
                        .and_then(Path::file_name)
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
                );
            }
        }
        bad
    }

    /// The record on disk; a missing, unreadable or invalid file is an empty record.
    pub fn load_record(&self) -> Record {
        fsx::read_limited(&self.record, MAX_RECORD)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .map_or_else(Record::default, |v| Record::from_value(&v))
    }

    /// Whether a takeover is on record (there is something to restore).
    pub fn recorded(&self) -> bool {
        !self.load_record().is_empty()
    }

    fn save_record(&self, record: &Record) -> Result<(), PowerMgrError> {
        let json = serde_json::to_vec(record).map_err(|e| {
            PowerMgrError::RecordNotSaved(FsError::Io {
                op: "encode",
                path: self.record.clone(),
                source: std::io::Error::other(e.to_string()),
            })
        })?;
        fsx::write_atomic_mkdir(&self.record, &json, 0o644).map_err(PowerMgrError::RecordNotSaved)
    }

    /// Stops and disables every running or enabled manager from the list, fixes the CPU defaults,
    /// and remembers it all. The record is written before anything is stopped, so a crash half way
    /// can still be undone; if it cannot be written nothing is changed.
    pub fn takeover(&self) -> Result<TakeoverReport, PowerMgrError> {
        let found = self.status();
        let mut record = self.load_record();
        if record.cpu.is_none() {
            record.cpu = Some(self.cpu_state()); // the state to go back to, before the first change
        }
        for f in &found {
            if !record.units.iter().any(|u| u.unit == f.unit) {
                record.units.push(RecordUnit {
                    unit: f.unit.clone(),
                    was_active: f.active,
                    was_enabled: f.enabled,
                });
            }
        }
        self.save_record(&record)?; // remembered BEFORE anything is stopped

        let (mut stopped, mut failed) = (Vec::new(), Vec::new());
        for f in &found {
            let (_, message) = self.systemctl(&["disable", "--now", &format!("{}.service", f.unit)]);
            // The runner gives no exit status, so "did it work" is asked of systemd itself.
            if self.is_active(&f.unit) {
                let text = if message.is_empty() { "it is still running".to_owned() } else { message };
                failed.push(format!(
                    "{}: {}",
                    f.unit,
                    text.chars().take(MAX_FAILURE_TEXT).collect::<String>()
                ));
            } else {
                stopped.push(f.unit.clone());
            }
        }
        // Defaults only when a manager was in charge or turbo was left off.
        let mut cpu_not_set = if !found.is_empty() || self.cpu_state().no_turbo.as_deref() == Some("1") {
            self.set_cpu(&CpuState::defaults())
        } else {
            Vec::new()
        };
        // A ceiling left low by turbo being off is raised whatever the reason.
        cpu_not_set.extend(self.fix_ceiling().into_iter().map(|c| format!("ceiling {c}")));
        Ok(TakeoverReport {
            found: found.into_iter().map(|f| f.unit).collect(),
            stopped,
            failed,
            cpu_not_set,
            cpu: self.cpu_state(),
            ceiling_fixed: self.ceiling_low().is_empty(),
        })
    }

    /// Starts the managers again exactly as they were and puts the CPU settings back. A unit that
    /// is not on the fixed list is never touched, whatever the record says. The record is removed
    /// afterwards.
    pub fn restore(&self) -> RestoreReport {
        let record = self.load_record();
        if record.is_empty() {
            return RestoreReport {
                restored: Vec::new(),
                cpu_not_set: Vec::new(),
                cpu: self.cpu_state(),
                note: Some("nothing was changed by TrueSense".into()),
            };
        }
        let mut restored = Vec::new();
        for unit in record.units.iter().filter(|u| is_known_unit(&u.unit)) {
            if unit.was_enabled {
                self.systemctl(&["enable", &format!("{}.service", unit.unit)]);
            }
            if unit.was_active {
                self.systemctl(&["start", &format!("{}.service", unit.unit)]);
            }
            restored.push(unit.unit.clone());
        }
        let cpu_not_set = record.cpu.as_ref().map_or_else(Vec::new, |cpu| self.set_cpu(cpu));
        // A stale record only makes the next restore a harmless repeat, so a failure to remove it
        // is not worth failing the restore.
        let _ = std::fs::remove_file(&self.record);
        RestoreReport { restored, cpu_not_set, cpu: self.cpu_state(), note: None }
    }
}

#[cfg(test)]
mod tests;
