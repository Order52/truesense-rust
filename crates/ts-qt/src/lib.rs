//! truesense-qt: the TrueSense window, Qt 6 and QML over cxx-qt.
//!
//! Layout of the crate (page agents read `docs/QT-APP.md`'s "page contract"; the short version):
//!
//! * `qml/` is the whole UI. `Main.qml` is the shell (sidebar, header, status line, toasts);
//!   `qml/pages/<Name>Page.qml` are the pages, listed once in `qml/pages/registry.json`;
//!   `qml/components/` is the component library; `Theme`, `Ui`, `Live`, `Api`, `Caps`, `Nav`,
//!   `Toasts` are the singletons a page may use. `build.rs` finds every file, so adding one needs
//!   no edit anywhere else.
//! * [`bus_object`] is the cxx-qt bridge (the `Bus` singleton): commands, live values, settings.
//!   Everything it does is a thin call into `ts-ui-model`, which holds the logic and its tests.
//! * [`run`] is the program: arguments, single instance, Qt start-up.
//!
//! # Rendering
//!
//! The scene graph runs on the **software** backend by default (about a third of the memory of the
//! GPU ones and nothing that wakes a graphics driver; measured numbers in `tools/qtprofile`).
//! `TRUESENSE_QT_RENDER=opengl|vulkan`, or `"renderer": "opengl"` in `ui.json`, asks for a GPU
//! backend; it is used only when a context can really be created, else the program falls back to
//! software. Qt's own `QT_QUICK_BACKEND` always wins. The QML must stay cheap on every backend: no
//! script-drawn `Canvas`, no animation that runs while nothing changes (`tests/render.rs` pins it).
//! `tools/qtprofile/qtprofile.py` measures idle CPU, wake-ups, frames per second and memory per page
//! and backend, and `... glitch` looks for stale or corrupted pictures on a live Wayland session.
//!
//! The Qt application object, the engine and the window exist once per process, so the tests that
//! need them run the real binary (`tests/app.rs`) instead of calling [`run`] in the test process.

pub mod bus_object;
pub mod models;
pub mod platform;

use std::sync::{Arc, Mutex};
use std::time::Instant;

use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QQuickStyle, QString, QUrl};
use ts_ui_model::app::AppModel;
use ts_ui_model::single::{self, Acquired};

use crate::bus_object::{Shared, TestConfig, install};

const MAIN_QML: &str = "qrc:/qt/qml/org/truesense/app/qml/Main.qml";
const REGISTRY: &str = include_str!("../qml/pages/registry.json");
/// The window class and Wayland app id; `TRUESENSE_APP_ID` overrides it for tests only.
pub const APP_ID: &str = "truesense";
const ICON: &str = ":/assets/truesense-256.png";

/// What the command line asked for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Args {
    /// `--page NAME`: the page to show (in this window, or in the one that is already open).
    pub page: Option<String>,
    /// `--background`: start without showing the window.
    pub background: bool,
    /// `--version`.
    pub version: bool,
    /// `--help`.
    pub help: bool,
}

/// Parses the arguments after the program name.
///
/// # Errors
/// The message to print for an unknown option or a missing value.
pub fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut out = Args::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--page" | "-p" => match it.next() {
                Some(name) if single::valid_page(name) => out.page = Some(name.clone()),
                Some(name) => return Err(format!("not a page name: {name:?}")),
                None => return Err("--page needs a page name".to_owned()),
            },
            "--background" => out.background = true,
            "--version" | "-V" => out.version = true,
            "--help" | "-h" => out.help = true,
            other => match other.strip_prefix("--page=") {
                Some(name) if single::valid_page(name) => out.page = Some(name.to_owned()),
                _ => return Err(format!("unknown argument: {other}")),
            },
        }
    }
    Ok(out)
}

/// The usage text.
pub const USAGE: &str = "usage: truesense-qt [--page NAME] [--background]\n\
    \n  --page NAME    show a page (home, monitor, performance, cooling, lighting, keys, battery,\
    \n                 audio, profiles, health, settings); in the open window when there is one\
    \n  --background   start without showing the window\
    \n  --version      print the version\n";

/// Resident set size in kB (0 when /proc is not readable).
pub fn rss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find_map(|l| l.strip_prefix("VmRSS:").map(str::to_owned)))
        .and_then(|r| r.split_whitespace().next().and_then(|n| n.parse().ok()))
        .unwrap_or(0)
}

