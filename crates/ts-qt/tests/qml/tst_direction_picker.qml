import QtQuick
import QtTest
import org.truesense.app

TestCase {
    name: "DirectionPicker"
    when: windowShown
    visible: true
    width: 760
    height: 200

    DirectionPicker {
        id: picker
        width: 720
        current: 0
    }
    SignalSpy { id: spy; target: picker; signalName: "activated" }

    function init() { spy.clear(); picker.width = 720; picker.current = 0 }

    function test_six_directions_with_the_stored_numbers_in_order() {
        compare(picker.model.length, 6)
        var values = picker.model.map(function (m) { return m.value })
        compare(values, [0, 1, 2, 3, 4, 5])
        var labels = picker.model.map(function (m) { return m.label })
        verify(labels.every(function (l) { return l.length > 0 }))
        compare(new Set(labels).size, 6, "every option has its own words")
    }

    function test_arrows_turn_and_the_counter_clockwise_one_is_mirrored() {
        var turns = picker.model.slice(0, 4).map(function (m) { return m.turn })
        compare(turns, [0, 180, 270, 90], "right, left, up, down")
        compare(picker.model[4].icon, "rotate-cw")
        compare(picker.model[5].icon, "rotate-cw")
        verify(picker.model[5].mirror === true && picker.model[4].mirror !== true)
    }

    function test_a_click_activates_the_tile_but_does_not_move_the_selection() {
        // Wide: six tiles in one row of 720 px with 8 px gaps; the fifth tile is clockwise.
        var tile = (720 - 5 * 8) / 6
        mouseClick(picker, 4 * (tile + 8) + tile / 2, 20)
        compare(spy.count, 1)
        compare(spy.signalArguments[0][0], 4)
        compare(picker.current, 0, "the page decides after the command worked")
        mouseClick(picker, 5 * (tile + 8) + tile / 2, 20)
        compare(spy.signalArguments[1][0], 5)
    }

    function test_changing_current_from_code_never_activates() {
        picker.current = 5
        picker.current = 2
        compare(spy.count, 0)
    }

    function test_narrow_widths_wrap_to_more_rows() {
        picker.width = 260
        verify(picker.columns < 6, "columns " + picker.columns)
        tryVerify(function () { return picker.implicitHeight > picker.tileHeight })
        picker.width = 720
        compare(picker.columns, 6)
    }
}
