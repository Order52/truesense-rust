import QtQuick
import QtTest
import org.truesense.app

TestCase {
    name: "TileRow"
    when: windowShown
    visible: true
    width: 520
    height: 160

    TileRow {
        id: row
        width: 480
        model: [
            { value: "eco", label: "Eco", icon: "eco" },
            { value: "quiet", label: "Quiet", icon: "quiet" },
            { value: "turbo", label: "Turbo", icon: "turbo", enabled: false }
        ]
        current: "eco"
    }
    SignalSpy { id: spy; target: row; signalName: "activated" }

    // The same row, picking optimistically (the power mode and fan mode tiles).
    TileRow {
        id: fast
        y: 80
        width: 480
        optimistic: true
        holdMs: 150
        model: [
            { value: "eco", label: "Eco", icon: "eco" },
            { value: "quiet", label: "Quiet", icon: "quiet" },
            { value: "stop", label: "Stop", icon: "max", asks: true }
        ]
        current: "eco"
    }
    SignalSpy { id: fastSpy; target: fast; signalName: "activated" }

    function init() { spy.clear(); fastSpy.clear(); row.current = "eco"; fast.settle(); fast.current = "eco" }

    function test_a_click_activates_the_tile_but_does_not_move_the_selection() {
        mouseClick(row, 240, 20)
        compare(spy.count, 1)
        compare(spy.signalArguments[0][0], "quiet")
        compare(row.current, "eco", "the page decides, after the command succeeded")
    }

    function test_changing_current_from_code_never_activates() {
        row.current = "quiet"
        row.current = undefined
        row.current = "eco"
        compare(spy.count, 0)
    }

    function test_a_disabled_tile_cannot_be_activated() {
        mouseClick(row, 400, 20)
        compare(spy.count, 0)
    }

    function test_tiles_wrap_when_narrow() {
        row.width = 200
        verify(row.columns < 3, "columns " + row.columns)
        tryVerify(function () { return row.implicitHeight > row.tileHeight })
        row.width = 480
        compare(row.columns, 3)
    }

    function test_a_new_array_with_the_same_tiles_keeps_the_buttons() {
        var first = row.children[0].children[0]
        verify(first !== undefined)
        var before = row.children[0].children.length
        row.model = [
            { value: "eco", label: "Eco", icon: "eco" },
            { value: "quiet", label: "Quiet", icon: "quiet" },
            { value: "turbo", label: "Turbo", icon: "turbo", enabled: false }
        ]
        compare(row.children[0].children.length, before)
        verify(row.children[0].children[0] === first, "the same button object, updated in place")
        row.model = [
            { value: "eco", label: "Eco!", icon: "eco" },
            { value: "quiet", label: "Quiet", icon: "quiet" },
            { value: "turbo", label: "Turbo", icon: "turbo", enabled: false }
        ]
        verify(row.children[0].children[0] === first)
        compare(row.children[0].children[0].modelData.label, "Eco!")
    }

    function test_an_optimistic_pick_shows_at_once_and_the_machine_takes_over() {
        mouseClick(fast, 240, 20)
        compare(fastSpy.count, 1)
        compare(fast.shown, "quiet", "selected before any answer")
        compare(fast.current, "eco")
        fast.current = "quiet"          // the live values confirm it
        compare(fast.picked, undefined)
        compare(fast.shown, "quiet")
    }

    function test_a_failed_optimistic_pick_goes_back() {
        mouseClick(fast, 240, 20)
        compare(fast.shown, "quiet")
        fast.settle()                   // the page's fail callback
        compare(fast.shown, "eco")
    }

    function test_an_unconfirmed_pick_falls_back_after_the_hold() {
        mouseClick(fast, 240, 20)
        compare(fast.shown, "quiet")
        tryCompare(fast, "shown", "eco", 2000)
    }

    function test_a_tile_that_asks_first_is_never_shown_early() {
        mouseClick(fast, 400, 20)
        compare(fastSpy.count, 1)
        compare(fastSpy.signalArguments[0][0], "stop")
        compare(fast.shown, "eco")
    }

    function test_a_plain_row_is_not_optimistic() {
        mouseClick(row, 240, 20)
        compare(row.shown, "eco")
    }
}
