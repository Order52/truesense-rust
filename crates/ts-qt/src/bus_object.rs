//! The one QObject the QML side talks to: the `Bus` singleton.
//!
//! It is a thin skin over [`ts_ui_model::app::AppModel`]. Rules it keeps: nothing blocks the GUI
//! thread (commands run on worker threads and come back as `commandFinished` signals, queued onto
//! the GUI thread); live values arrive about once a second **only while the window is active**
//! (`setActive`); QML never sees Rust types, only JSON text and plain values.
//!
//! QML does not use it directly in pages: `Api`, `Live`, `Caps`, `Ui` and `Nav` (qml/*.qml) wrap it,
//! so only `Main.qml` mentions `Bus` and the component tests run without Rust.

// The bridge macro generates public items (the QObject, its signals) that cannot carry doc comments.
#![allow(missing_docs)]
// SAFETY (for the whole module): the only `unsafe` here is what the `cxx_qt::bridge` macro
// requires and generates: `unsafe extern "C++"` declarations of Qt types and of the shims in
// cpp/shim.cpp, and the `unsafe fn` signatures of the QObject's invokables (they take a
// `Pin<&mut Bus>`). No pointer is dereferenced by hand; the shims take and return values, and
// the QObject is only touched from the GUI thread or through queued signals. The end-to-end test
// (tests/app.rs) runs every one of them under the offscreen platform.
#![allow(unsafe_code)]

use core::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use serde_json::{Value, json};
use ts_ui_model::app::AppModel;
use ts_ui_model::bus::{CODE_BAD_ARGS, CmdError, Done, Reply, Request};
use ts_ui_model::json::parse_args;
use ts_ui_model::poller::{Poke, Poller};
use ts_ui_model::single::{Primary, Server};

/// The tick of the live values.
pub const LIVE_INTERVAL: Duration = Duration::from_secs(1);

/// Switches that exist for the tests only (set through environment variables, never by a person).
#[derive(Debug, Clone, Default)]
pub struct TestConfig {
    /// `TRUESENSE_QT_TEST=1`: the shell runs its test hook (report, optional screenshot, quit).
    pub enabled: bool,
    /// `TRUESENSE_QT_EXIT_AFTER_MS`: how long after the first frame the hook reports and quits.
    pub exit_after_ms: i32,
    /// `TRUESENSE_QT_SHOT`: where the hook saves a PNG of the window.
    pub shot: Option<String>,
    /// `TRUESENSE_QT_FPS=1`: the shell prints `REPORT {"fps": n}` (frames presented in the last second) once a
    /// second, for `tools/qtprofile`; nothing runs when it is unset.
    pub fps: bool,
    /// `TRUESENSE_QT_SCROLL=1`: the shell scrolls the page up and down by itself (measuring).
    pub scroll: bool,
    /// `TRUESENSE_QT_BENCH=1` (or a comma list of phases: `pages,tabs,actions`): the shell runs the
    /// responsiveness script of `qml/BenchHook.qml` (page switches, tabs, actions), prints one
    /// `BENCH {json}` line per measurement and quits (`tools/qtprofile/qtprofile.py respond`).
    pub bench: Option<String>,
}

impl TestConfig {
    /// Reads the environment.
    pub fn from_env() -> Self {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        Self {
            enabled: get("TRUESENSE_QT_TEST").is_some(),
            exit_after_ms: get("TRUESENSE_QT_EXIT_AFTER_MS").and_then(|v| v.parse().ok()).unwrap_or(0),
            shot: get("TRUESENSE_QT_SHOT"),
            fps: get("TRUESENSE_QT_FPS").is_some(),
            scroll: get("TRUESENSE_QT_SCROLL").is_some(),
            bench: get("TRUESENSE_QT_BENCH"),
        }
    }
}

