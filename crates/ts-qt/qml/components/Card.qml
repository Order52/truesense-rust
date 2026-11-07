// A rounded panel that groups related controls. `title` and `icon` make a header row; children go
// below it in a column.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

Rectangle {
    id: root

    property string title: ""
    property string icon: ""
    /// Text at the right end of the header (a live value, a status).
    property string rightText: ""
    default property alias content: body.data

    implicitWidth: 320
    implicitHeight: column.implicitHeight + 2 * Theme.pad
    height: implicitHeight
    radius: Theme.radius
    color: Theme.card
    border.color: Theme.cardLine
    border.width: 1

    ColumnLayout {
        id: column
        anchors.fill: parent
        anchors.margins: Theme.pad
        spacing: Theme.spacing

        SectionHeader {
            Layout.fillWidth: true
            visible: root.title !== "" || root.icon !== ""
            title: root.title
            icon: root.icon
            rightText: root.rightText
        }
        ColumnLayout {
            id: body
            Layout.fillWidth: true
            spacing: Theme.spacing
        }
    }
}
