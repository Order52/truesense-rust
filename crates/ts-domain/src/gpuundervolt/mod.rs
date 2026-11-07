//! gpuundervolt: the NVIDIA GPU undervolt through NVML (the interface `nvidia-smi` uses): a
//! clock-curve offset plus an optional cap on the maximum clock, with a risk rating and the store
//! of profiles.
//!
//! Linux drivers do not expose GPU voltage directly. Shifting the voltage/frequency curve UP by an
//! offset (so each clock needs less voltage) and capping the maximum clock is how an undervolt is
//! done here: the GPU reaches its cap at a lower voltage. It needs root (the TrueSense service).
//! Neither setting survives a reboot, so a crash is always recovered by rebooting. The values of
//! the built-in profiles are conservative starting points for the RTX 4060 Laptop, NOT validated
//! on every unit.
//!
//! # Safety
//!
//! * [`GpuUndervolt::apply`] refuses an offset outside -300..=250 MHz or a cap that is not 0 or
//!   600..=3300 MHz before it talks to the GPU, and also an offset outside the range the driver
//!   reports;
//! * the offset is read back, and any failure after the first write puts the GPU back to stock;
//! * NVML is reached only through the C helper, run with a 10 s timeout (see [`helper`]).
//!
//! The risk bands ([`risk`]) and the crash guard (flag files) belong to the decision layer, which
//! needs only [`GpuUndervolt::apply`], [`GpuUndervolt::reset`] and [`GpuUndervolt::read`].
#![forbid(unsafe_code)]

pub mod helper;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use serde::Serialize;
use serde_json::Value;
use ts_foundation::fsx;

pub use crate::power::{Level, Rating};
pub use crate::undervolt::ProfileError;
use crate::undervolt::{Kind, NamedStore};
pub use helper::{HELPER_NAME, HELPER_PATH, HelperNvml, Nvml, NvmlError};

/// The lowest curve offset a profile may hold, in MHz.
pub const OFFSET_MIN_MHZ: i32 = -300;
/// The highest curve offset a profile may hold, in MHz. The driver reports its own limit too
/// (about +-1000), which [`GpuUndervolt::apply`] also enforces.
pub const OFFSET_MAX_MHZ: i32 = 250;
/// The lowest clock cap, in MHz (a cap is 0, meaning none, or [`CAP_MIN_MHZ`]..=[`CAP_MAX_MHZ`]).
pub const CAP_MIN_MHZ: i32 = 600;
/// The highest clock cap, in MHz.
pub const CAP_MAX_MHZ: i32 = 3300;
/// The lower end of the locked clock range: the GPU's idle clock, so that only the top is capped.
const MIN_LOCKED_CLOCK_MHZ: u32 = 210;

/// Up to here an offset is usually fine with a clock cap.
pub const SAFE_OFFSET_MHZ: i32 = 150;
/// Past here games and the driver can crash.
pub const DANGER_OFFSET_MHZ: i32 = 200;
/// A cap below this costs a lot of speed.
const LOW_CAP_MHZ: i32 = 1500;

/// Why a GPU operation failed.
#[derive(Debug)]
pub enum GpuError {
    /// A value was refused before the GPU was touched.
    Invalid(String),
    /// The driver does not allow this offset.
    DriverRange {
        /// The lowest offset the driver allows.
        lo: i32,
        /// The highest offset the driver allows.
        hi: i32,
    },
    /// The GPU did not keep the offset.
    OffsetNotKept {
        /// What the GPU reports.
        got: i32,
        /// What was written.
        wanted: i32,
    },
    /// NVML (the helper) failed in the middle of an operation.
    Nvml {
        /// What was being done, for the message; `None` when the failure is about reaching the
        /// GPU at all (no helper, no driver).
        context: Option<&'static str>,
        /// The failure.
        source: NvmlError,
    },
    /// Several steps failed (a reset tries both of its steps).
    Several(Vec<GpuError>),
}

impl GpuError {
    fn nvml(context: &'static str, source: NvmlError) -> Self {
        Self::Nvml { context: Some(context), source }
    }

    fn reach(source: NvmlError) -> Self {
        Self::Nvml { context: None, source }
    }
}

