//! undervolt: the CPU undervolt (the Intel voltage-offset mailbox, MSR 0x150), its safety zones,
//! the risk rating and the store of undervolt profiles.
//!
//! # Safety
//!
//! A wrong voltage offset can freeze or reboot the laptop, so this module is built so that nothing
//! outside the safety zones can reach the register:
//!
//! * the only write path is [`Undervolt::set`] / [`Undervolt::apply`] / [`Undervolt::reset`], and
//!   every offset is checked against the hard zone (0 to -120 mV) **before** the first write,
//!   for both planes of a pair;
//! * a write needs the explicit `confirm` flag (the caller asks the user every time);
//! * every write is read back; an offset that does not read back is rolled back to stock, and a
//!   pair that fails half way is reset;
//! * the real device ([`DevMsr`]) writes only register 0x150, whatever it is asked;
//! * the CPU keeps no offset across a reboot, so a freeze is always recovered by rebooting.
//!
//! The finer zones (past -60 mV core / -50 mV cache "caution", past -90 / -70 "danger") are advice:
//! [`risk`] rates a pair and the decision layer asks for a typed confirmation or `--force`. The
//! crash guard (flag files that make the next boot skip an offset that never proved stable) is
//! handled by the decision layer; it needs only [`Undervolt::apply`], [`Undervolt::reset`] and
//! [`Undervolt::read`] from here.
//!
//! Hardware access is behind the [`Msr`] trait, so the mailbox logic is tested against a fake and
//! never touches `/dev/cpu`.
#![forbid(unsafe_code)]

mod store;

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use serde::Serialize;
use serde_json::Value;
use ts_foundation::fsx;

pub use crate::power::{Level, Rating};
pub use store::ProfileError;
pub(crate) use store::{Kind, NamedStore};

/// The voltage-offset mailbox (verified layout, 13th-gen HX): a write of this register is a
/// request, the read that follows returns the answer.
pub(crate) const MAILBOX_MSR: u32 = 0x150;
/// CPU 0's MSR device; it needs root and the `msr` kernel module.
pub(crate) const DEFAULT_MSR_PATH: &str = "/dev/cpu/0/msr";

/// The deepest offset ever written: 0 to -120 mV.
pub(crate) const MAX_MV: i32 = 120;
/// Up to here the core offset is usually fine on 13th-gen HX.
pub(crate) const SAFE_CORE_MV: u32 = 60;
/// Up to here the cache offset is usually fine.
pub(crate) const SAFE_CACHE_MV: u32 = 50;
/// Beyond the safe value and up to here the core offset may be unstable: test under load.
pub(crate) const CAUTION_CORE_MV: u32 = 90;
/// Beyond the safe value and up to here the cache offset may be unstable.
pub(crate) const CAUTION_CACHE_MV: u32 = 70;

/// The mailbox works in steps of 1/1.024 mV, so a read-back may differ from the request by this.
const VERIFY_TOLERANCE_MV: i32 = 2;

/// A voltage plane of the mailbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plane {
    /// The CPU core plane.
    Core,
    /// The ring / cache plane.
    Cache,
}

impl Plane {
    /// The plane's number in the mailbox request.
    fn id(self) -> u64 {
        match self {
            Plane::Core => 0,
            Plane::Cache => 2,
        }
    }
}

/// Reads and writes model-specific registers of CPU 0.
pub trait Msr: Send + Sync {
    /// The register's value.
    fn read(&self, register: u32) -> io::Result<u64>;
    /// Stores a value in the register.
    fn write(&self, register: u32, value: u64) -> io::Result<()>;
}

/// The real MSR device. It opens the device for every access (nothing stays open between calls)
/// and the kernel's `msr` driver takes the register number as the file offset and moves 8 bytes.
///
/// It refuses to write any register but [`MAILBOX_MSR`]: this type exists for the voltage mailbox
/// only, and a stray write to another MSR (power limits, performance bias) is not something any
/// caller of this crate may ever ask for.
#[derive(Debug, Clone)]
pub struct DevMsr {
    path: PathBuf,
}

