//! api: the application layer every client shares.
//!
//! The command line, the terminal UI, the web window and the key service all do the same things
//! (set the power mode, save a fan curve, run the health check). They differ only in how they ask
//! (flags, keys, buttons) and how they show the answer. This module is the single place that knows
//! what an operation does and which safety rule applies, so a rule can never be enforced in one
//! client and forgotten in another.
//!
//! # Shape
//!
//! * [`Services`] is every injected dependency: the daemon link, the firmware, the user state, the
//!   configuration files, the profile stores, the keyboard, the sound system. The assembly layer
//!   (`adapt`) fills it with the real ones; [`testing`] fills it with the firmware simulator, a
//!   scripted daemon and temporary directories.
//! * [`Api`] owns a `Services` and offers the operations, grouped by area in the submodules
//!   (status, power, fan, cpu, gpu, keyboard, rear, audio, keys, ...). Every method returns a typed [`ApiResult`]; the result
//!   types serialise with the JSON field names the Python edition printed under `--json`, and have a
//!   `Display` with the line a person reads.
//!
//! # How an operation reaches the machine
//!
//! The daemon owns what is privileged or persistent (fan curves, the smart profile, undervolt,
//! power limits, the hold on a power pick). Where the Python edition could do something without
//! the service and it is safe to, the operation falls back to the hardware when the socket is
//! missing (`control.py`); everything else says that the service is not running. A fan is never
//! stopped without the confirmation and the service (see [`Refusal`]).
//!
//! # What the assembly layer must provide
//!
//! Every field of [`Services`] is documented with its real implementation; the adapters that do
//! not exist in the lower layers are the [`Notifier`] over the OSD (already implemented for
//! `ts_system::osd::Osd`), the `ts_domain::designs::ImageDecoder`, and the closure `fn_key_node`.
#![forbid(unsafe_code)]

mod alerts;
mod audio;
mod battery;
mod battery_info;
mod bindings;
mod capabilities;
mod cpu;
mod daemon;
mod error;
mod fan;
mod favourites;
mod gpu;
mod keyboard;
mod keys;
mod looks;
mod monitor;
mod power;
mod profile;
mod rear;
mod smart;
mod status;
mod system;
mod themes;
mod traits;
mod update;
mod window;

pub mod testing;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use ts_decision::{curves, smartprofile};
use ts_domain::actions::ActionRunner;
use ts_domain::audio::Audio;
use ts_domain::designs::{DesignStore, ImageDecoder};
use ts_domain::gpuundervolt;
use ts_domain::keybinds::KeyProfiles;
use ts_domain::keyboard::Keyboard;
use ts_domain::keymap::KeyMap;
use ts_domain::keyprofiles::ProfileStore as LightingProfiles;
use ts_domain::looks::LookStore;
use ts_domain::perkeyfx::FxStore;
use ts_domain::power::PowerLimits;
use ts_domain::powermgr::PowerManagers;
use ts_domain::undervolt;
use ts_foundation::config::{self, Config, ConfigError, Table};
use ts_foundation::userstate;
use ts_platform::hw::{Hw, LinuxSensors, Sleeper};
use ts_system::models::ProbeOptions;
use ts_system::sensors::{Clock, Sampler};
use ts_system::touchpad::Touchpad;

