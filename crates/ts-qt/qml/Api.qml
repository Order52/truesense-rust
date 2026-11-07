// Calling commands from QML. One function per kind of action, callbacks instead of promises:
//
//   Api.call("power", "set", {mode: "turbo"}, function (result) { ... })         // one-shot action
//   Api.write("fan.cpu", "fan", "set", {cpu: v})                                  // slider: newest wins
//   Api.call("fan", "off", {confirm: true}, ok, fail)                             // fail(err) replaces the toast
//
// A failed command shows its message in a toast, verbatim (COMMANDS.md: written for a person),
// unless a `fail` callback is given or the error is a superseded write. The error object is
// {message, code}; code "needs-daemon" and "no-permission" are explained by needsDaemon() so a
// page can show its own hint. The transport (the Rust Bus in the app, a fake in tests) is set by
// Main.qml and must offer run(area, verb, argsJson), write(key, area, verb, argsJson) and the signal
// commandFinished(id, ok, json).
pragma Singleton
import QtQuick

QtObject {
    id: api

    /// The object that executes commands; see above.
    property var transport: null
    /// Calls that have not answered yet.
    property int pending: 0

    property var _callbacks: ({})
    property var _conn: Connections {
        target: api.transport
        ignoreUnknownSignals: true
        function onCommandFinished(id, ok, json) { api._finished(id, ok, json) }
    }

    function _register(id, ok, fail) {
        _callbacks[id] = { ok: ok, fail: fail }
        pending++
    }

    function _finished(id, ok, json) {
        var cb = _callbacks[id]
        if (cb === undefined) return
        delete _callbacks[id]
        pending = Math.max(0, pending - 1)
        var body
        try { body = JSON.parse(json) } catch (e) { body = ok ? null : { message: json } }
        if (ok) {
            if (cb.ok) cb.ok(body)
            return
        }
        if (body && body.code === "superseded") return
        if (cb.fail) cb.fail(body)
        //% "The command failed."
        else Toasts.error(body && body.message ? body.message : qsTrId("api_error_unknown")
        )
    }

    /// Runs a command. `args` is an object (or undefined); `ok(result)` gets the decoded result.
    function call(area, verb, args, ok, fail) {
        if (transport === null) { if (fail) fail({ message: "no command bus", code: "internal" }); return -1 }
        var id = transport.run(area, verb, JSON.stringify(args === undefined ? {} : args))
        _register(id, ok, fail)
        return id
    }

    /// A write for the control `key`: while one runs, a newer write for the same key replaces any
    /// queued one, so a dragged slider ends on its last value and never works through a backlog.
    function write(key, area, verb, args, ok, fail) {
        if (transport === null) { if (fail) fail({ message: "no command bus", code: "internal" }); return -1 }
        var id = transport.write(key, area, verb, JSON.stringify(args === undefined ? {} : args))
        _register(id, ok, fail)
        return id
    }

    /// Whether an error means the root service is not running (the page may offer to start it).
    function needsDaemon(err) { return err !== null && err !== undefined && err.code === "needs-daemon" }
}
