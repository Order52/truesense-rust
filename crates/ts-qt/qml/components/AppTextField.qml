// A single-line text box in the app's style (the Basic style's own TextField is bare). Use
// `onTextEdited` for the person's typing; assigning `text` from code is silent.
import QtQuick
import QtQuick.Controls.Basic
import org.truesense.app

TextField {
    id: root

    implicitHeight: Theme.controlHeight
    leftPadding: 12
    rightPadding: 12
    color: Theme.text
    font.pixelSize: Theme.fontBody
    selectByMouse: true
    placeholderTextColor: Theme.muted
    selectedTextColor: Theme.accentInk
    selectionColor: Theme.accent
    opacity: enabled ? 1 : 0.5
    Accessible.role: Accessible.EditableText

    background: Rectangle {
        radius: Theme.radiusSmall + 1
        color: Theme.field
        border.width: 1
        border.color: root.activeFocus ? Theme.focusRing : Theme.buttonLine
    }
}