pub use alerts::{AlertsEdit, describe_alerts};
pub use audio::{DtsInfo, JackReport, MicReport, VoiceStatus, dts_is_on};
pub use battery::{BatteryStatus, ChargeFull, ChargeFullAction};
pub use bindings::{BindingResult, BindingsApplied, ModeBindings};
pub use capabilities::{Capabilities, Capability, Feature, ModelSummary};
pub use cpu::{
    AppliedLimits, AppliedUndervolt, AutoLimits, CpuUndervolt, GpuPowerLimits, LimitsInfo, LockBit,
    PowerLimitsState, UndervoltOffsets, UndervoltPlane, UndervoltRequest, UserLimits,
};
pub use daemon::{Daemon, DaemonState};
pub use error::{ApiError, ApiResult, Refusal};
pub use fan::{CurveEdit, FanCurveInUse, FanOutcome, FanRequest, FanRoute, FanStatus, describe_curve};
pub use favourites::{FavouriteInfo, FavouriteKind};
pub use gpu::{AppliedGpuUndervolt, GpuApplied, GpuUndervoltState};
pub use keyboard::{
    AnimationRequest, DesignInfo, EffectRequest, KeyCell, KeyLayout, KeyNames, KeyboardLook, KeyboardStatus,
    LightingProfileAnswer, Ordered, PaintItem, WaveDirection, parse_direction, wave_directions,
};
pub use keys::{KeyBindings, KeyProfileEdit, KeyProfileView, KeyTiming, KeysSettings};
pub use looks::StartupSettings;
pub use monitor::{
    GpuSource, MAX_TOP as MONITOR_MAX_TOP, MonitorCore, MonitorCpu, MonitorDisks, MonitorGpu, MonitorMemory,
    MonitorNetwork, MonitorOverview, SystemInfo, TemperatureReading,
};
pub use power::{
    AutoAction, AutoMode, CpuSettings, GameRuleAction, GameRules, Hold, ManagerUnit, PowerManagersRestored,
    PowerManagersState, PowerManagersStopped, PowerStatus,
};
pub use profile::ProfileInfo;
pub use rear::{RearPowerAction, RearRequest, RearStatus};
pub use smart::{SMART_SETTING_KEYS, SmartProfileInUse, describe_smart_profile};
pub use status::{Live, LiveLoad, LivePower, Status, Summary, Supply, describe_sensors};
pub use system::{DoctorAdapters, Step, real_doctor_env};
pub use themes::{ColourwayPick, ThemeDetail, ThemeList, ThemeSpec, ThemeStatus};
pub use traits::{NoNotifier, Notifier, ProfileBundles, RearMemoryInUserState, ServiceChecks, UpdateService};
pub use update::{UpdatePlan, describe_apply, describe_update};
pub use window::{DesignShown, ResetStep};

/// How a variable of the environment is looked up.
pub type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Finds the keyboard's Fn-key hidraw node.
pub type NodeFinder = Arc<dyn Fn() -> Option<PathBuf> + Send + Sync>;

/// Builds the health check's environment from this process's adapters.
pub type DoctorEnvBuilder = Arc<dyn Fn(DoctorAdapters) -> ts_system::doctor::Env + Send + Sync>;

/// The configuration files, read fresh for every use and written by the commands that change a
/// setting, exactly as the Python edition re-read them every time: an edit applies without
/// restarting anything.
#[derive(Clone)]
pub struct ConfigFiles {
    /// The machine-wide file (`/etc/truesense/config.toml`); `None` reads none.
    pub system: Option<PathBuf>,
    /// The user's own file; `None` without `HOME`, which makes every write fail loudly.
    pub user: Option<PathBuf>,
    /// The environment the load reads `SUDO_USER` and `USER` from.
    pub env: EnvLookup,
}

impl fmt::Debug for ConfigFiles {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfigFiles").field("system", &self.system).field("user", &self.user).finish()
    }
}

impl ConfigFiles {
    /// The standard places and the real environment.
    pub fn standard() -> Self {
        Self {
            system: Some(PathBuf::from(config::SYSTEM_PATH)),
            user: config::user_path(),
            env: Arc::new(ts_foundation::fsx::env_nonempty),
        }
    }

    /// The files at these two places and no environment (tests, and anything that must not read
    /// the real one).
    pub fn at(system: Option<PathBuf>, user: Option<PathBuf>) -> Self {
        Self { system, user, env: Arc::new(|_| None) }
    }

    /// Loads the configuration now: defaults, then the system file, then the user's.
    pub fn load(&self) -> Result<Config, ConfigError> {
        Config::load(self.system.as_deref(), self.user.as_deref(), self.env.as_ref())
    }

