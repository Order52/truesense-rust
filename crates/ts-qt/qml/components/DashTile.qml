// One tile of the Home dashboard: a name, a big dial (or, for the power mode, a big word and its
// icon), a line under it and the reading's history. The page decides which tiles show and in what
// order (ui.json `home_tiles`, the view-model `home dashboard`); this only draws one.
//
//   DashTile { label: "CPU"; icon: "chip"; kind: "dial"; value: 54; to: 105; warnAt: 85; critAt: 95
//              valueText: "54 °C"; subText: "Load 12 %"; values: Live.history("sensors.cpu_temp") }
//
// Nothing here animates by itself: the dial snaps to each reading (see Gauge), the sparkline repaints
// at most every `minRepaintMs`.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

Rectangle {
    id: root

    property string label: ""
    property string icon: ""
    /// "dial" or "mode".
    property string kind: "dial"
    property real value: 0
    property bool known: true
    property real from: 0
    property real to: 100
    property real warnAt: NaN
    property real critAt: NaN
    property string valueText: "–"
    property string subText: ""
    /// The mode tile's icon (the power mode's own).
    property string modeIcon: ""
    /// History for the sparkline; empty hides it.
    property var values: []
    property bool hasHistory: true
    /// The sparkline's fixed bottom, or NaN to follow the data (temperatures).
    property real historyMinimum: NaN
    property real historyMaximum: NaN
    property int minRepaintMs: 3000
    property bool hot: false
    /// Clicking the tile (the mode tile steps to the next power mode); none when not set.
    property bool clickable: false
    signal clicked()

    implicitHeight: col.implicitHeight + 2 * Theme.pad
    implicitWidth: 180
    radius: Theme.radiusLarge
    color: area.containsMouse && root.clickable ? Theme.tileHover : Theme.card
    border.width: 1
    border.color: root.hot ? Theme.bad : (area.activeFocus ? Theme.focusRing : Theme.cardLine)
    Accessible.role: root.clickable ? Accessible.Button : Accessible.StaticText
    Accessible.name: label + " " + valueText + (subText !== "" ? ", " + subText : "")

    // A thin accent bar on top: the dashboard's signature, in the accent (or the warning) colour.
    Rectangle {
        x: Theme.radiusLarge
        width: parent.width - 2 * Theme.radiusLarge
        height: 2
        color: root.hot ? Theme.bad : Theme.accent
        opacity: 0.85
    }

    MouseArea {
        id: area
        anchors.fill: parent
        enabled: root.clickable
        hoverEnabled: root.clickable
        activeFocusOnTab: root.clickable
        cursorShape: root.clickable ? Qt.PointingHandCursor : Qt.ArrowCursor
        onClicked: root.clicked()
        Keys.onReturnPressed: root.clicked()
        Keys.onSpacePressed: root.clicked()
    }

    ColumnLayout {
        id: col
        x: Theme.pad
        y: Theme.pad
        width: root.width - 2 * Theme.pad
        spacing: 6

        RowLayout {
            Layout.fillWidth: true
            spacing: 8
            Icon { name: root.icon; size: 16; color: root.hot ? Theme.bad : Theme.accent }
            Text {
                Layout.fillWidth: true
                text: root.label.toUpperCase()
                color: Theme.stat
                font.pixelSize: Theme.fontSmall
                font.bold: true
                font.letterSpacing: 0.8
                elide: Text.ElideRight
            }
        }

        // The reading: a dial, or the power mode as a word.
        Item {
            Layout.fillWidth: true
            Layout.preferredHeight: dialSize
            readonly property real dialSize: Math.max(90, Math.min(140, col.width * 0.72))
            Gauge {
                visible: root.kind === "dial"
                anchors.horizontalCenter: parent.horizontalCenter
                width: parent.dialSize
                height: parent.dialSize
                value: root.known ? root.value : root.from
                from: root.from
                to: root.to
                warnAt: root.warnAt
                critAt: root.critAt
                text: root.valueText
            }
            ColumnLayout {
                visible: root.kind === "mode"
                anchors.centerIn: parent
                spacing: 8
                Icon {
                    Layout.alignment: Qt.AlignHCenter
                    name: root.modeIcon
                    size: 44
                    color: Theme.accent
                }
                Text {
                    Layout.alignment: Qt.AlignHCenter
                    text: root.valueText
                    color: Theme.text
                    font.pixelSize: Theme.fontBig
                    font.bold: true
                }
            }
        }

        Text {
            Layout.fillWidth: true
            horizontalAlignment: Text.AlignHCenter
            text: root.subText
            color: root.hot ? Theme.bad : Theme.muted
            font.pixelSize: Theme.fontSmall
            elide: Text.ElideRight
            // Keeps every tile of a row the same height when one has no second line.
            Layout.minimumHeight: Theme.fontSmall + 4
        }
        Sparkline {
            Layout.fillWidth: true
            Layout.preferredHeight: 26
            visible: root.hasHistory
            values: root.values
            minRepaintMs: root.minRepaintMs
            minimum: root.historyMinimum
            maximum: root.historyMaximum
            lineColor: root.hot ? Theme.bad : Theme.chartLine
        }
        Item {
            visible: !root.hasHistory
            Layout.preferredHeight: 26
        }
    }
}
