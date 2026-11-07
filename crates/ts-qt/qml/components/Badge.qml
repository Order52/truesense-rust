// A small word in a coloured pill (OK, WARN, FAIL, PASS, SKIP): the word is the signal, the colour
// supports it. `kind`: "ok", "info", "warn", "fail".
import QtQuick
import org.truesense.app

Rectangle {
    id: root

    property string text: ""
    property string kind: "info"
    readonly property color tone: kind === "ok" ? Theme.ok : kind === "warn" ? Theme.warn : kind === "fail" ? Theme.bad : Theme.muted

    implicitWidth: label.implicitWidth + 16
    implicitHeight: label.implicitHeight + 6
    radius: height / 2
    color: "transparent"
    border.width: 1
    border.color: tone
    Accessible.role: Accessible.StaticText
    Accessible.name: text

    Text {
        id: label
        anchors.centerIn: parent
        text: root.text
        color: root.tone
        font.pixelSize: Theme.fontSmall
        font.bold: true
    }
}
