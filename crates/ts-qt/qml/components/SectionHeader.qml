// A section title with an icon and an optional value at the far end ("Performance Mode ... CPU 54 °C").
import QtQuick
import QtQuick.Layouts
import org.truesense.app

RowLayout {
    id: root

    property string title: ""
    property string icon: ""
    property string rightText: ""

    spacing: 10
    Accessible.role: Accessible.Heading
    Accessible.name: title

    Icon {
        visible: root.icon !== ""
        name: root.icon
        size: 20
        color: Theme.stat
    }
    Text {
        Layout.fillWidth: true
        text: root.title
        color: Theme.text
        font.pixelSize: Theme.fontSection
        font.bold: true
        elide: Text.ElideRight
    }
    Text {
        visible: root.rightText !== ""
        text: root.rightText
        color: Theme.muted
        font.pixelSize: Theme.fontBody
        horizontalAlignment: Text.AlignRight
    }
}