fn start_qt(shared: &Shared) -> i32 {
    let mut app = QGuiApplication::new();
    platform::quiet_platform_noise();
    QQuickStyle::set_style(&QString::from("Basic"));
    let app_id =
        std::env::var("TRUESENSE_APP_ID").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| APP_ID.to_owned());
    platform::set_identity(&app_id, "TrueSense", ICON);
    // The window hides to the tray (or the program ends) by QML's decision, never by Qt's.
    platform::set_quit_on_last_window_closed(false);

    let locale = platform::system_locale_name();
    let language = shared.model.language(&locale);
    platform::set_language(&language);

    let saved = shared.model.settings();
    let renderer = platform::choose_renderer(saved.extra_str("renderer"));
    bus_object::set_renderer(renderer);
    platform::allow_resource_reads();
    let mut engine = QQmlApplicationEngine::new();
    let Some(mut engine_ref) = engine.as_mut() else { return 1 };
    // A QML file that does not load leaves exec() spinning with no window: end with status 1.
    engine_ref
        .as_mut()
        .on_object_creation_failed(|_, url| {
            eprintln!("truesense-qt: QML failed to load: {url:?}");
            std::process::exit(1);
        })
        .release();
    platform::set_engine(engine_ref.as_mut());
    // `TRUESENSE_QT_QML_URL` exists so that a test can prove that a QML file that does not load ends
    // the program with status 1; nobody else sets it.
    let url = std::env::var("TRUESENSE_QT_QML_URL").ok().filter(|u| !u.is_empty());
    engine_ref.load(&QUrl::from(url.as_deref().unwrap_or(MAIN_QML)));
    match app.as_mut() {
        Some(app) => app.exec(),
        None => 1,
    }
}

/// The program. `args` are the arguments after the program name; returns the exit status.
pub fn run(args: &[String]) -> i32 {
    let started = Instant::now();
    let args = match parse_args(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("truesense-qt: {e}\n{USAGE}");
            return 2;
        }
    };
    if args.help {
        print!("{USAGE}");
        return 0;
    }
    if args.version {
        println!("truesense-qt {}", ts_foundation::buildinfo::VERSION);
        return 0;
    }

    // One window per user: a second start forwards its page to the first and ends at once, before
    // anything heavy (Qt, the application layer) is built.
    let dir = match single::runtime_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("truesense-qt: {e}");
            return 1;
        }
    };
    let primary = match single::acquire(&dir) {
        Ok(Acquired::Primary(p)) => p,
        Ok(Acquired::Existing) => {
            return match single::send_show(&dir, args.page.as_deref()) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("truesense-qt: {e}");
                    1
                }
            };
        }
        Err(e) => {
            eprintln!("truesense-qt: {e}");
            return 1;
        }
    };

    let model = match AppModel::system(REGISTRY) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("truesense-qt: {e}");
            return 1;
        }
    };
    for problem in model.settings_problems() {
        eprintln!("truesense-qt: {problem}");
    }
    let start_page = args
        .page
        .as_deref()
        .and_then(|p| model.routes.resolve_id(p))
        .or_else(|| model.routes.first().map(|p| p.id.as_str()))
        .unwrap_or("home")
        .to_owned();
    let shared = Arc::new(Shared {
        model,
        start_page,
        start_hidden: args.background,
        test: TestConfig::from_env(),
        started,
        primary: Mutex::new(Some(primary)),
    });
    if !install(shared.clone()) {
        eprintln!("truesense-qt: already started in this process");
        return 1;
    }
    start_qt(&shared)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn arguments_parse() {
        assert_eq!(parse_args(&a(&[])).unwrap(), Args::default());
        assert_eq!(parse_args(&a(&["--page", "cooling"])).unwrap().page.as_deref(), Some("cooling"));
        assert_eq!(parse_args(&a(&["--page=keys"])).unwrap().page.as_deref(), Some("keys"));
        assert!(parse_args(&a(&["--background"])).unwrap().background);
        assert!(parse_args(&a(&["-h"])).unwrap().help);
        assert!(parse_args(&a(&["--version"])).unwrap().version);
    }

    #[test]
    fn bad_arguments_are_refused() {
        assert!(parse_args(&a(&["--page"])).is_err());
        assert!(parse_args(&a(&["--page", "../x"])).is_err());
        assert!(parse_args(&a(&["--page="])).is_err());
        assert!(parse_args(&a(&["--nope"])).is_err());
        assert!(parse_args(&a(&["stray"])).is_err());
    }

    #[test]
    fn rss_is_readable() {
        assert!(rss_kb() > 0);
    }
}
