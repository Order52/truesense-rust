//! Typed configuration: built-in defaults, overridden by `/etc/truesense/config.toml` (the
//! machine's, root-owned) and then `~/.config/truesense/config.toml` (the user's own).
//!
//! Compatibility is the point of this module. Config files written for the Python and Go
//! editions (0.3.x, 0.4.x) must keep working unchanged, so:
//!
//! * every key is optional and a missing file is not an error;
//! * layering is per key, exactly as the Python `merge()`: a nested table merges, a list or a
//!   scalar replaces (a user's `levels = [30, 50]` is not appended to the default list);
//! * keys this edition does not know are ignored when loading (a newer or older edition may have
//!   written them) and kept when [`write_user`] rewrites the file; [`unknown_keys`] reports them
//!   so a typo like `pl_normall_w` can be shown instead of silently falling back;
//! * [`Config::validate`] reports mistakes as messages without refusing to load, because the
//!   daemon must still start with a bad value (and then use the safe default path) rather than
//!   leave the fans to a crash loop.
//!
//! Integers are `i64` throughout so that an out-of-range number such as `levels = [30, 500]` loads
//! and is *explained* by `validate`, instead of failing deserialisation with a type error.
//! The files the loader reads are not trusted to be small or well formed; a bad file gives a
//! [`ConfigError`] that names it.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fsx::{self, FsError};

pub use toml::{Table, Value};

/// The machine-wide configuration file.
pub const SYSTEM_PATH: &str = "/etc/truesense/config.toml";

/// The five power modes a configuration may name, in the order the firmware cycles them.
pub const MODES: [&str; 5] = ["eco", "quiet", "balanced", "performance", "turbo"];

/// The user's own configuration file: `$TRUESENSE_CONFIG`, else `$HOME/.config/truesense/config.toml`.
/// `None` when neither is known (no `HOME`), in which case only the system file applies.
///
/// `TRUESENSE_CONFIG` is an addition of the Go edition; the Python edition used only `HOME`.
pub fn user_path_from(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if let Some(p) = env("TRUESENSE_CONFIG") {
        return Some(PathBuf::from(p));
    }
    env("HOME").map(|h| PathBuf::from(h).join(".config/truesense/config.toml"))
}

/// [`user_path_from`] for the running process.
pub fn user_path() -> Option<PathBuf> {
    user_path_from(&fsx::env_nonempty)
}

/// Why a configuration could not be loaded or saved. Every variant that concerns a file names it.
#[derive(Debug)]
pub enum ConfigError {
    /// The file exists but could not be read (permissions, too large, not UTF-8, ...).
    Read(FsError),
    /// The file is not valid TOML.
    Syntax {
        /// The offending file.
        path: PathBuf,
        /// What the parser said, with line and column when known.
        message: String,
    },
    /// The file is valid TOML but a value has the wrong type (`pl_normal_w = "high"`).
    Type {
        /// The offending file.
        path: PathBuf,
        /// What did not fit.
        message: String,
    },
    /// The updated file could not be written.
    Write(FsError),
    /// The merged settings could not be turned into TOML (a value TOML cannot hold).
    Encode(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Read(e) | ConfigError::Write(e) => e.fmt(f),
            ConfigError::Syntax { path, message } | ConfigError::Type { path, message } => {
                write!(f, "{}: {message}", path.display())
            }
            ConfigError::Encode(m) => write!(f, "cannot write TOML: {m}"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Read(e) | ConfigError::Write(e) => Some(e),
            _ => None,
        }
    }
}

/// The whole configuration. `Serialize` is for `truesense config show`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The desktop user whose files the daemon touches. Empty in the file means `SUDO_USER`, then `USER`.
    pub user: String,
    /// What the service does by itself.
    pub daemon: DaemonConfig,
    /// The smart fan curve.
    pub fan: FanConfig,
    /// CPU power limits and the thermal clock governor.
    pub cpu: CpuConfig,
    /// "Native auto": follows the power source only, like PredatorSense on Windows.
    pub native: NativeConfig,
    /// The Smart auto engine's thresholds.
    pub smart: SmartConfig,
    /// How a picked power mode is held against the automatic one.
    pub profile: ProfileConfig,
    /// The PREDATOR key gestures.
    pub keys: KeysConfig,
    /// What is applied at login.
    pub startup: StartupConfig,
    /// Keyboard lighting behaviour.
    pub keyboard: KeyboardConfig,
    /// Jack handling and microphone defaults.
    pub audio: AudioConfig,
    /// Update checks.
    pub update: UpdateConfig,
    /// On-screen notifications.
    pub osd: OsdConfig,
    /// Shims for old status-bar widgets.
    pub compat: CompatConfig,
    /// Desktop alerts the daemon raises by itself (added in 0.5; the Python and Go editions have no such section).
    pub alerts: AlertsConfig,
}

/// `[daemon]`: a first install behaves like PredatorSense (firmware fans, native auto mode,
/// nothing of ours in the way); the user opts in to more.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DaemonConfig {
    /// Drive the fans from the smart curve instead of the firmware.
    pub smart_fan: bool,
    /// Pick the power mode automatically.
    pub auto_profile: bool,
}

/// `[fan]`: the smart curve. Level `i` is entered when the CPU reaches `cpu_up[i]` or the GPU
/// `gpu_up[i]` degrees C; the three lists must have equal length (checked by `validate`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FanConfig {
    /// Fan duty in percent for each level (20..100: below 20 the firmware stalls the fan).
    pub levels: Vec<i64>,
    /// CPU temperature at which each level is entered, ascending.
    pub cpu_up: Vec<i64>,
    /// GPU temperature at which each level is entered, ascending.
    pub gpu_up: Vec<i64>,
    /// Degrees below the entry temperature before a level is left.
    pub hysteresis: i64,
    /// Seconds a lower level must hold before the fan really slows down.
    pub down_hold: i64,
    /// Degrees of look-ahead added to a rising temperature.
    pub predict: i64,
    /// Degrees by which the curve moves on battery.
    pub battery_shift: i64,
    /// Degrees by which the hotter chip pulls the other one's fan.
    pub couple: i64,
    /// CPU temperature at which the curve jumps to full speed.
    pub hot_cpu: i64,
    /// GPU temperature at which the curve jumps to full speed.
    pub hot_gpu: i64,
    /// CPU temperature under which the machine counts as calm.
    pub calm_cpu: i64,
    /// GPU temperature under which the machine counts as calm.
    pub calm_gpu: i64,
    /// Seconds of calm before the fans may drop to the lowest level.
    pub calm_hold: i64,
}

