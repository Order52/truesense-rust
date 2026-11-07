// A name with a one-line text box (profile name, program name). `edited(text)` is the person's
// typing only; setting `text` from code is silent. `accepted()` is Enter.
//
//   TextFieldRow { label: "Save as"; placeholder: "name"; maxLength: 40; onAccepted: save() }
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import org.truesense.app

SettingRow {
    id: root

    property alias text: field.text
    property string placeholder: ""
    property int maxLength: 80
    property int fieldWidth: 220
    readonly property alias field: field

    signal edited(string text)
    signal accepted()

    TextField {
        id: field
        Layout.preferredWidth: root.fieldWidth
        Layout.minimumWidth: 120
        Layout.preferredHeight: Theme.controlHeight
        maximumLength: root.maxLength
        placeholderText: root.placeholder
        placeholderTextColor: Theme.disabledText
        color: Theme.text
        font.pixelSize: Theme.fontBody
        selectByMouse: true
        leftPadding: 12
        rightPadding: 12
        Accessible.name: root.label !== "" ? root.label : root.placeholder
        onTextEdited: root.edited(text)
        onAccepted: root.accepted()
        background: Rectangle {
            radius: Theme.radiusSmall + 1
            color: Theme.field
            border.width: 1
            border.color: field.activeFocus ? Theme.focusRing : Theme.buttonLine
            opacity: field.enabled ? 1 : 0.5
        }
    }
}
