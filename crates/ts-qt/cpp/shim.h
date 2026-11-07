// The few Qt calls cxx-qt-lib has no wrapper for. Everything here runs on the GUI thread.
#pragma once

#include <QtCore/QString>
#include <QtQml/QQmlApplicationEngine>

namespace truesense {

// Application identity: name, display name, desktop file name (the Wayland app id), window icon.
void set_identity(const QString &app_id, const QString &display_name, const QString &icon_path);

// Loads `:/i18n/<code>.qm` on top of the English catalogue (so a string missing in `code` shows
// its English text, not its id), switches the layout direction for right-to-left languages and
// retranslates the running QML engine. Returns false when no catalogue exists for `code`.
bool set_language(const QString &code, bool rtl);

// The desktop's locale name ("ar_EG", "en_US").
QString system_locale_name();

// Quit only when asked (the window hides to the tray instead of ending the program).
void set_quit_on_last_window_closed(bool quit);

// Lets QML read its own icon files with XMLHttpRequest (Qt treats qrc: as a local file and
// forbids it unless QML_XHR_ALLOW_FILE_READ is set). Our QML is the only QML that runs.
void allow_resource_reads();

// Remembers the engine so that set_language can retranslate it. Called once from main.
void set_engine(QQmlApplicationEngine &engine);

}  // namespace truesense
