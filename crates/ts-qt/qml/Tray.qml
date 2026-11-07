// The system tray icon and its menu: Open, the power mode (radio items), the fan mode, the first
// five favourites (click applies), an "Open page" submenu, Quit.
//
// Safety, in the order it matters:
//  * Main.qml loads this file through a Loader and never under the offscreen platform: a tray there
//    waits for a StatusNotifier host that does not exist and hangs the program. Nothing in this file
//    may change that, and nothing here creates a tray by itself.
//  * `visible: available`: on a desktop without a tray (some GNOME setups) the icon is never shown
//    and `Ui.trayAvailable` stays false, so closing the window ends the program instead of hiding it.
//  * No timer, no polling, no command at creation: the menu asks the machine only when it is about
//    to open (`aboutToShow`), so an idle tray costs nothing and a missing host cannot block anything.
//
// What the menu contains and what each entry does is decided by ts-ui-model (vm/tray.rs, tested);
// this file only draws it and forwards the clicked entry's id.
import QtQuick
import QtQml
import Qt.labs.platform as Platform
import org.truesense.app

Platform.SystemTrayIcon {
    id: tray

    signal openRequested()
    signal quitRequested()

    /// The newest `{summary, entries}` from the view-model.
    property var model: ({ summary: {}, entries: [] })
    /// When the model was last built (ms since the epoch): the live frames arrive every second and
    /// the tooltip and the menu need nothing fresher than a few seconds, while every build is a
    /// round trip through JSON into the view-model; see `onFrameChanged` below.
    property double builtAt: 0
    property var favourites: []
    property var supported: undefined
    /// The last status the menu was built from (a live frame, then the answer of `status get`).
    property var status: null
    /// Bumped to make every radio item read its state again after a click (Qt flips `checked` itself).
    property int generation: 0

    readonly property var summary: model.summary
    /// Where the first favourite slot sits in the menu: after Open and a separator, the power block
    /// (title and 5 modes), a separator, the fan block (title and 4 lines), a separator, the
    /// Favourites title and the "none starred" line.
    readonly property int favouritesAt: 2 + 6 + 1 + 5 + 1 + 2

    visible: available
    icon.source: "qrc:/assets/truesense-256.png"
    tooltip: summary.mode ? tooltipText() : "TrueSense"
    onActivated: (reason) => {
        if (reason === Platform.SystemTrayIcon.Trigger) openRequested()
    }

    function vm(verb, args) {
        var r = JSON.parse(Bus.vm("tray", verb, JSON.stringify(args)))
        return r.ok ? r.result : null
    }

    function build(frame, favs, modes) {
        var pages = Nav.pages.map(function (p) { return { id: p.id, title: Nav.titleOf(p) } })
        var args = { status: frame === null || frame === undefined ? {} : frame, favourites: favs, pages: pages }
        if (modes !== undefined) args.supported = modes
        var m = vm("menu", args)
        return m === null ? { summary: {}, entries: [] } : m
    }

    function rebuild() {
        model = build(status, favourites, supported)
        generation++
        builtAt = Date.now()
    }

    // The live frame no longer rebuilds the model by itself (a binding on `Live.frame` did, once a
    // second, whether or not anybody looked at the tray): at most every 5 s. Opening the menu reads
    // the machine afresh through `refresh()`, so nothing shown there is older than that.
    property bool liveFed: true
    Component.onCompleted: { status = Live.frame; rebuild() }
    Connections {
        target: Live
        function onFrameChanged() {
            if (tray.liveFed && Date.now() - tray.builtAt > 5000) {
                tray.status = Live.frame
                tray.rebuild()
            }
        }
    }

    /// The entry with this id, or null (a favourite slot that is not filled, the manual-fan line).
    function entry(id) {
        var list = model.entries
        for (var i = 0; i < list.length; ++i) {
            if (list[i].id === id) return list[i]
        }
        return null
    }

    function checked(id) {
        generation   // a dependency: re-read after every click
        var e = entry(id)
        return e !== null && e.checked === true
    }

    function allowed(id) {
        var e = entry(id)
        return e === null || e.enabled !== false
    }

    function modeLabel(mode) {
        switch (mode) {
        //% "Eco"
        case "eco": return qsTrId("tray_mode_eco")
        //% "Quiet"
        case "quiet": return qsTrId("tray_mode_quiet")
        //% "Balanced"
        case "balanced": return qsTrId("tray_mode_balanced")
        //% "Performance"
        case "performance": return qsTrId("tray_mode_performance")
        //% "Turbo"
        case "turbo": return qsTrId("tray_mode_turbo")
        default: return ""
        }
    }

    function tooltipText() {
        //% "TrueSense: %1, CPU %2, GPU %3"
        return qsTrId("tray_tooltip").arg(modeLabel(summary.mode)).arg(summary.cpu).arg(summary.gpu)
    }

    /// Reads the machine once, as the menu is about to open (the window may be hidden, so the live
    /// frame can be old). A failed read keeps what the menu already shows.
    function refresh() {
        Api.call("status", "get", {}, function (s) { status = s; rebuild() }, function () {})
        Api.call("favourite", "list", {}, function (f) { favourites = f; rebuild() }, function () {})
        Api.call("power", "status", {}, function (p) {
            supported = Array.isArray(p.supported) ? p.supported : undefined
            rebuild()
        }, function () {})
    }

    /// A menu entry was clicked.
    function invoke(id) {
        var c = vm("command", { id: id })
        generation++
        if (c === null) return
        switch (c.type) {
        case "open": openRequested(); break
        case "quit": quitRequested(); break
        case "page":
            Nav.go(c.page)
            Nav.raiseRequested()
            break
        case "command":
            Api.call(c.area, c.verb, c.args, function () { refresh() }, function (err) {
                // The window may be hidden: a desktop notification as well as the in-window toast.
                Toasts.error(err.message)
                tray.showMessage("TrueSense", err.message, Platform.SystemTrayIcon.Warning, 5000)
                refresh()
            })
            break
        }
    }

    component Title: Platform.MenuItem {
        required property string label
        text: label
        enabled: false
    }

    component Choice: Platform.MenuItem {
        required property string entryId
        required property string label
        checkable: true
        checked: tray.checked(entryId)
        enabled: tray.allowed(entryId)
        text: label
        onTriggered: tray.invoke(entryId)
    }

    menu: Platform.Menu {
        id: trayMenu
        onAboutToShow: tray.refresh()

        Platform.MenuItem {
            //% "Open TrueSense"
            text: qsTrId("tray_open")
            onTriggered: tray.invoke("open")
        }
        Platform.MenuSeparator {}

        Title {
            //% "Power mode"
            label: qsTrId("tray_title_power")
        }
        Choice { entryId: "power:eco"; label: tray.modeLabel("eco") }
        Choice { entryId: "power:quiet"; label: tray.modeLabel("quiet") }
        Choice { entryId: "power:balanced"; label: tray.modeLabel("balanced") }
        Choice { entryId: "power:performance"; label: tray.modeLabel("performance") }
        Choice { entryId: "power:turbo"; label: tray.modeLabel("turbo") }
        Platform.MenuSeparator {}

        Title {
            //% "Fans"
            label: qsTrId("tray_title_fan")
        }
        Choice {
            entryId: "fan:smart"
            //% "Smart curve"
            label: qsTrId("tray_fan_smart")
        }
        Choice {
            entryId: "fan:auto"
            //% "Firmware auto"
            label: qsTrId("tray_fan_auto")
        }
        Choice {
            entryId: "fan:max"
            //% "Max"
            label: qsTrId("tray_fan_max")
        }
        Choice {
            // Shown only while the fans run at a fixed speed set on the Cooling page: it is a state,
            // never something a menu click can ask for (it could not say how fast).
            entryId: "fan:manual"
            visible: tray.entry("fan:manual") !== null
            //% "Fixed speed (set on the Cooling page)"
            label: qsTrId("tray_fan_manual")
        }
        Platform.MenuSeparator {}

        Title {
            //% "Favourites"
            label: qsTrId("tray_title_favourites")
        }
        Title {
            visible: tray.entry("note:no-favourites") !== null
            //% "None starred yet"
            label: qsTrId("tray_no_favourites")
        }
        Instantiator {
            model: 5
            delegate: Platform.MenuItem {
                required property int index
                readonly property var item: tray.entry("favourite:" + (index + 1))
                visible: item !== null
                enabled: item !== null && item.enabled !== false
                text: item === null ? "" : item.label
                onTriggered: tray.invoke("favourite:" + (index + 1))
            }
            onObjectAdded: (index, object) => trayMenu.insertItem(tray.favouritesAt + index, object)
            onObjectRemoved: (index, object) => trayMenu.removeItem(object)
        }
        Platform.MenuSeparator {}

        Platform.Menu {
            id: pagesMenu
            //% "Open page"
            title: qsTrId("tray_pages")
            Instantiator {
                model: Nav.pages
                delegate: Platform.MenuItem {
                    required property var modelData
                    text: Nav.titleOf(modelData)
                    onTriggered: tray.invoke("page:" + modelData.id)
                }
                onObjectAdded: (index, object) => pagesMenu.insertItem(index, object)
                onObjectRemoved: (index, object) => pagesMenu.removeItem(object)
            }
        }
        Platform.MenuSeparator {}

        Platform.MenuItem {
            //% "Quit"
            text: qsTrId("tray_quit")
            onTriggered: tray.invoke("quit")
        }
    }
}
