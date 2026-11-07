// A pill with a few exclusive choices (Manual | Native | Smart). Like TileRow, `activated` is the
// person's pick only.
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import org.truesense.app

Rectangle {
    id: root

    /// [{value, label, enabled?}]
    property var model: []
    property var current: undefined
    signal activated(var value)

    implicitHeight: Theme.controlHeight
    height: implicitHeight
    implicitWidth: row.implicitWidth + 8
    radius: Theme.radius
    color: Theme.segBg
    border.color: Theme.segLine
    border.width: 1

    RowLayout {
        id: row
        anchors.fill: parent
        anchors.margins: 3
        spacing: 2

        Repeater {
            model: root.model
            delegate: Button {
                id: seg
                HoverHandler { cursorShape: parent.enabled ? Qt.PointingHandCursor : Qt.ArrowCursor }
                required property var modelData
                readonly property bool selected: root.current !== undefined && modelData.value === root.current

                Layout.fillWidth: true
                Layout.fillHeight: true
                implicitWidth: label.implicitWidth + 28
                enabled: modelData.enabled !== false
                hoverEnabled: true
                focusPolicy: Qt.StrongFocus
                Accessible.role: Accessible.RadioButton
                Accessible.name: modelData.label
                Accessible.checkable: true
                Accessible.checked: selected
                onClicked: root.activated(modelData.value)

                contentItem: Text {
                    id: label
                    text: seg.modelData.label
                    color: !seg.enabled ? Theme.disabledText : seg.selected ? Theme.segOnInk : Theme.muted
                    font.pixelSize: Theme.fontBody
                    font.bold: seg.selected
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                    elide: Text.ElideRight
                }
                background: Rectangle {
                    radius: Theme.radius - 3
                    color: seg.selected ? Theme.segOn : (seg.hovered && seg.enabled ? Theme.tileHover : "transparent")
                    border.width: seg.activeFocus ? 1 : 0
                    border.color: Theme.focusRing
                }
            }
        }
    }
}
