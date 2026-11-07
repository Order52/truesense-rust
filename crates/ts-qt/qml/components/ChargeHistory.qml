// The charge history of this session for the Battery page's chart. A singleton so that it keeps
// collecting while the person is on another page: it exists from the first time the Battery page is
// opened until the app ends, and it is never written to disk (persisted history is out of scope).
// It samples from the live stream (which ticks only while the window is visible), keeps one point
// every half minute and at most four hours; the rules are the tested `battery` view-model.
pragma Singleton
import QtQuick
import org.truesense.app

QtObject {
    id: history

    /// [[t, percent], ...]
    property var points: []
    /// {values, points, span_s, span_text, low, high} for a Sparkline and its caption.
    property var series: ({ values: [], points: 0, span_s: 0, span_text: "", low: null, high: null })
    property real _lastT: -1

    property var _conn: Connections {
        target: Live
        function onSerialChanged() { history.sample() }
    }

    function _vm(verb, args) {
        try {
            var r = JSON.parse(Bus.vm("battery", verb, JSON.stringify({ value: args })))
            return r.ok ? r.result : null
        } catch (e) { return null }
    }

    /// Takes the newest live reading (at most one in thirty seconds reaches the view-model).
    function sample() {
        var t = Live.value("t"), pct = Live.value("battery_pct")
        if (typeof t !== "number" || typeof pct !== "number") return
        if (_lastT >= 0 && t >= _lastT && t - _lastT < 30) return
        var next = _vm("history_push", { history: points, t: t, pct: pct })
        if (next === null) return
        points = next
        _lastT = next.length ? next[next.length - 1][0] : -1
        var s = _vm("history_series", { history: next })
        if (s !== null) series = s
    }

    Component.onCompleted: sample()
}
