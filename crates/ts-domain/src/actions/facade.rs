//! The small traits the actions (and the whole-setup profiles) reach the machine through.
//!
//! The assembly layer implements them over the real objects: `Hw` and the daemon client for
//! power, fans, battery and display; the keyboard, key-profile and look modules for lighting;
//! the audio module for sound; the user-state file for memory. Tests implement them with
//! fakes, so no action is ever tested against a device, a socket or a process.
//!
//! Every method returns a [`HostResult`]: the failure text is shown to the user verbatim (it
//! becomes the on-screen message of a key press), so adapters pass their own error's `Display`
//! through unchanged.
#![forbid(unsafe_code)]

use std::fmt;

use serde_json::Value;
use ts_foundation::config::Config;
use ts_foundation::userstate;

use super::parse::TouchpadAction;
use crate::favourites::Favourite;

/// A failure reported by the machine side of an action. The text is meant for the user and is
/// displayed unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostError(String);

impl HostError {
    /// An error with this message.
    pub fn new(message: impl Into<String>) -> HostError {
        HostError(message.into())
    }

    /// The message.
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HostError {}

impl From<String> for HostError {
    fn from(m: String) -> Self {
        HostError(m)
    }
}

impl From<&str> for HostError {
    fn from(m: &str) -> Self {
        HostError(m.to_owned())
    }
}

/// The result of a call into the machine.
pub type HostResult<T> = Result<T, HostError>;

/// What the actions need from the configuration, read on every action so an edit of the config
/// applies without restarting the key service.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionSettings {
    /// Minutes a manual power choice outranks the automatic one (`[profile] hold_minutes`).
    pub hold_minutes: f64,
    /// The modes `power-next` and `power-prev` walk through (`[keys] power_cycle`).
    pub power_cycle: Vec<String>,
}

impl ActionSettings {
    /// The settings out of a loaded configuration.
    pub fn from_config(cfg: &Config) -> ActionSettings {
        ActionSettings {
            hold_minutes: cfg.profile.hold_minutes as f64,
            power_cycle: cfg.keys.power_cycle.clone(),
        }
    }
}

/// Where the actions read their [`ActionSettings`] from. Any `Fn() -> HostResult<ActionSettings>`
/// closure is one.
pub trait SettingsSource: Send + Sync {
    /// The current settings.
    fn settings(&self) -> HostResult<ActionSettings>;
}

impl<F> SettingsSource for F
where
    F: Fn() -> HostResult<ActionSettings> + Send + Sync,
{
    fn settings(&self) -> HostResult<ActionSettings> {
        self()
    }
}

/// A fan request. The duties are 20..=100: nothing in this crate ever builds a request that
/// stops a fan (that needs the confirmation path of the architecture documents).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanCommand {
    /// The smart curve (the daemon follows the saved curve).
    Smart,
    /// Hand the fans back to the firmware.
    Auto,
    /// Maximum speed.
    Max,
    /// A fixed duty per fan, each 20..=100.
    Manual {
        /// CPU fan duty.
        cpu: u8,
        /// GPU fan duty.
        gpu: u8,
    },
}

/// The fan behaviour and the duty registers, as the firmware reports them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FanSnapshot {
    /// `auto`, `max`, `manual`, or whatever name the firmware layer gives an unknown behaviour.
    pub mode: String,
    /// CPU fan duty register, percent.
    pub cpu: u8,
    /// GPU fan duty register, percent.
    pub gpu: u8,
}

/// Power mode, fans, battery and display: the firmware features. Implemented over `Hw` and the
/// daemon client by the assembly layer.
pub trait PowerFacade: Send + Sync {
    /// The current power mode name (`eco`, `quiet`, ...); a number as text for a code we do
    /// not know.
    fn mode(&self) -> HostResult<String>;
    /// The power modes this laptop supports.
    fn supported_modes(&self) -> HostResult<Vec<String>>;
    /// Sets the mode and holds it `hold_minutes` against the automatic mode control (like a
    /// tile pick in the window). Returns the mode now in effect, read back.
    fn set_mode(&self, mode: &str, hold_minutes: f64) -> HostResult<String>;
    /// The fan behaviour and duty registers.
    fn fan_state(&self) -> HostResult<FanSnapshot>;
    /// Sets the fans (through the daemon when it runs, on the hardware otherwise).
    fn set_fan(&self, command: FanCommand) -> HostResult<()>;
    /// Whether the firmware's 80 % battery limit is on.
    fn battery_limit(&self) -> HostResult<bool>;
    /// Switches the 80 % limit; returns the value read back.
    fn set_battery_limit(&self, on: bool) -> HostResult<bool>;
    /// USB charging while the laptop is off: the level (0, 10, 20 or 30).
    fn usb_charge(&self) -> HostResult<i64>;
    /// Sets the USB charge level; returns the value read back.
    fn set_usb_charge(&self, level: i64) -> HostResult<i64>;
    /// LCD overdrive, or `None` when the firmware does not say.
    fn overdrive(&self) -> HostResult<Option<bool>>;
    /// Switches overdrive; returns the value read back.
    fn set_overdrive(&self, on: bool) -> HostResult<bool>;
    /// The boot sound.
    fn boot_sound(&self) -> HostResult<bool>;
    /// Switches the boot sound; returns the value read back.
    fn set_boot_sound(&self, on: bool) -> HostResult<bool>;
    /// The keyboard controller's own "backlight off after 30 s" flag.
    fn keyboard_timeout(&self) -> HostResult<bool>;
    /// Sets that flag; returns the value read back.
    fn set_keyboard_timeout(&self, on: bool) -> HostResult<bool>;
}

