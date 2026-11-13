// Lighting > Rear bar: the light bar on the back of the lid. Static colours three parts (left,
// middle, right); the other effects use one colour. Speed and direction show only for the effects
// that have them. Brightness 0 is "off" for the bar, so the
// switch is the way to turn it off and an effect picked while it is off turns it back on.
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import org.truesense.app

ColumnLayout {
    id: tab
    objectName: "rearTab"

    spacing: Theme.spacing + 2

    property string effectId: "static"
    property string colour: "00aec7"
    property var zones: ["00aec7", "00aec7", "00aec7"]
    property int speed: 3
    property int brightness: 100
    property int direction: 1
    property int lastBrightness: 100
    property bool lightOn: true
    property int zonePicking: -1

    readonly property var effects: LightingVm.call("rear_effects") || []
    readonly property var rule: {
        for (var i = 0; i < effects.length; ++i)
            if (effects[i].id === effectId) return effects[i]
        return null
    }
    readonly property bool usable: Caps.usable("rear-light") && Caps.usable("rear_light")

    function refresh() {
        Api.call("rear", "status", {}, function (r) {
            lightOn = r.brightness > 0
            if (r.brightness) lastBrightness = r.brightness
            brightness = r.brightness
            if (r.speed) speed = r.speed
            effectId = r.mode || effectId
            if (r.color) colour = r.color
            direction = r.direction === 0 ? 0 : 1
            var z = Array.isArray(r.zones) && r.zones.length === 3 ? r.zones : [colour, colour, colour]
            zones = z.map(function (c, i) { return c || colour })
        }, function () {})
    }

    /// Sends the rear light as the controls show it. `final` asks the service to read it back.
    function apply(final) {
        var r = LightingVm.call("rear_request", {
            effect: effectId, colour: colour, zones: zones, speed: speed, brightness: brightness,
            direction: direction, last_brightness: lastBrightness })
        if (r === undefined) return
        brightness = r.brightness
        lastBrightness = r.brightness
        lightOn = true
        var args = r.args
        args.verify = final === true
        Api.write("rear", "rear", "set", args)
    }

    function setColour(hex) {
        if (zonePicking >= 0) {
            var z = zones.slice()
            z[zonePicking] = hex
            zones = z
        } else {
            colour = hex
        }
        apply(false)
    }

    Component.onCompleted: refresh()

    CapabilityNote {
        capability: "rear-light"
        //% "This laptop has no rear light bar."
        text: qsTrId("light_rear_none")
    }
    // The same fact as `capabilities get` states it (its ids use underscores).
    CapabilityNote { capability: "rear_light" }

    Card {
        Layout.fillWidth: true
        enabled: tab.usable
        //% "Rear light bar"
        title: qsTrId("light_rear_title")
        icon: "bar"

        // The bar as it is set now, drawn after the laptop's rear. In the static effect a click on a
        // part picks its colour.
        Rectangle {
            Layout.fillWidth: true
            implicitHeight: bar.height + 2 * Theme.pad
            radius: Theme.radius
            color: "#0b0d10"
            border.width: 1
            border.color: Theme.cardLine
            RearBarPicture {
                id: bar
                y: Theme.pad
                width: Math.min(parent.width - 2 * Theme.pad, 900)
                anchors.horizontalCenter: parent.horizontalCenter
                height: Math.round(width * 0.2)
                effect: tab.effectId
                colour: tab.colour
                zones: tab.zones
                brightness: tab.brightness
                on: tab.lightOn
                direction: tab.direction
                partsClickable: tab.usable && tab.effectId === "static"
                //% "Picture of the rear light bar as it is set now"
                Accessible.name: qsTrId("light_rear_picture_a11y")
                onPartClicked: (part) => {
                    tab.zonePicking = part
                    picker.start(tab.zones[part])
                    picker.open()
                }
            }
        }
        HelpText {
            visible: tab.effectId === "static" && tab.lightOn
            //% "Click a part of the bar to change its colour."
            text: qsTrId("light_rear_picture_help")
        }
        HelpText {
            visible: tab.effectId !== "static" && tab.lightOn
            //% "A still frame of the effect: the bar itself moves."
            text: qsTrId("light_rear_picture_moves")
        }

        ToggleRow {
            Layout.fillWidth: true
            //% "Light"
            label: qsTrId("light_rear_on")
            checked: tab.lightOn
            onToggled: (on) => Api.call("rear", on ? "on" : "off", {}, function () { tab.refresh() })
        }
        TileRow {
            Layout.fillWidth: true
            variant: "outlined"
            tileHeight: 68
            minTileWidth: 96
            current: tab.effectId
            model: tab.effects.map(function (e) { return { value: e.id, label: LightingVm.effectName(e.id), icon: e.icon } })
            onActivated: (id) => { tab.effectId = id; tab.apply(true) }
        }

        // Static: three parts, each its own colour.
        SettingRow {
            Layout.fillWidth: true
            visible: tab.effectId === "static"
            //% "Colour of each part"
            label: qsTrId("light_rear_zones")
            Repeater {
                model: 3
                delegate: ColumnLayout {
                    id: zone
                    required property int index
                    readonly property string zoneName: index === 0
                        //% "Left"
                        ? qsTrId("light_rear_left") : (index === 1
                        //% "Middle"
                        ? qsTrId("light_rear_middle")
                        //% "Right"
                        : qsTrId("light_rear_right"))
                    spacing: 2
                    Button {
                        Layout.alignment: Qt.AlignHCenter
                        implicitWidth: 40
                        implicitHeight: 40
                        padding: 0
                        focusPolicy: Qt.StrongFocus
                        Accessible.name: zone.zoneName
                        onClicked: {
                            tab.zonePicking = zone.index
                            picker.start(tab.zones[zone.index])
                            picker.open()
                        }
                        background: Rectangle {
                            radius: 10
                            color: LightingVm.css(tab.zones[zone.index])
                            border.width: parent.activeFocus ? 3 : 1
                            border.color: parent.activeFocus ? Theme.focusRing : Theme.swatchLine
                        }
                    }
                    Text {
                        Layout.alignment: Qt.AlignHCenter
                        text: zone.zoneName
                        color: Theme.muted
                        font.pixelSize: Theme.fontSmall
                    }
                }
            }
        }
        Text {
            Layout.fillWidth: true
            Layout.topMargin: 4
            visible: tab.effectId !== "static" && tab.effectId !== "neon"
            //% "Colour"
            text: qsTrId("light_rear_colour")
            color: Theme.text
            font.pixelSize: Theme.fontBody
        }
        SwatchRow {
            Layout.fillWidth: true
            visible: tab.effectId !== "static" && tab.effectId !== "neon"
            colors: LightingVm.call("palette").map(LightingVm.css)
            current: LightingVm.css(tab.colour)
            allowCustom: true
            onActivated: (c) => { tab.zonePicking = -1; tab.setColour(c.substring(1)) }
            onCustomRequested: { tab.zonePicking = -1; picker.start(tab.colour); picker.open() }
        }
        // Speed and direction show only for the effects that have them.
        SliderRow {
            Layout.fillWidth: true
            visible: tab.rule !== null && tab.rule.speed
            //% "Speed"
            label: qsTrId("light_rear_speed")
            from: 1
            to: 5
            stepSize: 1
            value: tab.speed
            coalesceMs: 120
            onEdited: (v) => { tab.speed = v; tab.apply(false) }
            onReleased: (v) => { tab.speed = v; tab.apply(true) }
        }
        SliderRow {
            Layout.fillWidth: true
            //% "Brightness"
            label: qsTrId("light_rear_brightness")
            from: 5
            to: 100
            stepSize: 5
            unit: "%"
            value: Math.max(5, tab.brightness)
            coalesceMs: 120
            onEdited: (v) => { tab.brightness = v; tab.apply(false) }
            onReleased: (v) => { tab.brightness = v; tab.apply(true) }
        }
        SettingRow {
            Layout.fillWidth: true
            visible: tab.rule !== null && tab.rule.direction
            //% "Direction"
            label: qsTrId("light_rear_direction")
            Segmented {
                current: tab.direction
                model: [
                    //% "Left to right"
                    { value: 0, label: qsTrId("light_rear_dir_ltr") },
                    //% "Right to left"
                    { value: 1, label: qsTrId("light_rear_dir_rtl") }
                ]
                onActivated: (v) => { tab.direction = v; tab.apply(true) }
            }
        }
        SettingRow {
            Layout.fillWidth: true
            //% "Default"
            label: qsTrId("light_rear_default")
            SecondaryButton {
                //% "Reset to the stock look"
                text: qsTrId("light_rear_reset")
                onClicked: Api.call("rear", "set", { effect: "breathing", color: "3723c7", speed: 3, brightness: 100, direction: 1 }, function () {
                    //% "Rear light back to the stock look"
                    Toasts.show(qsTrId("light_rear_reset_done"), "ok")
                    tab.refresh()
                })
            }
        }
    }

    ColorPickerPopup {
        id: picker
        parent: Overlay.overlay
        anchors.centerIn: Overlay.overlay
        onPicked: (hex) => tab.setColour(hex)
    }
}
