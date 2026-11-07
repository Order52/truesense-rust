// The layout every settings row shares: the name on the left, the controls on the right (below the
// name when the row is narrow), and an optional explanation underneath.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

Item {
    id: root

    property string label: ""
    property string sub: ""
    /// The controls; they are laid out in a row at the far end.
    default property alias controls: ctl.data
    readonly property bool narrow: width < 440

    implicitHeight: col.implicitHeight
    height: implicitHeight
    implicitWidth: 320

    ColumnLayout {
        id: col
        width: parent.width
        spacing: 4

        GridLayout {
            Layout.fillWidth: true
            columns: root.narrow ? 1 : 2
            columnSpacing: Theme.spacing
            rowSpacing: 8

            Text {
                Layout.fillWidth: true
                Layout.alignment: Qt.AlignVCenter
                visible: root.label !== ""
                text: root.label
                color: Theme.text
                font.pixelSize: Theme.fontBody
                wrapMode: Text.Wrap
            }
            RowLayout {
                id: ctl
                Layout.alignment: Qt.AlignVCenter | (root.narrow ? Qt.AlignLeft : Qt.AlignRight)
                Layout.fillWidth: root.label === "" || root.narrow
                spacing: 8
            }
        }
        HelpText {
            Layout.fillWidth: true
            visible: root.sub !== ""
            text: root.sub
        }
    }
}
