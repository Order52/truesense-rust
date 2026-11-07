// One line under a control that says what the machine answered after the control was used: what
// was read back from the hardware, a refusal, or "no answer". The icon and the words carry the
// meaning, the colour only supports them.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

RowLayout {
    id: root

    property string text: ""
    /// "ok" (confirmed), "warn" (the machine kept something else) or "error" (refused).
    property string kind: "ok"

    visible: text !== ""
    Layout.fillWidth: true
    spacing: 6
    Accessible.role: Accessible.StaticText
    Accessible.name: root.text

    Icon {
        Layout.alignment: Qt.AlignTop
        name: root.kind === "ok" ? "check" : "warn"
        size: 14
        color: root.kind === "ok" ? Theme.ok : (root.kind === "warn" ? Theme.warn : Theme.bad)
    }
    Text {
        Layout.fillWidth: true
        text: root.text
        color: root.kind === "ok" ? Theme.muted : (root.kind === "warn" ? Theme.warn : Theme.bad)
        font.pixelSize: Theme.fontSmall + 1
        wrapMode: Text.Wrap
    }
}
