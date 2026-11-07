//! What this machine can do, as the UI needs to know it: every control that the laptop lacks is
//! shown greyed with one sentence, never missing and never an error toast (APP-DESIGN 4.2).
//!
//! Two sources feed the same model:
//!
//! * the answer of `status get`: a member that is `null` or absent means the machine has no such
//!   feature, and a `daemon` that is not a JSON object means the root service is not running, which
//!   gates the features only the daemon can do (Smart auto, power limits, undervolt);
//! * a future `capabilities get` command (`[{"id", "supported", "why"}]`, APP-DESIGN 4.4), which
//!   overrides what was derived because it knows more (sysfs facts, the vBIOS).
//!
//! The model carries a **state** and an optional free-text reason; the sentences the person reads
//! are chosen and translated on the QML side from the state, so this module has no user text.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::json::{at, bool_at, str_at};

/// Ids of the capabilities derived from `status get`. Pages use these strings.
pub mod id {
    /// The root service (`truesense.service`) answers.
    pub const DAEMON: &str = "daemon";
    /// Fan speeds can be read.
    pub const FANS: &str = "fans";
    /// The rear light.
    pub const REAR_LIGHT: &str = "rear-light";
    /// The keyboard backlight level.
    pub const KEYBOARD_LIGHT: &str = "keyboard-light";
    /// The 80 % battery limit.
    pub const BATTERY_LIMIT: &str = "battery-limit";
    /// Stop charging other devices over USB while off.
    pub const USB_CHARGE: &str = "usb-charge";
    /// Screen overdrive.
    pub const OVERDRIVE: &str = "overdrive";
    /// The boot sound.
    pub const BOOT_SOUND: &str = "boot-sound";
    /// The GPU mode switch.
    pub const GPU_MODE: &str = "gpu-mode";
    /// The touchpad switch.
    pub const TOUCHPAD: &str = "touchpad";
    /// The audio output controls.
    pub const AUDIO: &str = "audio";
    /// Smart auto mode control (daemon).
    pub const SMART_AUTO: &str = "smart-auto";
    /// CPU power limits (daemon).
    pub const POWER_LIMITS: &str = "power-limits";
    /// CPU and GPU undervolt (daemon).
    pub const UNDERVOLT: &str = "undervolt";
}

/// Whether one feature can be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapState {
    /// Works.
    Available,
    /// This machine does not have it; the text is the model's own explanation when it gave one.
    Unavailable(Option<String>),
    /// It exists, but only while the TrueSense service runs.
    NeedsDaemon,
    /// Not known (yet); the UI treats it as usable and lets the command refuse.
    Unknown,
}

impl CapState {
    /// The word QML switches on: `available`, `unavailable`, `needs-daemon`, `unknown`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Unavailable(_) => "unavailable",
            Self::NeedsDaemon => "needs-daemon",
            Self::Unknown => "unknown",
        }
    }

    /// Whether a control for it should be enabled (unknown counts: the command is the authority).
    pub fn usable(&self) -> bool {
        matches!(self, Self::Available | Self::Unknown)
    }
}

/// The capabilities of this machine by id.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Capabilities {
    map: BTreeMap<String, CapState>,
}

/// Status members that exist only when the hardware has the feature.
const FROM_STATUS: &[(&str, &str)] = &[
    (id::REAR_LIGHT, "rear"),
    (id::KEYBOARD_LIGHT, "kb_brightness"),
    (id::BATTERY_LIMIT, "battery_limit"),
    (id::USB_CHARGE, "usb_charge_stop"),
    (id::OVERDRIVE, "overdrive"),
    (id::BOOT_SOUND, "boot_sound"),
    (id::GPU_MODE, "gpu_mode"),
    (id::TOUCHPAD, "touchpad"),
    (id::AUDIO, "audio_output"),
    (id::FANS, "sensors.cpu_fan_rpm"),
];

