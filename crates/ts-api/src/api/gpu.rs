//! The GPU: the MUX mode and the guarded undervolt (curve offset and clock cap).
#![forbid(unsafe_code)]

use std::fmt;

use serde::{Deserialize, Serialize};
use ts_domain::gpuundervolt::{self, Level, Profile};
use ts_ipc::wire::Args;
use ts_platform::hw::{Coded, GpuMode, Reading};

use super::cpu::UndervoltRequest;
use super::{Api, ApiError, ApiResult, NamedList, Refusal};

/// A GPU undervolt the service applied or keeps for boot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedGpuUndervolt {
    /// The profile's name (or `manual`).
    pub name: Option<String>,
    /// Curve offset in MHz.
    pub offset: i32,
    /// Clock cap in MHz; 0 means none.
    pub cap: i32,
}

/// What the GPU reports after an apply or reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuApplied {
    /// The curve offset the GPU reports.
    pub offset_mhz: i32,
    /// The clock cap in MHz; 0 means none.
    pub cap_mhz: i32,
}

impl fmt::Display for GpuApplied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let cap = if self.cap_mhz == 0 { "none".to_owned() } else { self.cap_mhz.to_string() };
        write!(f, "offset {:+} MHz, cap {cap} MHz", self.offset_mhz)
    }
}

/// The service's view of the GPU undervolt: the curve offset and the range the driver allows (or
/// why they cannot be read), what was applied, what is kept for boot.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GpuUndervoltState {
    /// The curve offset in MHz.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset_mhz: Option<i32>,
    /// The offsets the driver allows (min, max).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range_mhz: Option<[i32; 2]>,
    /// Why the offset cannot be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// What was applied since boot.
    pub applied: Option<AppliedGpuUndervolt>,
    /// What is applied at boot.
    pub boot: Option<AppliedGpuUndervolt>,
    /// The boot crash-guard flag exists.
    pub pending: bool,
}

impl fmt::Display for GpuUndervoltState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.offset_mhz, self.range_mhz) {
            (Some(offset), Some([lo, hi])) => write!(f, "offset {offset:+} MHz (driver range {lo}..{hi})")?,
            _ => write!(f, "not readable: {}", self.error.as_deref().unwrap_or("?"))?,
        }
        if let Some(a) = &self.applied {
            let cap = if a.cap == 0 { "none".to_owned() } else { a.cap.to_string() };
            write!(f, "   (applied: {}, cap {cap} MHz)", a.name.as_deref().unwrap_or("?"))?;
        }
        if let Some(b) = &self.boot {
            write!(f, "   (at boot: {})", b.name.as_deref().unwrap_or("?"))?;
        }
        Ok(())
    }
}

impl Api {
    /// The GPU (MUX) mode: optimus, nvidia or auto.
    pub fn gpu_mode(&self) -> ApiResult<Reading<GpuMode>> {
        Ok(self.s.hw.get_gpu_mode()?)
    }

    /// Sets the GPU mode (verified by read-back); it takes effect after a reboot.
    pub fn set_gpu_mode(&self, mode: GpuMode) -> ApiResult<GpuMode> {
        Ok(self.s.hw.set_gpu_mode(mode)?)
    }

    /// The GPU mode named by text (`optimus`, `nvidia`, `auto`).
    pub fn parse_gpu_mode(text: &str) -> ApiResult<GpuMode> {
        Ok(GpuMode::from_name(text)?)
    }

    /// The service's view of the GPU undervolt.
    pub fn gpu_undervolt_state(&self) -> ApiResult<GpuUndervoltState> {
        self.ask("gpu_undervolt", Args::new().with("action", "get"))
    }

    /// Lists the GPU undervolt profiles (built-in first).
    pub fn gpu_undervolts(&self) -> ApiResult<NamedList> {
        let mut list = NamedList { names: self.s.gpu_undervolt.names()?, ..NamedList::default() };
        for n in &list.names {
            if let Some(p) = gpuundervolt::builtin(n) {
                list.descriptions.insert(n.clone(), p.description);
            }
        }
        Ok(list)
    }

    /// Reads one profile.
    pub fn gpu_undervolt_profile(&self, name: &str) -> ApiResult<Profile> {
        Ok(self.s.gpu_undervolt.load(name)?)
    }

