// A row of colour swatches (kept colours, presets). `activated(color)` is the person's pick;
// `customRequested()` is the "+" tile, for the page to open a colour dialog.
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import org.truesense.app

SettingRow {
    id: root

    /// Colours as "#rrggbb" strings.
    property var colors: []
    property string current: ""
    property bool allowCustom: false
    property int swatchSize: 30
    signal activated(string color)
    signal customRequested()

    Flow {
        Layout.fillWidth: true
        spacing: 8

        Repeater {
            model: root.colors
            delegate: Button {
                id: sw
                HoverHandler { cursorShape: parent.enabled ? Qt.PointingHandCursor : Qt.ArrowCursor }
                required property string modelData
                readonly property bool selected: root.current.toLowerCase() === modelData.toLowerCase()
                width: root.swatchSize
                height: root.swatchSize
                padding: 0
                hoverEnabled: true
                focusPolicy: Qt.StrongFocus
                Accessible.role: Accessible.RadioButton
                Accessible.name: modelData
                Accessible.checked: selected
                onClicked: root.activated(modelData)
                background: Rectangle {
                    radius: width / 2
                    color: sw.modelData
                    border.width: sw.selected || sw.activeFocus ? 3 : 1
                    border.color: sw.selected ? Theme.accent : (sw.activeFocus ? Theme.focusRing : Theme.swatchLine)
                    scale: sw.hovered ? 1.08 : 1
                    Behavior on scale { NumberAnimation { duration: Theme.animMs } }
                }
            }
        }
        Button {
            visible: root.allowCustom
            width: root.swatchSize
            height: root.swatchSize
            padding: 0
            focusPolicy: Qt.StrongFocus
            //% "Choose another colour"
            Accessible.name: qsTrId("swatch_custom")
            onClicked: root.customRequested()
            contentItem: Icon {
                name: "plus"
                size: 16
                color: Theme.muted
            }
            background: Rectangle {
                radius: width / 2
                color: "transparent"
                border.width: 1
                border.color: Theme.swatchLine
            }
        }
    }
}