/// Everything the Bus needs that exists before Qt does; installed once by `lib::run`.
pub struct Shared {
    /// The Qt-free application state.
    pub model: AppModel,
    /// The page the window opens on (already resolved), or `home`.
    pub start_page: String,
    /// Start without showing the window (autostart).
    pub start_hidden: bool,
    /// Test switches.
    pub test: TestConfig,
    /// Process start, for the first-frame measurement.
    pub started: Instant,
    /// The single-instance socket, until the Bus takes it over to serve it.
    pub primary: Mutex<Option<Primary>>,
    /// Milliseconds from `started` to named moments of the start (`model`: the application layer
    /// is built; `qml`: the shell's QML is complete), for `BenchHook.qml`'s start line.
    pub marks: Mutex<Vec<(&'static str, f64)>>,
}

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();
/// View-model calls and the time they took on the GUI thread (nanoseconds), since the last
/// `BENCH` line; read only by the responsiveness script.
static VM_CALLS: AtomicU64 = AtomicU64::new(0);
static VM_NANOS: AtomicU64 = AtomicU64::new(0);
/// Live ticks and the time they took on the poller thread (nanoseconds), likewise.
static TICKS: AtomicU64 = AtomicU64::new(0);
static TICK_NANOS: AtomicU64 = AtomicU64::new(0);

fn add_nanos(counter: &AtomicU64, since: Instant) {
    counter.fetch_add(u64::try_from(since.elapsed().as_nanos()).unwrap_or(u64::MAX), Ordering::Relaxed);
}
static RENDERER: OnceLock<String> = OnceLock::new();

/// Records which scene-graph backend was chosen (reported by the test hook and `tools/qtprofile`).
pub fn set_renderer(name: String) {
    let _ = RENDERER.set(name);
}

/// Records a start-up moment (see [`Shared::marks`]).
pub fn mark(shared: &Shared, name: &'static str) {
    let ms = shared.started.elapsed().as_secs_f64() * 1000.0;
    if let Ok(mut marks) = shared.marks.lock() {
        marks.push((name, ms));
    }
}

/// Hands the shared state to the Bus before the QML engine creates it (set once per process).
pub fn install(shared: Arc<Shared>) -> bool {
    SHARED.set(shared).is_ok()
}

/// The shared state, for the rest of the crate.
pub fn shared() -> Option<&'static Arc<Shared>> {
    SHARED.get()
}

