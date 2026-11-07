// A temperature -> duty curve editor: drag the points, double-click empty space to add one, right-click
// (or Delete) to remove one, arrow keys to nudge the focused point. The widget draws and routes input;
// every rule (grid, neighbours, "100 % by the limit", hit-testing, insertion, the pixel mapping) is
// answered by `engine`, so the same rules serve the page, the tests and any other front end.
//
//   CurveEditor {
//       engine: vm                      // an object with call(verb, args) -> result (see below)
//       side: "cpu"; points: draft.cpu; otherPoints: draft.gpu
//       nowTemp: Live.value("sensors.cpu_temp")
//       onEditStarted: undo.push()      // once per drag / key step / insert / remove, before `edited`
//       onEdited: (pts) => draft.cpu = pts
//       onRefused: (why) => Toasts.show(why, "info")
//   }
//
// `engine.call(verb, args)` returns the result object or null; the verbs used are `geometry`, `hit`,
// `drag`, `nudge`, `insert`, `insert_middle` and `remove` (ts-ui-model `vm/cooling.rs`; for a refused
// edit it throws or returns `{error}`: the editor shows the text via `refused`). The page passes an
// adapter around `Bus.vm("cooling", ...)`, which keeps this file free of the Rust side (component tests
// run with a fake).
//
// The chart never mirrors in a right-to-left layout (temperature always grows to the right).
//
// Drawing: scene-graph items (QtQuick.Shapes for the grid and the curves, Rectangle and Text for the
// rest), no script-drawn Canvas. The chart has two layers that change at different rates. The static
// layer (the Auto band, grid, labels, limit line and the two curves) is rebuilt only when `geo`
// changes: the points, the other side, the size. The live layer (the vertical "now" line, its dot
// and its label) moves with `nowTemp`, which ticks every second; moving two rectangles never
// touches the static layer. `drawCount` counts static rebuilds so a test can pin that.
import QtQuick
import QtQuick.Shapes
import org.truesense.app

