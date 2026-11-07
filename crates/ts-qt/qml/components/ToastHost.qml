// Draws the toasts of the Toasts singleton at the bottom centre. Errors stay 9 s, the rest 4.5 s;
// a click dismisses. Screen readers hear them as alerts. Put one in the window, nowhere else.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

Item {
    id: root
    anchors.fill: parent
    // Never block clicks on the page below.
    enabled: true

    ColumnLayout {
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottom: parent.bottom
        anchors.bottomMargin: 44
        width: Math.min(520, root.width - 32)
        spacing: 8

        Repeater {
            model: Toasts.items
            delegate: Rectangle {
                id: toast
                required property var modelData
                readonly property color tone: modelData.kind === "error" ? Theme.bad : modelData.kind === "ok" ? Theme.ok : Theme.accent
                Layout.fillWidth: true
                implicitHeight: row.implicitHeight + 20
                radius: Theme.radius
                color: Theme.dialog
                border.color: tone
                border.width: 1
                Accessible.role: Accessible.AlertMessage
                Accessible.name: modelData.text

                RowLayout {
                    id: row
                    anchors.fill: parent
                    anchors.margins: 10
                    anchors.leftMargin: 14
                    anchors.rightMargin: 14
                    spacing: 10
                    Icon {
                        name: toast.modelData.kind === "error" ? "warn" : toast.modelData.kind === "ok" ? "check" : "info"
                        size: 18
                        color: toast.tone
                    }
                    Text {
                        Layout.fillWidth: true
                        text: toast.modelData.text
                        color: Theme.text
                        font.pixelSize: Theme.fontBody
                        wrapMode: Text.Wrap
                    }
                }
                MouseArea {
                    anchors.fill: parent
                    onClicked: Toasts.dismiss(toast.modelData.id)
                }
                Timer {
                    running: true
                    interval: toast.modelData.kind === "error" ? 9000 : 4500
                    onTriggered: Toasts.dismiss(toast.modelData.id)
                }
            }
        }
    }
}
