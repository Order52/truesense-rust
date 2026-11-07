// The grouped list of pages (APP-DESIGN 4.1). Wide: icon, title, group headings. Compact (the window
// is narrower than Theme.breakIcons): an icon rail with tooltips.
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import org.truesense.app

Rectangle {
    id: root

    property bool compact: false

    color: Theme.sidebar
    Rectangle {
        anchors.right: parent.right
        width: 1
        height: parent.height
        color: Theme.sep
        x: Ui.rtl ? 0 : parent.width - 1
    }

    Flickable {
        anchors.fill: parent
        anchors.rightMargin: 1
        contentHeight: column.implicitHeight + 24
        clip: true
        boundsBehavior: Flickable.StopAtBounds

        ColumnLayout {
            id: column
            width: parent.width - 16
            x: 8
            y: 12
            spacing: 2

            RowLayout {
                Layout.fillWidth: true
                Layout.bottomMargin: 10
                Layout.leftMargin: root.compact ? 0 : 6
                spacing: 10
                Item { Layout.fillWidth: root.compact }
                Image {
                    source: "qrc:/assets/truesense-256.png"
                    sourceSize.width: 64
                    sourceSize.height: 64
                    Layout.preferredWidth: 30
                    Layout.preferredHeight: 30
                    smooth: true
                }
                Text {
                    visible: !root.compact
                    text: "TrueSense"
                    color: Theme.accent
                    font.pixelSize: Theme.fs(17)
                    font.bold: true
                    font.letterSpacing: 1
                }
                Item { Layout.fillWidth: true; visible: !root.compact }
                Item { Layout.fillWidth: root.compact }
            }

            Repeater {
                model: Nav.groups
                delegate: ColumnLayout {
                    id: group
                    required property var modelData
                    Layout.fillWidth: true
                    spacing: 2

                    Text {
                        Layout.fillWidth: true
                        Layout.topMargin: 10
                        Layout.leftMargin: 8
                        visible: !root.compact
                        text: Nav.groupTitle(group.modelData).toUpperCase()
                        color: Theme.muted
                        font.pixelSize: Theme.fontSmall - 1
                        font.bold: true
                        font.letterSpacing: 0.8
                        elide: Text.ElideRight
                    }
                    Rectangle {
                        visible: root.compact
                        Layout.fillWidth: true
                        Layout.topMargin: 6
                        Layout.bottomMargin: 6
                        height: 1
                        color: Theme.sep
                    }
                    Repeater {
                        model: Nav.inGroup(group.modelData.id)
                        delegate: SidebarItem {
                            required property var modelData
                            Layout.fillWidth: true
                            page: modelData
                            compact: root.compact
                        }
                    }
                }
            }
        }
    }
}
