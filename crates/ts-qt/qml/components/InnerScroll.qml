// A scrolling box inside a page (a log, a licence text). It takes the wheel only while it can still
// move that way; at its top or bottom the wheel goes on to the page, so the page never gets stuck
// under the pointer. Children go into it like into a Flickable.
//
//   InnerScroll { contentHeight: text.height; Text { id: text; width: parent.width } }
import QtQuick
import QtQuick.Controls.Basic
import org.truesense.app

Flickable {
    id: root

    property real wheelStep: 3 * 40

    clip: true
    contentWidth: width
    boundsBehavior: Flickable.StopAtBounds
    flickableDirection: Flickable.VerticalFlick
    pixelAligned: true
    ScrollBar.vertical: ScrollBar {
        policy: ScrollBar.AsNeeded
        visible: size < 1.0
        contentItem: Rectangle {
            implicitWidth: 6
            radius: 3
            color: Theme.knob
            opacity: parent.active ? 0.85 : 0.45
        }
    }

    WheelHandler {
        target: null
        acceptedDevices: PointerDevice.Mouse | PointerDevice.TouchPad
        onWheel: (event) => {
            var touch = event.pixelDelta.y !== 0
            var dy = touch ? event.pixelDelta.y : event.angleDelta.y / 120 * root.wheelStep
            var max = Math.max(0, root.contentHeight - root.height)
            if ((dy > 0 && root.contentY <= 0) || (dy < 0 && root.contentY >= max)) {
                // At the edge: the page scrolls (a Flickable would swallow the event otherwise).
                var page = root.parent
                while (page && page.scrollBy === undefined) page = page.parent
                if (page) page.scrollBy(-dy, !touch)
                return
            }
            root.contentY = Math.round(Math.max(0, Math.min(root.contentY - dy, max)))
        }
    }
}
