// One big choice button of the compact panel: an icon over a word, outlined in its colour while it
// is the choice in force (the way G-Helper shows Silent / Balanced / Turbo).
//
//   PanelTile { iconName: "turbo"; text: "Turbo"; tint: "#e5484d"; checked: mode === "turbo"; onClicked: ... }
import QtQuick
import QtQuick.Controls.Basic
import org.truesense.app

AbstractButton {
    id: tile

    /// An icon from qml/icons ("" for a word only).
    property string iconName: ""
    /// The outline and icon colour while checked.
    property color tint: Theme.accent
    /// A busy tile (the command is on its way) dims its word.
    property bool busy: false

    implicitWidth: 88
    implicitHeight: iconName !== "" ? 62 : Theme.controlHeight
    hoverEnabled: true
    focusPolicy: Qt.StrongFocus
    Accessible.role: Accessible.RadioButton
    Accessible.name: text
    Accessible.checked: checked

    HoverHandler { cursorShape: tile.enabled ? Qt.PointingHandCursor : Qt.ArrowCursor }

    background: Rectangle {
        radius: Theme.radiusSmall + 2
        color: tile.down ? Theme.tileLine : (tile.hovered ? Theme.tileHover : Theme.tile)
        border.width: tile.checked ? 2 : (tile.activeFocus ? 2 : 1)
        border.color: tile.checked ? tile.tint : (tile.activeFocus ? Theme.focusRing : Theme.tileLine)
        opacity: tile.enabled ? 1 : 0.45
    }
    contentItem: Item {
      Column {
        anchors.centerIn: parent
        width: parent.width
        spacing: 4
        Icon {
            visible: tile.iconName !== ""
            anchors.horizontalCenter: parent.horizontalCenter
            name: tile.iconName
            size: 20
            color: tile.checked ? tile.tint : Theme.text
        }
        Text {
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            text: tile.text
            color: tile.busy ? Theme.muted : Theme.text
            font.pixelSize: Theme.fontBody
            font.bold: tile.checked
            elide: Text.ElideRight
        }
      }
    }
}
