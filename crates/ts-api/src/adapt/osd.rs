//! The on-screen display for a user's client, over `ts_system::osd::Osd`.
//!
//! The Python edition read `[osd] style` and `[profile]`'s user from the configuration on every
//! message, so editing the file changed the next message without restarting the key service. The
//! system layer's `Osd` is immutable, so this adapter keeps one base `Osd` (its clones share the
//! counter of the nested silenced sections) and gives every message a clone with the style and
//! user of the configuration as it is now.
#![forbid(unsafe_code)]

use std::time::Duration;

use ts_domain::audio::{Osd as AudioOsd, Quiet};
use ts_system::osd::{DEFAULT_SETTLE, Osd};

use crate::api::{ConfigFiles, Notifier};

/// Shows messages for both consumers that need them: the application layer's [`Notifier`] and the
/// sound system's `ts_domain::audio::Osd`.
#[derive(Clone)]
pub(crate) struct LiveOsd {
    base: Osd,
    config: ConfigFiles,
    settle: Duration,
}

impl LiveOsd {
    /// A display that takes its style and user from `config` for every message.
    pub(crate) fn new(base: Osd, config: ConfigFiles) -> Self {
        Self { base, config, settle: DEFAULT_SETTLE }
    }

    /// The base display with today's configuration.
    fn current(&self) -> Osd {
        let cfg = self.config.load_or_default();
        self.base.clone().with_style(&cfg.osd.style).with_user(&cfg.user)
    }
}

impl Notifier for LiveOsd {
    fn show(&self, text: &str, icon: &str) {
        // Whether a helper took the message does not matter to the caller: a missing notifier
        // is never an error.
        let _ = self.current().show(text, icon);
    }
}

impl AudioOsd for LiveOsd {
    fn show(&self, text: &str, icon: &str) {
        let _ = self.current().show(text, icon);
    }

    fn quiet(&self) -> Quiet {
        let guard = self.current().quiet(self.settle);
        Quiet::new(move || drop(guard))
    }
}