/// `[cpu]`: power limits (25..120 W, enforced by `validate`) and the thermal clock governor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CpuConfig {
    /// Sustained limit in watts for normal use.
    pub pl_normal_w: i64,
    /// Sustained limit in watts while gaming.
    pub pl_gaming_w: i64,
    /// Temperature above which the governor lowers the clock cap.
    pub gov_hi: i64,
    /// Temperature below which the governor raises the clock cap again.
    pub gov_lo: i64,
    /// Whether the governor runs.
    pub gov_enabled: bool,
    /// Highest clock cap in kHz.
    pub cap_max_khz: i64,
    /// Lowest clock cap in kHz.
    pub cap_min_khz: i64,
    /// kHz removed per governor step when hot.
    pub cap_down_khz: i64,
    /// kHz given back per governor step when cool.
    pub cap_up_khz: i64,
}

/// `[native]`: the power mode for each power source in native auto.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NativeConfig {
    /// Mode on mains power.
    pub ac: String,
    /// Mode on battery.
    pub battery: String,
}

/// `[smart]`: the Smart auto engine. Names ending in `_idle`/`_browsing`/`_busy`/`_hot` are power modes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SmartConfig {
    /// Mode on mains when idle.
    pub ac_idle: String,
    /// Mode on mains for light use.
    pub ac_browsing: String,
    /// Mode on mains under load.
    pub ac_busy: String,
    /// Whether sustained load on mains may escalate to turbo.
    pub ac_turbo: bool,
    /// Seconds of heavy load before turbo is considered.
    pub turbo_after_s: i64,
    /// Turbo is refused above this CPU temperature.
    pub turbo_max_cpu: i64,
    /// Turbo is refused above this GPU temperature.
    pub turbo_max_gpu: i64,
    /// Seconds to stay out of turbo after a refusal.
    pub backoff_hold_s: i64,
    /// Seconds of low load before the machine counts as idle.
    pub idle_after_s: i64,
    /// Mode on battery when idle.
    pub battery_idle: String,
    /// Mode on battery for light use.
    pub battery_browsing: String,
    /// Mode on battery under load.
    pub battery_busy: String,
    /// Mode on battery when hot.
    pub battery_hot: String,
    /// Battery percentage under which the engine turns frugal.
    pub battery_low_pct: i64,
    /// Seconds a higher mode must be wanted before it is applied.
    pub up_dwell_s: i64,
    /// Seconds a lower mode must be wanted before it is applied.
    pub down_dwell_s: i64,
}

/// `[profile]`: how long a hand-picked mode outranks the automatic one, and whether to show it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileConfig {
    /// Show an on-screen message when the mode changes.
    pub osd: bool,
    /// Minutes a manual choice is kept before auto may change it.
    pub hold_minutes: i64,
    /// What the automatic profile picks in each situation.
    pub auto: ProfileAuto,
}

/// `[profile.auto]`: power mode per situation. The Python edition does not validate these names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileAuto {
    /// Mode on mains under load.
    pub ac_busy: String,
    /// Mode on mains otherwise.
    pub ac_normal: String,
    /// Mode on battery under load.
    pub battery_busy: String,
    /// Mode on battery when idle.
    pub battery_idle: String,
}

/// `[keys]`: what each gesture on the PREDATOR key does. Actions are free text, interpreted
/// by the actions layer (`power-next`, `exec:truesense gui`, `none`, ...).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeysConfig {
    /// Name of the key profile these settings came from (`default`, `custom`, `native`, ...).
    pub profile: String,
    /// Single tap.
    pub tap: String,
    /// Double tap.
    pub double: String,
    /// Triple tap.
    pub triple: String,
    /// Long press.
    pub long: String,
    /// Fn+Esc.
    pub fn_esc: String,
    /// Milliseconds that make a press "long".
    pub long_ms: i64,
    /// Milliseconds within which further taps join the same gesture.
    pub multi_ms: i64,
    /// The order `power-next` cycles through.
    pub power_cycle: Vec<String>,
}

/// `[startup]`: what is applied at login. `keyboard`: last | default | none | off | profile:NAME |
/// look:NAME. `rear`: last | default | none | off | look:NAME. `profile` is a profile name or empty.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StartupConfig {
    /// Profile applied at login, empty for none.
    pub profile: String,
    /// Keyboard lighting at login.
    pub keyboard: String,
    /// Rear lighting at login.
    pub rear: String,
}

/// `[keyboard]`: lighting that follows the power mode.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyboardConfig {
    /// Re-send the operating-mode colour on every power-mode change (the keyboard does not do it itself).
    pub operating_follow: bool,
    /// Per-mode colour overrides, mode name to `RRGGBB`. Free-form on purpose: unknown mode
    /// names are not a typo worth reporting.
    pub operating_colors: BTreeMap<String, String>,
}

/// `[audio]`: jack handling and microphone modes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// `auto`, `headphones` or `headset`: what a plug in the 3.5 mm jack is.
    pub jack: String,
    /// Notify when the microphone changes.
    pub notify: bool,
    /// Put every output on the DTS-processed sink.
    pub dts_all: bool,
    /// Microphone noise-reduction mode for a headset.
    pub headset_mode: String,
    /// Microphone noise-reduction mode for the built-in array.
    pub array_mode: String,
}

/// `[update]`: automatic checks are off until the user switches them on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateConfig {
    /// Check for a new release by itself.
    pub auto_check: bool,
    /// Hours between checks (1..168).
    pub check_hours: i64,
    /// GitHub `owner/name` that releases are fetched from. Only the root-owned system file is trusted for this.
    pub repo: String,
}

/// `[osd]`: how notifications appear (`native`, `noctalia` or `none`; `TRUESENSE_OSD` overrides).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OsdConfig {
    /// The style.
    pub style: String,
}

/// `[compat]`: shims for old status-bar widgets.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CompatConfig {
    /// Also write `/tmp/predator-state` and honour `~/.cache/predator-fan-mode`.
    pub predator_widgets: bool,
}

