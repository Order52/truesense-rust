// The "features supported on this machine" list: the model line and one row per feature with a
// tick or a cross and, for a missing one, the machine's own one-sentence reason. `info` is the
// output of the `settings` view-model `checklist` verb. With `detailed` each row also shows where
// the answer came from. Feature names are translated here by id; the English name the program sent
// is the fallback for a feature a newer build adds.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

ColumnLayout {
    id: root

    property var info: ({ available: false, items: [] })
    property bool detailed: false
    /// Show the model line and its tier above the list.
    property bool showModel: true
    /// Show the rows (false: only the model line, for a collapsed list).
    property bool showItems: true

    spacing: 8
    Layout.fillWidth: true

    readonly property var names: ({
        //% "Power modes"
        power_modes: qsTrId("cap_power_modes"),
        //% "Manual fan control"
        fan_control: qsTrId("cap_fan_control"),
        //% "Fan speed reading"
        fan_rpm: qsTrId("cap_fan_rpm"),
        //% "Display overdrive"
        overdrive: qsTrId("cap_overdrive"),
        //% "Battery 80 % limit"
        battery_limit: qsTrId("cap_battery_limit"),
        //% "Battery calibration"
        battery_calibration: qsTrId("cap_battery_calibration"),
        //% "USB charging while off"
        usb_charge: qsTrId("cap_usb_charge"),
        //% "GPU mode switch"
        gpu_mode: qsTrId("cap_gpu_mode"),
        //% "Rear light bar"
        rear_light: qsTrId("cap_rear_light"),
        //% "Keyboard backlight"
        keyboard_backlight: qsTrId("cap_keyboard_backlight"),
        //% "Per-key keyboard colours"
        per_key_rgb: qsTrId("cap_per_key_rgb"),
        //% "Start-up sound"
        boot_sound: qsTrId("cap_boot_sound"),
        //% "Keyboard backlight timeout"
        keyboard_timeout: qsTrId("cap_keyboard_timeout"),
        //% "Touchpad switch"
        touchpad: qsTrId("cap_touchpad"),
        //% "CPU undervolt"
        cpu_undervolt: qsTrId("cap_cpu_undervolt"),
        //% "GPU undervolt"
        gpu_undervolt: qsTrId("cap_gpu_undervolt"),
        //% "CPU power limits"
        power_limits: qsTrId("cap_power_limits"),
        //% "GPU power limit"
        gpu_power_limit: qsTrId("cap_gpu_power_limit"),
        //% "Predator key and Fn keys"
        fn_keys: qsTrId("cap_fn_keys"),
        //% "DTS sound"
        dts: qsTrId("cap_dts"),
        //% "Voice noise removal"
        rnnoise: qsTrId("cap_rnnoise"),
        //% "TrueSense service"
        daemon: qsTrId("cap_daemon")
    })

    function nameOf(item) { return names[item.id] !== undefined ? names[item.id] : item.label }

    readonly property string tierText: {
        var t = info.model ? info.model.tier : "unknown"
        if (t === "supported")
            //% "Verified on this model."
            return qsTrId("caps_tier_supported")
        if (t === "unverified")
            //% "A similar Acer laptop that nobody has verified: most things should work, some may not."
            return qsTrId("caps_tier_unverified")
        if (t === "unsupported")
            //% "Not an Acer Predator or Nitro: TrueSense may not work here."
            return qsTrId("caps_tier_unsupported")
        return ""
    }

    HelpText {
        visible: !root.info.available
        //% "The list of features could not be read."
        text: qsTrId("caps_unavailable")
    }
    ColumnLayout {
        visible: root.showModel && root.info.available === true
        Layout.fillWidth: true
        spacing: 2
        Text {
            Layout.fillWidth: true
            visible: text !== ""
            text: root.info.model ? root.info.model.title : ""
            color: Theme.text
            font.pixelSize: Theme.fontSection
            font.bold: true
            wrapMode: Text.Wrap
        }
        HelpText {
            visible: text !== ""
            text: root.tierText
        }
        HelpText {
            visible: root.info.simulated === true
            //% "This is the simulator, not a real laptop."
            text: qsTrId("caps_simulated")
        }
    }
    Repeater {
        model: root.showItems ? root.info.items : []
        delegate: ColumnLayout {
            id: line
            required property var modelData
            Layout.fillWidth: true
            spacing: 1
            RowLayout {
                Layout.fillWidth: true
                spacing: 8
                Icon {
                    name: line.modelData.supported ? "check" : "close"
                    size: 16
                    color: line.modelData.supported ? Theme.ok : Theme.muted
                }
                Text {
                    Layout.fillWidth: true
                    text: root.nameOf(line.modelData)
                    color: line.modelData.supported ? Theme.text : Theme.muted
                    font.pixelSize: Theme.fontBody
                    wrapMode: Text.Wrap
                    Accessible.name: text + ", " + (line.modelData.supported
                        //% "supported"
                        ? qsTrId("caps_supported")
                        //% "not available"
                        : qsTrId("caps_not_available"))
                }
                Text {
                    visible: root.detailed
                    text: line.modelData.source
                    color: Theme.muted
                    font.pixelSize: Theme.fontSmall
                }
            }
            HelpText {
                Layout.leftMargin: 24
                visible: text !== ""
                text: {
                    var parts = []
                    if (!line.modelData.supported && line.modelData.why) parts.push(line.modelData.why)
                    if (root.detailed && line.modelData.detail) parts.push(line.modelData.detail)
                    return parts.join(" · ")
                }
            }
        }
    }
}
