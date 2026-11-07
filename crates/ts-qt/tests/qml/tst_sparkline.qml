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

    function init() { s.values = []; s.minimum = NaN; s.maximum = NaN }

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
        s.values = [1, 2, 3]
        tryVerify(function () { return s.paintCount >= 1 })
        var before = s.paintCount
        wait(120)
        compare(s.paintCount, before, "no repaint without a change")
        s.values = [1, 2, 3, 4]
        tryVerify(function () { return s.paintCount > before })
    }
}
