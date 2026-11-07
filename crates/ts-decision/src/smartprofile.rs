//! Smart-auto profiles: the data model of how Smart auto chooses the power mode.
//!
//! A profile is the set of the `[smart]` settings: which mode to use on power and on battery,
//! how long a load must last before Turbo, which temperatures stop Turbo, how long to wait
//! between changes. The built-in profiles are read-only (copy one to make your own); the user's
//! own live in `~/.config/truesense/smart/NAME.json`, one file each. The profile in use is kept
//! by the daemon (in its state file), so it works before anyone logs in.
//!
//! This module only describes, validates and stores profiles. The decisions that use them are
//! made in [`crate::autoprofile`]. [`Mode`], the five power modes, lives here because a profile
//! is made of them and the other decision modules (game rules, the engine) share the type.
//!
//! [`validate`] is the one gate for untrusted input (the control socket and the profile files):
//! it never panics, ignores unknown keys, fills every missing setting from the defaults and
//! refuses a value that is the wrong type or outside its range, with the same messages as the
//! Python edition.
#![forbid(unsafe_code)]

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Serialize, Serializer};
use serde_json::Value;
use ts_foundation::fsx::{self, FsError};
use ts_foundation::names::is_valid as valid_name;

/// A firmware power mode, declared from the slowest and quietest to the fastest and loudest:
/// the derived order is the rank the automatic engine compares ("a game rule can only raise
/// the mode").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Mode {
    /// Lowest power, for battery.
    Eco,
    /// Quiet fans.
    Quiet,
    /// The firmware default.
    Balanced,
    /// Fast, louder.
    Performance,
    /// Fastest; backs off when hot.
    Turbo,
}

impl Mode {
    /// Every mode in rank order.
    pub const ALL: [Mode; 5] = [Mode::Eco, Mode::Quiet, Mode::Balanced, Mode::Performance, Mode::Turbo];

    /// The lower-case name used in config files, state and on the control socket.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Eco => "eco",
            Mode::Quiet => "quiet",
            Mode::Balanced => "balanced",
            Mode::Performance => "performance",
            Mode::Turbo => "turbo",
        }
    }

    /// The mode with that exact (lower-case) name. Case is not folded: the Python edition
    /// compared exactly too, and a stored `"Turbo"` is a typo worth refusing.
    pub fn parse(name: &str) -> Option<Mode> {
        Mode::ALL.into_iter().find(|m| m.name() == name)
    }

    /// Position in the rank order, 0 (eco) to 4 (turbo).
    pub fn rank(self) -> u8 {
        self as u8
    }

    /// The names, comma separated, as error messages list them.
    pub(crate) fn names_joined() -> String {
        Mode::ALL.map(Mode::name).join(", ")
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl Serialize for Mode {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.name())
    }
}

/// Why a profile could not be read, validated or stored. The messages are the Python edition's,
/// because clients show them to the user as they are.
#[derive(Debug)]
pub enum ProfileError {
    /// The settings are not acceptable (wrong type, out of range, unknown mode, not an object).
    Invalid(String),
    /// A profile name with characters outside letters, digits, `.`, `_`, `-` or longer than 40.
    BadName,
    /// No built-in or saved profile of that name. `hint` adds where to list the valid ones
    /// (the load message does, the delete message does not).
    NotFound {
        /// The name asked for.
        name: String,
        /// Whether the message points at `truesense power smart list`.
        hint: bool,
    },
    /// A built-in profile cannot be overwritten or deleted.
    BuiltIn {
        /// The built-in name.
        name: String,
        /// What was attempted.
        action: BuiltInAction,
    },
    /// The saved file is not JSON.
    NotJson(String),
    /// The profile directory or a profile file could not be read or written.
    Io(FsError),
}

/// What was attempted on a built-in profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltInAction {
    /// Overwriting it.
    Save,
    /// Deleting it.
    Delete,
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProfileError::Invalid(m) => f.write_str(m),
            ProfileError::BadName => f.write_str("profile names use letters, digits, . _ - (max 40)"),
            ProfileError::NotFound { name, hint: true } => {
                write!(f, "no smart profile '{name}' (truesense power smart list)")
            }
            ProfileError::NotFound { name, hint: false } => write!(f, "no smart profile '{name}'"),
            ProfileError::BuiltIn { name, action: BuiltInAction::Save } => {
                write!(
                    f,
                    "'{name}' is built in and read-only; copy it (power smart copy {name}) and edit the copy"
                )
            }
            ProfileError::BuiltIn { name, action: BuiltInAction::Delete } => {
                write!(f, "'{name}' is built in and cannot be deleted")
            }
            ProfileError::NotJson(name) => write!(f, "smart profile '{name}' is not valid JSON"),
            ProfileError::Io(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for ProfileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ProfileError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<FsError> for ProfileError {
    fn from(e: FsError) -> Self {
        ProfileError::Io(e)
    }
}

/// A mode-valued setting: its JSON key and the label a window shows for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModeSetting {
    /// The JSON key.
    pub key: &'static str,
    /// The label.
    pub label: &'static str,
}

/// A numeric setting: its JSON key, the inclusive range and the label a window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NumSetting {
    /// The JSON key.
    pub key: &'static str,
    /// Smallest accepted value.
    pub min: u32,
    /// Largest accepted value.
    pub max: u32,
    /// The label.
    pub label: &'static str,
}