Item {
    id: root

    /// Where the rules come from (see above).
    property var engine: null
    /// The side being edited, [[temperature, duty], ...] sorted by temperature.
    property var points: []
    /// The other side, drawn faint for comparison.
    property var otherPoints: []
    /// "cpu" or "gpu" (the 100 % limit differs).
    property string side: "cpu"
    /// A locked curve is drawn and cannot be changed.
    property bool locked: false
    /// The live temperature of this side (NaN = unknown): a marker and the duty the curve asks for.
    property real nowTemp: NaN
    property color lineColor: Theme.accent
    property color otherColor: Theme.muted
    /// The point that has the focus or is being dragged (-1: none).
    readonly property alias selected: state.selected
    readonly property bool dragging: state.dragIndex >= 0
    /// The text of the "Auto" band (duty 0: the firmware runs the fan).
    //% "Auto"
    property string autoText: qsTrId("curve_auto")
    /// "100 % by 95 °C" with the limit filled in by %1.
    //% "100 % by %1"
    property string limitText: qsTrId("curve_limit")
    //% "Right now"
    property string nowText: qsTrId("curve_now")
    //% "Fan curve graph"
    property string accessibleName: qsTrId("curve_graph")

    /// A change is about to be applied (the page saves an undo step).
    signal editStarted()
    /// The new points of `side` after an edit.
    signal edited(var points)
    /// An edit that was not possible, with the reason.
    signal refused(string reason)

    implicitWidth: 360
    implicitHeight: Math.max(220, Math.min(340, Math.round(width * 0.52)))
    Accessible.role: Accessible.Grouping
    Accessible.name: accessibleName
    LayoutMirroring.enabled: false
    LayoutMirroring.childrenInherit: true

    readonly property int _marginLeft: 12 + Math.ceil(4 * Theme.fontSmall * 0.62)
    readonly property var _margins: [_marginLeft, 14, 14, Theme.fontSmall + 18]
    /// Everything static: the grid, the ticks, the limit, the curves and the dots (no live marker).
    readonly property var geo: _geometry(points, otherPoints, side, width, height, NaN, _margins)
    /// The live marker on this side's curve ({x, y, temp, duty}), or null. Changes with `nowTemp`
    /// only; the static layer does not depend on it.
    readonly property var nowAt: {
        if (isNaN(nowTemp) || geo === null) return null
        var g = _geometry(points, otherPoints, side, width, height, nowTemp, _margins)
        return (g === null || g.now === undefined) ? null : g.now
    }
    /// How often the static layer was rebuilt (for tests: a new `nowTemp` must not count).
    property int drawCount: 0
    onGeoChanged: drawCount++

    function _answer(verb, args) {
        if (engine === null) return null
        try {
            var r = engine.call(verb, args)
            return r === undefined ? null : r
        } catch (e) {
            return { error: String(e) }
        }
    }

    function _geometry(pts, other, sd, w, h, temp, margins) {
        if (engine === null || w < 10 || h < 10) return null
        var r = _answer("geometry", { points: pts, other: other, side: sd, width: w, height: h,
                                      margins: margins, temp: isNaN(temp) ? null : temp })
        return (r === null || r.error !== undefined) ? null : r
    }

    function _args(extra) {
        var a = { points: points, side: side, width: width, height: height, margins: _margins }
        for (var k in extra) a[k] = extra[k]
        return a
    }

    function _apply(r, focusIndex) {
        if (r === null) return false
        if (r.error !== undefined) { refused(r.error); return false }
        editStarted()
        edited(r.points)
        state.selected = focusIndex === undefined ? r.index : focusIndex
        Qt.callLater(root._focusSelected)
        return true
    }

    function _focusSelected() {
        var item = dots.itemAt(state.selected)
        if (item) item.forceActiveFocus()
    }

    /// The point under (x, y) in item coordinates, or -1.
    function hitAt(x, y) {
        var r = _answer("hit", _args({ x: x, y: y }))
        return (typeof r === "number") ? r : -1
    }

    /// Adds a point at temperature `t` (on the existing line).
    function insertAt(t) {
        if (locked) return false
        return _apply(_answer("insert", _args({ t: t })))
    }

    /// Adds a point in the middle of the widest gap (the "Add point" button).
    function addMiddle() {
        if (locked) return false
        return _apply(_answer("insert_middle", _args({})))
    }

    function removeAt(i) {
        if (locked) return false
        return _apply(_answer("remove", _args({ index: i })))
    }

    function _nudge(i, dt, dd) {
        var r = _answer("nudge", _args({ index: i, dt: dt, dd: dd }))
        if (r === null || r.error !== undefined) { if (r !== null) refused(r.error); return }
        if (JSON.stringify(r.points) === JSON.stringify(points)) return
        _apply(r, i)
    }

    QtObject {
        id: state
        property int selected: -1
        property int dragIndex: -1
        property int hover: -1
        property bool started: false
        /// A point has the keyboard focus (the readout shows next to it).
        property bool focusOn: false
    }

    property color _grid: Theme.chartGrid
    property color _band: Theme.tile
    property color _text: Theme.muted

    function _pts(line) {
        var out = []
        if (!line || line.length < 2) return out
        for (var i = 0; i < line.length; ++i) out.push(Qt.point(line[i][0], line[i][1]))
        return out
    }
    function _alpha(c, a) { var k = Qt.color(c); return Qt.rgba(k.r, k.g, k.b, a) }

    // ---- the static layer ------------------------------------------------------------------------
    Item {
        id: chart
        anchors.fill: parent
        visible: root.geo !== null
        readonly property var g: root.geo
        readonly property real pl: g ? g.plot.left : 0
        readonly property real pr: g ? g.plot.right : 0
        readonly property real pt: g ? g.plot.top : 0
        readonly property real pb: g ? g.plot.bottom : 0
        readonly property real bandTop: g ? g.auto_band.top : 0
        readonly property real bandBottom: g ? g.auto_band.bottom : 0

        // the Auto band: duty 0 is "the firmware runs the fan"
        Rectangle {
            x: chart.pl
            y: chart.bandTop
            width: chart.pr - chart.pl
            height: chart.bandBottom - chart.bandTop
            color: root._band
        }
        Text {
            x: chart.pl + 6
            y: (chart.bandTop + chart.bandBottom) / 2 - height / 2
            text: root.autoText
            color: root._text
            font.pixelSize: Theme.fontSmall
        }

        Shape {
            anchors.fill: parent
            preferredRendererType: Shape.CurveRenderer
            // grid lines
            ShapePath {
                strokeColor: root._grid
                strokeWidth: 1
                fillColor: "transparent"
                PathMultiline {
                    paths: {
                        var g = chart.g
                        if (!g) return []
                        var out = []
                        for (var i = 0; i < g.y_ticks.length; ++i) {
                            var ty = Math.round(g.y_ticks[i].px) + 0.5
                            out.push([Qt.point(chart.pl, ty), Qt.point(chart.pr, ty)])
                        }
                        for (var j = 0; j < g.x_ticks.length; ++j) {
                            var tx = Math.round(g.x_ticks[j].px) + 0.5
                            out.push([Qt.point(tx, chart.pt), Qt.point(tx, chart.pb)])
                        }
                        return out
                    }
                }
            }
            // where this side must be at 100 %
            ShapePath {
                strokeColor: root._text
                strokeWidth: 1
                strokeStyle: ShapePath.DashLine
                dashPattern: [5, 4]
                fillColor: "transparent"
                PathMultiline {
                    paths: chart.g ? [[Qt.point(chart.g.limit.x, chart.pt), Qt.point(chart.g.limit.x, chart.pb)]] : []
                }
            }
            // the other side, faint
            ShapePath {
                strokeColor: root._alpha(root.otherColor, 0.55)
                strokeWidth: 1.5
                fillColor: "transparent"
                joinStyle: ShapePath.RoundJoin
                PathPolyline { path: chart.g ? root._pts(chart.g.other_line) : [] }
            }
            // this side
            ShapePath {
                strokeColor: root._alpha(root.lineColor, root.locked ? 0.6 : 1)
                strokeWidth: 3
                fillColor: "transparent"
                joinStyle: ShapePath.RoundJoin
                PathPolyline { path: chart.g ? root._pts(chart.g.line) : [] }
            }
        }

        Repeater {
            model: chart.g ? chart.g.y_ticks.length : 0
            delegate: Text {
                required property int index
                readonly property var tick: chart.g ? chart.g.y_ticks[index] : null
                x: chart.pl - 6 - width
                y: (tick ? Math.round(tick.px) + 0.5 : 0) - height / 2
                text: tick ? tick.value + "%" : ""
                color: root._text
                font.pixelSize: Theme.fontSmall
            }
        }
        Repeater {
            model: chart.g ? chart.g.x_ticks.length : 0
            delegate: Text {
                required property int index
                readonly property var tick: chart.g ? chart.g.x_ticks[index] : null
                x: (tick ? Math.round(tick.px) + 0.5 : 0) - width / 2
                y: chart.pb + 5
                text: tick ? tick.value + "°" : ""
                color: root._text
                font.pixelSize: Theme.fontSmall
            }
        }
        Text {
            x: (chart.g ? chart.g.limit.x : 0) - 4 - width
            y: chart.pt + 3
            text: chart.g ? root.limitText.arg(chart.g.limit.temp + " °C") : ""
            color: root._text
            font.pixelSize: Theme.fontSmall
        }
    }

    // ---- the live layer: the line at the live temperature and the dot on the curve -----------------
    Rectangle {
        visible: root.nowAt !== null && root.geo !== null
        x: root.nowAt ? Math.round(root.nowAt.x) : 0
        y: chart.pt
        width: 1
        height: chart.pb - chart.pt
        color: root._text
    }
    Rectangle {
        visible: root.nowAt !== null
        x: root.nowAt ? root.nowAt.x - 4 : 0
        y: root.nowAt ? root.nowAt.y - 4 : 0
        width: 8
        height: 8
        radius: 4
        color: root.lineColor
    }

    // The label of the live marker, in a Text so it follows the theme, the font scale and RTL text.
    Text {
        visible: root.geo !== null && root.nowAt !== null
        readonly property var now: root.nowAt
        x: now ? Math.max(root.geo.plot.left + 2, Math.min(root.geo.plot.right - width - 2, now.x + 8)) : 0
        y: now ? Math.max(root.geo.plot.top + Theme.fontSmall + 8, now.y - height - 8) : 0
        text: now ? (root.nowText + ": " + Fmt.temp(now.temp) + " → " + (now.duty === 0 ? root.autoText : Fmt.percent(now.duty))) : ""
        color: Theme.stat
        font.pixelSize: Theme.fontSmall
    }

    Repeater {
        id: dots
        model: root.geo === null ? 0 : root.geo.dots.length
        delegate: Rectangle {
            id: dot
            required property int index
            readonly property var px: root.geo.dots[index]
            readonly property var pt: root.points[index]
            readonly property bool current: root.selected === index && (activeFocus || root.dragging)
            x: px[0] - width / 2
            y: px[1] - height / 2
            width: current ? 18 : 14
            height: width
            radius: width / 2
            color: current ? root.lineColor : Theme.card
            border.width: 2
            border.color: activeFocus ? Theme.focusRing : root.lineColor
            opacity: root.locked ? 0.6 : 1
            activeFocusOnTab: !root.locked
            Accessible.role: Accessible.Slider
            //% "Point %1: %2, %3"
            Accessible.name: qsTrId("curve_point_name").arg(index + 1).arg(pt ? Fmt.temp(pt[0]) : "")
                .arg(pt ? (pt[1] === 0 ? root.autoText : Fmt.percent(pt[1])) : "")
            onActiveFocusChanged: {
                if (activeFocus) { state.selected = index; state.focusOn = true }
                else if (state.selected === index) state.focusOn = false
            }

            Keys.onPressed: (event) => {
                if (root.locked) return
                var step = ({ [Qt.Key_Left]: [-1, 0], [Qt.Key_Right]: [1, 0],
                              [Qt.Key_Up]: [0, 1], [Qt.Key_Down]: [0, -1] })[event.key]
                if (step !== undefined) {
                    root._nudge(index, step[0], step[1])
                    event.accepted = true
                } else if (event.key === Qt.Key_Delete || event.key === Qt.Key_Backspace) {
                    root.removeAt(index)
                    event.accepted = true
                }
            }
        }
    }

    // What the point being dragged or focused says, next to it.
    Rectangle {
        readonly property var pt: (root.selected >= 0 && root.selected < root.points.length) ? root.points[root.selected] : null
        readonly property var px: (root.geo !== null && root.selected >= 0 && root.selected < root.geo.dots.length) ? root.geo.dots[root.selected] : null
        visible: pt !== null && px !== null && (root.dragging || state.focusOn)
        radius: Theme.radiusSmall
        color: Theme.dialog
        border.color: Theme.cardLine
        width: readout.implicitWidth + 12
        height: readout.implicitHeight + 6
        x: px ? Math.max(0, Math.min(root.width - width, px[0] - width / 2)) : 0
        y: px ? (px[1] > height + 30 ? px[1] - height - 14 : px[1] + 14) : 0
        Text {
            id: readout
            anchors.centerIn: parent
            text: parent.pt ? (Fmt.temp(parent.pt[0]) + " · " + (parent.pt[1] === 0 ? root.autoText : Fmt.percent(parent.pt[1]))) : ""
            color: Theme.text
            font.pixelSize: Theme.fontSmall
        }
    }

    MouseArea {
        anchors.fill: parent
        hoverEnabled: true
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        cursorShape: root.locked ? Qt.ArrowCursor
            : (state.dragIndex >= 0 ? Qt.ClosedHandCursor : (state.hover >= 0 ? Qt.OpenHandCursor : Qt.CrossCursor))

        onPressed: (mouse) => {
            if (root.locked) return
            var i = root.hitAt(mouse.x, mouse.y)
            if (i < 0) return
            state.selected = i
            var item = dots.itemAt(i)
            if (item) item.forceActiveFocus()
            if (mouse.button === Qt.RightButton) {
                root.removeAt(i)
                return
            }
            state.dragIndex = i
            state.started = false
        }
        onPositionChanged: (mouse) => {
            if (root.locked) return
            if (state.dragIndex < 0) {
                state.hover = root.hitAt(mouse.x, mouse.y)
                return
            }
            var r = root._answer("drag", root._args({ index: state.dragIndex, x: mouse.x, y: mouse.y }))
            if (r === null || r.error !== undefined) return
            if (JSON.stringify(r.points) === JSON.stringify(root.points)) return
            if (!state.started) { state.started = true; root.editStarted() }
            root.edited(r.points)
        }
        onReleased: state.dragIndex = -1
        onCanceled: state.dragIndex = -1
        onDoubleClicked: (mouse) => {
            if (root.locked || mouse.button !== Qt.LeftButton) return
            if (root.hitAt(mouse.x, mouse.y) >= 0) return
            var r = root._answer("insert", root._args({ x: mouse.x }))
            root._apply(r)
        }
    }
}
