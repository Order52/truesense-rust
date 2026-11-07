// A row of exclusive icon tiles (power mode, fan mode, GPU mode): the selected one is filled with
// the accent colour. The tiles wrap onto more rows when the window is narrow.
//
//   TileRow {
//       //% "Eco"
//       model: [{ value: "eco", label: qsTrId("mode_eco"), icon: "eco" }, ...]
//       current: Live.value("power_mode")
//       onActivated: (value) => Api.call("power", "set", { mode: value })
//   }
//
// `activated` fires only when the person picks a tile (click, Space or Enter), never when `current`
// is changed by code, so a refresh from the hardware cannot send a command (APP-DESIGN finding 27).
// A tile with `enabled: false` stays visible and greyed.
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import org.truesense.app

Item {
    id: root

    /// [{value, label, icon, enabled?}]
    property var model: []
    /// The value of the selected tile (anything comparable; undefined selects none).
    property var current: undefined
    /// "filled": the selected tile is the accent colour; "outlined": soft fill and accent outline.
    property string variant: "filled"
    property int tileHeight: Theme.tileHeight
    property int minTileWidth: 92
    readonly property int columns: Math.max(1, Math.min(model.length, Math.floor((width + 8) / (minTileWidth + 8))))

    signal activated(var value)

    implicitHeight: grid.implicitHeight
    height: implicitHeight
    implicitWidth: model.length * (minTileWidth + 8)

    GridLayout {
        id: grid
        width: parent.width
        columns: root.columns
        columnSpacing: 8
        rowSpacing: 8

        Repeater {
            model: root.model
            delegate: Button {
                id: tile
                required property var modelData
                readonly property bool selected: root.current !== undefined && modelData.value === root.current
                readonly property bool off: modelData.enabled === false

                Layout.fillWidth: true
                Layout.preferredWidth: 1
                Layout.preferredHeight: root.tileHeight
                enabled: !off
                hoverEnabled: true
                focusPolicy: Qt.StrongFocus
                Accessible.role: Accessible.RadioButton
                Accessible.name: modelData.label
                Accessible.checkable: true
                Accessible.checked: selected
                onClicked: root.activated(modelData.value)

                readonly property color ink: off ? Theme.disabledText
                    : selected ? (root.variant === "filled" ? Theme.accentInk : Theme.accent) : Theme.stat

                contentItem: ColumnLayout {
                    spacing: 6
                    Item { Layout.fillHeight: true }
                    Icon {
                        Layout.alignment: Qt.AlignHCenter
                        visible: tile.modelData.icon !== undefined && tile.modelData.icon !== ""
                        name: tile.modelData.icon === undefined ? "" : tile.modelData.icon
                        size: 22
                        color: tile.ink
                    }
                    Text {
                        Layout.fillWidth: true
                        text: tile.modelData.label
                        color: tile.ink
                        font.pixelSize: Theme.fontSmall + 1
                        font.bold: tile.selected
                        horizontalAlignment: Text.AlignHCenter
                        elide: Text.ElideRight
                    }
                    Item { Layout.fillHeight: true }
                }

                background: Rectangle {
                    radius: Theme.radius
                    color: tile.selected
                        ? (root.variant === "filled" ? Theme.accent : Theme.selBg)
                        : tile.off ? Theme.buttonOff : (tile.down ? Theme.tileHover : tile.hovered ? Theme.tileHover : Theme.tile)
                    border.width: tile.activeFocus || (tile.selected && root.variant === "outlined") ? 1 : 0
                    border.color: tile.activeFocus ? Theme.focusRing : Theme.accent
                    Behavior on color { ColorAnimation { duration: Theme.animMs } }
                }
            }
        }
    }
}