/// The control socket of the daemon: settings the daemon holds (so they work before login)
/// and the touchpad switch (which needs root). Every method fails when the daemon is not
/// running; the actions decide what that means.
pub trait DaemonFacade: Send + Sync {
    /// One entry of the daemon's `state.settings` (`smart_fan`, `auto_mode`, ...), `None` when
    /// the daemon does not list it.
    fn setting(&self, key: &str) -> HostResult<Option<Value>>;
    /// Changes a daemon setting (the `set` request).
    fn set_setting(&self, key: &str, value: Value) -> HostResult<()>;
    /// The name of the fan curve in use (`fan_curve get`).
    fn fan_curve_name(&self) -> HostResult<String>;
    /// Selects a fan curve. The adapter decides how: a built-in goes by name, a saved curve
    /// travels with the request because the daemon (root) cannot read the user's directory.
    fn use_fan_curve(&self, name: &str) -> HostResult<()>;
    /// Asks the daemon to switch the touchpad; returns whether it is enabled afterwards.
    fn touchpad(&self, action: TouchpadAction) -> HostResult<bool>;
}

/// Switching the touchpad directly, for a root process with no daemon. Provide it only when
/// the process is root; its absence means a key press must go through the daemon.
pub trait LocalTouchpad: Send + Sync {
    /// Whether the touchpad is enabled now.
    fn enabled(&self) -> bool;
    /// Enables or disables it; returns the state afterwards.
    fn set_enabled(&self, on: bool) -> HostResult<bool>;
}

/// The light bar as read back from the firmware.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RearState {
    /// The effect name, or a bare number for an effect we do not know.
    pub mode: String,
    /// The colour, `rrggbb`.
    pub color: String,
    /// Speed 1..=5.
    pub speed: i64,
    /// Brightness 0..=100 (0 is off).
    pub brightness: i64,
    /// Direction 0 or 1 (the adapter supplies 1 when the firmware did not read it back).
    pub direction: i64,
}

/// A complete light bar setting to write (read back and verified by the adapter).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RearWrite {
    /// The effect name (one of the five firmware effects).
    pub mode: String,
    /// The colour, `rrggbb`.
    pub color: String,
    /// Speed 1..=5.
    pub speed: i64,
    /// Brightness 0..=100.
    pub brightness: i64,
    /// Direction 0 or 1.
    pub direction: i64,
    /// The three part colours of the static effect, left to right.
    pub zones: Option<[String; 3]>,
}

/// Keyboard backlight, light bar, per-key lighting profiles and looks. Implemented by the
/// assembly layer over the keyboard, keyprofiles, looks and hw modules.
pub trait LightingFacade: Send + Sync {
    /// Switches the backlight on again, restoring what it showed before it was switched off.
    fn keyboard_on(&self) -> HostResult<()>;
    /// Switches the backlight off.
    fn keyboard_off(&self) -> HostResult<()>;
    /// The default look (what `keyboard:on` falls back to when nothing was remembered).
    fn keyboard_restore_default(&self) -> HostResult<()>;
    /// The backlight brightness 0..=100 (0 is off).
    fn keyboard_brightness(&self) -> HostResult<i64>;
    /// Shows a keyboard effect.
    fn keyboard_effect(&self, effect: &str, color: &str, speed: i64, brightness: i64) -> HostResult<()>;
    /// Whether a saved per-key lighting profile exists.
    fn keyboard_profile_exists(&self, name: &str) -> bool;
    /// Applies a saved per-key lighting profile at the current brightness.
    fn keyboard_profile_apply(&self, name: &str) -> HostResult<()>;
    /// The names of the saved looks (the built-in `native` included).
    fn look_names(&self) -> Vec<String>;
    /// Loads and applies a saved look.
    fn look_apply(&self, name: &str) -> HostResult<()>;
    /// Reads the light bar.
    fn rear(&self) -> HostResult<RearState>;
    /// Writes the light bar.
    fn set_rear(&self, write: &RearWrite) -> HostResult<()>;
}

