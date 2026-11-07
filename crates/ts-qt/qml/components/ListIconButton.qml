// A small square icon button for list rows (move up, move down, remove). `name` is its accessible
// name (there is no visible text); `tone: "danger"` turns the icon red on hover.
import QtQuick
import QtQuick.Controls.Basic
import org.truesense.app

Button {
    id: root

    property string iconName: ""
    property string tone: ""
    property int iconRotation: 0

    implicitWidth: 34
    implicitHeight: 34
    hoverEnabled: true
    focusPolicy: Qt.StrongFocus
    Accessible.role: Accessible.Button
    Accessible.name: text

    contentItem: Item {
        Icon {
            anchors.centerIn: parent
            name: root.iconName
            size: 18
            rotation: root.iconRotation
            color: !root.enabled ? Theme.disabledText : (root.hovered && root.tone === "danger" ? Theme.bad : Theme.stat)
        }
    }
    background: Rectangle {
        radius: Theme.radiusSmall
        color: root.down ? Theme.tileHover : (root.hovered && root.enabled ? Theme.buttonHover : "transparent")
        border.width: root.activeFocus ? 1 : 0
        border.color: Theme.focusRing
    }
}