/// `[alerts]`: what the root daemon tells the desktop user without being asked (a hot CPU or GPU,
/// a low battery, a clock held down by the governor, a lost sensor). It lives in the daemon so it
/// works with every window closed. The urgency is never "critical" (a desktop never dismisses
/// those), so there is no key for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlertsConfig {
    /// Raise alerts at all.
    pub enabled: bool,
    /// Alert when the CPU temperature reaches this many degrees C (70..100). It re-arms 5 C below.
    pub cpu_limit_c: i64,
    /// The same for the GPU (70..100).
    pub gpu_limit_c: i64,
    /// Least minutes between two messages of the same alert (1..240), so a laptop that hovers
    /// around the limit does not pester.
    pub repeat_minutes: i64,
    /// How many consecutive ticks (one or two seconds each) a condition must hold before it is
    /// reported (1..30).
    pub debounce_samples: i64,
    /// Alert on battery at or below this percentage (5..50); 0 switches the battery alert off.
    pub battery_low_pct: i64,
    /// Tell when the thermal governor is holding the CPU clock down.
    pub throttling: bool,
    /// Tell when the CPU temperature cannot be read.
    pub sensor_lost: bool,
}

/// The smallest and largest temperature limit an alert may use.
pub const ALERT_LIMIT_RANGE: std::ops::RangeInclusive<i64> = 70..=100;

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

impl Default for DaemonConfig {
    fn default() -> Self {
        DaemonConfig { smart_fan: false, auto_profile: true }
    }
}

impl Default for FanConfig {
    fn default() -> Self {
        FanConfig {
            levels: vec![30, 40, 50, 60, 70, 85, 100],
            cpu_up: vec![52, 60, 68, 75, 80, 85, 89],
            gpu_up: vec![48, 55, 61, 66, 70, 74, 79],
            hysteresis: 4,
            down_hold: 8,
            predict: 5,
            battery_shift: 3,
            couple: 20,
            hot_cpu: 92,
            hot_gpu: 82,
            calm_cpu: 58,
            calm_gpu: 55,
            calm_hold: 20,
        }
    }
}

impl Default for CpuConfig {
    fn default() -> Self {
        CpuConfig {
            pl_normal_w: 80,
            pl_gaming_w: 65,
            gov_hi: 88,
            gov_lo: 83,
            gov_enabled: true,
            cap_max_khz: 5_000_000,
            cap_min_khz: 2_800_000,
            cap_down_khz: 200_000,
            cap_up_khz: 100_000,
        }
    }
}

impl Default for NativeConfig {
    fn default() -> Self {
        NativeConfig { ac: "balanced".into(), battery: "eco".into() }
    }
}

impl Default for SmartConfig {
    fn default() -> Self {
        SmartConfig {
            ac_idle: "balanced".into(),
            ac_browsing: "balanced".into(),
            ac_busy: "performance".into(),
            ac_turbo: true,
            turbo_after_s: 30,
            turbo_max_cpu: 85,
            turbo_max_gpu: 75,
            backoff_hold_s: 90,
            idle_after_s: 60,
            battery_idle: "eco".into(),
            battery_browsing: "eco".into(),
            battery_busy: "balanced".into(),
            battery_hot: "quiet".into(),
            battery_low_pct: 20,
            up_dwell_s: 5,
            down_dwell_s: 30,
        }
    }
}

impl Default for ProfileConfig {
    fn default() -> Self {
        ProfileConfig { osd: true, hold_minutes: 10, auto: ProfileAuto::default() }
    }
}

impl Default for ProfileAuto {
    fn default() -> Self {
        ProfileAuto {
            ac_busy: "turbo".into(),
            ac_normal: "balanced".into(),
            battery_busy: "balanced".into(),
            battery_idle: "eco".into(),
        }
    }
}

impl Default for KeysConfig {
    fn default() -> Self {
        KeysConfig {
            profile: "default".into(),
            tap: "power-next".into(),
            double: "output-toggle".into(),
            triple: "none".into(),
            long: "fan-max-toggle".into(),
            fn_esc: "none".into(),
            long_ms: 800,
            multi_ms: 450,
            power_cycle: strings(&MODES),
        }
    }
}

impl Default for StartupConfig {
    fn default() -> Self {
        StartupConfig { profile: String::new(), keyboard: "last".into(), rear: "last".into() }
    }
}

impl Default for AudioConfig {
    fn default() -> Self {
        AudioConfig {
            jack: "auto".into(),
            notify: true,
            dts_all: true,
            headset_mode: "conference".into(),
            array_mode: "conference".into(),
        }
    }
}

impl Default for UpdateConfig {
    fn default() -> Self {
        UpdateConfig { auto_check: false, check_hours: 24, repo: "Order52/truesense".into() }
    }
}

impl Default for OsdConfig {
    fn default() -> Self {
        OsdConfig { style: "native".into() }
    }
}

impl Default for AlertsConfig {
    fn default() -> Self {
        AlertsConfig {
            enabled: true,
            cpu_limit_c: 90,
            gpu_limit_c: 90,
            repeat_minutes: 5,
            debounce_samples: 3,
            battery_low_pct: 15,
            throttling: true,
            sensor_lost: true,
        }
    }
}

impl AlertsConfig {
    /// The mistakes in this section, one message each (empty means fine). Also run by
    /// [`Config::validate`], and by the commands that write the section before they write it.
    pub fn validate(&self) -> Vec<String> {
        let mut out = Vec::new();
        let range = ALERT_LIMIT_RANGE;
        for (key, v) in [("cpu_limit_c", self.cpu_limit_c), ("gpu_limit_c", self.gpu_limit_c)] {
            if !range.contains(&v) {
                out.push(format!("[alerts] {key} must be {}..{}", range.start(), range.end()));
            }
        }
        if !(1..=240).contains(&self.repeat_minutes) {
            out.push("[alerts] repeat_minutes must be 1..240".to_owned());
        }
        if !(1..=30).contains(&self.debounce_samples) {
            out.push("[alerts] debounce_samples must be 1..30".to_owned());
        }
        if self.battery_low_pct != 0 && !(5..=50).contains(&self.battery_low_pct) {
            out.push("[alerts] battery_low_pct must be 0 (off) or 5..50".to_owned());
        }
        out
    }
}