    /// The configuration, or the defaults when a file is broken: a command must not fail because
    /// of a typo in a setting it does not use. `config validate` reports the typo.
    pub fn load_or_default(&self) -> Config {
        self.load().unwrap_or_default()
    }

    /// Merges `updates` into the user's file.
    pub fn write_user(&self, updates: Table) -> ApiResult<()> {
        let path = self
            .user
            .as_deref()
            .ok_or_else(|| ApiError::invalid("there is no user configuration file (HOME is not set)"))?;
        config::write_user(path, updates)?;
        Ok(())
    }
}

/// Every dependency of the application layer. All fields are public: the assembly layer builds
/// the real one with a struct literal, [`testing`] builds a hermetic one. Nothing here reads the
/// real machine by itself.
pub struct Services {
    /// The daemon's control socket (`ts_ipc::client::Client::from_env()`).
    pub daemon: Arc<dyn Daemon>,
    /// The firmware features (`Hw::new(KernelDevice)`, with the keyboard's timeout flag and the
    /// rear light's memory plugged in).
    pub hw: Hw,
    /// The Linux side readings: CPU and GPU temperature, mains power.
    pub linux: LinuxSensors,
    /// Battery and load readings from `/sys` and `/proc`.
    pub sampler: Arc<Sampler>,
    /// The keyboard backlight.
    pub keyboard: Arc<Keyboard>,
    /// The per-user memory (`user.json`).
    pub user: Arc<userstate::Store>,
    /// The configuration files.
    pub config: ConfigFiles,
    /// The key-name table of this laptop.
    pub keymap: &'static KeyMap,
    /// The user's fan curves.
    pub curves: curves::Store,
    /// The user's Smart auto profiles.
    pub smart: smartprofile::Store,
    /// The user's CPU undervolt profiles.
    pub cpu_undervolt: undervolt::Store,
    /// The user's GPU undervolt profiles.
    pub gpu_undervolt: gpuundervolt::Store,
    /// The saved looks (keyboard and rear light).
    pub looks: LookStore,
    /// The saved per-key lighting profiles.
    pub lighting: LightingProfiles,
    /// The still designs.
    pub designs: DesignStore,
    /// The per-key animation settings.
    pub fx: FxStore,
    /// Decodes a picture file for `keyboard image` (the Python edition used Pillow); `None` makes
    /// that command say it is not available.
    pub image: Option<Arc<dyn ImageDecoder>>,
    /// The sound system.
    pub audio: Arc<Audio>,
    /// The CPU power limits, read straight from the hardware.
    pub power_limits: PowerLimits,
    /// The other power managers (here only for the health check's frequency-ceiling probe).
    pub power_managers: Arc<PowerManagers>,
    /// The touchpad switch.
    pub touchpad: Touchpad,
    /// Runs the PREDATOR-key actions that commands share (`power-next`, `rear:toggle`,
    /// `keyboard:on`, `keys test`): a key press and a command must be the same code.
    pub actions: Arc<dyn ActionRunner>,
    /// The key binding profiles.
    pub key_profiles: KeyProfiles,
    /// The whole-setup profiles.
    pub bundles: Arc<dyn ProfileBundles>,
    /// Finds and installs releases.
    pub updates: Arc<dyn UpdateService>,
    /// Checks and repairs the services; `None` where the process cannot (a window without
    /// systemd access).
    pub service_checks: Option<Arc<dyn ServiceChecks>>,
    /// Shows a short on-screen message.
    pub notifier: Arc<dyn Notifier>,
    /// Finds the keyboard's Fn-key hidraw node for the health check.
    pub fn_key_node: NodeFinder,
    /// Builds the health check's view of the machine from the adapters of [`DoctorAdapters`]
    /// (the real machine: `ts_system::doctor::Env::system`).
    pub doctor_env: DoctorEnvBuilder,
    /// Where the model probe looks.
    pub probe: ProbeOptions,
    /// The prefix of `/sys` for the readings made here (RAPL energy): `/` on a real system.
    pub root: PathBuf,
    /// The user's autostart directory (`~/.config/autostart`); `None` without `HOME`.
    pub autostart_dir: Option<PathBuf>,
    /// The effective user id of this process (0 is root).
    pub euid: u32,
    /// Wall-clock seconds since the epoch.
    pub now: Clock,
    /// Pauses (the legacy fan path waits one tick of an old daemon); tests pass a recorder.
    pub sleep: Sleeper,
}