/// The cxx-qt bridge: the `Bus` QObject and the shim declarations.
#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qqmlapplicationengine.h");
        type QQmlApplicationEngine = cxx_qt_lib::QQmlApplicationEngine;
    }

    // The Qt calls cxx-qt-lib has no wrapper for (cpp/shim.cpp).
    unsafe extern "C++" {
        include!("shim.h");

        #[namespace = "truesense"]
        fn set_identity(app_id: &QString, display_name: &QString, icon_path: &QString);
        #[namespace = "truesense"]
        fn set_language(code: &QString) -> bool;
        #[namespace = "truesense"]
        fn system_locale_name() -> QString;
        #[namespace = "truesense"]
        fn set_quit_on_last_window_closed(quit: bool);
        #[namespace = "truesense"]
        fn tray_host_registered() -> bool;
        #[namespace = "truesense"]
        fn allow_resource_reads();
        #[namespace = "truesense"]
        fn choose_renderer(requested: &QString) -> QString;
        #[namespace = "truesense"]
        fn set_engine(engine: Pin<&mut QQmlApplicationEngine>);
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(QString, live_json, cxx_name = "liveJson")]
        #[qproperty(bool, live_ok, cxx_name = "liveOk")]
        #[qproperty(QString, caps_json, cxx_name = "capsJson")]
        #[qproperty(QString, ui_json, cxx_name = "uiJson")]
        #[qproperty(QString, routes_json, cxx_name = "routesJson")]
        #[qproperty(QString, start_page, cxx_name = "startPage")]
        #[qproperty(bool, start_hidden, cxx_name = "startHidden")]
        type Bus = super::BusRust;

        /// Runs a command on a worker thread; the answer arrives as `commandFinished(id, ok, json)`.
        #[qinvokable]
        fn run(self: Pin<&mut Bus>, area: &QString, verb: &QString, args_json: &QString) -> i32;

        /// A keyed write: a newer write for the same `key` replaces one that has not started.
        #[qinvokable]
        fn write(
            self: Pin<&mut Bus>,
            key: &QString,
            area: &QString,
            verb: &QString,
            args_json: &QString,
        ) -> i32;

        /// Calls a view-model module (ts-ui-model/src/vm); answers `{"ok":true,"result":...}` or `{"ok":false,"error":"..."}`.
        #[qinvokable]
        fn vm(self: &Bus, module: &QString, verb: &QString, args_json: &QString) -> QString;

        /// The window is visible (live values tick) or hidden/minimised (they stop).
        #[qinvokable]
        #[cxx_name = "setActive"]
        fn set_active(self: &Bus, active: bool);

        /// Changes a setting of ui.json; answers "" or the message to show.
        #[qinvokable]
        #[cxx_name = "setUi"]
        fn set_ui(self: Pin<&mut Bus>, key: &QString, value_json: &QString) -> QString;

        /// Remembers the window geometry (x and y only when `has_pos`).
        #[qinvokable]
        #[cxx_name = "saveWindow"]
        fn save_window(self: &Bus, width: i32, height: i32, x: i32, y: i32, has_pos: bool);

        /// Re-reads what this machine can do; the answer arrives in `capsJson`.
        #[qinvokable]
        #[cxx_name = "refreshCapabilities"]
        fn refresh_capabilities(self: &Bus);

        /// The id of the page `name` means (aliases allowed), or "".
        #[qinvokable]
        #[cxx_name = "resolvePage"]
        fn resolve_page(self: &Bus, name: &QString) -> QString;

        /// Whether a panel really shows tray icons right now (see `shim.h`); asked when the window
        /// is closed, so closing quits unless the window can be found again in a tray.
        #[qinvokable]
        #[cxx_name = "trayHostRegistered"]
        fn tray_host_registered(self: &Bus) -> bool;

        /// Test switches as JSON: `{"enabled", "exit_after_ms", "shot"}`.
        #[qinvokable]
        #[cxx_name = "testConfig"]
        fn test_config(self: &Bus) -> QString;

        /// Called on the first frame; answers whether the test hook runs (so the shell can skip it).
        #[qinvokable]
        #[cxx_name = "framePresented"]
        fn frame_presented(self: &Bus) -> bool;

        /// Prints the test report line: the QML state merged with Rust's measurements.
        #[qinvokable]
        fn report(self: &Bus, state_json: &QString);

        /// Milliseconds since the process started, with a fraction (a monotonic clock for the
        /// measurements of `BenchHook.qml`; QML's own `Date.now()` has whole milliseconds only).
        #[qinvokable]
        #[cxx_name = "clockMs"]
        fn clock_ms(self: &Bus) -> f64;

        /// Records the start-up moment `qml` (the shell's QML is complete; see `Shared::marks`).
        #[qinvokable]
        #[cxx_name = "shellCompleted"]
        fn shell_completed(self: &Bus);

        /// Prints `BENCH {json}` (one measurement of `BenchHook.qml`), merged with the start-up marks.
        #[qinvokable]
        #[cxx_name = "benchLine"]
        fn bench_line(self: &Bus, json: &QString);

        /// A command finished (see `run`).
        #[qsignal]
        #[cxx_name = "commandFinished"]
        fn command_finished(self: Pin<&mut Bus>, id: i32, ok: bool, json: QString);

        /// Another start asked this window to come forward, showing `page` ("" = keep the page).
        #[qsignal]
        #[cxx_name = "showRequested"]
        fn show_requested(self: Pin<&mut Bus>, page: QString);
    }

    impl cxx_qt::Threading for Bus {}
    impl cxx_qt::Initialize for Bus {}
}

