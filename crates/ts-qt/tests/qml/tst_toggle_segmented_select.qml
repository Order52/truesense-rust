import QtQuick
import QtTest
import org.truesense.app

TestCase {
    name: "Rows"
    when: windowShown
    visible: true
    width: 520
    height: 300

    ToggleRow { id: toggle; x: 0; y: 0; width: 480; label: "Overdrive" }
    SignalSpy { id: toggled; target: toggle; signalName: "toggled" }

    Segmented {
        id: seg
        x: 0; y: 80; width: 300
        model: [{ value: "off", label: "Manual" }, { value: "native", label: "Native" }, { value: "smart", label: "Smart" }]
        current: "native"
    }
    SignalSpy { id: picked; target: seg; signalName: "activated" }

    SelectRow {
        id: select
        x: 0; y: 140; width: 480
        label: "Profile"
        options: ["a", { value: "b", text: "Bee" }]
        current: "b"
    }
    SignalSpy { id: chosen; target: select; signalName: "activated" }

    function init() { toggled.clear(); picked.clear(); chosen.clear() }

    function test_toggle_only_signals_on_user_action() {
        toggle.checked = true
        toggle.checked = false
        compare(toggled.count, 0)
        mouseClick(toggle, toggle.width - 20, 14)
        compare(toggled.count, 1)
        compare(toggled.signalArguments[0][0], true)
        compare(toggle.checked, true)
    }

    function test_segmented_activates_on_click_only() {
        seg.current = "smart"
        seg.current = "native"
        compare(picked.count, 0)
        mouseClick(seg, 40, 17)
        compare(picked.count, 1)
        compare(picked.signalArguments[0][0], "off")
    }

    function test_select_shows_current_and_is_silent_when_set_from_code() {
        compare(select.children.length > 0, true)
        select.current = "a"
        select.current = "b"
        compare(chosen.count, 0)
    }
}
