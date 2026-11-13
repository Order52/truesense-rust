// One short message at a time, at the top right of the window. Errors from commands arrive here with
// the text exactly as the command gave it (COMMANDS.md: it is written for a person). ToastHost draws
// it. A new message replaces the one showing, so messages never pile up over the page.
pragma Singleton
import QtQuick

QtObject {
    id: toasts

    /// [{id, text, kind}]: the message showing, or none.
    property var items: []
    property int nextId: 1

    /// kind: "info", "ok" or "error". Errors stay longer.
    function show(text, kind) {
        var k = kind === undefined ? "info" : kind
        items = [{ id: nextId++, text: text, kind: k }]
    }

    function error(text) { show(text, "error") }
    function ok(text) { show(text, "ok") }

    function dismiss(id) {
        items = items.filter(function (t) { return t.id !== id })
    }
}
