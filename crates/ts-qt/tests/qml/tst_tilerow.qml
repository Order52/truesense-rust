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

    function init() { spy.clear(); row.current = "eco" }

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
}