/// Features that need the daemon's answer to exist at all.
const NEEDS_DAEMON: &[&str] = &[id::SMART_AUTO, id::POWER_LIMITS, id::UNDERVOLT];

impl Capabilities {
    /// Derives the capabilities from the result of `status get`. A result that is not an object
    /// (the read failed) gives an empty model: everything then reads as [`CapState::Unknown`].
    pub fn from_status(status: &Value) -> Self {
        let mut map = BTreeMap::new();
        if !status.is_object() {
            return Self { map };
        }
        let daemon_up = at(status, "daemon").is_some_and(Value::is_object);
        map.insert(
            id::DAEMON.to_owned(),
            if daemon_up { CapState::Available } else { CapState::NeedsDaemon },
        );
        for (cap, member) in FROM_STATUS {
            let state =
                if at(status, member).is_some() { CapState::Available } else { CapState::Unavailable(None) };
            map.insert((*cap).to_owned(), state);
        }
        for cap in NEEDS_DAEMON {
            map.insert(
                (*cap).to_owned(),
                if daemon_up { CapState::Available } else { CapState::NeedsDaemon },
            );
        }
        Self { map }
    }

    /// Reads what `capabilities get` answers: either the bare list `[{"id", "supported": bool,
    /// "why"?: text}]` or the real report `{"model": .., "items": [..]}`. The report names features
    /// with underscores (`rear_light`) and the app's ids use hyphens (`rear-light`); an entry is
    /// stored under both spellings so either asks the right question. Entries of the wrong shape
    /// are skipped (an older or newer daemon must not break the app).
    pub fn from_list(list: &Value) -> Self {
        let items = list.get("items").unwrap_or(list);
        let mut map = BTreeMap::new();
        for entry in items.as_array().map(Vec::as_slice).unwrap_or_default() {
            let (Some(cap), Some(supported)) = (str_at(entry, "id"), bool_at(entry, "supported")) else {
                continue;
            };
            let state = if supported {
                CapState::Available
            } else {
                CapState::Unavailable(str_at(entry, "why").map(str::to_owned))
            };
            let hyphenated = cap.replace('_', "-");
            if hyphenated != cap {
                map.insert(hyphenated, state.clone());
            }
            map.insert(cap.to_owned(), state);
        }
        Self { map }
    }

    /// Lets `other` override entries of `self` (the explicit list knows more than the derivation).
    #[must_use]
    pub fn overlaid_with(mut self, other: Self) -> Self {
        self.map.extend(other.map);
        self
    }

    /// The state of `cap`; [`CapState::Unknown`] for an id the model has never heard of.
    pub fn state(&self, cap: &str) -> CapState {
        self.map.get(cap).cloned().unwrap_or(CapState::Unknown)
    }

    /// Whether a control for `cap` should be enabled.
    pub fn usable(&self, cap: &str) -> bool {
        self.state(cap).usable()
    }

