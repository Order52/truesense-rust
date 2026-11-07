// Monitor: what the machine is doing, in five tabs (Overview, CPU, GPU, Memory and disks, Network).
//
// This file only schedules and lays out. WHAT to ask and WHEN is `vm monitor plan` (ts-ui-model):
// only the visible tab is polled, only while the window is visible, at the cadence COMMANDS.md
// allows, and a graphics card that is asleep is never asked for detail. HOW a result reads (units,
// levels, process ranking, chart scale) is the other verbs of that module; the tabs just draw.
// A reading that cannot be taken is a dash, never a zero, and never a toast every second.
import QtQuick
import QtQuick.Layouts
import QtQuick.Window
import org.truesense.app

PageFrame {
    id: root

    title: Nav.currentPage ? Nav.titleOf(Nav.currentPage) : ""

    // ---- what is showing ------------------------------------------------------------------
    property string tab: "overview"
    property int procLimit: 8
    /// The page is on screen: it is the current page and the window is visible (not hidden to the
    /// tray, not minimised). Nothing is polled otherwise.
    readonly property bool shown: visible && Window.visibility !== Window.Hidden && Window.visibility !== Window.Minimized

    // ---- what is known --------------------------------------------------------------------
    property var results: ({})
    property var hist: ({})
    property string lastError: ""
    property var overviewView: null
    property var factsView: null
    property var cpuView: null
    property var gpuView: null
    property var memView: null
    property var netView: null
    property var powerScale: null
    readonly property bool ready: overviewView !== null && cpuView !== null && gpuView !== null && memView !== null && netView !== null

    // Bookkeeping of the polling (plain objects: nothing binds to them).
    readonly property var book: ({ last: {}, pending: {}, errors: {}, sample: {}, alive: true })

    Component.onCompleted: rebuildAll()
    Component.onDestruction: book.alive = false

    function vm(verb, args) {
        var reply
        try { reply = JSON.parse(Bus.vm("monitor", verb, JSON.stringify(args))) } catch (e) { return null }
        return reply && reply.ok ? reply.result : null
    }

    function history(name) { return hist[name] !== undefined ? hist[name] : [] }

    function pushHistory(name, value, now, intervalMs) {
        var last = book.sample[name]
        var next = vm("push", { series: history(name), value: value, now_ms: now,
                                last_ms: last === undefined ? null : last, interval_ms: intervalMs })
        var copy = {}
        for (var k in hist) copy[k] = hist[k]
        copy[name] = next === null ? history(name) : next
        hist = copy
        book.sample[name] = now
    }

    // ---- views ----------------------------------------------------------------------------
    function rebuildAll() {
        overviewView = vm("overview", { overview: results.overview })
        factsView = vm("facts", { system: results.system, uptime_s: results.overview ? results.overview.uptime_s : null })
        cpuView = vm("cpu", { cpu: results.cpu, limit: procLimit })
        gpuView = vm("gpu", { live: results.gpu_live, detail: results.gpu_detail })
        memView = vm("memory", { memory: results.memory, disks: results.disks, limit: procLimit })
        rebuildNet()
        rebuildPowerScale()
    }

    function rebuildNet() {
        netView = vm("network", { network: results.network, rx: history("net_rx"), tx: history("net_tx") })
    }

    function rebuildPowerScale() {
        powerScale = vm("scale", { series: [history("gpu_power")], floor: 10, unit: "watts" })
    }

    function accept(key, result, now) {
        var r = results
        r[key] = result
        results = r
        switch (key) {
        case "overview":
        case "system":
            overviewView = vm("overview", { overview: results.overview })
            factsView = vm("facts", { system: results.system, uptime_s: results.overview ? results.overview.uptime_s : null })
            break
        case "cpu":
            cpuView = vm("cpu", { cpu: result, limit: procLimit })
            pushHistory("cpu_load", result.load_pct === undefined ? null : result.load_pct, now, 1000)
            break
        case "gpu_live":
        case "gpu_detail":
            gpuView = vm("gpu", { live: results.gpu_live, detail: results.gpu_detail })
            if (key === "gpu_live" && gpuView !== null) {
                pushHistory("gpu_temp", gpuView.temperature.value, now, 1000)
                pushHistory("gpu_util", gpuView.util.value, now, 1000)
                pushHistory("gpu_power", gpuView.power.value, now, 1000)
                rebuildPowerScale()
            }
            break
        case "memory":
        case "disks":
            memView = vm("memory", { memory: results.memory, disks: results.disks, limit: procLimit })
            if (key === "memory" && result.memory) pushHistory("ram", result.memory.used_pct, now, 1000)
            break
        case "network":
            pushHistory("net_rx", result.rx_bps === undefined ? null : result.rx_bps, now, 1000)
            pushHistory("net_tx", result.tx_bps === undefined ? null : result.tx_bps, now, 1000)
            rebuildNet()
            break
        }
    }

    // ---- polling --------------------------------------------------------------------------
    function tick() {
        var now = Date.now()
        var gpu = results.gpu_live
        var plan = vm("plan", {
            tab: tab, active: shown, now_ms: now, last: book.last, pending: book.pending, errors: book.errors,
            gpu_state: gpu && gpu.state ? gpu.state : "", top: procLimit
        })
        if (plan === null) return
        poll.interval = plan.wait_ms
        for (var i = 0; i < plan.commands.length; ++i) start(plan.commands[i], now)
    }

    function start(cmd, now) {
        var key = cmd.key
        var token = book
        token.last[key] = now
        token.pending[key] = true
        Api.call(cmd.area, cmd.verb, cmd.args, function (result) {
            if (!token.alive) return
            token.pending[key] = false
            token.errors[key] = 0
            if (result !== null && result !== undefined && typeof result === "object") {
                root.lastError = ""
                root.accept(key, result, Date.now())
            }
        }, function (err) {
            if (!token.alive) return
            token.pending[key] = false
            token.errors[key] = (token.errors[key] || 0) + 1
            root.lastError = err && err.message ? err.message : ""
        })
    }

    onTabChanged: tick()
    onProcLimitChanged: { book.last = {}; tick() }

    Timer {
        id: poll
        interval: 500
        repeat: true
        running: root.shown
        triggeredOnStart: true
        onTriggered: root.tick()
    }

    function resetPeaks() {
        Api.call("monitor", "reset", {}, function () {
            root.hist = {}
            root.book.sample = {}
            root.book.last = {}
            //% "Lowest, highest and totals were cleared."
            Toasts.show(qsTrId("monitor_reset_done"), "ok")
            root.tick()
        })
    }

    // ---- layout ---------------------------------------------------------------------------
    readonly property bool narrowTabs: width < 640

    function tabModel() {
        //% "Overview"
        var overview = qsTrId("monitor_tab_overview")
        //% "CPU"
        var cpu = qsTrId("monitor_tab_cpu")
        //% "GPU"
        var gpu = qsTrId("monitor_tab_gpu")
        //% "Memory and disks"
        var memLong = qsTrId("monitor_tab_memory")
        //% "Memory"
        var memShort = qsTrId("monitor_tab_memory_short")
        //% "Network"
        var network = qsTrId("monitor_tab_network")
        return [
            { value: "overview", label: overview },
            { value: "cpu", label: cpu },
            { value: "gpu", label: gpu },
            { value: "memory", label: narrowTabs ? memShort : memLong },
            { value: "network", label: network }
        ]
    }

    GridLayout {
        Layout.fillWidth: true
        columns: root.width >= 700 ? 2 : 1
        columnSpacing: Theme.spacing
        rowSpacing: Theme.spacing

        Segmented {
            Layout.fillWidth: true
            Layout.maximumWidth: 560
            current: root.tab
            model: root.tabModel()
            onActivated: function (value) { root.tab = value }
        }
        SecondaryButton {
            //% "Reset lowest and highest"
            text: qsTrId("monitor_reset")
            iconName: "refresh"
            onClicked: root.resetPeaks()
        }
    }

    HelpText {
        visible: root.lastError !== ""
        color: Theme.warn
        //% "Some readings could not be taken: %1"
        text: qsTrId("monitor_error").arg(root.lastError)
    }

    Loader {
        id: body
        Layout.fillWidth: true
        Layout.preferredHeight: item ? item.implicitHeight : 0
        active: root.ready
        sourceComponent: root.tab === "cpu" ? cpuTab : root.tab === "gpu" ? gpuTab
                       : root.tab === "memory" ? memTab : root.tab === "network" ? netTab : overviewTab
        onLoaded: item.width = Qt.binding(function () { return body.width })
    }

    Component {
        id: overviewTab
        MonitorOverviewTab { view: root.overviewView; facts: root.factsView }
    }
    Component {
        id: cpuTab
        MonitorCpuTab {
            view: root.cpuView
            loadHistory: root.history("cpu_load")
            limit: root.procLimit
            onLimitRequested: function (n) { root.procLimit = n }
        }
    }
    Component {
        id: gpuTab
        MonitorGpuTab {
            view: root.gpuView
            tempHistory: root.history("gpu_temp")
            utilHistory: root.history("gpu_util")
            powerHistory: root.history("gpu_power")
            powerScale: root.powerScale
        }
    }
    Component {
        id: memTab
        MonitorMemoryTab { view: root.memView; ramHistory: root.history("ram") }
    }
    Component {
        id: netTab
        MonitorNetworkTab { view: root.netView; rxHistory: root.history("net_rx"); txHistory: root.history("net_tx") }
    }
}
