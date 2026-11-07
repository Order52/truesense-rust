// A favourite as a one-click chip: kind icon and name. A favourite whose item was deleted is greyed
// with a warning mark and cannot be applied (the page offers to remove it elsewhere).
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import org.truesense.app

Button {
    id: root

    property string iconName: ""
    property bool missing: false
    /// Spoken after the name, e.g. "Look" or "Fan curve".
    property string kindLabel: ""

    implicitHeight: 36
    implicitWidth: Math.min(row.implicitWidth + 24, 260)
    enabled: !missing
    hoverEnabled: true
    focusPolicy: Qt.StrongFocus
    Accessible.role: Accessible.Button
    Accessible.name: text + (kindLabel !== "" ? ", " + kindLabel : "")

    contentItem: RowLayout {
        id: row
        spacing: 8
        Icon {
            name: root.missing ? "warn" : root.iconName
            size: 16
            color: root.missing ? Theme.warn : Theme.accent
        }
        Text {
            Layout.fillWidth: true
            text: root.text
            color: root.missing ? Theme.disabledText : Theme.text
            font.pixelSize: Theme.fontBody
            elide: Text.ElideRight
            font.strikeout: root.missing
        }
    }
    background: Rectangle {
        radius: height / 2
        color: root.missing ? Theme.buttonOff : (root.down ? Theme.tileHover : root.hovered ? Theme.tileHover : Theme.tile)
        border.width: 1
        border.color: root.activeFocus ? Theme.focusRing : (root.missing ? Theme.buttonOffLine : Theme.tileLine)
        Behavior on color { ColorAnimation { duration: Theme.animMs } }
    }
}
