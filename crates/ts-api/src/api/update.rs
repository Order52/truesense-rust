//! Updates: look for a newer release and install it.
//!
//! Looking sends one request for the public release number. Installing downloads a version tag of
//! the repository named in the *system* configuration (never the user's), verifies it against the
//! release's `SHA256SUMS`, and runs its installer as root; all of that is `ts_system::update`'s.
#![forbid(unsafe_code)]

use ts_foundation::config::{Value, section_table};
use ts_system::update::{ApplyOptions, ApplyResult, Status};

use super::{Api, ApiError, ApiResult};

/// What `update apply` is about to install.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdatePlan {
    /// The answer of the forced check.
    pub status: Status,
    /// The release to install (`v0.5.1`).
    pub tag: String,
    /// False when you already have the newest version and none was asked for.
    pub needed: bool,
}

/// The line `update check` prints.
pub fn describe_update(status: &Status) -> String {
    let mut line = if status.available {
        format!(
            "TrueSense {}: update {} is available   (truesense update apply)",
            status.current,
            status.latest.as_deref().unwrap_or("?")
        )
    } else {
        let newest = status.latest.as_deref().map(|l| format!(" (newest {l})")).unwrap_or_default();
        format!("TrueSense {} is up to date{newest}", status.current)
    };
    if let Some(e) = &status.error {
        line.push_str(&format!("   [{e}]"));
    }
    line
}

/// The line `update apply` prints once the installer finished.
pub fn describe_apply(result: &ApplyResult) -> String {
    if result.returncode != 0 {
        return format!(
            "the installer exited with code {} (see /var/log/truesense-install.log)",
            result.returncode
        );
    }
    let checks = if result.verified == Some(true) {
        "checksum matches the release"
    } else {
        "this release predates published checksums"
    };
    let sha: String = result.sha256.chars().take(16).collect();
    format!(
        "updated to {} (download sha256 {sha}\u{2026}, {checks}); restart the window and the key service if they are open",
        result.version
    )
}

impl Api {
    /// The repository updates come from.
    pub fn update_repo(&self) -> String {
        self.s.updates.repo()
    }

    /// Asks GitHub for the newest release (once a day unless `force`). A failed lookup is in the
    /// status, not an error: the answer then carries what was cached.
    pub fn check_update(&self, force: bool) -> Status {
        let hours = u64::try_from(self.cfg().update.check_hours).ok();
        self.s.updates.check(force, hours)
    }

    /// Looks up the newest release and decides what to install: `version` (like `0.3.1`) picks a
    /// specific release instead of the newest.
    pub fn plan_update(&self, version: Option<&str>) -> ApiResult<UpdatePlan> {
        let status = self.check_update(true);
        if let (Some(error), None) = (&status.error, &status.info) {
            return Err(ApiError::invalid(error.clone()));
        }
        if let Some(v) = version.filter(|v| !v.is_empty()) {
            return Ok(UpdatePlan { tag: format!("v{}", v.trim_start_matches('v')), needed: true, status });
        }
        let tag = status
            .info
            .as_ref()
            .map(|i| i.tag.clone())
            .ok_or_else(|| ApiError::invalid("there is no release to install"))?;
        Ok(UpdatePlan { needed: status.available, tag, status })
    }

    /// Downloads a release, verifies it against its `SHA256SUMS` and runs its installer as root.
    /// The result is returned even when the installer failed: look at `returncode`.
    pub fn apply_update(&self, tag: Option<&str>) -> ApiResult<ApplyResult> {
        let options = ApplyOptions { tag: tag.map(str::to_owned), ..ApplyOptions::default() };
        Ok(self.s.updates.apply(&options)?)
    }

    /// Whether TrueSense looks for updates by itself once a day.
    pub fn auto_update_check(&self) -> ApiResult<bool> {
        Ok(self.load_config()?.update.auto_check)
    }

    /// Switches the daily check (a desktop notification; nothing is installed).
    pub fn set_auto_update_check(&self, on: bool) -> ApiResult<bool> {
        self.s.config.write_user(section_table("update", [("auto_check", Value::Boolean(on))]))?;
        Ok(on)
    }

    /// What the daily user timer runs: when automatic checks are on and a newer release exists that
    /// the desktop was not told about yet, shows one notification. Answers the version announced
    /// (`None` when nothing was).
    pub fn notify_new_update(&self) -> ApiResult<Option<String>> {
        if !self.cfg().update.auto_check {
            return Ok(None);
        }
        let status = self.check_update(false);
        let Some(latest) = status.latest.filter(|_| status.available) else { return Ok(None) };
        if self.s.updates.notified().as_deref() == Some(latest.as_str()) {
            return Ok(None);
        }
        self.s.notifier.show(
            &format!("TrueSense {latest} is available (truesense update apply, or Settings in the window)"),
            "update",
        );
        self.s.updates.mark_notified(&latest);
        Ok(Some(latest))
    }
}
