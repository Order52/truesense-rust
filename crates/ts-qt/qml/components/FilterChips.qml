// A row of small filter buttons, one of them chosen: "All 32", "Gaming 5", "Starred 2". The chosen
// chip is filled; a chip that would show nothing stays visible and greyed (so the row does not jump
// while a search narrows it). `picked` fires only for the person's own click.
//
//   FilterChips { model: [{ id: "all", label: "All", count: 32 }]; current: "all"; onPicked: (id) => ... }
import QtQuick
import QtQuick.Controls.Basic
import org.truesense.app

Flow {
    id: root

    /// [{id, label, count?}]; `count` is left out of the button when absent.
    property var model: []
    property string current: ""
    signal picked(string id)

    spacing: 6

    Repeater {
        model: root.model
        delegate: Button {
            id: chip
            required property var modelData
            readonly property bool chosen: modelData.id === root.current
            readonly property bool empty: modelData.count !== undefined && modelData.count === 0 && !chosen

            implicitHeight: 30
            implicitWidth: label.implicitWidth + 24
            enabled: !empty
            hoverEnabled: true
            focusPolicy: Qt.StrongFocus
            Accessible.role: Accessible.RadioButton
            Accessible.name: modelData.count !== undefined ? modelData.label + " " + modelData.count : modelData.label
            Accessible.checkable: true
            Accessible.checked: chosen
            onClicked: root.picked(modelData.id)

            contentItem: Text {
                id: label
                text: chip.modelData.count !== undefined ? chip.modelData.label + "  " + chip.modelData.count : chip.modelData.label
                color: !chip.enabled ? Theme.disabledText : chip.chosen ? Theme.accentInk : Theme.stat
                font.pixelSize: Theme.fontSmall + 1
                font.bold: chip.chosen
                horizontalAlignment: Text.AlignHCenter
                verticalAlignment: Text.AlignVCenter
            }
            background: Rectangle {
                radius: height / 2
                color: chip.chosen ? Theme.accent : (chip.hovered && chip.enabled ? Theme.tileHover : Theme.button)
                border.width: 1
                border.color: chip.activeFocus ? Theme.focusRing : (chip.chosen ? Theme.accent : Theme.buttonLine)
            }
        }
    }
}