/// The mode-valued settings in display order (the Python `LABELS`). Validation checks them in
/// this order, so the first error reported is the same as before.
pub const MODE_SETTINGS: [ModeSetting; 7] = [
    ModeSetting { key: "ac_idle", label: "On power, idle" },
    ModeSetting { key: "ac_browsing", label: "On power, everyday use" },
    ModeSetting { key: "ac_busy", label: "On power, gaming or heavy work" },
    ModeSetting { key: "battery_idle", label: "On battery, idle" },
    ModeSetting { key: "battery_browsing", label: "On battery, everyday use" },
    ModeSetting { key: "battery_busy", label: "On battery, gaming or heavy work" },
    ModeSetting { key: "battery_hot", label: "On battery, too hot" },
];

/// The numeric settings with their ranges, in the Python `NUMS` order.
pub const NUM_SETTINGS: [NumSetting; 12] = [
    NumSetting { key: "turbo_after_s", min: 5, max: 300, label: "Heavy load for this long before Turbo (s)" },
    NumSetting {
        key: "turbo_max_cpu",
        min: 60,
        max: 95,
        label: "No Turbo above this CPU temperature (\u{b0}C)",
    },
    NumSetting {
        key: "turbo_max_gpu",
        min: 55,
        max: 85,
        label: "No Turbo above this GPU temperature (\u{b0}C)",
    },
    NumSetting {
        key: "backoff_hold_s",
        min: 10,
        max: 600,
        label: "After overheating, wait before Turbo again (s)",
    },
    NumSetting {
        key: "idle_after_s",
        min: 10,
        max: 600,
        label: "Idle for this long before the idle mode (s)",
    },
    NumSetting { key: "battery_low_pct", min: 5, max: 50, label: "Below this battery level, force Eco (%)" },
    NumSetting { key: "up_dwell_s", min: 2, max: 120, label: "Wait before moving to a faster mode (s)" },
    NumSetting { key: "down_dwell_s", min: 2, max: 300, label: "Wait before moving to a slower mode (s)" },
    NumSetting {
        key: "busy_hold_s",
        min: 0,
        max: 600,
        label: "Stay in the gaming / heavy mode this long after the load drops (s)",
    },
    NumSetting {
        key: "min_change_s",
        min: 2,
        max: 600,
        label: "At least this long between automatic mode changes (s)",
    },
    NumSetting {
        key: "turbo_margin_c",
        min: 0,
        max: 20,
        label: "Enter Turbo only this far below its temperature limits (\u{b0}C)",
    },
    NumSetting {
        key: "turbo_retry_s",
        min: 10,
        max: 900,
        label: "After Turbo reaches its limit, wait before trying it again (s)",
    },
];

/// One Smart-auto profile. The field names are the JSON keys (the Python `[smart]` settings);
/// the declaration order is the order the Python edition wrote them in, so saved files look the
/// same. Times are seconds, temperatures degrees Celsius.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Profile {
    /// Mode on power when idle.
    pub ac_idle: Mode,
    /// Mode on power for everyday use.
    pub ac_browsing: Mode,
    /// Mode on power under load.
    pub ac_busy: Mode,
    /// Whether sustained load on power may escalate to Turbo.
    pub ac_turbo: bool,
    /// Heavy load for this long before Turbo.
    pub turbo_after_s: u32,
    /// No Turbo above this CPU temperature.
    pub turbo_max_cpu: u32,
    /// No Turbo above this GPU temperature.
    pub turbo_max_gpu: u32,
    /// After overheating, wait this long before Turbo again.
    pub backoff_hold_s: u32,
    /// Idle for this long before the idle mode.
    pub idle_after_s: u32,
    /// Mode on battery when idle.
    pub battery_idle: Mode,
    /// Mode on battery for everyday use.
    pub battery_browsing: Mode,
    /// Mode on battery under load.
    pub battery_busy: Mode,
    /// Mode on battery when too hot.
    pub battery_hot: Mode,
    /// Below this battery percentage, force Eco.
    pub battery_low_pct: u32,
    /// Wait this long before moving to a faster mode.
    pub up_dwell_s: u32,
    /// Wait this long before moving to a slower mode.
    pub down_dwell_s: u32,
    /// Stay in the gaming or heavy mode this long after the load drops.
    pub busy_hold_s: u32,
    /// At least this long between automatic mode changes.
    pub min_change_s: u32,
    /// Enter Turbo only this far below its temperature limits.
    pub turbo_margin_c: u32,
    /// After Turbo reaches its limit, wait this long before trying it again.
    pub turbo_retry_s: u32,
    /// Free text shown next to the name; the built-ins have one, a user profile may.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl Default for Profile {
    /// The settings a profile gets for everything it does not give, and what the `truesense`
    /// built-in is made of (the Python `DEFAULTS`).
    fn default() -> Self {
        Profile {
            ac_idle: Mode::Balanced,
            ac_browsing: Mode::Balanced,
            ac_busy: Mode::Performance,
            ac_turbo: true,
            turbo_after_s: 30,
            turbo_max_cpu: 85,
            turbo_max_gpu: 75,
            backoff_hold_s: 90,
            idle_after_s: 60,
            battery_idle: Mode::Eco,
            battery_browsing: Mode::Eco,
            battery_busy: Mode::Balanced,
            battery_hot: Mode::Quiet,
            battery_low_pct: 20,
            up_dwell_s: 5,
            down_dwell_s: 60,
            busy_hold_s: 60,
            min_change_s: 45,
            turbo_margin_c: 6,
            turbo_retry_s: 180,
            description: None,
        }
    }
}

