//! looks: lighting looks, a named snapshot of the keyboard look and the rear light bar.
//!
//! A look is the keyboard part (an effect with its colour, speed and brightness, or per-key
//! colours) and the rear part (mode, colour, speed, brightness, three zone colours). Looks are
//! saved per user in `~/.config/truesense/looks/NAME.json`. One look can also be chosen for BOOT:
//! the daemon (root) keeps a self-contained copy in `/var/lib/truesense/boot-look.json` and applies
//! it as soon as it starts and again after every resume from suspend, before anyone logs in.
//! The JSON of both files is what the Python and Go editions wrote, and either part may be
//! missing: a missing part means "leave it as it is".
#![forbid(unsafe_code)]

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use ts_foundation::fsx::{self, FsError};
use ts_foundation::names::is_valid as valid_name;
use ts_foundation::userstate::{self, Store};
use ts_platform::hw::{Coded, Hw, HwError, Rear as HwRear, RearMode, RearSettings, Rgb};

use crate::keyboard::{self, Keyboard, KeyboardError};
use crate::themes::ThemeSpec;

/// Where the daemon keeps the boot look unless `TRUESENSE_BOOT_LOOK` says otherwise.
pub(crate) const DEFAULT_BOOT_FILE: &str = "/var/lib/truesense/boot-look.json";

/// The built-in looks, which cannot be saved over or deleted.
pub const BUILTIN_NAMES: [&str; 1] = ["native"];

/// A failure of a look operation.
#[derive(Debug)]
pub enum LooksError {
    /// A look name or look that is not usable; the text says why (the Python edition's text).
    Invalid(String),
    /// A look file could not be read, written or removed.
    File {
        /// What was being done.
        op: &'static str,
        /// The underlying failure.
        source: FsError,
    },
    /// A look file or a look received from a client is not a look.
    Parse {
        /// Where the text came from (the look's name, or "look").
        what: String,
        /// The decoder's complaint.
        source: serde_json::Error,
    },
    /// The keyboard part could not be applied.
    Keyboard(KeyboardError),
    /// The rear part could not be applied.
    Rear(HwError),
}

impl fmt::Display for LooksError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::File { op, source } => write!(f, "{op}: {source}"),
            Self::Parse { what, source } => write!(f, "{what}: {source}"),
            Self::Keyboard(e) => e.fmt(f),
            Self::Rear(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for LooksError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::File { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            Self::Keyboard(e) => Some(e),
            Self::Rear(e) => Some(e),
            Self::Invalid(_) => None,
        }
    }
}

fn invalid(text: impl Into<String>) -> LooksError {
    LooksError::Invalid(text.into())
}

/// A look: the keyboard and the rear light bar. A missing part means "leave it unchanged".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Look {
    /// The keyboard part.
    #[serde(default)]
    pub keyboard: Option<keyboard::State>,
    /// The rear light part.
    #[serde(default)]
    pub rear: Option<Rear>,
    /// A theme the key service draws on the keyboard while this look is the current one.
    ///
    /// Optional and additive: the `keyboard` part is the static look written when the look is
    /// applied (and put back when the theme stops), so a program that does not know themes
    /// applies a themed look as its static look and ignores this member (`Look` does not deny
    /// unknown fields in the Rust edition, and no edition rewrites a look it only applies).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<ThemeSpec>,
}

/// The rear light bar as the hardware layer remembers it (`rear_state` in the user state).
///
/// The numbers are optional because a look written by hand may lack them; applying such a look
/// reports it instead of sending zeros (a brightness of 0 would switch the bar off).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rear {
    /// The effect name: `static`, `breathing`, `neon`, `wave` or `twinkling`.
    #[serde(default)]
    pub mode: String,
    /// The effect colour as `rrggbb`.
    #[serde(default)]
    pub color: String,
    /// The effect speed, 1 to 5.
    #[serde(default)]
    pub speed: Option<i64>,
    /// The brightness, 0 to 100.
    #[serde(default)]
    pub brightness: Option<i64>,
    /// The direction, 0 or 1; absent means 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<i64>,
    /// One colour, or three (left, middle, right) of the static effect; `null` is one colour
    /// for the whole bar.
    #[serde(default)]
    pub zones: Option<Vec<String>>,
}

