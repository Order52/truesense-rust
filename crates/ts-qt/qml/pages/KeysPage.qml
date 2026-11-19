// Keys: what the PREDATOR key does. Made to be understood at a glance:
//
//  1. The key itself and whether it works now (the key service), with a Start button if not.
//  2. Presets: big buttons (Open TrueSense, Standard, Gaming, Work, Lighting, and the person's own
//     setups); one click puts one in use.
//  3. What each press does: press, press twice, press three times, hold. Always editable, and every
//     change is applied at once: changing a preset saves the person's version as "My keys" (a new
//     name, never over another setup) and puts it in use. There is no copy step and no Save button.
//  4. Advanced, folded away: the press timing, saving the setup under a name, deleting one.
//
// A setup is a binding profile of the key service (`keys profile`); the key service reloads by
// itself. All action text is composed by ts-ui-model (vm/keys.rs).
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import org.truesense.app

PageFrame {
    id: page
    objectName: "keysPage"

    title: Nav.titleOf(Nav.currentPage)

    readonly property var gestures: ["tap", "double", "triple", "long"]

    // ---- state (a page lives only while it is shown) -----------------------------------------
    readonly property var catalogue: vm("catalogue", {})
    property var profileNames: []
    /// name -> description, for the preset buttons.
    property var descriptions: ({})
    /// The setup shown and edited (the one in use, once loaded).
    property string profileName: ""
    /// The setup in force now (what the key service uses).
    property string inUse: ""
    property bool locked: true
    property var loaded: ({})
    property var draft: ({})
    property var known: ({ "profile": [], "look": [], "keyboard-profile": [] })
    property var favourites: []
    property var service: ({ state: "unknown" })
    property bool loading: true
    property bool busy: false
    property string loadError: ""
    property string saveAsName: ""
    property bool advanced: false
    /// Shown for a moment after a change was applied.
    property bool justApplied: false

    readonly property var info: vm("status", { loaded: loaded, draft: draft, locked: locked })
    readonly property var nameCheck: vm("save_name", { name: saveAsName, existing: profileNames })
    readonly property var timing: vm("timing", draft)
    readonly property var builtins: catalogue.builtin_profiles || []

    function vm(verb, args) {
        var r = JSON.parse(Bus.vm("keys", verb, JSON.stringify(args)))
        return r.ok ? r.result : null
    }

    function gestureLabel(g) {
        switch (g) {
        //% "Press"
        case "tap": return qsTrId("keys_g2_tap")
        //% "Press twice"
        case "double": return qsTrId("keys_g2_double")
        //% "Press three times"
        case "triple": return qsTrId("keys_g2_triple")
        //% "Hold"
        case "long": return qsTrId("keys_g2_long")
        default: return g
        }
    }

    function gestureHint(g) {
        switch (g) {
        //% "One short press"
        case "tap": return qsTrId("keys_gh2_tap")
        //% "Two quick presses"
        case "double": return qsTrId("keys_gh2_double")
        //% "Three quick presses"
        case "triple": return qsTrId("keys_gh2_triple")
        //% "Keep it pressed for %1 s"
        case "long": return qsTrId("keys_gh2_long").arg(((page.timing ? page.timing.long_ms : 800) / 1000).toFixed(1))
        default: return ""
        }
    }

    /// The name a preset shows (built-ins have friendly names; the person's setups their own).
    function presetName(n) {
        switch (n) {
        //% "Open TrueSense"
        case "native": return qsTrId("keys_p_native")
        //% "Standard"
        case "default": return qsTrId("keys_p_default")
        //% "Gaming"
        case "gaming": return qsTrId("keys_p_gaming")
        //% "Work"
        case "work": return qsTrId("keys_p_work")
        //% "Lighting"
        case "lights": return qsTrId("keys_p_lights")
        //% "My keys"
        case "my-keys": return qsTrId("keys_p_mine")
        default: return n
        }
    }
    /// One line on what a built-in preset does; the person's setups show their own description.
    function presetSummary(n) {
        switch (n) {
        //% "Press opens TrueSense. Nothing else is bound."
        case "native": return qsTrId("keys_ps_native")
        //% "Press: next power mode · Twice: speakers/headphones · Hold: max fans"
        case "default": return qsTrId("keys_ps_default")
        //% "Press: Performance ⇄ Turbo · Twice: max fans · 3×: next lighting · Hold: Smart auto"
        case "gaming": return qsTrId("keys_ps_gaming")
        //% "Press: Eco, Quiet, Balanced · Twice: switch audio · 3×: mute mic · Hold: touchpad"
        case "work": return qsTrId("keys_ps_work")
        //% "Press: next keyboard effect · Twice: next rear effect · 3×: keyboard light · Hold: rear light"
        case "lights": return qsTrId("keys_ps_lights")
        default: return page.descriptions[n] || ""
        }
    }
    function presetIcon(n) {
        switch (n) {
        case "native": return "home"
        case "default": return "balanced"
        case "gaming": return "turbo"
        case "work": return "monitor"
        case "lights": return "palette"
        default: return "star"
        }
    }

    function bindingsOf(p) {
        var o = {}
        for (var i = 0; i < gestures.length; ++i)
            o[gestures[i]] = p[gestures[i]] === undefined ? "none" : p[gestures[i]]
        o.long_ms = p.long_ms
        o.multi_ms = p.multi_ms
        return o
    }

    function setAction(g, spec) {
        var d = JSON.parse(JSON.stringify(draft))
        d[g] = spec
        draft = d
        applyLater.restart()
    }

    function setTiming(key, value) {
        var d = JSON.parse(JSON.stringify(draft))
        d[key] = Math.round(value)
        draft = d
        applyLater.restart()
    }

    // ---- loading ------------------------------------------------------------------------------
    function reload(select) {
        loading = true
        var got = {}
        var waiting = 7
        function take(key, fallback) {
            return {
                ok: function (r) { got[key] = r; if (--waiting === 0) applyLoaded(got, select) },
                fail: function (e) {
                    got[key] = fallback
                    if (key === "names") page.loadError = e.message
                    if (--waiting === 0) applyLoaded(got, select)
                }
            }
        }
        var calls = [
            ["names", "keys", "profile", { action: "list" }, []],
            ["current", "keys", "profile", { action: "current" }, null],
            ["profiles", "profile", "list", {}, []],
            ["looks", "look", "list", {}, []],
            ["kprofiles", "keyboard", "profile", { action: "list" }, []],
            ["favourites", "favourite", "list", {}, []],
            ["services", "service", "check", {}, null]
        ]
        calls.forEach(function (c) {
            var t = take(c[0], c[4])
            Api.call(c[1], c[2], c[3], t.ok, t.fail)
        })
    }

    function applyLoaded(got, select) {
        if (got.names.length > 0) loadError = ""
        profileNames = got.names
        known = {
            "profile": got.profiles.map(function (p) { return p.name }),
            "look": got.looks,
            "keyboard-profile": got.kprofiles
        }
        favourites = got.favourites
        service = vm("service", { rows: got.services })
        inUse = got.current ? got.current.profile : ""
        got.names.forEach(function (n) {
            Api.call("keys", "profile", { action: "show", name: n }, function (p) {
                var d = {}
                for (var k in page.descriptions) d[k] = page.descriptions[k]
                d[n] = p.description || ""
                page.descriptions = d
            }, function () {})
        })
        var want = select !== undefined ? select
            : (got.current && got.names.indexOf(got.current.profile) >= 0 ? got.current.profile : "")
        if (want !== "") {
            showProfile(want)
        } else if (got.current) {
            // The bindings in force are not a saved setup (a hand edit): show them; a change saves
            // them as "My keys".
            profileName = ""
            locked = true
            loaded = bindingsOf(got.current)
            draft = bindingsOf(got.current)
            loading = false
        } else {
            loading = false
        }
    }

    function showProfile(name) {
        Api.call("keys", "profile", { action: "show", name: name }, function (p) {
            profileName = name
            locked = p.builtin === true
            loaded = bindingsOf(p)
            draft = bindingsOf(p)
            loading = false
        }, function (e) {
            profileName = name
            locked = true
            loaded = ({})
            draft = ({})
            loadError = e.message
            loading = false
        })
    }

    function refreshService() {
        Api.call("service", "check", {}, function (rows) { service = vm("service", { rows: rows }) },
                 function () { service = vm("service", {}) })
    }

    function startService() {
        busy = true
        Api.call("service", "repair", {}, function (steps) {
            busy = false
            var r = JSON.parse(Bus.vm("onboarding", "repair", JSON.stringify({ steps: steps }))).result
            if (r.failed.length === 0) {
                //% "The key service was started."
                Toasts.ok(qsTrId("keys_service_started"))
            } else {
                Toasts.error(r.failed[0])
            }
            refreshService()
        }, function (e) {
            busy = false
            Toasts.error(e.message)
        })
    }

    // ---- applying -----------------------------------------------------------------------------
    /// Puts a preset in use (one click on its button).
    function usePreset(name) {
        if (busy) return
        applyLater.stop()
        busy = true
        Api.call("keys", "profile", { action: "use", name: name }, function () {
            busy = false
            inUse = name
            //% "The PREDATOR key now uses “%1”."
            Toasts.ok(qsTrId("keys_using2").arg(page.presetName(name)))
            showProfile(name)
            refreshService()
        }, function (e) {
            busy = false
            Toasts.error(e.message)
        })
    }

    /// A free name for the person's version of a preset: "my-keys", else "my-keys-2", ...
    function freeMineName() {
        if (profileNames.indexOf("my-keys") < 0) return "my-keys"
        for (var i = 2; i < 100; ++i)
            if (profileNames.indexOf("my-keys-" + i) < 0) return "my-keys-" + i
        return "my-keys"
    }

    /// Saves the edited bindings and puts them in use: into the setup being edited when it is the
    /// person's, else under a new "My keys" name (a preset is never changed).
    function applyDraft() {
        if (loading || info === null || !info.changed || info.invalid.length > 0) return
        var target = locked || profileName === "" ? freeMineName() : profileName
        var a = vm("save_args", { name: target, draft: draft })
        if (a === null) return
        busy = true
        var saved = JSON.parse(JSON.stringify(draft))
        var newName = target !== profileName
        Api.call("keys", "profile", a, function () {
            Api.call("keys", "profile", { action: "use", name: target }, function () {
                busy = false
                profileName = target
                locked = false
                loaded = saved
                inUse = target
                if (newName) {
                    profileNames = profileNames.concat([target])
                    var d = {}
                    for (var k in page.descriptions) d[k] = page.descriptions[k]
                    //% "Your own setup"
                    d[target] = qsTrId("keys_mine_desc")
                    page.descriptions = d
                    //% "Saved as “%1” and in use. The preset itself is unchanged."
                    Toasts.ok(qsTrId("keys_saved_mine").arg(page.presetName(target)))
                }
                page.justApplied = true
                appliedFade.restart()
            }, function (e) { busy = false; Toasts.error(e.message) })
        }, function (e) { busy = false; Toasts.error(e.message) })
    }
    Timer { id: applyLater; interval: 500; onTriggered: page.applyDraft() }
    Timer { id: appliedFade; interval: 2500; onTriggered: page.justApplied = false }

    function saveAs(name) {
        var a = vm("save_args", { name: name, draft: draft })
        if (a === null) return
        busy = true
        Api.call("keys", "profile", a, function () {
            Api.call("keys", "profile", { action: "use", name: name }, function () {
                busy = false
                saveAsName = ""
                //% "Saved as “%1” and in use."
                Toasts.ok(qsTrId("keys_saved_as2").arg(name))
                reload(name)
            }, function (e) { busy = false; Toasts.error(e.message) })
        }, function (e) { busy = false; Toasts.error(e.message) })
    }

    function deleteProfile() {
        var name = profileName
        busy = true
        Api.call("keys", "profile", { action: "delete", name: name }, function () {
            // The key keeps working: back to the standard preset.
            Api.call("keys", "profile", { action: "use", name: "default" }, function () {
                busy = false
                //% "Deleted “%1”. The key uses Standard again."
                Toasts.ok(qsTrId("keys_deleted2").arg(page.presetName(name)))
                reload("default")
            }, function (e) { busy = false; Toasts.error(e.message); reload(undefined) })
        }, function (e) {
            busy = false
            Toasts.error(e.message)
        })
    }

    Component.onCompleted: reload(undefined)

    ConfirmDialog { id: confirm }

    // ---- 1. the key and whether it works ------------------------------------------------------
    Card {
        Layout.fillWidth: true

        RowLayout {
            Layout.fillWidth: true
            spacing: 16

            // The key, drawn as it is on the keyboard.
            Rectangle {
                Layout.alignment: Qt.AlignVCenter
                implicitWidth: 64
                implicitHeight: 64
                radius: 12
                color: "#16181c"
                border.width: 1
                border.color: page.service.state === "running" ? Theme.accent : Theme.cardLine
                PredatorEmblem {
                    anchors.centerIn: parent
                    height: 30
                    color: page.service.state === "running" ? Theme.accent : Theme.muted
                }
            }
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 4
                Text {
                    Layout.fillWidth: true
                    //% "The PREDATOR key"
                    text: qsTrId("keys_head_title")
                    color: Theme.text
                    font.pixelSize: Theme.fontSection
                    font.bold: true
                }
                Text {
                    Layout.fillWidth: true
                    //% "The key at the top left of the number pad. Press it once, twice, three times, or hold it: pick a preset below, or choose what each press does."
                    text: qsTrId("keys_head_help")
                    color: Theme.muted
                    font.pixelSize: Theme.fontBody
                    wrapMode: Text.Wrap
                }
                RowLayout {
                    spacing: 8
                    Rectangle {
                        width: 9; height: 9; radius: 4.5
                        color: page.service.state === "running" ? Theme.ok : page.service.state === "stopped" ? Theme.warn : Theme.muted
                    }
                    Text {
                        text: page.service.state === "running"
                            //% "Working"
                            ? qsTrId("keys_state_on")
                            : page.service.state === "stopped"
                                //% "Not working: the key service is stopped"
                                ? qsTrId("keys_state_off")
                                //% "Checking…"
                                : qsTrId("keys_state_unknown")
                        color: page.service.state === "stopped" ? Theme.warn : Theme.text
                        font.pixelSize: Theme.fontBody
                    }
                }
            }
            PrimaryButton {
                visible: page.service.state === "stopped"
                //% "Start it"
                text: qsTrId("keys_service_start")
                busy: page.busy
                onClicked: page.startService()
            }
        }
        HelpText {
            visible: page.service.state === "stopped" && page.service.fix === "sudo truesense service repair"
            //% "Starting it needs administrator rights. Run this in a terminal: sudo truesense service repair"
            text: qsTrId("keys_service_sudo")
        }
        HelpText {
            visible: !Live.daemonUp && Live.ok
            //% "The TrueSense system service is not running: actions that change the hardware will fail until it is."
            text: qsTrId("keys_daemon_down")
        }
    }

    // ---- 2. presets -----------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Presets"
        title: qsTrId("keys_presets_title")
        icon: "profiles"
        //% "One click puts a preset in use"
        rightText: qsTrId("keys_presets_hint")

        GridLayout {
            id: presetGrid
            Layout.fillWidth: true
            columns: Math.max(1, Math.min(3, Math.floor(width / 220)))
            columnSpacing: 10
            rowSpacing: 10
            Repeater {
                model: page.profileNames
                delegate: AbstractButton {
                    id: preset
                    required property string modelData
                    readonly property bool current: page.inUse === modelData
                    Layout.fillWidth: true
                    Layout.preferredHeight: 112
                    hoverEnabled: true
                    enabled: !page.busy
                    checkable: false
                    Accessible.role: Accessible.RadioButton
                    Accessible.name: page.presetName(modelData)
                    Accessible.checked: current
                    onClicked: if (!current) page.usePreset(modelData)
                    HoverHandler { cursorShape: Qt.PointingHandCursor }
                    background: Rectangle {
                        radius: Theme.radius
                        color: preset.hovered ? Theme.tileHover : Theme.tile
                        border.width: preset.current || preset.activeFocus ? 2 : 1
                        border.color: preset.current ? Theme.accent : preset.activeFocus ? Theme.focusRing : Theme.tileLine
                    }
                    contentItem: ColumnLayout {
                        spacing: 4
                        RowLayout {
                            Layout.fillWidth: true
                            spacing: 8
                            Icon { name: page.presetIcon(preset.modelData); size: 18; color: preset.current ? Theme.accent : Theme.text }
                            Text {
                                Layout.fillWidth: true
                                text: page.presetName(preset.modelData)
                                color: Theme.text
                                font.pixelSize: Theme.fontBody + 1
                                font.bold: true
                                elide: Text.ElideRight
                            }
                            Badge {
                                visible: preset.current
                                //% "In use"
                                text: qsTrId("keys_in_use_badge")
                                kind: "accent"
                            }
                        }
                        Text {
                            Layout.fillWidth: true
                            Layout.fillHeight: true
                            text: page.presetSummary(preset.modelData)
                            color: Theme.muted
                            font.pixelSize: Theme.fontSmall + 1
                            wrapMode: Text.Wrap
                            maximumLineCount: 3
                            elide: Text.ElideRight
                        }
                    }
                    leftPadding: 14; rightPadding: 14; topPadding: 12; bottomPadding: 10
                }
            }
        }
        HelpText {
            visible: page.loadError !== ""
            color: Theme.bad
            Accessible.role: Accessible.AlertMessage
            text: page.loadError
        }
    }

    // ---- 3. what each press does ----------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "What each press does"
        title: qsTrId("keys_gestures_title")
        icon: "keyboard"
        rightText: page.busy
            //% "Saving…"
            ? qsTrId("keys_saving")
            : page.justApplied
                //% "Saved and in use"
                ? qsTrId("keys_applied") : ""

        HelpText {
            visible: page.locked
            //% "Change anything below and it is used at once. Changing a preset saves your version as “My keys”; the preset stays as it was."
            text: qsTrId("keys_edit_help")
        }

        Repeater {
            model: page.gestures
            delegate: ColumnLayout {
                id: gestureRow
                required property string modelData
                required property int index
                Layout.fillWidth: true
                spacing: 8

                Rectangle {
                    Layout.fillWidth: true
                    visible: gestureRow.index > 0
                    height: 1
                    color: Theme.sep
                }
                RowLayout {
                    Layout.fillWidth: true
                    spacing: 12
                    // The gesture, drawn: one, two or three dots for presses, a bar for a hold.
                    Item {
                        implicitWidth: 44
                        implicitHeight: 22
                        Row {
                            anchors.centerIn: parent
                            spacing: 4
                            visible: gestureRow.modelData !== "long"
                            Repeater {
                                model: gestureRow.modelData === "tap" ? 1 : gestureRow.modelData === "double" ? 2 : 3
                                delegate: Rectangle { width: 9; height: 9; radius: 4.5; color: Theme.accent }
                            }
                        }
                        Rectangle {
                            anchors.centerIn: parent
                            visible: gestureRow.modelData === "long"
                            width: 36; height: 9; radius: 4.5
                            color: Theme.accent
                        }
                    }
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 0
                        Text {
                            text: page.gestureLabel(gestureRow.modelData)
                            color: Theme.text
                            font.pixelSize: Theme.fontBody + 1
                            font.bold: true
                            Accessible.role: Accessible.Heading
                        }
                        Text {
                            Layout.fillWidth: true
                            text: page.gestureHint(gestureRow.modelData)
                            color: Theme.muted
                            font.pixelSize: Theme.fontSmall
                            elide: Text.ElideRight
                        }
                    }
                }
                KeyActionEditor {
                    Layout.fillWidth: true
                    spec: page.draft[gestureRow.modelData] === undefined ? "none" : page.draft[gestureRow.modelData]
                    locked: page.loading
                    known: page.known
                    favourites: page.favourites
                    catalogue: page.catalogue
                    gestureLabel: page.gestureLabel(gestureRow.modelData)
                    onEdited: (s) => page.setAction(gestureRow.modelData, s)
                }
            }
        }
    }

    // ---- 4. advanced ----------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Advanced"
        title: qsTrId("keys_advanced_title")
        icon: "sliders"

        SecondaryButton {
            text: page.advanced
                //% "Hide timing and saved setups"
                ? qsTrId("keys_adv_hide")
                //% "Show timing and saved setups"
                : qsTrId("keys_adv_show")
            iconName: "chevron"
            onClicked: page.advanced = !page.advanced
        }

        ColumnLayout {
            Layout.fillWidth: true
            visible: page.advanced
            spacing: Theme.spacing

            SliderRow {
                Layout.fillWidth: true
                //% "A hold is a press longer than"
                label: qsTrId("keys_long_ms2")
                from: page.catalogue.long_ms.min
                to: page.catalogue.long_ms.max
                stepSize: page.catalogue.long_ms.step
                unit: "ms"
                value: page.timing === null ? page.catalogue.long_ms.default : page.timing.long_ms
                onEdited: (v) => page.setTiming("long_ms", v)
            }
            SliderRow {
                Layout.fillWidth: true
                //% "Quick presses count together within"
                label: qsTrId("keys_multi_ms2")
                from: page.catalogue.multi_ms.min
                to: page.catalogue.multi_ms.max
                stepSize: page.catalogue.multi_ms.step
                unit: "ms"
                value: page.timing === null ? page.catalogue.multi_ms.default : page.timing.multi_ms
                onEdited: (v) => page.setTiming("multi_ms", v)
            }
            HelpText {
                //% "A shorter time to count presses together makes a single press act sooner; a longer one makes double and triple presses easier."
                text: qsTrId("keys_timing_help2")
            }

            Rectangle { Layout.fillWidth: true; height: 1; color: Theme.sep }

            RowLayout {
                Layout.fillWidth: true
                spacing: 8
                AppTextField {
                    Layout.fillWidth: true
                    Layout.minimumWidth: 140
                    maximumLength: 40
                    //% "Save this setup as… (a name)"
                    placeholderText: qsTrId("keys_name_placeholder2")
                    Accessible.name: placeholderText
                    text: page.saveAsName
                    onTextEdited: page.saveAsName = text
                }
                SecondaryButton {
                    //% "Save as preset"
                    text: qsTrId("keys_save_as2")
                    iconName: "plus"
                    enabled: !page.busy && !page.loading && page.nameCheck.ok && page.info !== null
                        && page.info.invalid.length === 0
                    onClicked: {
                        var n = page.nameCheck.name
                        if (page.nameCheck.overwrites) {
                            confirm.ask({
                                //% "Replace this preset?"
                                title: qsTrId("keys_replace_title2"),
                                //% "“%1” already exists and will be overwritten."
                                text: qsTrId("keys_replace_text").arg(n),
                                //% "Replace"
                                confirmText: qsTrId("keys_replace_go"),
                                danger: true
                            }, function () { page.saveAs(n) })
                        } else {
                            page.saveAs(n)
                        }
                    }
                }
            }
            HelpText {
                visible: page.saveAsName !== "" && !page.nameCheck.ok
                color: Theme.warn
                text: {
                    switch (page.nameCheck.code) {
                    //% "Names use letters, digits and . _ - only (up to 40)."
                    case "chars": return qsTrId("keys_name_chars")
                    //% "That name is too long (up to 40)."
                    case "length": return qsTrId("keys_name_length")
                    //% "That is the name of a built-in preset. Pick another."
                    case "builtin": return qsTrId("keys_name_builtin2")
                    default: return ""
                    }
                }
            }
            AppButton {
                visible: !page.locked && page.profileName !== "" && page.builtins.indexOf(page.profileName) < 0
                kind: "danger-quiet"
                //% "Delete “%1”"
                text: qsTrId("keys_delete2").arg(page.presetName(page.profileName))
                iconName: "trash"
                enabled: !page.busy
                onClicked: confirm.ask({
                    //% "Delete this preset?"
                    title: qsTrId("keys_delete_title2"),
                    //% "“%1” will be removed and the key goes back to Standard. The built-in presets stay."
                    text: qsTrId("keys_delete_text2").arg(page.presetName(page.profileName)),
                    //% "Delete"
                    confirmText: qsTrId("keys_delete"),
                    danger: true
                }, page.deleteProfile)
            }
        }
    }
}
