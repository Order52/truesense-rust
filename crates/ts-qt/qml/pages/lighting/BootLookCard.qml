// The look the service puts on the lights when the laptop starts (and after every resume), before
// anyone logs in. `startup boot` reads it; `current` stores the lights as they are, a look name
// stores that look, `clear` leaves the lights alone.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

Card {
    id: root

    /// Offer a list of looks to choose from (the Looks tab); otherwise only "use what is on now".
    property bool showPicker: false
    property string text: ""
    property string picked: "__current__"

    //% "At boot"
    title: qsTrId("light_boot_title")
    icon: "power"

    function reload() {
        Api.call("startup", "boot", {}, function (look) {
            text = look
                //% "At boot: %1"
                ? qsTrId("light_boot_is").arg(LightingVm.describeLook(look))
                //% "At boot: the lights are left alone"
                : qsTrId("light_boot_none")
        }, function (err) {
            //% "At boot: unknown (%1)"
            text = qsTrId("light_boot_unknown").arg(err && err.message ? err.message : "")
        })
    }

    function _set(what) {
        Api.call("startup", "boot", { what: what }, function () { root.reload() })
    }

    Component.onCompleted: reload()

    // What happens at boot now, first: the buttons below change it.
    Text {
        Layout.fillWidth: true
        text: root.text
        color: Theme.text
        font.pixelSize: Theme.fontBody
        wrapMode: Text.Wrap
        Accessible.role: Accessible.StaticText
    }
    HelpText {
        //% "Applied by the TrueSense service when the laptop starts, before you log in, and again after every resume from suspend."
        text: qsTrId("light_boot_help")
    }
    SelectRow {
        Layout.fillWidth: true
        visible: root.showPicker
        //% "Look at boot"
        label: qsTrId("light_boot_look")
        options: {
            //% "What is on the lights now"
            var out = [{ value: "__current__", text: qsTrId("light_boot_current") }]
            for (var i = 0; i < LightingVm.looks.length; ++i)
                out.push({ value: LightingVm.looks[i], text: LightingVm.lookName(LightingVm.looks[i]) })
            return out
        }
        current: root.picked
        onActivated: (v) => root.picked = v
    }
    Flow {
        Layout.fillWidth: true
        spacing: 8
        PrimaryButton {
            text: root.showPicker
                //% "Use at boot"
                ? qsTrId("light_boot_use")
                //% "Use these lights at boot"
                : qsTrId("light_boot_use_now")
            onClicked: root._set(root.showPicker && root.picked !== "__current__" ? root.picked : "current")
        }
        SecondaryButton {
            //% "Leave the lights alone at boot"
            text: qsTrId("light_boot_clear")
            onClicked: root._set("clear")
        }
    }
}
