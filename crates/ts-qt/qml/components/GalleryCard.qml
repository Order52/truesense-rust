// One card of a gallery: a thumbnail of the keyboard, a name, one or two lines about it, an
// optional star, a badge ("Running"), and room for the card's own controls under them (colourway
// chips, Apply). Clicking the picture or the name selects the card (`activated`); the star and the
// controls are separate and do not select it.
//
//   GalleryCard {
//       title: "Hearth"; subtitle: "A calm fire"; layout: LightingVm.layout; cells: [...]
//       starred: true; badge: "Running"; highlighted: true
//       onActivated: ...; onStarToggled: ...
//       PrimaryButton { text: "Apply" }          // goes under the text
//   }
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import org.truesense.app

Rectangle {
    id: root

    property string title: ""
    property string subtitle: ""
    property var layout: ({ keys: [], width: 20.5, height: 6.4 })
    property var cells: []
    /// The star is only shown when `starrable` is true.
    property bool starrable: false
    property bool starred: false
    /// A short word on the card ("Running", "Yours"); empty for none.
    property string badge: ""
    property string badgeKind: "accent"
    /// The card is the one the detail below belongs to.
    property bool selected: false
    /// The card is in use now (outlined in the accent colour).
    property bool highlighted: false
    /// What the star's button is called for screen readers; the page words it.
    property string starName: ""
    property string description: ""
    default property alias content: extra.data

    signal activated()
    signal starToggled()

    implicitHeight: column.implicitHeight + 20
    radius: Theme.radius
    color: selected ? Theme.selBg : (pick.containsMouse ? Theme.tileHover : Theme.tile)
    border.width: highlighted || selected || pick.activeFocus ? 1 : 0
    border.color: pick.activeFocus ? Theme.focusRing : Theme.accent
    Accessible.role: Accessible.ListItem
    Accessible.name: title
    Accessible.description: description !== "" ? description : subtitle
    Accessible.selected: selected

    ColumnLayout {
        id: column
        anchors.fill: parent
        anchors.margins: 10
        spacing: 6

        // The picture and the name select the card.
        MouseArea {
            id: pick
            Layout.fillWidth: true
            Layout.preferredHeight: head.implicitHeight
            hoverEnabled: true
            activeFocusOnTab: true
            cursorShape: Qt.PointingHandCursor
            onClicked: root.activated()
            Keys.onReturnPressed: root.activated()
            Keys.onSpacePressed: root.activated()

            ColumnLayout {
                id: head
                width: parent.width
                spacing: 6
                KeyboardThumb {
                    Layout.fillWidth: true
                    Layout.preferredHeight: width * root.layout.height / Math.max(1, root.layout.width)
                    layout: root.layout
                    cells: root.cells
                    capColor: root.selected ? Theme.tileHover : Theme.buttonOff
                }
                RowLayout {
                    Layout.fillWidth: true
                    spacing: 6
                    Text {
                        Layout.fillWidth: true
                        text: root.title
                        color: root.selected || root.highlighted ? Theme.accent : Theme.text
                        font.pixelSize: Theme.fontBody
                        font.bold: true
                        elide: Text.ElideRight
                    }
                    Badge {
                        visible: root.badge !== ""
                        text: root.badge
                        kind: root.badgeKind
                    }
                    Button {
                        id: star
                        visible: root.starrable
                        Layout.preferredWidth: 30
                        Layout.preferredHeight: 30
                        hoverEnabled: true
                        focusPolicy: Qt.StrongFocus
                        Accessible.role: Accessible.CheckBox
                        Accessible.name: root.starName
                        Accessible.checkable: true
                        Accessible.checked: root.starred
                        onClicked: root.starToggled()
                        contentItem: Item {
                            Icon {
                                anchors.centerIn: parent
                                name: root.starred ? "star-filled" : "star"
                                size: 18
                                color: root.starred ? Theme.warn : Theme.muted
                            }
                        }
                        background: Rectangle {
                            radius: Theme.radiusSmall
                            color: star.down ? Theme.tileHover : (star.hovered ? Theme.buttonHover : "transparent")
                            border.width: star.activeFocus ? 1 : 0
                            border.color: Theme.focusRing
                        }
                    }
                }
                Text {
                    Layout.fillWidth: true
                    visible: root.subtitle !== ""
                    text: root.subtitle
                    color: Theme.muted
                    font.pixelSize: Theme.fontSmall
                    wrapMode: Text.Wrap
                    elide: Text.ElideRight
                    maximumLineCount: 2
                    // Two lines are kept for it, so the cards of one row end together.
                    Layout.minimumHeight: (Theme.fontSmall + 4) * 2
                }
            }
        }

        ColumnLayout {
            id: extra
            Layout.fillWidth: true
            spacing: 6
        }
    }
}
