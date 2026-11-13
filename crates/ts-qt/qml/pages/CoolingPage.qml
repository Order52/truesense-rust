// Cooling: the two fan dials, the fan mode (Smart, Auto, Max, Manual) with the stop flow, the fan curve
// editor, the curve library and the curve each power mode switches to.
//
// The rules live in Rust (ts-ui-model vm/cooling.rs): the curve validator, the geometry and the drag
// solver of the editor, the dial values, what a manual duty may be. This file draws and sends commands.
// A stop (a duty of 0, or "Stop both fans") always asks first and then sends `confirm: true`; when the
// service refuses (the laptop is warm) its sentence is shown as it is.
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import org.truesense.app

PageFrame {
    id: page

    title: Nav.currentPage ? Nav.titleOf(Nav.currentPage) : ""

    // ---- the view-model (Rust, pure) --------------------------------------------------------------
    QtObject {
        id: vm
        /// Calls a verb; the answer, or {error} (the editor's engine contract).
        function call(verb, args) {
            var r
            try { r = JSON.parse(Bus.vm("cooling", verb, JSON.stringify(args))) } catch (e) { return { error: String(e) } }
            return r.ok ? r.result : { error: r.error }
        }
        /// The answer, or `fallback` when the call failed.
        function get(verb, args, fallback) {
            var r = call(verb, args)
            return (r !== null && r !== undefined && r.error !== undefined) ? fallback : r
        }
    }

    // ---- state -------------------------------------------------------------------------------------
    readonly property var limits: vm.get("limits", {}, null)
    readonly property var dials: vm.get("dials", { frame: Live.frame }, null)
    /// The mode just chosen, until the live frames catch up (two ticks).
    property string forcedMode: ""
    property int forcedTicks: 0
    readonly property string fanMode: forcedMode !== "" ? forcedMode : (dials ? dials.mode : "unknown")
    property bool linked: true
    property bool stopAsking: false

    property var names: []
    property var favourites: []
    property string inUse: ""
    readonly property string inUseName: (dials && dials.curve) ? dials.curve : inUse
    property string curveName: ""
    property bool builtinCurve: false
    property bool nativeCurve: false
    property string description: ""
    property var savedCurve: null
    property var draft: null
    property var undoStack: []
    property string side: "cpu"
    property var bindData: null
    property real lastParamAt: 0
    property string lastParamKey: ""

    readonly property bool locked: builtinCurve || nativeCurve
    readonly property bool changed: draft !== null && savedCurve !== null && vm.get("changed", { a: savedCurve, b: draft }, false) === true
    readonly property var validity: draft !== null ? vm.get("validate", { curve: draft }, { valid: true, error: null }) : null
    readonly property bool valid: validity === null || validity.valid === true
    readonly property var preview: draft === null ? null : vm.get("preview", {
        curve: draft, cpu_temp: Live.value("sensors.cpu_temp"), gpu_temp: Live.value("sensors.gpu_temp"),
        on_battery: Live.value("ac") === false }, null)
    readonly property var library: vm.get("library", { names: names, current: inUseName, favourites: favourites }, { rows: [] })
    readonly property var bindView: vm.get("bindings", { bindings: bindData, curves: names,
                                                         power_mode: Live.value("power_mode") }, { rows: [], options: [] })

    function clone(o) { return JSON.parse(JSON.stringify(o)) }

    function dutyText(p) {
        if (p === null || p === undefined) return Fmt.unknown
        //% "Auto"
        return p.duty === 0 ? qsTrId("cooling_auto") : Fmt.percent(p.duty)
    }

    readonly property var modeNames: ({
        //% "Smart"
        smart: qsTrId("cooling_mode_smart"),
        //% "Auto"
        auto: qsTrId("cooling_mode_auto"),
        //% "Max"
        max: qsTrId("cooling_mode_max"),
        //% "Manual"
        manual: qsTrId("cooling_mode_manual"),
        //% "Unknown"
        unknown: qsTrId("cooling_mode_unknown")
    })
    readonly property var modeHelp: ({
        //% "TrueSense follows the fan curve below by temperature. This is the quietest way to stay cool."
        smart: qsTrId("cooling_help_smart"),
        //% "The firmware runs the fans by its own curve. They stop completely while the laptop is cool."
        auto: qsTrId("cooling_help_auto"),
        //% "Both fans at full speed."
        max: qsTrId("cooling_help_max"),
        //% "You set the speed of each fan yourself."
        manual: qsTrId("cooling_help_manual"),
        unknown: ""
    })
    readonly property var powerModeNames: ({
        //% "Eco"
        eco: qsTrId("cooling_pm_eco"),
        //% "Quiet"
        quiet: qsTrId("cooling_pm_quiet"),
        //% "Balanced"
        balanced: qsTrId("cooling_pm_balanced"),
        //% "Performance"
        performance: qsTrId("cooling_pm_performance"),
        //% "Turbo"
        turbo: qsTrId("cooling_pm_turbo")
    })

    // ---- fan mode and manual duty ----------------------------------------------------------------
    function pickMode(mode) {
        var done = function () { page.forcedMode = mode; page.forcedTicks = 0 }
        // The tile shows the pick at once (optimistic); a refusal puts the real mode back.
        var fail = function (err) { modeRow.settle(); Toasts.error(err.message) }
        if (mode === "smart") Api.call("fan", "smart", { action: "on" }, done, fail)
        else if (mode === "auto") Api.call("fan", "auto", {}, done, fail)
        else if (mode === "max") Api.call("fan", "max", {}, done, fail)
        else {
            // Manual starts from the sliders, never from a stop: a stop has its own question.
            var r = vm.call("manual", { cpu: Math.max(20, cpuS.value), gpu: Math.max(20, gpuS.value), snap: true })
            if (r.error !== undefined) { modeRow.settle(); Toasts.error(r.error); return }
            Api.call("fan", "set", r.args, done, fail)
        }
    }

    function clampDuty(v) { return v === 0 ? 0 : Math.max(20, Math.min(100, v)) }

    function liveDuty(which) {
        var d = dials ? dials[which] : null
        return (d && d.duty !== null && d.duty !== undefined) ? clampDuty(d.duty) : 40
    }

    /// Puts the sliders on the duty registers (silently), unless the person is using them.
    function syncSliders(force) {
        if (!force && (cpuS.dragging || gpuS.dragging || stopAsking)) return
        cpuS.value = liveDuty("cpu")
        gpuS.value = liveDuty("gpu")
    }

    function sendManual() {
        var r = vm.call("manual", { cpu: cpuS.value, gpu: gpuS.value, snap: true })
        if (r.error !== undefined) { Toasts.error(r.error); return }
        if (r.stop) return
        Api.write("fan.manual", "fan", "set", r.args, null, function (err) {
            Toasts.error(err.message)
            page.syncSliders(true)
        })
    }

    function manualEdited(which, v) {
        var self = which === "cpu" ? cpuS : gpuS
        if (v === 0) {
            // A drag across 0 is not a stop; the question comes when the person lets go.
            if (!self.dragging) askStop()
            return
        }
        if (v < 20) { self.value = 20 }
        if (linked) (which === "cpu" ? gpuS : cpuS).value = self.value
        sendManual()
    }

    function askStop() {
        if (stopAsking) return
        stopAsking = true
        var both = cpuS.value === 0 && gpuS.value === 0
        confirm.ask({
            title: both
                //% "Stop both fans?"
                ? qsTrId("cooling_stop_both_title")
                //% "Stop this fan?"
                : qsTrId("cooling_stop_one_title"),
            //% "With a fan stopped the laptop can overheat quickly under any load. This is only for quiet, light use. It is only allowed while the CPU is under 60 °C and the GPU under 55 °C. If the CPU reaches 70 °C or the GPU 65 °C, TrueSense gives the fans back to the firmware by itself. On this laptop the firmware may keep a stopped fan at a minimum speed. TrueSense checks the fan speed afterwards and says what really happened."
            text: qsTrId("cooling_stop_text"),
            //% "Stop"
            confirmText: qsTrId("cooling_stop_confirm"),
            danger: true
        }, function () {
            var r = vm.call("manual", { cpu: cpuS.value, gpu: gpuS.value, snap: true })
            if (r.error !== undefined) { Toasts.error(r.error); page.syncSliders(true); return }
            var args = r.args
            args.confirm = true
            Api.call("fan", "set", args, function (message) { Toasts.show(message, "info") },
                     function (err) { Toasts.error(err.message); page.syncSliders(true) })
        })
    }

    function stopBoth() {
        confirm.ask({
            title: qsTrId("cooling_stop_both_title"),
            text: qsTrId("cooling_stop_text"),
            confirmText: qsTrId("cooling_stop_confirm"),
            danger: true
        }, function () {
            Api.call("fan", "off", { confirm: true }, function (message) {
                page.forcedMode = "manual"
                page.forcedTicks = 0
                Toasts.show(message, "info")
            })
        })
    }

    // ---- curves: library, editor, saving ---------------------------------------------------------
    function refreshFail(err) { if (!Api.needsDaemon(err)) Toasts.error(err.message) }

    function refreshLibrary(then) {
        Api.call("fan", "curve", { action: "list" }, function (r) {
            page.names = r
            if (then) then()
        }, refreshFail)
        Api.call("fan", "curve", { action: "current" }, function (r) { page.inUse = r.name }, refreshFail)
        Api.call("favourite", "list", {}, function (r) { page.favourites = r }, refreshFail)
    }

    function refreshBindings() {
        Api.call("mode", "bindings", {}, function (r) { page.bindData = r }, refreshFail)
    }

    function normalise(c) {
        var o = { cpu: c.cpu, gpu: c.gpu }
        var ps = limits ? limits.params : ({})
        for (var k in ps) o[k] = (c[k] === undefined || c[k] === null) ? ps[k].default : c[k]
        return o
    }

    function loadCurve(name) {
        Api.call("fan", "curve", { action: "show", name: name }, function (c) {
            page.curveName = name
            page.builtinCurve = c.builtin === true
            page.nativeCurve = c.native === true
            page.description = c.description ? c.description : ""
            page.savedCurve = page.normalise(c)
            page.draft = page.clone(page.savedCurve)
            page.undoStack = []
        }, refreshFail)
    }

    function openCurve(name) {
        if (name === curveName) return
        if (!changed) { loadCurve(name); return }
        confirm.ask({
            //% "Discard your changes?"
            title: qsTrId("cooling_discard_title"),
            //% "The curve you are editing has changes that are not saved. Opening another curve drops them."
            text: qsTrId("cooling_discard_text"),
            //% "Discard"
            confirmText: qsTrId("cooling_discard_confirm"),
            danger: true
        }, function () { page.loadCurve(name) })
    }

    function pushUndo() {
        var s = undoStack.concat([JSON.stringify(draft)])
        undoStack = s.length > 60 ? s.slice(s.length - 60) : s
    }

    function undo() {
        if (undoStack.length === 0) return
        draft = JSON.parse(undoStack[undoStack.length - 1])
        undoStack = undoStack.slice(0, undoStack.length - 1)
    }

    function discard() {
        draft = clone(savedCurve)
        undoStack = []
    }

    function setPoints(pts) {
        var d = clone(draft)
        d[side] = pts
        draft = d
    }

    function setParam(key, v) {
        var spec = limits.params[key]
        v = Math.round(v / spec.step) * spec.step
        v = Math.round(v * 100) / 100
        if (draft[key] === v) return
        // one undo step per run of changes to the same slider
        var now = Date.now()
        if (lastParamKey !== key || now - lastParamAt > 1000) pushUndo()
        lastParamKey = key
        lastParamAt = now
        var d = clone(draft)
        d[key] = v
        draft = d
    }

    function payload() {
        var o = { cpu: draft.cpu, gpu: draft.gpu }
        for (var k in limits.params) o[k] = draft[k]
        return o
    }

    function used(r, unsaved) {
        page.inUse = r.name
        page.forcedMode = "smart"
        page.forcedTicks = 0
        if (unsaved)
            //% "Fan curve in use: %1 (with your changes, not saved yet)"
            Toasts.ok(qsTrId("cooling_in_use_unsaved").arg(r.name))
        else
            //% "Fan curve in use: %1"
            Toasts.ok(qsTrId("cooling_in_use").arg(r.name))
        page.refreshLibrary()
    }

    function useCurve() {
        var args = { action: "use", name: curveName }
        if (changed && !locked) args.curve = payload()
        Api.call("fan", "curve", args, function (r) { page.used(r, args.curve !== undefined) })
    }

    function saveAs(name) {
        if (!vm.get("valid_name", { name: name }, false)) {
            //% "Names use letters, digits, . _ - (up to 40)."
            Toasts.error(qsTrId("cooling_name_help"))
            return
        }
        Api.call("fan", "curve", { action: "save", name: name, curve: payload() }, function () {
            //% "Saved fan curve '%1'"
            Toasts.ok(qsTrId("cooling_saved").arg(name))
            nameField.text = ""
            page.refreshLibrary(function () { page.curveName = ""; page.loadCurve(name) })
        })
    }

    function copyCurve() {
        var typed = nameField.text.trim()
        var base = typed !== "" ? typed : curveName + "-copy"
        if (!vm.get("valid_name", { name: base }, false)) { Toasts.error(qsTrId("cooling_name_help")); return }
        var fresh = vm.get("copy_name", { base: base, existing: names }, base)
        Api.call("fan", "curve", { action: "copy", name: curveName, new: fresh }, function (r) {
            var made = typeof r === "string" ? r : fresh
            //% "Copied to '%1'. Drag the points to change it, then Save and Use."
            Toasts.ok(qsTrId("cooling_copied").arg(made))
            nameField.text = ""
            page.refreshLibrary(function () { page.curveName = ""; page.loadCurve(made) })
        })
    }

    function askDelete() {
        var name = curveName
        confirm.ask({
            //% "Delete this fan curve?"
            title: qsTrId("cooling_delete_title"),
            //% "The curve '%1' is deleted for good."
            text: qsTrId("cooling_delete_text").arg(name),
            //% "Delete"
            confirmText: qsTrId("cooling_delete_confirm"),
            danger: true
        }, function () {
            Api.call("fan", "curve", { action: "delete", name: name }, function () {
                //% "Deleted fan curve '%1'"
                Toasts.ok(qsTrId("cooling_deleted").arg(name))
                page.refreshLibrary(function () {
                    page.curveName = ""
                    page.loadCurve(page.names.indexOf(page.inUse) >= 0 ? page.inUse : "truesense")
                })
                page.refreshBindings()
            })
        })
    }

    function toggleStar(name) {
        Api.call("favourite", "toggle", { kind: "fan_curve", name: name }, function () {
            Api.call("favourite", "list", {}, function (r) { page.favourites = r }, page.refreshFail)
        })
    }

    function bindChoice(mode, curve) {
        var r = vm.call("bind_request", { mode: mode, curve: curve })
        if (r.error !== undefined) { Toasts.error(r.error); page.refreshBindings(); return }
        Api.call("mode", r.verb, r.args, function (res) { page.bindData = res },
                 function (err) { Toasts.error(err.message); page.refreshBindings() })
    }

    Connections {
        target: Live
        function onSerialChanged() {
            if (page.forcedMode !== "" && ++page.forcedTicks >= 2) page.forcedMode = ""
            if (page.fanMode === "manual") page.syncSliders(false)
        }
    }

    Component.onCompleted: {
        syncSliders(true)
        refreshLibrary()
        refreshBindings()
        Api.call("fan", "curve", { action: "current" }, function (r) { page.loadCurve(r.name) },
                 function (err) { page.refreshFail(err); page.loadCurve("truesense") })
    }

    ConfirmDialog {
        id: confirm
        onAccepted: page.stopAsking = false
        onRejected: {
            if (page.stopAsking) page.syncSliders(true)
            page.stopAsking = false
        }
    }

    // ---- the dials -----------------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Fans"
        title: qsTrId("cooling_fans")
        icon: "fan"
        rightText: page.modeNames[page.fanMode]

        GridLayout {
            Layout.fillWidth: true
            columns: 2
            columnSpacing: Theme.spacing
            rowSpacing: Theme.spacing

            Repeater {
                model: ["cpu", "gpu"]
                delegate: ColumnLayout {
                    id: dial
                    required property string modelData
                    readonly property var d: page.dials ? page.dials[modelData] : null
                    Layout.fillWidth: true
                    Layout.preferredWidth: 1
                    spacing: 6

                    Gauge {
                        Layout.alignment: Qt.AlignHCenter
                        Layout.preferredWidth: 150
                        Layout.preferredHeight: 150
                        value: (dial.d && dial.d.rpm !== null && dial.d.rpm !== undefined) ? dial.d.rpm : 0
                        from: 0
                        to: page.dials ? page.dials.rpm_scale : 6500
                        text: Fmt.rpm(dial.d ? dial.d.rpm : undefined)
                        caption: dial.modelData === "cpu"
                            //% "CPU fan"
                            ? qsTrId("cooling_cpu_fan")
                            //% "GPU fan"
                            : qsTrId("cooling_gpu_fan")
                    }
                    Sparkline {
                        Layout.fillWidth: true
                        Layout.preferredHeight: 26
                        minimum: 0
                        values: Live.history("sensors." + dial.modelData + "_fan_rpm")
                        minRepaintMs: 3000
                    }
                    Text {
                        Layout.fillWidth: true
                        horizontalAlignment: Text.AlignHCenter
                        //% "Duty register %1"
                        text: qsTrId("cooling_duty").arg(Fmt.percent(dial.d ? dial.d.duty : undefined))
                        color: Theme.stat
                        font.pixelSize: Theme.fontSmall + 1
                    }
                    Text {
                        Layout.fillWidth: true
                        horizontalAlignment: Text.AlignHCenter
                        text: dial.modelData === "cpu"
                            //% "CPU %1"
                            ? qsTrId("cooling_cpu_temp").arg(Fmt.temp(dial.d ? dial.d.temp : undefined))
                            //% "GPU %1"
                            : qsTrId("cooling_gpu_temp").arg(Fmt.temp(dial.d ? dial.d.temp : undefined))
                        color: Theme.stat
                        font.pixelSize: Theme.fontSmall + 1
                    }
                }
            }
        }

        // 0 RPM while cool in Auto or Smart is the firmware's curve, not a fault: a plain fact, no alarm.
        HelpText {
            visible: page.dials !== null && (page.dials.cpu.idle || page.dials.gpu.idle)
            //% "A fan at 0 RPM while the laptop is cool is normal: in Auto and Smart the firmware keeps the fans off until they are needed."
            text: qsTrId("cooling_idle_note")
        }
        HelpText {
            visible: page.dials !== null && page.dials.stopped
            //% "You stopped the fans. They go back to Auto by themselves if the laptop gets warm."
            text: qsTrId("cooling_stopped_note")
        }
        CapabilityNote {
            capability: "fans"
            //% "This laptop does not report its fans."
            text: qsTrId("cooling_fans_unavailable")
        }
    }

    // ---- the fan mode ---------------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Fan mode"
        title: qsTrId("cooling_mode")
        icon: "sliders"

        TileRow {
            id: modeRow
            Layout.fillWidth: true
            optimistic: true
            enabled: Caps.usable("fans")
            model: [
                { value: "smart", label: page.modeNames.smart, icon: "smart" },
                { value: "auto", label: page.modeNames.auto, icon: "auto" },
                { value: "max", label: page.modeNames.max, icon: "max" },
                { value: "manual", label: page.modeNames.manual, icon: "manual" }
            ]
            current: page.fanMode === "unknown" ? undefined : page.fanMode
            onActivated: (value) => page.pickMode(value)
        }
        HelpText { text: page.modeHelp[page.fanMode] }

        ColumnLayout {
            Layout.fillWidth: true
            visible: page.fanMode === "manual"
            spacing: Theme.spacing

            ToggleRow {
                Layout.fillWidth: true
                //% "Control both fans together"
                label: qsTrId("cooling_linked")
                checked: page.linked
                onToggled: (on) => {
                    page.linked = on
                    if (on) { gpuS.value = cpuS.value; page.sendManual() }
                }
            }
            SliderRow {
                id: cpuS
                Layout.fillWidth: true
                label: qsTrId("cooling_cpu_fan")
                from: 0; to: 100; stepSize: 5; value: 40
                unit: "%"
                //% "Stop"
                valueText: value === 0 ? qsTrId("cooling_stop_word") : ""
                onEdited: (v) => page.manualEdited("cpu", v)
                onReleased: (v) => { if (v === 0) page.askStop() }
            }
            SliderRow {
                id: gpuS
                Layout.fillWidth: true
                label: qsTrId("cooling_gpu_fan")
                from: 0; to: 100; stepSize: 5; value: 40
                unit: "%"
                valueText: value === 0 ? qsTrId("cooling_stop_word") : ""
                onEdited: (v) => page.manualEdited("gpu", v)
                onReleased: (v) => { if (v === 0) page.askStop() }
            }
            HelpText {
                //% "A fan is 0 (stopped, asks first) or 20 to 100 %. Below 20 % the firmware would stall it."
                text: qsTrId("cooling_manual_help")
            }
        }

        RowLayout {
            Layout.fillWidth: true
            AppButton {
                kind: "danger"
                enabled: Caps.usable("fans")
                //% "Stop both fans…"
                text: qsTrId("cooling_stop_both")
                onClicked: page.stopBoth()
            }
            Item { Layout.fillWidth: true }
        }
    }

    // ---- the curve editor -----------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Fan curve"
        title: qsTrId("cooling_curve")
        icon: "wave"
        rightText: page.curveName

        Flow {
            Layout.fillWidth: true
            spacing: 8
            visible: !page.nativeCurve

            Segmented {
                model: [
                    { value: "cpu", label: qsTrId("cooling_cpu_fan") },
                    { value: "gpu", label: qsTrId("cooling_gpu_fan") }
                ]
                current: page.side
                onActivated: (v) => page.side = v
            }
            Rectangle {
                visible: page.changed
                height: Theme.controlHeight
                width: badge.implicitWidth + 24
                radius: Theme.radiusSmall + 1
                color: Theme.accentSoft
                border.color: Theme.accentLine
                Text {
                    id: badge
                    anchors.centerIn: parent
                    //% "Changed"
                    text: qsTrId("cooling_changed")
                    color: Theme.accent
                    font.pixelSize: Theme.fontBody
                    font.bold: true
                }
            }
            AppButton {
                iconName: "undo"
                enabled: page.undoStack.length > 0
                //% "Undo"
                text: qsTrId("cooling_undo")
                onClicked: page.undo()
            }
            AppButton {
                enabled: page.changed
                //% "Discard"
                text: qsTrId("cooling_discard")
                onClicked: page.discard()
            }
            AppButton {
                iconName: "plus"
                enabled: !page.locked
                //% "Add point"
                text: qsTrId("cooling_add_point")
                onClicked: editor.addMiddle()
            }
        }

        HelpText {
            visible: page.nativeCurve
            //% "Native: the firmware controls the fans exactly like PredatorSense on Windows. There is no curve to edit."
            text: qsTrId("cooling_native_note")
        }
        HelpText {
            visible: page.builtinCurve && !page.nativeCurve
            //% "Built-in curves cannot be changed. Press Copy to make your own version."
            text: qsTrId("cooling_builtin_note")
        }
        HelpText {
            visible: page.description !== "" && !page.builtinCurve
            text: page.description
        }

        CurveEditor {
            id: editor
            Layout.fillWidth: true
            visible: !page.nativeCurve && page.draft !== null
            engine: vm
            side: page.side
            points: page.draft ? page.draft[page.side] : []
            otherPoints: page.draft ? page.draft[page.side === "cpu" ? "gpu" : "cpu"] : []
            locked: page.locked
            nowTemp: Live.value("sensors." + page.side + "_temp") === undefined ? NaN : Live.value("sensors." + page.side + "_temp")
            lineColor: page.side === "cpu" ? Theme.accent : Theme.ok
            onEditStarted: page.pushUndo()
            onEdited: (pts) => page.setPoints(pts)
            onRefused: (why) => Toasts.show(why, "info")
        }

        ColumnLayout {
            Layout.fillWidth: true
            visible: !page.nativeCurve && page.preview !== null
            spacing: 2
            Text {
                Layout.fillWidth: true
                //% "At CPU %1 this curve asks for %2."
                text: qsTrId("cooling_preview_cpu").arg(Fmt.temp(page.preview && page.preview.cpu ? page.preview.cpu.temp : undefined))
                    .arg(page.dutyText(page.preview ? page.preview.cpu : null))
                color: Theme.stat
                font.pixelSize: Theme.fontSmall + 1
                wrapMode: Text.Wrap
            }
            Text {
                Layout.fillWidth: true
                //% "At GPU %1 it asks for %2."
                text: qsTrId("cooling_preview_gpu").arg(Fmt.temp(page.preview && page.preview.gpu ? page.preview.gpu.temp : undefined))
                    .arg(page.dutyText(page.preview ? page.preview.gpu : null))
                color: Theme.stat
                font.pixelSize: Theme.fontSmall + 1
                wrapMode: Text.Wrap
            }
        }
        HelpText {
            visible: !page.nativeCurve
            //% "Drag a point to move it. Double-click empty space to add one; right-click a point or press Delete to remove it. Tab to a point and use the arrow keys for fine steps. The Auto band is where the firmware runs the fan, so it stops when cool. The dashed line is where the side must reach 100 %."
            text: qsTrId("cooling_editor_help")
        }
        Text {
            Layout.fillWidth: true
            visible: !page.valid
            text: page.validity && page.validity.error ? page.validity.error : ""
            color: Theme.bad
            font.pixelSize: Theme.fontSmall + 1
            wrapMode: Text.Wrap
        }

        // The behaviour of the follower; each one explained where the person changes it.
        Repeater {
            model: page.nativeCurve || page.draft === null || page.limits === null ? [] : [
                //% "Early boost when heating"
                { key: "predict", label: qsTrId("cooling_p_predict"),
                  //% "The curve is read this many degrees ahead while the temperature rises, so the fans start before the heat."
                  sub: qsTrId("cooling_p_predict_sub") },
                //% "Wait before slowing down"
                { key: "hysteresis", label: qsTrId("cooling_p_hysteresis"),
                  //% "How far below a point the temperature must fall before the fans may slow down, so they do not hunt."
                  sub: qsTrId("cooling_p_hysteresis_sub") },
                //% "Slow-down speed"
                { key: "down_rate", label: qsTrId("cooling_p_down_rate"),
                  //% "The fastest the fans may slow down, in percent per second. Speeding up is always immediate."
                  sub: qsTrId("cooling_p_down_rate_sub") },
                //% "Fan coupling"
                { key: "couple", label: qsTrId("cooling_p_couple"),
                  //% "How far the quieter fan may trail the louder one. The two fans share heat pipes, so one fan alone cannot cool its own chip."
                  sub: qsTrId("cooling_p_couple_sub") },
                //% "Battery shift"
                { key: "battery_shift", label: qsTrId("cooling_p_battery"),
                  //% "On battery the temperature is read this many degrees lower, so the fans come in later and the laptop is quieter."
                  sub: qsTrId("cooling_p_battery_sub") }
            ]
            delegate: SliderRow {
                id: param
                required property var modelData
                readonly property var spec: page.limits.params[modelData.key]
                Layout.fillWidth: true
                label: modelData.label
                sub: modelData.sub
                from: spec.min
                to: spec.max
                stepSize: spec.step
                unit: spec.unit === "C" ? "°C" : spec.unit
                value: page.draft ? page.draft[modelData.key] : spec.default
                enabled: !page.locked
                onEdited: (v) => page.setParam(modelData.key, v)
            }
        }

        // Use, save, copy, delete.
        TextField {
            id: nameField
            Layout.fillWidth: true
            visible: !page.nativeCurve
            maximumLength: 40
            color: Theme.text
            font.pixelSize: Theme.fontBody
            //% "Name to save or copy as"
            placeholderText: qsTrId("cooling_name_placeholder")
            Accessible.name: placeholderText
            background: Rectangle {
                radius: Theme.radiusSmall
                color: Theme.field
                border.color: nameField.activeFocus ? Theme.focusRing : Theme.buttonLine
            }
        }
        Flow {
            Layout.fillWidth: true
            spacing: 8
            PrimaryButton {
                enabled: page.valid && page.draft !== null
                //% "Use this curve"
                text: qsTrId("cooling_use")
                onClicked: page.useCurve()
            }
            AppButton {
                visible: !page.locked
                enabled: page.valid && page.changed
                iconName: "save"
                //% "Save"
                text: qsTrId("cooling_save")
                onClicked: page.saveAs(page.curveName)
            }
            AppButton {
                visible: !page.nativeCurve
                enabled: page.valid && nameField.text.trim() !== ""
                //% "Save as"
                text: qsTrId("cooling_save_as")
                onClicked: page.saveAs(nameField.text.trim())
            }
            AppButton {
                visible: !page.nativeCurve
                iconName: "copy"
                text: page.builtinCurve
                    //% "Copy to edit"
                    ? qsTrId("cooling_copy_edit")
                    //% "Copy"
                    : qsTrId("cooling_copy")
                onClicked: page.copyCurve()
            }
            AppButton {
                visible: !page.locked
                kind: "danger"
                iconName: "trash"
                //% "Delete"
                text: qsTrId("cooling_delete")
                onClicked: page.askDelete()
            }
        }
        HelpText {
            visible: !page.nativeCurve
            //% "Use applies what you see now and switches the fans to Smart. Save keeps it under a name."
            text: qsTrId("cooling_use_help")
        }
    }

    // ---- the library --------------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Curve library"
        title: qsTrId("cooling_library")
        icon: "profiles"

        Repeater {
            model: page.library.rows
            delegate: Rectangle {
                id: row
                required property var modelData
                readonly property bool open: modelData.name === page.curveName
                Layout.fillWidth: true
                implicitHeight: rowLayout.implicitHeight + 12
                radius: Theme.radiusSmall + 1
                color: open ? Theme.selBg : (area.containsMouse ? Theme.tileHover : Theme.tile)
                border.color: open ? Theme.accentLine : "transparent"
                border.width: 1
                Accessible.role: Accessible.Button
                Accessible.name: modelData.name

                // Below the buttons: a click anywhere else on the row opens the curve in the editor.
                MouseArea {
                    id: area
                    anchors.fill: parent
                    hoverEnabled: true
                    onClicked: page.openCurve(row.modelData.name)
                }

                RowLayout {
                    id: rowLayout
                    anchors.fill: parent
                    anchors.margins: 6
                    spacing: 8

                    Button {
                        id: star
                        implicitWidth: 38
                        implicitHeight: 38
                        enabled: !row.modelData.native
                        hoverEnabled: true
                        focusPolicy: Qt.StrongFocus
                        Accessible.name: row.modelData.starred
                            //% "Remove from favourites"
                            ? qsTrId("cooling_unstar")
                            //% "Add to favourites"
                            : qsTrId("cooling_star")
                        onClicked: page.toggleStar(row.modelData.name)
                        contentItem: Item {
                            Icon {
                                anchors.centerIn: parent
                                name: "star"
                                size: 20
                                color: row.modelData.starred ? Theme.warn : Theme.muted
                                opacity: star.enabled ? (row.modelData.starred ? 1 : 0.7) : 0.3
                            }
                        }
                        background: Rectangle {
                            radius: Theme.radiusSmall
                            color: star.hovered ? Theme.buttonHover : "transparent"
                            border.width: star.activeFocus ? 1 : 0
                            border.color: Theme.focusRing
                        }
                    }
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 0
                        Text {
                            Layout.fillWidth: true
                            text: row.modelData.name
                            color: Theme.text
                            font.pixelSize: Theme.fontBody
                            font.bold: row.open
                            elide: Text.ElideRight
                        }
                        Text {
                            Layout.fillWidth: true
                            visible: text !== ""
                            text: {
                                var tags = []
                                //% "Built in"
                                if (row.modelData.builtin) tags.push(qsTrId("cooling_tag_builtin"))
                                //% "In use"
                                if (row.modelData.current) tags.push(qsTrId("cooling_tag_in_use"))
                                return tags.join(" · ")
                            }
                            color: Theme.muted
                            font.pixelSize: Theme.fontSmall
                            elide: Text.ElideRight
                        }
                    }
                    AppButton {
                        visible: !row.modelData.current
                        //% "Use"
                        text: qsTrId("cooling_use_row")
                        onClicked: Api.call("fan", "curve", { action: "use", name: row.modelData.name },
                                            function (r) { page.used(r, false) })
                    }
                }
            }
        }
    }

    // ---- the curve of each power mode ----------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Fans for each power mode"
        title: qsTrId("cooling_bindings")
        icon: "performance"

        Repeater {
            model: page.bindView.rows
            delegate: SelectRow {
                id: bind
                required property var modelData
                Layout.fillWidth: true
                label: page.powerModeNames[modelData.mode] + (modelData.current
                    //% " (now)"
                    ? qsTrId("cooling_now_suffix") : "")
                options: {
                    //% "No curve"
                    var list = [{ value: "", text: qsTrId("cooling_no_curve") }]
                    var opts = page.bindView.options
                    for (var i = 0; i < opts.length; ++i) list.push({ value: opts[i], text: opts[i] })
                    //% "%1 (missing)"
                    if (modelData.missing) list.push({ value: modelData.curve, text: qsTrId("cooling_missing").arg(modelData.curve) })
                    return list
                }
                current: modelData.curve
                onActivated: (value) => page.bindChoice(modelData.mode, value)
            }
        }
        HelpText {
            //% "No curve: this power mode leaves the fans as they are. A power mode change never ends a manual fan setting, and a curve chosen for a power mode applies only while the fans are in Smart mode."
            text: qsTrId("cooling_bindings_help")
        }
    }
}
