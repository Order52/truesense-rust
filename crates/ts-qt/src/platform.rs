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

/// Names the application: class and Wayland app id `app_id`, window icon from `icon_path`.
pub fn set_identity(app_id: &str, display_name: &str, icon_path: &str) {
    let (a, d, i) = (QString::from(app_id), QString::from(display_name), QString::from(icon_path));
    qobject::set_identity(&a, &d, &i)
}

/// Installs the catalogue for `code` over English and mirrors the layout when `rtl`. False when
/// there is no catalogue for `code`.
pub fn set_language(code: &str, rtl: bool) -> bool {
    let c = QString::from(code);
    qobject::set_language(&c, rtl)
}

/// The desktop's locale name (`ar_EG`).
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

/// Lets QML read its own bundled icon files (see `Icon.qml`).
pub fn allow_resource_reads() {
    qobject::allow_resource_reads();
}

/// Whether the session's StatusNotifierWatcher reports a registered host (a panel with a tray).
pub fn tray_host_registered() -> bool {
    qobject::tray_host_registered()
}
