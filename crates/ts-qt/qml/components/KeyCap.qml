// A key or key combination drawn as a key cap ("Ctrl+1"), for the shortcut lists.
//
//   KeyCap { text: "Ctrl+Q" }
import QtQuick
import org.truesense.app

Rectangle {
    id: root

    property string text: ""

    implicitWidth: Math.max(label.implicitWidth + 16, 28)
    implicitHeight: label.implicitHeight + 8
    radius: Theme.radiusSmall - 2
    color: Theme.tile
    border.width: 1
    border.color: Theme.tileLine
    Accessible.role: Accessible.StaticText
    Accessible.name: text

    // The key's lower edge, like a real cap.
    Rectangle {
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        anchors.margins: 1
        height: 2
        radius: 1
        color: Theme.tileLine
    }
    Text {
        id: label
        anchors.centerIn: parent
        anchors.verticalCenterOffset: -1
        text: root.text
        color: Theme.text
        font.pixelSize: Theme.fontSmall
        font.bold: true
        font.family: "monospace"
    }
}
