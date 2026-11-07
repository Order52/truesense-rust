// The few Qt calls cxx-qt-lib has no wrapper for. Everything here runs on the GUI thread.
#pragma once

#include <QtCore/QString>
#include <QtQml/QQmlApplicationEngine>

namespace truesense {

// Application identity: name, display name, desktop file name (the Wayland app id), window icon.
void set_identity(const QString &app_id, const QString &display_name, const QString &icon_path);

// Loads `:/i18n/<code>.qm` on top of the English catalogue (so a string missing in `code` shows
// its English text, not its id) and retranslates the running QML engine. Returns false when no
// catalogue exists for `code`.
bool set_language(const QString &code);

// The desktop's locale name ("de_DE", "en_US").
QString system_locale_name();

// Quit only when asked (the window hides to the tray instead of ending the program).
void set_quit_on_last_window_closed(bool quit);

// Whether the desktop really shows tray icons: a StatusNotifierWatcher is on the session bus and
// says a host (a panel that draws the icons) is registered. Qt's own "tray available" is true as
// soon as the watcher exists, and hiding the window into a tray nobody draws leaves an invisible
// program running. Asks the bus (500 ms at most); false without a session bus.
bool tray_host_registered();

// Lets QML read its own icon files with XMLHttpRequest (Qt treats qrc: as a local file and
// forbids it unless QML_XHR_ALLOW_FILE_READ is set). Our QML is the only QML that runs.
void allow_resource_reads();

// Picks the software scene graph unless the person asked for OpenGL (TRUESENSE_QT_RENDER=opengl) or
// set QT_QUICK_BACKEND themselves. A control panel does not need a GPU: the software renderer
// roughly halves the resident memory (no GL driver libraries) and can never wake a sleeping
// discrete GPU. Must run before the first window exists.
void prefer_software_renderer();

// Remembers the engine so that set_language can retranslate it. Called once from main.
void set_engine(QQmlApplicationEngine &engine);

}  // namespace truesense
