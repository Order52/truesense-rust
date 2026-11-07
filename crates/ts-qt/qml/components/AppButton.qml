// The one button. PrimaryButton and SecondaryButton are this with a fixed `kind`.
//
//   kind "primary": accent fill, for the one action of a card (Apply, Save)
//   kind "secondary": quiet outline (Edit, Cancel)
//   kind "danger": red fill, for destructive confirmations
//
// `busy` disables the button while its command runs and shows "..." so a double click cannot send
// the command twice. Focus is drawn as a ring (keyboard users).
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import org.truesense.app

Button {
    id: root
    HoverHandler { cursorShape: parent.enabled ? Qt.PointingHandCursor : Qt.ArrowCursor }

    property string kind: "secondary"
    property string iconName: ""
    property bool busy: false

    implicitHeight: Theme.controlHeight
    implicitWidth: Math.max(contentItem.implicitWidth + 2 * 16, 64)
    enabled: !busy
    hoverEnabled: true
    focusPolicy: Qt.StrongFocus
    Accessible.role: Accessible.Button
    Accessible.name: text

    readonly property color _fg: !enabled && !busy ? Theme.disabledText
        : kind === "primary" ? Theme.accentInk
        : kind === "danger" ? "#ffffff" : Theme.text
    readonly property color _bg: !enabled && !busy ? Theme.buttonOff
        : kind === "primary" ? (down ? Qt.darker(Theme.accent, 1.2) : hovered ? Qt.lighter(Theme.accent, 1.1) : Theme.accent)
        : kind === "danger" ? (down ? Qt.darker(Theme.bad, 1.2) : hovered ? Qt.lighter(Theme.bad, 1.08) : Theme.bad)
        : (down ? Theme.tileHover : hovered ? Theme.buttonHover : Theme.button)

    contentItem: RowLayout {
        spacing: 8
        Icon {
            visible: root.iconName !== "" && !root.busy
            name: root.iconName
            size: 18
            color: root._fg
        }
        Text {
            Layout.fillWidth: true
            text: root.busy ? "…" : root.text
            color: root._fg
            font.pixelSize: Theme.fontBody
            font.bold: root.kind !== "secondary"
            horizontalAlignment: Text.AlignHCenter
            verticalAlignment: Text.AlignVCenter
            elide: Text.ElideRight
        }
    }

    background: Rectangle {
        radius: Theme.radiusSmall + 1
        color: root._bg
        border.width: root.kind === "secondary" || root.activeFocus ? 1 : 0
        border.color: root.activeFocus ? Theme.focusRing : (root.enabled ? Theme.buttonLine : Theme.buttonOffLine)
        Behavior on color { ColorAnimation { duration: Theme.animMs } }
    }
}
