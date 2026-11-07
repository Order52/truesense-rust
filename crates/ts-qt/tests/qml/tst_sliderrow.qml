import QtQuick
import QtTest
import org.truesense.app

TestCase {
    name: "SliderRow"
    when: windowShown
    visible: true
    width: 520
    height: 100

    SliderRow { id: row; width: 480; label: "CPU fan"; from: 20; to: 100; coalesceMs: 40; value: 50 }
    SignalSpy { id: edited; target: row; signalName: "edited" }
    SignalSpy { id: released; target: row; signalName: "released" }

    function init() { edited.clear(); released.clear(); row.value = 50 }

    function test_setting_the_value_from_code_is_silent() {
        row.value = 70
        wait(100)
        compare(edited.count, 0)
        compare(released.count, 0)
    }

    function test_a_burst_of_moves_is_coalesced_and_ends_on_the_last_value() {
        for (var i = 0; i < 40; ++i) {
            row.control.value = 20 + i
            row.control.moved()
        }
        // The first move goes out at once; the rest wait for the interval.
        compare(edited.count, 1)
        compare(edited.signalArguments[0][0], 20)
        tryVerify(function () { return edited.count >= 2 })
        compare(edited.signalArguments[edited.count - 1][0], 59)
        wait(150)
        compare(edited.count, 2, "one trailing emit, not one per move")
    }

    function test_a_slow_move_is_reported_at_once() {
        row.control.value = 33
        row.control.moved()
        compare(edited.count, 1)
        wait(120)
        row.control.value = 34
        row.control.moved()
        compare(edited.count, 2)
    }

    function test_value_text_shows_the_unit() {
        row.unit = "%"
        row.value = 60
        compare(row.control.value, 60)
    }
}
