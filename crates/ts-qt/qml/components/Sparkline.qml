// A small line chart of the last few minutes of one value. `values` is an array of numbers where
// null (or NaN) is a gap: the line breaks there instead of dropping to zero, so a sensor that
// disappeared for a moment looks like what it is. It repaints when `values` or the size change,
// and nothing else (no timer, no animation), so a hidden page costs nothing.
//
//   Sparkline { values: Live.history("sensors.cpu_temp"); height: 32 }
//
// The scale follows the data unless `minimum` / `maximum` are set (a percentage chart sets 0 and 100).
// Mirroring: charts never mirror, time always runs left to right (APP-DESIGN 4.7).
import QtQuick
import org.truesense.app

Item {
    id: root

    property var values: []
    property color lineColor: Theme.chartLine
    property color fillColor: Theme.chartFill
    property bool filled: false
    property real lineWidth: 1.6
    property real minimum: NaN
    property real maximum: NaN
    /// Show a dot on the newest sample.
    property bool dot: true

    /// The numbers actually drawn (no gaps).
    readonly property int pointCount: _valid(values).length
    /// The scale in use.
    readonly property real low: _range(values).lo
    readonly property real high: _range(values).hi
    /// How often the canvas painted (for tests).
    property int paintCount: 0

    implicitWidth: 120
    implicitHeight: 28

    function _isNum(v) { return v !== null && v !== undefined && typeof v === "number" && isFinite(v) }
    function _valid(a) { return a.filter(_isNum) }
    function _range(a) {
        var v = _valid(a)
        var lo = isNaN(minimum) ? (v.length ? Math.min.apply(null, v) : 0) : minimum
        var hi = isNaN(maximum) ? (v.length ? Math.max.apply(null, v) : 1) : maximum
        if (hi - lo < 1e-9) { hi = lo + 1 }
        return { lo: lo, hi: hi }
    }

    /// The shortest time between two repaints caused by new `values`. 0 repaints at once. A
    /// chart of minutes of history that shifts by one sample a second has nothing to show in the
    /// second after the last repaint, and a canvas repaint is the most expensive thing a live page
    /// does (each costs about as much as the rest of the second's update together), so the live
    /// pages ask for a few seconds; a change that arrives sooner is drawn when the time is up.
    property int minRepaintMs: 0

    // The live history arrives as a new array every second even when it holds the same numbers (a
    // flat fan-speed or battery series): repaint only when what would be drawn is different.
    property string _drawn: ""
    property double _paintedAt: 0
    function _repaintIfChanged() {
        var key = JSON.stringify(values)
        if (key === _drawn) { settle.stop(); return }
        var wait = minRepaintMs - (Date.now() - _paintedAt)
        if (wait > 0) {
            settle.interval = wait
            settle.restart()
            return
        }
        _paint(key)
    }
    function _paint(key) {
        settle.stop()
        _drawn = key
        _paintedAt = Date.now()
        canvas.requestPaint()
    }
    Timer {
        id: settle
        // Only runs while a change waits for its turn.
        onTriggered: root._repaintIfChanged()
    }
    onValuesChanged: _repaintIfChanged()
    onWidthChanged: canvas.requestPaint()
    onHeightChanged: canvas.requestPaint()
    onLineColorChanged: canvas.requestPaint()
    onFilledChanged: canvas.requestPaint()
    onMinimumChanged: canvas.requestPaint()
    onMaximumChanged: canvas.requestPaint()

    Canvas {
        id: canvas
        anchors.fill: parent
        // Never mirrored: a chart's time axis is not text direction.
        LayoutMirroring.enabled: false
        renderTarget: Canvas.Image
        onPainted: root.paintCount++
        onPaint: {
            var ctx = getContext("2d")
            ctx.reset()
            var a = root.values
            var n = a.length
            if (n < 2 || width <= 0 || height <= 0) return
            var r = root._range(a)
            var pad = root.lineWidth + (root.dot ? 2 : 0)
            var step = width / Math.max(1, n - 1)
            function xOf(i) { return pad / 2 + i * (width - pad) / Math.max(1, n - 1) }
            function yOf(v) { return pad / 2 + (height - pad) * (1 - (v - r.lo) / (r.hi - r.lo)) }
            ctx.lineWidth = root.lineWidth
            ctx.lineJoin = "round"
            ctx.lineCap = "round"
            var run = []
            var runs = []
            for (var i = 0; i < n; ++i) {
                if (root._isNum(a[i])) run.push(i)
                else if (run.length) { runs.push(run); run = [] }
            }
            if (run.length) runs.push(run)
            for (var k = 0; k < runs.length; ++k) {
                var seg = runs[k]
                if (root.filled && seg.length > 1) {
                    ctx.beginPath()
                    ctx.moveTo(xOf(seg[0]), height)
                    for (var f = 0; f < seg.length; ++f) ctx.lineTo(xOf(seg[f]), yOf(a[seg[f]]))
                    ctx.lineTo(xOf(seg[seg.length - 1]), height)
                    ctx.closePath()
                    ctx.fillStyle = root.fillColor
                    ctx.fill()
                }
                ctx.beginPath()
                for (var j = 0; j < seg.length; ++j) {
                    var x = xOf(seg[j]), y = yOf(a[seg[j]])
                    if (j === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y)
                }
                ctx.strokeStyle = root.lineColor
                ctx.stroke()
            }
            if (root.dot && runs.length && runs[runs.length - 1][runs[runs.length - 1].length - 1] === n - 1) {
                ctx.beginPath()
                ctx.arc(xOf(n - 1), yOf(a[n - 1]), root.lineWidth + 0.8, 0, 2 * Math.PI)
                ctx.fillStyle = root.lineColor
                ctx.fill()
            }
        }
    }

    Accessible.role: Accessible.Graphic
    //% "History chart"
    Accessible.name: qsTrId("sparkline_name")
}
