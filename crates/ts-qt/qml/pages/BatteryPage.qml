// Battery and screen: the charge gauge and the battery's measurements (`battery info`), a chart of
// the charge during this session, the charging switches (80 % limit, charge to 100 % once, USB
// charging while off, calibration) and the screen and boot switches. Every switch shows what the
// hardware read back after the change, or the refusal text, under itself.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

PageFrame {
    id: page
    title: Nav.currentPage ? Nav.titleOf(Nav.currentPage) : ""

    /// The `battery` view-model's summary of `battery info`; null until the first answer.
    property var info: null
    /// The view-model's state of "charge to 100 % once"; null until known.
    property var full: null
    property string fullNote: ""
    property string fullNoteKind: "ok"
    property string usbNote: ""
    property string usbNoteKind: "ok"
    property int usbLevel: -1
    property bool usbBusy: false
    property int _ticks: 0
    readonly property string healthClass: info && info.present && info.health ? info.health["class"] : "unknown"
    // A callback that arrives after the page was left must not touch it.
    readonly property var life: ({ alive: true })
    Component.onDestruction: life.alive = false

    function guarded(fn) {
        var l = life
        return function (x) { if (l.alive) fn(x) }
    }

    function vm(verb, args) {
        try {
            var r = JSON.parse(Bus.vm("battery", verb, JSON.stringify({ value: args })))
            return r.ok ? r.result : null
        } catch (e) { return null }
    }

    function statusText(code) {
        switch (code) {
        //% "Charging"
        case "charging": return qsTrId("battery_status_charging")
        //% "Discharging"
        case "discharging": return qsTrId("battery_status_discharging")
        //% "Full"
        case "full": return qsTrId("battery_status_full")
        //% "Not charging"
        case "not-charging": return qsTrId("battery_status_not_charging")
        default:
            //% "Status unknown"
            return qsTrId("battery_status_unknown")
        }
    }

    function rowLabel(id) {
        switch (id) {
        //% "Voltage"
        case "voltage": return qsTrId("battery_row_voltage")
        //% "Current"
        case "current": return qsTrId("battery_row_current")
        //% "Power"
        case "power": return qsTrId("battery_row_power")
        //% "Charge cycles"
        case "cycles": return qsTrId("battery_row_cycles")
        //% "Health"
        case "health": return qsTrId("battery_row_health")
        //% "Capacity (full / design)"
        case "capacity": return qsTrId("battery_row_capacity")
        //% "Temperature"
        case "temperature": return qsTrId("battery_row_temperature")
        default: return id
        }
    }

    function healthText(cls) {
        switch (cls) {
        //% "Good"
        case "good": return qsTrId("battery_health_good")
        //% "Fair"
        case "fair": return qsTrId("battery_health_fair")
        //% "Worn"
        case "poor": return qsTrId("battery_health_poor")
        default: return ""
        }
    }

    function onOff(v) {
        //% "on"
        return v ? qsTrId("battery_on")
                 //% "off"
                 : qsTrId("battery_off")
    }

    // ---- loading ------------------------------------------------------------------------------
    function loadInfo() {
        Api.call("battery", "info", {}, guarded(function (r) { page.info = vm("summary", r) }),
                 guarded(function () { page.info = null }))
    }
    function loadFull() {
        Api.call("battery", "full", { action: "get" }, guarded(function (r) { page.full = vm("full_state", r) }),
                 guarded(function () { page.full = vm("full_state", null) }))
    }
    function loadSwitches() {
        Api.call("battery", "status", {}, guarded(function (r) {
            if (typeof r.limit_80 === "boolean") limit.checked = r.limit_80
            if (typeof r.calibration === "boolean") calibration.checked = r.calibration
            if (typeof r.usb_charge_stop === "number") page.usbLevel = r.usb_charge_stop
        }), function () {})
        Api.call("status", "get", {}, guarded(function (r) {
            if (typeof r.overdrive === "boolean") overdrive.checked = r.overdrive
            if (typeof r.boot_sound === "boolean") bootSound.checked = r.boot_sound
            if (typeof r.kb_backlight_timeout === "boolean") kbTimeout.checked = r.kb_backlight_timeout
        }), function () {})
    }
    Component.onCompleted: { loadInfo(); loadFull(); loadSwitches() }

    // The charge changes slowly: a reading every fifth live tick (the stream ticks only while the
    // window is visible, so a hidden window asks for nothing).
    Connections {
        target: Live
        function onSerialChanged() {
            page._ticks++
            if (page._ticks % 5 === 0) { page.loadInfo(); page.loadFull() }
        }
    }

    // ---- sending a switch ---------------------------------------------------------------------
    function sendBool(ctl, area, verb, on) {
        ctl.busy = true
        Api.call(area, verb, { action: on ? "on" : "off" }, guarded(function (r) {
            ctl.busy = false
            var rb = vm("readback", { requested: on, result: r })
            var state = rb ? rb.state : "unknown"
            ctl.checked = typeof r === "boolean" ? r : on
            if (state === "confirmed") {
                //% "Read back from the machine: %1"
                ctl.note = qsTrId("battery_readback").arg(onOff(r))
                ctl.noteKind = "ok"
            } else if (state === "differs") {
                //% "The machine answered %1, so the change was not taken."
                ctl.note = qsTrId("battery_readback_differs").arg(onOff(r))
                ctl.noteKind = "warn"
            } else {
                //% "Sent, but the machine gave no answer to read back."
                ctl.note = qsTrId("battery_readback_none")
                ctl.noteKind = "warn"
            }
        }), guarded(function (err) {
            ctl.busy = false
            ctl.checked = !on
            ctl.note = err.message
            ctl.noteKind = "error"
        }))
    }

    ConfirmDialog { id: confirm }

    // ---- battery ------------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Battery"
        title: qsTrId("battery_title")
        icon: "battery"
        rightText: page.info && page.info.present
            ? page.statusText(page.info.status) + (page.info.ac === "online"
                //% " · on power"
                ? qsTrId("battery_on_power")
                : (page.info.ac === "offline"
                   //% " · on battery"
                   ? qsTrId("battery_on_battery") : ""))
            : ""

        HelpText {
            visible: page.info !== null && !page.info.present
            //% "No battery was found in this laptop."
            text: qsTrId("battery_none")
        }
        HelpText {
            visible: page.info === null
            //% "The battery could not be read."
            text: qsTrId("battery_unreadable")
        }

        GridLayout {
            Layout.fillWidth: true
            visible: page.info !== null && page.info.present === true
            columns: page.width >= 600 ? 2 : 1
            columnSpacing: Theme.pad * 2
            rowSpacing: Theme.spacing

            Gauge {
                Layout.alignment: Qt.AlignHCenter | Qt.AlignTop
                Layout.preferredWidth: 150
                Layout.preferredHeight: 150
                value: page.info && page.info.gauge ? page.info.gauge.value : 0
                from: 0
                to: 100
                text: page.info && page.info.gauge ? page.info.gauge.text : ""
                caption: page.info && page.info.present ? page.statusText(page.info.status) : ""
            }

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 6

                Repeater {
                    model: page.info && page.info.rows ? page.info.rows : []
                    delegate: RowLayout {
                        id: infoRow
                        required property var modelData
                        Layout.fillWidth: true
                        spacing: Theme.spacing
                        Text {
                            Layout.fillWidth: true
                            text: page.rowLabel(infoRow.modelData.id)
                            color: Theme.muted
                            font.pixelSize: Theme.fontBody
                            elide: Text.ElideRight
                        }
                        Text {
                            text: infoRow.modelData.text
                            color: Theme.text
                            font.pixelSize: Theme.fontBody
                            horizontalAlignment: Text.AlignRight
                        }
                        Badge {
                            visible: infoRow.modelData.id === "health" && page.healthClass !== "unknown"
                            text: page.healthText(page.healthClass)
                            kind: page.healthClass === "good" ? "ok" : (page.healthClass === "fair" ? "warn" : "fail")
                        }
                    }
                }
                ReadbackNote {
                    visible: page.info !== null && page.info.gauge !== undefined && page.info.gauge.low === true
                    kind: "warn"
                    //% "The charge is low: connect the charger."
                    text: qsTrId("battery_low")
                }
                HelpText {
                    visible: page.info !== null && page.info.limit !== undefined && page.info.limit.active === true
                    //% "The firmware stops charging at %1 (the 80 % limit)."
                    text: qsTrId("battery_limit_in_force").arg(page.info ? page.info.limit.text : "")
                }
            }
        }
    }

    // ---- history ------------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Charge during this session"
        title: qsTrId("battery_history")
        icon: "wave"
        rightText: ChargeHistory.series.points >= 2 ? ChargeHistory.series.span_text : ""

        Sparkline {
            Layout.fillWidth: true
            Layout.preferredHeight: 96
            visible: ChargeHistory.series.points >= 2
            values: ChargeHistory.series.values
            minimum: 0
            maximum: 100
            filled: true
        }
        HelpText {
            text: ChargeHistory.series.points >= 2
                //% "Lowest %1, highest %2. Kept in memory only: it starts again when the app does."
                ? qsTrId("battery_history_range").arg(Fmt.percent(ChargeHistory.series.low)).arg(Fmt.percent(ChargeHistory.series.high))
                //% "Collecting readings: one point every 30 seconds while this window is open."
                : qsTrId("battery_history_wait")
        }
    }

    // ---- charging -----------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Charging"
        title: qsTrId("battery_charging")
        icon: "power"

        ReadbackToggle {
            id: limit
            //% "80 % charge limit"
            label: qsTrId("battery_limit")
            //% "Stops charging at 80 % to keep the battery healthy over the years."
            sub: qsTrId("battery_limit_sub")
            enabled: Caps.usable("battery-limit")
            onToggled: (on) => { page.sendBool(limit, "battery", "limit", on); page.loadFull() }
        }
        CapabilityNote {
            capability: "battery-limit"
            //% "This laptop has no 80 % charge limit."
            text: qsTrId("battery_limit_missing")
        }

        SettingRow {
            Layout.fillWidth: true
            //% "Need a full battery"
            label: qsTrId("battery_full_label")
            //% "The firmware offers two levels: 80 % or 100 %. Charging to 100 % once lifts the limit for one charge; the 80 % limit comes back by itself when you unplug, when the battery has been full for 5 minutes, or after 24 hours."
            sub: qsTrId("battery_full_sub")
            AppButton {
                kind: page.full && page.full.can_cancel ? "secondary" : "primary"
                text: page.full && page.full.can_cancel
                    //% "Back to the 80 % limit"
                    ? qsTrId("battery_full_cancel")
                    //% "Charge to 100 % once"
                    : qsTrId("battery_full_start")
                enabled: page.full !== null && (page.full.can_start || page.full.can_cancel)
                onClicked: {
                    var cancelling = page.full.can_cancel
                    Api.call("battery", "full", { action: cancelling ? "cancel" : "start" }, page.guarded(function () {
                        page.fullNoteKind = "ok"
                        page.fullNote = cancelling
                            //% "The 80 % limit is back on."
                            ? qsTrId("battery_full_cancelled")
                            //% "Charging to 100 % this once: the 80 % limit comes back by itself."
                            : qsTrId("battery_full_started")
                        page.loadFull()
                    }), page.guarded(function (err) {
                        page.fullNoteKind = "error"
                        page.fullNote = err.message
                    }))
                }
            }
        }
        HelpText {
            text: {
                if (page.full === null || page.full.state === "unavailable")
                    //% "This needs the TrueSense service."
                    return qsTrId("battery_full_needs_service")
                //% "Battery at %1. "
                var cap = page.full.capacity === null ? "" : qsTrId("battery_full_at").arg(Fmt.percent(page.full.capacity))
                if (page.full.state === "active")
                    //% "Charging to full this time."
                    return cap + qsTrId("battery_full_active")
                if (page.full.state === "plug-in")
                    //% "Plug in the charger to use this."
                    return cap + qsTrId("battery_full_plug_in")
                if (page.full.state === "limit-off")
                    //% "The 80 % limit is off already."
                    return cap + qsTrId("battery_full_limit_off")
                return cap
            }
            visible: text !== ""
        }
        ReadbackNote {
            text: page.fullNote
            kind: page.fullNoteKind
        }

        SettingRow {
            Layout.fillWidth: true
            //% "USB charging while off"
            label: qsTrId("battery_usb")
            //% "Keeps USB ports powered while the laptop is off, down to this battery level."
            sub: qsTrId("battery_usb_sub")
            Segmented {
                enabled: Caps.usable("usb-charge") && !page.usbBusy
                current: page.usbLevel
                model: [
                    //% "Off"
                    { value: 0, label: qsTrId("battery_usb_off") },
                    { value: 10, label: "10 %" },
                    { value: 20, label: "20 %" },
                    { value: 30, label: "30 %" }
                ]
                onActivated: (value) => {
                    page.usbBusy = true
                    Api.call("battery", "usb-charge", { level: value }, page.guarded(function (r) {
                        page.usbBusy = false
                        var rb = page.vm("readback", { requested: value, result: r })
                        if (typeof r === "number") page.usbLevel = r
                        var shown = r === 0 ? page.onOff(false) : r + " %"
                        if (rb && rb.state === "confirmed") {
                            page.usbNoteKind = "ok"
                            page.usbNote = qsTrId("battery_readback").arg(shown)
                        } else if (rb && rb.state === "differs") {
                            page.usbNoteKind = "warn"
                            page.usbNote = qsTrId("battery_readback_differs").arg(shown)
                        } else {
                            page.usbNoteKind = "warn"
                            page.usbNote = qsTrId("battery_readback_none")
                        }
                    }), page.guarded(function (err) {
                        page.usbBusy = false
                        page.usbNoteKind = "error"
                        page.usbNote = err.message
                        page.loadSwitches()
                    }))
                }
            }
        }
        CapabilityNote {
            capability: "usb-charge"
            //% "This laptop cannot stop USB charging while off."
            text: qsTrId("battery_usb_missing")
        }
        ReadbackNote {
            text: page.usbNote
            kind: page.usbNoteKind
        }

        ReadbackToggle {
            id: calibration
            //% "Battery calibration"
            label: qsTrId("battery_calibration")
            //% "Needs AC power and takes hours. Switch it off here to cancel."
            sub: qsTrId("battery_calibration_sub")
            onToggled: (on) => {
                if (!on) { page.sendBool(calibration, "battery", "calibration", false); return }
                // Hold the switch off until the person has agreed.
                calibration.checked = false
                confirm.ask({
                    //% "Battery calibration"
                    title: qsTrId("battery_calibration_title"),
                    //% "Start a calibration? It needs the charger connected and takes several hours: the battery is charged and discharged fully. Switch it off here to cancel."
                    text: qsTrId("battery_calibration_text"),
                    //% "Start"
                    confirmText: qsTrId("battery_calibration_start")
                }, function () {
                    calibration.checked = true
                    page.sendBool(calibration, "battery", "calibration", true)
                })
            }
        }
    }

    // ---- screen and boot ----------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Screen and start-up"
        title: qsTrId("battery_screen")
        icon: "monitor"

        ReadbackToggle {
            id: overdrive
            //% "Screen overdrive"
            label: qsTrId("battery_overdrive")
            //% "Faster pixel response in games; can show slight overshoot on some content."
            sub: qsTrId("battery_overdrive_sub")
            enabled: Caps.usable("overdrive")
            onToggled: (on) => page.sendBool(overdrive, "display", "overdrive", on)
        }
        CapabilityNote {
            capability: "overdrive"
            //% "This screen has no overdrive switch."
            text: qsTrId("battery_overdrive_missing")
        }
        ReadbackToggle {
            id: bootSound
            //% "Start-up animation and sound"
            label: qsTrId("battery_boot_sound")
            enabled: Caps.usable("boot-sound")
            onToggled: (on) => page.sendBool(bootSound, "boot", "sound", on)
        }
        CapabilityNote {
            capability: "boot-sound"
            //% "This laptop has no start-up sound switch."
            text: qsTrId("battery_boot_sound_missing")
        }
        ReadbackToggle {
            id: kbTimeout
            //% "Keyboard light off after 30 seconds"
            label: qsTrId("battery_kb_timeout")
            //% "Turns the keyboard backlight off after 30 seconds without a key press."
            sub: qsTrId("battery_kb_timeout_sub")
            onToggled: (on) => page.sendBool(kbTimeout, "keyboard", "timeout", on)
        }
    }
}