/// The application layer: every operation, over one [`Services`].
pub struct Api {
    pub(crate) s: Services,
    rapl: status::RaplMeter,
    fans: status::FanCache,
    monitor: monitor::Monitor,
}

impl Api {
    /// The application layer over these services.
    pub fn new(services: Services) -> Self {
        let monitor = monitor::Monitor::new(services.root.clone());
        Self { s: services, rapl: status::RaplMeter::default(), fans: status::FanCache::default(), monitor }
    }

    /// The services this layer was built from, for a caller that needs one directly (the terminal
    /// UI's adapter, the key service).
    pub fn services(&self) -> &Services {
        &self.s
    }

    /// The configuration now (files over defaults); a broken file is an error.
    pub fn load_config(&self) -> Result<Config, ConfigError> {
        self.s.config.load()
    }

    /// The configuration, or the defaults when a file is broken.
    pub(crate) fn cfg(&self) -> Config {
        self.s.config.load_or_default()
    }

    /// Runs a PREDATOR-key action and answers the message the key would show.
    pub(crate) fn run_action(&self, spec: &str) -> ApiResult<String> {
        Ok(self.s.actions.run(spec)?)
    }
}

/// A list of profiles (fan curves, Smart auto profiles, undervolts, key bindings, ...): the names
/// in display order (built-in ones first), the one in use when the service says so, and a
/// one-line description of each built-in one.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct NamedList {
    /// The names, built-in first.
    pub names: Vec<String>,
    /// The one in use, when known.
    pub current: Option<String>,
    /// One line for each built-in profile.
    pub descriptions: BTreeMap<String, String>,
}

impl fmt::Display for NamedList {
    /// `* name  - description` with the star on the one in use, as the Python edition listed them.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, name) in self.names.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            let marker = if self.current.as_deref() == Some(name.as_str()) { "* " } else { "  " };
            write!(f, "{marker}{name}")?;
            if let Some(d) = self.descriptions.get(name) {
                write!(f, "  - {d}")?;
            }
        }
        Ok(())
    }
}

/// The word of a switch as the commands print it: `on` or `off`.
pub fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// Like [`on_off`] for a reading that may be unknown.
pub fn on_off_opt(on: Option<bool>) -> &'static str {
    on.map_or("unknown", on_off)
}

/// A switch the way the Python `tri` helper took it: `on`, `off`, `toggle` or `status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Switch {
    /// Turn it on.
    On,
    /// Turn it off.
    Off,
    /// Turn it the other way.
    Toggle,
    /// Only read it.
    Status,
}

impl Switch {
    /// The value to set given the current one, or `None` for a plain status question.
    pub fn target(self, current: bool) -> Option<bool> {
        match self {
            Self::On => Some(true),
            Self::Off => Some(false),
            Self::Toggle => Some(!current),
            Self::Status => None,
        }
    }
}

impl std::str::FromStr for Switch {
    type Err = ApiError;

    fn from_str(text: &str) -> Result<Self, ApiError> {
        match text {
            "on" => Ok(Self::On),
            "off" => Ok(Self::Off),
            "toggle" => Ok(Self::Toggle),
            "status" | "" => Ok(Self::Status),
            other => Err(ApiError::invalid(format!("'{other}': use on, off, toggle or status"))),
        }
    }
}
