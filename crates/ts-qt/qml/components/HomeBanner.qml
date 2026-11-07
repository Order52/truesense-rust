// One line that says something is wrong with the machinery (the service is down, the kernel module
// is missing), with an optional button to the page that explains more. Pages show it only when
// there is a problem. `kind`: "warn" (works, but limited) or "bad" (cannot drive the hardware).
// State is never by colour alone: the icon and the words say it too.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

Rectangle {
    id: root

    property string kind: "warn"
    property string text: ""
    property string actionText: ""
    signal actionClicked()

    readonly property color _tone: kind === "bad" ? Theme.bad : Theme.warn

    implicitHeight: row.implicitHeight + 20
    height: implicitHeight
    radius: Theme.radius
    color: Qt.rgba(_tone.r, _tone.g, _tone.b, Theme.dark ? 0.12 : 0.10)
    border.color: Qt.rgba(_tone.r, _tone.g, _tone.b, 0.55)
    border.width: 1
    Accessible.role: Accessible.AlertMessage
    Accessible.name: text

    GridLayout {
        id: row
        anchors.fill: parent
        anchors.margins: 10
        columns: root.width < 480 && root.actionText !== "" ? 2 : 3
        columnSpacing: 10
        rowSpacing: 8

        Icon {
            Layout.alignment: Qt.AlignTop
            name: "warn"
            size: 20
            color: root._tone
        }
        Text {
            Layout.fillWidth: true
            text: root.text
            color: Theme.text
            font.pixelSize: Theme.fontBody
            wrapMode: Text.Wrap
        }
        SecondaryButton {
            visible: root.actionText !== ""
            Layout.columnSpan: row.columns === 2 ? 2 : 1
            Layout.alignment: Qt.AlignRight
            text: root.actionText
            onClicked: root.actionClicked()
        }
    }
}
