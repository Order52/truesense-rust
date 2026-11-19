// The compact panel: the window TrueSense opens by default, after G-Helper's principle. One small
// fixed window, no navigation; every section says the state in force in its title and changes it
// with one click. The deep settings open in the full window ("Fans + Power", "Lighting",
// "Settings"), which is only built when it is first asked for.
//
// Sections hide what this laptop cannot do (capabilities). Reads: one `status get` when the panel
// shows and after each change; the live figures come from the shared live stream, which runs only
// while a window is visible. Nothing here polls.
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import QtQuick.Window
import org.truesense.app

Window {
    id: panel
    objectName: "panel"

    /// Opens the full window on a page ("" = the page it was on).
    signal openPage(string page)
    /// The panel's content (what a screenshot grabs).
    property alias content: shell

    /// Shows the panel in front.
    function present() {
        if (visibility === Window.Minimized) showNormal()
        else show()
        raise()
        requestActivate()
    }
    /// The tray click: shows the panel, or hides it when it is already in front.
    function toggle() {
        if (visible && active) hide()
        else present()
    }

    /// The newest `status get`, `power status` and the keyboard catalogue.
    property var st: ({})
    property var power: ({ supported: ["eco", "quiet", "balanced", "performance", "turbo"] })
    property string version: ""
    property bool autostart: false
    /// The GPU mode chosen in this run: it takes effect after a restart.
    property string gpuPending: ""
    readonly property var effects: LightingVm.call("effects") || []
    readonly property var rearEffects: LightingVm.call("rear_effects") || []

    readonly property int contentWidth: 456
    /// Fixed size (min = max), so a tiling compositor floats it like a tool window.
    readonly property int fixedHeight: Math.ceil(body.implicitHeight + 2 * 18)
    width: contentWidth
    height: fixedHeight
    minimumWidth: contentWidth
    maximumWidth: contentWidth
    minimumHeight: fixedHeight
    maximumHeight: fixedHeight
    title: "TrueSense"
    color: Theme.bg
    // Its own top-level window, not a child of the full window (which is hidden while the panel is
    // the program: a transient child of a hidden window is never shown).
    transientParent: null

    // ---- reading ---------------------------------------------------------------------------------
    function refresh() {
        Api.call("status", "get", {}, function (r) { panel.st = r || {} }, function () {})
        Api.call("power", "status", {}, function (r) { if (r) panel.power = r }, function () {})
    }
    function readOnce() {
        Api.call("version", "get", {}, function (r) { panel.version = r && r.truesense ? r.truesense : "" }, function () {})
        Api.call("gui", "autostart", { action: "status" }, function (r) { panel.autostart = r === true }, function () {})
    }
    onVisibleChanged: if (visible) refresh()
    Component.onCompleted: { readOnce(); if (visible) refresh() }

    /// A command, then a fresh read; its error goes to the message pill.
    function run(area, verb, args, done) {
        Api.call(area, verb, args, function (r) {
            if (done) done(r)
            panel.refresh()
        }, function (e) { Toasts.show(e && e.message ? e.message : "", "error") })
    }

    // ---- what is in force --------------------------------------------------------------------------
    readonly property string mode: Live.value("power_mode") || st.power_mode || ""
    readonly property string autoMode: Live.value("daemon.settings.auto_mode") || "off"
    readonly property int holdLeft: Live.value("daemon.hold_left_s") || 0
    /// The tile in force: a mode held by hand, else Auto while the automatic profile runs.
    readonly property string modeTile: autoMode !== "off" && holdLeft <= 0 ? "auto" : mode
    readonly property var kbLook: st.kb_look || ({})
    readonly property var rear: st.rear || ({})
    readonly property bool rearOn: (rear.brightness || 0) > 0
    readonly property string kbEffect: kbLook.mode === "effect" ? (kbLook.name || "") : ""
    readonly property var kbRule: {
        for (var i = 0; i < effects.length; ++i) if (effects[i].id === kbEffect) return effects[i]
        return null
    }
    readonly property var rearRule: {
        for (var i = 0; i < rearEffects.length; ++i) if (rearEffects[i].id === rear.mode) return rearEffects[i]
        return null
    }

    function temp(path) {
        var v = Live.value(path)
        return v === undefined || v === null ? "–" : Math.round(v) + "°C"
    }
    function rpm(path) {
        var v = Live.value(path)
        //% "fan off"
        return v === undefined || v === null ? "–" : (v > 0 ? v + " RPM" : qsTrId("panel_fan_off"))
    }
    readonly property var tints: ({
        eco: "#3fb950", quiet: "#2fbfaf", balanced: "#3b82f6", performance: "#f59e0b", turbo: "#e5484d", auto: Theme.accent
    })

    // ---- the window --------------------------------------------------------------------------------
    Item {
        id: shell
        anchors.fill: parent
        Rectangle { anchors.fill: parent; color: Theme.bg }

        ColumnLayout {
            id: body
            x: 18
            y: 18
            width: panel.contentWidth - 36
            spacing: 22

            // Performance mode, after G-Helper's first row.
            PanelSection {
                icon: "performance"
                //% "Performance Mode: %1"
                title: qsTrId("panel_mode_title").arg(panel.modeTile === "auto"
                    //% "Auto (%1)"
                    ? qsTrId("panel_mode_auto_now").arg(LightingVm.modeName(panel.mode))
                    : LightingVm.modeName(panel.mode))
                //% "CPU %1 · %2"
                info: qsTrId("panel_cpu_info").arg(panel.temp("sensors.cpu_temp")).arg(panel.rpm("sensors.cpu_fan_rpm"))
                Repeater {
                    model: ["eco", "quiet", "balanced", "performance", "turbo"].filter(function (m) {
                        return !panel.power.supported || panel.power.supported.indexOf(m) >= 0
                    })
                    delegate: PanelTile {
                        required property string modelData
                        Layout.fillWidth: true
                        iconName: modelData === "performance" ? "max" : modelData
                        text: LightingVm.modeName(modelData)
                        tint: panel.tints[modelData]
                        checked: panel.modeTile === modelData
                        onClicked: panel.run("power", "set", { mode: modelData })
                    }
                }
                PanelTile {
                    Layout.fillWidth: true
                    visible: Caps.usable("daemon")
                    iconName: "smart"
                    //% "Auto"
                    text: qsTrId("panel_mode_auto")
                    tint: panel.tints.auto
                    checked: panel.modeTile === "auto"
                    onClicked: panel.run("power", "auto", { action: "smart" }, function () {
                        Api.call("power", "release", {}, function () {}, function () {})
                    })
                }
                PanelTile {
                    Layout.fillWidth: true
                    Layout.columnSpan: 2
                    iconName: "fan"
                    //% "Fans + Power"
                    text: qsTrId("panel_fans_power")
                    onClicked: panel.openPage("cooling")
                }
            }

            // GPU mode (the MUX): a change needs a restart.
            PanelSection {
                visible: Caps.usable("gpu_mode")
                icon: "gpu"
                //% "GPU Mode: %1"
                title: qsTrId("panel_gpu_title").arg(panel.gpuName(panel.gpuPending || panel.st.gpu_mode || ""))
                    //% " (after restart)"
                    + (panel.gpuPending !== "" && panel.gpuPending !== panel.st.gpu_mode ? qsTrId("panel_gpu_restart") : "")
                //% "GPU %1 · %2"
                info: qsTrId("panel_gpu_info").arg(panel.temp("sensors.gpu_temp")).arg(panel.rpm("sensors.gpu_fan_rpm"))
                columns: 3
                Repeater {
                    model: ["optimus", "nvidia", "auto"]
                    delegate: PanelTile {
                        required property string modelData
                        Layout.fillWidth: true
                        iconName: modelData === "optimus" ? "optimus" : modelData === "nvidia" ? "gpu" : "auto"
                        text: panel.gpuName(modelData)
                        tint: modelData === "nvidia" ? panel.tints.turbo : modelData === "optimus" ? panel.tints.eco : Theme.accent
                        checked: (panel.gpuPending || panel.st.gpu_mode) === modelData
                        onClicked: panel.run("gpu", "set", { mode: modelData }, function () {
                            panel.gpuPending = modelData
                            //% "The GPU mode changes after a restart."
                            Toasts.show(qsTrId("panel_gpu_restart_toast"), "info")
                        })
                    }
                }
            }

            // The screen: overdrive.
            PanelSection {
                visible: Caps.usable("overdrive")
                icon: "monitor"
                //% "Laptop Screen: %1"
                title: qsTrId("panel_screen_title").arg(panel.st.overdrive
                    //% "Overdrive"
                    ? qsTrId("panel_screen_od")
                    //% "Normal"
                    : qsTrId("panel_screen_normal"))
                columns: 2
                PanelTile {
                    Layout.fillWidth: true
                    text: qsTrId("panel_screen_normal")
                    checked: panel.st.overdrive === false
                    onClicked: panel.run("display", "overdrive", { action: "off" })
                }
                PanelTile {
                    Layout.fillWidth: true
                    //% "Overdrive (faster pixels)"
                    text: qsTrId("panel_screen_od_long")
                    checked: panel.st.overdrive === true
                    onClicked: panel.run("display", "overdrive", { action: "on" })
                }
            }

            // The keyboard: effect, colour, the rest in Lighting.
            PanelSection {
                visible: Caps.usable("keyboard_backlight")
                icon: "keyboard"
                //% "Laptop Keyboard: %1"
                title: qsTrId("panel_kb_title").arg(panel.kbLook.mode === "effect" ? LightingVm.effectName(panel.kbEffect)
                    //% "Per-key colours"
                    : panel.kbLook.mode === "perkey" ? qsTrId("panel_kb_perkey")
                    //% "Off"
                    : panel.kbLook.mode === "off" ? qsTrId("panel_off") : "–")
                columns: 3
                AppComboBox {
                    Layout.fillWidth: true
                    //% "Keyboard effect"
                    label: qsTrId("panel_kb_effect")
                    //% "Custom"
                    placeholder: qsTrId("panel_custom")
                    options: panel.effects.map(function (e) { return { value: e.id, text: LightingVm.effectName(e.id) } })
                    current: panel.kbEffect
                    onPicked: (id) => {
                        var args = LightingVm.call("pick_effect", { name: id, current: {
                            colour: panel.kbLook.color || "00aec7", speed: panel.kbLook.speed || 3,
                            brightness: panel.kbLook.brightness || panel.st.kb_brightness || 50,
                            direction: panel.kbLook.direction || 1 } })
                        if (args !== undefined) panel.run("keyboard", "effect", args)
                    }
                }
                ColourButton {
                    Layout.fillWidth: true
                    enabled: panel.kbRule !== null && panel.kbRule.colour === true
                    hex: panel.kbLook.color || "000000"
                    onClicked: { picker.target = "kb"; picker.start(hex); picker.open() }
                }
                PanelTile {
                    Layout.fillWidth: true
                    //% "Lighting…"
                    text: qsTrId("panel_lighting")
                    onClicked: panel.openPage("lighting")
                }
            }

            // The rear light bar.
            PanelSection {
                visible: Caps.usable("rear_light")
                icon: "bar"
                //% "Rear Light Bar: %1"
                title: qsTrId("panel_rear_title").arg(panel.rearOn ? LightingVm.effectName(panel.rear.mode || "") : qsTrId("panel_off"))
                columns: 3
                AppComboBox {
                    Layout.fillWidth: true
                    //% "Rear light effect"
                    label: qsTrId("panel_rear_effect")
                    options: panel.rearEffects.map(function (e) { return { value: e.id, text: LightingVm.effectName(e.id) } })
                    current: panel.rear.mode || ""
                    onPicked: (id) => panel.setRear({ effect: id })
                }
                ColourButton {
                    Layout.fillWidth: true
                    enabled: panel.rear.mode !== "neon"
                    hex: panel.rear.color || "000000"
                    onClicked: { picker.target = "rear"; picker.start(hex); picker.open() }
                }
                PanelTile {
                    Layout.fillWidth: true
                    text: panel.rearOn
                        //% "Turn off"
                        ? qsTrId("panel_turn_off")
                        //% "Turn on"
                        : qsTrId("panel_turn_on")
                    onClicked: panel.run("rear", panel.rearOn ? "off" : "on", {})
                }
            }

            // The battery: Acer's limit is 80 % or none.
            PanelSection {
                visible: Caps.usable("battery_limit")
                icon: "battery"
                //% "Battery Charge Limit: %1"
                title: qsTrId("panel_batt_title").arg(panel.st.battery_limit ? "80%" : "100%")
                info: panel.st.battery_pct !== undefined && panel.st.battery_pct !== null
                    ? panel.st.battery_pct + "%" + (panel.st.ac ? " · AC" : "") : ""
                columns: 2
                PanelTile {
                    Layout.fillWidth: true
                    //% "80% (longer battery life)"
                    text: qsTrId("panel_batt_80")
                    tint: panel.tints.eco
                    checked: panel.st.battery_limit === true
                    onClicked: panel.run("battery", "limit", { action: "on" })
                }
                PanelTile {
                    Layout.fillWidth: true
                    //% "100% (full)"
                    text: qsTrId("panel_batt_100")
                    checked: panel.st.battery_limit === false
                    onClicked: panel.run("battery", "limit", { action: "off" })
                }
            }

            // Footer: version, start with the session, the full app, quit.
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 10
                Rectangle { Layout.fillWidth: true; height: 1; color: Theme.sep }
                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8
                    CheckBox {
                        id: startup
                        //% "Run on Startup"
                        text: qsTrId("panel_startup")
                        checked: panel.autostart
                        onToggled: Api.call("gui", "autostart", { action: checked ? "on" : "off" },
                            function (r) { panel.autostart = r === true },
                            function (e) { startup.checked = panel.autostart; Toasts.show(e.message, "error") })
                        indicator: Rectangle {
                            x: 0
                            y: (startup.height - height) / 2
                            width: 18; height: 18; radius: 4
                            color: startup.checked ? Theme.accent : Theme.field
                            border.width: 1
                            border.color: startup.activeFocus ? Theme.focusRing : (startup.checked ? Theme.accent : Theme.buttonLine)
                            Icon {
                                anchors.centerIn: parent
                                visible: startup.checked
                                name: "check"
                                size: 14
                                color: Theme.accentInk
                            }
                        }
                        contentItem: Text {
                            leftPadding: startup.indicator.width + 8
                            text: startup.text
                            color: Theme.text
                            font.pixelSize: Theme.fontBody
                            verticalAlignment: Text.AlignVCenter
                        }
                    }
                    Item { Layout.fillWidth: true }
                    SecondaryButton {
                        //% "Settings"
                        text: qsTrId("panel_settings")
                        iconName: "settings"
                        onClicked: panel.openPage("")
                    }
                    SecondaryButton {
                        //% "Quit"
                        text: qsTrId("panel_quit")
                        onClicked: Qt.quit()
                    }
                }
                Text {
                    visible: panel.version !== ""
                    //% "Version: %1"
                    text: qsTrId("panel_version").arg(panel.version)
                    color: Theme.muted
                    font.pixelSize: Theme.fontSmall
                    font.underline: hover.hovered
                    HoverHandler { id: hover; cursorShape: Qt.PointingHandCursor }
                    TapHandler { onTapped: panel.openPage("settings") }
                }
            }
        }

        ToastHost { anchors.fill: parent }
    }

    function gpuName(m) {
        switch (m) {
        //% "Hybrid"
        case "optimus": return qsTrId("panel_gpu_hybrid")
        //% "dGPU only"
        case "nvidia": return qsTrId("panel_gpu_dgpu")
        //% "Auto"
        case "auto": return qsTrId("panel_gpu_auto")
        default: return m === "" ? "–" : m
        }
    }

    /// The rear bar with one thing changed, the rest as it is (lit at its last brightness).
    function setRear(change) {
        var args = {
            effect: panel.rear.mode || "static", color: panel.rear.color || "00aec7",
            speed: panel.rear.speed || 3, brightness: panel.rearOn ? panel.rear.brightness : 100,
            direction: panel.rear.direction === 0 ? 0 : 1
        }
        for (var k in change) args[k] = change[k]
        panel.run("rear", "set", args)
    }

    Shortcut { sequences: [StandardKey.Quit]; onActivated: Qt.quit() }
    Shortcut { sequences: ["Esc", "Ctrl+W"]; onActivated: panel.close() }

    ColorPickerPopup {
        id: picker
        property string target: "kb"
        parent: shell
        anchors.centerIn: parent
        onPicked: (hex) => {
            if (target === "kb") panel.run("keyboard", "color", { color: hex })
            else panel.setRear({ color: hex })
        }
    }

    // A colour swatch button: "Colour ■".
    component ColourButton: AbstractButton {
        id: cb
        property string hex: "000000"
        implicitHeight: Theme.controlHeight
        hoverEnabled: true
        //% "Colour"
        text: qsTrId("panel_colour")
        Accessible.name: text
        HoverHandler { cursorShape: cb.enabled ? Qt.PointingHandCursor : Qt.ArrowCursor }
        background: Rectangle {
            radius: Theme.radiusSmall + 1
            color: cb.hovered ? Theme.buttonHover : Theme.button
            border.width: 1
            border.color: cb.activeFocus ? Theme.focusRing : Theme.buttonLine
            opacity: cb.enabled ? 1 : 0.45
        }
        contentItem: RowLayout {
            spacing: 8
            Text {
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignHCenter
                text: cb.text
                color: cb.enabled ? Theme.text : Theme.disabledText
                font.pixelSize: Theme.fontBody
            }
            Rectangle {
                Layout.rightMargin: 10
                width: 14; height: 14; radius: 3
                color: "#" + cb.hex
                border.width: 1
                border.color: Theme.swatchLine
                opacity: cb.enabled ? 1 : 0.45
            }
        }
    }
}