impl Profile {
    fn mode_mut(&mut self, key: &str) -> Option<&mut Mode> {
        Some(match key {
            "ac_idle" => &mut self.ac_idle,
            "ac_browsing" => &mut self.ac_browsing,
            "ac_busy" => &mut self.ac_busy,
            "battery_idle" => &mut self.battery_idle,
            "battery_browsing" => &mut self.battery_browsing,
            "battery_busy" => &mut self.battery_busy,
            "battery_hot" => &mut self.battery_hot,
            _ => return None,
        })
    }

    fn num_mut(&mut self, key: &str) -> Option<&mut u32> {
        Some(match key {
            "turbo_after_s" => &mut self.turbo_after_s,
            "turbo_max_cpu" => &mut self.turbo_max_cpu,
            "turbo_max_gpu" => &mut self.turbo_max_gpu,
            "backoff_hold_s" => &mut self.backoff_hold_s,
            "idle_after_s" => &mut self.idle_after_s,
            "battery_low_pct" => &mut self.battery_low_pct,
            "up_dwell_s" => &mut self.up_dwell_s,
            "down_dwell_s" => &mut self.down_dwell_s,
            "busy_hold_s" => &mut self.busy_hold_s,
            "min_change_s" => &mut self.min_change_s,
            "turbo_margin_c" => &mut self.turbo_margin_c,
            "turbo_retry_s" => &mut self.turbo_retry_s,
            _ => return None,
        })
    }

    /// Checks that every number of a hand-built profile is inside its range (the modes cannot
    /// be wrong, they are typed). Profiles from [`validate`] always pass.
    pub fn check(&self) -> Result<(), ProfileError> {
        let values = [
            self.turbo_after_s,
            self.turbo_max_cpu,
            self.turbo_max_gpu,
            self.backoff_hold_s,
            self.idle_after_s,
            self.battery_low_pct,
            self.up_dwell_s,
            self.down_dwell_s,
            self.busy_hold_s,
            self.min_change_s,
            self.turbo_margin_c,
            self.turbo_retry_s,
        ];
        // `values` is in NUM_SETTINGS order (a test pins that), so the first bad one is the
        // same one `validate` would name.
        match NUM_SETTINGS.iter().zip(values).find(|(ns, v)| *v < ns.min || *v > ns.max) {
            Some((ns, _)) => Err(range_error(ns)),
            None => Ok(()),
        }
    }

