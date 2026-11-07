//! The seams where the application layer meets things only the assembly layer can build.
//!
//! Each trait here names what the application needs, no more; the real implementation is a thin
//! impl over the domain or system type that already exists (so the assembly layer passes the real
//! object), and tests use the doubles of [`testing`](super::testing).
#![forbid(unsafe_code)]

use std::path::PathBuf;

use std::sync::Arc;

use ts_domain::actions::HostResult;
use ts_domain::profiles::{Bundle, BundleError, BundleSource, BundleStore};
use ts_foundation::userstate;
use ts_platform::hw::RearMemory;
use ts_system::osd::Osd;
use ts_system::services::{Manager, Step, Unit};
use ts_system::update::{ApplyOptions, ApplyResult, Status as UpdateStatus, UpdateError, Updater};

/// Shows a short on-screen message. The switch of the sound output takes about a second, so its
/// message comes first and needs a way to be shown from here.
pub trait Notifier: Send + Sync {
    /// Shows `text` with an icon name.
    fn show(&self, text: &str, icon: &str);
}

impl Notifier for Osd {
    fn show(&self, text: &str, icon: &str) {
        Osd::show(self, text, icon);
    }
}

/// A notifier that shows nothing (a process without a desktop session).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoNotifier;

impl Notifier for NoNotifier {
    fn show(&self, _text: &str, _icon: &str) {}
}

/// The whole-setup profiles (power, fans, lighting, battery and audio in one go). The real
/// implementation is [`BundleStore`], which the assembly layer builds from the same facades as
/// the action runner; the indirection keeps this layer testable without those facades.
pub trait ProfileBundles: Send + Sync {
    /// The profile names, shipped and yours.
    fn names(&self) -> Vec<String>;
    /// Where a profile comes from.
    fn source(&self, name: &str) -> Option<BundleSource>;
    /// Reads one profile.
    fn load(&self, name: &str) -> Result<Bundle, BundleError>;
    /// Applies what the profile lists and nothing else; answers the summary line.
    fn apply(&self, name: &str) -> Result<String, BundleError>;
    /// The profile after the last one applied.
    fn next_name(&self) -> Result<String, BundleError>;
    /// Records the current setup as a profile body.
    fn capture(&self, description: &str) -> HostResult<Bundle>;
    /// Saves a profile and answers the file written.
    fn save(&self, name: &str, bundle: &Bundle) -> Result<PathBuf, BundleError>;
    /// Removes one of your profiles.
    fn delete(&self, name: &str) -> Result<(), BundleError>;
}

impl ProfileBundles for BundleStore {
    fn names(&self) -> Vec<String> {
        self.files().names()
    }

    fn source(&self, name: &str) -> Option<BundleSource> {
        self.files().source(name)
    }

    fn load(&self, name: &str) -> Result<Bundle, BundleError> {
        self.files().load(name)
    }

    fn apply(&self, name: &str) -> Result<String, BundleError> {
        BundleStore::apply(self, name)
    }

    fn next_name(&self) -> Result<String, BundleError> {
        BundleStore::next_name(self)
    }

    fn capture(&self, description: &str) -> HostResult<Bundle> {
        BundleStore::capture(self, description)
    }

    fn save(&self, name: &str, bundle: &Bundle) -> Result<PathBuf, BundleError> {
        self.files().save(name, bundle)
    }

    fn delete(&self, name: &str) -> Result<(), BundleError> {
        self.files().delete(name)
    }
}

/// Finding and installing releases. [`Updater`] is the real one; it needs the repository from the
/// system configuration, which the assembly layer reads.
pub trait UpdateService: Send + Sync {
    /// The repository releases come from.
    fn repo(&self) -> String;
    /// Whether a newer release exists; `hours` of `None` is the updater's own setting.
    fn check(&self, force: bool, hours: Option<u64>) -> UpdateStatus;
    /// Downloads, verifies and installs a release.
    fn apply(&self, options: &ApplyOptions) -> Result<ApplyResult, UpdateError>;
    /// The version the user was last told about.
    fn notified(&self) -> Option<String>;
    /// Remembers that the user was told about `version`.
    fn mark_notified(&self, version: &str);
}

impl UpdateService for Updater {
    fn repo(&self) -> String {
        Updater::repo(self).to_owned()
    }

    fn check(&self, force: bool, hours: Option<u64>) -> UpdateStatus {
        Updater::check(self, force, hours)
    }

    fn apply(&self, options: &ApplyOptions) -> Result<ApplyResult, UpdateError> {
        Updater::apply(self, options)
    }

    fn notified(&self) -> Option<String> {
        Updater::notified(self)
    }

    fn mark_notified(&self, version: &str) {
        Updater::mark_notified(self, version);
    }
}

/// The unit check and repair of the services ([`Manager`] is the real one).
pub trait ServiceChecks: Send + Sync {
    /// The state of every unit, the kernel module and the control socket (read-only).
    fn check(&self) -> Vec<Unit>;
    /// The repair that needs root.
    fn repair_system(&self) -> Vec<Step>;
    /// The repair of the user's own services.
    fn repair_user(&self) -> Vec<Step>;
}

impl ServiceChecks for Manager {
    fn check(&self) -> Vec<Unit> {
        Manager::check(self)
    }

    fn repair_system(&self) -> Vec<Step> {
        Manager::repair_system(self)
    }

    fn repair_user(&self) -> Vec<Step> {
        Manager::repair_user(self)
    }
}

/// Where the rear light remembers its colours: the user state (`rear_state`, `rear_zones`), so the
/// window can show the three part colours again and `rear on` can bring them back.
#[derive(Debug, Clone)]
pub struct RearMemoryInUserState(pub Arc<userstate::Store>);

impl RearMemory for RearMemoryInUserState {
    fn remember(&self, key: &str, value: serde_json::Value) {
        // A convenience file: failing to write it must never fail the light.
        let _ = self.0.set(key, value);
    }
}
