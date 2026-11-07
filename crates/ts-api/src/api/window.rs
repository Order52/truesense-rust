//! What the web window needs beyond what the command line does: the whole-picture helpers, the
//! autostart entry, and "reset everything to defaults".
//!
//! The window shows the same operations as the other clients; these are only the shapes it wants
//! (a design with its colours, the services it can repair without a password prompt).
#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;

use serde::Serialize;
use ts_domain::keymap::Color;
use ts_foundation::buildinfo;
use ts_foundation::fsx;
use ts_platform::hw::{RearMode, Rgb};
use ts_system::services::Step;

use super::cpu::UndervoltRequest;
use super::keyboard::hex_cells;
use super::power::AutoMode;
use super::rear::RearRequest;
use super::{Api, ApiError, ApiResult};

/// A design as the window edits it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DesignShown {
    /// What it looks like.
    pub info: String,
    /// `built-in` or `yours`.
    pub source: String,
    /// The 128 colours as `rrggbb`.
    pub cells: Vec<String>,
}

/// One thing "reset all" did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResetStep {
    /// What it was.
    pub step: String,
    /// Whether it worked.
    pub ok: bool,
    /// Why it did not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The autostart entry that opens the window at login.
const AUTOSTART_ENTRY: &str =
    "[Desktop Entry]\nType=Application\nName=TrueSense\nExec=truesense gui\nX-GNOME-Autostart-enabled=true\n";

impl Api {
    /// This build's version.
    pub fn version() -> &'static str {
        buildinfo::VERSION
    }

    /// A design's colours.
    pub fn show_design(&self, name: &str) -> ApiResult<DesignShown> {
        let design = self.s.designs.get(name)?;
        let cells: Vec<Color> = design.cells_of(self.s.keymap)?;
        Ok(DesignShown {
            info: design.info,
            source: design.source.name().to_owned(),
            cells: hex_cells(&cells),
        })
    }

    /// Saves given colours as one of your designs (`None`: the picture now on the keyboard) and
    /// answers its file.
    pub fn save_design_cells(&self, name: &str, cells: Option<&[String]>) -> ApiResult<PathBuf> {
        if name.is_empty() {
            return Err(ApiError::invalid("give a design name"));
        }
        match cells {
            None => self.save_design(name),
            Some(c) => Ok(self.s.designs.save(name, &super::keyboard::parse_cells(c)?)?),
        }
    }

    fn autostart_path(&self) -> ApiResult<PathBuf> {
        self.s
            .autostart_dir
            .as_ref()
            .map(|d| d.join("truesense.desktop"))
            .ok_or_else(|| ApiError::unavailable("there is no autostart directory (HOME is not set)"))
    }

    /// Whether the window opens at login.
    pub fn autostart(&self) -> bool {
        self.autostart_path().is_ok_and(|p| fsx::exists(&p))
    }

    /// Makes the window open at login (an autostart entry in the user's configuration) or not, and
    /// answers the state afterwards.
    pub fn set_autostart(&self, on: bool) -> ApiResult<bool> {
        let path = self.autostart_path()?;
        if !on {
            return match fs::remove_file(&path) {
                Ok(()) => Ok(false),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(e) => Err(ApiError::domain("autostart", e)),
            };
        }
        fsx::write_atomic_mkdir(&path, AUTOSTART_ENTRY.as_bytes(), 0o644)?;
        Ok(true)
    }

    /// The part of the repair a window can do without a password prompt: the user's own services.
    /// When the system part is left, a step says so.
    pub fn repair_user_services(&self) -> ApiResult<Vec<Step>> {
        let checks = self.service_checks()?;
        let mut steps = checks.repair_user();
        if self.s.euid != 0 {
            steps.push(Step {
                what: "the system part (daemon, health timer, module) needs `sudo truesense service repair`"
                    .to_owned(),
                ok: true,
            });
        }
        Ok(steps)
    }

    /// Puts the fan curve, the Smart auto profile, the fans, the mode control, the key bindings, the
    /// power limits, both undervolts, the keyboard and the light bar back to their defaults. Every
    /// step runs even if one fails; saved profiles are kept.
    pub fn reset_all(&self) -> Vec<ResetStep> {
        let mut steps = Vec::new();
        let mut run = |name: &str, result: ApiResult<()>| {
            steps.push(ResetStep {
                step: name.to_owned(),
                ok: result.is_ok(),
                error: result.err().map(|e| e.to_string()),
            });
        };
        let none = UndervoltRequest::default();
        run("fan curve: native", self.use_fan_curve("native").map(drop));
        run("Smart auto profile: native", self.use_smart_profile("native").map(drop));
        run("fans: firmware auto", self.fan_auto().map(drop));
        run("mode control: native", self.set_auto_mode(AutoMode::Native));
        run("key bindings: native", self.use_key_profile("native"));
        run("power limits: automatic", self.reset_cpu_limits().map(drop));
        run("CPU undervolt: no boot profile", self.set_cpu_undervolt_boot(None, none).map(drop));
        run("CPU undervolt: stock", self.reset_cpu_undervolt().map(drop));
        run("GPU undervolt: no boot profile", self.set_gpu_undervolt_boot(None, none).map(drop));
        run("GPU undervolt: stock", self.reset_gpu_undervolt().map(drop));
        run("keyboard: stock look", self.restore_keyboard());
        let stock = RearRequest {
            mode: RearMode::Breathing,
            color: Rgb([0x37, 0x23, 0xc7]),
            speed: 3,
            brightness: 100,
            direction: 1,
            zones: None,
        };
        run("light bar: stock", self.set_rear(&stock).map(drop));
        steps
    }
}
