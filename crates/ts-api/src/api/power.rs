//! Power: the mode, the hold that keeps a hand-picked mode against the automatic profile, the
//! automatic mode, the game rules and the other power managers.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_ipc::client::ClientError;
use ts_ipc::wire::Args;
use ts_platform::hw::{Coded, PowerMode, Reading};

use super::{Api, ApiError, ApiResult};

/// The power mode and what the service adds when it runs: whether the automatic profile is on and
/// how long a hand-picked mode is still held. A missing service is not an error here.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PowerStatus {
    /// The firmware's power mode (`unknown(7)` for a code this edition has no name for).
    pub mode: Reading<PowerMode>,
    /// The modes this laptop's firmware accepts.
    pub supported: Vec<PowerMode>,
    /// Whether the automatic profile is on; absent without the service.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_profile: Option<bool>,
    /// Seconds a hand-picked mode is still held; absent without the service.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold_left_s: Option<i64>,
}

impl fmt::Display for PowerStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "power mode: {}", self.mode)?;
        if let Some(auto) = self.auto_profile {
            write!(f, "   (automatic profile {}", super::on_off(auto))?;
            if let Some(left) = self.hold_left_s.filter(|s| *s > 0) {
                write!(f, ", manual choice held for {} more min", left / 60)?;
            }
            f.write_str(")")?;
        }
        Ok(())
    }
}

/// How the power mode is chosen automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoMode {
    /// You choose.
    Off,
    /// The power source decides, like PredatorSense on Windows (AC balanced, battery eco).
    Native,
    /// Heat, load and battery aware.
    Smart,
}

impl AutoMode {
    /// The word the daemon's `auto_mode` setting uses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Native => "native",
            Self::Smart => "smart",
        }
    }
}

impl FromStr for AutoMode {
    type Err = ApiError;

    fn from_str(text: &str) -> Result<Self, ApiError> {
        match text {
            "off" => Ok(Self::Off),
            "native" => Ok(Self::Native),
            "smart" => Ok(Self::Smart),
            _ => Err(ApiError::invalid("auto_mode: off | native | smart")),
        }
    }
}

impl fmt::Display for AutoMode {
    /// The sentence `power auto` prints.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Off => "automatic power mode: off (you choose)",
            Self::Native => {
                "automatic power mode: native (power source only, like PredatorSense: AC balanced, battery eco)"
            }
            Self::Smart => "automatic power mode: smart (heat, load and battery aware)",
        })
    }
}

/// What `power auto ACTION` can be asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoAction {
    /// Set the mode.
    Set(AutoMode),
    /// `on` is the smart mode.
    On,
    /// Off when on, smart when off.
    Toggle,
    /// Only read it.
    Status,
}

impl FromStr for AutoAction {
    type Err = ApiError;

    fn from_str(text: &str) -> Result<Self, ApiError> {
        match text {
            "on" => Ok(Self::On),
            "toggle" => Ok(Self::Toggle),
            "status" | "" => Ok(Self::Status),
            other => other.parse().map(Self::Set),
        }
    }
}

/// Something done to the game rules (a program raises the power mode while it runs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameRuleAction {
    /// Show them.
    List,
    /// Raise the mode while `program` runs.
    Add {
        /// The program's name.
        program: String,
        /// The mode it asks for.
        mode: PowerMode,
    },
    /// Forget one.
    Remove {
        /// The program's name.
        program: String,
    },
    /// Forget all.
    Clear,
}

impl GameRuleAction {
    /// The action as the command line gives it (`list`, `add NAME MODE`, `remove NAME`, `clear`),
    /// with the usage message of the Python edition for a missing argument.
    pub fn parse(action: &str, name: Option<&str>, mode: Option<&str>) -> ApiResult<Self> {
        let name = name.filter(|n| !n.is_empty());
        match action {
            "list" | "" => Ok(Self::List),
            "clear" => Ok(Self::Clear),
            "add" => match (name, mode.filter(|m| !m.is_empty())) {
                (Some(program), Some(mode)) => {
                    Ok(Self::Add { program: program.to_owned(), mode: mode.parse()? })
                }
                _ => Err(ApiError::invalid(
                    "usage: truesense power game add PROGRAM MODE   (e.g. add eldenring.exe turbo)",
                )),
            },
            "remove" => name
                .map(|program| Self::Remove { program: program.to_owned() })
                .ok_or_else(|| ApiError::invalid("usage: truesense power game remove PROGRAM")),
            other => {
                Err(ApiError::invalid(format!("game rules: list | add | remove | clear (got '{other}')")))
            }
        }
    }
}

/// The program-to-power-mode rules and the mode a running program asks for now.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GameRules {
    /// Program name to power mode.
    pub rules: BTreeMap<String, String>,
    /// The mode a running program asks for now.
    pub running: Option<String>,
}

