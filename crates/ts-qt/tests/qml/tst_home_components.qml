import QtQuick
import QtTest
import org.truesense.app

TestCase {
    name: "HomeComponents"
    when: windowShown
    visible: true
    width: 480
    height: 360

    // A fake Bus that answers view-model calls from a table.
    QtObject {
        id: fake
        property var answers: ({})
        property var seen: []
        function vm(module, verb, args) {
            seen.push([module, verb, args])
            var a = answers[module + "." + verb]
            return a === undefined ? JSON.stringify({ ok: false, error: "no such verb" })
                                   : JSON.stringify({ ok: true, result: a })
        }
    }

    function cleanup() { Api.transport = null }

    // ---- Vm --------------------------------------------------------------------------------
    function test_vm_returns_the_result_or_null_with_the_reason() {
        compare(Vm.call("home", "view", {}), null)
        verify(Vm.lastError.length > 0, "no transport")
        fake.answers = { "home.view": { control: 1 } }
        Api.transport = fake
        compare(Vm.call("home", "view", { frame: null }).control, 1)
        compare(Vm.lastError, "")
        compare(fake.seen[fake.seen.length - 1][2], '{"frame":null}')
        compare(Vm.call("home", "nope", {}), null)
        compare(Vm.lastError, "no such verb")
    }

    // ---- HomeBanner ---------------------------------------------------------------------------
    Component { id: bannerC; HomeBanner { width: 440 } }
    SignalSpy { id: bannerSpy; signalName: "actionClicked" }

    function test_the_banner_shows_its_text_and_reports_the_button() {
        var b = createTemporaryObject(bannerC, this, { text: "Service down", actionText: "Open Health" })
        verify(b)
        compare(b.Accessible.name, "Service down")
        bannerSpy.target = b
        bannerSpy.clear()
        mouseClick(b, 400, b.height / 2)
        compare(bannerSpy.count, 1)
        verify(b.height > 20)
    }

    function test_a_banner_without_action_has_no_button() {
        var b = createTemporaryObject(bannerC, this, { text: "x" })
        bannerSpy.target = b
        bannerSpy.clear()
        mouseClick(b, 400, b.height / 2)
        compare(bannerSpy.count, 0)
    }

    // ---- FavouriteChip ------------------------------------------------------------------------
    Component { id: chipC; FavouriteChip { } }
    SignalSpy { id: chipSpy; signalName: "clicked" }

    function test_a_chip_applies_but_a_missing_one_does_not() {
        var c = createTemporaryObject(chipC, this, { text: "calm", iconName: "palette", kindLabel: "Look" })
        chipSpy.target = c
        chipSpy.clear()
        mouseClick(c)
        compare(chipSpy.count, 1)
        compare(c.Accessible.name, "calm, Look")
        c.missing = true
        mouseClick(c)
        compare(chipSpy.count, 1, "a deleted item cannot be applied")
        verify(!c.enabled)
    }

    // ---- ListIconButton -----------------------------------------------------------------------
    Component { id: iconBtnC; ListIconButton { iconName: "close"; text: "Remove" } }
    SignalSpy { id: iconSpy; signalName: "clicked" }

    function test_an_icon_button_clicks_unless_disabled_and_is_named() {
        var b = createTemporaryObject(iconBtnC, this)
        iconSpy.target = b
        iconSpy.clear()
        mouseClick(b)
        compare(iconSpy.count, 1)
        compare(b.Accessible.name, "Remove")
        b.enabled = false
        mouseClick(b)
        compare(iconSpy.count, 1)
    }

    // ---- HomeStatTile -------------------------------------------------------------------------
    Component { id: tileC; HomeStatTile { width: 200 } }

    function test_a_stat_tile_speaks_its_reading() {
        var t = createTemporaryObject(tileC, this, { label: "CPU", valueText: "54 °C", subText: "Load 12 %", values: [50, 51, 52] })
        compare(t.Accessible.name, "CPU 54 °C, Load 12 %")
        verify(t.height > 80)
        t.hot = true
        compare(t.border.color, Theme.bad)
    }
}
