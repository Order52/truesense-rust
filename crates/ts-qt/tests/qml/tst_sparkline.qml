import QtQuick
import QtTest
import org.truesense.app

TestCase {
    name: "Sparkline"
    when: windowShown
    visible: true
    width: 200
    height: 80

    Sparkline { id: s; width: 120; height: 30 }

    function init() { s.minRepaintMs = 0; s.values = []; s.minimum = NaN; s.maximum = NaN }

    function test_empty_draws_nothing_and_has_a_default_scale() {
        compare(s.pointCount, 0)
        compare(s.low, 0)
        compare(s.high, 1)
    }

    function test_scale_follows_the_data_and_ignores_gaps() {
        s.values = [10, null, 30, NaN, 20, undefined]
        compare(s.pointCount, 3)
        compare(s.low, 10)
        compare(s.high, 30)
    }

    function test_fixed_scale_wins() {
        s.minimum = 0
        s.maximum = 100
        s.values = [10, 20]
        compare(s.low, 0)
        compare(s.high, 100)
    }

    function test_flat_data_gets_a_nonzero_range() {
        s.values = [5, 5, 5]
        verify(s.high > s.low)
    }

    function test_repaints_when_values_change_and_only_then() {
        var first = s.paintCount
        s.values = [1, 2, 3]
        tryVerify(function () { return s.paintCount > first })
        wait(120)
        var before = s.paintCount
        wait(120)
        compare(s.paintCount, before, "no repaint without a change")
        s.values = [1, 2, 3, 4]
        tryVerify(function () { return s.paintCount > before })
    }

    function test_the_same_numbers_in_a_new_array_do_not_repaint() {
        var first = s.paintCount
        s.values = [1, 2, 3]
        tryVerify(function () { return s.paintCount > first })
        wait(100)
        var before = s.paintCount
        s.values = [1, 2, 3]
        wait(150)
        compare(s.paintCount, before)
    }

    function test_a_throttled_chart_draws_a_change_when_its_time_is_up() {
        var first = s.paintCount
        s.values = [1, 2, 3]
        tryVerify(function () { return s.paintCount > first })
        wait(100)
        s.minRepaintMs = 400
        var before = s.paintCount
        s.values = [1, 2, 3, 4]       // too soon after the last paint
        wait(100)
        compare(s.paintCount, before, "held back")
        s.values = [1, 2, 3, 4, 5]    // the newest values win when it is drawn
        tryVerify(function () { return s.paintCount > before }, 2000)
        s.minRepaintMs = 0
    }

    function test_the_default_redraws_at_most_every_two_seconds() {
        var fresh = createTemporaryObject(plain, this)
        compare(fresh.minRepaintMs, 2000)
    }

    function test_static_values_cost_nothing_however_long_they_sit() {
        s.values = [3, 4, 5, 4]
        tryVerify(function () { return s.paintCount >= 1 })
        wait(100)
        var before = s.paintCount
        wait(500)
        compare(s.paintCount, before)
        s.values = [3, 4, 5, 4]       // the live feed hands over a new array with the same numbers
        s.values = [3, 4, 5, 4]
        wait(150)
        compare(s.paintCount, before)
    }

    function test_a_resize_redraws_once_even_when_throttled() {
        s.values = [1, 2, 3]
        tryVerify(function () { return s.paintCount >= 1 })
        wait(100)
        s.minRepaintMs = 5000
        var before = s.paintCount
        s.width = 140
        tryVerify(function () { return s.paintCount > before })
        s.width = 120
        s.minRepaintMs = 0
    }

    Component { id: plain; Sparkline { } }
}