impl DevMsr {
    /// The device at `path` (a temporary file in tests).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// CPU 0's device, [`DEFAULT_MSR_PATH`].
    pub fn cpu0() -> Self {
        Self::new(DEFAULT_MSR_PATH)
    }
}

impl Msr for DevMsr {
    fn read(&self, register: u32) -> io::Result<u64> {
        let file = File::open(&self.path)?;
        let mut buf = [0u8; 8];
        file.read_exact_at(&mut buf, u64::from(register))
            .map_err(|e| io::Error::new(e.kind(), format!("read MSR {register:#x}: {e}")))?;
        Ok(u64::from_le_bytes(buf))
    }

    fn write(&self, register: u32, value: u64) -> io::Result<()> {
        if register != MAILBOX_MSR {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "refusing to write MSR {register:#x}: only the voltage mailbox {MAILBOX_MSR:#x} is ever written"
                ),
            ));
        }
        let file = OpenOptions::new().write(true).open(&self.path)?;
        file.write_all_at(&value.to_le_bytes(), u64::from(register))
            .map_err(|e| io::Error::new(e.kind(), format!("write MSR {register:#x}: {e}")))
    }
}

/// A read-only view of the same device, for the power-limit lock bit.
impl<T: Msr + ?Sized> crate::power::MsrReader for T {
    fn read_msr(&self, register: u32) -> io::Result<u64> {
        self.read(register)
    }
}

/// The current core and cache offsets in millivolts. Field names are the Python dict keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct Offsets {
    /// The core plane offset.
    pub core_mv: i32,
    /// The cache plane offset.
    pub cache_mv: i32,
}

/// Why an undervolt operation failed. Each message says what to do next.
#[derive(Debug)]
pub enum UndervoltError {
    /// A write was asked for without the explicit confirmation.
    NeedsConfirmation,
    /// The offset is outside the hard zone; nothing was written.
    OutOfRange {
        /// The refused offset.
        mv: i32,
    },
    /// The mailbox could not be read or written for a read request (no root, or no `msr` module).
    NotAccessible(io::Error),
    /// The mailbox refused the write.
    WriteRefused(io::Error),
    /// The offset did not read back as written.
    Readback {
        /// What the CPU reported.
        got_mv: i32,
        /// What was written.
        wanted_mv: i32,
        /// Whether the plane was put back to stock and verified.
        rolled_back: bool,
    },
    /// After a reset a plane did not read as 0.
    ResetIncomplete {
        /// What the planes read.
        offsets: Offsets,
        /// A write error met on the way, if any.
        note: Option<String>,
    },
}

impl fmt::Display for UndervoltError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NeedsConfirmation => {
                f.write_str("undervolt needs explicit --confirm (can crash the machine if too aggressive)")
            }
            Self::OutOfRange { .. } => {
                write!(f, "offset must be between 0 and -{MAX_MV} mV (start with -10)")
            }
            Self::NotAccessible(e) => {
                write!(f, "MSR {MAILBOX_MSR:#x} not accessible: {e} (sudo modprobe msr; run as root)")
            }
            Self::WriteRefused(e) => write!(f, "MSR write refused: {e} (BIOS may lock undervolting)"),
            Self::Readback { got_mv, wanted_mv, rolled_back } => write!(
                f,
                "readback {got_mv} mV != {wanted_mv} mV (BIOS lock?); {}",
                if *rolled_back {
                    "rolled back to stock"
                } else {
                    "COULD NOT roll back: reset with: uv reset"
                }
            ),
            Self::ResetIncomplete { offsets, note } => write!(
                f,
                "reset did not take: core {} mV, cache {} mV{}",
                offsets.core_mv,
                offsets.cache_mv,
                note.as_ref().map(|n| format!(" ({n})")).unwrap_or_default()
            ),
        }
    }
}

