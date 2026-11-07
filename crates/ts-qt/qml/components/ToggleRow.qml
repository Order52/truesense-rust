// A switch with a name and an optional explanation. `toggled(checked)` fires only when the person
// flips it; setting `checked` from code (a refresh) is silent.
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import org.truesense.app

SettingRow {
    id: root

    property alias checked: sw.checked
    signal toggled(bool checked)

    Switch {
        id: sw
        HoverHandler { cursorShape: parent.enabled ? Qt.PointingHandCursor : Qt.ArrowCursor }
        padding: 0
        implicitWidth: 44
        implicitHeight: 24
        hoverEnabled: true
        focusPolicy: Qt.StrongFocus
        Accessible.name: root.label
        Accessible.role: Accessible.CheckBox
        onToggled: root.toggled(checked)

        indicator: Rectangle {
            implicitWidth: 44
            implicitHeight: 24
            x: 0
            y: (sw.height - height) / 2
            radius: height / 2
            color: sw.checked ? Theme.accent : Theme.switchTrack
            border.width: sw.activeFocus ? 2 : 1
            border.color: sw.activeFocus ? Theme.focusRing : (sw.checked ? Theme.accent : Theme.knobLine)
            opacity: sw.enabled ? 1 : 0.5
            Behavior on color { ColorAnimation { duration: Theme.animMs } }

            Rectangle {
                width: 18
                height: 18
                radius: 9
                anchors.verticalCenter: parent.verticalCenter
                x: sw.checked ? parent.width - width - 3 : 3
                color: sw.checked ? Theme.accentInk : Theme.switchOff
                Behavior on x { NumberAnimation { duration: Theme.animMs; easing.type: Easing.OutCubic } }
            }
        }
        contentItem: Item {}
    }
}
