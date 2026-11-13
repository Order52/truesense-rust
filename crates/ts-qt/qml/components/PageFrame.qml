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
    /// How far one notch of a mouse wheel scrolls (three lines of text, as desktops do).
    property real wheelStep: 3 * 40

    /// Scrolls by `dy` pixels (positive: down), clamped to the content; `smooth` eases a mouse notch
    /// over Theme.animMs (nothing animates with reduced motion, nothing runs between scrolls).
    function scrollBy(dy, smooth) {
        var from = ease.running ? ease.to : scroll.contentY
        var to = Math.max(0, Math.min(from + dy, Math.max(0, scroll.contentHeight - scroll.height)))
        scroll.cancelFlick()
        ease.stop()
        if (smooth && Theme.animMs > 0 && Math.abs(to - scroll.contentY) > 1) {
            ease.from = scroll.contentY
            ease.to = to
            ease.start()
        } else {
            scroll.contentY = Math.round(to)
        }
    }
    function scrollTo(y) { scrollBy(y - scroll.contentY, false) }

    NumberAnimation {
        id: ease
        target: scroll
        property: "contentY"
        duration: Theme.animMs / 2  // ~70 ms: a notch eases, it does not lag
        easing.type: Easing.OutCubic
    }

    // Page keys: Page Up/Down a screen (less a little, so a line stays in view), Home/End the ends.
    // A text field that has the focus keeps Home and End for itself (it takes the shortcut first).
    Shortcut { sequences: [StandardKey.MoveToNextPage]; onActivated: root.scrollBy(scroll.height * 0.9, true) }
    Shortcut { sequences: [StandardKey.MoveToPreviousPage]; onActivated: root.scrollBy(-scroll.height * 0.9, true) }
    Shortcut { sequences: [StandardKey.MoveToStartOfDocument, "Home"]; onActivated: root.scrollTo(0) }
    Shortcut { sequences: [StandardKey.MoveToEndOfDocument, "End"]; onActivated: root.scrollTo(scroll.contentHeight) }

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
        // A flick by drag or touch: quick to start, settles fast (desktop, not a phone).
        flickDeceleration: 3500
        maximumFlickVelocity: 4000

        // The wheel and the touchpad scroll the page directly: a touchpad by exactly the distance the
        // fingers moved, a mouse by `wheelStep` per notch, eased briefly. An inner scrolling box
        // (InnerScroll) takes the wheel only while it can still move, then hands it back here.
        WheelHandler {
            target: null
            acceptedDevices: PointerDevice.Mouse | PointerDevice.TouchPad
            onWheel: (event) => {
                if (event.modifiers & Qt.ControlModifier) { event.accepted = false; return }
                var touch = event.pixelDelta.y !== 0
                var dy = touch ? event.pixelDelta.y : event.angleDelta.y / 120 * root.wheelStep
                root.scrollBy(-dy, !touch)
            }
        }
        ScrollBar.vertical: ScrollBar {
            policy: ScrollBar.AsNeeded
            visible: size < 1.0
            // Thin while you read, wider under the pointer so it is easy to grab.
            contentItem: Rectangle {
                implicitWidth: parent.hovered || parent.pressed ? 10 : 6
                radius: width / 2
                color: Theme.knob
                opacity: parent.pressed ? 1 : (parent.active ? 0.85 : 0.45)
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