impl fmt::Display for GpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::DriverRange { lo, hi } => write!(f, "the driver allows offsets {lo}..{hi} MHz"),
            Self::OffsetNotKept { got, wanted } => {
                write!(f, "the GPU kept offset {got} MHz instead of {wanted} (driver limit?)")
            }
            Self::Nvml { context: Some(context), source } => write!(f, "{context}: {source}"),
            Self::Nvml { context: None, source } => source.fmt(f),
            Self::Several(errors) => {
                for (i, e) in errors.iter().enumerate() {
                    if i > 0 {
                        f.write_str("; ")?;
                    }
                    e.fmt(f)?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for GpuError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Nvml { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// The current curve offset and the range the driver allows. Field names are the Python dict keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct State {
    /// The curve offset in MHz.
    pub offset_mhz: i32,
    /// The offsets the driver allows (min, max); `[0, 0]` when it cannot say, which turns the
    /// range check off.
    pub range_mhz: [i32; 2],
}

/// The verified result of [`GpuUndervolt::apply`] and [`GpuUndervolt::reset`]. Field names are the
/// Python dict keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Applied {
    /// The curve offset the GPU reports.
    pub offset_mhz: i32,
    /// The clock cap in MHz; 0 means none.
    pub cap_mhz: i32,
}

/// The GPU undervolt over an [`Nvml`]. NVML wants one caller at a time, so the calls of one value
/// are serialised.
pub struct GpuUndervolt {
    nvml: Arc<dyn Nvml>,
    gate: Mutex<()>,
}

impl GpuUndervolt {
    /// An undervolt driver over `nvml` ([`HelperNvml::system`] in production).
    pub fn new(nvml: Arc<dyn Nvml>) -> Self {
        Self { nvml, gate: Mutex::new(()) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        // The guard protects no data, only the order of calls: a panic elsewhere must not leave
        // the GPU without a way to be reset.
        self.gate.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn read_locked(&self) -> Result<State, GpuError> {
        let offset_mhz = self.nvml.offset().map_err(|e| match e {
            // "No helper", "no driver" and the like are about reaching the GPU at all.
            NvmlError::HelperMissing | NvmlError::LibraryNotFound | NvmlError::Init | NvmlError::NoDevice => {
                GpuError::reach(e)
            }
            e => GpuError::nvml("reading the GPU offset", e),
        })?;
        // A driver that cannot say its range gives [0, 0]: the range check is then off.
        let (lo, hi) = self.nvml.offset_range().unwrap_or((0, 0));
        Ok(State { offset_mhz, range_mhz: [lo, hi] })
    }

    /// The current curve offset and the allowed range (a harmless read).
    pub fn read(&self) -> Result<State, GpuError> {
        let _gate = self.lock();
        self.read_locked()
    }

    /// Shifts the curve by `offset` MHz and caps the clock at `cap_mhz` (0 = no cap), then reads the
    /// offset back. Both values are checked against the profile limits and the offset against the
    /// driver's range before anything is written. If a step fails after the first write, or the
    /// offset did not stick, the GPU is put back to stock (best effort) before the error returns.
    ///
    /// The caller is still responsible for the confirmation and the [`risk`] policy.
    pub fn apply(&self, offset: i32, cap_mhz: i32) -> Result<Applied, GpuError> {
        check_values(offset, cap_mhz)?;
        let _gate = self.lock();
        let state = self.read_locked()?;
        let [lo, hi] = state.range_mhz;
        if hi != 0 && !(lo..=hi).contains(&offset) {
            return Err(GpuError::DriverRange { lo, hi });
        }
        let result = self.write_locked(offset, cap_mhz);
        if result.is_err() {
            // Best effort: the error that matters is the original one.
            let _ = self.reset_locked();
        }
        result
    }

    fn write_locked(&self, offset: i32, cap_mhz: i32) -> Result<Applied, GpuError> {
        self.nvml.set_offset(offset).map_err(|e| GpuError::nvml("setting the GPU offset", e))?;
        if cap_mhz != 0 {
            // The cap is validated 600..=3300, so it fits.
            let max = u32::try_from(cap_mhz).unwrap_or(u32::MAX);
            self.nvml
                .lock_clocks(MIN_LOCKED_CLOCK_MHZ, max)
                .map_err(|e| GpuError::nvml("capping the GPU clock", e))?;
        } else {
            self.lift_cap()?;
        }
        let got = self.nvml.offset().map_err(|e| GpuError::nvml("reading the GPU offset", e))?;
        if got != offset {
            return Err(GpuError::OffsetNotKept { got, wanted: offset });
        }
        Ok(Applied { offset_mhz: got, cap_mhz })
    }

    /// Lifts the clock lock. A GPU or driver that does not support locked clocks has nothing to
    /// lift, which is success (the Python edition ignored the result entirely); any other failure
    /// is reported because a cap that stays on silently costs speed.
    fn lift_cap(&self) -> Result<(), GpuError> {
        match self.nvml.reset_clocks() {
            Err(e) if !e.is_not_supported() => Err(GpuError::nvml("lifting the GPU clock cap", e)),
            _ => Ok(()),
        }
    }

    fn reset_locked(&self) -> Result<Applied, GpuError> {
        // Both steps are tried even if the first fails, so a cap can never stay behind just
        // because the offset could not be reset.
        let mut errors = Vec::new();
        if let Err(e) = self.nvml.set_offset(0) {
            errors.push(GpuError::nvml("resetting the GPU offset", e));
        }
        if let Err(e) = self.lift_cap() {
            errors.push(e);
        }
        match errors.len() {
            0 => {}
            1 => return Err(errors.remove(0)),
            _ => return Err(GpuError::Several(errors)),
        }
        let state = self.read_locked()?;
        Ok(Applied { offset_mhz: state.offset_mhz, cap_mhz: 0 })
    }

    /// Puts the curve offset back to 0 and lifts the clock cap, and returns what the GPU reports.
    /// Never needs confirmation: stock is always safe.
    pub fn reset(&self) -> Result<Applied, GpuError> {
        let _gate = self.lock();
        self.reset_locked()
    }
}

/// Checks the limits a profile may hold, for a value that did not come through a profile.
fn check_values(offset: i32, cap_mhz: i32) -> Result<(), GpuError> {
    if !(OFFSET_MIN_MHZ..=OFFSET_MAX_MHZ).contains(&offset) {
        return Err(GpuError::Invalid(format!("offset: {OFFSET_MIN_MHZ}..{OFFSET_MAX_MHZ} MHz")));
    }
    if cap_mhz != 0 && !(CAP_MIN_MHZ..=CAP_MAX_MHZ).contains(&cap_mhz) {
        return Err(GpuError::Invalid(format!("cap: 0 (none) or {CAP_MIN_MHZ}..{CAP_MAX_MHZ} MHz")));
    }
    Ok(())
}

/// A named curve offset and clock cap. The JSON field names are the Python dict keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Profile {
    /// What the profile is for.
    pub description: String,
    /// MHz of curve shift, -300 to 250.
    pub offset: i32,
    /// Clock cap in MHz; 0 means none, otherwise 600 to 3300.
    pub cap: i32,
}

const BUILTIN_NAMES: [&str; 5] = ["native", "truesense", "mild", "cool", "efficient"];

/// The built-in profile of that name; they are read-only (copy one to make your own).
pub fn builtin(name: &str) -> Option<Profile> {
    let (description, offset, cap) = match name {
        "native" => ("PredatorSense default: no offset and no clock cap", 0, 0),
        "truesense" => (
            "Our profile for the RTX 4060 Laptop: a moderate curve shift with a clock cap, aiming for less power and heat at about the same speed. Unverified on your unit: test under load",
            120,
            2400,
        ),
        "mild" => ("Very safe: small shift, high cap", 60, 2500),
        "cool" => ("Cooler and quieter: lower cap, bigger shift", 100, 2100),
        "efficient" => ("Lowest power for light gaming: some games will run slower", 150, 1800),
        _ => return None,
    };
    Some(Profile { description: description.to_owned(), offset, cap })
}

/// The built-in profile names in display order.
pub fn builtin_names() -> &'static [&'static str] {
    &BUILTIN_NAMES
}

/// Whether `name` is a built-in (read-only) profile.
pub fn is_builtin(name: &str) -> bool {
    builtin(name).is_some()
}

fn offset_error() -> ProfileError {
    ProfileError::Invalid(format!("offset: {OFFSET_MIN_MHZ}..{OFFSET_MAX_MHZ} MHz"))
}

fn cap_error() -> ProfileError {
    ProfileError::Invalid(format!("cap: 0 (none) or {CAP_MIN_MHZ}..{CAP_MAX_MHZ} MHz"))
}

impl Profile {
    /// Checks the offset (-300..=250 MHz) and the cap (0 or 600..=3300 MHz).
    pub fn check(&self) -> Result<(), ProfileError> {
        if !(OFFSET_MIN_MHZ..=OFFSET_MAX_MHZ).contains(&self.offset) {
            return Err(offset_error());
        }
        if self.cap != 0 && !(CAP_MIN_MHZ..=CAP_MAX_MHZ).contains(&self.cap) {
            return Err(cap_error());
        }
        Ok(())
    }

    /// Validates a profile that arrived as decoded JSON and returns it normalised: only the known
    /// keys are kept, a missing cap means 0 (none), numbers are truncated to whole MHz, and text, a
    /// boolean or null is refused.
    pub fn from_value(value: &Value) -> Result<Profile, ProfileError> {
        let Some(object) = value.as_object() else {
            return Err(ProfileError::Invalid(
                "a GPU undervolt profile has 'offset' (MHz) and 'cap' (MHz, 0 = none)".into(),
            ));
        };
        let description = object.get("description").and_then(Value::as_str).unwrap_or_default().to_owned();
        let offset = match object.get("offset").and_then(Value::as_f64) {
            Some(v) if (f64::from(OFFSET_MIN_MHZ)..=f64::from(OFFSET_MAX_MHZ)).contains(&v) => v as i32,
            _ => return Err(offset_error()),
        };
        let cap = match object.get("cap") {
            None => 0,
            Some(raw) => match raw.as_f64() {
                Some(v) if v == 0.0 || (f64::from(CAP_MIN_MHZ)..=f64::from(CAP_MAX_MHZ)).contains(&v) => {
                    v as i32
                }
                _ => return Err(cap_error()),
            },
        };
        Ok(Profile { description, offset, cap })
    }
}

pub(crate) struct GpuKind;

impl Kind for GpuKind {
    type Profile = Profile;
    const WHAT: &'static str = "GPU undervolt";
    const LIST_COMMAND: &'static str = "truesense gpu undervolt list";
    const COPY_COMMAND: &'static str = "gpu undervolt";

    fn builtin(name: &str) -> Option<Profile> {
        builtin(name)
    }

    fn builtin_names() -> &'static [&'static str] {
        &BUILTIN_NAMES
    }

    fn parse(value: &Value) -> Result<Profile, ProfileError> {
        Profile::from_value(value)
    }

    fn check(profile: &Profile) -> Result<(), ProfileError> {
        profile.check()
    }
}