    /// The profile as the JSON object the control socket and the saved files carry.
    pub fn to_json(&self) -> Value {
        // A plain struct of numbers, booleans and strings cannot fail to serialise; Null is
        // only the unreachable fallback that keeps this function infallible.
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

fn mode_error(key: &str) -> ProfileError {
    ProfileError::Invalid(format!("{key} must be one of {}", Mode::names_joined()))
}

fn range_error(ns: &NumSetting) -> ProfileError {
    ProfileError::Invalid(format!("{}: {}..{}", ns.key, ns.min, ns.max))
}

/// Validates a profile that arrived as decoded JSON (a socket request, a file, the daemon's
/// stored copy) and returns it with the defaults filled in for every setting it does not give.
///
/// Unknown keys are ignored. Numbers are truncated to whole numbers after the range check, as
/// the Python `int(v)` did, so `300.5` is refused but `29.9` becomes 29. A boolean is not a
/// number. Checks run in the Python order (modes, `ac_turbo`, numbers) so the first message is
/// the same.
pub fn validate(value: &Value) -> Result<Profile, ProfileError> {
    let obj = value
        .as_object()
        .ok_or_else(|| ProfileError::Invalid("a smart profile is an object of settings".into()))?;
    // Python kept any value under "description"; a non-text one is dropped here (it would
    // only ever be shown as text).
    let mut p = Profile {
        description: obj.get("description").and_then(Value::as_str).map(str::to_owned),
        ..Profile::default()
    };
    for ms in MODE_SETTINGS {
        if let Some(v) = obj.get(ms.key) {
            let mode = v.as_str().and_then(Mode::parse).ok_or_else(|| mode_error(ms.key))?;
            if let Some(slot) = p.mode_mut(ms.key) {
                *slot = mode;
            }
        }
    }
    if let Some(v) = obj.get("ac_turbo") {
        p.ac_turbo = v.as_bool().ok_or_else(|| ProfileError::Invalid("ac_turbo is true or false".into()))?;
    }
    for ns in NUM_SETTINGS {
        if let Some(v) = obj.get(ns.key) {
            let f = if v.is_number() { v.as_f64() } else { None };
            // `!(lo <= f <= hi)` is false for NaN, which therefore fails like any other bad number.
            let ok = f.filter(|f| f64::from(ns.min) <= *f && *f <= f64::from(ns.max));
            let f = ok.ok_or_else(|| range_error(&ns))?;
            if let Some(slot) = p.num_mut(ns.key) {
                *slot = f as u32; // in range, so the cast truncates toward zero without overflow
            }
        }
    }
    Ok(p)
}

/// The names of the built-in profiles in display order.
pub const BUILTIN_NAMES: [&str; 6] =
    ["native", "truesense", "steady", "battery-first", "performance-first", "silent"];

/// A built-in profile, or `None` for any other name.
pub fn builtin(name: &str) -> Option<Profile> {
    let d = Profile::default;
    let p = match name {
        "native" => Profile {
            description: Some(
                "PredatorSense default: Balanced on power, Eco on battery, nothing else (no load or heat logic, like Native auto)"
                    .into(),
            ),
            ac_idle: Mode::Balanced,
            ac_browsing: Mode::Balanced,
            ac_busy: Mode::Balanced,
            ac_turbo: false,
            battery_idle: Mode::Eco,
            battery_browsing: Mode::Eco,
            battery_busy: Mode::Eco,
            battery_hot: Mode::Eco,
            up_dwell_s: 2,
            down_dwell_s: 2,
            busy_hold_s: 0,
            min_change_s: 2,
            turbo_margin_c: 0,
            turbo_retry_s: 10,
            ..d()
        },
        "truesense" => Profile {
            description: Some(
                "Our profile for this laptop: Balanced day to day, Performance when you game, Turbo only when sustained and cool, Eco on battery"
                    .into(),
            ),
            ..d()
        },
        "steady" => Profile {
            description: Some(
                "For long games and cooling that is not the laptop's own (external fans): Performance when busy, never Turbo, and it changes mode only after minutes of quiet"
                    .into(),
            ),
            ac_turbo: false,
            busy_hold_s: 180,
            down_dwell_s: 180,
            min_change_s: 180,
            up_dwell_s: 8,
            ..d()
        },
        "battery-first" => Profile {
            description: Some("Longest battery and quietest: no Turbo, light modes everywhere".into()),
            ac_idle: Mode::Quiet,
            ac_browsing: Mode::Quiet,
            ac_turbo: false,
            battery_idle: Mode::Eco,
            battery_browsing: Mode::Eco,
            battery_busy: Mode::Quiet,
            battery_low_pct: 30,
            down_dwell_s: 20,
            ..d()
        },
        "performance-first" => Profile {
            description: Some("Fastest: Performance for everyday use on power, Turbo after 10 s of heavy load".into()),
            ac_idle: Mode::Balanced,
            ac_browsing: Mode::Performance,
            turbo_after_s: 10,
            turbo_max_cpu: 90,
            turbo_max_gpu: 80,
            backoff_hold_s: 45,
            battery_idle: Mode::Quiet,
            battery_browsing: Mode::Balanced,
            battery_busy: Mode::Performance,
            battery_low_pct: 15,
            up_dwell_s: 3,
            ..d()
        },
        "silent" => Profile {
            description: Some("As quiet as possible: Quiet and Balanced only, no Turbo".into()),
            ac_idle: Mode::Quiet,
            ac_browsing: Mode::Quiet,
            ac_busy: Mode::Balanced,
            ac_turbo: false,
            battery_idle: Mode::Eco,
            battery_browsing: Mode::Eco,
            battery_busy: Mode::Quiet,
            battery_hot: Mode::Eco,
            ..d()
        },
        _ => return None,
    };
    Some(p)
}

/// Whether `name` is a built-in (read-only) profile.
pub fn is_builtin(name: &str) -> bool {
    BUILTIN_NAMES.contains(&name)
}

/// The built-in `truesense` profile: what the daemon falls back to when the chosen profile is
/// unreadable or invalid, so a corrupt file can never leave Smart auto without settings.
pub fn fallback() -> Profile {
    builtin("truesense").unwrap_or_default()
}

/// The profile the daemon uses, with the reason when it had to fall back.
#[derive(Debug)]
pub struct Resolved {
    /// The settings to use (always valid).
    pub profile: Profile,
    /// Why the chosen profile was not usable and [`fallback`] was taken instead; the daemon
    /// logs it ("smart profile invalid, using the built-in: ...").
    pub problem: Option<ProfileError>,
}

/// Python truthiness of a stored JSON value: the daemon treats `null`, `{}`, `0`, `""`, `false`
/// as "no custom profile stored".
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// The user's saved profiles: one `NAME.json` per profile in a directory. The built-ins are
/// always available on top and cannot be changed.
#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// A store over `dir` (created on the first save, a missing one is an empty store).
    pub fn new(dir: impl Into<PathBuf>) -> Store {
        Store { dir: dir.into() }
    }

    /// The store of a user's home directory: `<home>/.config/truesense/smart`.
    pub fn for_home(home: &Path) -> Store {
        Store::new(home.join(".config/truesense/smart"))
    }

    /// The directory the profiles live in.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, name: &str) -> Result<PathBuf, ProfileError> {
        if valid_name(name) { Ok(self.dir.join(format!("{name}.json"))) } else { Err(ProfileError::BadName) }
    }

    /// The built-in names first, then the user's own sorted. Files whose name is not a valid
    /// profile name are not listed (the Python edition listed them although they could never
    /// be loaded). A directory that does not exist yet is not an error.
    pub fn names(&self) -> Vec<String> {
        let mut own: Vec<String> = match std::fs::read_dir(&self.dir) {
            Ok(rd) => rd
                .filter_map(Result::ok)
                .filter_map(|e| {
                    e.file_name().to_str().and_then(|n| n.strip_suffix(".json")).map(str::to_owned)
                })
                .filter(|n| valid_name(n) && !is_builtin(n))
                .collect(),
            Err(_) => Vec::new(),
        };
        own.sort();
        BUILTIN_NAMES.iter().map(|n| (*n).to_owned()).chain(own).collect()
    }

