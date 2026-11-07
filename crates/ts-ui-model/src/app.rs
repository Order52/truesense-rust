//! The whole Qt-free application state in one object: the command bus, the live store, the page
//! registry, the app's own settings and the capability model. The Qt bridge holds one
//! `Arc<AppModel>` and forwards to it; everything here is testable over a fake [`Commands`].
#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::{Value, json};
use ts_gui::gui::{ApiCommands, Commands};

use crate::bus::Bus;
use crate::capability::Capabilities;
use crate::hooks::window_hooks;
use crate::live::{HISTORY_LEN, LiveStore};
use crate::routes::Routes;
use crate::settings::{self, UiStore};

/// Everything the UI shell shares.
pub struct AppModel {
    commands: Arc<dyn Commands>,
    /// Asynchronous commands and writes.
    pub bus: Bus,
    /// The newest live frame and the sparkline histories.
    pub live: LiveStore,
    /// The pages of the app.
    pub routes: Routes,
    ui: Mutex<UiStore>,
    caps: Mutex<Capabilities>,
}

fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl AppModel {
    /// An app over `commands`, with its settings in `ui_path` and the page registry in
    /// `routes_json` (`qml/pages/registry.json`).
    ///
    /// # Errors
    /// The text of the problem: an invalid registry or a thread that could not start.
    pub fn new(
        commands: Arc<dyn Commands>,
        ui_path: Option<PathBuf>,
        routes_json: &str,
    ) -> Result<Self, String> {
        let routes = Routes::parse(routes_json).map_err(|e| e.to_string())?;
        let bus = Bus::new(commands.clone()).map_err(|e| format!("cannot start the command bus: {e}"))?;
        Ok(Self {
            commands,
            bus,
            live: LiveStore::new(HISTORY_LEN),
            routes,
            ui: Mutex::new(UiStore::open(ui_path)),
            caps: Mutex::new(Capabilities::default()),
        })
    }

    /// The app over the real machine (or the firmware simulator when `TRUESENSE_SIM=1`): the
    /// in-process command bus of the window, exactly as `truesense gui` builds it.
    ///
    /// # Errors
    /// The text of the problem: the application layer could not be built.
    pub fn system(routes_json: &str) -> Result<Self, String> {
        let api = ts_api::adapt::system_api().map_err(|e| e.to_string())?;
        let api = Arc::new(api);
        let commands = Arc::new(ApiCommands::new(api.clone(), window_hooks(&api)));
        let ui_path = settings::default_path_from(&ts_foundation::fsx::env_nonempty);
        Self::new(commands, ui_path, routes_json)
    }

    /// One live reading, folded into the store. Blocks for the hardware reads: call it from the
    /// poller thread. Returns the snapshot JSON (see [`LiveStore::snapshot`]) when a reading was
    /// accepted, `None` when the machine could not be read or a newer reading already won.
    pub fn poll_live(&self) -> Option<String> {
        let ticket = self.live.ticket();
        let frame = self.bus.live()?;
        self.live.push(ticket, frame).then(|| self.live.snapshot().to_string())
    }

    /// Reads `status get` (and `capabilities get` where the daemon has it) and rebuilds the
    /// capability model; returns its JSON. Blocks: call it on a worker thread.
    pub fn refresh_capabilities(&self) -> String {
        let status = self.commands.run("status", "get", &json!({})).unwrap_or(Value::Null);
        let mut caps = Capabilities::from_status(&status);
        // `capabilities get` arrives with a later backend; until then the command is "unknown"
        // and the derivation from `status get` stands.
        if let Ok(list) = self.commands.run("capabilities", "get", &json!({})) {
            caps = caps.overlaid_with(Capabilities::from_list(&list));
        }
        let json = caps.to_json().to_string();
        *locked(&self.caps) = caps;
        json
    }

    /// The capability model as last refreshed (`{}` before the first refresh).
    pub fn capabilities_json(&self) -> String {
        locked(&self.caps).to_json().to_string()
    }

    /// The settings for QML: every setting plus `problem` (why the file is unusable, or `null`) and
    /// the window size to open with.
    pub fn ui_json(&self) -> String {
        let ui = locked(&self.ui);
        let mut map = ui.settings().to_map();
        let (w, h) = ui.settings().window_size();
        map.insert("window_width".to_owned(), json!(w));
        map.insert("window_height".to_owned(), json!(h));
        map.insert("problem".to_owned(), json!(ui.problem()));
        Value::Object(map).to_string()
    }

    /// Changes one setting (`theme`, `language`, `minimise_to_tray`, `font_scale`, `reduced_motion`).
    ///
    /// # Errors
    /// The message to show: a bad value, or the settings file could not be written (the value is
    /// still in effect for this run).
    pub fn set_ui(&self, key: &str, value: &Value) -> Result<(), String> {
        locked(&self.ui).set(key, value).map_err(|e| e.to_string())
    }

    /// Remembers the window geometry.
    ///
    /// # Errors
    /// The message to show when the file could not be written.
    pub fn set_window(&self, size: (u32, u32), pos: Option<(i32, i32)>) -> Result<(), String> {
        locked(&self.ui).set_window(size, pos).map_err(|e| e.to_string())
    }

    /// The language to load now, from the setting and the desktop's locale name.
    pub fn language(&self, system_locale: &str) -> String {
        settings::resolve_language(&locked(&self.ui).settings().language, system_locale).to_owned()
    }

    /// The saved settings, for decisions Rust makes (the start-up size, the tray flag).
    pub fn settings(&self) -> settings::UiSettings {
        locked(&self.ui).settings().clone()
    }

    /// The problems found in the settings file at start (unreadable file, dropped entries), for the log.
    pub fn settings_problems(&self) -> Vec<String> {
        let ui = locked(&self.ui);
        ui.problem()
            .map(|p| format!("ui.json: {p}"))
            .into_iter()
            .chain(ui.dropped().iter().map(|d| format!("ui.json: ignored entry {d}")))
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use ts_gui::gui::CommandError;

    const REGISTRY: &str = include_str!("../../ts-qt/qml/pages/registry.json");

    struct Fake {
        daemon: bool,
    }

    impl Commands for Fake {
        fn run(&self, area: &str, verb: &str, _args: &Value) -> Result<Value, CommandError> {
            match (area, verb) {
                ("status", "get") => Ok(json!({
                    "power_mode": "turbo", "sensors": {"cpu_temp": 61, "cpu_fan_rpm": 3000}, "rear": null,
                    "daemon": if self.daemon { json!({"live": {}}) } else { json!("ERROR: not running") },
                })),
                _ => Err(CommandError::unknown(area, verb)),
            }
        }
        fn live(&self) -> Option<Value> {
            Some(json!({"sensors": {"cpu_temp": 61}}))
        }
    }

    fn model(daemon: bool) -> AppModel {
        AppModel::new(Arc::new(Fake { daemon }), None, REGISTRY).unwrap()
    }

    #[test]
    fn polling_folds_a_frame_into_the_store_and_returns_the_snapshot() {
        let m = model(true);
        let snap: Value = serde_json::from_str(&m.poll_live().unwrap()).unwrap();
        assert_eq!(snap["frame"]["sensors"]["cpu_temp"], json!(61));
        assert_eq!(snap["history"]["sensors.cpu_temp"], json!([61.0]));
        m.poll_live().unwrap();
        assert_eq!(m.live.accepted(), 2);
    }

    #[test]
    fn capabilities_are_derived_from_status_and_unknown_commands_are_ignored() {
        let up: Value = serde_json::from_str(&model(true).refresh_capabilities()).unwrap();
        assert_eq!(up["rear-light"]["state"], json!("unavailable"));
        assert_eq!(up["fans"]["state"], json!("available"));
        assert_eq!(up["undervolt"]["state"], json!("available"));
        let m = model(false);
        assert_eq!(m.capabilities_json(), "{}");
        let down: Value = serde_json::from_str(&m.refresh_capabilities()).unwrap();
        assert_eq!(down["undervolt"]["state"], json!("needs-daemon"));
        assert_eq!(serde_json::from_str::<Value>(&m.capabilities_json()).unwrap(), down);
    }

    #[test]
    fn an_invalid_registry_is_refused_with_its_reason() {
        let err = AppModel::new(Arc::new(Fake { daemon: true }), None, "{}").err().unwrap();
        assert!(err.contains("page registry"), "{err}");
    }

    #[test]
    fn settings_round_trip_through_json_and_report_problems() {
        let m = model(true);
        let ui: Value = serde_json::from_str(&m.ui_json()).unwrap();
        assert_eq!(ui["theme"], json!("dark"));
        assert_eq!(ui["window_width"], json!(1000));
        assert_eq!(ui["problem"], Value::Null);
        m.set_ui("theme", &json!("light")).unwrap();
        assert!(m.set_ui("theme", &json!("x")).is_err());
        assert_eq!(m.language("ar_SA"), "en", "no Arabic catalogue any more");
        assert!(m.settings_problems().is_empty());
        m.set_window((1111, 700), None).unwrap();
        assert_eq!(m.settings().window_size(), (1111, 700));
    }
}