/// Rates an offset and cap. The decision layer refuses a [`Level::Danger`] setting unless the
/// caller says force.
pub fn risk(offset: i32, cap_mhz: i32) -> Rating {
    if offset > DANGER_OFFSET_MHZ {
        return Rating::new(
            Level::Danger,
            format!(
                "Beyond the danger line (+{DANGER_OFFSET_MHZ} MHz). Games and the driver can crash; the screen can glitch."
            ),
        );
    }
    if offset > SAFE_OFFSET_MHZ {
        return Rating::new(
            Level::Caution,
            format!(
                "Past the safety line (+{SAFE_OFFSET_MHZ} MHz). It may crash in some games: test under load."
            ),
        );
    }
    if offset > 0 && cap_mhz == 0 {
        return Rating::new(
            Level::Caution,
            "A positive offset without a clock cap can push the clock above stock: set a cap (2400 MHz is typical) or keep the offset small.",
        );
    }
    if offset < -100 {
        return Rating::new(
            Level::Caution,
            "A large negative offset only slows the GPU; it does not save voltage the way a positive offset with a cap does.",
        );
    }
    if cap_mhz != 0 && cap_mhz < LOW_CAP_MHZ {
        return Rating::new(Level::Caution, "A cap this low will cost a lot of speed in games.");
    }
    if offset != 0 || cap_mhz != 0 {
        Rating::new(Level::Safe, "Inside the safe range.")
    } else {
        Rating::new(Level::Safe, "Stock.")
    }
}

