// A dial for one measured value (temperature, load): a 270 degree arc that fills with the value and
// changes colour at `warnAt` and `critAt` (the colour is never the only signal: the number is
// always shown). The needle eases to a new value unless reduced motion is on.
//
//   Gauge { value: 54; from: 0; to: 110; warnAt: 80; critAt: 95; text: "54 °C"; caption: "CPU" }
import QtQuick
import org.truesense.app

Item {
    id: root

    property real value: 0
    property real from: 0
    property real to: 100
    property real warnAt: NaN
    property real critAt: NaN
    /// The big text in the middle ("54 °C"); empty shows the rounded value.
    property string text: ""
    /// The small text under it ("CPU").
    property string caption: ""
    /// The value drawn (eased).
    property real shown: value
    readonly property real fraction: Math.max(0, Math.min(1, (shown - from) / Math.max(1e-9, to - from)))
    readonly property color zoneColor: (!isNaN(critAt) && value >= critAt) ? Theme.bad
        : (!isNaN(warnAt) && value >= warnAt) ? Theme.warn : Theme.accent

    implicitWidth: 140
    implicitHeight: 140
    Accessible.role: Accessible.ProgressBar
    Accessible.name: caption
    Accessible.description: text

    Behavior on shown { NumberAnimation { duration: Theme.animMs * 3; easing.type: Easing.OutCubic } }
    onFractionChanged: canvas.requestPaint()
    onZoneColorChanged: canvas.requestPaint()
    onWidthChanged: canvas.requestPaint()
    onHeightChanged: canvas.requestPaint()

    Canvas {
        id: canvas
        anchors.fill: parent
        LayoutMirroring.enabled: false
        onPaint: {
            var ctx = getContext("2d")
            ctx.reset()
            var size = Math.min(width, height)
            var lw = Math.max(8, size * 0.09)
            var cx = width / 2, cy = height / 2
            var r = size / 2 - lw / 2 - 2
            var start = 0.75 * Math.PI, sweep = 1.5 * Math.PI
            ctx.lineWidth = lw
            ctx.lineCap = "round"
            ctx.strokeStyle = Theme.groove
            ctx.beginPath()
            ctx.arc(cx, cy, r, start, start + sweep)
            ctx.stroke()
            if (root.fraction > 0.001) {
                ctx.strokeStyle = root.zoneColor
                ctx.beginPath()
                ctx.arc(cx, cy, r, start, start + sweep * root.fraction)
                ctx.stroke()
            }
        }
    }

    Column {
        anchors.centerIn: parent
        spacing: 2
        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            text: root.text !== "" ? root.text : String(Math.round(root.value))
            color: Theme.text
            font.pixelSize: Theme.fs(Math.max(14, root.width * 0.17))
            font.bold: true
        }
        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            visible: root.caption !== ""
            text: root.caption
            color: Theme.muted
            font.pixelSize: Theme.fontSmall
        }
    }
}
