//! Whole-setup profiles: a saved bundle of settings applied in one go (command line, PREDATOR
//! key, window). The Python edition's `profiles.py`.
//!
//! A bundle is a TOML file in `~/.config/truesense/profiles/` (yours) or in the shipped
//! directories (yours win). Every section is optional and applying a bundle changes only what
//! it lists:
//!
//! ```toml
//! description = "..."
//! [power]    mode = "turbo"   auto = "off" | "native" | "smart"
//! [fan]      mode = "smart" | "auto" | "max" | "manual"   (manual also needs cpu, gpu: 20..100)   curve = "truesense"
//! [keyboard] on = true  effect = "wave" color = "00aec7" speed = 5 brightness = 30   or   profile = "<lighting profile>"   timeout = true
//! [rear]     on = true  effect = "breathing" color = "ff0000" speed = 3 brightness = 100
//! [battery]  limit = true   usb_charge = 30      (0 10 20 30)
//! [display]  overdrive = true
//! [boot]     sound = false
//! [audio]    output = "speakers" | "headphones"  voice_headset = "conference"  voice_array = "off"  mic_muted = false  dts_mode = "movie"
//! ```
//!
//! Applying a bundle never changes the GPU mode (that needs a reboot) and never touches
//! undervolt.
//!
//! A bundle is kept as a generic TOML table, not a struct, so a file written by hand or by a
//! newer edition round-trips without losing keys, and a wrong type in one setting fails that
//! setting alone (it is reported) instead of the whole bundle.
//!
//! Applying walks the sections in the fixed order of [`BundleStore::apply`], never a map: the
//! order is the order of the Python edition (power auto before the mode, the fan curve before
//! the fan mode, the keyboard before the rear light, the audio last), and the same on every
//! run.
#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use serde_json::json;
use ts_foundation::config::{Table, Value};
use ts_foundation::fsx::{self, FsError, MAX_SETTINGS_FILE};

use super::store::NameRule;
use crate::actions::{
    ActionRunner, ActionState, AudioFacade, DaemonFacade, FanCommand, HostError, HostResult, LightingFacade,
    PowerFacade, ProfilesFacade, REAR_EFFECTS, RearWrite, truthy as json_truthy,
};

/// The user-state key that remembers the last bundle applied, so `profile-next` continues
/// from it.
const STATE_KEY: &str = "profile";

/// The rule for bundle names: the oldest kind, letters, digits, `-` and `_`.
pub const NAME_RULE: NameRule = NameRule::Strict;

/// A decoded bundle: sections of settings.
pub type Bundle = Table;

/// Where a bundle comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleSource {
    /// The user's own file.
    Yours,
    /// Shipped with the program.
    Shipped,
}

impl BundleSource {
    /// `yours` or `shipped`, as the command line and the window show it.
    pub fn as_str(self) -> &'static str {
        match self {
            BundleSource::Yours => "yours",
            BundleSource::Shipped => "shipped",
        }
    }
}

/// Why a bundle operation failed. The messages are the Python edition's.
#[derive(Debug)]
pub enum BundleError {
    /// The name is not letters, digits, `-` and `_`.
    BadName,
    /// There is no such bundle; the text includes the hint to list them where the Python
    /// edition had one.
    NotFound(String),
    /// A shipped bundle cannot be deleted.
    Shipped(String),
    /// The file is not valid TOML; names the file.
    Parse {
        /// The file.
        path: PathBuf,
        /// What the parser said.
        message: String,
    },
    /// Nothing is saved yet (`profile-next` has nothing to step to).
    NoProfiles,
    /// The bundle cannot be written as TOML.
    Encode(String),
    /// The disk refused.
    Io {
        /// What was being done.
        op: String,
        /// The file system error.
        source: FsError,
    },
}

impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BundleError::BadName => f.write_str(NAME_RULE.message()),
            BundleError::NotFound(m) | BundleError::Shipped(m) | BundleError::Encode(m) => f.write_str(m),
            BundleError::Parse { path, message } => write!(f, "{}: {message}", path.display()),
            BundleError::NoProfiles => f.write_str("no profiles saved"),
            BundleError::Io { op, source } => write!(f, "{op}: {source}"),
        }
    }
}