impl Rear {
    /// A part without anything in it is as good as absent (an empty dict was falsy in Python).
    fn is_empty(&self) -> bool {
        *self == Rear::default()
    }

    /// The firmware setting for this part, read back after writing.
    fn settings(&self) -> Result<RearSettings, LooksError> {
        let (Some(speed), Some(brightness)) = (self.speed, self.brightness) else {
            return Err(Self::incomplete());
        };
        if self.mode.is_empty() || self.color.is_empty() {
            return Err(Self::incomplete());
        }
        let zones = match &self.zones {
            None => None,
            Some(z) => Some(
                z.iter().map(|c| Rgb::parse(c)).collect::<Result<Vec<_>, _>>().map_err(LooksError::Rear)?,
            ),
        };
        Ok(RearSettings {
            mode: RearMode::from_name(&self.mode).map_err(LooksError::Rear)?,
            color: Rgb::parse(&self.color).map_err(LooksError::Rear)?,
            speed,
            brightness,
            direction: self.direction.unwrap_or(1),
            zones,
            verify: true,
        })
    }

    fn incomplete() -> LooksError {
        invalid("the rear part of this look needs mode, color, speed and brightness")
    }
}

impl Look {
    /// Decodes a look file or a look received over the control socket.
    pub fn parse(text: &str) -> Result<Look, LooksError> {
        serde_json::from_str(text).map_err(|source| LooksError::Parse { what: "look".to_owned(), source })
    }

    /// Decodes a look copy that a binding or the boot look keeps, and accepts it only when it
    /// changes something: the one test every copy goes through before it is stored or applied,
    /// in the daemon and in the application layer alike. The error is the text a person sees.
    pub fn parse_copy(value: &serde_json::Value) -> Result<Look, String> {
        let look: Look = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if look.is_empty() {
            return Err("a look needs keyboard and/or rear settings".to_owned());
        }
        Ok(look)
    }

    /// Whether the look changes nothing: no part, or only empty ones.
    pub fn is_empty(&self) -> bool {
        self.keyboard.as_ref().is_none_or(keyboard::State::is_empty)
            && self.rear.as_ref().is_none_or(Rear::is_empty)
            && self.theme.is_none()
    }

    /// The look as one line, as `truesense look show` prints it.
    pub fn describe(&self) -> String {
        let keyboard = match self.keyboard.as_ref().filter(|k| !k.is_empty()) {
            Some(k) => {
                let name =
                    [k.name.as_str(), k.mode.as_str()].into_iter().find(|s| !s.is_empty()).unwrap_or("None");
                let brightness = k.brightness.map_or_else(|| "?".to_owned(), |b| b.to_string());
                let speed = k.speed.filter(|s| *s != 0).map(|s| format!(" speed {s}")).unwrap_or_default();
                format!("keyboard {name} brightness {brightness}{speed}")
            }
            None => "keyboard unchanged".to_owned(),
        };
        let rear = match self.rear.as_ref().filter(|r| !r.is_empty()) {
            Some(r) => format!(
                "rear {} brightness {}",
                if r.mode.is_empty() { "None" } else { &r.mode },
                r.brightness.map_or_else(|| "None".to_owned(), |b| b.to_string())
            ),
            None => "rear unchanged".to_owned(),
        };
        match &self.theme {
            Some(t) => format!("{keyboard}; {rear}; theme {}", t.name),
            None => format!("{keyboard}; {rear}"),
        }
    }
}

/// What PredatorSense leaves on this laptop on Windows (from the captures): keyboard cells
/// `#00aec7` at brightness 12, rear bar Breathing `#3723c7` speed 3 brightness 100.
fn native() -> Look {
    let mut cells = vec!["00aec7".to_owned(); 126];
    cells.extend(["000000".to_owned(), "000000".to_owned()]);
    Look {
        keyboard: Some(keyboard::State {
            mode: keyboard::mode::PERKEY.into(),
            brightness: Some(12),
            cells,
            ..keyboard::State::default()
        }),
        rear: Some(Rear {
            mode: "breathing".into(),
            color: "3723c7".into(),
            speed: Some(3),
            brightness: Some(100),
            direction: Some(1),
            zones: None,
        }),
        theme: None,
    }
}

