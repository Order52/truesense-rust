import QtQuick
import QtTest
import org.truesense.app

TestCase {
    name: "KeyboardView"
    when: windowShown
    visible: true
    width: 520
    height: 200

    KeyboardView {
        id: board
        width: 400
        layout: ({ width: 4, height: 2, keys: [
            { name: "a", label: "A", cells: [0], x: 0, y: 0, w: 1, h: 1 },
            { name: "b", label: "B", cells: [6], x: 2, y: 0, w: 1, h: 1 }
        ] })
        cells: ["ff0000", "000000", "000000", "000000", "000000", "000000", "00ff00"]
        interactive: true
    }
    SignalSpy { id: pressedSpy; target: board; signalName: "pressedAt" }
    SignalSpy { id: dragSpy; target: board; signalName: "dragged" }
    SignalSpy { id: releaseSpy; target: board; signalName: "released" }
    SignalSpy { id: navSpy; target: board; signalName: "navigated" }

    function init() {
        board.interactive = true
        pressedSpy.clear(); dragSpy.clear(); releaseSpy.clear(); navSpy.clear()
    }

    function test_the_picture_is_as_wide_as_the_layout_allows() {
        // 400 px for 4 units: 100 px a unit, and the height follows the layout (2 units).
        compare(board._unit, 70, "a unit never grows beyond 70 px")
        compare(board.implicitHeight, 2 * 70 + 2)
    }

    function test_a_click_reports_the_position_in_layout_units() {
        // The layout is centred: unit 70, offset (400 - 4 * 70) / 2 = 60.
        mouseClick(board, 60 + 35, 35)
        compare(pressedSpy.count, 1)
        var p = pressedSpy.signalArguments[0][0]
        verify(Math.abs(p.x - 0.5) < 0.01 && Math.abs(p.y - 0.5) < 0.01, JSON.stringify(p))
        compare(pressedSpy.signalArguments[0][1], Qt.LeftButton)
        compare(releaseSpy.count, 1)
    }

    function test_a_drag_reports_each_step_from_where_the_last_one_ended() {
        mousePress(board, 60 + 10, 30)
        mouseMove(board, 60 + 150, 30)
        mouseMove(board, 60 + 200, 30)
        mouseRelease(board, 60 + 200, 30)
        compare(dragSpy.count, 2)
        var first = dragSpy.signalArguments[0]
        var second = dragSpy.signalArguments[1]
        verify(Math.abs(first[1].x - second[0].x) < 1e-9, "the second step starts where the first ended")
        compare(releaseSpy.count, 1)
    }

    function test_right_button_is_reported_for_the_eyedropper() {
        mouseClick(board, 60 + 35, 35, Qt.RightButton)
        compare(pressedSpy.signalArguments[0][1], Qt.RightButton)
    }

    function test_a_picture_that_is_not_interactive_ignores_the_pointer() {
        board.interactive = false
        mouseClick(board, 60 + 35, 35)
        compare(pressedSpy.count, 0)
    }

    function test_arrow_keys_and_space_are_reported() {
        board.forceActiveFocus()
        keyClick(Qt.Key_Right)
        keyClick(Qt.Key_Space)
        compare(navSpy.count, 2)
        compare(navSpy.signalArguments[0][0], "right")
        compare(navSpy.signalArguments[1][0], "activate")
    }
}
