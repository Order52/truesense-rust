// The "not available on this model" state, as one sentence under the control it explains. It shows
// nothing while the capability is usable, so a page can always put it there:
//
//   CapabilityNote {
//       capability: "rear-light"
//       //% "This laptop has no rear light."
//       text: qsTrId("rear_note")
//   }
//   SliderRow { enabled: Caps.usable("rear-light"); ... }
//
// What it says: the page's `text` for a feature this machine lacks (or the machine's own reason
// when `capabilities get` supplied one), and a generic line when the feature needs the TrueSense
// service. `stateOverride` forces a state for tests and design.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

RowLayout {
    id: root

    /// A capability id of ts-ui-model (`rear-light`, `undervolt`, ...).
    property string capability: ""
    /// The page's sentence for the "lacks it" case.
    property string text: ""
    property string stateOverride: ""
    readonly property string capState: stateOverride !== "" ? stateOverride : Caps.state(capability)
    readonly property bool shown: capState === "unavailable" || capState === "needs-daemon"

    visible: shown
    spacing: 8
    Layout.fillWidth: true
    Accessible.role: Accessible.StaticText
    Accessible.name: label.text

    Icon {
        Layout.alignment: Qt.AlignTop
        name: root.capState === "needs-daemon" ? "lock" : "info"
        size: 16
        color: Theme.muted
    }
    Text {
        id: label
        Layout.fillWidth: true
        text: root.capState === "needs-daemon"
            //% "This needs the TrueSense service, which is not running."
            ? qsTrId("cap_needs_daemon")
            : (root.text !== "" ? root.text : (Caps.why(root.capability) !== "" ? Caps.why(root.capability)
               //% "Not available on this model."
               : qsTrId("cap_unavailable")))
        color: Theme.muted
        font.pixelSize: Theme.fontSmall + 1
        wrapMode: Text.Wrap
        font.italic: true
    }
}
