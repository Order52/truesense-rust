//! Safe wrappers over the few Qt calls in `cpp/shim.cpp` that cxx-qt-lib has no binding for
//! (application identity, translators, layout direction, quit policy).
//!
//! The C++ side takes only values and a reference to the engine, touches Qt's global application
//! state, and must be called from the GUI thread, which every caller here is (QML invokables and
//! `main`). cxx declares them safe to call (`unsafe extern "C++"`); these wrappers exist to give
//! them plain Rust types and to document that rule in one place.

use core::pin::Pin;

use cxx_qt_lib::{QQmlApplicationEngine, QString};

use crate::bus_object::qobject;

/// The backend used when nothing asks for another one (see docs/QT-APP.md, "Rendering").
pub const DEFAULT_RENDERER: &str = "software";

/// Names the application: class and Wayland app id `app_id`, window icon from `icon_path`.
pub fn set_identity(app_id: &str, display_name: &str, icon_path: &str) {
    let (a, d, i) = (QString::from(app_id), QString::from(display_name), QString::from(icon_path));
    qobject::set_identity(&a, &d, &i)
}

/// Installs the catalogue for `code` over English. False when there is no catalogue for `code`.
pub fn set_language(code: &str) -> bool {
    let c = QString::from(code);
    qobject::set_language(&c)
}

/// The desktop's locale name (`de_DE`).
pub fn system_locale_name() -> String {
    qobject::system_locale_name().to_string()
}

/// Whether Qt ends the program when the last window closes.
pub fn set_quit_on_last_window_closed(quit: bool) {
    qobject::set_quit_on_last_window_closed(quit)
}

/// Registers the engine that `set_language` retranslates. The engine must outlive every later call
/// of `set_language`; `lib::start_qt` keeps it alive until `exec` returns.
pub fn set_engine(engine: Pin<&mut QQmlApplicationEngine>) {
    qobject::set_engine(engine)
}

/// Which backend is asked for: the environment (`TRUESENSE_QT_RENDER`) wins over the `renderer`
/// member of `ui.json`, which wins over [`DEFAULT_RENDERER`]; each is `software`, `opengl` or
/// `vulkan` (`gl` is accepted for `opengl`), and an unknown word counts as not set.
pub fn requested_renderer(env_value: Option<&str>, file_value: Option<&str>) -> &'static str {
    let known = |v: Option<&str>| match v.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("software") => Some("software"),
        Some("opengl" | "gl") => Some("opengl"),
        Some("vulkan") => Some("vulkan"),
        _ => None,
    };
    known(env_value).or_else(|| known(file_value)).unwrap_or(DEFAULT_RENDERER)
}

/// Chooses the Qt Quick scene-graph backend (see `cpp/shim.h`) and returns what was chosen. Must
/// run once the `QGuiApplication` exists and before the first window.
pub fn choose_renderer(file_value: Option<&str>) -> String {
    let env = std::env::var("TRUESENSE_QT_RENDER").ok();
    let wanted = QString::from(requested_renderer(env.as_deref(), file_value));
    qobject::choose_renderer(&wanted).to_string()
}

/// Silences Qt's text-input focus warning (see `cpp/shim.h`); `QT_LOGGING_RULES` still wins.
pub fn quiet_platform_noise() {
    qobject::quiet_platform_noise();
}

/// Lets QML read its own bundled icon files (see `Icon.qml`).
pub fn allow_resource_reads() {
    qobject::allow_resource_reads();
}

/// Whether the session's StatusNotifierWatcher reports a registered host (a panel with a tray).
pub fn tray_host_registered() -> bool {
    qobject::tray_host_registered()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_renderer_request_is_read_leniently_and_defaults() {
        assert_eq!(requested_renderer(Some("software"), None), "software");
        assert_eq!(requested_renderer(Some(" OpenGL "), None), "opengl");
        assert_eq!(requested_renderer(Some("gl"), None), "opengl");
        assert_eq!(requested_renderer(Some("VULKAN"), None), "vulkan");
        assert_eq!(requested_renderer(Some(""), None), DEFAULT_RENDERER);
        assert_eq!(requested_renderer(Some("direct3d"), None), DEFAULT_RENDERER);
        assert_eq!(requested_renderer(None, None), DEFAULT_RENDERER);
    }

    #[test]
    fn the_environment_beats_the_file_and_an_unknown_word_falls_through() {
        assert_eq!(requested_renderer(Some("software"), Some("opengl")), "software");
        assert_eq!(requested_renderer(None, Some("opengl")), "opengl");
        assert_eq!(requested_renderer(Some("direct3d"), Some("vulkan")), "vulkan");
        assert_eq!(requested_renderer(Some(""), Some("nonsense")), DEFAULT_RENDERER);
    }
}
