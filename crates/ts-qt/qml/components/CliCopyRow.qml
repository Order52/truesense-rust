// "Copy as TrueSense CLI": the one terminal command that puts what the card shows on the laptop,
// shown in a code line and copied with one click. The command is composed in Rust (the Lighting
// view-model's `cli` verb) and the ts-cli tests run every kind of it through the real command line.
//
//   CliCopyRow { args: ({ what: "keyboard", effect: "wave", speed: 5, brightness: 40 }) }
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import org.truesense.app

ColumnLayout {
    id: root

    /// The arguments of the `cli` verb (what, and the setting); null hides the row.
    property var args: null
    readonly property var command: args === null ? null : (LightingVm.call("cli", args) || null)
    readonly property string text: command === null ? "" : command.text

    visible: text !== ""
    spacing: 6
    Layout.fillWidth: true

    RowLayout {
        Layout.fillWidth: true
        spacing: 8
        Rectangle {
            Layout.fillWidth: true
            implicitHeight: Math.max(Theme.controlHeight, line.implicitHeight + 12)
            radius: Theme.radiusSmall
            color: Theme.field
            border.width: 1
            border.color: Theme.cardLine
            Text {
                id: line
                anchors.fill: parent
                anchors.leftMargin: 10
                anchors.rightMargin: 10
                verticalAlignment: Text.AlignVCenter
                // A long command (a per-key picture) shows its start; the button copies all of it.
                text: "$ " + root.text
                wrapMode: Text.WrapAtWordBoundaryOrAnywhere
                maximumLineCount: 2
                elide: Text.ElideRight
                color: Theme.text
                font.family: "monospace"
                font.pixelSize: Theme.fontSmall + 1
                Accessible.role: Accessible.StaticText
                //% "Terminal command: %1"
                Accessible.name: qsTrId("cli_copy_a11y").arg(root.text)
            }
        }
        SecondaryButton {
            //% "Copy as TrueSense CLI"
            text: qsTrId("cli_copy")
            iconName: "copy"
            onClicked: root.copy()
        }
    }
    HelpText {
        //% "Paste it in a terminal to apply exactly this with one command (in scripts, keybinds or autostart too)."
        text: qsTrId("cli_copy_help")
    }

    // The clipboard is reached through a text control (QML has no clipboard object of its own).
    TextEdit { id: clip; visible: false }

    function copy() {
        if (root.text === "") return
        clip.text = root.text
        clip.selectAll()
        clip.copy()
        //% "Copied: %1"
        Toasts.show(qsTrId("cli_copied").arg(root.text), "ok")
    }
}
