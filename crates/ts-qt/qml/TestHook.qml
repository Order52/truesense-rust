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
    property var config: ({ exit_after_ms: 0, shot: null, set: null })
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
        if (config.set) setter.start()
    }

    // TRUESENSE_QT_SET: {"objectName": {"property": value, "function()": [arguments]}} is set on (or
    // called on) the items with that objectName.
    // Tried a few times, because a Loader creates an item (a Lighting tab, say) only after the first
    // assignment made it appear. Screenshots only: no page reads anything of this.
    property var _done: ({})
    function _visit(item) {
        if (!item) return
        var want = config.set[item.objectName]
        if (item.objectName && want !== undefined && !_done[item.objectName]) {
            for (var k in want) {
                if (k.slice(-2) === "()") item[k.slice(0, -2)].apply(item, want[k])
                else item[k] = want[k]
            }
            _done[item.objectName] = true
        }
        for (var i = 0; i < item.children.length; ++i) _visit(item.children[i])
        if (item.contentItem && item.contentItem !== item) _visit(item.contentItem)
    }
    Timer {
        id: setter
        interval: 150
        repeat: true
        property int left: 12
        onTriggered: {
            hook._visit(hook.target)
            if (--left <= 0) stop()
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
            pending: Api.pending
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