/// The sound system. The output actions show their own on-screen message (the switch takes
/// about a second, so the message must come first); the adapter does that, which is why the
/// toggle actions return no text.
pub trait AudioFacade: Send + Sync {
    /// Switches between speakers and headphones.
    fn output_toggle(&self) -> HostResult<()>;
    /// Steps through the outputs.
    fn output_cycle(&self) -> HostResult<()>;
    /// Chooses an output (`speakers` or `headphones`).
    fn set_output(&self, output: &str) -> HostResult<()>;
    /// The active output (`speakers` or `headphones`).
    fn active_output(&self) -> HostResult<String>;
    /// Flips the microphone mute; returns whether the microphones are muted now.
    fn mic_toggle(&self) -> HostResult<bool>;
    /// Mutes or unmutes the microphones.
    fn set_mic_mute(&self, muted: bool) -> HostResult<()>;
    /// Moves the noise reduction of the device in use to its next mode; returns the device
    /// (`headset` or `array`) and the mode now set (`off`, `personal`, `conference`).
    fn voice_next(&self) -> HostResult<(String, String)>;
    /// The noise-reduction modes: (headset, built-in array).
    fn voice_modes(&self) -> HostResult<(String, String)>;
    /// Sets the noise reduction of `device` (`headset` or `array`).
    fn voice_set(&self, device: &str, mode: &str) -> HostResult<()>;
    /// Sets the DTS mode by name.
    fn dts_mode(&self, name: &str) -> HostResult<()>;
}

/// The whole-setup profiles, as the actions `profile:NAME` and `profile-next` see them. The
/// assembly adapts a `profiles::BundleStore`; the indirection exists because a bundle applies
/// its power, fan and lighting settings by running actions, so the two modules must not import
/// each other.
pub trait ProfilesFacade: Send + Sync {
    /// Applies a whole-setup profile; returns the summary line.
    fn apply(&self, name: &str) -> HostResult<String>;
    /// The profile after the last one applied (the first when none was).
    fn next_name(&self) -> HostResult<String>;
}

/// The user's favourites, as the actions `favourite:N` and `favourite-next` see them. The
/// favourites file is the user's own, so the key service (which runs as the user) reads it
/// directly; applying one reuses the facades above for looks, fan curves and whole-setup
/// profiles. Only the Smart auto profile has no facade of its own, so it is here.
pub trait FavouritesFacade: Send + Sync {
    /// The favourites in order (empty when there are none or the file is unreadable).
    fn favourites(&self) -> HostResult<Vec<Favourite>>;
    /// Makes a Smart auto profile the one the daemon runs.
    fn use_smart_profile(&self, name: &str) -> HostResult<()>;
}

/// Starts an `exec:` command. It must run as the calling user, detached (its own session, no
/// input, no output) and return as soon as the command is started: `exec:truesense gui` opens a
/// window that must outlive the key press.
pub trait Spawner: Send + Sync {
    /// Starts `command` through the shell.
    fn spawn(&self, command: &str) -> HostResult<()>;
}

/// The small per-user memory (`user.json`): which effect the keys reached, what the light bar
/// was before it was switched off. A convenience, never a requirement: a failed write is
/// ignored by the actions.
pub trait ActionState: Send + Sync {
    /// One key, `None` when absent.
    fn get(&self, key: &str) -> Option<Value>;
    /// Stores one key.
    fn set(&self, key: &str, value: Value) -> HostResult<()>;
}

impl ActionState for userstate::Store {
    fn get(&self, key: &str) -> Option<Value> {
        userstate::Store::get(self, key)
    }

    fn set(&self, key: &str, value: Value) -> HostResult<()> {
        userstate::Store::set(self, key, value).map_err(|e| HostError::new(e.to_string()))
    }
}

/// Python's idea of true, which the 0.3.x state files and profiles were written against:
/// `null`, `false`, zero, the empty string, list and object are false.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_none_or(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn truthiness_is_pythons() {
        for (v, want) in [
            (json!(null), false),
            (json!(false), false),
            (json!(true), true),
            (json!(0), false),
            (json!(0.0), false),
            (json!(2), true),
            (json!(""), false),
            (json!("x"), true),
            (json!([]), false),
            (json!([0]), true),
            (json!({}), false),
            (json!({"a": 1}), true),
        ] {
            assert_eq!(truthy(&v), want, "{v}");
        }
    }

    #[test]
    fn host_errors_display_verbatim() {
        assert_eq!(
            HostError::from("the TrueSense daemon is not running").to_string(),
            "the TrueSense daemon is not running"
        );
        assert_eq!(HostError::new(String::from("x")).message(), "x");
    }

    #[test]
    fn settings_come_from_a_config() {
        let cfg = Config::default();
        let s = ActionSettings::from_config(&cfg);
        assert_eq!(s.power_cycle.len(), 5);
        assert!(s.hold_minutes >= 0.0);
        let f = || -> HostResult<ActionSettings> { Ok(s.clone()) };
        assert_eq!(SettingsSource::settings(&f).map(|x| x.power_cycle.len()), Ok(5));
    }
}
