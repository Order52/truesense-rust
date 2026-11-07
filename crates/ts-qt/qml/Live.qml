// The live state of the machine, one parsed copy for every page. Main.qml hands it the snapshot the
// Rust side builds about once a second while the window is visible: the newest frame and the
// history of a few series for sparklines. Pages bind to `Live.frame`, `Live.value("sensors.cpu_temp")`
// and `Live.history("sensors.cpu_temp")`; they never poll and never call the hardware for these.
pragma Singleton
import QtQuick

QtObject {
    id: live

    /// The newest frame (see COMMANDS.md, "The live stream"), or null before the first one.
    property var frame: null
    /// {"sensors.cpu_temp": [54, null, 55, ...], ...}, oldest first, null = gap.
    property var histories: ({})
    /// Frames accepted so far; changes on every tick.
    property int serial: 0
    /// The last reading worked. False while the machine cannot be read or the window was hidden for long.
    property bool ok: false
    /// The root service answered in the last frame.
    readonly property bool daemonUp: frame !== null && frame.daemon !== undefined && frame.daemon !== null

    /// A value of the newest frame by dotted path ("load.cpu_pct"); undefined when absent or null.
    function value(path) {
        var v = frame
        var parts = path.split(".")
        for (var i = 0; v !== null && v !== undefined && i < parts.length; ++i)
            v = v[parts[i]]
        return (v === null) ? undefined : v
    }

    /// The history of a series (see DEFAULT_SERIES in ts-ui-model), [] when unknown.
    function history(path) {
        var h = histories[path]
        return h === undefined ? [] : h
    }

    function load(text) {
        var o
        try { o = JSON.parse(text) } catch (e) { return }
        frame = o.frame === undefined ? null : o.frame
        histories = o.history === undefined ? ({}) : o.history
        serial = o.n === undefined ? serial + 1 : o.n
        ok = frame !== null
    }

    function markLost() { ok = false }
}
