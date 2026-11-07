// Tab "Mode and Smart": the power mode now, who chooses it (Manual, Native, Smart), what Smart auto is
// doing and why, and the Smart auto profile editor with its library (use, copy, save as, delete,
// favourite). A built-in profile is shown read-only; "Copy" makes one that can be edited. Edits stay
// in this tab until Save or Use, with a Changed badge and Undo (the logic is performance.rs).
import QtQuick
import QtQuick.Layouts
import org.truesense.app

ColumnLayout {
    id: root

    required property var host

    spacing: Theme.spacing + 4

    // ---- the profile being edited ---------------------------------------------------------------
    property var names: []
    property string name: "native"
    property bool locked: true
    property var original: ({})
    property var edited: ({})
    property var flags: ({})
    property int changes: 0
    property var starred: ({})
    property string note: ""
    property string pendingName: ""

    readonly property var schema: PerfVm.val("smart_schema", {}, { modes: [], mode_keys: [], turbo_key: "ac_turbo", numbers: [] })
    readonly property var now: PerfVm.val("now_line", { daemon: Live.value("daemon") }, { available: false, control: "off" })
    readonly property bool smartUsable: Caps.usable("smart-auto")

    function recompute() {
        var c = PerfVm.val("changed", { original: original, edited: edited }, { flags: {}, count: 0 })
        flags = c.flags
        changes = c.count
    }

    function setValue(key, value) {
        edited = PerfVm.val("smart_set", { edited: edited, key: key, value: value }, edited)
        recompute()
    }

    function undoOne(key) {
        edited = PerfVm.val("undo", { original: original, edited: edited, key: key }, edited)
        recompute()
    }

    function undoAll() {
        edited = PerfVm.val("undo", { original: original, edited: edited }, edited)
        recompute()
    }

    function failNote(err) {
        note = err && err.message ? err.message : ""
    }

    function loadStars() {
        Api.call("favourite", "list", {}, function (list) {
            var m = {}
            for (var i = 0; i < list.length; ++i)
                if (list[i].kind === "smart_profile") m[list[i].name] = true
            starred = m
        }, function () {})
    }

    function loadProfile(n) {
        Api.call("power", "smart", { action: "show", name: n }, function (p) {
            name = n
            locked = p.builtin === true
            original = p
            edited = p
            note = locked ? "" : (p.description || "")
            recompute()
        }, failNote)
    }

    function loadList(select) {
        Api.call("power", "smart", { action: "list" }, function (list) {
            names = list
            loadProfile(select !== undefined && list.indexOf(select) >= 0 ? select : (list.indexOf(name) >= 0 ? name : "native"))
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

    function useProfile() {
        var args = { action: "use", name: name }
        if (!locked) {
            var bad = PerfVm.problem("smart_payload", { edited: edited })
            if (bad !== "") { Toasts.error(bad); return }
            args.profile = PerfVm.val("smart_payload", { edited: edited }, ({}))
        }
        Api.call("power", "smart", args, function (r) {
            //% "Smart auto profile in use: %1"
            Toasts.ok(qsTrId("perf_smart_in_use").arg(r && r.name ? r.name : name))
        })
    }

    function saveProfile() {
        var typed = saveAs.text.trim()
        var target = typed !== "" ? typed : name
        if (!PerfVm.val("name_check", { name: target }, { ok: false }).ok) {
            //% "Names use letters, digits, . _ - (up to 40)."
            Toasts.error(qsTrId("perf_name_help"))
            return
        }
        var bad = PerfVm.problem("smart_payload", { edited: edited })
        if (bad !== "") { Toasts.error(bad); return }
        Api.call("power", "smart", { action: "save", name: target, profile: PerfVm.val("smart_payload", { edited: edited }, ({})) }, function () {
            //% "Saved Smart auto profile '%1'. Press Use to run it."
            note = qsTrId("perf_smart_saved").arg(target)
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
        Api.call("power", "smart", { action: "copy", name: name, new: target }, function (r) {
            //% "Copied to '%1'. Change what you like, then Save and Use."
            note = qsTrId("perf_smart_copied").arg(typeof r === "string" ? r : target)
            saveAs.text = ""
            loadList(typeof r === "string" ? r : target)
        })
    }

    function deleteProfile() {
        confirm.ask({
            //% "Delete Smart auto profile"
            title: qsTrId("perf_smart_delete_title"),
            //% "Delete the profile '%1'? This cannot be undone."
            text: qsTrId("perf_smart_delete_text").arg(name),
            //% "Delete"
            confirmText: qsTrId("perf_delete"),
            danger: true
        }, function () {
            Api.call("power", "smart", { action: "delete", name: name }, function () {
                //% "Deleted Smart auto profile '%1'"
                Toasts.ok(qsTrId("perf_smart_deleted").arg(name))
                loadList("native")
            })
        })
    }

    function toggleStar() {
        Api.call("favourite", "toggle", { kind: "smart_profile", name: name }, function (r) {
            var m = {}
            for (var k in starred) m[k] = starred[k]
            if (r && r.starred) m[name] = true
            else delete m[name]
            starred = m
        })
    }

    function resetToDefault() {
        Api.call("power", "smart", { action: "use", name: "native" }, function () {
            //% "Smart auto is back on the default profile (native)"
            Toasts.ok(qsTrId("perf_smart_reset"))
            loadList("native")
        })
    }

    Component.onCompleted: {
        loadStars()
        Api.call("power", "smart", { action: "list" }, function (list) {
            names = list
            Api.call("power", "smart", { action: "current" }, function (cur) {
                loadProfile(cur && cur.name && list.indexOf(cur.name) >= 0 ? cur.name : "native")
            }, function () { loadProfile("native") })
        }, failNote)
    }

    ConfirmDialog { id: confirm }

    // ---- the power mode now ---------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Power mode"
        title: qsTrId("perf_mode_title")
        icon: "performance"
        rightText: PerfVm.modeText(Live.value("power_mode"))

        TileRow {
            Layout.fillWidth: true
            model: PerfVm.modeNames.map(function (m) { return { value: m, label: PerfVm.modeText(m), icon: PerfVm.modeIcon(m) } })
            current: Live.value("power_mode")
            onActivated: (mode) => Api.call("power", "set", { mode: mode }, function () {
                //% "Power mode %1, held for 30 minutes"
                Toasts.ok(qsTrId("perf_mode_held").arg(PerfVm.modeText(mode)))
            })
        }
        HelpText {
            //% "A mode you pick stays for 30 minutes, or until something changes (plug in, a game starts or stops, heat). Then the control below takes over again."
            text: qsTrId("perf_mode_help")
        }

        SettingRow {
            Layout.fillWidth: true
            //% "Who chooses the mode"
            label: qsTrId("perf_control_label")
            //% "Manual keeps your pick. Native does what PredatorSense does: Balanced on power, Eco on battery. Smart reacts to load and heat with the profile below."
            sub: qsTrId("perf_control_sub")
            Segmented {
                enabled: root.smartUsable
                model: [
                    //% "Manual"
                    { value: "off", label: qsTrId("perf_control_off") },
                    //% "Native"
                    { value: "native", label: qsTrId("perf_control_native") },
                    //% "Smart"
                    { value: "smart", label: qsTrId("perf_control_smart") }
                ]
                current: root.now.control
                onActivated: (v) => Api.call("power", "auto", { action: v })
            }
        }
        CapabilityNote {
            capability: "smart-auto"
            //% "Smart auto needs the TrueSense service."
            text: qsTrId("perf_smart_needs_service")
        }

        RowLayout {
            Layout.fillWidth: true
            visible: root.now.hold_active === true
            spacing: 10
            Icon { name: "info"; size: 16; color: Theme.muted }
            Text {
                Layout.fillWidth: true
                //% "Your pick stays for %1 more, then automatic control takes over again."
                text: qsTrId("perf_hold_text").arg(Fmt.duration(root.now.hold_left_s))
                color: Theme.stat
                font.pixelSize: Theme.fontSmall + 1
                wrapMode: Text.Wrap
            }
            AppButton {
                //% "Release now"
                text: qsTrId("perf_hold_release")
                onClicked: Api.call("power", "release", {}, function () {})
            }
        }

        RowLayout {
            Layout.fillWidth: true
            visible: root.now.available === true && root.now.control !== "off"
            spacing: 10
            Badge {
                //% "Now: %1"
                text: qsTrId("perf_now_activity").arg(PerfVm.activityText(root.now.activity))
                kind: root.now.activity === "hot" ? "bad" : (root.now.activity === "gaming" || root.now.activity === "heavy") ? "warn" : "neutral"
            }
            Text {
                Layout.fillWidth: true
                visible: root.now.reason !== undefined && root.now.reason !== null
                text: root.now.reason === undefined || root.now.reason === null ? "" : root.now.reason
                color: Theme.stat
                font.pixelSize: Theme.fontSmall + 1
                wrapMode: Text.Wrap
            }
        }
    }

    // ---- the profile library --------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Smart auto profile"
        title: qsTrId("perf_profile_title")
        icon: "smart"
        rightText: root.locked
            //% "built in"
            ? qsTrId("perf_profile_builtin")
            //% "yours"
            : qsTrId("perf_profile_yours")

        HelpText {
            //% "How Smart auto chooses the power mode. Built-in profiles cannot be edited: copy one, then change the copy."
            text: qsTrId("perf_profile_help")
        }

        SelectRow {
            Layout.fillWidth: true
            //% "Profile"
            label: qsTrId("perf_profile_label")
            options: root.names
            current: root.name
            onActivated: (v) => root.pick(v)
            AppButton {
                kind: root.starred[root.name] === true ? "primary" : "secondary"
                iconName: "star"
                //% "Favourite"
                text: qsTrId("perf_favourite")
                Accessible.description: root.starred[root.name] === true
                    //% "This profile is a favourite"
                    ? qsTrId("perf_favourite_on")
                    //% "Add this profile to the favourites"
                    : qsTrId("perf_favourite_off")
                onClicked: root.toggleStar()
            }
        }

        HelpText {
            visible: root.note !== ""
            text: root.note
        }
        HelpText {
            visible: root.note === "" && root.locked
            //% "This profile is built in and read-only. Press Copy to make your own version."
            text: qsTrId("perf_profile_locked")
        }

        Flow {
            Layout.fillWidth: true
            spacing: 8
            AppButton {
                kind: "primary"
                iconName: "check"
                //% "Use"
                text: qsTrId("perf_use")
                enabled: root.smartUsable
                onClicked: root.useProfile()
            }
            AppButton {
                iconName: "copy"
                //% "Copy"
                text: qsTrId("perf_copy")
                onClicked: root.copyProfile()
            }
            AppButton {
                kind: "danger"
                iconName: "trash"
                //% "Delete"
                text: qsTrId("perf_delete")
                enabled: !root.locked
                onClicked: root.deleteProfile()
            }
            AppButton {
                iconName: "refresh"
                //% "Reset to default"
                text: qsTrId("perf_reset_default")
                enabled: root.smartUsable
                onClicked: root.resetToDefault()
            }
        }

        TextFieldRow {
            id: saveAs
            Layout.fillWidth: true
            //% "Save as"
            label: qsTrId("perf_save_as")
            //% "Names use letters, digits, . _ - (up to 40)."
            sub: qsTrId("perf_name_help")
            //% "name"
            placeholder: qsTrId("perf_name_placeholder")
            maxLength: 40
            onAccepted: root.saveProfile()
            AppButton {
                //% "Save"
                text: qsTrId("perf_save")
                enabled: !root.locked || saveAs.text.trim() !== ""
                onClicked: root.saveProfile()
            }
        }
    }

    // ---- the editor -----------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Which mode when"
        title: qsTrId("perf_edit_modes")
        icon: "sliders"

        RowLayout {
            Layout.fillWidth: true
            visible: root.changes > 0
            spacing: 10
            Badge {
                //% "Changed (%1)"
                text: qsTrId("perf_changed").arg(root.changes)
                kind: "warn"
            }
            Item { Layout.fillWidth: true }
            AppButton {
                iconName: "undo"
                //% "Undo all changes"
                text: qsTrId("perf_undo_all")
                onClicked: root.undoAll()
            }
        }

        Repeater {
            model: root.schema.mode_keys
            delegate: SelectRow {
                required property string modelData
                Layout.fillWidth: true
                label: PerfVm.modeSettingLabel(modelData)
                options: PerfVm.modeOptions()
                current: root.edited[modelData]
                enabled: !root.locked
                onActivated: (v) => root.setValue(modelData, v)
                AppButton {
                    visible: root.flags[modelData] === true
                    iconName: "undo"
                    //% "Undo"
                    text: qsTrId("perf_undo")
                    onClicked: root.undoOne(modelData)
                }
            }
        }
    }

    Card {
        Layout.fillWidth: true
        //% "Turbo and timing"
        title: qsTrId("perf_edit_timing")
        icon: "turbo"

        ToggleRow {
            Layout.fillWidth: true
            //% "Allow Turbo on power"
            label: qsTrId("perf_turbo_allow")
            //% "Turbo is the fastest and loudest mode. Smart auto backs off by itself when the laptop gets too hot."
            sub: qsTrId("perf_turbo_allow_sub")
            checked: root.edited[root.schema.turbo_key] === true
            enabled: !root.locked
            onToggled: (on) => root.setValue(root.schema.turbo_key, on)
            AppButton {
                visible: root.flags[root.schema.turbo_key] === true
                iconName: "undo"
                text: qsTrId("perf_undo")
                onClicked: root.undoOne(root.schema.turbo_key)
            }
        }

        Repeater {
            model: root.schema.numbers
            delegate: SliderRow {
                id: numberRow
                required property var modelData
                Layout.fillWidth: true
                label: PerfVm.numberSettingLabel(modelData.key)
                from: modelData.min
                to: modelData.max
                stepSize: 1
                unit: PerfVm.numberUnit(modelData.key)
                value: root.edited[modelData.key] === undefined ? modelData.min : root.edited[modelData.key]
                enabled: !root.locked
                onEdited: (v) => root.setValue(modelData.key, v)
                AppButton {
                    visible: root.flags[numberRow.modelData.key] === true
                    iconName: "undo"
                    text: qsTrId("perf_undo")
                    onClicked: root.undoOne(numberRow.modelData.key)
                }
            }
        }
    }
}
