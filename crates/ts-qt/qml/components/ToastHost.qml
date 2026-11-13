// Draws the message of the Toasts singleton: one small pill at the top right, over the empty end of
// the page title row, so it never covers the controls of a page or the status bar. Errors stay 6 s,
// the rest 2.5 s; a click dismisses. Screen readers hear it as an alert. Put one in the window,
// nowhere else.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

Item {
    id: root
    objectName: "toastHost"
    anchors.fill: parent

    /// Same as `Toasts.show` (lets the screenshot hook put a message up by objectName).
    function show(text, kind) { Toasts.show(text, kind) }

    readonly property var current: Toasts.items.length > 0 ? Toasts.items[Toasts.items.length - 1] : null
    readonly property color tone: current === null ? Theme.accent
        : current.kind === "error" ? Theme.bad : current.kind === "ok" ? Theme.ok : Theme.accent

    Rectangle {
        id: pill
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.rightMargin: 20
        // Below the header of the narrow layout; beside the page title otherwise.
        anchors.topMargin: root.width < 520 ? 56 : 18
        width: Math.min(row.implicitWidth + 24, Math.min(400, root.width - 40))
        height: row.implicitHeight + 14
        radius: Math.min(height / 2, 16)
        color: Theme.dialog
        border.width: 1
        border.color: Qt.rgba(root.tone.r, root.tone.g, root.tone.b, 0.55)
        visible: opacity > 0
        opacity: root.current !== null ? 1 : 0
        Behavior on opacity { NumberAnimation { duration: 140 } }
        Accessible.role: Accessible.AlertMessage
        Accessible.name: root.current !== null ? root.current.text : ""

        RowLayout {
            id: row
            anchors.verticalCenter: parent.verticalCenter
            x: 12
            width: pill.width - 24
            spacing: 8
            Icon {
                name: root.current === null ? "info"
                    : root.current.kind === "error" ? "warn" : root.current.kind === "ok" ? "check" : "info"
                size: 15
                color: root.tone
            }
            Text {
                id: label
                Layout.fillWidth: true
                Layout.maximumWidth: 360
                text: root.current !== null ? root.current.text : ""
                color: Theme.text
                font.pixelSize: Theme.fontSmall + 1
                // An error may need its words; anything else is one line.
                maximumLineCount: root.current !== null && root.current.kind === "error" ? 3 : 1
                wrapMode: Text.Wrap
                elide: Text.ElideRight
            }
        }
        MouseArea {
            anchors.fill: parent
            cursorShape: Qt.PointingHandCursor
            onClicked: if (root.current !== null) Toasts.dismiss(root.current.id)
        }
        Timer {
            // Restarts for every new message (its id changes).
            property int forId: root.current !== null ? root.current.id : 0
            onForIdChanged: if (forId > 0) restart(); else stop()
            interval: root.current !== null && root.current.kind === "error" ? 6000 : 2500
            onTriggered: if (root.current !== null) Toasts.dismiss(root.current.id)
        }
    }
}