impl std::error::Error for BundleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BundleError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// The shipped bundle directories: `TRUESENSE_PROFILES_DIR` replaces the two system ones (the
/// tests and a source tree use it).
pub fn default_sys_dirs(env: &dyn Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    match env("TRUESENSE_PROFILES_DIR") {
        Some(d) => vec![PathBuf::from(d)],
        None => vec![
            PathBuf::from("/usr/local/share/truesense/profiles"),
            PathBuf::from("/usr/share/truesense/profiles"),
        ],
    }
}

/// The bundle files on disk: the user's directory and the shipped ones.
#[derive(Debug, Clone)]
pub struct BundleFiles {
    user_dir: PathBuf,
    sys_dirs: Vec<PathBuf>,
}

impl BundleFiles {
    /// Bundles in `user_dir`, then `sys_dirs` in order (the first directory holding a name wins).
    pub fn new(user_dir: impl Into<PathBuf>, sys_dirs: Vec<PathBuf>) -> BundleFiles {
        BundleFiles { user_dir: user_dir.into(), sys_dirs }
    }

    /// `<home>/.config/truesense/profiles`.
    pub fn default_user_dir(home: &Path) -> PathBuf {
        home.join(".config").join("truesense").join("profiles")
    }

    /// The directory of the user's bundles.
    pub fn user_dir(&self) -> &Path {
        &self.user_dir
    }

    fn dirs(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.user_dir.as_path()).chain(self.sys_dirs.iter().map(PathBuf::as_path))
    }

    /// Where the bundle lives (the user's directory first), `None` when there is none.
    fn path(&self, name: &str) -> Result<Option<PathBuf>, BundleError> {
        if !NAME_RULE.accepts(name) {
            return Err(BundleError::BadName);
        }
        Ok(self.dirs().map(|d| d.join(format!("{name}.toml"))).find(|p| p.is_file()))
    }

    /// Every bundle, shipped and yours, sorted, each once. Files whose name could never be
    /// loaded are not listed.
    pub fn names(&self) -> Vec<String> {
        let mut seen = BTreeSet::new();
        for dir in self.dirs() {
            let Ok(entries) = std::fs::read_dir(dir) else { continue };
            for e in entries.filter_map(Result::ok) {
                if !e.path().is_file() {
                    continue;
                }
                if let Some(n) = e.file_name().to_str().and_then(|f| f.strip_suffix(".toml"))
                    && NAME_RULE.accepts(n)
                {
                    seen.insert(n.to_owned());
                }
            }
        }
        seen.into_iter().collect()
    }

    /// Reads a bundle.
    pub fn load(&self, name: &str) -> Result<Bundle, BundleError> {
        let Some(path) = self.path(name)? else {
            return Err(BundleError::NotFound(format!("no profile '{name}' (truesense profile list)")));
        };
        let bytes = fsx::read_limited(&path, MAX_SETTINGS_FILE)
            .map_err(|source| BundleError::Io { op: format!("read profile '{name}'"), source })?;
        let text = String::from_utf8(bytes)
            .map_err(|_| BundleError::Parse { path: path.clone(), message: "invalid UTF-8".into() })?;
        parse(&text, &path.display().to_string())
    }

    /// Where a bundle comes from, `None` when there is none (or the name is refused).
    pub fn source(&self, name: &str) -> Option<BundleSource> {
        let path = self.path(name).ok()??;
        Some(if path.parent() == Some(self.user_dir.as_path()) {
            BundleSource::Yours
        } else {
            BundleSource::Shipped
        })
    }

    /// Writes your bundle (a shipped one of the same name is overridden, not changed) and
    /// returns the file's path.
    pub fn save(&self, name: &str, data: &Bundle) -> Result<PathBuf, BundleError> {
        if !NAME_RULE.accepts(name) {
            return Err(BundleError::BadName);
        }
        let text =
            toml_text(data).map_err(|m| BundleError::Encode(format!("encode profile '{name}': {m}")))?;
        let path = self.user_dir.join(format!("{name}.toml"));
        fsx::write_atomic_mkdir(&path, text.as_bytes(), 0o644)
            .map_err(|source| BundleError::Io { op: format!("save profile '{name}'"), source })?;
        Ok(path)
    }

    /// Removes your bundle; a shipped one cannot be deleted.
    pub fn delete(&self, name: &str) -> Result<(), BundleError> {
        let Some(path) = self.path(name)? else {
            return Err(BundleError::NotFound(format!("no profile '{name}'")));
        };
        if path.parent() != Some(self.user_dir.as_path()) {
            return Err(BundleError::Shipped(format!(
                "'{name}' is a shipped profile; save your own copy with the same name to override it"
            )));
        }
        std::fs::remove_file(&path).map_err(|e| BundleError::Io {
            op: format!("delete profile '{name}'"),
            source: FsError::Io { op: "remove", path, source: e },
        })
    }
}