impl std::error::Error for UndervoltError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NotAccessible(e) | Self::WriteRefused(e) => Some(e),
            _ => None,
        }
    }
}

/// Builds a mailbox request. Offsets are in units of 1/1.024 mV, two's complement, in the 11
/// bits at bit 21 (the field is masked with 12 bits like the Python edition did); the Python
/// edition rounded half to even, so this does too.
fn command(plane: Plane, write: bool, offset_mv: i32) -> u64 {
    let (op, offset) = if write {
        let units = (f64::from(offset_mv) * 1.024).round_ties_even() as i64;
        (0x11u64, ((units & 0xFFF) as u64) << 21)
    } else {
        (0x10u64, 0)
    };
    (1 << 63) | (plane.id() << 40) | (op << 32) | offset
}

/// The signed offset in millivolts from a mailbox answer.
fn decode(answer: u64) -> i32 {
    let mut raw = ((answer >> 21) & 0x7FF) as i32;
    if raw & 0x400 != 0 {
        raw -= 0x800;
    }
    (f64::from(raw) / 1.024).round_ties_even() as i32
}

/// Checks that an offset lies in the hard zone, 0 to -[`MAX_MV`].
fn check_offset(offset_mv: i32) -> Result<(), UndervoltError> {
    if !(-MAX_MV..=0).contains(&offset_mv) {
        return Err(UndervoltError::OutOfRange { mv: offset_mv });
    }
    Ok(())
}

/// The undervolt mailbox. One value serialises every mailbox transaction, because a request and
/// its answer must not be interleaved with another caller's.
pub struct Undervolt {
    msr: Arc<dyn Msr>,
    gate: Mutex<()>,
}

