// The page list as a popup, for windows narrower than Theme.breakMenu where the sidebar is gone.
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import org.truesense.app

Popup {
    id: root

    parent: Overlay.overlay
    modal: true
    focus: true
    x: Ui.rtl ? parent.width - width - 8 : 8
    y: 52
    width: Math.min(300, parent.width - 16)
    padding: 8
    closePolicy: Popup.CloseOnEscape | Popup.CloseOnPressOutside
    Overlay.modal: Rectangle { color: Theme.scrim }

    background: Rectangle {
        radius: Theme.radiusLarge
        color: Theme.dialog
        border.color: Theme.cardLine
    }

    contentItem: Flickable {
        implicitHeight: Math.min(column.implicitHeight, root.parent.height - 80)
        contentHeight: column.implicitHeight
        clip: true
        ColumnLayout {
            id: column
            width: parent.width
            spacing: 2
            Repeater {
                model: Nav.groups
                delegate: ColumnLayout {
                    id: group
                    required property var modelData
                    Layout.fillWidth: true
                    spacing: 2
                    Text {
                        Layout.topMargin: 8
                        Layout.leftMargin: 8
                        text: Nav.groupTitle(group.modelData).toUpperCase()
                        color: Theme.muted
                        font.pixelSize: Theme.fontSmall - 1
                        font.bold: true
                        font.letterSpacing: 0.8
                    }
                    Repeater {
                        model: Nav.inGroup(group.modelData.id)
                        delegate: SidebarItem {
                            required property var modelData
                            Layout.fillWidth: true
                            page: modelData
                            onClicked: root.close()
                        }
                    }
                }
            }
        }
    }
}
