// One processor's undervolt: what is applied now, the profile library, two sliders with their safety
// zones, Apply (always asked about; past the danger line the word APPLY is typed and `force` is sent),
// Reset, and the boot profile with its crash guard. `kind` is "cpu" (core and cache in millivolts,
// shown as a reduction) or "gpu" (curve offset and clock cap in MHz). The commands are the ones of
// `cpu undervolt` / `gpu undervolt` (COMMANDS.md); the bands and the confirmation are performance.rs.
// Slider space: a = core reduction | curve offset, b = cache reduction | clock cap slider.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

ColumnLayout {
    id: root

    /// "cpu" or "gpu".
    required property string kind
    /// Whether the controls work (the machine can, the service runs).
    required property bool usable
    /// The machine's reason when it cannot (shown under the controls).
    property string whyNot: ""

    readonly property bool isCpu: kind === "cpu"
    readonly property string area: isCpu ? "cpu" : "gpu"

    spacing: Theme.spacing + 4

    property var names: []
    property string name: "native"
    property bool locked: true
    property var original: ({ a: 0, b: 0 })
    property var edited: ({ a: 0, b: 0 })
    property var flags: ({})
    property int changes: 0
    property var uvState: ({})
    property string note: ""
    property string pendingName: ""

    readonly property var view: PerfVm.val("uv_state", { kind: kind, state: uvState }, { readable: false })
    readonly property bool bootOn: view.boot !== null && view.boot !== undefined
    readonly property var values: isCpu
        ? PerfVm.val("uv_cpu_values", { core_cut: edited.a, cache_cut: edited.b }, { core: 0, cache: 0 })
        : PerfVm.val("uv_gpu_values", { offset: edited.a, cap_slider: edited.b }, { offset: 0, cap: 0 })
    readonly property var risk: isCpu
        ? PerfVm.val("risk_cpu_uv", { core: values.core, cache: values.cache }, { level: "", why: "" })
        : PerfVm.val("risk_gpu_uv", { offset: values.offset, cap: values.cap }, { level: "", why: "" })
    readonly property bool stock: isCpu ? (values.core === 0 && values.cache === 0) : (values.offset === 0 && values.cap === 0)

    function kindLabel() {
        //% "CPU undervolt"
        return isCpu ? qsTrId("perf_uv_cpu") :
            //% "GPU undervolt"
            qsTrId("perf_uv_gpu")
    }

    function describe(v) {
        if (isCpu) {
            //% "core %1 mV, cache %2 mV"
            return qsTrId("perf_uv_describe_cpu").arg(v.core).arg(v.cache)
        }
        //% "offset %1, cap %2"
        return qsTrId("perf_uv_describe_gpu").arg(Fmt.signed(v.offset, "MHz")).arg(v.cap
            ? v.cap + " MHz"
            //% "none"
            : qsTrId("perf_uv_none"))
    }

    function sliderPair(profile) {
        return isCpu ? { a: -profile.core, b: -profile.cache } : { a: profile.offset, b: profile.cap }
    }

    function recompute() {
        var c = PerfVm.val("changed", { original: original, edited: edited }, { flags: {}, count: 0 })
        flags = c.flags
        changes = c.count
    }

    function setValue(key, v) {
        var e = { a: edited.a, b: edited.b }
        e[key] = v
        edited = e
        recompute()
    }

    function undoAll() {
        edited = { a: original.a, b: original.b }
        recompute()
    }

    function failNote(err) {
        note = err && err.message ? err.message : ""
    }

    function refreshState() {
        Api.call(area, "undervolt", { action: "get" }, function (r) {
            uvState = r
            resyncBoot()
        }, function (err) {
            uvState = { error: err && err.message ? err.message : "" }
            resyncBoot()
        })
    }

    function resyncBoot() {
        bootSwitch.checked = Qt.binding(function () { return root.bootOn })
    }

    function loadProfile(n) {
        Api.call(area, "undervolt", { action: "show", name: n }, function (p) {
            name = n
            locked = p.builtin === true
            var pair = sliderPair(p)
            original = pair
            edited = { a: pair.a, b: pair.b }
            note = p.description ? p.description : ""
            recompute()
        }, failNote)
    }

    function loadList(select) {
        Api.call(area, "undervolt", { action: "list" }, function (list) {
            names = list
            loadProfile(select !== undefined && list.indexOf(select) >= 0 ? select : "native")
        }, failNote)
    }

    function pick(n) {
        if (changes === 0) { loadProfile(n); return }
        pendingName = n
        confirm.ask({
            //% "Discard your changes?"
            title: qsTrId("perf_smart_discard_title"),
            //% "You changed this profile and did not save it. Opening another profile throws the changes away."
            text: qsTrId("perf_smart_discard_text"),
            //% "Discard"
            confirmText: qsTrId("perf_discard"),
            danger: true
        }, function () { loadProfile(pendingName) })
    }

    /// Asks as the risk level demands, then calls `go(force)`.
    function askThen(title, question, footer, go) {
        var r = risk
        var plan = PerfVm.val("confirm_plan", { level: r.level, always: true }, { ask: true, danger: true, force: true, type_word: "APPLY" })
        var text = question + "\n\n" + r.why + "\n\n" + footer
        if (r.level === "danger")
            //% "This is beyond the danger line."
            text += "\n\n" + qsTrId("perf_uv_beyond")
        confirm.ask({
            title: title,
            text: text,
            //% "Apply"
            confirmText: qsTrId("perf_apply"),
            danger: plan.danger,
            typeWord: plan.type_word
        }, function () { go(plan.force) })
    }

    function valueArgs(v) {
        return isCpu ? { core: v.core, cache: v.cache } : { offset: v.offset, cap: v.cap }
    }

    function extend(base, extra) {
        for (var k in extra) base[k] = extra[k]
        return base
    }

    function apply() {
        var v = values
        if (stock) { reset(); return }
        //% "A reboot always returns voltages to stock. Save your work first."
        var footer = qsTrId("perf_uv_footer")
        askThen(
            //% "Apply %1"
            qsTrId("perf_uv_apply_title").arg(kindLabel()),
            //% "Apply '%1': %2?"
            qsTrId("perf_uv_apply_question").arg(name).arg(describe(v)),
            footer,
            function (force) {
                Api.call(area, "undervolt", extend({ action: "apply", name: name, confirm: true, force: force }, valueArgs(v)), function () {
                    //% "%1 applied: %2"
                    Toasts.ok(qsTrId("perf_uv_applied").arg(kindLabel()).arg(describe(v)))
                    refreshState()
                })
            })
    }

    function reset() {
        var finish = function () {
            Api.call(area, "undervolt", { action: "reset" }, function () {
                //% "%1 reset to stock (nothing applied at boot)"
                Toasts.ok(qsTrId("perf_uv_reset_done").arg(kindLabel()))
                loadList("native")
                refreshState()
            })
        }
        Api.call(area, "undervolt", { action: "boot", name: "off" }, finish, finish)
    }

    function setBoot(on) {
        if (!on) {
            Api.call(area, "undervolt", { action: "boot", name: "off" }, function () {
                //% "%1 at boot: off"
                Toasts.ok(qsTrId("perf_uv_boot_off").arg(kindLabel()))
                refreshState()
            }, function (err) { Toasts.error(err.message); resyncBoot() })
            return
        }
        var v = values
        askThen(
            //% "%1 at boot"
            qsTrId("perf_uv_boot_title").arg(kindLabel()),
            //% "Apply '%1' (%2) every time the laptop starts? If it does not stay up for 10 minutes after applying it, the next boot skips it and this switches off."
            qsTrId("perf_uv_boot_question").arg(name).arg(describe(v)),
            qsTrId("perf_uv_footer"),
            function (force) {
                Api.call(area, "undervolt", extend({ action: "boot", name: name, confirm: true, force: force }, valueArgs(v)), function () {
                    //% "%1 at boot: %2"
                    Toasts.ok(qsTrId("perf_uv_boot_on").arg(kindLabel()).arg(name))
                    refreshState()
                }, function (err) { Toasts.error(err.message); resyncBoot() })
            })
        // A cancelled question leaves the switch where it was.
        resyncBoot()
    }

    function saveProfile() {
        var typed = saveAs.text.trim()
        var target = typed !== "" ? typed : name
        if (!PerfVm.val("name_check", { name: target }, { ok: false }).ok) {
            //% "Names use letters, digits, . _ - (up to 40)."
            Toasts.error(qsTrId("perf_name_help"))
            return
        }
        var v = values
        Api.call(area, "undervolt", extend({ action: "save", name: target }, valueArgs(v)), function () {
            //% "Saved %1 profile '%2'"
            Toasts.ok(qsTrId("perf_uv_saved").arg(kindLabel()).arg(target))
            saveAs.text = ""
            loadList(target)
        })
    }

    function copyProfile() {
        var typed = saveAs.text.trim()
        var base = typed !== "" ? typed : name + "-copy"
        if (!PerfVm.val("name_check", { name: base }, { ok: false }).ok) {
            Toasts.error(qsTrId("perf_name_help"))
            return
        }
        var target = PerfVm.val("copy_name", { base: base, names: names }, base)
        Api.call(area, "undervolt", { action: "copy", name: name, new: target }, function (r) {
            //% "Copied to '%1'. Change it, then Save and Apply."
            note = qsTrId("perf_uv_copied").arg(typeof r === "string" ? r : target)
            saveAs.text = ""
            loadList(typeof r === "string" ? r : target)
        })
    }

    function deleteProfile() {
        confirm.ask({
            //% "Delete profile"
            title: qsTrId("perf_uv_delete_title"),
            //% "Delete the profile '%1'? This cannot be undone."
            text: qsTrId("perf_smart_delete_text").arg(name),
            confirmText: qsTrId("perf_delete"),
            danger: true
        }, function () {
            Api.call(area, "undervolt", { action: "delete", name: name }, function () {
                //% "Deleted %1 profile '%2'"
                Toasts.ok(qsTrId("perf_uv_deleted").arg(kindLabel()).arg(name))
                loadList("native")
            })
        })
    }

    Component.onCompleted: {
        loadList("native")
        refreshState()
    }

    ConfirmDialog { id: confirm }

    // ---- what is applied now ----------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Right now"
        title: qsTrId("perf_uv_now")
        icon: root.isCpu ? "chip" : "gpu"
        rightText: !root.view.readable ? ""
            : root.isCpu
            //% "core %1 mV, cache %2 mV"
            ? qsTrId("perf_uv_describe_cpu").arg(root.view.core_mv).arg(root.view.cache_mv)
            //% "offset %1"
            : qsTrId("perf_uv_now_gpu").arg(Fmt.signed(root.view.offset_mhz, "MHz"))

        HelpText {
            visible: !root.isCpu
            //% "GPU undervolt shifts the GPU's voltage and frequency curve up and caps the top clock, so it reaches its speed at a lower voltage. Neither setting survives a reboot."
            text: qsTrId("perf_uv_gpu_intro")
        }
        HelpText {
            visible: !root.view.readable && root.view.error !== null && root.view.error !== undefined
            text: root.view.error ? root.view.error : ""
            color: Theme.warn
        }
        HelpText {
            visible: root.view.readable === true
            text: {
                var t = root.view.applied
                    //% "Applied by TrueSense: %1."
                    ? qsTrId("perf_uv_state_applied").arg(root.view.applied)
                    //% "Stock voltage, nothing applied by TrueSense."
                    : qsTrId("perf_uv_state_stock")
                if (root.view.boot)
                    //% "At boot: %1."
                    t += " " + qsTrId("perf_uv_state_boot").arg(root.view.boot)
                if (root.isCpu && root.view.pending)
                    //% "A boot undervolt is waiting to prove it is stable."
                    t += " " + qsTrId("perf_uv_state_pending")
                if (!root.isCpu && root.view.range_mhz && root.view.range_mhz[0] !== null)
                    //% "Driver range %1 to %2 MHz."
                    t += " " + qsTrId("perf_uv_state_range").arg(root.view.range_mhz[0]).arg(root.view.range_mhz[1])
                return t
            }
        }
        CapabilityNote {
            stateOverride: root.usable ? "available" : "unavailable"
            text: root.whyNot
        }
    }

    // ---- the profile ------------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Profile"
        title: qsTrId("perf_profile_label")
        icon: "sliders"
        rightText: root.locked ? qsTrId("perf_profile_builtin") : qsTrId("perf_profile_yours")

        SelectRow {
            Layout.fillWidth: true
            label: qsTrId("perf_profile_label")
            options: root.names
            current: root.name
            enabled: root.usable
            onActivated: (v) => root.pick(v)
        }
        HelpText {
            visible: root.note !== ""
            text: root.note
        }
        HelpText {
            visible: root.note === "" && root.locked
            //% "Built-in profiles cannot be edited. Press Copy to make your own version."
            text: qsTrId("perf_uv_locked")
        }

        RowLayout {
            Layout.fillWidth: true
            visible: root.changes > 0
            spacing: 10
            Badge {
                text: qsTrId("perf_changed").arg(root.changes)
                kind: "warn"
            }
            Item { Layout.fillWidth: true }
            AppButton {
                iconName: "undo"
                text: qsTrId("perf_undo_all")
                onClicked: root.undoAll()
            }
        }

        // First slider: CPU core / GPU curve offset.
        SliderRow {
            id: sliderA
            Layout.fillWidth: true
            label: root.isCpu
                //% "Core offset"
                ? qsTrId("perf_uv_core")
                //% "Curve offset"
                : qsTrId("perf_uv_offset")
            from: root.isCpu ? 0 : -300
            to: root.isCpu ? 120 : 250
            stepSize: root.isCpu ? 5 : 10
            value: root.edited.a
            valueText: root.isCpu ? (root.edited.a ? "−" + root.edited.a + " mV" : "0 mV") : Fmt.signed(root.edited.a, "MHz")
            enabled: root.usable && !root.locked
            onEdited: (v) => root.setValue("a", v)
            AppButton {
                visible: root.flags.a === true
                iconName: "undo"
                text: qsTrId("perf_undo")
                onClicked: root.setValue("a", root.original.a)
            }
        }
        ZoneBar {
            from: sliderA.from
            to: sliderA.to
            safeUntil: root.isCpu ? 60 : 150
            cautionUntil: root.isCpu ? 90 : 200
            value: root.edited.a
        }

        // Second slider: CPU cache / GPU clock cap.
        SliderRow {
            id: sliderB
            Layout.fillWidth: true
            label: root.isCpu
                //% "Cache offset"
                ? qsTrId("perf_uv_cache")
                //% "Clock cap (below 600 MHz means none)"
                : qsTrId("perf_uv_cap")
            from: 0
            to: root.isCpu ? 120 : 3300
            stepSize: root.isCpu ? 5 : 50
            value: root.edited.b
            valueText: root.isCpu ? (root.edited.b ? "−" + root.edited.b + " mV" : "0 mV")
                : (root.edited.b < 600 ? qsTrId("perf_uv_none") : root.edited.b + " MHz")
            enabled: root.usable && !root.locked
            onEdited: (v) => root.setValue("b", v)
            AppButton {
                visible: root.flags.b === true
                iconName: "undo"
                text: qsTrId("perf_undo")
                onClicked: root.setValue("b", root.original.b)
            }
        }
        ZoneBar {
            visible: root.isCpu
            from: 0
            to: 120
            safeUntil: 50
            cautionUntil: 70
            value: root.edited.b
        }

        RiskLine {
            level: root.risk.level
            why: root.risk.why
        }

        Flow {
            Layout.fillWidth: true
            spacing: 8
            AppButton {
                kind: "primary"
                iconName: "check"
                text: qsTrId("perf_apply")
                enabled: root.usable
                onClicked: root.apply()
            }
            AppButton {
                iconName: "refresh"
                //% "Reset to stock"
                text: qsTrId("perf_uv_reset")
                enabled: root.usable
                onClicked: root.reset()
            }
            AppButton {
                iconName: "copy"
                text: qsTrId("perf_copy")
                enabled: root.usable
                onClicked: root.copyProfile()
            }
            AppButton {
                kind: "danger"
                iconName: "trash"
                text: qsTrId("perf_delete")
                enabled: root.usable && !root.locked
                onClicked: root.deleteProfile()
            }
        }

        TextFieldRow {
            id: saveAs
            Layout.fillWidth: true
            label: qsTrId("perf_save_as")
            sub: qsTrId("perf_name_help")
            placeholder: qsTrId("perf_name_placeholder")
            maxLength: 40
            enabled: root.usable
            onAccepted: root.saveProfile()
            AppButton {
                text: qsTrId("perf_save")
                enabled: root.usable && (!root.locked || saveAs.text.trim() !== "")
                onClicked: root.saveProfile()
            }
        }
    }

    // ---- boot --------------------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "At boot"
        title: qsTrId("perf_uv_boot_card")
        icon: "power"

        ToggleRow {
            id: bootSwitch
            Layout.fillWidth: true
            //% "Apply the values above at boot"
            label: qsTrId("perf_uv_boot_label")
            //% "Off by default. This is guarded: if the laptop does not stay up for 10 minutes after the profile was applied, the next boot skips it and turns this off, so a profile that crashes the machine cannot lock you out."
            sub: qsTrId("perf_uv_boot_sub")
            checked: root.bootOn
            enabled: root.usable
            onToggled: (on) => root.setBoot(on)
        }
    }
}
