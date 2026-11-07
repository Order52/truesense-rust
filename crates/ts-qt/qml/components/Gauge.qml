// A dial for one measured value (temperature, load): a 270 degree arc that fills with the value and
// changes colour at `warnAt` and `critAt` (the colour is never the only signal: the number is
// always shown).
//
// It is two scene-graph arcs (QtQuick.Shapes), not a Canvas: a new value moves a vertex, nothing is
// painted by script and no image is uploaded. The value SNAPS: the live pages hand it a new reading
// every second, and an easing between two readings kept the whole window rendering at the display's
// refresh rate (165 frames a second on a fast panel) for nothing. A caller that wants an eased
// change (a value the person just set) sets `easeMs`, capped at 200.
//
//   Gauge { value: 54; from: 0; to: 110; warnAt: 80; critAt: 95; text: "54 °C"; caption: "CPU" }
import QtQuick
import QtQuick.Shapes
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
    /// Easing time for a change of `value`; 0 (the default) snaps. At most 200, and 0 with reduced motion.
    property int easeMs: 0
    /// The value drawn.
    property real shown: value
    readonly property real fraction: Math.max(0, Math.min(1, (shown - from) / Math.max(1e-9, to - from)))
    readonly property color zoneColor: (!isNaN(critAt) && value >= critAt) ? Theme.bad
        : (!isNaN(warnAt) && value >= warnAt) ? Theme.warn : Theme.accent
    readonly property real _size: Math.min(width, height)
    readonly property real _lw: Math.max(8, _size * 0.09)
    readonly property real _r: Math.max(1, _size / 2 - _lw / 2 - 2)

    implicitWidth: 140
    implicitHeight: 140
    Accessible.role: Accessible.ProgressBar
    Accessible.name: caption
    Accessible.description: text

    Behavior on shown {
        enabled: root.easeMs > 0 && !Ui.reducedMotion
        NumberAnimation { duration: Math.min(200, root.easeMs); easing.type: Easing.OutCubic }
    }

    Shape {
        anchors.fill: parent
        // The dial's direction is not text direction.
        LayoutMirroring.enabled: false
        preferredRendererType: Shape.CurveRenderer
        ShapePath {
            strokeColor: Theme.groove
            strokeWidth: root._lw
            fillColor: "transparent"
            capStyle: ShapePath.RoundCap
            PathAngleArc {
                centerX: root.width / 2; centerY: root.height / 2
                radiusX: root._r; radiusY: root._r
                startAngle: 135; sweepAngle: 270
            }
        }
        ShapePath {
            strokeColor: root.fraction > 0.001 ? root.zoneColor : "transparent"
            strokeWidth: root._lw
            fillColor: "transparent"
            capStyle: ShapePath.RoundCap
            PathAngleArc {
                centerX: root.width / 2; centerY: root.height / 2
                radiusX: root._r; radiusY: root._r
                startAngle: 135; sweepAngle: 270 * root.fraction
            }
        }
    }

    Column {
        anchors.centerIn: parent
        spacing: 2
        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            // Never wider than the inside of the ring: a long text ("5324 RPM") shrinks instead of
            // running over the arc.
            width: Math.max(10, 2 * root._r - root._lw * 1.3)
            horizontalAlignment: Text.AlignHCenter
            text: root.text !== "" ? root.text : String(Math.round(root.value))
            color: Theme.text
            font.pixelSize: Theme.fs(Math.max(14, root.width * 0.17))
            minimumPixelSize: 9
            fontSizeMode: Text.HorizontalFit
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
