import QtQuick
import QtTest
import org.truesense.app

TestCase {
    name: "Misc components"
    when: windowShown
    visible: true
    width: 520
    height: 400

    NavGrid {
        id: grid
        width: 480
        model: [{ id: "lighting", title: "Lighting", icon: "palette" }, { id: "audio", title: "Audio", icon: "audio" }]
    }
    SignalSpy { id: navigated; target: grid; signalName: "navigate" }

    SwatchRow {
        id: swatches
        y: 120
        width: 480
        colors: ["#ff0000", "#00ff00", "#0000ff"]
        current: "#00FF00"
        allowCustom: true
    }
    SignalSpy { id: picked; target: swatches; signalName: "activated" }
    SignalSpy { id: custom; target: swatches; signalName: "customRequested" }

    PrimaryButton { id: button; y: 200; text: "Apply" }
    SignalSpy { id: clicked; target: button; signalName: "clicked" }

    Card { id: card; y: 260; width: 300; title: "Title"; icon: "fan"; HelpText { text: "hello" } }

    function init() { navigated.clear(); picked.clear(); custom.clear(); clicked.clear() }

    function test_nav_grid_navigates_by_id() {
        mouseClick(grid, 20, 20)
        compare(navigated.count, 1)
        compare(navigated.signalArguments[0][0], "lighting")
    }

    function test_swatches_compare_case_insensitively_and_offer_custom() {
        mouseClick(swatches, 12, 12)
        compare(picked.count, 1)
        compare(picked.signalArguments[0][0], "#ff0000")
        mouseClick(swatches, 3 * 38 + 12, 12)
        compare(custom.count, 1)
    }

    function test_a_busy_button_cannot_be_clicked_twice() {
        button.busy = true
        mouseClick(button)
        compare(clicked.count, 0)
        button.busy = false
        mouseClick(button)
        compare(clicked.count, 1)
    }

    function test_card_sizes_to_its_content() {
        verify(card.implicitHeight > 60)
    }

    function test_gauge_zones() {
        var g = createTemporaryObject(gaugeComponent, this)
        g.value = 50
        compare(g.zoneColor.toString(), Theme.accent.toString())
        g.value = 85
        compare(g.zoneColor.toString(), Theme.warn.toString())
        g.value = 99
        compare(g.zoneColor.toString(), Theme.bad.toString())
    }

    function test_gauge_snaps_to_a_new_value_so_the_live_tick_runs_no_animation() {
        var g = createTemporaryObject(gaugeComponent, this)
        compare(g.easeMs, 0)
        g.value = 55
        compare(g.shown, 55)
        compare(g.fraction, 0.5)
        g.value = 110
        compare(g.fraction, 1)
        g.value = -20
        compare(g.fraction, 0)
    }

    function test_gauge_easing_is_opt_in_and_short() {
        var g = createTemporaryObject(gaugeComponent, this)
        g.easeMs = 5000                // asked for much; capped at 200 ms
        g.value = 110
        verify(g.shown < 110, "still moving")
        tryVerify(function () { return g.shown === 110 }, 600)
    }

    Component { id: gaugeComponent; Gauge { width: 100; height: 100; from: 0; to: 110; warnAt: 80; critAt: 95 } }
}