    /// A built-in or saved profile, validated.
    pub fn load(&self, name: &str) -> Result<Profile, ProfileError> {
        if let Some(p) = builtin(name) {
            return Ok(p);
        }
        let path = self.path(name)?;
        let bytes = match fsx::read_limited(&path, fsx::MAX_SETTINGS_FILE) {
            Ok(b) => b,
            Err(e) if e.is_not_found() => {
                return Err(ProfileError::NotFound { name: name.to_owned(), hint: true });
            }
            Err(e) => return Err(e.into()),
        };
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| ProfileError::NotJson(name.to_owned()))?;
        validate(&value)
    }

    /// Validates `profile` and writes it under `name`, atomically (a power loss never leaves a
    /// half-written profile). Built-in names are refused.
    pub fn save(&self, name: &str, profile: &Profile) -> Result<Profile, ProfileError> {
        if is_builtin(name) {
            return Err(ProfileError::BuiltIn { name: name.to_owned(), action: BuiltInAction::Save });
        }
        profile.check()?;
        let path = self.path(name)?;
        let data = pretty_json(profile)?;
        fsx::write_atomic_mkdir(&path, &data, 0o644)?;
        Ok(profile.clone())
    }

    /// Removes a saved profile. Built-ins cannot be deleted.
    pub fn delete(&self, name: &str) -> Result<(), ProfileError> {
        if is_builtin(name) {
            return Err(ProfileError::BuiltIn { name: name.to_owned(), action: BuiltInAction::Delete });
        }
        let path = self.path(name)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Err(ProfileError::NotFound { name: name.to_owned(), hint: false })
            }
            Err(source) => Err(FsError::Io { op: "remove", path, source }.into()),
        }
    }

    /// Saves a copy of profile `name` under `new` (or `NAME-copy` when none is given), adding
    /// `-2`, `-3`, ... while the target name is taken, and returns the name used. This is how a
    /// built-in becomes editable.
    pub fn copy(&self, name: &str, new: Option<&str>) -> Result<String, ProfileError> {
        let base = match new {
            Some(n) if !n.is_empty() => n.to_owned(),
            _ => format!("{name}-copy"),
        };
        let taken = self.names();
        let mut target = base.clone();
        let mut n = 2;
        while taken.contains(&target) {
            target = format!("{base}-{n}");
            n += 1;
        }
        let profile = self.load(name)?;
        self.save(&target, &profile)?;
        Ok(target)
    }

    /// The profile the daemon runs: the settings stored with the daemon (`smart_profile_data`)
    /// when there are some, else the saved or built-in profile called `name`. Anything invalid
    /// gives the built-in `truesense` profile and the reason in [`Resolved::problem`]; this
    /// never fails, because Smart auto must always have settings.
    pub fn resolve(&self, data: Option<&Value>, name: &str) -> Resolved {
        let attempt = match data.filter(|d| truthy(d)) {
            Some(d) => validate(d),
            None => self.load(name),
        };
        match attempt {
            Ok(profile) => Resolved { profile, problem: None },
            Err(e) => Resolved { profile: fallback(), problem: Some(e) },
        }
    }
}

/// The saved form: one space of indent, like the Python `json.dump(indent=1)`.
fn pretty_json(profile: &Profile) -> Result<Vec<u8>, ProfileError> {
    let mut buf = Vec::new();
    let fmt = serde_json::ser::PrettyFormatter::with_indent(b" ");
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
    profile
        .serialize(&mut ser)
        .map_err(|e| ProfileError::Invalid(format!("cannot encode the profile: {e}")))?;
    Ok(buf)
}

/// A private temporary directory for tests of this crate, removed on drop.
#[cfg(test)]
#[allow(clippy::expect_used)]
pub(crate) mod testdir {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    pub(crate) struct TempDir(PathBuf);

