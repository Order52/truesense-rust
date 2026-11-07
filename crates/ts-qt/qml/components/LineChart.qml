// A line chart of one or more series over time: the page's real chart, where Sparkline is the small
// one-value strip. Written once for the Monitor page and generic enough for any page.
//
//   LineChart {
//       capacity: 120
//       series: [ { values: rx, color: Theme.accent, label: "Download", filled: true },
//                 { values: tx, color: Theme.ok,     label: "Upload" } ]
//       minimum: 0; maximum: scale.max                 // omit for auto-scaling
//       grid: [ { value: 0, text: "0" }, { value: 1e6, text: "1 MB/s" } ]
//   }
//
// * `values` of a series: numbers, `null` (or NaN) is a gap: the line breaks there, so a sensor that
//   was missing, or a time the tab was not shown, looks like what it is instead of a drop to zero.
// * Scale: `minimum` and `maximum` when set, else the range of the data with a little headroom
//   (`zeroBased` pins the bottom to 0). `grid` is a list of {value, text} drawn as faint lines with
//   their label at the start edge; the page computes round numbers and unit texts (ts-ui-model).
// * `capacity` > 0 fixes the time span at that many samples, newest at the right edge, so a chart
//   that has just started fills from the right instead of stretching two points across the width.
// * It is scene-graph geometry (QtQuick.Shapes), not a script-drawn Canvas. The drawing is rebuilt when
//   what would be drawn changes (the same numbers in a new array do not count) and at most every
//   `minRepaintMs`, when the scale or the size changes, and at no other time: no timer that runs,
//   no animation, so a hidden page costs nothing. A page must assign a new array to change the
//   data (an in-place change raises no signal).
// * Never mirrored: time runs left to right in every language (the legend above follows the
//   language's direction).
import QtQuick
import QtQuick.Layouts
import QtQuick.Shapes
import org.truesense.app

