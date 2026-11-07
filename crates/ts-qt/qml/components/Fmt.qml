// Formatting of measured values: the JavaScript twin of crates/ts-ui-model/src/format.rs (same
// outputs; both are tested against crates/ts-ui-model/tests/format-vectors.json). An unknown value
// (undefined, null, NaN) is an en dash, never "0 °C".
pragma Singleton
import QtQuick

QtObject {
    readonly property string unknown: "–"

    function _num(v) { return (typeof v === "number" && isFinite(v)) ? v : undefined }
    function _round(v) { return Math.round(v) }

    function temp(v) { v = _num(v); return v === undefined ? unknown : _round(v) + " °C" }
    function rpm(v) { v = _num(v); return v === undefined ? unknown : _round(v) + " RPM" }
    function percent(v) { v = _num(v); return v === undefined ? unknown : _round(v) + " %" }
    function watts(v) { v = _num(v); return v === undefined ? unknown : (Math.round(v * 10) / 10) + " W" }

    function signed(v, unit) {
        v = _num(v)
        if (v === undefined) return unknown
        var sign = v > 0 ? "+" : (v < 0 ? "−" : "")
        return sign + Math.abs(v) + (unit ? " " + unit : "")
    }

    function duration(seconds) {
        var v = _num(seconds)
        if (v === undefined) return unknown
        var s = Math.round(Math.max(0, v))
        if (s < 90) return s + " s"
        var m = Math.round(s / 60)
        return m < 90 ? m + " min" : (Math.round(m / 6) / 10) + " h"
    }

    function bytes(v) {
        var n = _num(v)
        if (n === undefined || n < 0) return unknown
        var units = ["B", "KB", "MB", "GB", "TB"]
        for (var i = 0; i < units.length; ++i) {
            if (n < 1024 || i === units.length - 1)
                return (i === 0 || n >= 100) ? Math.round(n) + " " + units[i] : n.toFixed(1) + " " + units[i]
            n /= 1024
        }
        return unknown
    }

    function capitalize(s) { return s ? s.charAt(0).toUpperCase() + s.slice(1) : "" }
}