impl Undervolt {
    /// An undervolt driver over `msr` ([`DevMsr::cpu0`] in production).
    pub fn new(msr: Arc<dyn Msr>) -> Self {
        Self { msr, gate: Mutex::new(()) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        // The guard protects no data, only the order of register accesses: a panic elsewhere
        // must not make the undervolt unusable (and unable to reset).
        self.gate.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn request(&self, plane: Plane, write: bool, offset_mv: i32) -> io::Result<()> {
        self.msr.write(MAILBOX_MSR, command(plane, write, offset_mv))
    }

    fn get_locked(&self, plane: Plane) -> Result<i32, UndervoltError> {
        self.request(plane, false, 0).map_err(UndervoltError::NotAccessible)?;
        let answer = self.msr.read(MAILBOX_MSR).map_err(UndervoltError::NotAccessible)?;
        Ok(decode(answer))
    }

    fn read_locked(&self) -> Result<Offsets, UndervoltError> {
        Ok(Offsets { core_mv: self.get_locked(Plane::Core)?, cache_mv: self.get_locked(Plane::Cache)? })
    }

    /// A plane's current offset in mV (a mailbox READ request: harmless). Needs root and the msr
    /// module.
    pub fn get(&self, plane: Plane) -> Result<i32, UndervoltError> {
        let _gate = self.lock();
        self.get_locked(plane)
    }

    /// Both planes' offsets.
    pub fn read(&self) -> Result<Offsets, UndervoltError> {
        let _gate = self.lock();
        self.read_locked()
    }

    fn set_locked(&self, plane: Plane, offset_mv: i32) -> Result<i32, UndervoltError> {
        self.request(plane, true, offset_mv).map_err(UndervoltError::WriteRefused)?;
        let got = match self.get_locked(plane) {
            Ok(got) => got,
            Err(e) => {
                self.roll_back(plane);
                return Err(e);
            }
        };
        if (got - offset_mv).abs() > VERIFY_TOLERANCE_MV {
            let rolled_back = self.roll_back(plane);
            return Err(UndervoltError::Readback { got_mv: got, wanted_mv: offset_mv, rolled_back });
        }
        Ok(got)
    }

    /// Puts one plane back to stock and says whether that is verified.
    fn roll_back(&self, plane: Plane) -> bool {
        self.request(plane, true, 0).is_ok()
            && self.get_locked(plane).is_ok_and(|v| v.abs() <= VERIFY_TOLERANCE_MV)
    }

    /// Writes one plane's offset (0 to -[`MAX_MV`] mV) and verifies it by reading it back; returns
    /// the offset the CPU reports. Without `confirm` it refuses, and an offset outside the hard zone
    /// is refused before anything is written. An offset that does not read back is rolled back.
    pub fn set(&self, plane: Plane, offset_mv: i32, confirm: bool) -> Result<i32, UndervoltError> {
        if !confirm {
            return Err(UndervoltError::NeedsConfirmation);
        }
        check_offset(offset_mv)?;
        let _gate = self.lock();
        self.set_locked(plane, offset_mv)
    }

    /// Sets both planes (core, then cache) with confirmation and verification, and returns what
    /// the CPU reports. Both offsets are checked before the first write, and if either write
    /// fails the pair is reset to stock before the error is returned, so a half-applied pair
    /// never stays.
    pub fn apply(&self, core_mv: i32, cache_mv: i32, confirm: bool) -> Result<Offsets, UndervoltError> {
        if !confirm {
            return Err(UndervoltError::NeedsConfirmation);
        }
        check_offset(core_mv)?;
        check_offset(cache_mv)?;
        let _gate = self.lock();
        let result = self
            .set_locked(Plane::Core, core_mv)
            .and_then(|_| self.set_locked(Plane::Cache, cache_mv))
            .and_then(|_| self.read_locked());
        if result.is_err() {
            // Best effort: the error that matters is the original one.
            let _ = self.reset_locked();
        }
        result
    }

    fn reset_locked(&self) -> Result<Offsets, UndervoltError> {
        let mut note = None;
        for plane in [Plane::Core, Plane::Cache] {
            // One refused write must not stop the other plane from being reset.
            if let Err(e) = self.request(plane, true, 0) {
                note.get_or_insert_with(|| e.to_string());
            }
        }
        let offsets = self.read_locked()?;
        if offsets != Offsets::default() {
            return Err(UndervoltError::ResetIncomplete { offsets, note });
        }
        Ok(offsets)
    }

    /// Writes 0 to both planes (stock voltage; it never needs confirmation) and reads them back.
    /// A write that is refused does not stop the other plane from being reset; planes that do not
    /// read back as 0 are an error that carries what they read.
    pub fn reset(&self) -> Result<Offsets, UndervoltError> {
        let _gate = self.lock();
        self.reset_locked()
    }
}

/// A named pair of offsets. The values are common conservative starting points for 13th-gen HX
/// chips, NOT validated on every unit: test each under load before trusting it. The JSON field
/// names are the Python dict keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Profile {
    /// What the profile is for.
    pub description: String,
    /// Core plane offset in mV, 0 to -120.
    pub core: i32,
    /// Cache plane offset in mV, 0 to -120.
    pub cache: i32,
}

const BUILTIN_NAMES: [&str; 5] = ["native", "truesense", "mild", "balanced", "aggressive"];

/// The built-in profile of that name; they are read-only (copy one to make your own).
pub fn builtin(name: &str) -> Option<Profile> {
    let (description, core, cache) = match name {
        "native" => ("PredatorSense default: no undervolt (0 mV)", 0, 0),
        "truesense" => (
            "Our profile for the i7-13700HX: a conservative offset that usually gives a few degrees less heat at the same speed. Unverified on your unit: test under load",
            -50,
            -40,
        ),
        "mild" => ("Very safe: small offset", -30, -30),
        "balanced" => ("Moderate: more heat saved, more chance of instability", -70, -50),
        "aggressive" => ("Maximum offered: only if you have tested it; can freeze or reboot", -100, -80),
        _ => return None,
    };
    Some(Profile { description: description.to_owned(), core, cache })
}

/// The built-in profile names in display order.
pub fn builtin_names() -> &'static [&'static str] {
    &BUILTIN_NAMES
}