/// What the Bus owns: the threads that must stop with it.
#[derive(Default)]
pub struct BusRust {
    live_json: QString,
    live_ok: bool,
    caps_json: QString,
    ui_json: QString,
    routes_json: QString,
    start_page: QString,
    start_hidden: bool,
    poller: Option<Poller>,
    /// Asks the poller for a tick after a command that changed the machine.
    poke: Option<Poke>,
    server: Option<Server>,
    first_frame_ms: Mutex<Option<u128>>,
}

/// The reply text for QML: `(ok, json)`.
fn reply_parts(reply: &Reply) -> (bool, String) {
    reply.to_json()
}

impl cxx_qt::Initialize for qobject::Bus {
    fn initialize(mut self: Pin<&mut Self>) {
        let Some(shared) = shared().cloned() else { return };
        let qt = self.qt_thread();
        self.as_mut().set_routes_json(QString::from(shared.model.routes.to_json().to_string().as_str()));
        self.as_mut().set_start_page(QString::from(shared.start_page.as_str()));
        self.as_mut().set_start_hidden(shared.start_hidden);
        self.as_mut().set_ui_json(QString::from(ui_json(&shared).as_str()));

        // The live ticker: idle until the window says it is visible.
        let ticker = {
            let (shared, qt) = (shared.clone(), qt.clone());
            Poller::spawn(LIVE_INTERVAL, move || {
                let started = Instant::now();
                let snapshot = shared.model.poll_live();
                TICKS.fetch_add(1, Ordering::Relaxed);
                add_nanos(&TICK_NANOS, started);
                let ok = snapshot.is_some();
                let _ = qt.queue(move |mut bus| {
                    if let Some(text) = snapshot {
                        bus.as_mut().set_live_json(QString::from(text.as_str()));
                    }
                    if *bus.live_ok() != ok {
                        bus.as_mut().set_live_ok(ok);
                    }
                });
            })
        };
        match ticker {
            Ok(p) => {
                let mut rust = self.as_mut().rust_mut();
                rust.poke = Some(p.poke_handle());
                rust.poller = Some(p);
            }
            Err(e) => eprintln!("truesense-qt: cannot start the live ticker: {e}"),
        }

        // The single-instance socket: "show PAGE" requests from later starts.
        let primary = shared.primary.lock().ok().and_then(|mut p| p.take());
        if let Some(primary) = primary {
            let (shared, qt) = (shared.clone(), qt.clone());
            let served = primary.serve(Box::new(move |page| {
                let id = page
                    .and_then(|p| shared.model.routes.resolve_id(&p).map(str::to_owned))
                    .unwrap_or_default();
                let _ = qt.queue(move |bus| bus.show_requested(QString::from(id.as_str())));
            }));
            match served {
                Ok(s) => self.as_mut().rust_mut().server = Some(s),
                Err(e) => eprintln!("truesense-qt: cannot serve the single-instance socket: {e}"),
            }
        }
    }
}

/// The settings JSON QML reads: ui.json plus the language actually in effect.
fn ui_json(shared: &Shared) -> String {
    let locale = crate::platform::system_locale_name();
    let language = shared.model.language(&locale);
    let mut value: Value = serde_json::from_str(&shared.model.ui_json()).unwrap_or_else(|_| json!({}));
    value["language_effective"] = json!(language);
    value.to_string()
}

