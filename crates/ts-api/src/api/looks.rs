//! Saved lighting looks (keyboard and rear light together), the look applied at boot, and what the
//! lights and the profile do at login.
#![forbid(unsafe_code)]

use serde::Serialize;
use ts_domain::looks::{BUILTIN_NAMES, Look};
use ts_ipc::wire::Args;

use super::{Api, ApiError, ApiResult};

/// What the lights and the profile do at login (the `[startup]` section of the configuration).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StartupSettings {
    /// A whole-setup profile to apply, or empty.
    pub profile: String,
    /// `last`, `default`, `none`, `off`, `profile:NAME` or `look:NAME`.
    pub keyboard: String,
    /// `last`, `default`, `none`, `off` or `look:NAME`.
    pub rear: String,
}

impl Api {
    /// The saved looks, built-in ones first.
    pub fn looks(&self) -> Vec<String> {
        self.s.looks.names()
    }

    /// Saves what is on the lights now (keyboard look and rear light) under `name`.
    pub fn save_look(&self, name: &str) -> ApiResult<Look> {
        Ok(self.s.looks.save(name, None)?)
    }

    /// Saves a look as the window holds it.
    pub fn save_look_value(&self, name: &str, look: &Look) -> ApiResult<Look> {
        Ok(self.s.looks.save(name, Some(look))?)
    }

    /// Reads a saved or built-in look.
    pub fn look(&self, name: &str) -> ApiResult<Look> {
        Ok(self.s.looks.load(name)?)
    }

    /// Puts a look on the lights and answers what was done.
    pub fn apply_look(&self, name: &str) -> ApiResult<Vec<String>> {
        let look = self.s.looks.load(name)?;
        Ok(self.s.looks.apply(&look)?)
    }

    /// Removes one of your looks.
    pub fn delete_look(&self, name: &str) -> ApiResult<()> {
        Ok(self.s.looks.delete(name)?)
    }

    /// Saves a look under a new name (`NAME-copy`, numbered until free) and answers it.
    pub fn copy_look(&self, name: &str, new_name: Option<&str>) -> ApiResult<String> {
        Ok(self.s.looks.copy(name, new_name)?)
    }

    /// Whether a look is read-only.
    pub fn look_is_builtin(name: &str) -> bool {
        BUILTIN_NAMES.contains(&name)
    }

    /// The login settings.
    pub fn startup(&self) -> StartupSettings {
        let c = self.cfg().startup;
        StartupSettings { profile: c.profile, keyboard: c.keyboard, rear: c.rear }
    }

    /// The look the service applies at boot (before login) and after suspend; `None` when none.
    pub fn boot_look(&self) -> ApiResult<Option<Look>> {
        self.ask("boot_look", Args::new().with("action", "get"))
    }

    /// Stops applying a look at boot.
    pub fn clear_boot_look(&self) -> ApiResult<()> {
        self.ask_value("boot_look", Args::new().with("action", "clear"))?;
        Ok(())
    }

    /// Makes a look the boot look: `current` is what is on the lights now, anything else a saved
    /// look's name. The service keeps its own copy, so it works before login.
    pub fn set_boot_look(&self, what: &str) -> ApiResult<Look> {
        let look = if what == "current" { self.s.looks.capture()? } else { self.s.looks.load(what)? };
        if look.is_empty() {
            return Err(ApiError::invalid("a look needs keyboard and/or rear settings"));
        }
        let value = serde_json::to_value(&look)?;
        self.ask_value("boot_look", Args::new().with("action", "set").with("look", value))?;
        Ok(look)
    }
}