/// Decodes TOML text; `origin` names the file in the error.
pub fn parse(text: &str, origin: &str) -> Result<Bundle, BundleError> {
    Table::from_str(text)
        .map_err(|e| BundleError::Parse { path: PathBuf::from(origin), message: e.to_string() })
}

/// The file text. Written with `write!` rather than `to_string` because the encoder can refuse
/// a table (and `to_string` would panic on that).
fn toml_text(data: &Bundle) -> Result<String, String> {
    use fmt::Write;
    let mut out = String::new();
    write!(out, "{data}").map_err(|e| e.to_string())?;
    Ok(out)
}

// ---- describe ------------------------------------------------------------------------------

/// What a bundle will change, in plain words, one line per area.
pub fn describe(d: &Bundle) -> Vec<String> {
    let mut out = Vec::new();
    let (p, f, k, r, b, a) = (
        section(d, "power"),
        section(d, "fan"),
        section(d, "keyboard"),
        section(d, "rear"),
        section(d, "battery"),
        section(d, "audio"),
    );
    if p.contains_key("mode") || p.contains_key("auto") {
        let mut bits = Vec::new();
        if let Some(m) = p.get("mode") {
            bits.push(format!("{} mode (held for a while)", capitalize(&text(m))));
        }
        if let Some(a) = p.get("auto") {
            bits.push(format!("mode control {}", text(a)));
        }
        out.push(format!("Power: {}", bits.join(", ")));
    }
    if f.contains_key("mode") || f.contains_key("curve") {
        let mut bits = Vec::new();
        if f.contains_key("mode") {
            bits.push(fan_text(f));
        }
        if let Some(c) = f.get("curve") {
            bits.push(format!("curve {}", text(c)));
        }
        out.push(format!(
            "Fans: {}",
            bits.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ")
        ));
    }
    if !k.is_empty() {
        let mut bits = Vec::new();
        if let Some(e) = k.get("effect") {
            bits.push(format!("{} effect", text(e)));
        }
        if let Some(p) = k.get("profile") {
            bits.push(format!("key set {}", text(p)));
        }
        if let Some(on) = k.get("on") {
            bits.push(format!("backlight {}", on_off(on)));
        }
        if let Some(t) = k.get("timeout") {
            bits.push(format!("light off after 30 s {}", on_off(t)));
        }
        out.push(format!("Keyboard: {}", bits.join(", ")));
    }
    if !r.is_empty() {
        let line = if r.get("effect").is_some_and(truthy) {
            let colour = r.get("color").map(text).unwrap_or_default();
            format!("{} {colour}", r.get("effect").map(text).unwrap_or_default()).trim().to_owned()
        } else if r.get("on").is_some_and(truthy) {
            "on".to_owned()
        } else {
            "off".to_owned()
        };
        out.push(format!("Rear light: {line}"));
    }
    if !b.is_empty() {
        let mut bits = Vec::new();
        if let Some(l) = b.get("limit") {
            bits.push(format!("80 % limit {}", on_off(l)));
        }
        if let Some(u) = b.get("usb_charge") {
            bits.push(format!("USB charging while off {}", text(u)));
        }
        out.push(format!("Battery: {}", bits.join(", ")));
    }
    let display = section(d, "display");
    if !display.is_empty() {
        out.push(format!(
            "Screen: overdrive {}",
            display.get("overdrive").map_or("off", |v| if truthy(v) { "on" } else { "off" })
        ));
    }
    let boot = section(d, "boot");
    if !boot.is_empty() {
        out.push(format!(
            "Boot sound {}",
            boot.get("sound").map_or("off", |v| if truthy(v) { "on" } else { "off" })
        ));
    }
    if !a.is_empty() {
        let mut bits = Vec::new();
        if let Some(o) = a.get("output") {
            bits.push(text(o));
        }
        if let Some(v) = a.get("voice_headset") {
            bits.push(format!("headset noise reduction {}", text(v)));
        }
        if let Some(v) = a.get("voice_array") {
            bits.push(format!("built-in noise reduction {}", text(v)));
        }
        out.push(format!(
            "Audio: {}",
            bits.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ")
        ));
    }
    if out.is_empty() { vec!["(nothing)".to_owned()] } else { out }
}

