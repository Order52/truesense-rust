// Scrolling a page: the wheel moves it by a fixed step per notch, Page Down by a screen, Home and
// End to the ends, and an inner scrolling box hands the wheel back to the page at its edges. These
// drive the real PageFrame and InnerScroll (their Flickables), with real wheel events.
import QtQuick
import QtQuick.Layouts
import QtTest
import org.truesense.app

TestCase {
    id: stest
    name: "Scroll"
    when: windowShown
    visible: true
    width: 600
    height: 400

    PageFrame {
        id: frame
        anchors.fill: parent
        Rectangle { Layout.fillWidth: true; Layout.preferredHeight: 600; color: "gray" }
        InnerScroll {
            id: inner
            Layout.fillWidth: true
            Layout.preferredHeight: 200
            contentHeight: 500
            Rectangle { width: inner.width; height: 500; color: "blue" }
        }
        Rectangle { Layout.fillWidth: true; Layout.preferredHeight: 1200; color: "gray" }
    }

    function init() {
        frame.flick.contentY = 0
        inner.contentY = 0
        Ui.reducedMotion = true      // no easing: the result is there at once
    }
    function cleanup() { Ui.reducedMotion = false }

    function test_one_wheel_notch_scrolls_one_step_and_never_past_the_end() {
        mouseWheel(frame, 300, 100, 0, -120)
        tryCompare(frame.flick, "contentY", frame.wheelStep)
        mouseWheel(frame, 300, 100, 0, 120)
        tryCompare(frame.flick, "contentY", 0)
        mouseWheel(frame, 300, 100, 0, 120)
        compare(frame.flick.contentY, 0, "not above the top")
        for (var i = 0; i < 40; ++i) mouseWheel(frame, 300, 100, 0, -120)
        tryCompare(frame.flick, "contentY", frame.flick.contentHeight - frame.flick.height)
    }

    function test_a_notch_is_eased_when_motion_is_allowed() {
        Ui.reducedMotion = false
        mouseWheel(frame, 300, 100, 0, -120)
        tryCompare(frame.flick, "contentY", frame.wheelStep, 1000)
    }

    function test_page_keys() {
        frame.forceActiveFocus()
        keyClick(Qt.Key_PageDown)
        tryVerify(function () { return Math.abs(frame.flick.contentY - Math.round(frame.flick.height * 0.9)) <= 1 })
        keyClick(Qt.Key_End)
        tryCompare(frame.flick, "contentY", frame.flick.contentHeight - frame.flick.height)
        keyClick(Qt.Key_Home)
        tryCompare(frame.flick, "contentY", 0)
    }

    function test_an_inner_box_scrolls_first_then_hands_the_wheel_to_the_page() {
        // Bring the inner box under the pointer.
        frame.flick.contentY = 500
        var p = inner.mapToItem(stest, inner.width / 2, inner.height / 2)
        mouseWheel(stest, p.x, p.y, 0, -120)
        tryCompare(inner, "contentY", 120)
        compare(frame.flick.contentY, 500, "the page stays while the box can move")
        mouseWheel(stest, p.x, p.y, 0, -120)
        tryCompare(inner, "contentY", 240)
        mouseWheel(stest, p.x, p.y, 0, -120)
        tryCompare(inner, "contentY", 300)       // its end (500 - 200)
        compare(frame.flick.contentY, 500, "still the box's wheel")
        mouseWheel(stest, p.x, p.y, 0, -120)
        tryCompare(frame.flick, "contentY", 500 + frame.wheelStep, 1000, "at its end the box hands the wheel to the page")
        compare(inner.contentY, 300)
    }
}
