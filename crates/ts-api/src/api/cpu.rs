//! The CPU: power limits (PL1 and PL2) and the guarded undervolt.
//!
//! What persists or needs root goes through the service; the plain readings
//! ([`Api::cpu_info`]) come straight from the hardware. An undervolt always needs the explicit
//! confirmation, and a value beyond the danger line needs `force` as well, checked here before the
//! request is sent (and again by the service).
#![forbid(unsafe_code)]

use std::fmt;

use serde::{Deserialize, Serialize};
use ts_domain::power::{self, Info, MsrLock};
use ts_domain::undervolt::{self, Level, Profile};
use ts_ipc::wire::Args;

use super::{Api, ApiError, ApiResult, NamedList, Refusal};

/// Whether the package power limits are locked until reboot: a flag, or the reason it cannot be read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LockBit {
    /// Bit 63 of `MSR_PKG_POWER_LIMIT`.
    Locked(bool),
    /// Why the register could not be read.
    Unreadable(String),
}

impl Default for LockBit {
    fn default() -> Self {
        Self::Locked(false)
    }
}

/// The GPU power limits in watts as `nvidia-smi` prints them (text: a driver may print `[N/A]`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GpuPowerLimits {
    /// The limit in force.
    pub current: String,
    /// The factory default.
    pub default: String,
    /// The lowest the driver accepts.
    pub min: String,
    /// The highest the driver accepts.
    pub max: String,
}

/// The CPU package power limits, the MSR lock and the GPU power limit, as read from the hardware.
/// What cannot be read is `null`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LimitsInfo {
    /// PL1 (sustained) in watts.
    pub pl1_w: Option<f64>,
    /// PL2 (burst) in watts.
    pub pl2_w: Option<f64>,
    /// The `enabled` attribute of the RAPL zone.
    pub rapl_enabled: Option<String>,
    /// Whether the limits are locked.
    pub msr_lock_bit: LockBit,
    /// The GPU limits, when `nvidia-smi` answers.
    pub nvidia_power_limit_w: Option<GpuPowerLimits>,
}

impl From<Info> for LimitsInfo {
    fn from(i: Info) -> Self {
        Self {
            pl1_w: i.pl1_w,
            pl2_w: i.pl2_w,
            rapl_enabled: i.rapl_enabled,
            msr_lock_bit: match i.msr_lock_bit {
                MsrLock::Locked(b) => LockBit::Locked(b),
                MsrLock::Unreadable(why) => LockBit::Unreadable(why),
            },
            nvidia_power_limit_w: i.nvidia_power_limit_w.map(|n| GpuPowerLimits {
                current: n.current,
                default: n.default,
                min: n.min,
                max: n.max,
            }),
        }
    }
}

/// Power limits you set (the service keeps them).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct UserLimits {
    /// Sustained limit in watts.
    pub pl1: f64,
    /// Burst limit in watts.
    pub pl2: f64,
}

/// The limits the service picks by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoLimits {
    /// Watts normally.
    pub normal_w: i64,
    /// Watts while gaming.
    pub gaming_w: i64,
}

/// The service's view of the CPU power limits: the live values, the ones you set (`null` when
/// automatic) and the automatic ones from the configuration.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PowerLimitsState {
    /// The live values.
    #[serde(flatten)]
    pub info: LimitsInfo,
    /// What you set; `None` when automatic.
    pub user: Option<UserLimits>,
    /// What the service picks by itself.
    pub auto: AutoLimits,
}

impl fmt::Display for PowerLimitsState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let watts = |w: Option<f64>| w.map_or_else(|| "None".to_owned(), |v| v.to_string());
        write!(f, "PL1 {} W, PL2 {} W   ", watts(self.info.pl1_w), watts(self.info.pl2_w))?;
        match self.user {
            Some(u) => write!(f, "(yours: {}/{} W)", u.pl1, u.pl2),
            None => write!(f, "(automatic: {} W, {} W when gaming)", self.auto.normal_w, self.auto.gaming_w),
        }
    }
}

/// The limits the hardware reports after a write.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AppliedLimits {
    /// PL1 in watts, as read back.
    pub pl1_w: f64,
    /// PL2 in watts, as read back.
    pub pl2_w: f64,
}

impl fmt::Display for AppliedLimits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "power limits set: PL1 {} W, PL2 {} W   (the service keeps them; `truesense cpu limits --auto` goes back)",
            self.pl1_w, self.pl2_w
        )
    }
}

/// How an undervolt is asked for: it needs the explicit confirmation, and a value beyond the danger
/// line needs `force` as well.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UndervoltRequest {
    /// "I understand this can freeze or reboot the laptop."
    pub confirm: bool,
    /// Allow values beyond the danger line.
    pub force: bool,
}

