// Settings: this machine and what it supports, the window's own preferences (theme, text
// size, motion, tray), start at login and what the lights do then, the touchpad, desktop alerts,
// updates, reset and the About box. The update runs as a Job (progress shown, the answer kept).
import QtQuick
import QtQuick.Layouts
import org.truesense.app

PageFrame {
    id: page
    title: Nav.currentPage ? Nav.titleOf(Nav.currentPage) : ""

    property var caps: ({ available: false, items: [] })
    property bool capsOpen: false
    property var alerts: null
    property string alertsNote: ""
    property string alertsKind: "ok"
    property var startup: null
    property string startupNote: ""
    property string version: ""
    property var upd: null
    property bool checking: false
    property string updateError: ""
    property string resetNote: ""
    property string resetKind: "ok"
    property bool resetBusy: false
    property var licence: null
    property bool licenceOpen: false
    readonly property var updateJob: Jobs.get("update")
    readonly property var applied: updateJob.done && updateJob.ok ? vm("apply_summary", updateJob.result) : null
    readonly property var licencePaths: [
        "file:///usr/share/doc/truesense/THIRD_PARTY.md",
        "file:///usr/share/licenses/truesense/THIRD_PARTY.md",
        "file:///usr/local/share/doc/truesense/THIRD_PARTY.md",
        "file:///usr/local/share/licenses/truesense/THIRD_PARTY.md",
        "qrc:/assets/THIRD_PARTY.md"
    ]
    readonly property var life: ({ alive: true })
    Component.onDestruction: life.alive = false

    function guarded(fn) {
        var l = life
        return function (x) { if (l.alive) fn(x) }
    }
    function vm(verb, args) {
        try {
            var r = JSON.parse(Bus.vm("settings", verb, JSON.stringify({ value: args })))
            return r.ok ? r.result : null
        } catch (e) { return null }
    }

    // ---- loading ------------------------------------------------------------------------------
    function loadCaps() {
        Api.call("capabilities", "get", {}, guarded(function (r) { page.caps = vm("checklist", r) }),
                 guarded(function () { page.caps = vm("checklist", null) }))
    }
    function loadAlerts() {
        Api.call("alerts", "get", {}, guarded(function (r) {
            var v = vm("alerts_view", r)
            page.alerts = v
            if (v === null) return
            alertsEnabled.checked = v.enabled
            cpuLimit.value = v.cpu_limit_c
            gpuLimit.value = v.gpu_limit_c
            repeat.value = v.repeat_minutes
            debounce.value = v.debounce_samples
            batteryLowOn.checked = v.battery_low_on
            batteryLow.value = v.battery_low_pct
            throttling.checked = v.throttling
            sensorLost.checked = v.sensor_lost
        }), guarded(function (err) {
            page.alerts = null
            page.alertsKind = "error"
            page.alertsNote = err.message
        }))
    }
    function loadStartup() {
        var acc = {}
        var pending = 4
        function done() {
            if (--pending === 0) page.startup = vm("startup_options", acc)
        }
        function ask(key, area, verb, args) {
            Api.call(area, verb, args, guarded(function (r) { acc[key] = r; done() }),
                     guarded(function () { done() }))
        }
        ask("show", "startup", "show", {})
        ask("looks", "look", "list", {})
        ask("keysets", "keyboard", "profile", { action: "list" })
        ask("profiles", "profile", "list", {})
    }
    function loadSimple() {
        Api.call("gui", "autostart", { action: "status" }, guarded(function (r) {
            if (typeof r === "boolean") autostart.checked = r
        }), function () {})
        Api.call("touchpad", "get", {}, guarded(function (r) {
            if (typeof r === "boolean") touchpad.checked = r
        }), function () {})
        Api.call("update", "auto", { action: "status" }, guarded(function (r) {
            if (typeof r === "boolean") autoCheck.checked = r
        }), function () {})
        Api.call("version", "get", {}, guarded(function (r) {
            if (r && r.truesense) page.version = r.truesense
        }), function () {})
    }
    function loadLicence(i) {
        if (i >= licencePaths.length) return
        var x = new XMLHttpRequest()
        x.open("GET", licencePaths[i])
        x.onreadystatechange = function () {
            if (x.readyState !== XMLHttpRequest.DONE || !life.alive) return
            var l = vm("licence", { text: x.responseText })
            if (l !== null && l.available) page.licence = l
            else page.loadLicence(i + 1)
        }
        x.send()
    }
    Component.onCompleted: { loadCaps(); loadAlerts(); loadStartup(); loadSimple(); loadLicence(0) }

    // ---- actions ------------------------------------------------------------------------------
    function sendAlert(field, value) {
        var check = vm("alerts_validate", { field: field, value: value })
        if (check === null || !check.ok) {
            alertsKind = "error"
            //% "That value is not allowed: it must be from %1 to %2."
            alertsNote = check && check.reason === "range" ? qsTrId("settings_alert_range").arg(check.min).arg(check.max)
                //% "That value is not allowed."
                : qsTrId("settings_alert_refused")
            loadAlerts()
            return
        }
        var args = {}
        args[field] = check.value
        Api.call("alerts", "set", args, guarded(function () {
            page.alertsKind = "ok"
            //% "Saved. The service picks the change up within about ten seconds."
            page.alertsNote = qsTrId("settings_alert_saved")
            page.loadAlerts()
        }), guarded(function (err) {
            page.alertsKind = "error"
            page.alertsNote = err.message
            page.loadAlerts()
        }))
    }
    function setStartup(target, value) {
        Api.call("startup", "set", { target: target, value: value }, guarded(function () {
            page.startupNote = ""
            page.loadStartup()
        }), guarded(function (err) {
            Toasts.error(err.message)
            page.loadStartup()
        }))
    }
    function optionText(o) {
        if (o.kind === "stock") {
            switch (o.value) {
            //% "Last used"
            case "last": return qsTrId("settings_opt_last")
            //% "Stock look"
            case "default": return qsTrId("settings_opt_default")
            //% "Leave as the firmware has it"
            case "none": return qsTrId("settings_opt_none")
            //% "Lights off"
            default: return qsTrId("settings_opt_off")
            }
        }
        //% "Look: %1"
        if (o.kind === "look") return qsTrId("settings_opt_look").arg(o.name)
        //% "Key set: %1"
        if (o.kind === "keyset") return qsTrId("settings_opt_keyset").arg(o.name)
        //% "None"
        if (o.kind === "none") return qsTrId("settings_opt_no_profile")
        //% "%1 (no longer exists)"
        if (o.kind === "missing") return qsTrId("settings_opt_missing").arg(o.name)
        return o.name
    }
    function optionsOf(section) {
        if (startup === null) return []
        return startup[section].options.map(function (o) { return { value: o.value, text: optionText(o) } })
    }
    function check(force) {
        checking = true
        updateError = ""
        Api.call("update", "check", { force: force }, guarded(function (r) {
            page.checking = false
            page.upd = vm("update_summary", r)
            if (page.upd && page.upd.current !== "") page.version = page.upd.current
        }), guarded(function (err) {
            page.checking = false
            page.updateError = err.message
        }))
    }

    ConfirmDialog { id: confirm }

    // ---- this machine -------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "This machine"
        title: qsTrId("settings_machine")
        icon: "chip"
        rightText: page.caps.available
            //% "%1 of %2 features supported"
            ? qsTrId("settings_machine_count").arg(page.caps.supported).arg(page.caps.total) : ""

        CapabilityChecklist {
            info: page.caps
            showItems: page.capsOpen
        }
        SecondaryButton {
            visible: page.caps.available
            text: page.capsOpen
                //% "Hide the list"
                ? qsTrId("settings_machine_hide")
                //% "Show what is supported"
                : qsTrId("settings_machine_show")
            onClicked: page.capsOpen = !page.capsOpen
        }
        HelpText {
            //% "A feature that is missing here is greyed out on its page, with the reason."
            text: qsTrId("settings_machine_sub")
        }
    }

    // ---- appearance ---------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Appearance"
        title: qsTrId("settings_appearance")
        icon: "palette"

        TileRow {
            Layout.fillWidth: true
            variant: "outlined"
            current: Ui.theme
            model: [
                //% "Dark"
                { value: "dark", label: qsTrId("settings_theme_dark"), icon: "moon" },
                //% "Light"
                { value: "light", label: qsTrId("settings_theme_light"), icon: "sun" },
                //% "System"
                { value: "system", label: qsTrId("settings_theme_system"), icon: "auto" }
            ]
            onActivated: (value) => Ui.set("theme", value)
        }
        SliderRow {
            Layout.fillWidth: true
            //% "Text size"
            label: qsTrId("settings_text_size")
            from: 80
            to: 160
            stepSize: 10
            unit: "%"
            value: Ui.fontScale * 100
            onReleased: (v) => Ui.set("font_scale", v / 100)
        }
        ToggleRow {
            Layout.fillWidth: true
            //% "Reduce motion"
            label: qsTrId("settings_reduce_motion")
            //% "Turns off easing of gauges and other non-essential animation."
            sub: qsTrId("settings_reduce_motion_sub")
            checked: Ui.reducedMotion
            onToggled: (on) => Ui.set("reduced_motion", on)
        }
        ToggleRow {
            Layout.fillWidth: true
            //% "Keep running in the tray when the window is closed"
            label: qsTrId("settings_tray")
            sub: Ui.trayAvailable ? ""
                //% "This desktop shows no tray, so closing the window ends the app."
                : qsTrId("settings_tray_missing")
            enabled: Ui.trayAvailable
            checked: Ui.minimiseToTray
            onToggled: (on) => Ui.set("minimise_to_tray", on)
        }
        HelpText {
            visible: Ui.problem !== ""
            text: Ui.problem
            color: Theme.warn
        }
    }

    // ---- start up -----------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Start up"
        title: qsTrId("settings_startup")
        icon: "power"

        ToggleRow {
            id: autostart
            Layout.fillWidth: true
            //% "Open this window at login"
            label: qsTrId("settings_autostart")
            //% "Writes ~/.config/autostart/truesense.desktop. Hyprland needs `exec-once = truesense gui` (or uwsm) to run autostart entries."
            sub: qsTrId("settings_autostart_sub")
            onToggled: (on) => Api.call("gui", "autostart", { action: on ? "on" : "off" }, page.guarded(function (r) {
                if (typeof r === "boolean") autostart.checked = r
            }), page.guarded(function (err) {
                autostart.checked = !on
                Toasts.error(err.message)
            }))
        }
        HelpText {
            //% "What the lights and a profile do when you log in (the truesense-startup service). Lighting before login is set on the Lighting page."
            text: qsTrId("settings_login_sub")
        }
        SelectRow {
            Layout.fillWidth: true
            //% "Keyboard lighting"
            label: qsTrId("settings_login_keyboard")
            options: page.optionsOf("keyboard")
            current: page.startup ? page.startup.keyboard.current : undefined
            enabled: page.startup !== null
            onActivated: (value) => page.setStartup("keyboard", value)
        }
        SelectRow {
            Layout.fillWidth: true
            //% "Rear light bar"
            label: qsTrId("settings_login_rear")
            options: page.optionsOf("rear")
            current: page.startup ? page.startup.rear.current : undefined
            enabled: page.startup !== null
            onActivated: (value) => page.setStartup("rear", value)
        }
        SelectRow {
            Layout.fillWidth: true
            //% "Profile applied first"
            label: qsTrId("settings_login_profile")
            options: page.optionsOf("profile")
            current: page.startup ? page.startup.profile.current : undefined
            enabled: page.startup !== null
            onActivated: (value) => page.setStartup("profile", value)
        }
        SettingRow {
            Layout.fillWidth: true
            //% "Try it now"
            label: qsTrId("settings_login_try")
            SecondaryButton {
                //% "Apply the login setup"
                text: qsTrId("settings_login_apply")
                onClicked: Api.call("startup", "apply", {}, page.guarded(function () {
                    //% "Login setup applied."
                    Toasts.ok(qsTrId("settings_login_applied"))
                }))
            }
        }
    }

    // ---- touchpad -----------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Touchpad"
        title: qsTrId("settings_touchpad")
        icon: "sliders"

        ToggleRow {
            id: touchpad
            Layout.fillWidth: true
            //% "Touchpad on"
            label: qsTrId("settings_touchpad_on")
            //% "The same as Fn+F10."
            sub: qsTrId("settings_touchpad_sub")
            enabled: Caps.usable("touchpad")
            onToggled: (on) => Api.call("touchpad", "set", { action: on ? "on" : "off" }, page.guarded(function (r) {
                if (typeof r === "boolean") touchpad.checked = r
            }), page.guarded(function (err) {
                touchpad.checked = !on
                Toasts.error(err.message)
            }))
        }
        CapabilityNote {
            capability: "touchpad"
            //% "This laptop has no touchpad switch."
            text: qsTrId("settings_touchpad_missing")
        }
    }

    // ---- alerts -------------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Alerts"
        title: qsTrId("settings_alerts")
        icon: "warn"

        HelpText {
            //% "The service raises these as desktop notifications, so they work with no window open. They are never urgent-level."
            text: qsTrId("settings_alerts_sub")
        }
        ToggleRow {
            id: alertsEnabled
            Layout.fillWidth: true
            //% "Show alerts"
            label: qsTrId("settings_alerts_enabled")
            onToggled: (on) => page.sendAlert("enabled", on)
        }
        SliderRow {
            id: cpuLimit
            Layout.fillWidth: true
            enabled: alertsEnabled.checked
            //% "CPU temperature limit"
            label: qsTrId("settings_alerts_cpu")
            from: 70
            to: 100
            stepSize: 1
            unit: " °C"
            onReleased: (v) => page.sendAlert("cpu_limit_c", v)
        }
        SliderRow {
            id: gpuLimit
            Layout.fillWidth: true
            enabled: alertsEnabled.checked
            //% "GPU temperature limit"
            label: qsTrId("settings_alerts_gpu")
            from: 70
            to: 100
            stepSize: 1
            unit: " °C"
            onReleased: (v) => page.sendAlert("gpu_limit_c", v)
        }
        SliderRow {
            id: repeat
            Layout.fillWidth: true
            enabled: alertsEnabled.checked
            //% "Repeat at most every"
            label: qsTrId("settings_alerts_repeat")
            //% "The same alert is not repeated sooner than this."
            sub: qsTrId("settings_alerts_repeat_sub")
            from: 1
            to: 240
            stepSize: 1
            unit: " min"
            onReleased: (v) => page.sendAlert("repeat_minutes", v)
        }
        SliderRow {
            id: debounce
            Layout.fillWidth: true
            enabled: alertsEnabled.checked
            //% "Readings in a row before an alert"
            label: qsTrId("settings_alerts_debounce")
            //% "A reading is taken every second or two; a brief spike does not alert."
            sub: qsTrId("settings_alerts_debounce_sub")
            from: 1
            to: 30
            stepSize: 1
            onReleased: (v) => page.sendAlert("debounce_samples", v)
        }
        ToggleRow {
            id: batteryLowOn
            Layout.fillWidth: true
            enabled: alertsEnabled.checked
            //% "Alert when the battery is low"
            label: qsTrId("settings_alerts_battery")
            onToggled: (on) => page.sendAlert("battery_low_pct", on ? Math.round(batteryLow.value) : 0)
        }
        SliderRow {
            id: batteryLow
            Layout.fillWidth: true
            enabled: alertsEnabled.checked && batteryLowOn.checked
            //% "Battery low at"
            label: qsTrId("settings_alerts_battery_at")
            from: 5
            to: 50
            stepSize: 1
            unit: " %"
            onReleased: (v) => page.sendAlert("battery_low_pct", v)
        }
        ToggleRow {
            id: throttling
            Layout.fillWidth: true
            enabled: alertsEnabled.checked
            //% "Alert when the CPU is slowed down by heat or power"
            label: qsTrId("settings_alerts_throttling")
            onToggled: (on) => page.sendAlert("throttling", on)
        }
        ToggleRow {
            id: sensorLost
            Layout.fillWidth: true
            enabled: alertsEnabled.checked
            //% "Alert when a temperature sensor stops answering"
            label: qsTrId("settings_alerts_sensor")
            onToggled: (on) => page.sendAlert("sensor_lost", on)
        }
        ReadbackNote {
            text: page.alertsNote
            kind: page.alertsKind
        }
    }

    // ---- updates ------------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Updates"
        title: qsTrId("settings_updates")
        icon: "refresh"
        rightText: page.version

        SettingRow {
            Layout.fillWidth: true
            //% "Installed version"
            label: qsTrId("settings_update_installed")
            RowLayout {
                spacing: 8
                AppButton {
                    //% "Check now"
                    text: qsTrId("settings_update_check")
                    busy: page.checking
                    onClicked: page.check(true)
                }
                SecondaryButton {
                    visible: page.upd !== null && page.upd.url !== null
                    //% "What's new"
                    text: qsTrId("settings_update_whatsnew")
                    iconName: "external"
                    onClicked: Qt.openUrlExternally(page.upd.url)
                }
                PrimaryButton {
                    visible: page.upd !== null && page.upd.state === "available"
                    enabled: !Jobs.busy("update")
                    //% "Update…"
                    text: qsTrId("settings_update_apply")
                    onClicked: confirm.ask({
                        //% "Update TrueSense"
                        title: qsTrId("settings_update_title"),
                        //% "Update TrueSense from %1 to %2? This downloads the release from GitHub, checks its checksum and runs its installer with administrator rights: you will be asked for your password. The service restarts for a moment, and the fans go back to the firmware meanwhile."
                        text: qsTrId("settings_update_text").arg(page.upd.current).arg(page.upd.latest),
                        //% "Update"
                        confirmText: qsTrId("settings_update_confirm")
                    }, function () { Jobs.start("update", "update", "apply", { yes: true }) })
                }
            }
        }
        HelpText {
            visible: text !== ""
            text: {
                if (page.updateError !== "") return page.updateError
                if (page.upd === null)
                    //% "Not checked yet."
                    return qsTrId("settings_update_unchecked")
                var line
                if (page.upd.state === "available")
                    //% "TrueSense %1 is available (you have %2)."
                    line = qsTrId("settings_update_available").arg(page.upd.latest).arg(page.upd.current)
                else if (page.upd.state === "current")
                    //% "You have the newest version (%1)."
                    line = qsTrId("settings_update_current").arg(page.upd.current)
                else
                    //% "The newest version could not be found."
                    line = qsTrId("settings_update_unknown")
                return page.upd.error ? line + "  " + page.upd.error : line
            }
            color: page.updateError !== "" ? Theme.bad : Theme.muted
        }
        LongRunning {
            job: page.updateJob
            //% "Downloading and installing (enter your password when asked)"
            text: qsTrId("settings_update_running")
            onStopWaiting: Jobs.cancelWaiting("update")
        }
        ReadbackNote {
            visible: page.updateJob.done
            kind: page.updateJob.ok && page.applied && page.applied.ok ? "ok" : "error"
            text: {
                if (!page.updateJob.done) return ""
                // A refusal (a failed checksum, no published checksums, already the newest) is the
                // program's own sentence, written for a person: shown as it is.
                if (!page.updateJob.ok) return page.updateJob.error ? page.updateJob.error.message : ""
                if (page.applied === null) return ""
                if (page.applied.ok)
                    //% "Updated to %1. Close this window and open it again to use it."
                    return qsTrId("settings_update_done").arg(page.applied.version)
                //% "The update did not finish: the installer exited with code %1 (see %2)."
                return qsTrId("settings_update_failed").arg(page.applied.returncode).arg(page.applied.log)
            }
        }
        ToggleRow {
            id: autoCheck
            Layout.fillWidth: true
            //% "Check for updates automatically"
            label: qsTrId("settings_update_auto")
            //% "Once a day it asks GitHub for the newest release number (nothing else is sent) and shows a notification; nothing is installed without you pressing Update."
            sub: qsTrId("settings_update_auto_sub")
            onToggled: (on) => Api.call("update", "auto", { action: on ? "on" : "off" }, page.guarded(function (r) {
                if (typeof r === "boolean") autoCheck.checked = r
            }), page.guarded(function (err) {
                autoCheck.checked = !on
                Toasts.error(err.message)
            }))
        }
    }

    // ---- reset --------------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Reset"
        title: qsTrId("settings_reset")
        icon: "undo"

        SettingRow {
            Layout.fillWidth: true
            //% "Everything back to defaults"
            label: qsTrId("settings_reset_label")
            //% "Back to how a fresh install behaves: the firmware's own fan curve and mode control, power limits left to the firmware, lighting stock, undervolt off, the PREDATOR key at its default. Your saved profiles are kept."
            sub: qsTrId("settings_reset_sub")
            AppButton {
                kind: "danger-quiet"
                //% "Reset all…"
                text: qsTrId("settings_reset_button")
                busy: page.resetBusy
                onClicked: confirm.ask({
                    //% "Reset to defaults"
                    title: qsTrId("settings_reset_title"),
                    //% "Reset the fan curve, Smart auto profile, PREDATOR key, power limits, lighting and undervolt to their defaults? Your saved profiles are not deleted."
                    text: qsTrId("settings_reset_text"),
                    //% "Reset all"
                    confirmText: qsTrId("settings_reset_confirm"),
                    danger: true,
                    //% "RESET"
                    typeWord: qsTrId("settings_reset_word")
                }, function () {
                    page.resetBusy = true
                    page.resetNote = ""
                    Api.call("config", "reset-all", {}, page.guarded(function (r) {
                        page.resetBusy = false
                        var s = page.vm("reset_summary", r)
                        if (s !== null && s.all_ok) {
                            page.resetKind = "ok"
                            //% "Everything is back to its defaults."
                            page.resetNote = qsTrId("settings_reset_done")
                        } else {
                            page.resetKind = "error"
                            //% "Reset done, but these steps failed: %1"
                            page.resetNote = qsTrId("settings_reset_partial").arg(s === null ? "?"
                                : s.failed.map(function (f) { return f.step + (f.error !== "" ? " (" + f.error + ")" : "") }).join("; "))
                        }
                        page.loadAlerts()
                        page.loadStartup()
                    }), page.guarded(function (err) {
                        page.resetBusy = false
                        page.resetKind = "error"
                        page.resetNote = err.message
                    }))
                })
            }
        }
        ReadbackNote {
            text: page.resetNote
            kind: page.resetKind
        }
    }

    // ---- about --------------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "About"
        title: qsTrId("settings_about")
        icon: "info"

        RowLayout {
            Layout.fillWidth: true
            spacing: Theme.spacing
            Image {
                source: "qrc:/assets/truesense-256.png"
                sourceSize.width: 96
                sourceSize.height: 96
                Layout.preferredWidth: 48
                Layout.preferredHeight: 48
                fillMode: Image.PreserveAspectFit
                Accessible.ignored: true
            }
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 2
                Text {
                    Layout.fillWidth: true
                    text: page.version !== "" ? "TrueSense " + page.version : "TrueSense"
                    color: Theme.text
                    font.pixelSize: Theme.fontSection
                    font.bold: true
                }
                HelpText {
                    //% "Hardware control for the Acer Predator Helios 16 (PH16-71): fans, power modes, lighting, battery, sound."
                    text: qsTrId("settings_about_text")
                }
            }
        }
        HelpText {
            //% "Parts of this app are adapted from predator-sense by cleyton1986 (GPL-3.0), used with the author's permission."
            text: qsTrId("settings_about_credit")
        }
        HelpText {
            //% "Acer, Predator, PredatorSense, Helios and Nitro are trademarks of Acer Inc., used here only to name the hardware. TrueSense is not made or endorsed by Acer."
            text: qsTrId("settings_about_trademark")
        }
        SecondaryButton {
            visible: page.licence !== null
            text: page.licenceOpen
                //% "Hide licences and credits"
                ? qsTrId("settings_licence_hide")
                //% "Show licences and credits"
                : qsTrId("settings_licence_show")
            onClicked: page.licenceOpen = !page.licenceOpen
        }
        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: 280
            visible: page.licenceOpen && page.licence !== null
            radius: Theme.radiusSmall
            color: Theme.field
            border.color: Theme.cardLine

            Flickable {
                id: licenceScroll
                anchors.fill: parent
                anchors.margins: 10
                clip: true
                contentWidth: width
                contentHeight: licenceText.height
                boundsBehavior: Flickable.StopAtBounds
                Text {
                    id: licenceText
                    width: licenceScroll.width
                    // Plain text on purpose: the file is shown, never interpreted.
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                    text: page.licence ? page.licence.text : ""
                    color: Theme.stat
                    font.pixelSize: Theme.fontSmall
                }
            }
        }
    }
}