impl fmt::Display for GameRules {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.rules.is_empty() {
            f.write_str("  (no game rules)")?;
        }
        for (i, (name, mode)) in self.rules.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "  {name:<28} {mode}")?;
        }
        if let Some(mode) = &self.running {
            write!(f, "\n  running now: {mode} mode")?;
        }
        Ok(())
    }
}

/// One other power manager that runs or is enabled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagerUnit {
    /// The unit name.
    pub unit: String,
    /// Whether it runs.
    pub active: bool,
    /// Whether it starts at boot.
    pub enabled: bool,
}

/// The CPU settings TrueSense sets when it manages power. A value the kernel does not offer is absent.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CpuSettings {
    /// `no_turbo`: `0` is turbo on.
    pub no_turbo: Option<String>,
    /// The scaling governor.
    pub governor: Option<String>,
    /// The energy performance preference.
    pub epp: Option<String>,
}

impl CpuSettings {
    fn describe(&self) -> String {
        let turbo = match self.no_turbo.as_deref() {
            Some("0") => "on",
            Some("") | None => "n/a",
            Some(_) => "OFF",
        };
        format!(
            "governor {}, energy preference {}, turbo {turbo}",
            self.governor.as_deref().unwrap_or("?"),
            self.epp.as_deref().unwrap_or("?")
        )
    }
}

/// The other power managers that run or are enabled, and whether TrueSense manages power.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PowerManagersState {
    /// TrueSense manages power (it stopped the others).
    pub on: bool,
    /// The managers found.
    pub found: Vec<ManagerUnit>,
    /// What was stopped is written down, so `restore` can undo it.
    pub recorded: bool,
}

impl fmt::Display for PowerManagersState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TrueSense manages power: {}", if self.on { "ON" } else { "off" })?;
        if self.found.is_empty() {
            f.write_str("\n  no other power manager is running or enabled")?;
        }
        for m in &self.found {
            write!(
                f,
                "\n  {:<24} {}, {}",
                m.unit,
                if m.active { "running" } else { "not running" },
                if m.enabled { "enabled" } else { "disabled" }
            )?;
        }
        Ok(())
    }
}

/// What "TrueSense manages power" did.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PowerManagersStopped {
    /// The managers that were running or enabled.
    pub found: Vec<String>,
    /// The ones that were stopped.
    pub stopped: Vec<String>,
    /// The ones that could not be stopped.
    pub failed: Vec<String>,
    /// CPU settings the kernel does not offer.
    pub cpu_not_set: Vec<String>,
    /// The CPU settings now.
    pub cpu: CpuSettings,
    /// A leftover frequency ceiling was lifted.
    pub ceiling_fixed: bool,
    /// TrueSense manages power.
    pub on: bool,
}

impl fmt::Display for PowerManagersStopped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.stopped.is_empty() {
            f.write_str("stopped: nothing (no other power manager was running)")?;
        } else {
            write!(f, "stopped: {}", self.stopped.join(", "))?;
        }
        if !self.failed.is_empty() {
            write!(f, "\nCOULD NOT STOP: {}", self.failed.join(", "))?;
        }
        write!(f, "\nCPU now: {}\n(undo: truesense power managers restore)", self.cpu.describe())
    }
}

/// What handing power back did.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PowerManagersRestored {
    /// The managers started again.
    pub restored: Vec<String>,
    /// CPU settings the kernel does not offer (absent when there was a note instead).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cpu_not_set: Vec<String>,
    /// The CPU settings now.
    pub cpu: CpuSettings,
    /// Why nothing was restored, when nothing was recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// TrueSense manages power.
    pub on: bool,
}

impl fmt::Display for PowerManagersRestored {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.restored.is_empty() {
            f.write_str("restored: nothing (TrueSense had not stopped anything)")
        } else {
            write!(f, "restored: {}", self.restored.join(", "))
        }
    }
}

/// How long a hand-picked power mode is held; `None` means no hold. It serialises as its message,
/// which is what the Python edition printed for `--json`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hold {
    /// Minutes the hold lasts from now.
    pub minutes: Option<f64>,
}

impl Serialize for Hold {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl fmt::Display for Hold {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.minutes {
            Some(m) => write!(f, "holding the current power mode for {m} min"),
            None => f.write_str("hold released: the automatic profile is in charge again"),
        }
    }
}

impl Api {
    /// Reads the power mode from the firmware and, if the service answers, its automatic profile
    /// switch and hold.
    pub fn power_status(&self) -> ApiResult<PowerStatus> {
        let mode = self.s.hw.get_mode()?;
        let supported = self.s.hw.supported_modes()?;
        let mut st = PowerStatus { mode, supported, auto_profile: None, hold_left_s: None };
        if let Ok(d) = self.daemon_state() {
            st.auto_profile = Some(d.setting_bool("auto_profile").unwrap_or(false));
            st.hold_left_s = Some(d.hold_left_s);
        }
        Ok(st)
    }

    /// The modes this laptop's firmware accepts.
    pub fn power_modes(&self) -> ApiResult<Vec<PowerMode>> {
        Ok(self.s.hw.supported_modes()?)
    }

