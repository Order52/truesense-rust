// A switch whose result is shown: below it the answer the hardware gave after the person flipped
// it ("Read back from the firmware: on"), or the refusal text. Like ToggleRow, `toggled(on)` fires
// only for the person's own input; the page sends the command and then sets `checked` to what the
// machine said and `note`/`noteKind` to the sentence (see BatteryPage for the pattern).
//
//   ReadbackToggle { label: "..."; busy: false; note: "..."; noteKind: "ok"; onToggled: (on) => ... }
import QtQuick
import QtQuick.Layouts
import org.truesense.app

ColumnLayout {
    id: root

    property alias label: row.label
    property alias sub: row.sub
    property alias checked: row.checked
    /// A command is on its way: the switch is off-limits meanwhile (no double send).
    property bool busy: false
    property string note: ""
    property string noteKind: "ok"
    signal toggled(bool checked)

    Layout.fillWidth: true
    spacing: 4

    ToggleRow {
        id: row
        Layout.fillWidth: true
        enabled: root.enabled && !root.busy
        onToggled: (on) => root.toggled(on)
    }
    ReadbackNote {
        text: root.note
        kind: root.noteKind
    }
}
