//! Desktop alerts in the root daemon: a hot CPU or GPU, a low battery, a clock held down by the
//! thermal governor, a lost sensor. The rules are `ts_decision::alerts`; this file feeds them
//! and shows what they decide.
//!
//! They live here, not in the window, so they work with every window closed. Three rules keep
//! them from ever getting in the way of the machine:
//!
//! * they use what the tick already has (the smoothed temperatures, the charger, the governor);
//!   only the battery level is read, and then only every few ticks and only on battery;
//! * a message is shown by a short-lived helper thread, never in the tick: the on-screen display
//!   runs `hyprctl` and `notify-send` as the desktop user and may wait seconds for a helper that
//!   hangs, and a panicking notifier is caught there. A failing or missing notifier costs a
//!   journal line and nothing else;
//! * the rules themselves run under `catch_unwind`, so a bug in them cannot turn into the
//!   emergency "fans to 100 %" fallback of a failed tick.
//!
//! The settings are the `[alerts]` section of the configuration. The daemon used to read its
//! configuration once, at start; for this section it looks at the files' modification times every
//! [`RELOAD_EVERY_S`] seconds and loads them again when they changed, so `truesense alerts set`
//! takes effect within about ten seconds without a restart. A file that no longer parses keeps the
//! settings in force (and says so in the journal).
//!
//! The desktop user's file is `~/.config/truesense/config.toml` of the configured user: the daemon
//! runs as root with no `HOME` of its own, so the user's file cannot be found from the
//! environment.
//!
//! The on-screen display uses a normal urgency. There is no way to ask for a critical one: a
//! desktop never dismisses those, and a temperature that stays high would leave a permanent banner.

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

use ts_decision::alerts::{Alert, Evaluator, Event, Sample, Settings};
use ts_foundation::config::{self, AlertsConfig, Config};
use ts_foundation::sync::locked;
use ts_system::osd::UserDirectory;

use super::daemon::{Daemon, Inner};
use super::step::panic_text;

/// The configuration files are looked at this often (monotonic seconds): two `stat` calls.
const RELOAD_EVERY_S: f64 = 10.0;
/// The battery level is read every this many ticks while on battery.
const BATTERY_EVERY_TICKS: u64 = 5;

/// Where the daemon re-reads the `[alerts]` settings from.
pub trait AlertSettings: Send + Sync {
    /// A cheap fingerprint of the source: it differs from the previous answer whenever the
    /// settings may have changed. Called every few seconds from the tick.
    fn stamp(&self) -> u64;
    /// Loads the settings now. The error text says what is wrong (a file that does not parse, a
    /// value out of range); the daemon then keeps the settings it has.
    fn load(&self) -> Result<AlertsConfig, String>;
}

/// The settings from the configuration files: the machine's and the desktop user's.
#[derive(Debug, Clone)]
pub struct ConfigFileAlerts {
    system: Option<PathBuf>,
    user: Option<PathBuf>,
}

impl ConfigFileAlerts {
    /// The settings in these two files (either may be absent).
    pub fn new(system: Option<PathBuf>, user: Option<PathBuf>) -> Self {
        Self { system, user }
    }

    /// The machine's file and the file of `user` found through `users`. `TRUESENSE_CONFIG` in
    /// `env` names the user's file when set, as everywhere else; a user the directory does not
    /// know falls back to what `HOME` says (usually nothing, as a service).
    pub fn for_desktop_user(
        system: &Path,
        users: &dyn UserDirectory,
        user: &str,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Self {
        let user_file = match env("TRUESENSE_CONFIG") {
            Some(path) => Some(PathBuf::from(path)),
            None => match users.lookup(user) {
                Some(entry) => Some(PathBuf::from(entry.home).join(".config/truesense/config.toml")),
                None => config::user_path_from(env),
            },
        };
        Self::new(Some(system.to_path_buf()), user_file)
    }
}

impl AlertSettings for ConfigFileAlerts {
    fn stamp(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        for path in [&self.system, &self.user].into_iter().flatten() {
            // A missing file hashes as "missing", so creating or deleting one changes the stamp.
            let seen = fs::metadata(path).ok().map(|m| (m.modified().ok(), m.len()));
            seen.hash(&mut hasher);
        }
        hasher.finish()
    }

    fn load(&self) -> Result<AlertsConfig, String> {
        let loaded = Config::load(self.system.as_deref(), self.user.as_deref(), &|_| None)
            .map_err(|e| e.to_string())?;
        let problems = loaded.alerts.validate();
        if problems.is_empty() { Ok(loaded.alerts) } else { Err(problems.join("; ")) }
    }
}

/// What a tick saw, for the rules.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Observed {
    /// The smoothed CPU temperature; `None` when no sensor answered.
    pub(super) cpu_c: Option<f64>,
    /// The smoothed GPU temperature; `None` when the GPU gave no reading.
    pub(super) gpu_c: Option<f64>,
}

impl Observed {
    /// The tick found no temperature at all.
    pub(super) const LOST: Observed = Observed { cpu_c: None, gpu_c: None };
}