    /// Switches the power mode and, with `hold_minutes`, holds it against the automatic profile.
    /// Through the service when it runs (one request, no settle delay); on the hardware otherwise,
    /// where the hold is a best effort (there is nothing to hold it against without the service).
    /// A service too old to know `power` is skipped the same way.
    pub fn set_power(&self, mode: PowerMode, hold_minutes: Option<f64>) -> ApiResult<PowerMode> {
        if self.daemon_available() {
            let args = Args::new().with("mode", mode.name()).with("hold_minutes", hold_value(hold_minutes));
            match self.s.daemon.call("power", args) {
                Ok(answer) => {
                    return Ok(answer.as_str().and_then(|n| PowerMode::from_name(n).ok()).unwrap_or(mode));
                }
                Err(e) if e.is_unknown_command() => {}
                Err(e) => return Err(e.into()),
            }
        }
        let new = self.s.hw.set_mode(mode)?;
        if let Some(minutes) = hold_minutes.filter(|m| *m != 0.0) {
            // A hold without a service is not an error: there is no automatic profile to hold against.
            let _: Result<Value, ClientError> =
                self.s.daemon.call("hold", Args::new().with("minutes", minutes));
        }
        Ok(new)
    }

    /// Switches to the next mode of the configured cycle (the PREDATOR key's `power-next`) and
    /// answers the message the key would show.
    pub fn next_power(&self) -> ApiResult<String> {
        self.run_action("power-next")
    }

    /// [`Api::next_power`] in the other direction.
    pub fn prev_power(&self) -> ApiResult<String> {
        self.run_action("power-prev")
    }

    /// Keeps the current power mode against the automatic profile for `minutes`.
    pub fn hold_power(&self, minutes: f64) -> ApiResult<Hold> {
        self.ask_value("hold", Args::new().with("minutes", minutes))?;
        Ok(Hold { minutes: Some(minutes) })
    }

    /// Ends a hold: the automatic profile is in charge again.
    pub fn release_hold(&self) -> ApiResult<Hold> {
        self.ask_value("hold", Args::new().with("minutes", Value::Null))?;
        Ok(Hold { minutes: None })
    }

    /// The automatic power mode the service runs. Older services only have the `auto_profile`
    /// switch: on means smart.
    pub fn auto_mode(&self) -> ApiResult<AutoMode> {
        let d = self.daemon_state()?;
        if let Some(mode) = d.setting_str("auto_mode").and_then(|m| m.parse().ok()) {
            return Ok(mode);
        }
        Ok(if d.setting_bool("auto_profile").unwrap_or(false) { AutoMode::Smart } else { AutoMode::Off })
    }

    /// Sets the automatic power mode (the service takes over at once for native and smart).
    pub fn set_auto_mode(&self, mode: AutoMode) -> ApiResult<()> {
        self.daemon_set("auto_mode", mode.as_str())?;
        Ok(())
    }

    /// `power auto ACTION`: sets, toggles or reads the automatic mode and answers the mode in force.
    pub fn apply_auto_mode(&self, action: AutoAction) -> ApiResult<AutoMode> {
        let current = self.auto_mode()?;
        let want = match action {
            AutoAction::Set(m) => m,
            AutoAction::On => AutoMode::Smart,
            AutoAction::Status => current,
            AutoAction::Toggle if current == AutoMode::Off => AutoMode::Smart,
            AutoAction::Toggle => AutoMode::Off,
        };
        if want != current {
            self.set_auto_mode(want)?;
        }
        Ok(want)
    }

    /// Lists, adds, removes or clears the rules that raise the power mode while a program runs
    /// (needs the service).
    pub fn game_rules(&self, action: &GameRuleAction) -> ApiResult<GameRules> {
        let (verb, name, mode) = match action {
            GameRuleAction::List => ("list", None, None),
            GameRuleAction::Add { program, mode } => ("add", Some(program.as_str()), Some(mode.name())),
            GameRuleAction::Remove { program } => ("remove", Some(program.as_str()), None),
            GameRuleAction::Clear => ("clear", None, None),
        };
        let args = Args::new().with("action", verb).with("name", name).with("mode", mode);
        self.ask("game_rules", args)
    }

    /// Lists the other power managers (needs the service).
    pub fn power_managers(&self) -> ApiResult<PowerManagersState> {
        self.ask("power_managers", Args::new().with("action", "status"))
    }

    /// Makes TrueSense manage power: stops the other managers, sets sane CPU defaults and
    /// remembers it all so [`Api::restore_power_managers`] can undo it.
    pub fn stop_power_managers(&self) -> ApiResult<PowerManagersStopped> {
        self.ask("power_managers", Args::new().with("action", "takeover"))
    }

    /// Starts the other managers again exactly as they were.
    pub fn restore_power_managers(&self) -> ApiResult<PowerManagersRestored> {
        self.ask("power_managers", Args::new().with("action", "restore"))
    }
}

fn hold_value(minutes: Option<f64>) -> Value {
    minutes.map_or(Value::Null, Value::from)
}