impl qobject::Bus {
    fn shared(&self) -> Option<&'static Arc<Shared>> {
        shared()
    }

    /// Where the answer of a command goes. A command that may have changed the machine also asks
    /// for a live tick at once (whether it worked or not: a failed change must show the real state
    /// again), so what the page shows from the live values follows within milliseconds, not at
    /// the next one-second tick.
    fn deliver(&self, mutates: bool) -> impl Fn(Reply) + Send + 'static {
        let qt = self.qt_thread();
        let poke = if mutates { self.poke.clone() } else { None };
        move |reply: Reply| {
            let superseded = reply.outcome.as_ref().err().is_some_and(CmdError::is_superseded);
            let (ok, text) = reply_parts(&reply);
            let id = reply.id as i32;
            let _ = qt.queue(move |bus| bus.command_finished(id, ok, QString::from(text.as_str())));
            if let (Some(p), false) = (&poke, superseded) {
                p.poke();
            }
        }
    }

    /// Runs a command on a worker thread.
    pub fn run(self: Pin<&mut Self>, area: &QString, verb: &QString, args_json: &QString) -> i32 {
        let Some(shared) = self.shared() else { return -1 };
        match Request::parse(&area.to_string(), &verb.to_string(), &args_json.to_string()) {
            Ok(request) => {
                let done: Done = Box::new(self.deliver(request.mutates()));
                shared.model.bus.run(request, done) as i32
            }
            Err(e) => shared.model.bus.refuse(e, Box::new(self.deliver(false))) as i32,
        }
    }

    /// A coalesced write.
    pub fn write(
        self: Pin<&mut Self>,
        key: &QString,
        area: &QString,
        verb: &QString,
        args_json: &QString,
    ) -> i32 {
        let Some(shared) = self.shared() else { return -1 };
        match Request::parse(&area.to_string(), &verb.to_string(), &args_json.to_string()) {
            Ok(request) => {
                let done: Done = Box::new(self.deliver(request.mutates()));
                shared.model.bus.write(&key.to_string(), request, done) as i32
            }
            Err(e) => shared.model.bus.refuse(e, Box::new(self.deliver(false))) as i32,
        }
    }

    /// A pure view-model call.
    pub fn vm(&self, module: &QString, verb: &QString, args_json: &QString) -> QString {
        let started = Instant::now();
        let answer = match parse_args(&args_json.to_string()) {
            Ok(args) => match ts_ui_model::vm::dispatch(&module.to_string(), &verb.to_string(), &args) {
                Ok(result) => json!({"ok": true, "result": result}),
                Err(e) => json!({"ok": false, "error": e}),
            },
            Err(e) => json!({"ok": false, "error": CmdError::new(e, Some(CODE_BAD_ARGS)).message}),
        };
        let text = QString::from(answer.to_string().as_str());
        VM_CALLS.fetch_add(1, Ordering::Relaxed);
        add_nanos(&VM_NANOS, started);
        text
    }

    /// Whether the tray has a host (see `shim.h`).
    pub fn tray_host_registered(&self) -> bool {
        crate::platform::tray_host_registered()
    }

    /// Starts or stops the live ticker.
    pub fn set_active(&self, active: bool) {
        if let Some(p) = &self.poller {
            p.set_active(active);
        }
    }

    /// Changes a setting; the language also switches the translator.
    pub fn set_ui(mut self: Pin<&mut Self>, key: &QString, value_json: &QString) -> QString {
        let Some(shared) = self.shared() else { return QString::from("not ready") };
        let key = key.to_string();
        let value: Value = serde_json::from_str(&value_json.to_string()).unwrap_or(Value::Null);
        let result = shared.model.set_ui(&key, &value);
        // A failed write still changed the setting for this run; QML gets the new state either way.
        let text = ui_json(shared);
        if key == "language" {
            let v: Value = serde_json::from_str(&text).unwrap_or_default();
            let code = v["language_effective"].as_str().unwrap_or("en").to_owned();
            crate::platform::set_language(&code);
        }
        self.as_mut().set_ui_json(QString::from(text.as_str()));
        QString::from(result.err().unwrap_or_default().as_str())
    }

    /// Saves the window geometry.
    pub fn save_window(&self, width: i32, height: i32, x: i32, y: i32, has_pos: bool) {
        let Some(shared) = self.shared() else { return };
        let size = (u32::try_from(width).unwrap_or(0), u32::try_from(height).unwrap_or(0));
        if let Err(e) = shared.model.set_window(size, has_pos.then_some((x, y))) {
            eprintln!("truesense-qt: {e}");
        }
    }

    /// Refreshes the capability model on a worker thread.
    pub fn refresh_capabilities(&self) {
        let Some(shared) = self.shared().cloned() else { return };
        let qt = self.qt_thread();
        let spawned = std::thread::Builder::new().name("ts-caps".to_owned()).spawn(move || {
            let text = shared.model.refresh_capabilities();
            let _ = qt.queue(move |mut bus| bus.as_mut().set_caps_json(QString::from(text.as_str())));
        });
        if let Err(e) = spawned {
            eprintln!("truesense-qt: cannot start a worker thread: {e}");
        }
    }

    /// Resolves a page name.
    pub fn resolve_page(&self, name: &QString) -> QString {
        let id = self.shared().and_then(|s| s.model.routes.resolve_id(&name.to_string()).map(str::to_owned));
        QString::from(id.unwrap_or_default().as_str())
    }

    /// The test switches as JSON.
    pub fn test_config(&self) -> QString {
        let t = self.shared().map(|s| s.test.clone()).unwrap_or_default();
        QString::from(
            json!({"enabled": t.enabled, "exit_after_ms": t.exit_after_ms, "shot": t.shot, "fps": t.fps, "scroll": t.scroll, "bench": t.bench})
                .to_string()
                .as_str(),
        )
    }

    /// Records the time of the first frame.
    pub fn frame_presented(&self) -> bool {
        let Some(shared) = self.shared() else { return false };
        if let Ok(mut slot) = self.first_frame_ms.lock() {
            slot.get_or_insert_with(|| shared.started.elapsed().as_millis());
        }
        shared.test.enabled
    }

    /// Prints `REPORT {json}`: what the test hook saw in QML plus Rust's own measurements.
    pub fn report(&self, state_json: &QString) {
        let mut state: Value = serde_json::from_str(&state_json.to_string()).unwrap_or_else(|_| json!({}));
        let first = self.first_frame_ms.lock().ok().and_then(|s| *s);
        state["first_frame_ms"] = json!(first);
        state["rss_kb"] = json!(crate::rss_kb());
        state["live_frames"] = json!(self.shared().map_or(0, |s| s.model.live.accepted()));
        state["renderer"] = json!(RENDERER.get().map_or("", String::as_str));
        println!("REPORT {state}");
    }

    /// The measurement clock.
    pub fn clock_ms(&self) -> f64 {
        self.shared().map_or(0.0, |s| s.started.elapsed().as_secs_f64() * 1000.0)
    }

    /// Records the `qml` mark.
    pub fn shell_completed(&self) {
        if let Some(s) = self.shared() {
            mark(s, "qml");
        }
    }

    /// One measurement line.
    pub fn bench_line(&self, json: &QString) {
        let mut v: Value = serde_json::from_str(&json.to_string()).unwrap_or_else(|_| json!({}));
        let calls = VM_CALLS.swap(0, Ordering::Relaxed);
        let nanos = VM_NANOS.swap(0, Ordering::Relaxed);
        let ticks = TICKS.swap(0, Ordering::Relaxed);
        let tick_nanos = TICK_NANOS.swap(0, Ordering::Relaxed);
        if v.get("phase").and_then(Value::as_str) == Some("reset") {
            return;
        }
        v["vm_calls"] = json!(calls);
        v["vm_ms"] = json!((nanos as f64 / 1e4).round() / 100.0);
        v["ticks"] = json!(ticks);
        v["tick_ms"] =
            json!(if ticks == 0 { 0.0 } else { (tick_nanos as f64 / ticks as f64 / 1e4).round() / 100.0 });
        if v.get("phase").and_then(Value::as_str) == Some("start") {
            v["first_frame_ms"] = json!(self.first_frame_ms.lock().ok().and_then(|s| *s));
            v["rss_kb"] = json!(crate::rss_kb());
            v["renderer"] = json!(RENDERER.get().map_or("", String::as_str));
            if let Some(s) = self.shared()
                && let Ok(marks) = s.marks.lock()
            {
                for (name, ms) in marks.iter() {
                    v[format!("{name}_ms")] = json!((ms * 10.0).round() / 10.0);
                }
            }
        }
        println!("BENCH {v}");
    }
}