/// Whether `name` is a built-in (read-only) profile.
pub fn is_builtin(name: &str) -> bool {
    builtin(name).is_some()
}

impl Profile {
    /// Checks that both offsets are 0 to -[`MAX_MV`] mV.
    pub fn check(&self) -> Result<(), ProfileError> {
        for (key, mv) in [("core", self.core), ("cache", self.cache)] {
            if !(-MAX_MV..=0).contains(&mv) {
                return Err(offset_error(key));
            }
        }
        Ok(())
    }

    /// Validates a profile that arrived as decoded JSON and returns it normalised: only the three
    /// known keys are kept, offsets are truncated to whole millivolts and may not be text, a
    /// boolean or null.
    pub fn from_value(value: &Value) -> Result<Profile, ProfileError> {
        let Some(object) = value.as_object() else {
            return Err(ProfileError::Invalid(
                "an undervolt profile has 'core' and 'cache' offsets in mV".into(),
            ));
        };
        let description = object.get("description").and_then(Value::as_str).unwrap_or_default().to_owned();
        let offset = |key: &'static str| -> Result<i32, ProfileError> {
            match object.get(key).and_then(Value::as_f64) {
                // A float is cut towards zero (-50.9 is -50), like Python's int().
                Some(v) if (-f64::from(MAX_MV)..=0.0).contains(&v) => Ok(v as i32),
                _ => Err(offset_error(key)),
            }
        };
        Ok(Profile { description, core: offset("core")?, cache: offset("cache")? })
    }
}

fn offset_error(key: &str) -> ProfileError {
    ProfileError::Invalid(format!("{key}: 0 to -{MAX_MV} mV"))
}

pub(crate) struct CpuKind;

impl Kind for CpuKind {
    type Profile = Profile;
    const WHAT: &'static str = "undervolt";
    const LIST_COMMAND: &'static str = "truesense cpu undervolt list";
    const COPY_COMMAND: &'static str = "cpu undervolt";

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

/// Rates a pair of offsets (in mV, negative): the safety line is where instability becomes likely,
/// the danger line where freezes are likely. The decision layer asks for a typed confirmation or
/// `--force` for [`Level::Danger`].
pub fn risk(core_mv: i32, cache_mv: i32) -> Rating {
    let (core, cache) = (core_mv.unsigned_abs(), cache_mv.unsigned_abs());
    if core > CAUTION_CORE_MV || cache > CAUTION_CACHE_MV {
        return Rating::new(
            Level::Danger,
            format!(
                "Beyond the danger line (core {CAUTION_CORE_MV} mV / cache {CAUTION_CACHE_MV} mV). Freezes and reboots are likely; unsaved work can be lost."
            ),
        );
    }
    if core > SAFE_CORE_MV || cache > SAFE_CACHE_MV {
        return Rating::new(
            Level::Caution,
            format!(
                "Past the safety line (core {SAFE_CORE_MV} mV / cache {SAFE_CACHE_MV} mV). It may be unstable: test under load before trusting it."
            ),
        );
    }
    if core != 0 || cache != 0 {
        Rating::new(Level::Safe, "Inside the safe range.")
    } else {
        Rating::new(Level::Safe, "Stock voltage.")
    }
}

/// The user's own undervolt profiles, one JSON file per profile in a directory. The built-in
/// profiles are always available and read-only. Saving a profile applies nothing.
pub struct Store(NamedStore<CpuKind>);

/// `<home>/.config/truesense/undervolt` from an environment lookup; `None` without `HOME`.
pub fn default_dir_from(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    env("HOME").map(|home| PathBuf::from(home).join(".config/truesense/undervolt"))
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