fn builtin(name: &str) -> Option<Look> {
    BUILTIN_NAMES.contains(&name).then(native)
}

/// What applying a look needs from the keyboard controller.
pub trait KeyboardControl: Send + Sync {
    /// What was last put on the keyboard.
    fn last(&self) -> keyboard::State;
    /// Puts a remembered look back; returns what was done, or `None` if there was nothing to restore.
    fn replay(&self, state: &keyboard::State) -> Result<Option<String>, KeyboardError>;
    /// Backlight off, keeping the look.
    fn off(&self) -> Result<(), KeyboardError>;
}

impl KeyboardControl for Keyboard {
    fn last(&self) -> keyboard::State {
        Keyboard::last(self)
    }

    fn replay(&self, state: &keyboard::State) -> Result<Option<String>, KeyboardError> {
        Keyboard::replay(self, state)
    }

    fn off(&self) -> Result<(), KeyboardError> {
        Keyboard::off(self)
    }
}

/// What applying a look needs from the firmware's rear light.
pub trait RearControl: Send + Sync {
    /// Writes the light bar and reads the result back.
    fn rear_set(&self, settings: &RearSettings) -> Result<HwRear, HwError>;
}

impl RearControl for Hw {
    fn rear_set(&self, settings: &RearSettings) -> Result<HwRear, HwError> {
        Hw::rear_set(self, settings)
    }
}

/// `~/.config/truesense/looks` under `home`. (The Python edition did not honour
/// `XDG_CONFIG_HOME` here, unlike for designs; the files must stay where it put them.)
pub fn default_dir(home: &Path) -> PathBuf {
    home.join(".config/truesense/looks")
}

/// The boot look path: `TRUESENSE_BOOT_LOOK` (empty counts as unset) or `DEFAULT_BOOT_FILE`.
pub fn default_boot_file(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    env("TRUESENSE_BOOT_LOOK").map_or_else(|| PathBuf::from(DEFAULT_BOOT_FILE), PathBuf::from)
}

/// A directory of looks, the boot look, and the devices a look is applied to.
pub struct LookStore {
    dir: PathBuf,
    boot_file: PathBuf,
    state: Arc<Store>,
    keyboard: Arc<dyn KeyboardControl>,
    rear: Arc<dyn RearControl>,
    now: Arc<dyn Fn() -> f64 + Send + Sync>,
}

fn file_error(op: &'static str) -> impl FnOnce(FsError) -> LooksError {
    move |source| LooksError::File { op, source }
}

/// JSON with one space of indentation, as Python's `json.dump(indent=1)` writes it.
fn to_pretty_json(value: &Look) -> Result<Vec<u8>, LooksError> {
    let mut out = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b" ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, formatter);
    value
        .serialize(&mut serializer)
        .map_err(|source| LooksError::Parse { what: "look".to_owned(), source })?;
    Ok(out)
}

impl LookStore {
    /// A store over `dir` and `boot_file`, remembering nothing itself: the rear light is read
    /// from `state` (where the hardware layer keeps `rear_state`), the keyboard and rear light are
    /// written through `keyboard` and `rear`.
    pub fn new(
        dir: impl Into<PathBuf>,
        boot_file: impl Into<PathBuf>,
        state: Arc<Store>,
        keyboard: Arc<dyn KeyboardControl>,
        rear: Arc<dyn RearControl>,
    ) -> Self {
        Self {
            dir: dir.into(),
            boot_file: boot_file.into(),
            state,
            keyboard,
            rear,
            now: Arc::new(crate::perkeyfx::epoch_seconds),
        }
    }