/// The user's own GPU undervolt profiles, one JSON file per profile in a directory. The built-in
/// profiles are always available and read-only. Saving a profile applies nothing.
pub struct Store(NamedStore<GpuKind>);

/// `<home>/.config/truesense/gpu-undervolt` from an environment lookup; `None` without `HOME`.
pub fn default_dir_from(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    env("HOME").map(|home| PathBuf::from(home).join(".config/truesense/gpu-undervolt"))
}

/// [`default_dir_from`] for the running process.
pub fn default_dir() -> Option<PathBuf> {
    default_dir_from(&fsx::env_nonempty)
}

impl Store {
    /// A store in `dir` (created when the first profile is saved).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self(NamedStore::new(dir))
    }

    /// The directory the profiles live in.
    pub fn dir(&self) -> &Path {
        self.0.dir()
    }

    /// The built-in names first, then the user's, sorted.
    pub fn names(&self) -> Result<Vec<String>, ProfileError> {
        self.0.names()
    }

    /// A built-in or saved profile, validated. `stock` is the old name of `native`.
    pub fn load(&self, name: &str) -> Result<Profile, ProfileError> {
        self.0.load(name)
    }

    /// Validates a profile and writes it atomically under `name`; built-in names are refused.
    pub fn save(&self, name: &str, profile: &Profile) -> Result<Profile, ProfileError> {
        self.0.save(name, profile)
    }

    /// Removes a saved profile; built-in profiles cannot be deleted.
    pub fn delete(&self, name: &str) -> Result<(), ProfileError> {
        self.0.delete(name)
    }

    /// Saves a copy of profile `name` as `new`, or as `NAME-copy` (then `-2`, `-3`, ...) when `new`
    /// is missing, empty or taken; returns the name used.
    pub fn copy(&self, name: &str, new: Option<&str>) -> Result<String, ProfileError> {
        self.0.copy(name, new)
    }
}

#[cfg(test)]
mod tests;
