// Keys: what the PREDATOR key and Fn+Esc do. A profile of bindings (tap, double, triple, long, Fn+Esc
// and the press timing) is picked, edited as a draft, saved under your own name and put in use; the
// key service (a user service) reloads by itself. Built-in profiles are shown but never edited:
// copy one, then change the copy. All action text is composed by ts-ui-model (vm/keys.rs).
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import org.truesense.app

PageFrame {
    id: page

    title: Nav.titleOf(Nav.currentPage)

    readonly property var gestures: ["tap", "double", "triple", "long", "fn_esc"]

    // ---- state (a page lives only while it is shown) -----------------------------------------
    readonly property var catalogue: vm("catalogue", {})
    property var profileNames: []
    /// The profile the bindings were loaded from; "" while they are the current bindings that are
    /// not saved under any name (a hand edit of the configuration).
    property string profileName: ""
    /// The profile in force now (what the key service uses).
    property string inUse: ""
    property bool locked: true
    property string description: ""
    property var loaded: ({})
    property var draft: ({})
    property var known: ({ "profile": [], "look": [], "keyboard-profile": [] })
    property var favourites: []
    property var service: ({ state: "unknown" })
    property bool loading: true
    property bool busy: false
    property string loadError: ""
    property string saveAsName: ""

    readonly property var info: vm("status", { loaded: loaded, draft: draft, locked: locked })
    readonly property var nameCheck: vm("save_name", { name: saveAsName, existing: profileNames })
    readonly property var timing: vm("timing", draft)
    readonly property bool unsaved: profileName === ""

    function vm(verb, args) {
        var r = JSON.parse(Bus.vm("keys", verb, JSON.stringify(args)))
        return r.ok ? r.result : null
    }

    function gestureLabel(g) {
        switch (g) {
        //% "Tap"
        case "tap": return qsTrId("keys_g_tap")
        //% "Double tap"
        case "double": return qsTrId("keys_g_double")
        //% "Triple tap"
        case "triple": return qsTrId("keys_g_triple")
        //% "Long press"
        case "long": return qsTrId("keys_g_long")
        //% "Fn + Esc"
        case "fn_esc": return qsTrId("keys_g_fn_esc")
        default: return g
        }
    }

    function gestureHint(g) {
        switch (g) {
        //% "One press of the PREDATOR key."
        case "tap": return qsTrId("keys_gh_tap")
        //% "Two presses in quick succession."
        case "double": return qsTrId("keys_gh_double")
        //% "Three presses in quick succession."
        case "triple": return qsTrId("keys_gh_triple")
        //% "Hold the PREDATOR key."
        case "long": return qsTrId("keys_gh_long")
        //% "The Fn key and Esc together."
        case "fn_esc": return qsTrId("keys_gh_fn_esc")
        default: return ""
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
    }

    function setTiming(key, value) {
        var d = JSON.parse(JSON.stringify(draft))
        d[key] = Math.round(value)
        draft = d
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
        var want = select !== undefined ? select
            : (got.current && got.names.indexOf(got.current.profile) >= 0 ? got.current.profile : "")
        if (want !== "") {
            showProfile(want)
        } else if (got.current) {
            // The bindings in force are not a saved profile (a hand edit): show them, unlocked.
            profileName = ""
            locked = false
            description = ""
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
            description = p.description === undefined ? "" : p.description
            loaded = bindingsOf(p)
            draft = bindingsOf(p)
            loading = false
        }, function (e) {
            // A profile that cannot be read (a hand edit with an action that no longer exists) is
            // shown as such, read-only, instead of leaving the previous profile's values on screen.
            profileName = name
            locked = true
            description = ""
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

    // ---- profile actions ----------------------------------------------------------------------
    function saveAs(name, done) {
        var a = vm("save_args", { name: name, draft: draft })
        if (a === null) return
        busy = true
        Api.call("keys", "profile", a, function () {
            busy = false
            //% "Saved the key profile “%1”."
            Toasts.ok(qsTrId("keys_saved").arg(name))
            if (done) done()
        }, function (e) {
            busy = false
            Toasts.error(e.message)
        })
    }

    function useProfile() {
        var apply = function () {
            Api.call("keys", "profile", { action: "use", name: profileName }, function () {
                busy = false
                inUse = profileName
                //% "The PREDATOR key now uses “%1”. The key service reloads by itself."
                Toasts.ok(qsTrId("keys_using").arg(profileName))
                refreshService()
            }, function (e) {
                busy = false
                Toasts.error(e.message)
            })
        }
        busy = true
        if (!locked && info.changed) {
            var a = vm("save_args", { name: profileName, draft: draft })
            if (a === null) { busy = false; return }
            Api.call("keys", "profile", a, function () { loaded = JSON.parse(JSON.stringify(draft)); apply() },
                     function (e) { busy = false; Toasts.error(e.message) })
        } else {
            apply()
        }
    }

    function copyProfile() {
        busy = true
        var args = { action: "copy", name: profileName }
        if (nameCheck.ok) args["new"] = nameCheck.name
        Api.call("keys", "profile", args, function (newName) {
            busy = false
            saveAsName = ""
            //% "Copied to “%1”. Change what you like, then save and use it."
            Toasts.ok(qsTrId("keys_copied").arg(newName))
            reload(newName)
        }, function (e) {
            busy = false
            Toasts.error(e.message)
        })
    }

    function deleteProfile() {
        var name = profileName
        busy = true
        Api.call("keys", "profile", { action: "delete", name: name }, function () {
            busy = false
            //% "Deleted the key profile “%1”."
            Toasts.ok(qsTrId("keys_deleted").arg(name))
            reload(undefined)
        }, function (e) {
            busy = false
            Toasts.error(e.message)
        })
    }

    function choose(name) {
        if (name === profileName) return
        if (info.changed && !locked) {
            confirm.ask({
                //% "Discard your changes?"
                title: qsTrId("keys_discard_title"),
                //% "The changes to this profile are not saved yet."
                text: qsTrId("keys_discard_text"),
                //% "Discard"
                confirmText: qsTrId("keys_discard_go"),
                danger: true
            }, function () { showProfile(name) })
        } else {
            showProfile(name)
        }
    }

    Component.onCompleted: reload(undefined)

    ConfirmDialog { id: confirm }

    // ---- the key service ----------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Key service"
        title: qsTrId("keys_service_title")
        icon: "key"

        RowLayout {
            Layout.fillWidth: true
            spacing: 10
            Rectangle {
                Layout.alignment: Qt.AlignVCenter
                width: 10
                height: 10
                radius: 5
                color: page.service.state === "running" ? Theme.ok : page.service.state === "stopped" ? Theme.warn : Theme.muted
            }
            Text {
                Layout.fillWidth: true
                color: Theme.text
                font.pixelSize: Theme.fontBody
                wrapMode: Text.Wrap
                text: page.service.state === "running"
                    //% "The key service is running: the PREDATOR key and Fn+Esc are active."
                    ? qsTrId("keys_service_running")
                    : page.service.state === "stopped"
                        //% "The key service is not running, so the PREDATOR key does nothing."
                        ? qsTrId("keys_service_stopped")
                        //% "The state of the key service could not be read."
                        : qsTrId("keys_service_unknown")
            }
            SecondaryButton {
                //% "Check again"
                text: qsTrId("keys_service_refresh")
                iconName: "refresh"
                onClicked: page.refreshService()
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

    // ---- the profile --------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Binding profile"
        title: qsTrId("keys_profile_title")
        icon: "profiles"
        rightText: page.inUse !== ""
            //% "In use: %1"
            ? qsTrId("keys_in_use").arg(page.inUse) : ""

        HelpText {
            //% "A profile is a saved set of what each press does. Built-in profiles cannot be changed: copy one, then edit the copy."
            text: qsTrId("keys_profile_help")
        }
        SelectRow {
            Layout.fillWidth: true
            //% "Profile"
            label: qsTrId("keys_profile_pick")
            options: page.profileNames.map(function (n) {
                var mine = page.catalogue.builtin_profiles.indexOf(n) < 0
                //% "%1 (yours)"
                return { value: n, text: mine ? qsTrId("keys_yours").arg(n) : n }
            })
            current: page.profileName === "" ? undefined : page.profileName
            enabled: !page.loading && !page.busy
            onActivated: (v) => page.choose(v)
        }
        HelpText {
            visible: page.unsaved && !page.loading
            //% "These are the bindings in force now. They are not saved as a profile: save them under a name to keep them."
            text: qsTrId("keys_unsaved")
        }
        HelpText {
            visible: page.description !== ""
            text: page.description
        }
        HelpText {
            visible: page.loadError !== ""
            color: Theme.bad
            Accessible.role: Accessible.AlertMessage
            text: page.loadError
        }
        Flow {
            Layout.fillWidth: true
            spacing: 8
            PrimaryButton {
                //% "Use these keys"
                text: qsTrId("keys_use")
                iconName: "check"
                busy: page.busy
                enabled: !page.loading && page.info !== null && page.info.can_use && !page.unsaved
                onClicked: page.useProfile()
            }
            SecondaryButton {
                visible: !page.locked && !page.unsaved
                //% "Save changes"
                text: qsTrId("keys_save")
                iconName: "save"
                enabled: !page.busy && page.info !== null && page.info.changed && page.info.can_save
                onClicked: page.saveAs(page.profileName, function () { page.reload(page.profileName) })
            }
            SecondaryButton {
                visible: page.locked
                //% "Copy to edit"
                text: qsTrId("keys_copy")
                iconName: "copy"
                enabled: !page.busy && (page.saveAsName === "" || page.nameCheck.ok)
                onClicked: page.copyProfile()
            }
            AppButton {
                visible: !page.locked && !page.unsaved
                kind: "danger"
                //% "Delete"
                text: qsTrId("keys_delete")
                iconName: "trash"
                enabled: !page.busy
                onClicked: confirm.ask({
                    //% "Delete this key profile?"
                    title: qsTrId("keys_delete_title"),
                    //% "“%1” will be removed. The built-in profiles stay."
                    text: qsTrId("keys_delete_text").arg(page.profileName),
                    confirmText: qsTrId("keys_delete"),
                    danger: true
                }, page.deleteProfile)
            }
        }
        RowLayout {
            Layout.fillWidth: true
            spacing: 8
            AppTextField {
                Layout.fillWidth: true
                Layout.minimumWidth: 140
                maximumLength: 40
                //% "Name to save as"
                placeholderText: qsTrId("keys_name_placeholder")
                Accessible.name: placeholderText
                text: page.saveAsName
                onTextEdited: page.saveAsName = text
            }
            SecondaryButton {
                //% "Save as"
                text: qsTrId("keys_save_as")
                iconName: "plus"
                enabled: !page.busy && !page.loading && page.nameCheck.ok && page.info !== null
                    && page.info.invalid.length === 0
                onClicked: {
                    var n = page.nameCheck.name
                    var go = function () {
                        page.saveAs(n, function () { page.saveAsName = ""; page.reload(n) })
                    }
                    if (page.nameCheck.overwrites) {
                        confirm.ask({
                            //% "Replace this profile?"
                            title: qsTrId("keys_replace_title"),
                            //% "“%1” already exists and will be overwritten."
                            text: qsTrId("keys_replace_text").arg(n),
                            //% "Replace"
                            confirmText: qsTrId("keys_replace_go"),
                            danger: true
                        }, go)
                    } else {
                        go()
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
                //% "That is the name of a built-in profile. Pick another."
                case "builtin": return qsTrId("keys_name_builtin")
                default: return ""
                }
            }
        }
    }

    // ---- the gestures -------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "What each press does"
        title: qsTrId("keys_gestures_title")
        icon: "keyboard"
        rightText: page.locked
            //% "Read-only: copy the profile to edit"
            ? qsTrId("keys_readonly")
            : page.info !== null && page.info.changed
                //% "Changed"
                ? qsTrId("keys_changed") : ""

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
                    spacing: 8
                    Text {
                        text: page.gestureLabel(gestureRow.modelData)
                        color: Theme.text
                        font.pixelSize: Theme.fontBody
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
                KeyActionEditor {
                    Layout.fillWidth: true
                    spec: page.draft[gestureRow.modelData] === undefined ? "none" : page.draft[gestureRow.modelData]
                    locked: page.locked
                    known: page.known
                    favourites: page.favourites
                    catalogue: page.catalogue
                    gestureLabel: page.gestureLabel(gestureRow.modelData)
                    onEdited: (s) => page.setAction(gestureRow.modelData, s)
                }
            }
        }

        HelpText {
            //% "Favourites are the looks, fan curves, Smart profiles and profiles you starred. “Favourite: apply number N” applies the Nth star, counted as the Profiles and favourites page lists them; “Favourite: next” steps through all of them and shows each name. Undervolt and power limits can never be starred, so a key never changes them."
            text: qsTrId("keys_favourites_help")
        }
    }

    // ---- timing -------------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Timing"
        title: qsTrId("keys_timing_title")
        icon: "sliders"

        SliderRow {
            Layout.fillWidth: true
            //% "Long press after"
            label: qsTrId("keys_long_ms")
            from: page.catalogue.long_ms.min
            to: page.catalogue.long_ms.max
            stepSize: page.catalogue.long_ms.step
            unit: "ms"
            value: page.timing === null ? page.catalogue.long_ms.default : page.timing.long_ms
            enabled: !page.locked
            onEdited: (v) => page.setTiming("long_ms", v)
        }
        SliderRow {
            Layout.fillWidth: true
            //% "Taps count together within"
            label: qsTrId("keys_multi_ms")
            from: page.catalogue.multi_ms.min
            to: page.catalogue.multi_ms.max
            stepSize: page.catalogue.multi_ms.step
            unit: "ms"
            value: page.timing === null ? page.catalogue.multi_ms.default : page.timing.multi_ms
            enabled: !page.locked
            onEdited: (v) => page.setTiming("multi_ms", v)
        }
        HelpText {
            //% "A press held longer than the first time is a long press. Taps closer together than the second time count as a double or triple tap. While neither is set, a tap acts the moment the key comes up."
            text: qsTrId("keys_timing_help")
        }
    }
}
