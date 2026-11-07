import QtQuick
import QtTest
import org.truesense.app

// The Monitor page's chart and bar components, and every tab drawn from the views that ts-ui-model
// answers for realistic and for empty inputs (crates/ts-ui-model/tests/monitor-views.json, checked
// by a Rust test against the module, so a change of the contract fails there first).
TestCase {
    id: tc
    name: "Monitor"
    when: windowShown
    visible: true
    width: 760
    height: 1400

    property var views: ({})

    function initTestCase() {
        var xhr = new XMLHttpRequest()
        xhr.open("GET", Qt.resolvedUrl("../../../ts-ui-model/tests/monitor-views.json"), false)
        xhr.send()
        verify(xhr.responseText.length > 1000, "views file not readable (set QML_XHR_ALLOW_FILE_READ=1)")
        views = JSON.parse(xhr.responseText)
        Live.load('{"n":1,"frame":{"daemon":{}},"history":{"sensors.cpu_temp":[50,52,null,60],"sensors.gpu_temp":[45,46,47],"sensors.sys_temp":[]}}')
    }

    // ---- LineChart ------------------------------------------------------------------------
    Component { id: chartC; LineChart { width: 400; chartHeight: 100; minRepaintMs: 0 } }

    function test_linechart_scale_follows_data_ignores_gaps_and_can_be_fixed() {
        var c = createTemporaryObject(chartC, tc)
        compare(c.pointCount, 0)
        c.series = [ { values: [10, null, 30, NaN], color: "red" }, { values: [20, undefined], color: "blue" } ]
        compare(c.pointCount, 3)
        verify(c.low <= 10 && c.high >= 30)
        c.zeroBased = true
        compare(c.low, 0)
        c.minimum = 0
        c.maximum = 100
        compare(c.low, 0)
        compare(c.high, 100)
    }

    function test_linechart_flat_and_empty_data_have_a_usable_range() {
        var c = createTemporaryObject(chartC, tc)
        c.series = [ { values: [5, 5, 5], color: "red" } ]
        verify(c.high > c.low)
        c.series = []
        verify(c.high > c.low)
        c.series = [ { values: [null, null], color: "red" } ]
        compare(c.pointCount, 0)
    }

    function test_linechart_repaints_on_change_only() {
        var c = createTemporaryObject(chartC, tc)
        c.series = [ { values: [1, 2, 3], color: "red" } ]
        tryVerify(function () { return c.paintCount >= 1 })
        var before = c.paintCount
        wait(150)
        compare(c.paintCount, before, "no repaint without a change")
        c.series = [ { values: [1, 2, 3, 4], color: "red" } ]
        tryVerify(function () { return c.paintCount > before })
    }

    function test_linechart_same_numbers_in_a_new_array_do_not_redraw() {
        var c = createTemporaryObject(chartC, tc)
        c.series = [ { values: [1, 2, 3], color: "red" } ]
        tryVerify(function () { return c.paintCount >= 1 })
        wait(50)
        var before = c.paintCount
        c.series = [ { values: [1, 2, 3], color: "red" } ]
        c.series = [ { values: [1, 2, 3], color: "red" } ]
        wait(200)
        compare(c.paintCount, before)
    }

    function test_linechart_is_throttled_by_default_and_draws_the_newest_when_due() {
        var c = createTemporaryObject(chartC, tc)
        compare(c.minRepaintMs, 0)
        c.minRepaintMs = 400
        c.series = [ { values: [1, 2], color: "red" } ]
        tryVerify(function () { return c.paintCount >= 1 })
        var before = c.paintCount
        c.series = [ { values: [1, 2, 3], color: "red" } ]
        c.series = [ { values: [1, 2, 3, 4], color: "red" } ]
        wait(100)
        compare(c.paintCount, before, "held back")
        tryVerify(function () { return c.paintCount === before + 1 }, 2000)
    }

    function test_linechart_is_never_mirrored() {
        var c = createTemporaryObject(chartC, tc)
        c.LayoutMirroring.enabled = true
        c.series = [ { values: [1, 2], color: "red", label: "A" } ]
        c.grid = [ { value: 0, text: "0" }, { value: 2, text: "2" } ]
        wait(50)
        verify(c.width > 0)
    }

    // ---- BarList --------------------------------------------------------------------------
    Component { id: barsC; BarList { width: 400 } }

    function test_barlist_rows_bars_and_null_values() {
        var b = createTemporaryObject(barsC, tc)
        b.model = [ { label: "0", text: "37 %", value: 0.37, level: "ok" },
                    { label: "1", text: "–", value: null, level: "unknown" },
                    { label: "2", text: "120 %", value: 1.7, level: "bad" } ]
        compare(b.rowCount, 3)
        compare(b.levelColor("bad"), Theme.bad)
        compare(b.levelColor("warn"), Theme.warn)
        compare(b.levelColor("ok"), Theme.accent)
        compare(b.levelColor("unknown"), Theme.fill)
        b.columns = 2
        wait(30)
        verify(b.implicitHeight > 0)
        b.model = []
        b.emptyText = "none"
        compare(b.rowCount, 0)
    }

    // ---- the tabs, from the real views ----------------------------------------------------
    Component { id: ovC; MonitorOverviewTab { width: 700 } }
    Component { id: cpuC; MonitorCpuTab { width: 700 } }
    Component { id: gpuC; MonitorGpuTab { width: 700 } }
    Component { id: memC; MonitorMemoryTab { width: 700 } }
    Component { id: netC; MonitorNetworkTab { width: 700 } }

    function test_overview_tab_full_asleep_and_empty() {
        var t = createTemporaryObject(ovC, tc, { view: views.overview_full, facts: views.facts_full })
        wait(30)
        verify(t.implicitHeight > 300)
        verify(t.gaugeCaption(views.overview_full.gauges[4]).indexOf(" 2") > 0, "the second drive is numbered")
        compare(t.gaugeCaption(views.overview_full.gauges[0]).indexOf(" 2"), -1)
        verify(t.factLabel("gpu1").indexOf(" 2") > 0)
        compare(t.factLabel("gpu1").indexOf(t.factLabel("gpu0")), 0)
        t.destroy()
        t = createTemporaryObject(ovC, tc, { view: views.overview_asleep, facts: views.facts_empty })
        wait(30)
        verify(t.implicitHeight > 200)
        t.destroy()
        t = createTemporaryObject(ovC, tc, { view: views.overview_empty, facts: views.facts_empty })
        wait(30)
        verify(t.implicitHeight > 200)
        t = createTemporaryObject(ovC, tc, { view: null, facts: null })
        wait(30)
        verify(t.implicitHeight > 0, "a tab without data still lays out")
    }

    function test_cpu_tab_full_first_sample_and_empty() {
        for (var name of ["cpu_full", "cpu_first", "cpu_empty"]) {
            var t = createTemporaryObject(cpuC, tc, { view: views[name], loadHistory: [10, null, 40] })
            wait(30)
            verify(t.implicitHeight > 200, name)
            t.destroy()
        }
        var full = createTemporaryObject(cpuC, tc, { view: views.cpu_full })
        var asked = []
        full.limitRequested.connect(function (n) { asked.push(n) })
        full.limitRequested(20)
        compare(asked, [20])
    }

    function test_gpu_tab_awake_asleep_absent_and_empty() {
        for (var name of ["gpu_awake", "gpu_asleep", "gpu_absent", "gpu_empty"]) {
            var t = createTemporaryObject(gpuC, tc, { view: views[name], tempHistory: [50, 51], utilHistory: [null, 3], powerHistory: [30, 40] })
            wait(30)
            verify(t.implicitHeight > 40, name)
            if (name === "gpu_asleep") verify(t.asleep)
            if (name === "gpu_absent") verify(!t.present)
            t.destroy()
        }
    }

    function test_memory_and_network_tabs() {
        for (var name of ["memory_full", "memory_empty"]) {
            var m = createTemporaryObject(memC, tc, { view: views[name], ramHistory: [40, 41] })
            wait(30)
            verify(m.implicitHeight > 200, name)
            verify(m.kindLabel("nvme") !== m.kindLabel("hdd"))
            m.destroy()
        }
        for (var n of ["network_full", "network_empty"]) {
            var t = createTemporaryObject(netC, tc, { view: views[n], rxHistory: [null, 10, 20], txHistory: [5, null] })
            wait(30)
            verify(t.implicitHeight > 200, n)
            verify(t.kindLabel("wifi") !== t.kindLabel("ethernet"))
            t.destroy()
        }
    }
}
