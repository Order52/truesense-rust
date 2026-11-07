// A small line chart of the last few minutes of one value. `values` is an array of numbers where
// null (or NaN) is a gap: the line breaks there instead of dropping to zero, so a sensor that
// disappeared for a moment looks like what it is. The drawing is rebuilt when what would be drawn
// changes (and at most every `minRepaintMs`), when the size changes, and at no other time (no
// timer that runs, no animation), so a hidden page or an unchanged series costs nothing.
//
// It is scene-graph geometry (QtQuick.Shapes polylines), not a script-drawn Canvas: a rebuild
// replaces a few vertices and the window repaints only this item's rectangle, where a Canvas
// re-ran JavaScript, filled an image and uploaded it as a texture.
//
//   Sparkline { values: Live.history("sensors.cpu_temp"); height: 32 }
//
// The scale follows the data unless `minimum` / `maximum` are set (a percentage chart sets 0 and 100).
// Mirroring: charts never mirror, time always runs left to right (APP-DESIGN 4.7).
import QtQuick
import QtQuick.Shapes
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
    /// How often the drawing was rebuilt (for tests).
    property int paintCount: 0

    /// The shortest time between two redraws caused by new `values`: a chart of minutes of history
    /// that shifts by one sample a second has nothing to show in the two seconds after the last
    /// redraw. A change that arrives sooner is drawn when the time is up (the newest wins); 0 draws
    /// at once. A resize or a new scale always redraws at once.
    property int minRepaintMs: 2000

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

    // The live history arrives as a new array every second even when it holds the same numbers (a
    // flat fan-speed or battery series): redraw only when what would be drawn is different.
    property string _drawn: ""
    property var _shown: []
    property double _paintedAt: 0
    // What the Shape draws.
    property var _lines: []
    property var _polys: []
    property var _dotAt: null

    function _repaintIfChanged() {
        var key = JSON.stringify(values)
        if (key === _drawn) { settle.stop(); return }
        var wait = minRepaintMs - (Date.now() - _paintedAt)
        if (wait > 0) {
            settle.interval = wait
            settle.restart()
            return
        }
        settle.stop()
        _drawn = key
        _shown = values
        _paintedAt = Date.now()
        _rebuild()
    }
    Timer {
        id: settle
        // Only runs while a change waits for its turn.
        onTriggered: root._repaintIfChanged()
    }
    onValuesChanged: _repaintIfChanged()
    onWidthChanged: Qt.callLater(_rebuild)
    onHeightChanged: Qt.callLater(_rebuild)
    onFilledChanged: Qt.callLater(_rebuild)
    onDotChanged: Qt.callLater(_rebuild)
    onLineWidthChanged: Qt.callLater(_rebuild)
    onMinimumChanged: Qt.callLater(_rebuild)
    onMaximumChanged: Qt.callLater(_rebuild)

    property string _built: ""
    function _rebuild() {
        // Nothing to do when the picture would be the one already drawn (a NaN-valued property
        // such as `minimum` announces a change at creation).
        var r = _range(_shown)
        var key = _drawn + "|" + r.lo + "|" + r.hi + "|" + width + "x" + height + "|" + filled + dot + lineWidth
        if (key === _built) return
        _built = key
        paintCount++
        var a = _shown
        var n = a.length
        var lines = [], polys = [], dotAt = null
        if (n >= 2 && width > 0 && height > 0) {
            var pad = lineWidth + (dot ? 2 : 0)
            var w = width, h = height
            var xOf = function (i) { return pad / 2 + i * (w - pad) / (n - 1) }
            var yOf = function (v) { return pad / 2 + (h - pad) * (1 - (v - r.lo) / (r.hi - r.lo)) }
            var run = []
            var runs = []
            for (var i = 0; i < n; ++i) {
                if (_isNum(a[i])) run.push(i)
                else if (run.length) { runs.push(run); run = [] }
            }
            if (run.length) runs.push(run)
            for (var k = 0; k < runs.length; ++k) {
                var seg = runs[k]
                if (seg.length < 2) continue
                var pts = []
                for (var j = 0; j < seg.length; ++j) pts.push(Qt.point(xOf(seg[j]), yOf(a[seg[j]])))
                lines.push(pts)
                if (filled) {
                    polys.push([Qt.point(pts[0].x, h)].concat(pts).concat([Qt.point(pts[pts.length - 1].x, h)]))
                }
            }
            if (dot && runs.length && runs[runs.length - 1][runs[runs.length - 1].length - 1] === n - 1)
                dotAt = Qt.point(xOf(n - 1), yOf(a[n - 1]))
        }
        _lines = lines
        _polys = polys
        _dotAt = dotAt
    }

    Shape {
        anchors.fill: parent
        // Never mirrored: a chart's time axis is not text direction.
        LayoutMirroring.enabled: false
        preferredRendererType: Shape.CurveRenderer
        ShapePath {
            strokeColor: "transparent"
            fillColor: root.fillColor
            PathMultiline { paths: root._polys }
        }
        ShapePath {
            strokeColor: root.lineColor
            strokeWidth: root.lineWidth
            fillColor: "transparent"
            capStyle: ShapePath.RoundCap
            joinStyle: ShapePath.RoundJoin
            PathMultiline { paths: root._lines }
        }
    }

    Rectangle {
        visible: root._dotAt !== null
        readonly property real d: 2 * (root.lineWidth + 0.8)
        x: root._dotAt ? root._dotAt.x - d / 2 : 0
        y: root._dotAt ? root._dotAt.y - d / 2 : 0
        width: d
        height: d
        radius: d / 2
        color: root.lineColor
    }

    Accessible.role: Accessible.Graphic
    //% "History chart"
    Accessible.name: qsTrId("sparkline_name")
}
