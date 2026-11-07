// Edits the action of one gesture of the PREDATOR key: a kind picker built from the action
// catalogue, the control that kind needs (a choice, a fan speed, a list of power modes, the name of
// a look or profile, a favourite number, a command), and a Test button.
//
// The grammar lives in Rust (ts-ui-model vm/keys.rs mirrors ts-domain's action parser and is tested
// against it); this file only draws what `Bus.vm("keys", ...)` describes and never assembles action
// text itself.
//
//   KeyActionEditor {
//       spec: draft.tap; known: lists; favourites: favs; locked: profileIsBuiltIn
//       onEdited: (s) => draft.tap = s
//   }
//
// `edited(spec)` is the person's own change only; setting `spec` from code (loading a profile) is
// silent and never sends anything.
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import org.truesense.app

ColumnLayout {
    id: root

    /// The action text shown. Changing it from code is silent.
    property string spec: "none"
    /// A built-in profile is shown but cannot be edited.
    property bool locked: false
    /// {profile: [names], look: [names], "keyboard-profile": [names]}: what the pickers offer.
    property var known: ({})
    /// The answer of `favourite list`, for the line under the favourite number.
    property var favourites: []
    /// The `catalogue` verb's answer, shared by the five editors of the page.
    property var catalogue: ({ groups: [], modes: [], fan: { min: 20, max: 100, step: 5 }, favourites: { max: 24 } })
    /// The name of the gesture, for accessible names.
    property string gestureLabel: ""

    /// The person changed the action; `spec` is its text (an unreadable one while a field is still
    /// empty, which the page reports and refuses to save).
    signal edited(string spec)

    property var st: ({ kind: "none", choice: "", text: "", percent: 50, position: 1, modes: [] })
    property string error: ""
    property bool missing: false
    property string _emitted: ""
    property bool testing: false

    readonly property var _kinds: {
        var out = []
        for (var g = 0; g < catalogue.groups.length; ++g)
            for (var k = 0; k < catalogue.groups[g].kinds.length; ++k)
                out.push(catalogue.groups[g].kinds[k])
        return out
    }
    readonly property var _def: {
        for (var i = 0; i < _kinds.length; ++i)
            if (_kinds[i].kind === st.kind) return _kinds[i]
        return { kind: st.kind, param: "none" }
    }
    readonly property var _kindOptions: _kinds.map(function (d) { return { value: d.kind, text: kindLabel(d.kind) } })

    spacing: 8

    function _vm(verb, args) {
        var r = JSON.parse(Bus.vm("keys", verb, JSON.stringify(args)))
        return r.ok ? r.result : null
    }

    function _load() {
        if (spec === _emitted) return
        var r = _vm("parse", { spec: spec, known: known })
        if (r === null) return
        st = r.state
        error = r.valid ? "" : r.error
        missing = r.missing
    }
    onSpecChanged: _load()
    onKnownChanged: _load()
    Component.onCompleted: { _emitted = ""; _load() }

    /// Sets members of the state, composes, and reports the new text.
    function change(members) {
        var s = JSON.parse(JSON.stringify(st))
        for (var key in members) s[key] = members[key]
        st = s
        var c = _vm("compose", { state: s })
        if (c === null) return
        error = c.valid ? "" : c.error
        // An incomplete action (an empty command) is reported as the unreadable text it is, so the
        // page can refuse to save it; it is never replaced by a guess.
        var text = c.valid ? c.spec : s.kind + ":"
        missing = c.valid ? _vm("parse", { spec: text, known: known }).missing : false
        _emitted = text
        edited(text)
    }

    function pickKind(kind) {
        var d = _vm("defaults", { kind: kind, known: known })
        if (d === null) return
        change(d)
    }

    function toggleMode(mode) {
        var list = st.modes.slice()
        var at = list.indexOf(mode)
        if (at >= 0) list.splice(at, 1)
        else list.push(mode)
        change({ modes: list })
    }

    function runTest() {
        var c = _vm("compose", { state: st })
        if (c === null || !c.valid) return
        testing = true
        Api.call("keys", "test", { action: c.spec }, function (result) {
            testing = false
            //% "Done."
            Toasts.ok(typeof result === "string" && result !== "" ? result : qsTrId("keys_test_done"))
        }, function (err) {
            testing = false
            Toasts.error(err.message)
        })
    }

    // ---- the words, translated (one place) ----------------------------------------------------
    function kindLabel(kind) {
        switch (kind) {
        //% "Do nothing"
        case "none": return qsTrId("keys_kind_none")
        //% "Power mode: next"
        case "power-next": return qsTrId("keys_kind_power_next")
        //% "Power mode: previous"
        case "power-prev": return qsTrId("keys_kind_power_prev")
        //% "Power mode: set one"
        case "power": return qsTrId("keys_kind_power")
        //% "Power mode: cycle through a list"
        case "power-cycle": return qsTrId("keys_kind_power_cycle")
        //% "Mode control: next (Manual, Native, Smart)"
        case "auto-next": return qsTrId("keys_kind_auto_next")
        //% "Mode control: choose"
        case "auto": return qsTrId("keys_kind_auto")
        //% "Fans: set"
        case "fan": return qsTrId("keys_kind_fan")
        //% "Fans: toggle Max"
        case "fan-max-toggle": return qsTrId("keys_kind_fan_max_toggle")
        //% "Fans: toggle the Smart curve"
        case "smart-fan-toggle": return qsTrId("keys_kind_smart_fan_toggle")
        //% "Profile: apply one"
        case "profile": return qsTrId("keys_kind_profile")
        //% "Profile: next"
        case "profile-next": return qsTrId("keys_kind_profile_next")
        //% "Favourite: apply number N"
        case "favourite": return qsTrId("keys_kind_favourite")
        //% "Favourite: next"
        case "favourite-next": return qsTrId("keys_kind_favourite_next")
        //% "Sound: switch speakers and headphones"
        case "output-toggle": return qsTrId("keys_kind_output_toggle")
        //% "Sound: cycle the outputs"
        case "output-cycle": return qsTrId("keys_kind_output_cycle")
        //% "Sound: choose an output"
        case "output": return qsTrId("keys_kind_output")
        //% "Microphone: mute or unmute"
        case "mic-toggle": return qsTrId("keys_kind_mic_toggle")
        //% "Microphone: choose mute or unmute"
        case "mic": return qsTrId("keys_kind_mic")
        //% "Microphone: next voice mode"
        case "voice-next": return qsTrId("keys_kind_voice_next")
        //% "Touchpad: toggle"
        case "touchpad-toggle": return qsTrId("keys_kind_touchpad_toggle")
        //% "Touchpad: choose on or off"
        case "touchpad": return qsTrId("keys_kind_touchpad")
        //% "Keyboard light: on, off or toggle"
        case "keyboard": return qsTrId("keys_kind_keyboard")
        //% "Keyboard light: next effect"
        case "keyboard-effect-next": return qsTrId("keys_kind_keyboard_effect_next")
        //% "Keyboard light: apply a per-key profile"
        case "keyboard-profile": return qsTrId("keys_kind_keyboard_profile")
        //% "Rear light: on, off or toggle"
        case "rear": return qsTrId("keys_kind_rear")
        //% "Rear light: next effect"
        case "rear-effect-next": return qsTrId("keys_kind_rear_effect_next")
        //% "Lighting: apply a look"
        case "look": return qsTrId("keys_kind_look")
        //% "Battery: toggle the 80 % limit"
        case "battery-limit-toggle": return qsTrId("keys_kind_battery_limit_toggle")
        //% "Screen: toggle overdrive"
        case "overdrive-toggle": return qsTrId("keys_kind_overdrive_toggle")
        //% "Run a program or script"
        case "exec": return qsTrId("keys_kind_exec")
        //% "Show a message"
        case "notify": return qsTrId("keys_kind_notify")
        default: return kind
        }
    }

    function choiceLabel(kind, word) {
        switch (word) {
        //% "Eco"
        case "eco": return qsTrId("keys_choice_eco")
        //% "Quiet"
        case "quiet": return qsTrId("keys_choice_quiet")
        //% "Balanced"
        case "balanced": return qsTrId("keys_choice_balanced")
        //% "Performance"
        case "performance": return qsTrId("keys_choice_performance")
        //% "Turbo"
        case "turbo": return qsTrId("keys_choice_turbo")
        //% "On"
        case "on": return qsTrId("keys_choice_on")
        //% "Toggle"
        case "toggle": return qsTrId("keys_choice_toggle")
        //% "Native"
        case "native": return qsTrId("keys_choice_native")
        //% "Smart"
        case "smart": return qsTrId("keys_choice_smart")
        //% "Speakers"
        case "speakers": return qsTrId("keys_choice_speakers")
        //% "Headphones"
        case "headphones": return qsTrId("keys_choice_headphones")
        //% "Mute"
        case "mute": return qsTrId("keys_choice_mute")
        //% "Unmute"
        case "unmute": return qsTrId("keys_choice_unmute")
        case "off":
            //% "Manual"
            return kind === "auto" ? qsTrId("keys_choice_manual")
                //% "Off"
                : qsTrId("keys_choice_off")
        default: return word
        }
    }

    function fanLabel(word) {
        switch (word) {
        //% "Smart curve"
        case "smart": return qsTrId("keys_fan_smart")
        //% "Firmware auto"
        case "auto": return qsTrId("keys_fan_auto")
        //% "Max"
        case "max": return qsTrId("keys_fan_max")
        //% "Fixed speed"
        case "manual": return qsTrId("keys_fan_manual")
        default: return word
        }
    }

    // ---- the kind picker and Test ---------------------------------------------------------------
    RowLayout {
        Layout.fillWidth: true
        spacing: 8

        SelectRow {
            Layout.fillWidth: true
            options: root._kindOptions
            current: root.error !== "" && root.st.kind === "none" ? undefined : root.st.kind
            minimumControlWidth: 220
            enabled: !root.locked
            Accessible.name: root.gestureLabel
            onActivated: (value) => root.pickKind(value)
        }
        AppButton {
            //% "Test"
            text: qsTrId("keys_test")
            iconName: "refresh"
            busy: root.testing
            enabled: root.error === ""
            //% "Run %1 now"
            Accessible.name: qsTrId("keys_test_named").arg(root.gestureLabel)
            onClicked: {
                if (root.st.kind === "exec") {
                    //% "Run this command now?"
                    confirm.ask({ title: qsTrId("keys_test_exec_title"),
                                  //% "It runs as you, exactly as the key would: %1"
                                  text: qsTrId("keys_test_exec_text").arg(root.st.text),
                                  //% "Run it"
                                  confirmText: qsTrId("keys_test_exec_go") }, root.runTest)
                } else {
                    root.runTest()
                }
            }
        }
    }

    // ---- what the kind needs --------------------------------------------------------------------
    Loader {
        Layout.fillWidth: true
        active: root._def.param !== "none"
        visible: active
        sourceComponent: {
            switch (root._def.param) {
            case "choice": return choiceControl
            case "fan": return fanControl
            case "modes": return modesControl
            case "library": return libraryControl
            case "position": return positionControl
            case "text": return textControl
            default: return null
            }
        }
    }

    HelpText {
        Layout.fillWidth: true
        visible: root.error !== ""
        color: Theme.bad
        //% "Not an action TrueSense knows (%1). Pick one above to replace it."
        text: qsTrId("keys_unreadable").arg(root.error)
    }

    Component {
        id: choiceControl
        ColumnLayout {
            anchors.left: parent.left
            anchors.right: parent.right
            readonly property bool few: root._def.options.length <= 3
            readonly property var items: (root._def.options || []).map(function (w) {
                return { value: w, label: root.choiceLabel(root.st.kind, w), text: root.choiceLabel(root.st.kind, w) }
            })
            spacing: 0
            Segmented {
                Layout.alignment: Qt.AlignLeft
                visible: few
                model: items
                current: root.st.choice
                enabled: !root.locked
                onActivated: (v) => root.change({ choice: v })
            }
            SelectRow {
                Layout.fillWidth: true
                visible: !few
                minimumControlWidth: 220
                options: items
                current: root.st.choice
                enabled: !root.locked
                onActivated: (v) => root.change({ choice: v })
            }
        }
    }

    Component {
        id: fanControl
        ColumnLayout {
            anchors.left: parent.left
            anchors.right: parent.right
            spacing: 8
            readonly property var items: (root.catalogue.fan.choices || []).map(function (w) {
                return { value: w, label: root.fanLabel(w), text: root.fanLabel(w) }
            })
            // Four pills do not fit a narrow window: a drop-down then.
            readonly property bool pills: root.width >= 500
            Segmented {
                Layout.alignment: Qt.AlignLeft
                visible: parent.pills
                model: parent.items
                current: root.st.choice
                enabled: !root.locked
                onActivated: (v) => root.change({ choice: v })
            }
            SelectRow {
                Layout.fillWidth: true
                visible: !parent.pills
                minimumControlWidth: 220
                options: parent.items
                current: root.st.choice
                enabled: !root.locked
                onActivated: (v) => root.change({ choice: v })
            }
            SliderRow {
                Layout.fillWidth: true
                visible: root.st.choice === "manual"
                //% "Fan speed"
                label: qsTrId("keys_fan_speed")
                from: root.catalogue.fan.min
                to: root.catalogue.fan.max
                stepSize: root.catalogue.fan.step
                unit: "%"
                value: root.st.percent
                enabled: !root.locked
                onEdited: (v) => root.change({ percent: Math.round(v) })
            }
            HelpText {
                visible: root.st.choice === "manual"
                //% "A key never stops a fan: %1 % is the lowest speed it can set."
                text: qsTrId("keys_fan_floor").arg(root.catalogue.fan.min)
            }
        }
    }

    Component {
        id: modesControl
        ColumnLayout {
            anchors.left: parent.left
            anchors.right: parent.right
            spacing: 8
            Flow {
                Layout.fillWidth: true
                spacing: 8
                Repeater {
                    model: root.catalogue.modes
                    delegate: Button {
                        id: chip
                        required property var modelData
                        readonly property bool on: root.st.modes.indexOf(modelData) >= 0
                        enabled: !root.locked
                        checkable: false
                        hoverEnabled: true
                        focusPolicy: Qt.StrongFocus
                        implicitHeight: 32
                        Accessible.role: Accessible.CheckBox
                        Accessible.name: root.choiceLabel("power", modelData)
                        Accessible.checkable: true
                        Accessible.checked: on
                        onClicked: root.toggleMode(modelData)
                        contentItem: Text {
                            text: root.choiceLabel("power", chip.modelData)
                            color: !chip.enabled ? Theme.disabledText : chip.on ? Theme.accentInk : Theme.stat
                            font.pixelSize: Theme.fontBody
                            font.bold: chip.on
                            horizontalAlignment: Text.AlignHCenter
                            verticalAlignment: Text.AlignVCenter
                        }
                        background: Rectangle {
                            radius: 16
                            color: chip.on ? Theme.accent : (chip.hovered ? Theme.tileHover : Theme.tile)
                            border.width: chip.activeFocus ? 1 : 0
                            border.color: Theme.focusRing
                        }
                    }
                }
            }
            HelpText {
                //% "Each press goes to the next mode of the list, in the order you picked them: %1."
                text: root.st.modes.length > 0
                    ? qsTrId("keys_cycle_help").arg(root.st.modes.map(function (m) { return root.choiceLabel("power", m) }).join(" → "))
                    //% "Pick at least one power mode."
                    : qsTrId("keys_cycle_empty")
            }
        }
    }

    Component {
        id: libraryControl
        ColumnLayout {
            anchors.left: parent.left
            anchors.right: parent.right
            readonly property var names: {
                var l = root.known[root.st.kind]
                return l === undefined ? [] : l
            }
            spacing: 6
            SelectRow {
                Layout.fillWidth: true
                minimumControlWidth: 220
                options: {
                    var o = names.map(function (n) { return { value: n, text: n } })
                    if (root.st.text !== "" && names.indexOf(root.st.text) < 0)
                        o.push({ value: root.st.text, text: root.st.text })
                    return o
                }
                current: root.st.text
                enabled: !root.locked
                onActivated: (v) => root.change({ text: v })
            }
            HelpText {
                visible: root.missing
                color: Theme.warn
                //% "“%1” no longer exists. Pick another, or the key will report an error."
                text: qsTrId("keys_missing").arg(root.st.text)
            }
            HelpText {
                visible: names.length === 0
                //% "Nothing saved yet: create one on its own page first."
                text: qsTrId("keys_library_empty")
            }
        }
    }

    Component {
        id: positionControl
        ColumnLayout {
            anchors.left: parent.left
            anchors.right: parent.right
            spacing: 6
            SliderRow {
                Layout.fillWidth: true
                //% "Favourite number"
                label: qsTrId("keys_fav_number")
                from: 1
                to: root.catalogue.favourites.max
                stepSize: 1
                value: root.st.position
                enabled: !root.locked
                onEdited: (v) => root.change({ position: Math.round(v) })
            }
            HelpText {
                readonly property var item: {
                    for (var i = 0; i < root.favourites.length; ++i)
                        if (root.favourites[i].position === root.st.position) return root.favourites[i]
                    return null
                }
                text: item !== null
                    //% "Number %1 is now: %2"
                    ? qsTrId("keys_fav_is").arg(root.st.position).arg(item.name)
                    //% "There is no favourite number %1 yet; the key will say so."
                    : qsTrId("keys_fav_none").arg(root.st.position)
            }
        }
    }

    Component {
        id: textControl
        ColumnLayout {
            anchors.left: parent.left
            anchors.right: parent.right
            spacing: 6
            AppTextField {
                Layout.fillWidth: true
                text: root.st.text
                enabled: !root.locked
                maximumLength: 500
                placeholderText: root.st.kind === "exec"
                    //% "e.g. notify-send hello, or /home/me/script.sh"
                    ? qsTrId("keys_exec_placeholder")
                    //% "The message to show"
                    : qsTrId("keys_notify_placeholder")
                Accessible.name: root.kindLabel(root.st.kind)
                onTextEdited: root.change({ text: text })
            }
            HelpText {
                visible: root.st.kind === "exec"
                //% "Runs as you, in the background, when the key is pressed. Use Test to try it."
                text: qsTrId("keys_exec_help")
            }
        }
    }

    ConfirmDialog { id: confirm }
}