    /// The same store reading the wall clock (seconds since the epoch) from `now`, which stamps
    /// the start of a theme; tests pass a fixed one.
    pub fn with_clock(mut self, now: impl Fn() -> f64 + Send + Sync + 'static) -> Self {
        self.now = Arc::new(now);
        self
    }

    fn path(&self, name: &str) -> Result<PathBuf, LooksError> {
        if !valid_name(name) {
            return Err(invalid("look names use letters, digits, . _ - (max 40)"));
        }
        Ok(self.dir.join(format!("{name}.json")))
    }

    /// The built-in looks first, then yours sorted.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = BUILTIN_NAMES.iter().map(|n| (*n).to_owned()).collect();
        let mut mine: Vec<String> = std::fs::read_dir(&self.dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter_map(|f| f.strip_suffix(".json").map(str::to_owned))
                    .filter(|n| !BUILTIN_NAMES.contains(&n.as_str()))
                    .collect()
            })
            .unwrap_or_default();
        mine.sort();
        names.extend(mine);
        names
    }

    /// What is on the lights now, as remembered by this tool (the hardware cannot be asked which
    /// effect is running).
    pub fn capture(&self) -> Result<Look, LooksError> {
        let keyboard = Some(self.keyboard.last()).filter(|k| !k.is_empty());
        let rear = self
            .state
            .get(userstate::keys::REAR_STATE)
            .filter(|v| v.as_object().is_some_and(|m| !m.is_empty()));
        let rear = rear.and_then(|v| serde_json::from_value::<Rear>(v).ok());
        // A theme that is running is part of what is on the keyboard now.
        let theme = self.state.get_as::<crate::themes::Active>(crate::themes::ACTIVE_KEY).map(|a| a.spec);
        if keyboard.is_none() && rear.is_none() && theme.is_none() {
            return Err(invalid("nothing to save yet: set the keyboard or rear light first"));
        }
        Ok(Look { keyboard, rear, theme })
    }

    /// Stores a look under `name`; `None` saves the current lights ([`LookStore::capture`]).
    /// Returns what was saved.
    pub fn save(&self, name: &str, look: Option<&Look>) -> Result<Look, LooksError> {
        if BUILTIN_NAMES.contains(&name) {
            return Err(invalid(format!(
                "'{name}' is built in and read-only; save your look under another name"
            )));
        }
        let look = match look {
            Some(l) => l.clone(),
            None => self.capture()?,
        };
        if let Some(theme) = &look.theme {
            theme.check_shape().map_err(|e| invalid(e.to_string()))?;
        }
        let path = self.path(name)?;
        fsx::write_atomic_mkdir(&path, &to_pretty_json(&look)?, 0o644).map_err(file_error("save look"))?;
        Ok(look)
    }

    /// Returns a saved or built-in look.
    pub fn load(&self, name: &str) -> Result<Look, LooksError> {
        if let Some(look) = builtin(name) {
            return Ok(look);
        }
        let path = self.path(name)?;
        let bytes = match fsx::read_limited(&path, fsx::MAX_SETTINGS_FILE) {
            Ok(b) => b,
            Err(e) if e.is_not_found() => {
                return Err(invalid(format!("no look '{name}' (truesense look list)")));
            }
            Err(source) => return Err(LooksError::File { op: "read look", source }),
        };
        serde_json::from_slice(&bytes)
            .map_err(|source| LooksError::Parse { what: format!("look '{name}'"), source })
    }

    /// Removes a saved look.
    pub fn delete(&self, name: &str) -> Result<(), LooksError> {
        if BUILTIN_NAMES.contains(&name) {
            return Err(invalid(format!("'{name}' is built in and cannot be deleted")));
        }
        let path = self.path(name)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(invalid(format!("no look '{name}'"))),
            Err(source) => Err(LooksError::File {
                op: "delete look",
                source: FsError::Io { op: "remove", path, source },
            }),
        }
    }

    /// Saves a look under a new name (`NAME-copy` by default, numbered until it is free) and
    /// returns that name.
    pub fn copy(&self, name: &str, new_name: Option<&str>) -> Result<String, LooksError> {
        let base = new_name.filter(|n| !n.is_empty()).map_or_else(|| format!("{name}-copy"), str::to_owned);
        let taken = self.names();
        let mut target = base.clone();
        let mut n = 2;
        while taken.contains(&target) {
            target = format!("{base}-{n}");
            n += 1;
        }
        let look = self.load(name)?;
        self.save(&target, Some(&look))?;
        Ok(target)
    }

    /// Puts a look on the lights and returns what was done, one line per part.
    pub fn apply(&self, look: &Look) -> Result<Vec<String>, LooksError> {
        let mut done = Vec::new();
        if let Some(k) = look.keyboard.as_ref().filter(|k| !k.is_empty()) {
            let mut what = self.keyboard.replay(k).map_err(LooksError::Keyboard)?;
            if what.is_none() && k.mode == keyboard::mode::OFF {
                self.keyboard.off().map_err(LooksError::Keyboard)?;
                what = Some("off".to_owned());
            }
            done.push(format!("keyboard: {}", what.unwrap_or_else(|| "nothing to restore".to_owned())));
        }
        if let Some(r) = look.rear.as_ref().filter(|r| !r.is_empty()) {
            self.rear.rear_set(&r.settings()?).map_err(LooksError::Rear)?;
            done.push(format!("rear: {}", r.mode));
        }
        // The theme goes last: it is drawn over the static keyboard look just written, and the key
        // service takes that look as the one to put back when the theme stops. A look with a
        // keyboard part and no theme ends the theme that was running; a rear-only look leaves it.
        let keyboard_part = look.keyboard.as_ref().is_some_and(|k| !k.is_empty());
        if look.theme.is_some() || keyboard_part {
            if let Some(line) = self.set_theme(look.theme.as_ref())? {
                done.push(line);
            }
        }
        Ok(done)
    }

    /// Records the theme to draw (`Some`) or ends the running one (`None`); answers the line to
    /// report, if anything was done. A theme the store does not hold, or one that cannot run, is
    /// the key service's to refuse (it records why where `theme show` says it); the shape is
    /// checked here.
    fn set_theme(&self, spec: Option<&ThemeSpec>) -> Result<Option<String>, LooksError> {
        match spec {
            Some(spec) => {
                spec.check_shape().map_err(|e| invalid(e.to_string()))?;
                let active = crate::themes::Active { spec: spec.clone(), started: (self.now)() };
                self.state.set_as(crate::themes::ACTIVE_KEY, &active).map_err(file_error("start theme"))?;
                Ok(Some(format!("theme: {}", spec.name)))
            }
            None => {
                let running = self.state.get_as::<crate::themes::Active>(crate::themes::ACTIVE_KEY).is_some();
                if !running {
                    return Ok(None);
                }
                self.state
                    .set(crate::themes::ACTIVE_KEY, serde_json::Value::Null)
                    .map_err(file_error("stop theme"))?;
                Ok(Some("theme: stopped".to_owned()))
            }
        }
    }

    // ---- the boot look (root; used by the daemon) ----------------------------------------------

    /// The boot look, or `None` when none is set or the file is unreadable or not a look.
    pub fn read_boot(&self) -> Option<Look> {
        let bytes = fsx::read_limited(&self.boot_file, fsx::MAX_SETTINGS_FILE).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Stores the boot look, or removes it with `None`. The file is self-contained so that
    /// nothing in a user's home is needed before anyone logs in.
    pub fn write_boot(&self, look: Option<&Look>) -> Result<(), LooksError> {
        match look {
            None => match std::fs::remove_file(&self.boot_file) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(source) => Err(LooksError::File {
                    op: "remove boot look",
                    source: FsError::Io { op: "remove", path: self.boot_file.clone(), source },
                }),
            },
            Some(look) => {
                let bytes = serde_json::to_vec(look)
                    .map_err(|source| LooksError::Parse { what: "boot look".to_owned(), source })?;
                fsx::write_atomic_mkdir(&self.boot_file, &bytes, 0o644).map_err(file_error("write boot look"))
            }
        }
    }
}

#[cfg(test)]
mod tests;
