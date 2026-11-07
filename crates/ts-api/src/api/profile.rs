//! Whole-setup profiles: named bundles of settings (power, fans, lighting, battery, audio)
//! applied in one go. Applying one never changes the GPU mode (that needs a reboot) and never
//! touches an undervolt.
#![forbid(unsafe_code)]

use std::path::PathBuf;

use serde::Serialize;
use ts_domain::profiles::{Bundle, describe};
use ts_foundation::config::Value;
use ts_foundation::userstate::keys;

use super::{Api, ApiError, ApiResult};

/// One whole-setup profile of the lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfileInfo {
    /// The profile's name.
    pub name: String,
    /// `yours` or `shipped`.
    pub source: String,
    /// What it is for.
    pub description: String,
}

impl Api {
    /// The profile names, shipped and yours.
    pub fn profile_names(&self) -> Vec<String> {
        self.s.bundles.names()
    }

    /// Lists the profiles with their source and description.
    pub fn profiles(&self) -> Vec<ProfileInfo> {
        self.s
            .bundles
            .names()
            .into_iter()
            .map(|name| {
                let source = self.s.bundles.source(&name).map(|s| s.as_str().to_owned()).unwrap_or_default();
                let description =
                    match self.s.bundles.load(&name).ok().and_then(|b| b.get("description").cloned()) {
                        Some(Value::String(d)) => d,
                        _ => String::new(),
                    };
                ProfileInfo { name, source, description }
            })
            .collect()
    }

    /// Reads one profile.
    pub fn profile(&self, name: &str) -> ApiResult<Bundle> {
        Ok(self.s.bundles.load(name)?)
    }

    /// A profile as the TOML text of its file.
    pub fn profile_toml(bundle: &Bundle) -> String {
        bundle.to_string()
    }

    /// Says in one line what a profile changes (empty when it cannot be read).
    pub fn describe_profile(&self, name: &str) -> String {
        self.s.bundles.load(name).map(|b| describe(&b).join("  \u{b7}  ")).unwrap_or_default()
    }

    /// Applies what the profile lists and nothing else, and answers the summary line.
    pub fn apply_profile(&self, name: &str) -> ApiResult<String> {
        Ok(self.s.bundles.apply(name)?)
    }

    /// Applies the profile after the last one applied.
    pub fn next_profile(&self) -> ApiResult<String> {
        let name = self.s.bundles.next_name()?;
        Ok(self.s.bundles.apply(&name)?)
    }

    /// Saves the current state as a profile and answers the file written.
    pub fn save_profile(&self, name: &str, description: &str) -> ApiResult<PathBuf> {
        if name.is_empty() {
            return Err(ApiError::invalid("profile names: letters, digits, - and _"));
        }
        let bundle = self.s.bundles.capture(description)?;
        Ok(self.s.bundles.save(name, &bundle)?)
    }

    /// Removes one of your profiles.
    pub fn delete_profile(&self, name: &str) -> ApiResult<()> {
        Ok(self.s.bundles.delete(name)?)
    }

    /// The whole-setup profile applied last; `None` when none.
    pub fn current_profile(&self) -> Option<String> {
        self.s.user.get_as::<String>(keys::PROFILE).filter(|p| !p.is_empty())
    }
}
