// The app's own settings as seen from QML (theme, language, scale, tray). The values come from
// ui.json through the Rust side: Main.qml calls load() with the JSON text whenever it changes and
// forwards changeRequested() to the Rust side. This file never touches Rust itself, so components
// that read it also run in the component tests.
pragma Singleton
import QtQuick

QtObject {
    id: ui

    /// "dark", "light" or "system".
    property string theme: "dark"
    /// What the person chose: "system", "en", "ar" (or "pseudo" in tests).
    property string languageSetting: "system"
    /// The language actually loaded.
    property string language: "en"
    /// Whether the layout is mirrored (Arabic).
    property bool rtl: false
    property real fontScale: 1.0
    property bool reducedMotion: false
    property bool minimiseToTray: true
    /// Set by the shell: a system tray exists on this desktop.
    property bool trayAvailable: false
    /// Why the settings file could not be used, or "".
    property string problem: ""

    /// A page asks for a setting to change: `Ui.set("theme", "light")`.
    signal changeRequested(string key, var value)

    function set(key, value) { changeRequested(key, value) }

    function load(text) {
        var o = {}
        try { o = JSON.parse(text) } catch (e) { return }
        if (o.theme !== undefined) theme = o.theme
        if (o.language !== undefined) languageSetting = o.language
        if (o.language_effective !== undefined) language = o.language_effective
        if (o.rtl !== undefined) rtl = o.rtl
        if (o.font_scale !== undefined) fontScale = o.font_scale
        if (o.reduced_motion !== undefined) reducedMotion = o.reduced_motion
        if (o.minimise_to_tray !== undefined) minimiseToTray = o.minimise_to_tray
        problem = o.problem ? o.problem : ""
    }
}
