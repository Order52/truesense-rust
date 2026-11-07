// The risk of a value as one line: an icon, the level in words and the explanation. Used where a
// number can be set past a safety line (power limits, undervolt): the colour is a hint, the word
// ("Safe", "Caution", "Danger") and the icon say it too.
//
//   RiskLine { level: "caution"; why: "Past the safety line (core 60 mV / cache 50 mV)." }
import QtQuick
import QtQuick.Layouts
import org.truesense.app

RowLayout {
    id: root

    /// "safe", "caution" or "danger"; anything else is shown as no risk line.
    property string level: ""
    /// The explanation (the daemon's own sentence).
    property string why: ""

    readonly property color _ink: level === "danger" ? Theme.bad : level === "caution" ? Theme.warn : Theme.ok

    visible: level === "safe" || level === "caution" || level === "danger"
    spacing: 8
    Layout.fillWidth: true
    Accessible.role: Accessible.StaticText
    Accessible.name: word.text + ". " + root.why

    Icon {
        Layout.alignment: Qt.AlignTop
        name: root.level === "safe" ? "check" : "warn"
        size: 16
        color: root._ink
    }
    Text {
        id: word
        Layout.alignment: Qt.AlignTop
        text: root.level === "danger"
            //% "Danger"
            ? qsTrId("risk_danger")
            : root.level === "caution"
            //% "Caution"
            ? qsTrId("risk_caution")
            //% "Safe"
            : qsTrId("risk_safe")
        color: root._ink
        font.pixelSize: Theme.fontSmall + 1
        font.bold: true
    }
    Text {
        Layout.fillWidth: true
        text: root.why
        color: Theme.stat
        font.pixelSize: Theme.fontSmall + 1
        wrapMode: Text.Wrap
    }
}
