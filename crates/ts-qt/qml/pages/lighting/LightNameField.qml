// A one-line text field in the app's style, for names (looks, designs, key sets).
// `accepted()` is Enter; `text` can be set from code.
import QtQuick
import QtQuick.Controls.Basic
import org.truesense.app

TextField {
    id: root

    implicitHeight: Theme.controlHeight
    implicitWidth: 200
    color: Theme.text
    placeholderTextColor: Theme.muted
    font.pixelSize: Theme.fontBody
    maximumLength: 40
    selectByMouse: true
    leftPadding: 12
    rightPadding: 12
    verticalAlignment: TextInput.AlignVCenter
    Accessible.role: Accessible.EditableText
    Accessible.name: placeholderText

    background: Rectangle {
        radius: Theme.radiusSmall + 1
        color: Theme.field
        border.width: 1
        border.color: root.activeFocus ? Theme.focusRing : Theme.buttonLine
        opacity: root.enabled ? 1 : 0.5
    }
}