ColumnLayout {
    id: root

    /// [{values, color, label?, filled?}]
    property var series: []
    property real minimum: NaN
    property real maximum: NaN
    property bool zeroBased: false
    /// [{value, text}], drawn bottom to top.
    property var grid: []
    /// Samples across the whole width; 0 stretches the data over the width.
    property int capacity: 0
    property real lineWidth: 1.8
    property bool showLegend: true
    property int chartHeight: 140
    /// Said when no series has a number (nothing measured yet).
    property string emptyText: ""
    /// For screen readers: the latest values in words (the page knows the units).
    property string summary: ""
    /// How often the drawing was rebuilt (for tests).
    property int paintCount: 0
    /// The shortest time between two redraws caused by new `series` (the newest wins when it is
    /// up). A resize or a new scale redraws at once.
    property int minRepaintMs: 1000

    /// The numbers drawn, over all series.
    readonly property int pointCount: _count(series)
    /// The vertical range in use.
    readonly property real low: _range(series).lo
    readonly property real high: _range(series).hi

    spacing: 6
    implicitHeight: legend.implicitHeight + chartHeight + spacing
    Accessible.role: Accessible.Graphic
    Accessible.name: summary
    //% "Chart"
    Accessible.description: qsTrId("linechart_name")

    function _isNum(v) { return v !== null && v !== undefined && typeof v === "number" && isFinite(v) }
    function _all(list) {
        var out = []
        for (var i = 0; i < list.length; ++i) {
            var v = list[i].values || []
            for (var j = 0; j < v.length; ++j) if (_isNum(v[j])) out.push(v[j])
        }
        return out
    }
    function _count(list) { return _all(list).length }
    function _range(list) {
        var v = _all(list)
        var lo = isNaN(minimum) ? (zeroBased ? 0 : (v.length ? Math.min.apply(null, v) : 0)) : minimum
        var hi = isNaN(maximum) ? (v.length ? Math.max.apply(null, v) : 1) : maximum
        if (isNaN(maximum) && v.length) hi += (hi - lo) * 0.08
        if (isNaN(minimum) && !zeroBased && v.length) lo -= (hi - lo) * 0.08
        if (hi - lo < 1e-9) hi = lo + 1
        return { lo: lo, hi: hi }
    }

    // What is drawn: a snapshot of `series` (so a throttled chart keeps showing one consistent
    // picture), its scale, and the geometry built from both.
    property string _drawn: ""
    property var _shown: []
    property var _view: ({ lo: 0, hi: 1, span: 2 })
    property double _paintedAt: 0

    function _seriesKey(list) {
        var parts = []
        for (var i = 0; i < list.length; ++i)
            parts.push(JSON.stringify(list[i].values || []) + "|" + String(list[i].color) + "|" + (list[i].filled === true))
        return parts.join(";")
    }
    function _redrawIfChanged() {
        var key = _seriesKey(series)
        if (key === _drawn) { settle.stop(); return }
        var wait = minRepaintMs - (Date.now() - _paintedAt)
        if (wait > 0) {
            settle.interval = wait
            settle.restart()
            return
        }
        settle.stop()
        _drawn = key
        _shown = series
        _paintedAt = Date.now()
        _rebuild()
    }
    Timer {
        id: settle
        // Only runs while a change waits for its turn.
        onTriggered: root._redrawIfChanged()
    }
    property string _built: ""
    function _rebuild() {
        var r = _range(_shown)
        var longest = 0
        for (var i = 0; i < _shown.length; ++i) longest = Math.max(longest, (_shown[i].values || []).length)
        var span = Math.max(2, capacity > 0 ? capacity : longest)
        // A NaN-valued property (`minimum`, `maximum`) announces itself at creation: do nothing
        // when the picture would be the one already drawn.
        var key = _drawn + "|" + r.lo + "|" + r.hi + "|" + span
        if (key === _built) return
        _built = key
        paintCount++
        _view = { lo: r.lo, hi: r.hi, span: span }
    }
    onSeriesChanged: _redrawIfChanged()
    onMinimumChanged: Qt.callLater(_rebuild)
    onMaximumChanged: Qt.callLater(_rebuild)
    onZeroBasedChanged: Qt.callLater(_rebuild)
    onCapacityChanged: Qt.callLater(_rebuild)

    Flow {
        id: legend
        Layout.fillWidth: true
        visible: root.showLegend && root.series.length > 0 && (root.series[0].label !== undefined)
        spacing: 14
        Repeater {
            model: root.series
            delegate: Row {
                required property var modelData
                spacing: 6
                visible: modelData.label !== undefined && modelData.label !== ""
                Rectangle {
                    width: 10; height: 10; radius: 5
                    anchors.verticalCenter: parent.verticalCenter
                    color: modelData.color
                }
                Text {
                    text: modelData.label || ""
                    color: Theme.muted
                    font.pixelSize: Theme.fontSmall
                }
            }
        }
    }

    Item {
        id: plot
        Layout.fillWidth: true
        Layout.preferredHeight: root.chartHeight
        // Time never runs right to left.
        LayoutMirroring.enabled: false
        LayoutMirroring.childrenInherit: true

        // The space the labels take at the start edge.
        readonly property real labelWidth: {
            var w = 0
            for (var i = 0; i < labels.count; ++i) {
                var item = labels.itemAt(i)
                if (item) w = Math.max(w, item.implicitWidth)
            }
            return w > 0 ? w + 8 : 0
        }

        // The plot area: where the grid and the series are drawn.
        readonly property real padTop: 6
        readonly property real padBottom: 6
        readonly property real padRight: 6
        readonly property real plotH: Math.max(1, height - padTop - padBottom)
        readonly property real plotW: Math.max(1, width - labelWidth - padRight)
        function yOf(v) { return padTop + plotH * (1 - (v - root._view.lo) / (root._view.hi - root._view.lo)) }

        // The grid: one line per entry of `grid` (three evenly spaced ones when the page gave none).
        Shape {
            x: plot.labelWidth
            width: plot.plotW
            height: plot.height
            preferredRendererType: Shape.CurveRenderer
            ShapePath {
                strokeColor: Theme.chartGrid
                strokeWidth: 1
                fillColor: "transparent"
                PathMultiline {
                    paths: {
                        var g = root.grid
                        var lines = g.length ? g.map(function (e) { return e.value })
                                             : [root._view.lo, (root._view.lo + root._view.hi) / 2, root._view.hi]
                        return lines.map(function (v) {
                            var gy = Math.round(plot.yOf(v)) + 0.5
                            return [Qt.point(0, gy), Qt.point(plot.plotW, gy)]
                        })
                    }
                }
            }
        }

        // The series, one Shape each (a Shape cannot be repeated in itself).
        Repeater {
            model: root._shown
            delegate: Shape {
                id: line
                required property var modelData
                x: plot.labelWidth
                width: plot.plotW
                height: plot.height
                preferredRendererType: Shape.CurveRenderer

                readonly property color tint: Qt.color(modelData.color)
                readonly property var a: modelData.values || []
                readonly property int n: a.length
                readonly property real span: root._view.span
                function xOf(i) { return plot.plotW - (n - 1 - i) * plot.plotW / (span - 1) }
                // [[sample indexes of one unbroken run], ...]
                readonly property var runs: {
                    var out = [], seg = []
                    for (var i = 0; i < n; ++i) {
                        if (root._isNum(a[i])) seg.push(i)
                        else if (seg.length) { out.push(seg); seg = [] }
                    }
                    if (seg.length) out.push(seg)
                    return out
                }
                readonly property var lines: runs.filter(function (r) { return r.length > 1 }).map(function (r) {
                    return r.map(function (i) { return Qt.point(xOf(i), plot.yOf(a[i])) })
                })
                readonly property var polys: modelData.filled !== true ? [] : lines.map(function (pts) {
                    return [Qt.point(pts[0].x, plot.padTop + plot.plotH)].concat(pts)
                        .concat([Qt.point(pts[pts.length - 1].x, plot.padTop + plot.plotH)])
                })
                // A lone sample has no line: it is drawn as a dot; the newest sample gets one too.
                readonly property var dots: {
                    var out = []
                    for (var i = 0; i < runs.length; ++i)
                        if (runs[i].length === 1) out.push(Qt.point(xOf(runs[i][0]), plot.yOf(a[runs[i][0]])))
                    var last = runs.length ? runs[runs.length - 1] : null
                    if (last && last.length > 1 && last[last.length - 1] === n - 1)
                        out.push(Qt.point(xOf(n - 1), plot.yOf(a[n - 1])))
                    return out
                }

                ShapePath {
                    strokeColor: "transparent"
                    fillColor: Qt.rgba(line.tint.r, line.tint.g, line.tint.b, 0.16)
                    PathMultiline { paths: line.polys }
                }
                ShapePath {
                    strokeColor: line.tint
                    strokeWidth: root.lineWidth
                    fillColor: "transparent"
                    capStyle: ShapePath.RoundCap
                    joinStyle: ShapePath.RoundJoin
                    PathMultiline { paths: line.lines }
                }
                Repeater {
                    model: line.dots
                    delegate: Rectangle {
                        required property var modelData
                        readonly property real d: 2 * (root.lineWidth + 0.8)
                        x: modelData.x - d / 2
                        y: modelData.y - d / 2
                        width: d
                        height: d
                        radius: d / 2
                        color: line.tint
                    }
                }
            }
        }

        // Axis labels at the start edge, one per grid line.
        Repeater {
            id: labels
            model: root.grid
            delegate: Text {
                required property var modelData
                x: 0
                y: {
                    var r = root._view
                    var top = 6, h = plot.height - 12
                    var centred = top + h * (1 - (modelData.value - r.lo) / (r.hi - r.lo)) - implicitHeight / 2
                    return Math.max(0, Math.min(plot.height - implicitHeight, centred))
                }
                text: modelData.text
                color: Theme.muted
                font.pixelSize: Theme.fontSmall - 1
            }
        }

        Text {
            anchors.centerIn: parent
            visible: root.emptyText !== "" && root.pointCount === 0
            text: root.emptyText
            color: Theme.muted
            font.pixelSize: Theme.fontSmall + 1
        }
    }
}