/// Merges `over` into `base` the way the Python `merge()` does: a table on both sides merges
/// key by key, anything else (scalar, list, or a table over a non-table) replaces.
pub fn merge(base: &mut Table, over: Table) {
    for (key, value) in over {
        match (base.get_mut(&key), value) {
            (Some(Value::Table(b)), Value::Table(o)) => merge(b, o),
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

/// A one-section update for [`write_user`]: `section_table("keys", [("tap", "none".into())])`.
pub fn section_table<'a>(section: &str, pairs: impl IntoIterator<Item = (&'a str, Value)>) -> Table {
    let inner: Table = pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
    let mut t = Table::new();
    t.insert(section.to_owned(), Value::Table(inner));
    t
}

impl Config {
    /// Loads the defaults, then `system`, then `user` over them (a `None` path is skipped).
    ///
    /// A file that does not exist is skipped; one that cannot be parsed, or that gives a key
    /// the wrong type, is an error naming that file. When `user` is empty after merging it
    /// falls back to `SUDO_USER`, then `USER`, from `env` (so `sudo truesense` acts for the
    /// person who typed it), and to the empty string if neither is set.
    pub fn load(
        system: Option<&Path>,
        user: Option<&Path>,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Config, ConfigError> {
        let mut merged =
            Table::try_from(Config::default()).map_err(|e| ConfigError::Encode(e.to_string()))?;
        let mut cfg = Config::default();
        for path in [system, user].into_iter().flatten() {
            let Some(layer) = read_table(path)? else { continue };
            merge(&mut merged, layer);
            // Checked after every layer so a type error is blamed on the file that caused it.
            cfg = merged.clone().try_into().map_err(|e: toml::de::Error| ConfigError::Type {
                path: path.to_path_buf(),
                message: e.to_string(),
            })?;
        }
        if cfg.user.is_empty() {
            cfg.user = env("SUDO_USER").or_else(|| env("USER")).unwrap_or_default();
        }
        Ok(cfg)
    }

    /// [`Config::load`] at the standard places with the real environment: the system file and
    /// the current user's file.
    pub fn load_standard() -> Result<Config, ConfigError> {
        let user = user_path();
        Config::load(Some(Path::new(SYSTEM_PATH)), user.as_deref(), &fsx::env_nonempty)
    }

    /// Returns the problems that would otherwise silently fall back to a default or misbehave
    /// (empty means fine). The text of each message is what `truesense config validate` prints,
    /// and is the same as the Python edition's.
    pub fn validate(&self) -> Vec<String> {
        let mut out = Vec::new();
        let f = &self.fan;
        if !(f.levels.len() == f.cpu_up.len() && f.levels.len() == f.gpu_up.len()) {
            out.push("[fan] levels, cpu_up and gpu_up must have the same length".to_owned());
        }
        if f.levels.iter().any(|x| !(20..=100).contains(x)) {
            out.push("[fan] levels must be 20..100".to_owned());
        }
        if !f.cpu_up.is_sorted() || !f.gpu_up.is_sorted() {
            out.push("[fan] cpu_up / gpu_up must be ascending".to_owned());
        }
        let c = &self.cpu;
        if !(25..=120).contains(&c.pl_normal_w) || !(25..=120).contains(&c.pl_gaming_w) {
            out.push("[cpu] power limits must be 25..120 W".to_owned());
        }
        if !matches!(self.audio.jack.as_str(), "auto" | "headphones" | "headset") {
            out.push("[audio] jack must be auto, headphones or headset".to_owned());
        }
        let modes = MODES.join(", ");
        let s = &self.smart;
        let smart_modes = [
            ("ac_idle", &s.ac_idle),
            ("ac_browsing", &s.ac_browsing),
            ("ac_busy", &s.ac_busy),
            ("battery_idle", &s.battery_idle),
            ("battery_browsing", &s.battery_browsing),
            ("battery_busy", &s.battery_busy),
            ("battery_hot", &s.battery_hot),
        ];
        for (key, value) in smart_modes {
            if !MODES.contains(&value.as_str()) {
                out.push(format!("[smart] {key} must be one of {modes}"));
            }
        }
        for (key, value) in [("ac", &self.native.ac), ("battery", &self.native.battery)] {
            if !MODES.contains(&value.as_str()) {
                out.push(format!("[native] {key} must be one of {modes}"));
            }
        }
        if !startup_ok(&self.startup.keyboard, &["profile:", "look:"]) {
            out.push(
                "[startup] keyboard must be last, default, none, off, profile:NAME or look:NAME".to_owned(),
            );
        }
        if !startup_ok(&self.startup.rear, &["look:"]) {
            out.push("[startup] rear must be last, default, none, off or look:NAME".to_owned());
        }
        if !(1..=168).contains(&self.update.check_hours) {
            out.push("[update] check_hours must be 1..168".to_owned());
        }
        if !matches!(self.osd.style.as_str(), "native" | "noctalia" | "none") {
            out.push("[osd] style must be native, noctalia or none".to_owned());
        }
        out.extend(self.alerts.validate());
        out
    }
}

fn startup_ok(value: &str, prefixes: &[&str]) -> bool {
    matches!(value, "last" | "default" | "none" | "off") || prefixes.iter().any(|p| value.starts_with(p))
}

/// Reads and parses one TOML file. `Ok(None)` when it does not exist.
fn read_table(path: &Path) -> Result<Option<Table>, ConfigError> {
    let text = match fsx::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.is_not_found() => return Ok(None),
        Err(e) => return Err(ConfigError::Read(e)),
    };
    match text.parse::<Table>() {
        Ok(t) => Ok(Some(t)),
        Err(e) => {
            Err(ConfigError::Syntax { path: path.to_path_buf(), message: describe_syntax_error(&text, &e) })
        }
    }
}

/// The parser's message with a `line N, column M` that is safe to compute for any input.
fn describe_syntax_error(text: &str, e: &toml::de::Error) -> String {
    match e.span() {
        Some(span) => {
            let before = &text.as_bytes()[..span.start.min(text.len())];
            let line = before.iter().filter(|&&b| b == b'\n').count() + 1;
            let column = before.iter().rev().take_while(|&&b| b != b'\n').count() + 1;
            format!("{} (line {line}, column {column})", e.message())
        }
        None => e.message().to_owned(),
    }
}

/// Merges `updates` (a nested table) into the user's file at `path` and writes it back
/// atomically (creating the directory), returning the file's new content. Keys the update does
/// not mention, including ones this edition does not know, are kept. Comments in the file are
/// not: TOML has no comment-preserving writer here, as in the Python edition.
///
/// An existing file that is not valid TOML is an error and is left as it is.
pub fn write_user(path: &Path, updates: Table) -> Result<Table, ConfigError> {
    let mut current = read_table(path)?.unwrap_or_default();
    merge(&mut current, updates);
    let text = toml::to_string(&current).map_err(|e| ConfigError::Encode(e.to_string()))?;
    fsx::write_atomic_mkdir(path, text.as_bytes(), 0o644).map_err(ConfigError::Write)?;
    Ok(current)
}

/// Lists the keys in the file at `path` that this edition does not know, as `section.key`
/// (usually a typo or a key from another edition). Sorted. A missing, unreadable or malformed
/// file has none: the load error is the report in that case.
pub fn unknown_keys(path: &Path) -> Vec<String> {
    let Ok(Some(found)) = read_table(path) else { return Vec::new() };
    let Ok(known) = Table::try_from(Config::default()) else { return Vec::new() };
    let mut out = Vec::new();
    walk_unknown("", &found, &known, &mut out);
    out
}

fn walk_unknown(prefix: &str, found: &Table, known: &Table, out: &mut Vec<String>) {
    for (key, value) in found {
        match (known.get(key), value) {
            (None, _) => out.push(format!("{prefix}{key}")),
            // keyboard.operating_colors is a free-form table: its keys are not checked.
            (Some(Value::Table(k)), Value::Table(f))
                if !(prefix == "keyboard." && key == "operating_colors") =>
            {
                walk_unknown(&format!("{prefix}{key}."), f, k, out);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::fsx::testing::TempDir;
    use std::fs;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn put(dir: &TempDir, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, body).expect("write fixture");
        p
    }

    fn table(text: &str) -> Table {
        text.parse().expect("test TOML")
    }

    #[test]
    fn defaults_are_valid_and_match_the_python_defaults() {
        let c = Config::default();
        assert_eq!(c.validate(), Vec::<String>::new());
        // A sample of the values the Python DEFAULTS table fixes (config.py).
        assert_eq!(c.fan.levels, [30, 40, 50, 60, 70, 85, 100]);
        assert_eq!(c.fan.cpu_up, [52, 60, 68, 75, 80, 85, 89]);
        assert_eq!(c.fan.gpu_up, [48, 55, 61, 66, 70, 74, 79]);
        assert_eq!((c.fan.hot_cpu, c.fan.hot_gpu, c.fan.calm_cpu, c.fan.calm_gpu), (92, 82, 58, 55));
        assert_eq!(
            (c.cpu.pl_normal_w, c.cpu.pl_gaming_w, c.cpu.cap_max_khz, c.cpu.cap_min_khz),
            (80, 65, 5_000_000, 2_800_000)
        );
        assert!(!c.daemon.smart_fan && c.daemon.auto_profile);
        assert_eq!((c.native.ac.as_str(), c.native.battery.as_str()), ("balanced", "eco"));
        assert_eq!(
            (c.smart.ac_busy.as_str(), c.smart.battery_hot.as_str(), c.smart.down_dwell_s),
            ("performance", "quiet", 30)
        );
        assert_eq!((c.profile.hold_minutes, c.profile.auto.ac_busy.as_str()), (10, "turbo"));
        assert_eq!(
            (c.keys.tap.as_str(), c.keys.long.as_str(), c.keys.long_ms, c.keys.multi_ms),
            ("power-next", "fan-max-toggle", 800, 450)
        );
        assert_eq!(c.keys.power_cycle, ["eco", "quiet", "balanced", "performance", "turbo"]);
        assert_eq!(
            (c.startup.keyboard.as_str(), c.startup.rear.as_str(), c.startup.profile.as_str()),
            ("last", "last", "")
        );
        assert!(!c.update.auto_check && c.update.check_hours == 24 && c.update.repo == "Order52/truesense");
        assert_eq!(c.osd.style, "native");
        assert!(!c.compat.predator_widgets && c.keyboard.operating_colors.is_empty());
        assert!(c.audio.notify && c.audio.dts_all && c.audio.jack == "auto");
    }

    #[test]
    fn defaults_equal_the_python_editions_merged_config() {
        // testdata/python-config-defaults.json is `truesense --json config show` of the Python
        // edition (0.3.17) with no config files, `user` blanked: every key, every value, same names.
        let golden: serde_json::Value =
            serde_json::from_str(include_str!("../testdata/python-config-defaults.json"))
                .expect("golden JSON");
        // The Python edition has no `[alerts]` section (it is new in 0.5): everything else is its.
        let mut ours = serde_json::to_value(Config::default()).expect("encode");
        let alerts = ours.as_object_mut().expect("object").remove("alerts").expect("alerts section");
        assert_eq!(ours, golden);
        assert_eq!(
            alerts,
            serde_json::json!({
                "enabled": true,
                "cpu_limit_c": 90,
                "gpu_limit_c": 90,
                "repeat_minutes": 5,
                "debounce_samples": 3,
                "battery_low_pct": 15,
                "throttling": true,
                "sensor_lost": true
            })
        );
    }

    #[test]
    fn alerts_section_loads_validates_and_is_known() {
        let d = TempDir::new("cfg-alerts");
        let p = put(
            &d,
            "c.toml",
            "[alerts]\nenabled = false\ncpu_limit_c = 85\nrepeat_minutes = 10\nbattery_lo_pct = 1\n",
        );
        let c = Config::load(None, Some(&p), &no_env).expect("load");
        assert!(!c.alerts.enabled);
        assert_eq!((c.alerts.cpu_limit_c, c.alerts.gpu_limit_c, c.alerts.repeat_minutes), (85, 90, 10));
        assert_eq!(unknown_keys(&p), ["alerts.battery_lo_pct"]);
        assert_eq!(c.validate(), Vec::<String>::new());
    }

    #[test]
    fn alerts_validation_boundaries_and_messages() {
        let check = |f: &dyn Fn(&mut AlertsConfig)| {
            let mut c = Config::default();
            f(&mut c.alerts);
            c.validate()
        };
        assert!(check(&|a| a.cpu_limit_c = 70).is_empty());
        assert!(check(&|a| a.gpu_limit_c = 100).is_empty());
        assert_eq!(check(&|a| a.cpu_limit_c = 69), ["[alerts] cpu_limit_c must be 70..100"]);
        assert_eq!(check(&|a| a.gpu_limit_c = 101), ["[alerts] gpu_limit_c must be 70..100"]);
        assert_eq!(check(&|a| a.repeat_minutes = 0), ["[alerts] repeat_minutes must be 1..240"]);
        assert!(check(&|a| a.repeat_minutes = 240).is_empty());
        assert_eq!(check(&|a| a.repeat_minutes = 241).len(), 1);
        assert_eq!(check(&|a| a.debounce_samples = 31), ["[alerts] debounce_samples must be 1..30"]);
        assert!(check(&|a| a.debounce_samples = 1).is_empty());
        assert!(check(&|a| a.battery_low_pct = 0).is_empty(), "0 switches the alert off");
        assert!(check(&|a| a.battery_low_pct = 5).is_empty());
        assert_eq!(check(&|a| a.battery_low_pct = 4), ["[alerts] battery_low_pct must be 0 (off) or 5..50"]);
        assert_eq!(check(&|a| a.battery_low_pct = 51).len(), 1);
        assert_eq!(check(&|a| a.cpu_limit_c = i64::MIN).len(), 1);
    }

    #[test]
    fn defaults_survive_a_round_trip_through_toml() {
        let t = Table::try_from(Config::default()).expect("encode");
        let back: Config = t.try_into().expect("decode");
        assert_eq!(back, Config::default());
    }

    #[test]
    fn user_overrides_system_overrides_defaults_and_lists_replace() {
        let d = TempDir::new("cfg-layers");
        let sys = put(
            &d,
            "sys.toml",
            "[cpu]\npl_normal_w = 70\npl_gaming_w = 60\n[fan]\nlevels = [30, 50]\ncpu_up = [50, 70]\ngpu_up = [45, 65]\n",
        );
        let usr = put(&d, "usr.toml", "[cpu]\npl_gaming_w = 55\n");
        let c = Config::load(Some(&sys), Some(&usr), &no_env).expect("load");
        assert_eq!((c.cpu.pl_normal_w, c.cpu.pl_gaming_w), (70, 55));
        assert_eq!(c.fan.levels, [30, 50], "a list replaces the default, it is not appended");
        assert_eq!(c.fan.hysteresis, 4, "untouched keys keep the default");
        assert_eq!(c.validate(), Vec::<String>::new());
    }

    #[test]
    fn missing_files_and_missing_paths_are_fine() {
        let d = TempDir::new("cfg-missing");
        let c = Config::load(Some(&d.join("a.toml")), Some(&d.join("b.toml")), &no_env).expect("load");
        assert_eq!(c, Config::default());
        assert_eq!(Config::load(None, None, &no_env).expect("load"), Config::default());
    }

    #[test]
    fn a_syntax_error_names_the_file_and_the_line() {
        let d = TempDir::new("cfg-syntax");
        let bad = put(&d, "bad.toml", "[cpu]\npl_normal_w = 70\n[fan\nlevels = 1\n");
        let err = Config::load(None, Some(&bad), &no_env).expect_err("syntax error");
        assert!(matches!(err, ConfigError::Syntax { .. }), "{err}");
        let msg = err.to_string();
        assert!(msg.contains("bad.toml") && msg.contains("line 3"), "{msg}");
    }

    #[test]
    fn a_type_error_is_blamed_on_the_file_that_caused_it() {
        let d = TempDir::new("cfg-type");
        let good = put(&d, "sys.toml", "[cpu]\npl_normal_w = 70\n");
        let bad = put(&d, "usr.toml", "[cpu]\npl_normal_w = \"high\"\n");
        let err = Config::load(Some(&good), Some(&bad), &no_env).expect_err("type error");
        assert!(matches!(err, ConfigError::Type { .. }), "{err}");
        assert!(err.to_string().contains("usr.toml") && !err.to_string().contains("sys.toml"), "{err}");
        // A table replaced by a scalar is a type error too.
        let bad2 = put(&d, "usr2.toml", "cpu = 5\n");
        assert!(matches!(Config::load(None, Some(&bad2), &no_env), Err(ConfigError::Type { .. })));
    }

    #[test]
    fn hostile_files_are_errors_not_panics() {
        let d = TempDir::new("cfg-hostile");
        let cases: [(&str, &[u8]); 6] = [
            ("empty-key", b"= 1\n"),
            ("binary", &[0xff, 0xfe, 0x00, 0x01]),
            ("deep", b"a = [[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]\n"),
            ("unterminated", b"user = \"abc\n"),
            ("duplicate", b"[cpu]\n[cpu]\n"),
            ("multibyte-error", "user = \"é\" é\n".as_bytes()),
        ];
        for (name, body) in cases {
            let p = d.join(name);
            fs::write(&p, body).expect("write");
            assert!(Config::load(None, Some(&p), &no_env).is_err(), "{name} must be refused");
        }
        // A directory where the file should be is a read error.
        assert!(matches!(Config::load(None, Some(&d.0), &no_env), Err(ConfigError::Read(_))));
    }

    #[test]
    fn user_falls_back_to_sudo_user_then_user() {
        let sudo = |k: &str| match k {
            "SUDO_USER" => Some("alice".to_owned()),
            "USER" => Some("root".to_owned()),
            _ => None,
        };
        let plain = |k: &str| (k == "USER").then(|| "bob".to_owned());
        assert_eq!(Config::load(None, None, &sudo).expect("load").user, "alice");
        assert_eq!(Config::load(None, None, &plain).expect("load").user, "bob");
        assert_eq!(Config::load(None, None, &no_env).expect("load").user, "");
        let d = TempDir::new("cfg-user");
        let p = put(&d, "c.toml", "user = \"carol\"\n");
        assert_eq!(Config::load(Some(&p), None, &sudo).expect("load").user, "carol", "the file wins");
        let empty = put(&d, "e.toml", "user = \"\"\n");
        assert_eq!(
            Config::load(Some(&empty), None, &sudo).expect("load").user,
            "alice",
            "empty counts as unset"
        );
    }

    #[test]
    fn config_files_of_0_3_x_still_load() {
        // The author's real files, as written by the Python edition's `truesense keys set`, `audio` and `startup` commands.
        let d = TempDir::new("cfg-compat");
        let sys = put(
            &d,
            "etc.toml",
            "# TrueSense system configuration (see docs/USAGE.md). Built-in defaults apply to everything not set here.\nuser = \"sloppy\"\n",
        );
        let usr = put(
            &d,
            "user.toml",
            "\n[audio]\njack = \"headset\"\nnotify = true\n\n[keys]\nfn_esc = \"none\"\ntap = \"exec:truesense gui\"\ndouble = \"none\"\ntriple = \"none\"\nlong = \"none\"\nlong_ms = 800\nmulti_ms = 450\nprofile = \"native\"\n\n[startup]\nrear = \"off\"\nkeyboard = \"last\"\n\n[update]\nauto_check = false\n",
        );
        let c = Config::load(Some(&sys), Some(&usr), &no_env).expect("load");
        assert_eq!(c.user, "sloppy");
        assert_eq!(
            (c.audio.jack.as_str(), c.keys.tap.as_str(), c.keys.profile.as_str(), c.startup.rear.as_str()),
            ("headset", "exec:truesense gui", "native", "off")
        );
        assert_eq!(c.keys.long, "none");
        assert_eq!(c.validate(), Vec::<String>::new());
        assert!(unknown_keys(&usr).is_empty() && unknown_keys(&sys).is_empty());
    }

    #[test]
    fn a_fully_populated_0_3_x_file_loads_with_every_key() {
        let d = TempDir::new("cfg-full");
        let p = put(
            &d,
            "full.toml",
            r#"
user = "me"
[daemon]
smart_fan = true
auto_profile = false
[fan]
levels = [30, 50, 100]
cpu_up = [50, 70, 90]
gpu_up = [45, 65, 80]
hysteresis = 3
down_hold = 6
predict = 4
battery_shift = 2
couple = 10
hot_cpu = 90
hot_gpu = 80
calm_cpu = 55
calm_gpu = 50
calm_hold = 15
[cpu]
pl_normal_w = 90
pl_gaming_w = 70
gov_hi = 90
gov_lo = 80
gov_enabled = false
cap_max_khz = 4800000
cap_min_khz = 2000000
cap_down_khz = 100000
cap_up_khz = 50000
[native]
ac = "performance"
battery = "quiet"
[smart]
ac_idle = "eco"
ac_browsing = "quiet"
ac_busy = "turbo"
ac_turbo = false
turbo_after_s = 10
turbo_max_cpu = 80
turbo_max_gpu = 70
backoff_hold_s = 60
idle_after_s = 30
battery_idle = "eco"
battery_browsing = "quiet"
battery_busy = "balanced"
battery_hot = "eco"
battery_low_pct = 15
up_dwell_s = 3
down_dwell_s = 20
[profile]
osd = false
hold_minutes = 5
[profile.auto]
ac_busy = "performance"
ac_normal = "quiet"
battery_busy = "quiet"
battery_idle = "eco"
[keys]
profile = "p"
tap = "power:eco"
double = "mic-toggle"
triple = "fan-max-toggle"
long = "none"
fn_esc = "none"
long_ms = 900
multi_ms = 300
power_cycle = ["eco", "turbo"]
[startup]
profile = "work"
keyboard = "profile:rgb"
rear = "look:calm"
[keyboard]
operating_follow = true
[keyboard.operating_colors]
turbo = "ff00ff"
[audio]
jack = "headphones"
notify = false
dts_all = false
headset_mode = "noise"
array_mode = "off"
[update]
auto_check = true
check_hours = 48
repo = "me/ts"
[osd]
style = "noctalia"
[compat]
predator_widgets = true
"#,
        );
        let c = Config::load(Some(&p), None, &no_env).expect("load");
        assert_eq!(c.validate(), Vec::<String>::new());
        assert!(unknown_keys(&p).is_empty());
        assert_eq!((c.fan.levels.len(), c.fan.hysteresis, c.fan.calm_hold), (3, 3, 15));
        assert!(c.daemon.smart_fan && !c.daemon.auto_profile && !c.cpu.gov_enabled && !c.smart.ac_turbo);
        assert_eq!((c.profile.hold_minutes, c.profile.auto.ac_normal.as_str()), (5, "quiet"));
        assert_eq!(c.keys.power_cycle, ["eco", "turbo"]);
        assert_eq!(c.keyboard.operating_colors.get("turbo").map(String::as_str), Some("ff00ff"));
        assert_eq!((c.startup.keyboard.as_str(), c.startup.rear.as_str()), ("profile:rgb", "look:calm"));
        assert!(
            c.compat.predator_widgets
                && c.update.auto_check
                && c.update.repo == "me/ts"
                && c.osd.style == "noctalia"
        );
    }

    #[test]
    fn unknown_keys_do_not_stop_loading() {
        let d = TempDir::new("cfg-extra");
        let p = put(
            &d,
            "c.toml",
            "future_key = 1\n[cpu]\npl_normal_w = 70\nturbo_boost = true\n[brand_new]\nx = 1\n",
        );
        let c = Config::load(Some(&p), None, &no_env).expect("load");
        assert_eq!(c.cpu.pl_normal_w, 70);
    }

    #[test]
    fn validate_catches_mistakes() {
        let mut c = Config::default();
        c.fan.levels = vec![30, 120];
        c.fan.cpu_up = vec![70, 50];
        c.cpu.pl_gaming_w = 500;
        c.audio.jack = "bluetooth".into();
        c.smart.ac_busy = "ludicrous".into();
        c.startup.rear = "profile:x".into();
        c.update.check_hours = 0;
        c.osd.style = "fancy".into();
        let all = c.validate().join("\n");
        for want in [
            "levels must be 20..100",
            "ascending",
            "25..120",
            "[audio] jack",
            "[smart] ac_busy",
            "[startup] rear",
            "check_hours",
            "[osd] style",
            "same length",
        ] {
            assert!(all.contains(want), "missing complaint {want:?} in:\n{all}");
        }
        // The python test: ascending temperatures only.
        let mut c = Config::default();
        c.fan.cpu_up = vec![90, 10];
        assert!(!c.validate().is_empty());
    }

    #[test]
    fn validate_messages_are_the_python_ones_in_the_python_order() {
        let mut c = Config::default();
        c.fan.levels = vec![10];
        c.cpu.pl_normal_w = 24;
        c.audio.jack = "x".into();
        c.smart.battery_hot = "y".into();
        c.native.ac = "z".into();
        c.startup.keyboard = "w".into();
        c.startup.rear = "w".into();
        c.update.check_hours = 169;
        c.osd.style = "q".into();
        assert_eq!(
            c.validate(),
            [
                "[fan] levels, cpu_up and gpu_up must have the same length",
                "[fan] levels must be 20..100",
                "[cpu] power limits must be 25..120 W",
                "[audio] jack must be auto, headphones or headset",
                "[smart] battery_hot must be one of eco, quiet, balanced, performance, turbo",
                "[native] ac must be one of eco, quiet, balanced, performance, turbo",
                "[startup] keyboard must be last, default, none, off, profile:NAME or look:NAME",
                "[startup] rear must be last, default, none, off or look:NAME",
                "[update] check_hours must be 1..168",
                "[osd] style must be native, noctalia or none",
            ]
        );
    }

    #[test]
    fn validate_boundaries() {
        let ok = |f: &dyn Fn(&mut Config)| {
            let mut c = Config::default();
            f(&mut c);
            c.validate().is_empty()
        };
        assert!(ok(&|c| c.cpu.pl_normal_w = 25));
        assert!(ok(&|c| c.cpu.pl_gaming_w = 120));
        assert!(!ok(&|c| c.cpu.pl_gaming_w = 121));
        assert!(!ok(&|c| c.cpu.pl_normal_w = i64::MIN));
        assert!(ok(&|c| c.update.check_hours = 168));
        assert!(!ok(&|c| c.update.check_hours = 0));
        assert!(ok(&|c| c.startup.keyboard = "profile:".into()), "an empty NAME passes, as in Python");
        assert!(!ok(&|c| c.startup.rear = "profile:x".into()), "rear has no profile: form");
        assert!(ok(&|c| c.fan.cpu_up = vec![50, 50, 50, 50, 50, 50, 50]), "equal is not descending");
        assert!(ok(&|c| c.smart.ac_turbo = false), "non-mode keys are not mode-checked");
        assert!(
            ok(&|c| c.profile.auto.ac_busy = "anything".into()),
            "profile.auto is not validated in Python"
        );
    }

    #[test]
    fn unknown_keys_finds_typos_but_not_free_form_colours() {
        let d = TempDir::new("cfg-unknown");
        let p = put(
            &d,
            "c.toml",
            "[cpu]\npl_normal_w = 70\npl_normall_w = 1\n[nonsense]\nx = 1\n[keyboard.operating_colors]\nfree = \"form\"\n[profile.auto]\nac_bussy = \"turbo\"\n",
        );
        assert_eq!(unknown_keys(&p), ["cpu.pl_normall_w", "nonsense", "profile.auto.ac_bussy"]);
        assert!(unknown_keys(&d.join("missing")).is_empty());
        let bad = put(&d, "bad.toml", "[[[");
        assert!(unknown_keys(&bad).is_empty());
    }

    #[test]
    fn write_user_merges_and_keeps_other_keys() {
        let d = TempDir::new("cfg-write");
        let p = d.join("sub/config.toml");
        write_user(&p, section_table("keys", [("tap", "none".into())])).expect("first");
        let mut second = section_table("keys", [("double", "x".into())]);
        merge(&mut second, section_table("audio", [("notify", false.into())]));
        let cur = write_user(&p, second).expect("second");
        // The Python test: exactly these two sections with exactly these keys.
        assert_eq!(cur, table("[keys]\ntap = \"none\"\ndouble = \"x\"\n[audio]\nnotify = false\n"));
        assert_eq!(read_table(&p).expect("read").expect("exists"), cur);
        let c = Config::load(None, Some(&p), &no_env).expect("load");
        assert_eq!((c.keys.tap.as_str(), c.keys.double.as_str(), c.audio.notify), ("none", "x", false));
        assert!(fs::metadata(&p).expect("stat").len() > 0);
    }

    #[test]
    fn write_user_keeps_unknown_keys_and_other_value_types() {
        let d = TempDir::new("cfg-write-unknown");
        let p = put(
            &d,
            "c.toml",
            "future = 1\n[keys]\npower_cycle = [\"eco\", \"turbo\"]\nlong_ms = 900\n[a.b]\nc = 1.5\n",
        );
        write_user(&p, section_table("keys", [("tap", "none".into())])).expect("write");
        let t = read_table(&p).expect("read").expect("exists");
        assert_eq!(
            t,
            table(
                "future = 1\n[keys]\npower_cycle = [\"eco\", \"turbo\"]\nlong_ms = 900\ntap = \"none\"\n[a.b]\nc = 1.5\n"
            )
        );
    }

    #[test]
    fn write_user_refuses_to_overwrite_a_broken_file() {
        let d = TempDir::new("cfg-write-bad");
        let p = put(&d, "c.toml", "[keys\n");
        let err = write_user(&p, section_table("keys", [("tap", "none".into())])).expect_err("broken file");
        assert!(matches!(err, ConfigError::Syntax { .. }));
        assert_eq!(fs::read_to_string(&p).expect("read"), "[keys\n", "left untouched");
    }

    #[test]
    fn toml_round_trip_of_the_values_python_dumps_writes() {
        // Python test_toml_roundtrip_and_user_write: strings, a list, an int, a bool, nested tables.
        let d = TempDir::new("cfg-roundtrip");
        let p = d.join("c.toml");
        let mut upd = section_table(
            "keys",
            [
                ("tap", "power:turbo".into()),
                ("power_cycle", Value::Array(vec!["eco".into(), "turbo".into()])),
                ("long_ms", 900.into()),
            ],
        );
        upd.insert("flag".into(), true.into());
        let mut b = Table::new();
        b.insert("c".into(), 1.into());
        let mut a = Table::new();
        a.insert("b".into(), Value::Table(b));
        upd.insert("a".into(), Value::Table(a));
        let written = write_user(&p, upd.clone()).expect("write");
        assert_eq!(written, upd);
        assert_eq!(read_table(&p).expect("read").expect("exists"), upd);
        // Quotes and backslashes in a string survive.
        let tricky = section_table("keys", [("tap", Value::String("exec:echo \"a\\b\"".into()))]);
        write_user(&p, tricky).expect("write");
        let c = Config::load(None, Some(&p), &no_env).expect("load");
        assert_eq!(c.keys.tap, "exec:echo \"a\\b\"");
    }

    #[test]
    fn merge_replaces_non_tables_and_merges_tables() {
        let mut base = table("a = 1\n[t]\nx = 1\ny = 2\n[u]\nk = 1\n");
        merge(&mut base, table("a = [1, 2]\n[t]\ny = 3\nz = 4\nu = 5\n[u]\n[v]\nw = 1\n"));
        assert_eq!(base, table("a = [1, 2]\n[t]\nx = 1\ny = 3\nz = 4\nu = 5\n[u]\nk = 1\n[v]\nw = 1\n"));
        // A table over a scalar replaces it; a scalar over a table replaces the table.
        let mut base = table("a = 1\n[b]\nx = 1\n");
        merge(&mut base, table("b = 2\n[a]\nq = 1\n"));
        assert_eq!(base, table("b = 2\n[a]\nq = 1\n"));
    }

    #[test]
    fn user_path_prefers_the_override_then_home() {
        let both = |k: &str| match k {
            "TRUESENSE_CONFIG" => Some("/x/c.toml".to_owned()),
            "HOME" => Some("/home/u".to_owned()),
            _ => None,
        };
        let home = |k: &str| (k == "HOME").then(|| "/home/u".to_owned());
        assert_eq!(user_path_from(&both), Some(PathBuf::from("/x/c.toml")));
        assert_eq!(user_path_from(&home), Some(PathBuf::from("/home/u/.config/truesense/config.toml")));
        assert_eq!(user_path_from(&no_env), None);
    }
}