/// The rules, their settings source and what they have decided to say. Lives in `Inner`.
pub(super) struct AlertsRuntime {
    source: Arc<dyn AlertSettings>,
    eval: Evaluator,
    /// The source's stamp when the settings were last loaded; `None` until the first look.
    stamp: Option<u64>,
    /// The monotonic time of the next look at the source.
    next_check: f64,
    battery_pct: f64,
    /// The tick the battery was last read on; `None` before the first read.
    battery_tick: Option<u64>,
    /// Messages decided under the lock, shown after it is released.
    pending: Vec<Alert>,
}

impl AlertsRuntime {
    /// Rules with the settings the daemon started with. The first tick loads the source anyway:
    /// the configuration the daemon started with may not have included the desktop user's file
    /// (a service has no `HOME`), which this source does know.
    pub(super) fn new(source: Arc<dyn AlertSettings>, initial: &AlertsConfig) -> Self {
        Self {
            source,
            eval: Evaluator::new(Settings::from(initial)),
            stamp: None,
            next_check: 0.0,
            battery_pct: 100.0,
            battery_tick: None,
            pending: Vec::new(),
        }
    }
}

impl Daemon {
    /// Feeds one tick's readings to the rules. Never fails and never panics out: whatever goes
    /// wrong here is a journal line.
    pub(super) fn alerts_observe(&self, g: &mut Inner, mono: f64, seen: Observed) {
        let ran = catch_unwind(AssertUnwindSafe(|| self.alerts_evaluate(g, mono, seen)));
        if let Err(payload) = ran {
            self.warn_every(
                "alerts-panic",
                60.0,
                format!("the alert rules failed: {}", panic_text(payload.as_ref())),
            );
        }
    }

    fn alerts_evaluate(&self, g: &mut Inner, mono: f64, seen: Observed) {
        self.alerts_reload(g, mono);
        if !g.alerts.eval.settings().enabled {
            return;
        }
        let on_ac = g.last_ac.unwrap_or(true);
        if !on_ac && g.alerts.eval.settings().battery_low_pct > 0.0 {
            let due = g.alerts.battery_tick.is_none_or(|t| g.tick.saturating_sub(t) >= BATTERY_EVERY_TICKS);
            if due {
                g.alerts.battery_pct = self.sensors.battery_pct();
                g.alerts.battery_tick = Some(g.tick);
            }
        }
        let sample = Sample {
            cpu_c: seen.cpu_c,
            gpu_c: seen.gpu_c,
            battery_pct: g.alerts.battery_pct,
            on_ac,
            throttled: g.governor.capped(),
        };
        for event in g.alerts.eval.step(mono, &sample) {
            match event {
                // Logged once per episode, whether or not it is shown (it may have to wait).
                Event::Began(a) => {
                    self.info(format!("alert {} ({}): {}", a.id.as_str(), a.severity.as_str(), a.message))
                }
                Event::Cleared(id) => self.info(format!("alert {} is over", id.as_str())),
                Event::Notify(a) => g.alerts.pending.push(a),
            }
        }
    }

    /// Loads the settings again when the files changed, at most every [`RELOAD_EVERY_S`].
    fn alerts_reload(&self, g: &mut Inner, mono: f64) {
        if mono < g.alerts.next_check {
            return;
        }
        g.alerts.next_check = mono + RELOAD_EVERY_S;
        let stamp = g.alerts.source.stamp();
        if g.alerts.stamp == Some(stamp) {
            return;
        }
        // Remembered even when the load fails: a broken file is reported once per change, not
        // every ten seconds.
        g.alerts.stamp = Some(stamp);
        match g.alerts.source.load() {
            Ok(loaded) => {
                let settings = Settings::from(&loaded);
                if settings != *g.alerts.eval.settings() {
                    g.alerts.eval.set_settings(settings);
                    self.info("alert settings reloaded");
                }
            }
            Err(why) => self.warn(format!("alert settings not reloaded, keeping the current ones: {why}")),
        }
    }

    /// Shows the messages the last tick decided on, from a helper thread. Called with the lock
    /// released.
    pub(super) fn alerts_deliver(&self) {
        let batch = std::mem::take(&mut self.lock().alerts.pending);
        if batch.is_empty() {
            return;
        }
        let Some(me) = self.me.upgrade() else { return };
        let spawned = thread::Builder::new().name("alerts".to_owned()).spawn(move || me.alerts_show(&batch));
        match spawned {
            Ok(handle) => {
                let mut running = locked(&self.background);
                // Alerts come all through the daemon's life: do not keep the finished ones.
                running.retain(|h| !h.is_finished());
                running.push(handle);
            }
            Err(e) => {
                self.warn_every("alerts-thread", 60.0, format!("could not start the alert thread: {e}"))
            }
        }
    }

    fn alerts_show(&self, batch: &[Alert]) {
        for alert in batch {
            if self.stop.is_requested() {
                return;
            }
            let shown = catch_unwind(AssertUnwindSafe(|| self.osd.show(&alert.message, alert.id.icon())));
            if let Err(payload) = shown {
                self.warn_every(
                    "alerts-notify",
                    60.0,
                    format!(
                        "the notifier failed for alert {}: {}",
                        alert.id.as_str(),
                        panic_text(payload.as_ref())
                    ),
                );
            }
        }
    }
}
