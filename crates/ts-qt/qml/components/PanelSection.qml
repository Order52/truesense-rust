// One section of the compact panel: an icon and a bold title that says the state in force
// ("Performance Mode: Turbo"), live figures on the right ("CPU 48°C · 2400 RPM"), and the choices
// below it.
//
//   PanelSection { icon: "performance"; title: "Performance Mode: Turbo"; info: "CPU 48°C"; PanelTile {...} }
import QtQuick
import QtQuick.Layouts
import org.truesense.app

ColumnLayout {
    id: section

    property string icon: ""
    property string title: ""
    /// Muted figures at the right of the title.
    property string info: ""
    /// Choices per row in the grid below.
    property int columns: 4
    default property alias content: grid.data

    spacing: 8
    Layout.fillWidth: true

    RowLayout {
        Layout.fillWidth: true
        spacing: 8
        Icon { name: section.icon; size: 18; color: Theme.text; visible: section.icon !== "" }
        Text {
            Layout.fillWidth: true
            text: section.title
            color: Theme.text
            font.pixelSize: Theme.fontBody + 1
            font.bold: true
            elide: Text.ElideRight
            Accessible.role: Accessible.Heading
            Accessible.name: text
        }
        Text {
            visible: section.info !== ""
            text: section.info
            color: Theme.muted
            font.pixelSize: Theme.fontSmall + 1
        }
    }
    GridLayout {
        id: grid
        Layout.fillWidth: true
        columns: section.columns
        columnSpacing: 8
        rowSpacing: 8
        uniformCellWidths: true
    }
}
