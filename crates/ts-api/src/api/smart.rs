//! Smart auto: the profiles that steer the automatic power mode, and the game rules that raise it
//! (the game rules live with the power operations).
#![forbid(unsafe_code)]

use std::fmt;

use serde::Serialize;
use serde_json::{Map, Value};
use ts_decision::smartprofile::{self, MODE_SETTINGS, NUM_SETTINGS, Profile};
use ts_ipc::wire::Args;

use super::{Api, ApiError, ApiResult, NamedList};

/// The settings of a Smart auto profile in the order they are shown.
pub const SMART_SETTING_KEYS: [&str; 20] = [
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
];

/// The service's answer to which Smart auto profile runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SmartProfileInUse {
    /// The profile's name.
    pub name: String,
    /// The settings in force.
    pub profile: Profile,
    /// Every profile name the service knows.
    pub names: Vec<String>,
}

impl fmt::Display for SmartProfileInUse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Smart auto profile in use: {}\n{}", self.name, describe_smart_profile(&self.profile))
    }
}

/// The profile's settings one per line as `truesense power smart show` prints them.
pub fn describe_smart_profile(profile: &Profile) -> String {
    let object = profile.to_json();
    SMART_SETTING_KEYS
        .iter()
        .map(|k| {
            let shown = match object.get(*k) {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Bool(true)) => "True".to_owned(),
                Some(Value::Bool(false)) => "False".to_owned(),
                Some(other) => other.to_string(),
                None => "?".to_owned(),
            };
            format!("  {k} = {shown}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Applies one `key=value` to a profile's settings, the way `--set` gives it.
fn set_setting(settings: &mut Map<String, Value>, item: &str) -> ApiResult<()> {
    let keys = SMART_SETTING_KEYS.join(", ");
    let bad = || ApiError::invalid(format!("--set key=value, key one of: {keys}"));
    let (key, text) = item
        .split_once('=')
        .filter(|(k, v)| !v.is_empty() && SMART_SETTING_KEYS.contains(k))
        .ok_or_else(bad)?;
    let value = if key == "ac_turbo" {
        Value::Bool(matches!(text.to_lowercase().as_str(), "1" | "true" | "yes" | "on"))
    } else if MODE_SETTINGS.iter().any(|m| m.key == key) {
        Value::String(text.to_owned())
    } else if NUM_SETTINGS.iter().any(|n| n.key == key) {
        let n: i64 = text
            .parse()
            .map_err(|_| ApiError::invalid(format!("{key} must be a whole number, not '{text}'")))?;
        Value::from(n)
    } else {
        return Err(bad());
    };
    settings.insert(key.to_owned(), value);
    Ok(())
}

impl Api {
    /// Lists the Smart auto profiles and, when the service runs, the one in use.
    pub fn smart_profiles(&self) -> ApiResult<NamedList> {
        let mut list = NamedList { names: self.s.smart.names(), ..NamedList::default() };
        for n in &list.names {
            if let Some(d) = smartprofile::builtin(n).and_then(|p| p.description) {
                list.descriptions.insert(n.clone(), d);
            }
        }
        if self.daemon_available() {
            list.current = Some(self.smart_profile_in_use()?.name);
        }
        Ok(list)
    }

    /// Asks the service which profile runs (it keeps it, so it works before login).
    pub fn smart_profile_in_use(&self) -> ApiResult<SmartProfileInUse> {
        let answer: Value = self.ask("smart_profile", Args::new().with("action", "get"))?;
        let name = answer.get("name").and_then(Value::as_str).unwrap_or_default().to_owned();
        let profile = smartprofile::validate(answer.get("profile").unwrap_or(&Value::Null))?;
        let names = answer
            .get("names")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|n| n.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        Ok(SmartProfileInUse { name, profile, names })
    }

    /// Reads one profile, built-in or yours.
    pub fn smart_profile(&self, name: &str) -> ApiResult<Profile> {
        Ok(self.s.smart.load(name)?)
    }

    /// Makes a profile the one the service runs. A saved profile travels with the request as data,
    /// so the service (root) does not depend on files in a home directory.
    pub fn use_smart_profile(&self, name: &str) -> ApiResult<String> {
        if name.is_empty() {
            return Err(ApiError::invalid("which profile? (truesense power smart list)"));
        }
        let data =
            if smartprofile::is_builtin(name) { Value::Null } else { self.s.smart.load(name)?.to_json() };
        self.use_smart_request(name, data)
    }

    /// Makes a profile that is not saved (the window's editor) the one the service runs; `name` is
    /// only a label. Settings the object lacks take their defaults and every value is checked.
    pub fn use_smart_profile_data(&self, name: &str, settings: &Value) -> ApiResult<String> {
        let checked = smartprofile::validate(settings)?;
        let label = if name.is_empty() { "custom" } else { name };
        self.use_smart_request(label, checked.to_json())
    }

    fn use_smart_request(&self, name: &str, data: Value) -> ApiResult<String> {
        let args = Args::new().with("action", "use").with("name", name).with("data", data);
        let answer: Value = self.ask("smart_profile", args)?;
        Ok(answer.get("name").and_then(Value::as_str).unwrap_or(name).to_owned())
    }

    /// Saves a copy under `new_name` (`NAME-copy` when none) and answers the name.
    pub fn copy_smart_profile(&self, name: &str, new_name: Option<&str>) -> ApiResult<String> {
        if name.is_empty() {
            return Err(ApiError::invalid("copy which profile?"));
        }
        Ok(self.s.smart.copy(name, new_name)?)
    }

    /// Saves a profile: starts from `base` (or from your own profile of that name, else the
    /// defaults) and applies the `key=value` settings.
    pub fn save_smart_profile(&self, name: &str, base: Option<&str>, set: &[String]) -> ApiResult<Profile> {
        if name.is_empty() {
            return Err(ApiError::invalid("save under which name?"));
        }
        let start = match base.filter(|b| !b.is_empty()) {
            Some(base) => self.s.smart.load(base)?,
            None if !smartprofile::is_builtin(name) && self.s.smart.names().iter().any(|n| n == name) => {
                self.s.smart.load(name)?
            }
            None => Profile::default(),
        };
        let mut settings = match start.to_json() {
            Value::Object(o) => o,
            _ => Map::new(),
        };
        for item in set {
            set_setting(&mut settings, item)?;
        }
        let profile = smartprofile::validate(&Value::Object(settings))?;
        Ok(self.s.smart.save(name, &profile)?)
    }

    /// Saves a whole profile as it was edited.
    pub fn save_smart_profile_data(&self, name: &str, settings: &Value) -> ApiResult<Profile> {
        if name.is_empty() {
            return Err(ApiError::invalid("save under which name?"));
        }
        let profile = smartprofile::validate(settings)?;
        Ok(self.s.smart.save(name, &profile)?)
    }

    /// Removes one of your profiles (built-in ones are read-only).
    pub fn delete_smart_profile(&self, name: &str) -> ApiResult<()> {
        Ok(self.s.smart.delete(name)?)
    }

    /// Whether a profile is read-only.
    pub fn smart_profile_is_builtin(name: &str) -> bool {
        smartprofile::is_builtin(name)
    }
}
