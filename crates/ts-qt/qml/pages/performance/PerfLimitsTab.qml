// Tab "Power limits": CPU PL1 and PL2 with the risk bands (past the safety line a question, past the
// danger line the word APPLY and `force`), the firmware lock, going back to automatic, the GPU power
// limit as a read-out (this laptop's firmware fixes it) and the takeover of other power managers.
// Bands, wording and the confirmation each needs come from performance.rs, which mirrors the daemon;
// the daemon refuses an unconfirmed dangerous value itself.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

ColumnLayout {
    id: root

    required property var host

    spacing: Theme.spacing + 4

    property var info: ({})
    property bool asked: false
    property string failure: ""
    property bool seeded: false
    property real pl1: 65
    property real pl2: 80
    property var managers: ({ on: false, found: [] })
    property bool managersAsked: false
    property string managersFailure: ""

    readonly property var view: PerfVm.val("limits_view", { info: info }, { readable: false, locked: null, gpu_known: false, gpu: ({}) })
    readonly property var risk: PerfVm.val("risk_pl", { pl1: pl1, pl2: pl2 }, { level: "", why: "", problem: null })
    readonly property bool engineUsable: Caps.usable("power-limits") && host.capUsable("power_limits")
    readonly property bool editable: engineUsable && view.locked !== true && view.readable === true

    function takeInfo(r) {
        info = r
        asked = true
        failure = ""
        var v = PerfVm.val("limits_view", { info: r }, ({}))
        if (!seeded && v.readable) {
            seeded = true
            pl1 = v.user ? v.user.pl1 : v.pl1
            pl2 = v.user ? v.user.pl2 : v.pl2
        }
    }

    function refresh() {
        Api.call("cpu", "info", {}, takeInfo, function (err) {
            asked = true
            failure = err && err.message ? err.message : ""
        })
    }

    function refreshManagers() {
        Api.call("power", "managers", { action: "list" }, function (r) {
            managers = r
            managersAsked = true
            managersFailure = ""
        }, function (err) {
            managersAsked = true
            managersFailure = err && err.message ? err.message : ""
        })
    }

    function problemText(code) {
        switch (code) {
        //% "PL2 (burst) must not be below PL1 (sustained)."
        case "order": return qsTrId("perf_pl_order")
        //% "PL1 and PL2 must be between 25 and 120 W."
        case "range": return qsTrId("perf_pl_range")
        //% "Enter both limits in watts."
        default: return qsTrId("perf_pl_number")
        }
    }

    function applyLimits() {
        var r = risk
        if (r.problem) { Toasts.error(problemText(r.problem)); return }
        var plan = PerfVm.val("confirm_plan", { level: r.level, always: false }, { ask: true, danger: true, force: true, type_word: "APPLY" })
        var a = Math.round(pl1)
        var b = Math.round(pl2)
        var go = function () {
            Api.call("cpu", "limits", { pl1: a, pl2: b, force: plan.force }, function (x) {
                //% "Power limits set: PL1 %1, PL2 %2"
                Toasts.ok(qsTrId("perf_pl_set").arg(Fmt.watts(x.pl1_w)).arg(Fmt.watts(x.pl2_w)))
                refresh()
            })
        }
        if (!plan.ask) { go(); return }
        confirm.ask({
            //% "Apply power limits"
            title: qsTrId("perf_pl_confirm_title"),
            //% "Set PL1 to %1 W and PL2 to %2 W?"
            text: qsTrId("perf_pl_confirm_text").arg(a).arg(b) + "\n\n" + r.why + "\n\n"
                //% "Save your work first."
                + qsTrId("perf_pl_confirm_footer"),
            //% "Apply"
            confirmText: qsTrId("perf_apply"),
            danger: plan.danger,
            typeWord: plan.type_word
        }, go)
    }

    function backToAutomatic() {
        Api.call("cpu", "limits", { auto: true }, function () {
            //% "Power limits are automatic again"
            Toasts.ok(qsTrId("perf_pl_auto_done"))
            seeded = false
            refresh()
        })
    }

    function takeOver(on) {
        if (!on) {
            Api.call("power", "managers", { action: "restore" }, function (r) {
                //% "Restored: %1"
                Toasts.ok(qsTrId("perf_pm_restored").arg((r.restored && r.restored.length) ? r.restored.join(", ")
                    //% "nothing to restore"
                    : qsTrId("perf_pm_nothing_restore")))
                refreshManagers()
            }, function (err) { Toasts.error(err.message); refreshManagers() })
            return
        }
        confirm.ask({
            //% "Let TrueSense manage power?"
            title: qsTrId("perf_pm_confirm_title"),
            //% "The other power managers on this laptop are stopped and disabled, and the CPU is set back to sane defaults (turbo on). Turning this off puts them back exactly as they were."
            text: qsTrId("perf_pm_confirm_text"),
            //% "Stop them"
            confirmText: qsTrId("perf_pm_confirm_ok")
        }, function () {
            Api.call("power", "managers", { action: "stop" }, function (r) {
                var stopped = (r.stopped && r.stopped.length) ? r.stopped.join(", ")
                    //% "nothing was running"
                    : qsTrId("perf_pm_nothing_running")
                //% "Stopped: %1"
                var text = qsTrId("perf_pm_stopped").arg(stopped)
                if (r.failed && r.failed.length)
                    //% "Could not stop: %1"
                    text += ". " + qsTrId("perf_pm_failed").arg(r.failed.join(", "))
                Toasts.ok(text)
                refreshManagers()
            }, function (err) { Toasts.error(err.message); refreshManagers() })
        })
        refreshManagers()
    }

    Component.onCompleted: {
        refresh()
        refreshManagers()
    }

    ConfirmDialog { id: confirm }

    // ---- CPU --------------------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "CPU power limits"
        title: qsTrId("perf_pl_title")
        icon: "chip"
        rightText: root.view.readable
            //% "PL1 %1, PL2 %2"
            ? qsTrId("perf_pl_now").arg(Fmt.watts(root.view.pl1)).arg(Fmt.watts(root.view.pl2))
            : ""

        HelpText {
            //% "How much power the CPU may draw: PL1 is what it can keep up, PL2 is for short bursts. Stock is 80 W. TrueSense keeps your values until you go back to automatic."
            text: qsTrId("perf_pl_help")
        }

        RowLayout {
            Layout.fillWidth: true
            spacing: 10
            Badge {
                text: root.view.locked === true
                    //% "Locked by the firmware until restart"
                    ? qsTrId("perf_pl_locked")
                    : root.view.locked === false
                    //% "Not locked"
                    ? qsTrId("perf_pl_unlocked")
                    //% "Lock state unknown"
                    : qsTrId("perf_pl_lock_unknown")
                kind: root.view.locked === true ? "warn" : "neutral"
            }
            Badge {
                visible: root.view.user !== null && root.view.user !== undefined
                //% "Yours: %1 / %2"
                text: root.view.user ? qsTrId("perf_pl_yours").arg(Fmt.watts(root.view.user.pl1)).arg(Fmt.watts(root.view.user.pl2)) : ""
                kind: "accent"
            }
            Item { Layout.fillWidth: true }
        }
        HelpText {
            visible: root.view.locked === null || root.view.locked === undefined
            text: root.view.lock_note ? root.view.lock_note
                //% "Whether the firmware locked the limits needs the TrueSense service to read."
                : qsTrId("perf_pl_lock_needs_root")
        }
        HelpText {
            visible: root.view.auto !== null && root.view.auto !== undefined && !root.view.user
            //% "Automatic: %1 normally, %2 while gaming."
            text: root.view.auto ? qsTrId("perf_pl_auto_text").arg(Fmt.watts(root.view.auto.normal_w)).arg(Fmt.watts(root.view.auto.gaming_w)) : ""
        }

        SliderRow {
            Layout.fillWidth: true
            //% "PL1, sustained power"
            label: qsTrId("perf_pl1")
            from: 25; to: 120; stepSize: 1; unit: "W"
            value: root.pl1
            enabled: root.editable
            onEdited: (v) => root.pl1 = v
        }
        SliderRow {
            Layout.fillWidth: true
            //% "PL2, burst power"
            label: qsTrId("perf_pl2")
            from: 25; to: 120; stepSize: 1; unit: "W"
            value: root.pl2
            enabled: root.editable
            onEdited: (v) => root.pl2 = v
        }
        RiskLine {
            level: root.editable ? root.risk.level : ""
            why: root.risk.why
        }

        Flow {
            Layout.fillWidth: true
            spacing: 8
            AppButton {
                kind: "primary"
                iconName: "check"
                //% "Apply"
                text: qsTrId("perf_apply")
                enabled: root.editable && !root.risk.problem
                onClicked: root.applyLimits()
            }
            AppButton {
                iconName: "refresh"
                //% "Back to automatic"
                text: qsTrId("perf_pl_auto")
                enabled: root.engineUsable && root.view.locked !== true
                onClicked: root.backToAutomatic()
            }
        }
        HelpText {
            //% "Automatic is TrueSense's own choice of limits (a lower one normally, a higher one while gaming), set in its configuration. It is not the firmware's reset: a restart always returns the firmware's values."
            text: qsTrId("perf_pl_auto_help")
        }

        CapabilityNote {
            capability: "power-limits"
            //% "Power limits need the TrueSense service."
            text: qsTrId("perf_pl_needs_service")
        }
        CapabilityNote {
            stateOverride: root.host.capUsable("power_limits") ? "available" : "unavailable"
            text: root.host.capWhy("power_limits")
        }
        HelpText {
            visible: root.asked && !root.view.readable && root.failure === ""
            //% "The CPU's power limits cannot be read on this machine (the kernel offers no RAPL interface)."
            text: qsTrId("perf_pl_unreadable")
        }
        HelpText {
            visible: root.failure !== "" && Caps.state("power-limits") !== "needs-daemon"
            text: root.failure
            color: Theme.warn
        }
    }

    // ---- GPU (read-only) --------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "GPU power limit"
        title: qsTrId("perf_gpu_title")
        icon: "gpu"
        rightText: root.view.gpu_known && root.view.gpu.current !== null ? Fmt.watts(root.view.gpu.current) : ""

        RowLayout {
            Layout.fillWidth: true
            spacing: 8
            Icon { Layout.alignment: Qt.AlignTop; name: "lock"; size: 16; color: Theme.muted }
            Text {
                Layout.fillWidth: true
                //% "The GPU power limit is fixed by this laptop's firmware: TrueSense shows it but does not change it."
                text: qsTrId("perf_gpu_fixed")
                color: Theme.stat
                font.pixelSize: Theme.fontSmall + 1
                wrapMode: Text.Wrap
            }
        }
        GridLayout {
            Layout.fillWidth: true
            visible: root.view.gpu_known === true
            columns: 4
            columnSpacing: 16
            rowSpacing: 2
            Repeater {
                model: [
                    //% "Current"
                    { key: "current", label: qsTrId("perf_gpu_current") },
                    //% "Default"
                    { key: "default", label: qsTrId("perf_gpu_default") },
                    //% "Lowest"
                    { key: "min", label: qsTrId("perf_gpu_min") },
                    //% "Highest"
                    { key: "max", label: qsTrId("perf_gpu_max") }
                ]
                delegate: ColumnLayout {
                    id: cell
                    required property var modelData
                    Layout.fillWidth: true
                    spacing: 0
                    Text { text: cell.modelData.label; color: Theme.muted; font.pixelSize: Theme.fontSmall }
                    Text {
                        text: Fmt.watts(root.view.gpu[cell.modelData.key])
                        color: Theme.stat
                        font.pixelSize: Theme.fontBody
                    }
                }
            }
        }
        HelpText {
            visible: root.asked && root.view.gpu_known !== true
            //% "The driver is not answering (the GPU may be asleep; TrueSense does not wake it to ask), so no value is shown."
            text: qsTrId("perf_gpu_unknown")
        }
    }

    // ---- other power managers ---------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Other power managers"
        title: qsTrId("perf_pm_title")
        icon: "power"

        HelpText {
            //% "Programs such as power-profiles-daemon, TLP, tuned or auto-cpufreq also set the CPU's speed and fight the power mode TrueSense chooses. Let TrueSense manage power to stop and disable them and put the CPU back to sane defaults; turning it off starts them again exactly as they were."
            text: qsTrId("perf_pm_help")
        }
        ToggleRow {
            Layout.fillWidth: true
            //% "TrueSense manages power"
            label: qsTrId("perf_pm_toggle")
            checked: root.managers.on === true
            enabled: Caps.usable("daemon")
            onToggled: (on) => root.takeOver(on)
        }
        Repeater {
            model: root.managers.found === undefined ? [] : root.managers.found
            delegate: RowLayout {
                id: unitRow
                required property var modelData
                Layout.fillWidth: true
                spacing: 10
                Text {
                    Layout.fillWidth: true
                    text: unitRow.modelData.unit
                    color: Theme.text
                    font.pixelSize: Theme.fontBody
                    elide: Text.ElideRight
                }
                Badge {
                    text: unitRow.modelData.active
                        //% "running"
                        ? qsTrId("perf_pm_running")
                        //% "not running"
                        : qsTrId("perf_pm_not_running")
                    kind: unitRow.modelData.active ? "warn" : "neutral"
                }
                Badge {
                    text: unitRow.modelData.enabled
                        //% "enabled"
                        ? qsTrId("perf_pm_enabled")
                        //% "disabled"
                        : qsTrId("perf_pm_disabled")
                }
            }
        }
        HelpText {
            visible: root.managersAsked && root.managersFailure === "" && (root.managers.found === undefined || root.managers.found.length === 0)
            //% "No other power manager is running or enabled."
            text: qsTrId("perf_pm_none")
        }
        CapabilityNote {
            capability: "daemon"
            //% "Power managers can only be changed while the TrueSense service runs."
            text: qsTrId("perf_pm_needs_service")
        }
    }
}
