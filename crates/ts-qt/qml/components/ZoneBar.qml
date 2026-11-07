// A thin bar under a slider that shows where the safe, caution and danger zones are and where the
// current value sits, with the three words under it so the zones never rely on colour alone.
//
//   ZoneBar { from: 0; to: 120; safeUntil: 60; cautionUntil: 90; value: coreCut }
//
// `value` is compared on the same scale as `from` .. `to`; the zone edges are where a value stops
// being safe (`safeUntil`) and where it becomes dangerous (`cautionUntil`).
import QtQuick
import QtQuick.Layouts
import org.truesense.app

ColumnLayout {
    id: root

    property real from: 0
    property real to: 100
    property real safeUntil: 50
    property real cautionUntil: 80
    property real value: 0

    spacing: 3
    Layout.fillWidth: true
    Accessible.role: Accessible.StaticText
    Accessible.name: (value <= safeUntil
        //% "Safe zone"
        ? qsTrId("zone_safe")
        : value <= cautionUntil
        //% "Caution zone"
        ? qsTrId("zone_caution")
        //% "Danger zone"
        : qsTrId("zone_danger"))

    function _x(v, w) { return Math.max(0, Math.min(1, (v - from) / Math.max(1e-9, to - from))) * w }

    Item {
        id: bar
        Layout.fillWidth: true
        implicitHeight: 8
        LayoutMirroring.enabled: false

        Rectangle { x: 0; width: root._x(root.safeUntil, bar.width); height: 4; y: 2; radius: 2; color: Theme.ok; opacity: 0.8 }
        Rectangle {
            x: root._x(root.safeUntil, bar.width)
            width: root._x(root.cautionUntil, bar.width) - x
            height: 4; y: 2; color: Theme.warn; opacity: 0.8
        }
        Rectangle {
            x: root._x(root.cautionUntil, bar.width)
            width: bar.width - x
            height: 4; y: 2; radius: 2; color: Theme.bad; opacity: 0.8
        }
        Rectangle {
            x: root._x(root.value, bar.width) - 1
            width: 3; height: 8; radius: 1
            color: Theme.text
        }
    }
    RowLayout {
        Layout.fillWidth: true
        LayoutMirroring.enabled: false
        Text {
            //% "Safe"
            text: qsTrId("risk_safe")
            color: Theme.muted
            font.pixelSize: Theme.fontSmall
        }
        Item { Layout.fillWidth: true }
        Text {
            //% "Caution"
            text: qsTrId("risk_caution")
            color: Theme.muted
            font.pixelSize: Theme.fontSmall
        }
        Item { Layout.fillWidth: true }
        Text {
            //% "Danger"
            text: qsTrId("risk_danger")
            color: Theme.muted
            font.pixelSize: Theme.fontSmall
        }
    }
}