    /// Puts the curve offset back to 0 and lifts the clock cap (never needs a confirmation).
    pub fn reset_gpu_undervolt(&self) -> ApiResult<GpuApplied> {
        self.ask("gpu_undervolt", Args::new().with("action", "reset"))
    }

    fn check_gpu_undervolt(&self, offset: i32, cap: i32, request: UndervoltRequest) -> ApiResult<()> {
        // Without the service nothing can be applied, and its own message says so.
        if !self.daemon_available() {
            return Ok(());
        }
        if !request.confirm {
            return Err(Refusal::UndervoltNeedsConfirmation { gpu: true }.into());
        }
        let rating = gpuundervolt::risk(offset, cap);
        if rating.level == Level::Danger && !request.force {
            return Err(Refusal::Danger { why: rating.message }.into());
        }
        Ok(())
    }

    /// Applies a profile through the service (until the next reboot).
    pub fn apply_gpu_undervolt(&self, name: &str, request: UndervoltRequest) -> ApiResult<GpuApplied> {
        let p = self.s.gpu_undervolt.load(name)?;
        self.apply_gpu_undervolt_values(name, p.offset, p.cap, request)
    }

    /// Applies an offset and cap given as numbers (the window's editor) under a name that labels
    /// them. It needs the confirmation, and `force` beyond the danger line.
    pub fn apply_gpu_undervolt_values(
        &self,
        name: &str,
        offset: i32,
        cap: i32,
        request: UndervoltRequest,
    ) -> ApiResult<GpuApplied> {
        self.check_gpu_undervolt(offset, cap, request)?;
        let args = Args::new()
            .with("action", "apply")
            .with("offset", offset)
            .with("cap", cap)
            .with("confirm", request.confirm)
            .with("name", name)
            .with("force", request.force);
        self.ask("gpu_undervolt", args)
    }

    /// Saves a copy under `new_name` (`NAME-copy` when none) and answers the name.
    pub fn copy_gpu_undervolt(&self, name: &str, new_name: Option<&str>) -> ApiResult<String> {
        Ok(self.s.gpu_undervolt.copy(name, new_name)?)
    }

    /// Saves a profile (it applies nothing); a `cap` of 0 means no cap.
    pub fn save_gpu_undervolt(&self, name: &str, offset: Option<i32>, cap: i32) -> ApiResult<Profile> {
        match (name.is_empty(), offset) {
            (false, Some(offset)) => {
                Ok(self.s.gpu_undervolt.save(name, &Profile { description: String::new(), offset, cap })?)
            }
            _ => Err(ApiError::invalid("usage: truesense gpu undervolt save NAME --offset 100 --cap 2400")),
        }
    }

    /// Removes one of your profiles.
    pub fn delete_gpu_undervolt(&self, name: &str) -> ApiResult<()> {
        Ok(self.s.gpu_undervolt.delete(name)?)
    }

    /// Makes the service apply a profile at boot, guarded. `None` or `off` clears it.
    pub fn set_gpu_undervolt_boot(
        &self,
        name: Option<&str>,
        request: UndervoltRequest,
    ) -> ApiResult<Option<AppliedGpuUndervolt>> {
        match name.filter(|n| !n.is_empty() && *n != "off") {
            None => {
                self.ask_value("gpu_undervolt", Args::new().with("action", "boot_clear"))?;
                Ok(None)
            }
            Some(name) => {
                let p = self.s.gpu_undervolt.load(name)?;
                self.set_gpu_undervolt_boot_values(name, p.offset, p.cap, request).map(Some)
            }
        }
    }

    /// Makes the service apply an offset and cap given as numbers at boot (guarded).
    pub fn set_gpu_undervolt_boot_values(
        &self,
        name: &str,
        offset: i32,
        cap: i32,
        request: UndervoltRequest,
    ) -> ApiResult<AppliedGpuUndervolt> {
        self.check_gpu_undervolt(offset, cap, request)?;
        let args = Args::new()
            .with("action", "boot")
            .with("offset", offset)
            .with("cap", cap)
            .with("confirm", request.confirm)
            .with("name", name)
            .with("force", request.force);
        self.ask("gpu_undervolt", args)
    }

    /// Whether a profile is read-only.
    pub fn gpu_undervolt_is_builtin(name: &str) -> bool {
        gpuundervolt::is_builtin(name)
    }
}