fn fan_text(f: &Table) -> String {
    let mode = f.get("mode").map(text).unwrap_or_default();
    match mode.as_str() {
        "smart" => "smart curve".to_owned(),
        "auto" => "firmware Auto".to_owned(),
        "max" => "maximum".to_owned(),
        "manual" => format!(
            "manual {} % / {} %",
            f.get("cpu").map_or_else(|| "?".to_owned(), text),
            f.get("gpu").map_or_else(|| "?".to_owned(), text)
        ),
        _ => mode,
    }
}

/// `str.capitalize()`: first letter upper, the rest lower.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars.flat_map(char::to_lowercase)).collect(),
        None => String::new(),
    }
}

fn on_off(v: &Value) -> &'static str {
    if truthy(v) { "on" } else { "off" }
}

/// A section of the bundle, or an empty table when it is absent or not a table.
fn section<'a>(d: &'a Bundle, name: &str) -> &'a Table {
    static EMPTY: std::sync::OnceLock<Table> = std::sync::OnceLock::new();
    match d.get(name) {
        Some(Value::Table(t)) => t,
        _ => EMPTY.get_or_init(Table::new),
    }
}

/// A TOML value as text the way a Python f-string showed it (`True`, `1.5`, plain strings), so
/// the lines match the Python edition's.
fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Boolean(true) => "True".into(),
        Value::Boolean(false) => "False".into(),
        Value::Integer(n) => n.to_string(),
        Value::Float(x) => format!("{x:?}"),
        Value::Datetime(d) => d.to_string(),
        Value::Array(_) | Value::Table(_) => v.to_string(),
    }
}

/// Python's idea of true, which the 0.3.x profiles were written against.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Boolean(b) => *b,
        Value::Integer(n) => *n != 0,
        Value::Float(x) => *x != 0.0,
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Table(t) => !t.is_empty(),
        Value::Datetime(_) => true,
    }
}

/// A TOML value as a whole number the way Python's `int()` read it: integers, floats
/// (truncated), booleans and numeric text.
fn to_int(v: &Value) -> Result<i64, String> {
    match v {
        Value::Integer(n) => Ok(*n),
        Value::Float(x) if x.is_finite() && x.abs() <= 9_007_199_254_740_992.0 => Ok(*x as i64),
        Value::Float(x) => Err(format!("{x} is not a whole number")),
        Value::Boolean(b) => Ok(i64::from(*b)),
        Value::String(s) => s.trim().parse::<i64>().map_err(|_| format!("{s:?} is not a number")),
        other => Err(format!("{other} is not a number")),
    }
}

fn int_of(t: &Table, key: &str, default: i64) -> Result<i64, String> {
    t.get(key).map_or(Ok(default), to_int)
}

fn str_of(t: &Table, key: &str, default: &str) -> Result<String, String> {
    match t.get(key) {
        None => Ok(default.to_owned()),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(_) => Err(format!("{key} must be text")),
    }
}

// ---- apply and capture ---------------------------------------------------------------------

