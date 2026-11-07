// A live reading with a history line: name, big value, a small second line, and a sparkline of the
// last minutes. `hot` colours the value with the warning tone (and says so in the accessible name).
import QtQuick
import QtQuick.Layouts
import org.truesense.app

Rectangle {
    id: root

    property string icon: ""
    property string label: ""
    property string valueText: "–"
    property string subText: ""
    property var values: []
    property real minimum: NaN
    property real maximum: NaN
    property bool hot: false

    implicitHeight: col.implicitHeight + 24
    height: implicitHeight
    radius: Theme.radius
    color: Theme.card
    border.color: root.hot ? Theme.bad : Theme.cardLine
    border.width: 1
    Accessible.role: Accessible.StaticText
    Accessible.name: label + " " + valueText + (subText !== "" ? ", " + subText : "")

    ColumnLayout {
        id: col
        anchors.fill: parent
        anchors.margins: 12
        spacing: 4

        RowLayout {
            spacing: 8
            Icon { name: root.icon; size: 18; color: Theme.stat }
            Text {
                Layout.fillWidth: true
                text: root.label
                color: Theme.muted
                font.pixelSize: Theme.fontSmall + 1
                elide: Text.ElideRight
            }
        }
        Text {
            Layout.fillWidth: true
            text: root.valueText
            color: root.hot ? Theme.bad : Theme.text
            font.pixelSize: Theme.fontBig
            font.bold: true
            elide: Text.ElideRight
        }
        Text {
            Layout.fillWidth: true
            text: root.subText
            color: Theme.muted
            font.pixelSize: Theme.fontSmall
            elide: Text.ElideRight
            // Keeps the tile height steady when the line is empty.
            Layout.minimumHeight: Theme.fontSmall + 4
        }
        Sparkline {
            Layout.fillWidth: true
            Layout.preferredHeight: 30
            values: root.values
            minimum: root.minimum
            maximum: root.maximum
            lineColor: root.hot ? Theme.bad : Theme.chartLine
        }
    }
}
