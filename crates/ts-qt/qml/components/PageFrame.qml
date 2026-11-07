// The container of every page: a scrolling column, centred, no wider than Theme.pageMaxWidth, with
// the page title on top. Children go into the column, so a page is a list of Cards:
//
//   PageFrame {
//       //% "Cooling"
//       title: qsTrId("cooling_title")
//       Card { ... }
//       Card { ... }
//   }
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import org.truesense.app

Item {
    id: root

    property string title: ""
    property string subtitle: ""
    default property alias content: column.data
    /// The column, for pages that need its width.
    readonly property alias column: column
    /// The scrolling area (tools/qtprofile scrolls it to measure what a backend costs while moving).
    readonly property alias flick: scroll
    readonly property int margin: width < 560 ? 12 : 24

    Flickable {
        id: scroll
        anchors.fill: parent
        contentWidth: width
        contentHeight: column.implicitHeight + 2 * root.margin
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        flickableDirection: Flickable.VerticalFlick
        // Whole pixels only: a fractional offset makes the software renderer resample every glyph and
        // image of the page on every scroll frame, and blurs text for no gain.
        pixelAligned: true
        ScrollBar.vertical: ScrollBar {
            policy: ScrollBar.AsNeeded
            visible: size < 1.0
            contentItem: Rectangle {
                implicitWidth: 6
                radius: 3
                color: Theme.knob
                opacity: parent.active ? 0.9 : 0.5
            }
        }

        ColumnLayout {
            id: column
            x: Math.max(root.margin, (scroll.width - width) / 2)
            y: root.margin
            width: Math.min(scroll.width - 2 * root.margin, Theme.pageMaxWidth)
            spacing: Theme.spacing + 4

            ColumnLayout {
                Layout.fillWidth: true
                visible: root.title !== "" && !Nav.headerShowsTitle
                spacing: 2
                Text {
                    Layout.fillWidth: true
                    text: root.title
                    color: Theme.text
                    font.pixelSize: Theme.fontTitle
                    font.bold: true
                    elide: Text.ElideRight
                    Accessible.role: Accessible.Heading
                    Accessible.name: root.title
                }
                HelpText {
                    visible: root.subtitle !== ""
                    text: root.subtitle
                }
            }
        }
    }
}
