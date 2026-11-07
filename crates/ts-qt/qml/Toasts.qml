// Short messages at the bottom of the window. Errors from commands arrive here with the text
// exactly as the command gave it (COMMANDS.md: it is written for a person). ToastHost draws them.
pragma Singleton
import QtQuick

QtObject {
    id: toasts

    /// [{id, text, kind}], oldest first, at most 4.
    property var items: []
    property int nextId: 1

    /// kind: "info", "ok" or "error". Errors stay longer.
    function show(text, kind) {
        var k = kind === undefined ? "info" : kind
        var list = items.slice(-3)
        list.push({ id: nextId++, text: text, kind: k })
        items = list
    }

    function error(text) { show(text, "error") }
    function ok(text) { show(text, "ok") }

    function dismiss(id) {
        items = items.filter(function (t) { return t.id !== id })
    }
}
