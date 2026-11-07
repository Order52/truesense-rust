// Lighting > Keyboard: a live picture of the keyboard, the 13 built-in effects with their options,
// the backlight, and what follows the power mode, idle and boot. Which options an effect uses
// (colour, speed, direction) comes from the view-model's catalogue; the controls it does not use
// stay visible and greyed, with a line saying why.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

ColumnLayout {
    id: tab

    spacing: Theme.spacing + 2

    // What the lights are now (the last status read, then our own changes).
    property string kind: "unknown"      // effect | perkey | off | unknown
    property string effectId: ""
    property string colour: "00aec7"
    property int speed: 5
    property int direction: 0
    property int brightness: 25
    property bool backlightOn: true
    property bool follow: false
    property bool idleOff: false
    property var perKeyCells: null
    property var preview: ({ cells: [], kind: "unknown", exact: false })

    readonly property var catalogue: LightingVm.call("effects") || []
    readonly property var rule: {
        for (var i = 0; i < catalogue.length; ++i)
            if (catalogue[i].id === effectId) return catalogue[i]
        return null
    }
    readonly property bool usable: Caps.usable("keyboard-light") && Caps.usable("keyboard_backlight")
    readonly property bool effectActive: kind === "effect" && rule !== null

    function _look() {
        if (kind === "effect") return { mode: "effect", name: effectId, color: colour, speed: speed, brightness: brightness }
        if (kind === "perkey") return { mode: "perkey", brightness: brightness }
        if (kind === "off") return { mode: "off" }
        return {}
    }

    function updatePreview() {
        var p = LightingVm.call("preview", { look: _look(), cells: perKeyCells })
        if (p !== undefined) preview = p
    }

    function refresh() {
        Api.call("keyboard", "status", {}, function (st) {
            var look = st.look || {}
            if (typeof st.brightness === "number") {
                brightness = st.brightness
                backlightOn = st.brightness > 0
            }
            if (look.mode === "effect") {
                kind = "effect"
                effectId = look.name || ""
                colour = look.color || "00aec7"
                if (look.speed) speed = look.speed
                direction = look.direction === 1 ? 1 : 0
            } else if (look.mode === "perkey") {
                kind = "perkey"
                effectId = ""
            } else {
                kind = look.mode === "off" ? "off" : "unknown"
                effectId = ""
            }
            if (kind === "perkey") {
                Api.call("keyboard", "cells", {}, function (c) {
                    perKeyCells = c.cells
                    updatePreview()
                }, function () { updatePreview() })
            } else {
                updatePreview()
            }
        }, function () { updatePreview() })
        Api.call("keyboard", "operating", { action: "status" }, function (v) { if (typeof v === "boolean") follow = v }, function () {})
        Api.call("keyboard", "timeout", { action: "status" }, function (v) { if (typeof v === "boolean") idleOff = v }, function () {})
    }

    function pick(id) {
        var args = LightingVm.call("pick_effect", {
            name: id, current: { colour: colour, speed: speed, brightness: brightness, direction: direction } })
        if (args === undefined) return
        Api.call("keyboard", "effect", args, function () {
            kind = "effect"
            effectId = id
            brightness = args.brightness
            backlightOn = true
            updatePreview()
        })
    }

    function setColour(hex) {
        colour = hex
        Api.write("kb.color", "keyboard", "color", { color: hex })
        updatePreview()
    }

    Component.onCompleted: {
        LightingVm.loadLayout()
        refresh()
    }

    CapabilityNote {
        capability: "keyboard-light"
        //% "This laptop has no keyboard backlight control."
        text: qsTrId("light_kb_no_backlight")
    }
    CapabilityNote { capability: "keyboard_backlight" }

    Card {
        Layout.fillWidth: true
        //% "Keyboard now"
        title: qsTrId("light_kb_preview")
        icon: "keyboard"

        KeyboardView {
            Layout.fillWidth: true
            cells: tab.preview.cells
            //% "Picture of the keyboard in the colours it shows now"
            description: qsTrId("light_kb_preview_a11y")
        }
        RowLayout {
            Layout.fillWidth: true
            spacing: 8
            HelpText {
                text: tab.preview.kind === "unknown"
                    //% "The effect on the keyboard is not known yet: pick one below."
                    ? qsTrId("light_kb_preview_unknown")
                    : (tab.preview.exact ? ""
                        //% "A moving effect is drawn here as one still frame."
                        : qsTrId("light_kb_preview_approx"))
                visible: text !== ""
            }
            Item { Layout.fillWidth: true }
            SecondaryButton {
                //% "Refresh"
                text: qsTrId("light_kb_refresh")
                iconName: "refresh"
                onClicked: tab.refresh()
            }
        }
    }

    Card {
        Layout.fillWidth: true
        enabled: tab.usable
        //% "Backlight"
        title: qsTrId("light_kb_backlight")
        icon: "sun"

        ToggleRow {
            Layout.fillWidth: true
            //% "Keyboard light"
            label: qsTrId("light_kb_on")
            checked: tab.backlightOn
            onToggled: (on) => Api.call("keyboard", on ? "on" : "off", {}, function () { tab.refresh() })
        }
        SliderRow {
            Layout.fillWidth: true
            //% "Brightness"
            label: qsTrId("light_kb_brightness")
            from: 0
            to: 50
            stepSize: 1
            value: tab.brightness
            valueText: value === 0
                //% "Off"
                ? qsTrId("light_kb_level_off") : Math.round(value * 2) + " %"
            coalesceMs: 90
            onEdited: (v) => {
                tab.brightness = v
                tab.backlightOn = v > 0
                Api.write("kb.level", "keyboard", "brightness", { level: v })
                tab.updatePreview()
            }
        }
    }

    Card {
        Layout.fillWidth: true
        enabled: tab.usable
        //% "Effect"
        title: qsTrId("light_kb_effect")
        icon: "sparkle"

        TileRow {
            Layout.fillWidth: true
            variant: "outlined"
            tileHeight: 76
            minTileWidth: 96
            current: tab.kind === "effect" ? tab.effectId : undefined
            model: {
                var out = []
                for (var i = 0; i < tab.catalogue.length; ++i)
                    out.push({ value: tab.catalogue[i].id, label: LightingVm.effectName(tab.catalogue[i].id), icon: tab.catalogue[i].icon })
                return out
            }
            onActivated: (id) => tab.pick(id)
        }

        Text {
            Layout.fillWidth: true
            Layout.topMargin: 4
            //% "Colour"
            text: qsTrId("light_kb_colour")
            color: tab.effectActive && tab.rule.colour ? Theme.text : Theme.disabledText
            font.pixelSize: Theme.fontBody
        }
        SwatchRow {
            Layout.fillWidth: true
            enabled: tab.effectActive && tab.rule.colour
            colors: LightingVm.call("palette").map(LightingVm.css)
            current: LightingVm.css(tab.colour)
            allowCustom: true
            onActivated: (c) => tab.setColour(c.substring(1))
            onCustomRequested: { picker.start(tab.colour); picker.open() }
        }
        HelpText {
            visible: tab.effectActive && !tab.rule.colour
            //% "This effect has its own colours: the keyboard ignores a colour choice."
            text: qsTrId("light_kb_no_colour")
        }
        SliderRow {
            Layout.fillWidth: true
            enabled: tab.effectActive && tab.rule.speed
            //% "Speed"
            label: qsTrId("light_kb_speed")
            from: 1
            to: 12
            stepSize: 1
            value: tab.speed
            coalesceMs: 120
            onEdited: (v) => {
                tab.speed = v
                Api.write("kb.speed", "keyboard", "speed", { level: v })
            }
        }
        SettingRow {
            Layout.fillWidth: true
            enabled: tab.effectActive && tab.rule.direction
            //% "Direction"
            label: qsTrId("light_kb_direction")
            Segmented {
                current: tab.direction
                model: [
                    //% "Left to right"
                    { value: 0, label: qsTrId("light_kb_dir_ltr") },
                    //% "Right to left"
                    { value: 1, label: qsTrId("light_kb_dir_rtl") }
                ]
                onActivated: (v) => {
                    tab.direction = v
                    Api.write("kb.effect", "keyboard", "effect", {
                        name: tab.effectId, color: tab.colour, speed: tab.speed, brightness: tab.brightness, direction: v })
                }
            }
        }
        HelpText {
            visible: tab.kind === "perkey"
            //% "The keyboard shows your own key colours. Pick an effect to replace them."
            text: qsTrId("light_kb_perkey_note")
        }
    }

    Card {
        Layout.fillWidth: true
        enabled: tab.usable
        //% "Follow and idle"
        title: qsTrId("light_kb_follow")
        icon: "power"

        ToggleRow {
            Layout.fillWidth: true
            //% "Colour follows the power mode"
            label: qsTrId("light_kb_operating")
            //% "Eco green, Quiet blue, Balanced cyan, Performance orange, Turbo red. Off leaves the colours you picked."
            sub: qsTrId("light_kb_operating_sub")
            checked: tab.follow
            onToggled: (on) => Api.call("keyboard", "operating", { action: on ? "on" : "off" }, function (v) {
                if (typeof v === "boolean") tab.follow = v
            }, function (err) {
                tab.follow = !on
                Toasts.show(err && err.message ? err.message : "", "error")
            })
        }
        ToggleRow {
            Layout.fillWidth: true
            //% "Turn the light off after 30 seconds without input"
            label: qsTrId("light_kb_idle")
            checked: tab.idleOff
            onToggled: (on) => Api.call("keyboard", "timeout", { action: on ? "on" : "off" }, function (v) {
                if (typeof v === "boolean") tab.idleOff = v
            }, function (err) {
                tab.idleOff = !on
                Toasts.show(err && err.message ? err.message : "", "error")
            })
        }
        SettingRow {
            Layout.fillWidth: true
            //% "Default"
            label: qsTrId("light_kb_default")
            SecondaryButton {
                //% "Reset to the stock look"
                text: qsTrId("light_kb_reset")
                onClicked: Api.call("keyboard", "restore", {}, function () {
                    //% "Keyboard back to the stock look"
                    Toasts.show(qsTrId("light_kb_reset_done"), "ok")
                    tab.refresh()
                })
            }
        }
    }

    BootLookCard {
        Layout.fillWidth: true
    }

    ColorPickerPopup {
        id: picker
        parent: Overlay.overlay
        anchors.centerIn: Overlay.overlay
        onPicked: (hex) => tab.setColour(hex)
    }
}
