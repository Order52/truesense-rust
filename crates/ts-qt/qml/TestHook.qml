// Runs only when the app is started by a test (TRUESENSE_QT_TEST=1): waits, exercises one command
// through the real bus, reports what the shell looks like, optionally saves a screenshot, quits.
import QtQuick
import org.truesense.app

Item {
    id: hook

    /// The window to describe and grab.
    property var window: null
    /// The item to save as a PNG (the shell; a Window's own content item cannot be grabbed).
    property var target: null
    property var config: ({ exit_after_ms: 0, shot: null, set: null, probe: null })
    property var versionResult: null
    property string versionError: ""
    property var daemonRefusal: null

    function begin() {
        Api.call("version", "get", {}, function (r) { hook.versionResult = r },
                 function (e) { hook.versionError = e.message })
        // A command the bus does not have: the error must arrive with its text and code intact.
        Api.call("nope", "nothing", {}, null, function (e) { hook.daemonRefusal = e })
        timer.interval = Math.max(0, config.exit_after_ms)
        timer.start()
        if (config.set) { setter.start(); _schedule() }
    }

    // TRUESENSE_QT_SET: {"objectName": {"property": value, "a.b": value, "function()": [arguments]}}
    // is set on (or called on) the items with that objectName. Tried a few times, because a Loader
    // creates an item (a Lighting tab, say) only after the first assignment made it appear.
    // A member "@1200": {...} is the same map applied 1200 ms after the first frame (steps in time:
    // scroll, then press Apply, then look). TRUESENSE_QT_PROBE: ["objectName.a.b", ...] is read at the
    // end and reported as `probe`. Tests and screenshots only: no page reads anything of this.
    property var _done: ({})
    property var _later: []
    function _find(item, name) {
        if (!item) return null
        if (item.objectName === name) return item
        for (var i = 0; i < item.children.length; ++i) {
            var r = _find(item.children[i], name)
            if (r) return r
        }
        if (item.contentItem && item.contentItem !== item) return _find(item.contentItem, name)
        return null
    }
    function _apply(item, want) {
        for (var k in want) {
            var call = k.slice(-2) === "()"
            var path = (call ? k.slice(0, -2) : k).split(".")
            var obj = item
            for (var i = 0; i < path.length - 1; ++i) obj = obj[path[i]]
            var last = path[path.length - 1]
            if (call) obj[last].apply(obj, want[k])
            else obj[last] = want[k]
        }
    }
    function _visit(map) {
        for (var name in map) {
            if (name.charAt(0) === "@" || _done[name]) continue
            var item = _find(hook.target, name)
            if (item) { _apply(item, map[name]); _done[name] = true }
        }
    }
    function _probe() {
        var out = {}
        var list = config.probe || []
        for (var i = 0; i < list.length; ++i) {
            var path = list[i].split(".")
            var v = _find(hook.target, path[0])
            for (var j = 1; v !== null && v !== undefined && j < path.length; ++j) v = v[path[j]]
            out[list[i]] = (v === undefined || v === null || typeof v === "object") ? (v === undefined ? null : v) : v
        }
        return out
    }
    Timer {
        id: setter
        interval: 150
        repeat: true
        property int left: 12
        onTriggered: {
            hook._visit(hook.config.set)
            if (--left <= 0) stop()
        }
    }
    Component {
        id: laterStep
        Timer {
            property var map: ({})
            repeat: false
            running: true
            onTriggered: {
                // A later step finds its items afresh (they may have been created since).
                for (var name in map) {
                    var item = hook._find(hook.target, name)
                    if (item) hook._apply(item, map[name])
                    else console.warn("test hook: no item " + name)
                }
            }
        }
    }
    function _schedule() {
        for (var k in config.set) {
            if (k.charAt(0) !== "@") continue
            _later.push(laterStep.createObject(hook, { interval: parseInt(k.slice(1)), map: config.set[k] }))
        }
    }

    function describe() {
        return JSON.stringify({
            page: Nav.current,
            page_qml: Nav.currentQml,
            pages: Nav.pages.length,
            groups: Nav.groups.length,
            title_home: qsTrId("nav_home"),
            title_current: Nav.currentPage ? Nav.titleOf(Nav.currentPage) : "",
            language: Ui.language,
            theme_dark: Theme.dark,
            width: window.width,
            height: window.height,
            visibility: window.visibility,
            live_ok: Live.ok,
            live_serial: Live.serial,
            cpu_temp: Live.value("sensors.cpu_temp") === undefined ? null : Live.value("sensors.cpu_temp"),
            caps: Object.keys(Caps.model).length,
            version_result: versionResult,
            version_error: versionError,
            refusal: daemonRefusal,
            tray: Ui.trayAvailable,
            pending: Api.pending,
            probe: config.probe ? _probe() : null
        })
    }

    Timer {
        id: timer
        onTriggered: {
            Bus.report(hook.describe())
            if (hook.config.shot) {
                var queued = hook.target.grabToImage(function (result) {
                    result.saveToFile(hook.config.shot)
                    Qt.quit()
                })
                if (!queued) Qt.quit()
            } else {
                Qt.quit()
            }
        }
    }
}
