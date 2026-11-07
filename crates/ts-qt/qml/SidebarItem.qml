// One entry of the sidebar: icon and title (title hidden in the icon rail, shown as a tooltip).
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import org.truesense.app

Button {
    id: root
    HoverHandler { cursorShape: parent.enabled ? Qt.PointingHandCursor : Qt.ArrowCursor }

    /// A registry entry {id, icon, titleId, ...}.
    property var page: ({})
    property bool compact: false
    readonly property bool selected: Nav.current === page.id
    readonly property string title: Nav.titleOf(page)

    implicitHeight: 40
    hoverEnabled: true
    focusPolicy: Qt.StrongFocus
    Accessible.role: Accessible.PageTab
    Accessible.name: title
    Accessible.selected: selected
    onClicked: Nav.go(page.id)

    ToolTip.visible: compact && hovered
    ToolTip.text: title
    ToolTip.delay: 400

    contentItem: RowLayout {
        spacing: 12
        Item { Layout.fillWidth: root.compact }
        Icon {
            name: root.page.icon
            size: 20
            color: root.selected ? Theme.accent : Theme.stat
        }
        Text {
            visible: !root.compact
            Layout.fillWidth: true
            text: root.title
            color: root.selected ? Theme.text : Theme.stat
            font.pixelSize: Theme.fontBody
            font.bold: root.selected
            elide: Text.ElideRight
        }
        Item { Layout.fillWidth: root.compact }
    }
    leftPadding: root.compact ? 0 : 14
    rightPadding: root.compact ? 0 : 10

    background: Rectangle {
        radius: Theme.radiusSmall + 1
        color: root.selected ? Theme.selBg : (root.hovered || root.down ? Theme.tileHover : "transparent")
        border.width: root.activeFocus ? 1 : 0
        border.color: Theme.focusRing
        Rectangle {
            visible: root.selected
            width: 3
            height: parent.height - 14
            radius: 2
            anchors.verticalCenter: parent.verticalCenter
            x: 0
            color: Theme.accent
        }
    }
}