/// What applying and capturing a bundle reach. The assembly layer passes the same adapters it
/// gave the action runner, plus that runner itself.
#[derive(Clone)]
pub struct BundleHost {
    /// Runs the settings that are also key actions (power held like a tile pick, fans,
    /// keyboard and rear on/off, lighting profiles), so a bundle and a key press cannot drift
    /// apart.
    pub actions: Arc<dyn ActionRunner>,
    /// Firmware features.
    pub power: Arc<dyn PowerFacade>,
    /// The daemon (settings, fan curve).
    pub daemon: Arc<dyn DaemonFacade>,
    /// Keyboard and light bar.
    pub lighting: Arc<dyn LightingFacade>,
    /// Sound.
    pub audio: Arc<dyn AudioFacade>,
    /// The per-user memory (which bundle was applied last).
    pub state: Arc<dyn ActionState>,
}

/// The bundle files plus everything applying and capturing reach.
pub struct BundleStore {
    files: BundleFiles,
    host: BundleHost,
}

impl BundleStore {
    /// A store over `files` acting through `host`.
    pub fn new(files: BundleFiles, host: BundleHost) -> BundleStore {
        BundleStore { files, host }
    }

    /// The files (listing, loading, saving, deleting need no host).
    pub fn files(&self) -> &BundleFiles {
        &self.files
    }

    /// The bundle after the last one applied (the first when none was, or when the last has
    /// since been deleted).
    pub fn next_name(&self) -> Result<String, BundleError> {
        let names = self.files.names();
        let current = self.host.state.get(STATE_KEY);
        let at = current.as_ref().and_then(|v| v.as_str()).and_then(|c| names.iter().position(|n| n == c));
        let next = at.map_or(0, |i| (i + 1) % names.len().max(1));
        names.get(next).cloned().ok_or(BundleError::NoProfiles)
    }

    /// Records the current setup as a bundle: what can be read back. Keyboard colours cannot
    /// be, so only the keyboard's "off after 30 s" setting is captured. Daemon-held settings
    /// (smart fan, fan curve, mode control) are recorded when the daemon runs and left out when
    /// it does not.
    pub fn capture(&self, description: &str) -> HostResult<Bundle> {
        let h = &self.host;
        let mut d = Bundle::new();
        d.insert(
            "description".into(),
            Value::String(if description.is_empty() { "captured".into() } else { description.into() }),
        );
        let mut power = Table::new();
        power.insert("mode".into(), Value::String(h.power.mode()?));
        let fan = h.power.fan_state()?;
        let settings_smart = h.daemon.setting("smart_fan");
        let smart = settings_smart.as_ref().is_ok_and(|v| v.as_ref().is_some_and(json_truthy));
        let mut fan_section = Table::new();
        fan_section
            .insert("mode".into(), Value::String(if smart { "smart".into() } else { fan.mode.clone() }));
        if let Ok(name) = h.daemon.fan_curve_name() {
            fan_section.insert("curve".into(), Value::String(name));
        }
        if settings_smart.is_ok() {
            let auto = match h.daemon.setting("auto_mode") {
                Ok(Some(serde_json::Value::String(s))) => s,
                _ => "off".to_owned(),
            };
            power.insert("auto".into(), Value::String(auto));
        }
        if fan.mode == "manual" && !smart {
            fan_section.insert("cpu".into(), Value::Integer(i64::from(fan.cpu)));
            fan_section.insert("gpu".into(), Value::Integer(i64::from(fan.gpu)));
        }
        d.insert("power".into(), Value::Table(power));
        d.insert("fan".into(), Value::Table(fan_section));

        let rear = h.lighting.rear()?;
        let mut rear_section = Table::new();
        rear_section.insert("on".into(), Value::Boolean(rear.brightness > 0));
        if rear.brightness > 0 && REAR_EFFECTS.contains(&rear.mode.as_str()) {
            rear_section.insert("effect".into(), Value::String(rear.mode));
            rear_section.insert("color".into(), Value::String(rear.color));
            rear_section.insert("speed".into(), Value::Integer(rear.speed));
            rear_section.insert("brightness".into(), Value::Integer(rear.brightness));
        }
        d.insert("rear".into(), Value::Table(rear_section));

        let mut battery = Table::new();
        battery.insert("limit".into(), Value::Boolean(h.power.battery_limit()?));
        battery.insert("usb_charge".into(), Value::Integer(h.power.usb_charge()?));
        d.insert("battery".into(), Value::Table(battery));
        let mut display = Table::new();
        display.insert("overdrive".into(), Value::Boolean(h.power.overdrive()?.unwrap_or(false)));
        d.insert("display".into(), Value::Table(display));
        let mut boot = Table::new();
        boot.insert("sound".into(), Value::Boolean(h.power.boot_sound()?));
        d.insert("boot".into(), Value::Table(boot));
        let mut keyboard = Table::new();
        keyboard.insert("timeout".into(), Value::Boolean(h.power.keyboard_timeout()?));
        d.insert("keyboard".into(), Value::Table(keyboard));
        if let (Ok(output), Ok((headset, array))) = (h.audio.active_output(), h.audio.voice_modes()) {
            let mut audio = Table::new();
            audio.insert("output".into(), Value::String(output));
            audio.insert("voice_headset".into(), Value::String(headset));
            audio.insert("voice_array".into(), Value::String(array));
            d.insert("audio".into(), Value::Table(audio));
        }
        Ok(d)
    }