    /// `{"<id>": {"state": "available", "why": "..."?}}` for QML.
    pub fn to_json(&self) -> Value {
        let entries: Map<String, Value> = self
            .map
            .iter()
            .map(|(cap, state)| {
                let mut entry = json!({ "state": state.code() });
                if let CapState::Unavailable(Some(why)) = state {
                    entry["why"] = json!(why);
                }
                (cap.clone(), entry)
            })
            .collect();
        Value::Object(entries)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn status(daemon: Value) -> Value {
        json!({
            "power_mode": "balanced",
            "sensors": {"cpu_temp": 50, "cpu_fan_rpm": 2000},
            "daemon": daemon,
            "battery_limit": true, "usb_charge_stop": 0, "overdrive": false, "boot_sound": null,
            "gpu_mode": "optimus", "rear": null, "touchpad": true, "audio_output": {"active": "speakers"},
            "kb_brightness": 30,
        })
    }

    #[test]
    fn present_members_are_available_and_null_ones_are_not_on_this_model() {
        let c = Capabilities::from_status(&status(json!({"live": {}})));
        assert_eq!(c.state(id::BATTERY_LIMIT), CapState::Available);
        assert_eq!(c.state(id::USB_CHARGE), CapState::Available, "0 is a value, not a missing one");
        assert_eq!(c.state(id::OVERDRIVE), CapState::Available, "false is a value too");
        assert_eq!(c.state(id::BOOT_SOUND), CapState::Unavailable(None));
        assert_eq!(c.state(id::REAR_LIGHT), CapState::Unavailable(None));
        assert_eq!(c.state(id::FANS), CapState::Available);
        assert!(!c.usable(id::REAR_LIGHT));
        assert!(c.usable(id::KEYBOARD_LIGHT));
    }

    #[test]
    fn daemon_features_follow_the_service() {
        let up = Capabilities::from_status(&status(json!({"live": {}})));
        assert_eq!(up.state(id::DAEMON), CapState::Available);
        assert_eq!(up.state(id::UNDERVOLT), CapState::Available);
        let down = Capabilities::from_status(&status(json!("ERROR: the daemon is not running")));
        assert_eq!(down.state(id::DAEMON), CapState::NeedsDaemon);
        assert_eq!(down.state(id::SMART_AUTO), CapState::NeedsDaemon);
        assert_eq!(down.state(id::POWER_LIMITS).code(), "needs-daemon");
        assert!(!down.usable(id::UNDERVOLT));
        // Features that need no daemon are untouched by it.
        assert_eq!(down.state(id::TOUCHPAD), CapState::Available);
    }

    #[test]
    fn a_failed_status_read_leaves_everything_unknown_and_usable() {
        let c = Capabilities::from_status(&Value::Null);
        assert_eq!(c.state(id::REAR_LIGHT), CapState::Unknown);
        assert!(c.usable(id::REAR_LIGHT));
        assert_eq!(c.to_json(), json!({}));
    }

    #[test]
    fn the_explicit_list_overrides_what_was_derived_and_skips_junk() {
        let derived = Capabilities::from_status(&status(json!({"live": {}})));
        let list = json!([
            {"id": "gpu-mode", "supported": false, "why": "TGP is fixed by this laptop's firmware"},
            {"id": "rear-light", "supported": true},
            {"id": 7, "supported": true},
            {"id": "x"},
            "junk",
        ]);
        let merged = derived.overlaid_with(Capabilities::from_list(&list));
        assert_eq!(
            merged.state(id::GPU_MODE),
            CapState::Unavailable(Some("TGP is fixed by this laptop's firmware".into()))
        );
        assert_eq!(merged.state(id::REAR_LIGHT), CapState::Available);
        assert_eq!(merged.state("x"), CapState::Unknown);
        assert_eq!(Capabilities::from_list(&json!({"not": "a list"})), Capabilities::default());
    }

    #[test]
    fn the_real_report_shape_and_underscore_ids_are_understood() {
        let report = json!({
            "model": {"product": "Predator PH16-71"},
            "items": [
                {"id": "rear_light", "supported": false, "why": "no light bar"},
                {"id": "power_limits", "supported": true},
                {"id": "daemon", "supported": true},
            ]
        });
        let caps = Capabilities::from_list(&report);
        assert_eq!(caps.state("rear_light"), CapState::Unavailable(Some("no light bar".into())));
        assert_eq!(caps.state("rear-light"), CapState::Unavailable(Some("no light bar".into())));
        assert!(caps.usable("power-limits") && caps.usable("power_limits") && caps.usable("daemon"));
    }

    #[test]
    fn json_for_qml_has_state_words_and_the_reason_only_when_given() {
        let list =
            json!([{"id": "a", "supported": false, "why": "because"}, {"id": "b", "supported": false}]);
        let j = Capabilities::from_list(&list).to_json();
        assert_eq!(j["a"], json!({"state": "unavailable", "why": "because"}));
        assert_eq!(j["b"], json!({"state": "unavailable"}));
    }
}
