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
// * It repaints when `series`, the scale, the size or a colour changes and at no other time: no
//   timer, no animation, so a hidden page costs nothing. A page must assign a new array to change
//   the data (an in-place change raises no signal).
// * Never mirrored: time runs left to right in every language (the legend above follows the
//   language's direction).
import QtQuick
import QtQuick.Layouts
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
    /// How often the canvas painted (for tests).
    property int paintCount: 0

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

    onSeriesChanged: canvas.requestPaint()
    onMinimumChanged: canvas.requestPaint()
    onMaximumChanged: canvas.requestPaint()
    onZeroBasedChanged: canvas.requestPaint()
    onCapacityChanged: canvas.requestPaint()
    onGridChanged: canvas.requestPaint()

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

        Canvas {
            id: canvas
            x: plot.labelWidth
            width: plot.width - plot.labelWidth
            height: plot.height
            renderTarget: Canvas.Image
            onPainted: root.paintCount++
            onWidthChanged: requestPaint()
            onHeightChanged: requestPaint()
            Connections {
                target: Theme
                function onDarkChanged() { canvas.requestPaint() }
            }
            onPaint: {
                var ctx = getContext("2d")
                ctx.reset()
                if (width <= 0 || height <= 0) return
                var r = root._range(root.series)
                var padTop = 6, padBottom = 6, padRight = 6
                var plotH = height - padTop - padBottom
                var plotW = width - padRight
                function yOf(v) { return padTop + plotH * (1 - (v - r.lo) / (r.hi - r.lo)) }

                // grid
                ctx.lineWidth = 1
                ctx.strokeStyle = Theme.chartGrid
                var g = root.grid
                var lines = g.length ? g.map(function (e) { return e.value }) : [r.lo, (r.lo + r.hi) / 2, r.hi]
                for (var k = 0; k < lines.length; ++k) {
                    var gy = Math.round(yOf(lines[k])) + 0.5
                    ctx.beginPath()
                    ctx.moveTo(0, gy)
                    ctx.lineTo(plotW, gy)
                    ctx.stroke()
                }

                var longest = 0
                for (var s0 = 0; s0 < root.series.length; ++s0)
                    longest = Math.max(longest, (root.series[s0].values || []).length)
                var span = Math.max(2, root.capacity > 0 ? root.capacity : longest)

                ctx.lineJoin = "round"
                ctx.lineCap = "round"
                for (var si = 0; si < root.series.length; ++si) {
                    var spec = root.series[si]
                    var a = spec.values || []
                    var n = a.length
                    if (n < 1) continue
                    var color = spec.color
                    // x of sample i: the newest sample is at the right edge
                    var xOf = function (i) { return plotW - (n - 1 - i) * plotW / (span - 1) }
                    var seg = []
                    var runs = []
                    for (var i = 0; i < n; ++i) {
                        if (root._isNum(a[i])) seg.push(i)
                        else if (seg.length) { runs.push(seg); seg = [] }
                    }
                    if (seg.length) runs.push(seg)
                    for (var ri = 0; ri < runs.length; ++ri) {
                        var run = runs[ri]
                        if (spec.filled && run.length > 1) {
                            ctx.beginPath()
                            ctx.moveTo(xOf(run[0]), padTop + plotH)
                            for (var f = 0; f < run.length; ++f) ctx.lineTo(xOf(run[f]), yOf(a[run[f]]))
                            ctx.lineTo(xOf(run[run.length - 1]), padTop + plotH)
                            ctx.closePath()
                            ctx.globalAlpha = 0.16
                            ctx.fillStyle = color
                            ctx.fill()
                            ctx.globalAlpha = 1
                        }
                        ctx.beginPath()
                        for (var j = 0; j < run.length; ++j) {
                            var x = xOf(run[j]), y = yOf(a[run[j]])
                            if (j === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y)
                        }
                        ctx.lineWidth = root.lineWidth
                        ctx.strokeStyle = color
                        ctx.stroke()
                        // a lone sample has no line: draw it as a dot
                        if (run.length === 1) {
                            ctx.beginPath()
                            ctx.arc(xOf(run[0]), yOf(a[run[0]]), root.lineWidth + 0.6, 0, 2 * Math.PI)
                            ctx.fillStyle = color
                            ctx.fill()
                        }
                    }
                    // the newest sample, when there is one
                    if (root._isNum(a[n - 1]) && runs[runs.length - 1].length > 1) {
                        ctx.beginPath()
                        ctx.arc(xOf(n - 1), yOf(a[n - 1]), root.lineWidth + 0.8, 0, 2 * Math.PI)
                        ctx.fillStyle = color
                        ctx.fill()
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
                    var r = root._range(root.series)
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
