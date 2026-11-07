// A grid of buttons that open pages (the shortcuts on Home). Entries are {id, title, icon}; the
// page decides what `navigate(id)` does, normally `Nav.go(id)`.
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import org.truesense.app

Item {
    id: root

    /// [{id, title, icon}]
    property var model: []
    property int minButtonWidth: 168
    readonly property int columns: Math.max(1, Math.min(model.length, Math.floor((width + 8) / (minButtonWidth + 8))))
    signal navigate(string id)

    implicitHeight: grid.implicitHeight
    height: implicitHeight

    GridLayout {
        id: grid
        width: parent.width
        columns: root.columns
        columnSpacing: 8
        rowSpacing: 8

        Repeater {
            model: root.model
            delegate: Button {
                id: b
                required property var modelData
                Layout.fillWidth: true
                Layout.preferredWidth: 1
                Layout.preferredHeight: Theme.controlHeight + 6
                hoverEnabled: true
                focusPolicy: Qt.StrongFocus
                Accessible.role: Accessible.Button
                Accessible.name: modelData.title
                onClicked: root.navigate(modelData.id)

                contentItem: RowLayout {
                    spacing: 10
                    Item { Layout.fillWidth: true }
                    Icon {
                        visible: b.modelData.icon !== undefined
                        name: b.modelData.icon === undefined ? "" : b.modelData.icon
                        size: 18
                        color: Theme.stat
                    }
                    Text {
                        text: b.modelData.title
                        color: Theme.text
                        font.pixelSize: Theme.fontBody
                        elide: Text.ElideRight
                    }
                    Item { Layout.fillWidth: true }
                }
                background: Rectangle {
                    radius: Theme.radius
                    color: b.down || b.hovered ? Theme.buttonHover : Theme.button
                    border.width: 1
                    border.color: b.activeFocus ? Theme.focusRing : Theme.buttonLine
                }
            }
        }
    }
}