    /// Applies what the bundle LISTS and nothing else, and returns the summary line, e.g.
    /// `Profile 'gaming' applied (4 settings)`. A setting that fails is reported in that line
    /// and does not stop the others: a half-applied bundle is better than a laptop left in an
    /// unknown state by an early exit.
    pub fn apply(&self, name: &str) -> Result<String, BundleError> {
        let d = self.files.load(name)?;
        let mut steps = Steps::default();
        let h = &self.host;
        let run = |spec: String| -> Result<(), String> {
            h.actions.run(&spec).map(|_| ()).map_err(|e| e.to_string())
        };

        let p = section(&d, "power");
        if let Some(auto) = p.get("auto") {
            steps.run("power auto", || {
                let value = serde_json::to_value(auto).map_err(|e| e.to_string())?;
                h.daemon.set_setting("auto_mode", value).map_err(|e| e.to_string())
            });
        }
        if let Some(mode) = p.get("mode") {
            steps.run("power", || run(format!("power:{}", text(mode))));
        }
        let f = section(&d, "fan");
        if let Some(curve) = f.get("curve") {
            steps.run("fan curve", || h.daemon.use_fan_curve(&text(curve)).map_err(|e| e.to_string()));
        }
        if f.get("mode").is_some_and(|m| m.as_str() == Some("manual")) {
            steps.run("fan", || self.fan_manual(f));
        } else if let Some(mode) = f.get("mode") {
            steps.run("fan", || run(format!("fan:{}", text(mode))));
        }
        let k = section(&d, "keyboard");
        if let Some(profile) = k.get("profile") {
            steps.run("keyboard profile", || run(format!("keyboard-profile:{}", text(profile))));
        } else if let Some(effect) = k.get("effect") {
            steps.run("keyboard effect", || self.keyboard_effect(&text(effect), k));
        } else if let Some(on) = k.get("on") {
            steps.run("keyboard", || run(if truthy(on) { "keyboard:on" } else { "keyboard:off" }.to_owned()));
        }
        if let Some(t) = k.get("timeout") {
            steps.run("keyboard timeout", || {
                h.power.set_keyboard_timeout(truthy(t)).map(|_| ()).map_err(|e| e.to_string())
            });
        }
        let r = section(&d, "rear");
        if let Some(effect) = r.get("effect") {
            steps.run("rear light", || self.rear(&text(effect), r));
        } else if let Some(on) = r.get("on") {
            steps.run("rear light", || run(if truthy(on) { "rear:on" } else { "rear:off" }.to_owned()));
        }
        let b = section(&d, "battery");
        if let Some(limit) = b.get("limit") {
            steps.run("battery limit", || {
                h.power.set_battery_limit(truthy(limit)).map(|_| ()).map_err(|e| e.to_string())
            });
        }
        if let Some(usb) = b.get("usb_charge") {
            steps.run("usb charge", || {
                let level = to_int(usb)?;
                h.power.set_usb_charge(level).map(|_| ()).map_err(|e| e.to_string())
            });
        }
        if let Some(od) = section(&d, "display").get("overdrive") {
            steps.run("overdrive", || {
                h.power.set_overdrive(truthy(od)).map(|_| ()).map_err(|e| e.to_string())
            });
        }
        if let Some(sound) = section(&d, "boot").get("sound") {
            steps.run("boot sound", || {
                h.power.set_boot_sound(truthy(sound)).map(|_| ()).map_err(|e| e.to_string())
            });
        }
        let a = section(&d, "audio");
        if let Some(output) = a.get("output") {
            steps.run("audio output", || h.audio.set_output(&text(output)).map_err(|e| e.to_string()));
        }
        if let Some(mode) = a.get("voice_headset") {
            steps.run("voice (headset)", || {
                h.audio.voice_set("headset", &text(mode)).map_err(|e| e.to_string())
            });
        }
        if let Some(mode) = a.get("voice_array") {
            steps.run("voice (array)", || h.audio.voice_set("array", &text(mode)).map_err(|e| e.to_string()));
        }
        if let Some(muted) = a.get("mic_muted") {
            steps.run("microphone", || h.audio.set_mic_mute(truthy(muted)).map_err(|e| e.to_string()));
        }
        if let Some(mode) = a.get("dts_mode") {
            steps.run("DTS mode", || h.audio.dts_mode(&text(mode)).map_err(|e| e.to_string()));
        }
        // A convenience, never a requirement.
        let _ = h.state.set(STATE_KEY, json!(name));
        Ok(steps.summary(name))
    }

