// The busiest processes: name, who runs it and what it does, a bar and the number. `rows` is the
// process model of ts-ui-model (vm monitor: cpu / memory): [{pid, name, cmdline, user, state_id,
// threads, cpu_text, mem_text, value}], biggest first; `mode` picks the number that is shown ("cpu"
// or "mem"). Nothing here sorts or formats.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

ColumnLayout {
    id: root

    property var rows: []
    property string mode: "cpu"
    /// Said instead of the rows (the first scan has no percentages yet).
    property string emptyText: ""
    readonly property bool narrow: width < 520

    spacing: 6

    function stateLabel(id) {
        switch (id) {
        //% "running"
        case "running": return qsTrId("monitor_state_running")
        //% "sleeping"
        case "sleeping": return qsTrId("monitor_state_sleeping")
        //% "waiting for disk"
        case "waiting": return qsTrId("monitor_state_waiting")
        //% "zombie"
        case "zombie": return qsTrId("monitor_state_zombie")
        //% "stopped"
        case "stopped": return qsTrId("monitor_state_stopped")
        //% "idle"
        case "idle": return qsTrId("monitor_state_idle")
        default: return ""
        }
    }

    function detailLine(p) {
        var parts = []
        if (p.user) parts.push(p.user)
        var s = stateLabel(p.state_id)
        if (s !== "") parts.push(s)
        if (p.pid !== null && p.pid !== undefined) parts.push("PID " + p.pid)
        //% "threads"
        var threadsWord = qsTrId("monitor_threads")
        if (!narrow && p.threads !== null && p.threads !== undefined) parts.push(p.threads + " " + threadsWord)
        return parts.join(" · ")
    }

    Text {
        Layout.fillWidth: true
        visible: root.rows.length === 0 && root.emptyText !== ""
        text: root.emptyText
        color: Theme.muted
        font.pixelSize: Theme.fontSmall + 1
        wrapMode: Text.Wrap
    }

    Repeater {
        model: root.rows
        delegate: RowLayout {
            id: row
            required property var modelData
            Layout.fillWidth: true
            spacing: 10
            Accessible.role: Accessible.ListItem
            Accessible.name: modelData.name
            Accessible.description: root.mode === "mem" ? modelData.mem_text : (modelData.cpu_text + ", " + modelData.mem_text)

            ColumnLayout {
                Layout.fillWidth: true
                Layout.preferredWidth: 1
                spacing: 0
                Text {
                    Layout.fillWidth: true
                    text: row.modelData.name
                    color: Theme.text
                    font.pixelSize: Theme.fontBody
                    elide: Text.ElideRight
                    textFormat: Text.PlainText
                }
                Text {
                    Layout.fillWidth: true
                    text: root.detailLine(row.modelData)
                    color: Theme.muted
                    font.pixelSize: Theme.fontSmall - 1
                    elide: Text.ElideRight
                    textFormat: Text.PlainText
                }
            }
            Rectangle {
                visible: !root.narrow
                Layout.preferredWidth: 90
                Layout.preferredHeight: 6
                radius: 3
                color: Theme.groove
                Rectangle {
                    width: parent.width * (row.modelData.value === null ? 0 : row.modelData.value)
                    height: parent.height
                    radius: 3
                    color: Theme.accent
                    visible: width > 0
                }
            }
            ColumnLayout {
                // A nested layout fills by default when a child does: only the name column fills.
                Layout.fillWidth: false
                Layout.preferredWidth: 72
                spacing: 0
                Text {
                    Layout.fillWidth: true
                    text: root.mode === "mem" ? row.modelData.mem_text : row.modelData.cpu_text
                    color: Theme.text
                    font.pixelSize: Theme.fontBody
                    font.bold: true
                    horizontalAlignment: Text.AlignRight
                    LayoutMirroring.enabled: false
                }
                Text {
                    Layout.fillWidth: true
                    visible: root.mode !== "mem"
                    text: row.modelData.mem_text
                    color: Theme.muted
                    font.pixelSize: Theme.fontSmall - 1
                    horizontalAlignment: Text.AlignRight
                    LayoutMirroring.enabled: false
                }
            }
        }
    }
}