/// Which voltage plane a single offset applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UndervoltPlane {
    /// The CPU core plane (plane 0).
    Core,
    /// The cache plane (plane 2).
    Cache,
}

impl UndervoltPlane {
    /// The plane's number as the command line gives it: 0 or 2.
    pub fn from_number(n: i64) -> ApiResult<Self> {
        match n {
            0 => Ok(Self::Core),
            2 => Ok(Self::Cache),
            _ => Err(ApiError::invalid("plane: 0 (core) or 2 (cache)")),
        }
    }
}

/// An undervolt the service applied or keeps for boot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedUndervolt {
    /// The profile's name (or `manual`).
    pub name: Option<String>,
    /// Core offset in mV.
    pub core: i32,
    /// Cache offset in mV.
    pub cache: i32,
}

/// The current offsets in mV.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndervoltOffsets {
    /// The core plane offset.
    pub core_mv: i32,
    /// The cache plane offset.
    pub cache_mv: i32,
}

impl fmt::Display for UndervoltOffsets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "core {} mV, cache {} mV", self.core_mv, self.cache_mv)
    }
}

/// The service's view of the CPU undervolt: the offsets (or why they cannot be read), what was
/// applied, what is kept for boot, and whether the boot crash guard is armed.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CpuUndervolt {
    /// The core offset in mV.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub core_mv: Option<i32>,
    /// The cache offset in mV.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_mv: Option<i32>,
    /// Why the offsets cannot be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// What was applied since boot.
    pub applied: Option<AppliedUndervolt>,
    /// What is applied at boot.
    pub boot: Option<AppliedUndervolt>,
    /// The boot crash-guard flag exists (the previous boot has not yet stayed up 10 minutes).
    pub pending: bool,
}

impl fmt::Display for CpuUndervolt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.core_mv, self.cache_mv) {
            (Some(core), Some(cache)) => write!(f, "core {core} mV, cache {cache} mV")?,
            _ => write!(f, "not readable: {}", self.error.as_deref().unwrap_or("?"))?,
        }
        if let Some(a) = &self.applied {
            write!(f, "   (applied: {})", a.name.as_deref().unwrap_or("?"))?;
        }
        if let Some(b) = &self.boot {
            write!(f, "   (at boot: {})", b.name.as_deref().unwrap_or("?"))?;
        }
        Ok(())
    }
}

impl Api {
    /// The CPU package power limits, the MSR lock and the GPU power limit, straight from the
    /// hardware (every part is best effort: what cannot be read is `null`).
    pub fn cpu_info(&self) -> LimitsInfo {
        self.s.power_limits.info().into()
    }

    /// Asks the service for the power limits.
    pub fn cpu_limits(&self) -> ApiResult<PowerLimitsState> {
        self.ask("cpu_info", Args::new())
    }

    /// Sets PL1 (sustained) and PL2 (burst) in watts; the service keeps them. A pair beyond the
    /// danger line is refused unless `force`.
    pub fn set_cpu_limits(&self, pl1: f64, pl2: f64, force: bool) -> ApiResult<AppliedLimits> {
        let rating = power::risk(pl1, pl2);
        if rating.level == Level::Danger && !force && self.daemon_available() {
            return Err(Refusal::Danger { why: rating.message }.into());
        }
        self.ask("cpu_limits", Args::new().with("pl1", pl1).with("pl2", pl2).with("force", force))
    }

    /// Goes back to the automatic limits and answers what the service reports then.
    pub fn reset_cpu_limits(&self) -> ApiResult<PowerLimitsState> {
        self.ask_value("cpu_limits", Args::new().with("reset", true))?;
        self.cpu_limits()
    }

    /// The service's view of the CPU undervolt.
    pub fn cpu_undervolt_state(&self) -> ApiResult<CpuUndervolt> {
        self.ask("undervolt", Args::new().with("action", "get"))
    }

    /// Lists the CPU undervolt profiles (built-in first).
    pub fn cpu_undervolts(&self) -> ApiResult<NamedList> {
        let mut list = NamedList { names: self.s.cpu_undervolt.names()?, ..NamedList::default() };
        for n in &list.names {
            if let Some(p) = undervolt::builtin(n) {
                list.descriptions.insert(n.clone(), p.description);
            }
        }
        Ok(list)
    }

    /// Reads one profile.
    pub fn cpu_undervolt_profile(&self, name: &str) -> ApiResult<Profile> {
        Ok(self.s.cpu_undervolt.load(name)?)
    }

    /// Writes stock voltage (never needs a confirmation).
    pub fn reset_cpu_undervolt(&self) -> ApiResult<UndervoltOffsets> {
        self.ask("undervolt", Args::new().with("action", "reset"))
    }

