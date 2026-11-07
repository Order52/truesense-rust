import QtQuick
import QtTest
import org.truesense.app

TestCase {
    name: "Dialog and capability"
    when: windowShown
    visible: true
    width: 600
    height: 400

    ConfirmDialog { id: dialog }
    SignalSpy { id: accepted; target: dialog; signalName: "accepted" }
    SignalSpy { id: rejected; target: dialog; signalName: "rejected" }

    CapabilityNote { id: note; capability: "rear-light"; text: "This laptop has no rear light." }

    property int callbacks: 0

    function init() { accepted.clear(); rejected.clear(); callbacks = 0; Caps.model = ({}) }

    function test_ask_opens_and_confirm_runs_the_callback() {
        dialog.ask({ title: "Sure?", text: "x", confirmText: "Do it" }, function () { callbacks++ })
        tryVerify(function () { return dialog.opened })
        compare(dialog.title, "Sure?")
        dialog.confirmButton.clicked()
        compare(callbacks, 1)
        compare(accepted.count, 1)
        compare(rejected.count, 0)
    }

    function test_escape_rejects_and_runs_nothing() {
        dialog.ask({ title: "Sure?" }, function () { callbacks++ })
        tryVerify(function () { return dialog.opened })
        keyClick(Qt.Key_Escape)
        tryVerify(function () { return !dialog.opened })
        compare(callbacks, 0)
        compare(rejected.count, 1)
    }

    function test_type_word_gates_the_confirm_button() {
        dialog.ask({ title: "Danger", danger: true, typeWord: "APPLY" }, function () { callbacks++ })
        tryVerify(function () { return dialog.opened })
        compare(dialog.confirmEnabled, false)
        dialog.close()
    }

    function test_capability_note_is_hidden_while_usable() {
        compare(note.visible, false)
        Caps.model = { "rear-light": { state: "unavailable" } }
        compare(note.visible, true)
        compare(note.capState, "unavailable")
        Caps.model = { "rear-light": { state: "needs-daemon" } }
        compare(note.visible, true)
        Caps.model = { "rear-light": { state: "available" } }
        compare(note.visible, false)
        note.stateOverride = "unavailable"
        compare(note.visible, true)
        note.stateOverride = ""
    }
}
