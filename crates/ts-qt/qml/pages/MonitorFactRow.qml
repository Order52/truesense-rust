// A name and its value on one line (two lines when narrow): the facts of the Monitor page.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

GridLayout {
    id: root

    property string label: ""
    property string value: ""
    readonly property bool narrow: width < 420

    Layout.fillWidth: true
    columns: narrow ? 1 : 2
    columnSpacing: Theme.spacing
    rowSpacing: 0
    Accessible.role: Accessible.StaticText
    Accessible.name: label
    Accessible.description: value

    Text {
        Layout.preferredWidth: root.narrow ? -1 : 150
        Layout.fillWidth: root.narrow
        Layout.alignment: Qt.AlignTop
        text: root.label
        color: Theme.muted
        font.pixelSize: Theme.fontSmall + 1
        elide: Text.ElideRight
    }
    Text {
        Layout.fillWidth: true
        text: root.value !== "" ? root.value : Fmt.unknown
        color: Theme.text
        font.pixelSize: Theme.fontBody
        wrapMode: Text.Wrap
        textFormat: Text.PlainText
    }
}
