// Home: the page the old window opened on. Big power-mode tiles with the Manual / Native / Smart
// control, fan mode tiles with the manual sliders, GPU mode tiles, four live readings with history,
// the favourites strip and, only when something is wrong, a health banner.
//
// Every decision (which tile is enabled, which mode is in force, whether a hold runs, what the
// sliders send) is in ts-ui-model/src/vm/home.rs; this file lays it out and sends the commands.
// Refreshing from the live frame never sends a command: the components emit `activated` only for
// the person's own input.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

PageFrame {
    id: page
    objectName: "homePage"

    title: Nav.currentPage ? Nav.titleOf(Nav.currentPage) : ""

    // ---- what the page knows beyond the live frame -------------------------------------------
    property var supportedModes: null      // `power status` supported (null: not read yet)
    property var smartProfiles: []
    /// The Smart profile in use as `power smart current` says it, for when the live frame does not
    /// name one (the selector would be blank otherwise).
    property string smartCurrent: ""
    property var fanCurves: []
    property bool statusLoaded: false
    property string gpuMode: ""             // "" = the laptop has no GPU switch (once statusLoaded)
    property string gpuAtLoad: ""
    property var capItems: null              // `capabilities get` items, for the health banner
    property var favourites: ({ items: [], empty: true })

    // What the person just picked, shown until the next live frame confirms or corrects it.
    property string powerPick: ""
    property string controlPick: ""
    property string fanPick: ""

    // Manual fan state (never persisted: pages are destroyed when left).
    property bool linked: true
    property double lastTouch: 0

    readonly property var emptyView: ({
        control: { daemon_up: false, mode_control: null, smart_profile: null, now: null,
                   hold: { active: false, seconds: 0, control: null }, tiles: [] },
        fans: { mode: "auto", duty: null, stopped: false, fan_curve: null, tiles: [] },
        tiles: { cpu: {}, gpu: {}, fan: {}, battery: {} },
        banner: { show: false, problems: [] }
    })
    readonly property var view: Vm.call("home", "view", { frame: Live.frame, items: capItems }) || emptyView
    readonly property var control: view.control
    readonly property var fans: view.fans
    readonly property var tiles: view.tiles

    readonly property string shownPower: powerPick !== "" ? powerPick : (Live.value("power_mode") || "")
    readonly property string shownControl: controlPick !== "" ? controlPick : (control.mode_control || "")
    readonly property string shownFan: fanPick !== "" ? fanPick : (fans.mode || "auto")

    function labelOfPower(m) {
        switch (m) {
        //% "Eco"
        case "eco": return qsTrId("home_mode_eco")
        //% "Quiet"
        case "quiet": return qsTrId("home_mode_quiet")
        //% "Balanced"
        case "balanced": return qsTrId("home_mode_balanced")
        //% "Performance"
        case "performance": return qsTrId("home_mode_performance")
        //% "Turbo"
        case "turbo": return qsTrId("home_mode_turbo")
        }
        return Fmt.capitalize(m)
    }
    function labelOfControl(c) {
        switch (c) {
        //% "Manual"
        case "off": return qsTrId("home_control_manual")
        //% "Native"
        case "native": return qsTrId("home_control_native")
        //% "Smart"
        case "smart": return qsTrId("home_control_smart")
        }
        return c
    }
    function hintOfControl(c) {
        switch (c) {
        //% "You choose the mode. A mode you pick stays until you change it."
        case "off": return qsTrId("home_hint_manual")
        //% "Like PredatorSense on Windows: Balanced on power, Eco on battery, nothing else."
        case "native": return qsTrId("home_hint_native")
        //% "Switches by heat, load and battery: Performance and Turbo when gaming or heavy work (backs off if it overheats), Eco on battery. A mode you pick yourself ends by itself when the situation changes."
        case "smart": return qsTrId("home_hint_smart")
        }
        return ""
    }
    function labelOfFan(m) {
        switch (m) {
        //% "Smart"
        case "smart": return qsTrId("home_fan_smart")
        //% "Auto"
        case "auto": return qsTrId("home_fan_auto")
        //% "Max"
        case "max": return qsTrId("home_fan_max")
        //% "Manual"
        case "manual": return qsTrId("home_fan_manual")
        }
        return m
    }
    function hintOfFan(m) {
        switch (m) {
        //% "Follows your fan curve: quiet when cool, faster as it heats. Edit it under Cooling."
        case "smart": return qsTrId("home_fanhint_smart")
        //% "The firmware's own control, exactly like Auto in PredatorSense on Windows (the fans stop when cool)."
        case "auto": return qsTrId("home_fanhint_auto")
        //% "Both fans at full speed until you pick another mode."
        case "max": return qsTrId("home_fanhint_max")
        //% "The sliders go from 20 to 100 %. Turn fans off asks first and only works while cool; the fans return to Auto by themselves if it gets warm. On this laptop the firmware may keep them at a minimum speed, and TrueSense tells you what they really do."
        case "manual": return qsTrId("home_fanhint_manual")
        }
        return ""
    }
    function labelOfGpu(m) {
        switch (m) {
        //% "Optimus"
        case "optimus": return qsTrId("home_gpu_optimus")
        //% "Nvidia"
        case "nvidia": return qsTrId("home_gpu_nvidia")
        //% "Auto"
        case "auto": return qsTrId("home_gpu_auto")
        }
        return Fmt.capitalize(m)
    }
    function kindName(kind) {
        switch (kind) {
        //% "Look"
        case "look": return qsTrId("home_kind_look")
        //% "Fan curve"
        case "fan_curve": return qsTrId("home_kind_curve")
        //% "Smart profile"
        case "smart_profile": return qsTrId("home_kind_smart")
        //% "Profile"
        case "profile": return qsTrId("home_kind_profile")
        //% "Theme"
        case "theme": return qsTrId("home_kind_theme")
        }
        return ""
    }

    /// The tiles of a vm answer with the page's words (and the icon named like the value unless given).
    function dressed(list, labeler, iconOf) {
        return (list || []).map(function (t) {
            return { value: t.value, enabled: t.enabled, label: labeler(t.value), icon: iconOf(t.value) }
        })
    }
    readonly property var powerTiles: dressed(
        Vm.call("home", "power_tiles", { supported: supportedModes }), labelOfPower, function (v) { return v })
    readonly property var controlTiles: dressed(control.tiles, labelOfControl, function (v) { return "" })
    readonly property var fanTiles: dressed(fans.tiles, labelOfFan, function (v) { return v })
    readonly property var gpuTiles: dressed(
        Vm.call("home", "gpu_tiles", { current: !statusLoaded ? null : (gpuMode === "" ? "none" : gpuMode) }),
        labelOfGpu, function (v) { return v === "nvidia" ? "gpu" : v })

    // ---- reading the machine once when the page opens ----------------------------------------
    function quiet(err) { /* a background read that failed leaves its part of the page empty */ }

    function refresh() {
        Api.call("power", "status", {}, function (r) {
            supportedModes = (r && Array.isArray(r.supported)) ? r.supported : null
        }, quiet)
        Api.call("power", "smart", { action: "list" }, function (r) { smartProfiles = Array.isArray(r) ? r : [] }, quiet)
        Api.call("power", "smart", { action: "current" }, function (r) {
            // No name means the built-in profile is in use (the Performance page reads it the same way).
            smartCurrent = (r && typeof r.name === "string" && r.name !== "") ? r.name : "native"
        }, quiet)
        Api.call("fan", "curve", { action: "list" }, function (r) { fanCurves = Array.isArray(r) ? r : [] }, quiet)
        Api.call("status", "get", {}, function (r) {
            gpuMode = (r && typeof r.gpu_mode === "string") ? r.gpu_mode : ""
            if (gpuAtLoad === "") gpuAtLoad = gpuMode
            statusLoaded = true
        }, function (e) { statusLoaded = true })
        readCapabilities()
        loadFavourites()
        readQuick()
    }
    // ---- quick actions: what the lights and the theme are now (read once, and after each action)
    property int kbLevel: -1            // keyboard brightness, -1 = unknown
    property int rearLevel: -1
    property string runningTheme: ""
    function readQuick() {
        Api.call("keyboard", "status", {}, function (r) {
            kbLevel = (r && typeof r.brightness === "number") ? r.brightness : -1
        }, quiet)
        Api.call("rear", "status", {}, function (r) {
            rearLevel = (r && typeof r.brightness === "number") ? r.brightness : -1
        }, quiet)
        Api.call("theme", "list", {}, function (r) {
            var a = r && r.status && r.status.active ? r.status.active.spec : null
            var name = ""
            if (a) {
                name = a.name
                for (var i = 0; i < (r.themes || []).length; ++i) if (r.themes[i].id === a.name) name = r.themes[i].name
            }
            runningTheme = name
        }, quiet)
    }
    function readCapabilities() {
        Api.call("capabilities", "get", {}, function (r) {
            capItems = (r && Array.isArray(r.items)) ? r.items : null
        }, quiet)
    }
    function loadFavourites() {
        Api.call("favourite", "list", {}, function (r) {
            favourites = Vm.call("profiles", "favourites", { list: r }) || ({ items: [], empty: true })
        }, quiet)
    }
    Component.onCompleted: { refresh(); syncSliders() }

    // A frame arrived: what the person picked is now either confirmed or corrected by the machine.
    Connections {
        target: Live
        function onSerialChanged() {
            page.powerPick = ""
            page.controlPick = ""
            page.fanPick = ""
            page.syncSliders()
        }
        // The service came up (or went down) since the capability list was read.
        function onDaemonUpChanged() { page.readCapabilities() }
    }

    // ---- actions -----------------------------------------------------------------------------
    /// The fail callback of an optimistic tile row: the row shows the machine's state again.
    function failed(row) {
        return function (err) { row.settle(); Toasts.error(err && err.message ? err.message : String(err)) }
    }
    function pickPower(mode) {
        Api.call("power", "set", { mode: mode }, function (r) {
            powerPick = mode
            //% "Power mode: %1 (held 30 min)"
            Toasts.ok(qsTrId("home_toast_power").arg(labelOfPower(typeof r === "string" && r !== "" ? r : mode)))
        }, failed(powerRow))
    }
    function pickControl(c) {
        Api.call("power", "auto", { action: c }, function (r) {
            controlPick = c
            //% "Mode control: %1"
            Toasts.ok(qsTrId("home_toast_control").arg(labelOfControl(c)))
        })
    }
    function useSmartProfile(name) {
        Api.call("power", "smart", { action: "use", name: name }, function (r) {
            page.smartCurrent = r && r.name ? r.name : name
            //% "Smart auto profile in use: %1"
            Toasts.ok(qsTrId("home_toast_smart").arg(r && r.name ? r.name : name))
        })
    }
    function useCurve(name) {
        Api.call("fan", "curve", { action: "use", name: name }, function (r) {
            //% "Fan curve in use: %1"
            Toasts.ok(qsTrId("home_toast_curve").arg(r && r.name ? r.name : name))
        })
    }
    function pickFan(mode) {
        if (mode === "smart") {
            Api.call("fan", "smart", { action: "on" }, function () {
                fanPick = mode
                //% "Fans: smart"
                Toasts.ok(qsTrId("home_toast_fan_smart"))
            }, failed(fanRow))
        } else if (mode === "auto") {
            Api.call("fan", "auto", {}, function () {
                fanPick = mode
                //% "Fans: firmware auto"
                Toasts.ok(qsTrId("home_toast_fan_auto"))
            }, failed(fanRow))
        } else if (mode === "max") {
            Api.call("fan", "max", {}, function () {
                fanPick = mode
                //% "Fans: max"
                Toasts.ok(qsTrId("home_toast_fan_max"))
            }, failed(fanRow))
        } else {
            enterManual()
        }
    }

    // ---- manual fans ---------------------------------------------------------------------------
    function enterManual() {
        var d = fans.duty
        if (d) {
            linked = d.cpu === d.gpu
            setSliders(d.cpu, d.gpu)
        }
        var out = Vm.call("home", "duties", { linked: linked, single: single.value, cpu: cpuSlider.value, gpu: gpuSlider.value })
        if (!out) { fanRow.settle(); return }
        lastTouch = Date.now()
        Api.call("fan", "set", { cpu: out.cpu, gpu: out.gpu }, function () {
            fanPick = "manual"
            //% "Fans: manual %1 % / %2 %"
            Toasts.ok(qsTrId("home_toast_fan_manual").arg(out.cpu).arg(out.gpu))
        }, failed(fanRow))
    }
    function setSliders(cpu, gpu) {
        single.value = Math.max(cpu, gpu)
        cpuSlider.value = cpu
        gpuSlider.value = gpu
    }
    // The sliders follow the machine unless the person touched them a moment ago.
    function syncSliders() {
        if (fans.mode !== "manual" || !fans.duty) return
        if (single.dragging || cpuSlider.dragging || gpuSlider.dragging) return
        if (Date.now() - lastTouch < 3000) return
        setSliders(fans.duty.cpu, fans.duty.gpu)
    }
    function writeDuty() {
        lastTouch = Date.now()
        var out = Vm.call("home", "duties", { linked: linked, single: single.value, cpu: cpuSlider.value, gpu: gpuSlider.value })
        if (out) Api.write("fan.duty", "fan", "set", { cpu: out.cpu, gpu: out.gpu })
    }
    function toggleLink(on) {
        var r = Vm.call("home", "link", { linked: on, single: single.value, cpu: cpuSlider.value, gpu: gpuSlider.value })
        linked = on
        if (r) {
            single.value = r.single
            cpuSlider.value = r.cpu
            gpuSlider.value = r.gpu
        }
        if (shownFan === "manual") writeDuty()
    }
    function askFansOff() {
        confirm.ask({
            //% "Turn the fans off"
            title: qsTrId("home_off_title"),
            text: [
                //% "Stop both fans?"
                qsTrId("home_off_1"),
                //% "With the fans off the laptop can overheat quickly under any load. This is only for quiet, light use."
                qsTrId("home_off_2"),
                //% "It is only allowed while the CPU is under 60 °C and the GPU under 55 °C. If the CPU reaches 70 °C or the GPU 65 °C, TrueSense gives the fans back to the firmware automatically."
                qsTrId("home_off_3"),
                //% "On this laptop the firmware may keep the fans at a minimum speed. TrueSense checks the fan speed afterwards and tells you what really happened."
                qsTrId("home_off_4")
            ].join("\n\n"),
            //% "Turn fans off"
            confirmText: qsTrId("home_off_confirm"),
            danger: true
        }, function () {
            Api.call("fan", "off", { confirm: true }, function () {
                //% "Fans stopped. Checking what they really do…"
                Toasts.show(qsTrId("home_off_checking"), "info")
                verify.restart()
            })
        })
    }
    Timer {
        id: verify
        interval: 8000
        onTriggered: {
            var cpu = Live.value("sensors.cpu_fan_rpm")
            var gpu = Live.value("sensors.gpu_fan_rpm")
            var v = Vm.call("home", "stop_verdict", { cpu_rpm: cpu === undefined ? null : cpu, gpu_rpm: gpu === undefined ? null : gpu })
            if (!v || !v.known) return
            if (v.turning)
                //% "The firmware accepted the stop but the fans still turn (CPU %1, GPU %2): it keeps a minimum speed in manual mode, so the fans cannot really be stopped on this laptop. They stay under TrueSense's watch; Auto brings them back to the firmware."
                Toasts.show(qsTrId("home_off_still").arg(Fmt.rpm(cpu)).arg(Fmt.rpm(gpu)), "info")
            else
                //% "Fans stopped (confirmed by the fan speed). They come back on Auto automatically if the laptop gets warm."
                Toasts.ok(qsTrId("home_off_stopped"))
        }
    }

    // ---- GPU -----------------------------------------------------------------------------------
    function askGpu(mode) {
        confirm.ask({
            //% "Change the GPU mode"
            title: qsTrId("home_gpu_title"),
            //% "Switch the GPU mode to %1? It takes effect after the next reboot. Pick your previous mode again before rebooting to cancel."
            text: qsTrId("home_gpu_text").arg(labelOfGpu(mode)),
            //% "Apply"
            confirmText: qsTrId("home_gpu_apply")
        }, function () {
            Api.call("gpu", "set", { mode: mode }, function (r) {
                gpuMode = mode
                //% "GPU mode: %1 (reboot to apply)"
                Toasts.ok(qsTrId("home_toast_gpu").arg(labelOfGpu(typeof r === "string" && r !== "" ? r : mode)))
            })
        })
    }

    ConfirmDialog { id: confirm }

    // ---- health banner (only when something is wrong) ----------------------------------------
    ColumnLayout {
        Layout.fillWidth: true
        spacing: 8
        visible: page.view.banner.show
        Repeater {
            model: page.view.banner.problems.length
            delegate: HomeBanner {
                required property int index
                readonly property var modelData: page.view.banner.problems[index]
                Layout.fillWidth: true
                kind: modelData.kind === "daemon" ? "warn" : "bad"
                text: modelData.kind === "daemon"
                    //% "The TrueSense service is not running. Smart auto, power limits and undervolt are unavailable until it starts."
                    ? qsTrId("home_banner_daemon")
                    : modelData.why
                //% "Open Health"
                actionText: qsTrId("home_banner_action")
                onActionClicked: Nav.go("health")
            }
        }
    }

    // ---- dashboard: the tiles the person chose (Settings or "Customise" here) -------------------
    readonly property var dashTiles: Vm.call("home", "dashboard", { frame: Live.frame, tiles: Ui.homeTiles }) || []
    property bool editingDash: false
    readonly property var dashChoices: Vm.call("home", "dashboard_choices", { tiles: Ui.homeTiles }) || ({ items: [], full: false })

    function dashLabel(id) {
        switch (id) {
        //% "CPU"
        case "cpu": return qsTrId("home_tile_cpu")
        //% "GPU"
        case "gpu": return qsTrId("home_tile_gpu")
        //% "Fans"
        case "fans": return qsTrId("home_tile_fans")
        //% "Battery"
        case "battery": return qsTrId("home_tile_battery")
        //% "Power mode"
        case "power": return qsTrId("home_tile_power")
        //% "Board"
        case "board": return qsTrId("home_tile_board")
        //% "CPU load"
        case "cpu_load": return qsTrId("home_tile_cpu_load")
        //% "GPU load"
        case "gpu_load": return qsTrId("home_tile_gpu_load")
        //% "CPU power"
        case "cpu_watts": return qsTrId("home_tile_cpu_watts")
        //% "GPU power"
        case "gpu_watts": return qsTrId("home_tile_gpu_watts")
        }
        return id
    }
    function dashValue(t) {
        if (t.unit === "mode") return t.value ? labelOfPower(t.value) : Fmt.unknown
        if (t.value === null || t.value === undefined) return Fmt.unknown
        if (t.unit === "temp") return Fmt.temp(t.value)
        if (t.unit === "rpm") return Fmt.rpm(t.value)
        if (t.unit === "percent") return Fmt.percent(t.value)
        if (t.unit === "watts") return Fmt.watts(t.value)
        return String(t.value)
    }
    function dashSub(t) {
        switch (t.id) {
        //% "Load %1"
        case "cpu": return qsTrId("home_tile_load").arg(Fmt.percent(page.tiles.cpu.load))
        case "gpu": return qsTrId("home_tile_load").arg(Fmt.percent(page.tiles.gpu.load))
        case "fans":
            //% "CPU %1 · GPU %2"
            return qsTrId("home_tile_fan_sub").arg(t.sub.cpu_rpm == null ? Fmt.unknown : Math.round(t.sub.cpu_rpm))
                                              .arg(t.sub.gpu_rpm == null ? Fmt.unknown : Math.round(t.sub.gpu_rpm))
        case "battery":
            return t.sub.ac === true
                //% "Plugged in"
                ? qsTrId("home_tile_ac")
                : t.sub.ac === false
                    //% "On battery"
                    ? qsTrId("home_tile_batt") : ""
        //% "Click for the next mode"
        case "power": return qsTrId("home_tile_power_sub")
        }
        return ""
    }
    /// The power mode after the one in force, among those this laptop has (the mode tile's click).
    function nextPower() {
        var usable = page.powerTiles.filter(function (t) { return t.enabled }).map(function (t) { return t.value })
        if (usable.length === 0) return
        var i = usable.indexOf(page.shownPower)
        page.pickPower(usable[(i + 1) % usable.length])
    }
    function setTiles(list) { Ui.set("home_tiles", list) }
    function moveTile(id, delta) {
        var l = Ui.homeTiles.slice()
        var i = l.indexOf(id)
        var j = i + delta
        if (i < 0 || j < 0 || j >= l.length) return
        l.splice(i, 1)
        l.splice(j, 0, id)
        setTiles(l)
    }
    function toggleTile(id, show) {
        var l = Ui.homeTiles.filter(function (x) { return x !== id })
        if (show) l.push(id)
        if (l.length > 0) setTiles(l)
    }

    RowLayout {
        Layout.fillWidth: true
        spacing: 8
        Text {
            Layout.fillWidth: true
            //% "Dashboard"
            text: qsTrId("home_dash_title")
            color: Theme.stat
            font.pixelSize: Theme.fontSmall
            font.bold: true
            font.letterSpacing: 1
            Accessible.role: Accessible.Heading
        }
        SecondaryButton {
            text: page.editingDash ? page._doneWord : page._customiseWord
            iconName: page.editingDash ? "check" : "sliders"
            onClicked: page.editingDash = !page.editingDash
        }
    }
    //% "Done"
    readonly property string _doneWord: qsTrId("home_dash_done_edit")
    //% "Customise"
    readonly property string _customiseWord: qsTrId("home_dash_edit")

    // The editor: which tiles show and in what order (saved in ui.json, so it stays).
    Card {
        Layout.fillWidth: true
        visible: page.editingDash
        //% "Dashboard tiles"
        title: qsTrId("home_dash_editor_title")
        icon: "sliders"
        //% "Up to %1 tiles"
        rightText: qsTrId("home_dash_editor_max").arg(6)

        HelpText {
            //% "Choose the readings Home shows and their order. The power mode tile changes the mode when you click it."
            text: qsTrId("home_dash_editor_help")
        }
        Repeater {
            model: page.dashChoices.items
            delegate: RowLayout {
                required property var modelData
                Layout.fillWidth: true
                spacing: 8
                ToggleRow {
                    Layout.fillWidth: true
                    label: page.dashLabel(modelData.id)
                    checked: modelData.shown
                    enabled: modelData.shown ? modelData.can_hide : modelData.can_show
                    onToggled: (on) => page.toggleTile(modelData.id, on)
                }
                ListIconButton {
                    iconName: "chevron"
                    iconRotation: -90
                    enabled: modelData.can_up
                    opacity: modelData.shown ? 1 : 0
                    //% "Move up"
                    text: qsTrId("home_dash_up")
                    onClicked: page.moveTile(modelData.id, -1)
                }
                ListIconButton {
                    iconName: "chevron"
                    iconRotation: 90
                    enabled: modelData.can_down
                    opacity: modelData.shown ? 1 : 0
                    //% "Move down"
                    text: qsTrId("home_dash_down")
                    onClicked: page.moveTile(modelData.id, 1)
                }
            }
        }
        SettingRow {
            Layout.fillWidth: true
            SecondaryButton {
                //% "Back to the default tiles"
                text: qsTrId("home_dash_reset")
                onClicked: page.setTiles(["cpu", "gpu", "fans", "battery"])
            }
        }
    }

    GridLayout {
        Layout.fillWidth: true
        // As many as fit (170 px each), then balanced: four tiles that do not fit in a row make two
        // rows of two, not three and one.
        readonly property int fit: Math.max(1, Math.floor((width + columnSpacing) / (170 + columnSpacing)))
        readonly property int n: Math.max(1, page.dashTiles.length)
        columns: Math.ceil(n / Math.ceil(n / Math.min(fit, n)))
        columnSpacing: Theme.spacing
        rowSpacing: Theme.spacing

        Repeater {
            model: page.dashTiles.length
            delegate: DashTile {
                required property int index
                readonly property var t: page.dashTiles[index]
                Layout.fillWidth: true
                Layout.preferredWidth: 1
                Layout.fillHeight: true
                label: page.dashLabel(t.id)
                icon: t.icon
                kind: t.kind
                known: t.value !== null && t.value !== undefined
                value: known && t.kind === "dial" ? t.value : 0
                from: t.from !== undefined ? t.from : 0
                to: t.to !== undefined ? t.to : 100
                warnAt: t.warn !== undefined ? t.warn : NaN
                critAt: t.crit !== undefined ? t.crit : NaN
                valueText: page.dashValue(t)
                subText: page.dashSub(t)
                modeIcon: t.kind === "mode" && t.value ? t.value : "performance"
                hasHistory: t.series !== null
                values: t.series !== null ? Live.history(t.series) : []
                historyMinimum: t.unit === "temp" ? NaN : 0
                historyMaximum: t.unit === "percent" ? 100 : NaN
                hot: t.hot === true
                clickable: t.kind === "mode"
                //% "Power mode: %1. Click for the next mode."
                Accessible.description: t.kind === "mode" ? qsTrId("home_tile_power_a11y").arg(valueText) : ""
                onClicked: page.nextPower()
            }
        }
    }

    // ---- quick actions --------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Quick actions"
        title: qsTrId("home_quick_title")
        icon: "sparkle"

        Flow {
            Layout.fillWidth: true
            spacing: 8
            SecondaryButton {
                iconName: "profiles"
                //% "Next profile"
                text: qsTrId("home_quick_next_profile")
                onClicked: Api.call("profile", "next", {}, function (r) {
                    //% "Next profile applied"
                    Toasts.ok(typeof r === "string" && r !== "" ? r : qsTrId("home_quick_next_done"))
                })
            }
            SecondaryButton {
                iconName: "keyboard"
                enabled: page.kbLevel >= 0 && Caps.usable("keyboard-light")
                text: page.kbLevel > 0
                    //% "Keyboard light off"
                    ? qsTrId("home_quick_kb_off")
                    //% "Keyboard light on"
                    : qsTrId("home_quick_kb_on")
                onClicked: Api.call("keyboard", page.kbLevel > 0 ? "off" : "on", {}, function () { page.readQuick() })
            }
            SecondaryButton {
                iconName: "bar"
                enabled: page.rearLevel >= 0 && Caps.usable("rear-light")
                text: page.rearLevel > 0
                    //% "Rear light off"
                    ? qsTrId("home_quick_rear_off")
                    //% "Rear light on"
                    : qsTrId("home_quick_rear_on")
                onClicked: Api.call("rear", page.rearLevel > 0 ? "off" : "on", {}, function () { page.readQuick() })
            }
            SecondaryButton {
                visible: page.runningTheme !== ""
                iconName: "stop"
                //% "Stop the theme %1"
                text: qsTrId("home_quick_stop_theme").arg(page.runningTheme)
                onClicked: Api.call("theme", "stop", {}, function () { page.readQuick() })
            }
            SecondaryButton {
                iconName: "palette"
                //% "Lighting themes"
                text: qsTrId("home_quick_themes")
                onClicked: Nav.go("lighting")
            }
        }
    }

    // ---- favourites strip (hidden while there are none) -----------------------------------------
    Card {
        Layout.fillWidth: true
        visible: !page.favourites.empty
        //% "Favourites"
        title: qsTrId("home_fav_title")
        icon: "star"

        Flow {
            Layout.fillWidth: true
            Layout.preferredHeight: childrenRect.height
            spacing: 8
            Repeater {
                model: page.favourites.items
                delegate: FavouriteChip {
                    required property var modelData
                    text: modelData.title !== undefined ? modelData.title : modelData.name
                    iconName: modelData.icon
                    missing: !modelData.exists
                    kindLabel: page.kindName(modelData.kind)
                    onClicked: Api.call("favourite", "apply", { position: modelData.position }, function (r) {
                        Toasts.ok(typeof r === "string" && r !== "" ? r : modelData.name)
                    })
                }
            }
        }
    }

    // ---- power mode ----------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Performance mode"
        title: qsTrId("home_power_title")
        icon: "performance"
        //% "CPU: %1 · %2"
        rightText: qsTrId("home_power_right").arg(Fmt.temp(page.tiles.cpu.temp)).arg(Fmt.rpm(Live.value("sensors.cpu_fan_rpm")))

        TileRow {
            id: powerRow
            Layout.fillWidth: true
            optimistic: true
            model: page.powerTiles
            current: page.shownPower !== "" ? page.shownPower : undefined
            onActivated: (value) => page.pickPower(value)
        }

        SettingRow {
            Layout.fillWidth: true
            //% "Mode control"
            label: qsTrId("home_control_label")
            Segmented {
                model: page.controlTiles
                current: page.shownControl !== "" ? page.shownControl : undefined
                onActivated: (value) => page.pickControl(value)
            }
        }
        CapabilityNote { capability: "smart-auto" }
        HelpText { visible: page.shownControl !== ""; text: page.hintOfControl(page.shownControl) }

        ColumnLayout {
            Layout.fillWidth: true
            spacing: 8
            visible: page.shownControl === "smart"
            SelectRow {
                Layout.fillWidth: true
                //% "Smart profile"
                label: qsTrId("home_smart_profile")
                options: page.smartProfiles
                // The live frame's name when it is one of the list, else what `power smart current` said.
                current: page.smartProfiles.indexOf(page.control.smart_profile) >= 0
                    ? page.control.smart_profile : (page.smartProfiles.indexOf(page.smartCurrent) >= 0 ? page.smartCurrent : undefined)
                onActivated: (value) => page.useSmartProfile(value)
                SecondaryButton {
                    //% "Edit"
                    text: qsTrId("home_edit")
                    onClicked: Nav.go("performance")
                }
            }
            HelpText {
                visible: page.control.now !== null && page.control.now !== undefined
                //% "Now: %1"
                text: qsTrId("home_now").arg(page.control.now)
            }
        }

        GridLayout {
            Layout.fillWidth: true
            columns: page.width < 560 ? 1 : 2
            columnSpacing: Theme.spacing
            rowSpacing: 8
            visible: page.control.hold.active
            HelpText {
                //% "Your pick stays until something changes (plug, a game starting or stopping, heat) or %1 more. Then %2 auto takes over by itself."
                text: qsTrId("home_hold").arg(Fmt.duration(page.control.hold.seconds))
                                         .arg(page.labelOfControl(page.control.hold.control || "").toLowerCase())
            }
            SecondaryButton {
                //% "Resume auto"
                text: qsTrId("home_resume")
                onClicked: Api.call("power", "release", {}, function () {
                    //% "Auto is back in charge"
                    Toasts.ok(qsTrId("home_toast_resume"))
                })
            }
        }
    }

    // ---- fans ----------------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Fans"
        title: qsTrId("home_fans_title")
        icon: "fan"
        rightText: Fmt.rpm(page.tiles.fan.rpm)

        TileRow {
            id: fanRow
            Layout.fillWidth: true
            optimistic: true
            model: page.fanTiles
            current: page.shownFan
            onActivated: (value) => page.pickFan(value)
        }
        HelpText { text: page.hintOfFan(page.shownFan) }

        SelectRow {
            Layout.fillWidth: true
            visible: page.shownFan === "smart"
            //% "Smart curve"
            label: qsTrId("home_curve_label")
            options: page.fanCurves
            current: page.fans.fan_curve === null ? undefined : page.fans.fan_curve
            onActivated: (value) => page.useCurve(value)
            SecondaryButton {
                text: qsTrId("home_edit")
                onClicked: Nav.go("cooling")
            }
        }

        ColumnLayout {
            Layout.fillWidth: true
            spacing: Theme.spacing
            visible: page.shownFan === "manual"

            Text {
                Layout.fillWidth: true
                visible: page.fans.stopped
                //% "The fans are stopped. Pick another mode or move a slider to start them."
                text: qsTrId("home_stopped")
                color: Theme.warn
                font.pixelSize: Theme.fontBody
                wrapMode: Text.Wrap
            }
            ToggleRow {
                Layout.fillWidth: true
                //% "Control both fans together"
                label: qsTrId("home_linked")
                checked: page.linked
                onToggled: (on) => page.toggleLink(on)
            }
            SliderRow {
                id: single
                Layout.fillWidth: true
                visible: page.linked
                //% "Fan speed"
                label: qsTrId("home_fan_speed")
                from: 20; to: 100; stepSize: 5; value: 40; unit: "%"
                onEdited: page.writeDuty()
            }
            SliderRow {
                id: cpuSlider
                Layout.fillWidth: true
                visible: !page.linked
                //% "CPU fan"
                label: qsTrId("home_cpu_fan")
                from: 20; to: 100; stepSize: 5; value: 40; unit: "%"
                onEdited: page.writeDuty()
            }
            SliderRow {
                id: gpuSlider
                Layout.fillWidth: true
                visible: !page.linked
                //% "GPU fan"
                label: qsTrId("home_gpu_fan")
                from: 20; to: 100; stepSize: 5; value: 40; unit: "%"
                onEdited: page.writeDuty()
            }
            SettingRow {
                Layout.fillWidth: true
                //% "Fans off"
                label: qsTrId("home_off_label")
                //% "Asks first; only allowed while cool."
                sub: qsTrId("home_off_sub")
                AppButton {
                    kind: "danger-quiet"
                    //% "Turn fans off…"
                    text: qsTrId("home_off_button")
                    onClicked: page.askFansOff()
                }
            }
        }
    }

    // ---- GPU mode ------------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "GPU mode"
        title: qsTrId("home_gpu_card")
        icon: "gpu"
        //% "GPU: %1 · %2"
        rightText: qsTrId("home_gpu_right").arg(Fmt.temp(page.tiles.gpu.temp)).arg(Fmt.rpm(Live.value("sensors.gpu_fan_rpm")))

        TileRow {
            Layout.fillWidth: true
            model: page.gpuTiles
            current: page.gpuMode !== "" ? page.gpuMode : undefined
            onActivated: (value) => page.askGpu(value)
        }
        CapabilityNote {
            capability: "gpu-mode"
            //% "This laptop has no GPU mode switch."
            text: qsTrId("home_gpu_none")
        }
        HelpText {
            //% "A change needs a reboot to take effect."
            text: qsTrId("home_gpu_reboot")
        }
        Text {
            Layout.fillWidth: true
            visible: page.gpuAtLoad !== "" && page.gpuMode !== page.gpuAtLoad
            //% "Reboot to switch from %1 to %2."
            text: qsTrId("home_gpu_pending").arg(page.labelOfGpu(page.gpuAtLoad)).arg(page.labelOfGpu(page.gpuMode))
            color: Theme.warn
            font.pixelSize: Theme.fontBody
            wrapMode: Text.Wrap
        }
    }
}
