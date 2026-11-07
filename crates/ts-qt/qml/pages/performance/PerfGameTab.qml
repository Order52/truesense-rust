// Tab "Game rules": programs that raise the power mode while they run. The daemon answers which MODE
// the strongest running rule asks for, not which program, so the list marks "Running now" only when
// that is certain and "May be running" when several rules share the mode (performance.rs, rules_model).
// The list refreshes every few live ticks, which only come while the window is visible.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

ColumnLayout {
    id: root

    required property var host

    spacing: Theme.spacing + 4

    property var answer: ({ rules: {}, running: null })
    property bool loaded: false
    property string failure: ""
    property string newMode: "performance"
    property int ticks: 0

    readonly property var model: PerfVm.val("rules_model", answer, { rows: [], count: 0, running: null, full: false })
    readonly property var check: PerfVm.val("rule_check", { program: program.text, mode: newMode, rules: answer.rules }, { ok: false })

    function take(r) {
        answer = r
        loaded = true
        failure = ""
    }

    function refresh() {
        Api.call("power", "game", { action: "list" }, take, function (err) {
            failure = err && err.message ? err.message : ""
        })
    }

    function problemText(code) {
        switch (code) {
        //% "Type the program's name first."
        case "empty": return qsTrId("perf_rule_empty")
        //% "Program names use letters, digits, . _ + - and spaces (up to 80). A path is not a name."
        case "chars": return qsTrId("perf_rule_chars")
        //% "That name is too long (up to 80 characters)."
        case "too_long": return qsTrId("perf_rule_too_long")
        //% "Pick a power mode."
        case "mode": return qsTrId("perf_rule_mode")
        //% "There can be at most 64 rules. Remove one first."
        case "full": return qsTrId("perf_rule_full")
        default: return ""
        }
    }

    function add() {
        var c = check
        if (!c.ok) { Toasts.error(problemText(c.problem)); return }
        Api.call("power", "game", { action: "add", name: c.program, mode: c.mode }, function (r) {
            take(r)
            program.text = ""
        })
    }

    function remove(name) {
        Api.call("power", "game", { action: "remove", name: name }, take)
    }

    Component.onCompleted: refresh()

    // The daemon looks at the running programs every few seconds: ask about as often.
    Connections {
        target: Live
        function onSerialChanged() {
            root.ticks += 1
            if (root.ticks % 3 === 0) root.refresh()
        }
    }

    ConfirmDialog { id: confirm }

    Card {
        Layout.fillWidth: true
        //% "Game rules"
        title: qsTrId("perf_rules_title")
        icon: "performance"
        rightText: root.loaded
            //% "%1 rules"
            ? qsTrId("perf_rules_count").arg(root.model.count)
            : ""

        HelpText {
            //% "While one of these programs runs, the power mode is at least the one you pick (Native and Smart auto). When it closes, automatic control takes over again. Turbo still backs off if the laptop gets too hot. A name matches the program's name, its file name, or any .exe on a Wine or Proton command line; capitals do not matter."
            text: qsTrId("perf_rules_help")
        }

        RowLayout {
            Layout.fillWidth: true
            visible: root.loaded
            spacing: 10
            Badge {
                text: root.model.running
                    //% "A rule is active: %1"
                    ? qsTrId("perf_rules_active").arg(PerfVm.modeText(root.model.running))
                    //% "No game rule is active"
                    : qsTrId("perf_rules_none_active")
                kind: root.model.running ? "accent" : "neutral"
            }
        }

        CapabilityNote {
            capability: "daemon"
            //% "Game rules need the TrueSense service."
            text: qsTrId("perf_rules_needs_service")
        }
        HelpText {
            visible: root.failure !== "" && Caps.state("daemon") !== "needs-daemon"
            text: root.failure
            color: Theme.warn
        }
    }

    Card {
        Layout.fillWidth: true
        //% "Add a rule"
        title: qsTrId("perf_rules_add_title")
        icon: "plus"

        TextFieldRow {
            id: program
            Layout.fillWidth: true
            //% "Program name"
            label: qsTrId("perf_rules_program")
            //% "e.g. eldenring.exe or hl2_linux"
            placeholder: qsTrId("perf_rules_program_placeholder")
            maxLength: 80
            fieldWidth: 260
            enabled: Caps.usable("daemon")
            onAccepted: root.add()
        }
        SelectRow {
            Layout.fillWidth: true
            //% "Power mode while it runs"
            label: qsTrId("perf_rules_mode")
            options: PerfVm.modeOptions()
            current: root.newMode
            enabled: Caps.usable("daemon")
            onActivated: (v) => root.newMode = v
            AppButton {
                kind: "primary"
                iconName: "plus"
                //% "Add"
                text: qsTrId("perf_rules_add")
                enabled: Caps.usable("daemon") && program.text.trim() !== ""
                onClicked: root.add()
            }
        }
        HelpText {
            visible: root.check.replaces === true && program.text.trim() !== ""
            //% "This program already has a rule; adding it replaces that rule."
            text: qsTrId("perf_rules_replaces")
        }
    }

    Card {
        Layout.fillWidth: true
        //% "Your rules"
        title: qsTrId("perf_rules_list_title")
        icon: "profiles"

        HelpText {
            visible: root.loaded && root.model.count === 0
            //% "No game rules yet. Add the program of a game above and Turbo or Performance follows it."
            text: qsTrId("perf_rules_empty")
        }

        Repeater {
            model: root.model.rows
            delegate: Rectangle {
                id: row
                required property var modelData
                Layout.fillWidth: true
                implicitHeight: line.implicitHeight + 16
                radius: Theme.radiusSmall + 1
                color: modelData.state === "running" ? Theme.accentSoft : Theme.tile
                border.width: 1
                border.color: modelData.state === "running" ? Theme.accentLine : Theme.tileLine

                RowLayout {
                    id: line
                    anchors.fill: parent
                    anchors.margins: 8
                    anchors.leftMargin: 12
                    spacing: 10
                    Text {
                        Layout.fillWidth: true
                        text: row.modelData.program
                        color: Theme.text
                        font.pixelSize: Theme.fontBody
                        elide: Text.ElideRight
                    }
                    Badge {
                        visible: row.modelData.state !== "idle"
                        text: row.modelData.state === "running"
                            //% "Running now"
                            ? qsTrId("perf_rules_running")
                            //% "May be running"
                            : qsTrId("perf_rules_maybe")
                        kind: row.modelData.state === "running" ? "accent" : "neutral"
                    }
                    Badge { text: PerfVm.modeText(row.modelData.mode) }
                    AppButton {
                        iconName: "trash"
                        //% "Remove"
                        text: qsTrId("perf_rules_remove")
                        enabled: Caps.usable("daemon")
                        Accessible.name: qsTrId("perf_rules_remove") + " " + row.modelData.program
                        onClicked: root.remove(row.modelData.program)
                    }
                }
            }
        }

        AppButton {
            visible: root.model.count > 1
            kind: "danger"
            iconName: "trash"
            //% "Remove all rules"
            text: qsTrId("perf_rules_clear")
            enabled: Caps.usable("daemon")
            onClicked: confirm.ask({
                //% "Remove every game rule?"
                title: qsTrId("perf_rules_clear_title"),
                //% "All %1 rules are deleted. Games no longer raise the power mode."
                text: qsTrId("perf_rules_clear_text").arg(root.model.count),
                //% "Remove all"
                confirmText: qsTrId("perf_rules_clear_confirm"),
                danger: true
            }, function () { Api.call("power", "game", { action: "clear" }, root.take) })
        }
    }
}
