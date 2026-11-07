// A small pill of text that states a condition ("Changed", "Locked", "OK", "WARN"). The word
// carries the meaning; `kind` only tints it, so it never relies on colour alone.
//
//   Badge { text: qsTrId("x_changed"); kind: "warn" }
//
// kind: "neutral" (default; "info" is the same), "ok", "warn", "bad" ("fail" is the same), "accent".
import QtQuick
import org.truesense.app

Rectangle {
    id: root

    property string text: ""
    property string kind: "neutral"

    readonly property color _ink: kind === "ok" ? Theme.ok
        : kind === "warn" ? Theme.warn
        : (kind === "bad" || kind === "fail") ? Theme.bad
        : kind === "accent" ? Theme.accent
        : Theme.stat

    implicitWidth: label.implicitWidth + 16
    implicitHeight: label.implicitHeight + 6
    radius: height / 2
    color: Theme.pick(Qt.rgba(_ink.r, _ink.g, _ink.b, 0.16), Qt.rgba(_ink.r, _ink.g, _ink.b, 0.12))
    border.width: 1
    border.color: Qt.rgba(_ink.r, _ink.g, _ink.b, 0.55)
    Accessible.role: Accessible.StaticText
    Accessible.name: text

    Text {
        id: label
        anchors.centerIn: parent
        text: root.text
        color: root._ink
        font.pixelSize: Theme.fontSmall
        font.bold: true
    }
}