    impl TempDir {
        pub(crate) fn new(tag: &str) -> TempDir {
            static N: AtomicU32 = AtomicU32::new(0);
            let p = std::env::temp_dir().join(format!(
                "ts-decision-{tag}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&p).expect("create temp dir");
            TempDir(p)
        }

        pub(crate) fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::testdir::TempDir;
    use super::*;
    use serde_json::json;

    fn err(v: Value) -> String {
        validate(&v).expect_err("must be refused").to_string()
    }

    #[test]
    fn mode_names_ranks_and_parsing() {
        for (i, m) in Mode::ALL.into_iter().enumerate() {
            assert_eq!(Mode::parse(m.name()), Some(m));
            assert_eq!(usize::from(m.rank()), i);
        }
        assert!(Mode::Eco < Mode::Quiet && Mode::Performance < Mode::Turbo);
        for bad in ["", "Turbo", "TURBO", " turbo", "warp", "turbo\n"] {
            assert_eq!(Mode::parse(bad), None, "{bad:?}");
        }
        assert_eq!(Mode::names_joined(), "eco, quiet, balanced, performance, turbo");
    }

    #[test]
    fn defaults_are_the_python_defaults_and_are_valid() {
        let d = Profile::default();
        assert_eq!(
            (d.busy_hold_s, d.min_change_s, d.turbo_margin_c, d.turbo_retry_s, d.down_dwell_s),
            (60, 45, 6, 180, 60)
        );
        assert_eq!(
            (d.turbo_after_s, d.turbo_max_cpu, d.turbo_max_gpu, d.backoff_hold_s, d.idle_after_s),
            (30, 85, 75, 90, 60)
        );
        assert_eq!((d.battery_low_pct, d.up_dwell_s), (20, 5));
        assert!(d.ac_turbo);
        assert_eq!(
            (d.ac_idle, d.ac_browsing, d.ac_busy),
            (Mode::Balanced, Mode::Balanced, Mode::Performance)
        );
        assert_eq!(
            (d.battery_idle, d.battery_browsing, d.battery_busy, d.battery_hot),
            (Mode::Eco, Mode::Eco, Mode::Balanced, Mode::Quiet)
        );
        d.check().expect("defaults in range");
        assert_eq!(validate(&json!({})).expect("empty object is all defaults"), d);
    }

    #[test]
    fn every_builtin_validates_and_round_trips_through_json() {
        for name in BUILTIN_NAMES {
            let p = builtin(name).expect("listed built-in exists");
            assert_eq!(validate(&p.to_json()).expect(name), p, "{name}");
            assert!(p.description.is_some(), "{name} has a description");
        }
        assert!(builtin("nope").is_none() && builtin("").is_none() && builtin("Native").is_none());
        assert!(is_builtin("silent") && !is_builtin("mine"));
    }

    #[test]
    fn builtin_values_match_the_python_table() {
        let n = builtin("native").expect("native");
        assert_eq!((n.busy_hold_s, n.min_change_s), (0, 2), "native stays as simple as PredatorSense");
        assert_eq!((n.turbo_margin_c, n.turbo_retry_s, n.up_dwell_s, n.down_dwell_s), (0, 10, 2, 2));
        assert!(!n.ac_turbo);
        assert_eq!((n.ac_busy, n.battery_busy, n.battery_hot), (Mode::Balanced, Mode::Eco, Mode::Eco));
        let s = builtin("steady").expect("steady");
        assert_eq!(
            (s.ac_turbo, s.busy_hold_s, s.down_dwell_s, s.min_change_s, s.up_dwell_s),
            (false, 180, 180, 180, 8)
        );
        let b = builtin("battery-first").expect("battery-first");
        assert_eq!(
            (b.ac_idle, b.ac_browsing, b.ac_turbo, b.battery_busy, b.battery_low_pct, b.down_dwell_s),
            (Mode::Quiet, Mode::Quiet, false, Mode::Quiet, 30, 20)
        );
        let p = builtin("performance-first").expect("performance-first");
        assert_eq!(
            (p.ac_browsing, p.turbo_after_s, p.turbo_max_cpu, p.turbo_max_gpu, p.backoff_hold_s),
            (Mode::Performance, 10, 90, 80, 45)
        );
        assert_eq!(
            (p.battery_idle, p.battery_browsing, p.battery_busy, p.battery_low_pct, p.up_dwell_s),
            (Mode::Quiet, Mode::Balanced, Mode::Performance, 15, 3)
        );
        let q = builtin("silent").expect("silent");
        assert_eq!(
            (q.ac_idle, q.ac_busy, q.ac_turbo, q.battery_hot, q.battery_busy),
            (Mode::Quiet, Mode::Balanced, false, Mode::Eco, Mode::Quiet)
        );
        assert_eq!(builtin("truesense").expect("truesense").to_json()["ac_busy"], "performance");
    }

    #[test]
    fn missing_settings_take_the_defaults_so_old_profiles_still_load() {
        let d = builtin("truesense").expect("truesense").to_json();
        let mut old = d.as_object().expect("object").clone();
        for k in ["busy_hold_s", "min_change_s", "turbo_margin_c", "turbo_retry_s"] {
            old.remove(k);
        }
        assert_eq!(validate(&Value::Object(old)).expect("old profile").busy_hold_s, 60);
    }

    #[test]
    fn out_of_range_numbers_are_refused_with_the_python_message() {
        for (key, bad) in [
            ("busy_hold_s", 601),
            ("turbo_margin_c", 21),
            ("turbo_retry_s", 5),
            ("min_change_s", 1),
            ("turbo_after_s", 4),
            ("battery_low_pct", 51),
            ("turbo_max_cpu", 96),
            ("turbo_max_gpu", 54),
            ("up_dwell_s", 121),
            ("down_dwell_s", 301),
            ("backoff_hold_s", 9),
            ("idle_after_s", 601),
        ] {
            let ns = NUM_SETTINGS.iter().find(|n| n.key == key).expect("known setting");
            assert_eq!(err(json!({ key: bad })), format!("{key}: {}..{}", ns.min, ns.max));
        }
        // both ends of every range are accepted
        for ns in NUM_SETTINGS {
            for v in [ns.min, ns.max] {
                assert!(validate(&json!({ ns.key: v })).is_ok(), "{} = {v}", ns.key);
            }
        }
    }

    #[test]
    fn numbers_are_truncated_after_the_range_check_and_must_be_numbers() {
        assert_eq!(validate(&json!({"turbo_after_s": 29.9})).expect("float").turbo_after_s, 29);
        assert_eq!(validate(&json!({"turbo_after_s": 30.0})).expect("float").turbo_after_s, 30);
        assert!(err(json!({"turbo_after_s": 300.5})).starts_with("turbo_after_s:"));
        assert!(err(json!({"turbo_after_s": 4.9})).starts_with("turbo_after_s:"));
        for bad in [
            json!(true),
            json!("30"),
            json!(null),
            json!([30]),
            json!({"a": 1}),
            json!(-1),
            json!(1e300),
            json!(-1e300),
        ] {
            assert!(err(json!({"turbo_after_s": bad.clone()})).starts_with("turbo_after_s:"), "{bad}");
        }
    }

    #[test]
    fn bad_modes_and_flags_are_refused() {
        for bad in
            [json!("warp"), json!("Turbo"), json!(""), json!(4), json!(null), json!(["eco"]), json!(true)]
        {
            assert_eq!(
                err(json!({"ac_busy": bad.clone()})),
                "ac_busy must be one of eco, quiet, balanced, performance, turbo",
                "{bad}"
            );
        }
        for ms in MODE_SETTINGS {
            assert!(err(json!({ ms.key: "warp" })).starts_with(ms.key));
        }
        for bad in [json!("yes"), json!(1), json!(0), json!(null), json!([])] {
            assert_eq!(err(json!({"ac_turbo": bad})), "ac_turbo is true or false");
        }
        assert!(!validate(&json!({"ac_turbo": false})).expect("false").ac_turbo);
    }

    #[test]
    fn the_first_error_follows_the_python_order() {
        // modes are checked before ac_turbo, which is checked before numbers
        assert!(
            err(json!({"turbo_after_s": 1, "ac_turbo": "x", "battery_hot": "warp"}))
                .starts_with("battery_hot")
        );
        assert_eq!(err(json!({"turbo_after_s": 1, "ac_turbo": "x"})), "ac_turbo is true or false");
    }

    #[test]
    fn non_objects_are_refused_and_unknown_keys_ignored() {
        for bad in [json!(null), json!([]), json!("x"), json!(1), json!(true)] {
            assert_eq!(err(bad), "a smart profile is an object of settings");
        }
        let p = validate(&json!({"bogus": 1, "description": "mine", "ac_idle": "quiet"}))
            .expect("unknown key ignored");
        assert_eq!((p.description.as_deref(), p.ac_idle), (Some("mine"), Mode::Quiet));
        assert_eq!(validate(&json!({"description": 5})).expect("odd description").description, None);
    }

    #[test]
    fn json_keys_and_order_match_the_python_files() {
        let text = serde_json::to_string(&builtin("truesense").expect("truesense")).expect("serialises");
        let keys = [
            "ac_idle",
            "ac_browsing",
            "ac_busy",
            "ac_turbo",
            "turbo_after_s",
            "turbo_max_cpu",
            "turbo_max_gpu",
            "backoff_hold_s",
            "idle_after_s",
            "battery_idle",
            "battery_browsing",
            "battery_busy",
            "battery_hot",
            "battery_low_pct",
            "up_dwell_s",
            "down_dwell_s",
            "busy_hold_s",
            "min_change_s",
            "turbo_margin_c",
            "turbo_retry_s",
            "description",
        ];
        let mut at = 0;
        for k in keys {
            let pos = text.find(&format!("\"{k}\":")).unwrap_or_else(|| panic!("{k} missing"));
            assert!(pos >= at, "{k} out of order");
            at = pos;
        }
        assert!(!serde_json::to_string(&Profile::default()).expect("serialises").contains("description"));
    }

    #[test]
    fn check_catches_a_hand_built_profile_out_of_range() {
        let p = Profile { turbo_retry_s: 5, ..Profile::default() };
        assert_eq!(p.check().expect_err("range").to_string(), "turbo_retry_s: 10..900");
        // each setting is checked against its own range (the order of `check` matches the table)
        for (i, ns) in NUM_SETTINGS.iter().enumerate() {
            let mut q = Profile::default();
            *q.num_mut(ns.key).expect("known key") = ns.max + 1;
            assert_eq!(
                q.check().expect_err(ns.key).to_string(),
                format!("{}: {}..{}", ns.key, ns.min, ns.max),
                "setting {i}"
            );
        }
    }

    #[test]
    fn store_names_lists_builtins_first_then_own_sorted() {
        let dir = TempDir::new("sp-names");
        let store = Store::new(dir.path().join("smart"));
        assert_eq!(store.names(), BUILTIN_NAMES, "a missing directory is an empty store");
        std::fs::create_dir_all(store.dir()).expect("mkdir");
        for f in ["zeta.json", "alpha.json", "silent.json", "notes.txt", "bad name.json", "x.json.tmp"] {
            std::fs::write(store.dir().join(f), "{}").expect("write");
        }
        let names = store.names();
        assert_eq!(&names[..6], BUILTIN_NAMES);
        assert_eq!(
            &names[6..],
            ["alpha", "zeta"],
            "built-in shadows, bad names and other files are not listed"
        );
    }

    #[test]
    fn store_saves_loads_deletes_and_refuses_builtins() {
        let dir = TempDir::new("sp-save");
        let store = Store::new(dir.path().join("smart"));
        let silent = store.save("silent", &Profile::default()).expect_err("built-in");
        assert_eq!(
            silent.to_string(),
            "'silent' is built in and read-only; copy it (power smart copy silent) and edit the copy"
        );
        let mine = Profile { ac_busy: Mode::Turbo, description: Some("mine".into()), ..Profile::default() };
        store.save("mine", &mine).expect("save");
        assert_eq!(store.load("mine").expect("load"), mine);
        let text = std::fs::read_to_string(store.dir().join("mine.json")).expect("file");
        assert!(text.contains("\n \"ac_idle\": \"balanced\""), "indent of one space like Python: {text}");
        assert_eq!(store.load("truesense").expect("built-in"), builtin("truesense").expect("truesense"));
        store.delete("mine").expect("delete");
        assert_eq!(store.delete("mine").expect_err("gone").to_string(), "no smart profile 'mine'");
        assert_eq!(
            store.delete("native").expect_err("built-in").to_string(),
            "'native' is built in and cannot be deleted"
        );
        assert_eq!(
            store.load("mine").expect_err("gone").to_string(),
            "no smart profile 'mine' (truesense power smart list)"
        );
    }

    #[test]
    fn store_save_refuses_an_out_of_range_profile_and_writes_nothing() {
        let dir = TempDir::new("sp-range");
        let store = Store::new(dir.path());
        let bad = Profile { turbo_after_s: 1, ..Profile::default() };
        assert!(store.save("bad", &bad).is_err());
        assert!(!dir.path().join("bad.json").exists());
    }

    #[test]
    fn profile_names_cannot_escape_the_directory() {
        let dir = TempDir::new("sp-names-esc");
        let store = Store::new(dir.path());
        let long = "a".repeat(41);
        for bad in ["", "../x", "a/b", "a b", "x\0", "é", "a\n", long.as_str(), "..\\x"] {
            for r in [
                store.load(bad).map(|_| ()),
                store.save(bad, &Profile::default()).map(|_| ()),
                store.delete(bad),
            ] {
                assert_eq!(
                    r.expect_err("refused").to_string(),
                    "profile names use letters, digits, . _ - (max 40)",
                    "{bad:?}"
                );
            }
        }
        assert!(store.save(&"a".repeat(40), &Profile::default()).is_ok(), "40 characters is the longest");
        assert!(store.save("a.b_c-D9", &Profile::default()).is_ok());
    }

    #[test]
    fn a_damaged_file_is_reported_not_trusted() {
        let dir = TempDir::new("sp-damage");
        let store = Store::new(dir.path());
        let write = |name: &str, bytes: &[u8]| {
            std::fs::write(dir.path().join(format!("{name}.json")), bytes).expect("write")
        };
        write("garbage", b"{not json");
        write("binary", &[0xff, 0xfe, 0x00]);
        write("empty", b"");
        write("array", b"[1,2]");
        write("range", br#"{"turbo_after_s": 1}"#);
        write("partial", br#"{"ac_idle": "quiet"}"#);
        for name in ["garbage", "binary", "empty"] {
            assert_eq!(
                store.load(name).expect_err(name).to_string(),
                format!("smart profile '{name}' is not valid JSON")
            );
        }
        assert_eq!(
            store.load("array").expect_err("array").to_string(),
            "a smart profile is an object of settings"
        );
        assert_eq!(store.load("range").expect_err("range").to_string(), "turbo_after_s: 5..300");
        assert_eq!(store.load("partial").expect("partial").ac_idle, Mode::Quiet);
        let big = vec![b' '; (fsx::MAX_SETTINGS_FILE + 1) as usize];
        write("huge", &big);
        assert!(
            matches!(store.load("huge"), Err(ProfileError::Io(_))),
            "an oversized file is refused without being read whole"
        );
    }

    #[test]
    fn copy_picks_a_free_name() {
        let dir = TempDir::new("sp-copy");
        let store = Store::new(dir.path().join("smart"));
        assert_eq!(store.copy("silent", None).expect("copy"), "silent-copy");
        assert_eq!(store.copy("silent", None).expect("copy"), "silent-copy-2");
        assert_eq!(store.copy("silent", None).expect("copy"), "silent-copy-3");
        assert_eq!(store.copy("silent", Some("mine")).expect("copy"), "mine");
        assert_eq!(store.copy("silent", Some("mine")).expect("copy"), "mine-2");
        assert_eq!(store.copy("silent", Some("silent")).expect("copy over a built-in name"), "silent-2");
        assert_eq!(store.load("mine").expect("load").ac_busy, Mode::Balanced, "a copy is the same settings");
        assert_eq!(
            store.copy("nothing", None).expect_err("source missing").to_string(),
            "no smart profile 'nothing' (truesense power smart list)"
        );
    }

    #[test]
    fn resolve_prefers_stored_data_then_the_named_profile_then_the_builtin() {
        let dir = TempDir::new("sp-resolve");
        let store = Store::new(dir.path());
        let data = json!({"ac_busy": "turbo"});
        let r = store.resolve(Some(&data), "silent");
        assert!(r.problem.is_none());
        assert_eq!(r.profile.ac_busy, Mode::Turbo, "stored data wins over the name");
        for empty in [json!(null), json!({})] {
            let r = store.resolve(Some(&empty), "silent");
            assert_eq!(r.profile, builtin("silent").expect("silent"), "an empty stored value means none");
        }
        assert_eq!(store.resolve(None, "steady").profile, builtin("steady").expect("steady"));
        let bad = json!({"ac_busy": "warp"});
        let r = store.resolve(Some(&bad), "silent");
        assert_eq!(r.profile, fallback());
        assert_eq!(
            r.problem.expect("reported").to_string(),
            "ac_busy must be one of eco, quiet, balanced, performance, turbo"
        );
        let r = store.resolve(None, "ghost");
        assert_eq!(r.profile, fallback());
        assert!(r.problem.is_some());
        assert_eq!(fallback().ac_busy, Mode::Performance);
        assert!(fallback().description.is_some());
        let r = store.resolve(Some(&json!("text")), "silent");
        assert!(r.problem.is_some(), "a truthy non-object is an invalid profile");
    }

    #[test]
    fn for_home_uses_the_config_directory() {
        assert_eq!(Store::for_home(Path::new("/home/u")).dir(), Path::new("/home/u/.config/truesense/smart"));
    }
}
