// One measured value with its name and a line of context, for the Monitor tabs:
//
//   MonitorStat { label: "Download"; value: "1.2 MB/s"; sub: "peak 5.0 MB/s" }
//
// `level` ("warn", "bad") tints the number; the number is always there, so colour is never the
// only signal. An empty `value` shows a dash.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

ColumnLayout {
    id: root

    property string label: ""
    property string value: ""
    property string sub: ""
    property string level: "ok"
    property int valueSize: Theme.fs(20)

    spacing: 1
    Layout.fillWidth: true
    Layout.alignment: Qt.AlignTop
    Layout.preferredWidth: 1
    Layout.minimumWidth: 80
    Accessible.role: Accessible.StaticText
    Accessible.name: label
    Accessible.description: value + (sub !== "" ? ", " + sub : "")

    Text {
        Layout.fillWidth: true
        text: root.label
        color: Theme.muted
        font.pixelSize: Theme.fontSmall
        elide: Text.ElideRight
    }
    Text {
        Layout.fillWidth: true
        text: root.value !== "" ? root.value : Fmt.unknown
        color: root.level === "bad" ? Theme.bad : root.level === "warn" ? Theme.warn : Theme.text
        font.pixelSize: root.valueSize
        font.bold: true
        elide: Text.ElideRight
    }
    Text {
        Layout.fillWidth: true
        visible: root.sub !== ""
        text: root.sub
        color: Theme.muted
        font.pixelSize: Theme.fontSmall
        elide: Text.ElideRight
    }
}