    fn check_cpu_undervolt(&self, core: i32, cache: i32, request: UndervoltRequest) -> ApiResult<()> {
        // Without the service nothing can be applied, and its own message says so.
        if !self.daemon_available() {
            return Ok(());
        }
        if !request.confirm {
            return Err(Refusal::UndervoltNeedsConfirmation { gpu: false }.into());
        }
        let rating = undervolt::risk(core, cache);
        if rating.level == Level::Danger && !request.force {
            return Err(Refusal::Danger { why: rating.message }.into());
        }
        Ok(())
    }

    /// Applies a profile through the service (until the next reboot).
    pub fn apply_cpu_undervolt(&self, name: &str, request: UndervoltRequest) -> ApiResult<UndervoltOffsets> {
        let p = self.s.cpu_undervolt.load(name)?;
        self.apply_cpu_undervolt_values(name, p.core, p.cache, request)
    }

    /// Changes one plane and keeps the other at what the service reports now (the profile name is
    /// `manual`).
    pub fn set_cpu_undervolt(
        &self,
        plane: UndervoltPlane,
        mv: i32,
        request: UndervoltRequest,
    ) -> ApiResult<UndervoltOffsets> {
        let current = self.cpu_undervolt_state()?;
        let (mut core, mut cache) = (current.core_mv.unwrap_or(0), current.cache_mv.unwrap_or(0));
        match plane {
            UndervoltPlane::Core => core = mv,
            UndervoltPlane::Cache => cache = mv,
        }
        self.apply_cpu_undervolt_values("manual", core, cache, request)
    }

    /// Applies offsets given as numbers (the window's editor) under a name that labels them. Like
    /// every undervolt it needs the confirmation, and a value beyond the danger line needs `force`
    /// as well: refused here even if the caller already asked the user.
    pub fn apply_cpu_undervolt_values(
        &self,
        name: &str,
        core: i32,
        cache: i32,
        request: UndervoltRequest,
    ) -> ApiResult<UndervoltOffsets> {
        self.check_cpu_undervolt(core, cache, request)?;
        let args = Args::new()
            .with("action", "apply")
            .with("core", core)
            .with("cache", cache)
            .with("confirm", request.confirm)
            .with("name", name)
            .with("force", request.force);
        self.ask("undervolt", args)
    }

    /// Saves a copy under `new_name` (`NAME-copy` when none) and answers the name.
    pub fn copy_cpu_undervolt(&self, name: &str, new_name: Option<&str>) -> ApiResult<String> {
        Ok(self.s.cpu_undervolt.copy(name, new_name)?)
    }

    /// Saves a profile (it applies nothing).
    pub fn save_cpu_undervolt(
        &self,
        name: &str,
        core: Option<i32>,
        cache: Option<i32>,
    ) -> ApiResult<Profile> {
        let (Some(core), Some(cache)) = (core, cache) else {
            return Err(ApiError::invalid("usage: truesense cpu undervolt save NAME --core -50 --cache -40"));
        };
        if name.is_empty() {
            return Err(ApiError::invalid("usage: truesense cpu undervolt save NAME --core -50 --cache -40"));
        }
        Ok(self.s.cpu_undervolt.save(name, &Profile { description: String::new(), core, cache })?)
    }

    /// Removes one of your profiles.
    pub fn delete_cpu_undervolt(&self, name: &str) -> ApiResult<()> {
        Ok(self.s.cpu_undervolt.delete(name)?)
    }

    /// Makes the service apply a profile at boot, guarded: it is skipped and switched off when the
    /// previous boot did not stay up. `None` or `off` clears it.
    pub fn set_cpu_undervolt_boot(
        &self,
        name: Option<&str>,
        request: UndervoltRequest,
    ) -> ApiResult<Option<AppliedUndervolt>> {
        match name.filter(|n| !n.is_empty() && *n != "off") {
            None => {
                self.ask_value("undervolt", Args::new().with("action", "boot_clear"))?;
                Ok(None)
            }
            Some(name) => {
                let p = self.s.cpu_undervolt.load(name)?;
                self.set_cpu_undervolt_boot_values(name, p.core, p.cache, request).map(Some)
            }
        }
    }

    /// Makes the service apply offsets given as numbers at boot (guarded).
    pub fn set_cpu_undervolt_boot_values(
        &self,
        name: &str,
        core: i32,
        cache: i32,
        request: UndervoltRequest,
    ) -> ApiResult<AppliedUndervolt> {
        self.check_cpu_undervolt(core, cache, request)?;
        let args = Args::new()
            .with("action", "boot")
            .with("core", core)
            .with("cache", cache)
            .with("confirm", request.confirm)
            .with("name", name)
            .with("force", request.force);
        self.ask("undervolt", args)
    }

    /// Whether a profile is read-only.
    pub fn cpu_undervolt_is_builtin(name: &str) -> bool {
        undervolt::is_builtin(name)
    }
}
