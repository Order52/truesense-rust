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

    /// [{value, label, icon, enabled?, turn?, mirror?}]: `turn` rotates the icon by that many degrees
    /// and `mirror` flips it left to right (a direction arrow, a counter-clockwise arrow).
    property var model: []
    /// The value of the selected tile (anything comparable; undefined selects none).
    property var current: undefined
    /// "filled": the selected tile is the accent colour; "outlined": soft fill and accent outline.
    property string variant: "filled"
    property int tileHeight: Theme.tileHeight
    property int minTileWidth: 92
    /// Long labels wrap onto a second line instead of being cut.
    property bool wrapLabels: false
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

        // The model is the NUMBER of tiles and each tile reads its entry from `root.model`: a new
        // array with the same tiles (every live frame builds one) then updates the bindings in
        // place instead of destroying and recreating every button once a second.
        Repeater {
            model: root.model.length
            delegate: Button {
                id: tile
                HoverHandler { cursorShape: parent.enabled ? Qt.PointingHandCursor : Qt.ArrowCursor }
                required property int index
                readonly property var modelData: root.model[index]
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
                        rotation: tile.modelData.turn === undefined ? 0 : tile.modelData.turn
                        transform: Scale {
                            origin.x: 11
                            xScale: tile.modelData.mirror === true ? -1 : 1
                        }
                    }
                    Text {
                        Layout.fillWidth: true
                        text: tile.modelData.label
                        color: tile.ink
                        font.pixelSize: Theme.fontSmall + 1
                        font.bold: tile.selected
                        horizontalAlignment: Text.AlignHCenter
                        wrapMode: root.wrapLabels ? Text.WordWrap : Text.NoWrap
                        maximumLineCount: root.wrapLabels ? 2 : 1
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