    /// A manual fan setting. Both duties are required and must be 20..=100: a bundle cannot
    /// confirm a stop, so it can never ask for one.
    fn fan_manual(&self, f: &Table) -> Result<(), String> {
        let (Some(cpu), Some(gpu)) = (f.get("cpu"), f.get("gpu")) else {
            return Err("manual fans need cpu and gpu".into());
        };
        let duty = |v: &Value| -> Result<u8, String> {
            let n = to_int(v)?;
            u8::try_from(n)
                .ok()
                .filter(|d| (20..=100).contains(d))
                .ok_or_else(|| format!("fan duty {n} is outside 20..100 (a profile cannot stop a fan)"))
        };
        let (cpu, gpu) = (duty(cpu)?, duty(gpu)?);
        self.host.power.set_fan(FanCommand::Manual { cpu, gpu }).map_err(|e| e.to_string())
    }

    fn keyboard_effect(&self, effect: &str, k: &Table) -> Result<(), String> {
        let color = str_of(k, "color", "00aec7")?;
        let speed = int_of(k, "speed", 5)?;
        let brightness = if k.contains_key("brightness") {
            int_of(k, "brightness", 0)?
        } else {
            match self.host.lighting.keyboard_brightness().map_err(|e| e.to_string())? {
                0 => 30,
                b => b,
            }
        };
        self.host.lighting.keyboard_effect(effect, &color, speed, brightness).map_err(|e| e.to_string())
    }

    fn rear(&self, effect: &str, r: &Table) -> Result<(), String> {
        let write = RearWrite {
            mode: effect.to_owned(),
            color: str_of(r, "color", "ffffff")?,
            speed: int_of(r, "speed", 3)?,
            brightness: int_of(r, "brightness", 100)?,
            direction: int_of(r, "direction", 1)?,
            zones: None,
        };
        self.host.lighting.set_rear(&write).map_err(|e| e.to_string())
    }
}

impl ProfilesFacade for BundleStore {
    fn apply(&self, name: &str) -> HostResult<String> {
        BundleStore::apply(self, name).map_err(|e| HostError::new(e.to_string()))
    }

    fn next_name(&self) -> HostResult<String> {
        BundleStore::next_name(self).map_err(|e| HostError::new(e.to_string()))
    }
}

/// Counts settings applied and collects the failures, in the order they happened.
#[derive(Default)]
struct Steps {
    done: usize,
    failed: Vec<String>,
}

impl Steps {
    fn run(&mut self, label: &str, step: impl FnOnce() -> Result<(), String>) {
        match step() {
            Ok(()) => self.done += 1,
            Err(e) => self.failed.push(format!("{label}: {e}")),
        }
    }

    fn summary(&self, name: &str) -> String {
        let mut msg = format!("Profile '{name}' applied ({} settings", self.done);
        if !self.failed.is_empty() {
            msg.push_str(&format!(", {} FAILED: {}", self.failed.len(), self.failed.join("; ")));
        }
        msg.push(')');
        msg
    }
}
