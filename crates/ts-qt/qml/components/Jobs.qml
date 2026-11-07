// Long commands (self-test, DTS set-up, sound repair, update) that must outlive the page that
// started them. Pages are destroyed when the person leaves them, but the command keeps running on
// its worker thread; its progress and its answer live here, so coming back shows the run (or its
// result) instead of a blank page, and a second start cannot be sent while the first is still going.
//
//   Jobs.start("selftest", "selftest", "run", { skip_fans: true })
//   Jobs.get("selftest")        // {running, startedMs, done, ok, result, error, cancelled}
//   Jobs.cancelWaiting("selftest")   // stop waiting; the hardware command itself is NOT interrupted
//
// Cancelling only ends the UI's waiting: the command is never aborted half way (the self-test
// restores what it changed by itself, and an installer must not be killed), so `busy(id)` stays true
// until the command has really answered and `start` refuses meanwhile. An answer that arrives after
// the person stopped waiting is dropped silently.
pragma Singleton
import QtQuick
import org.truesense.app

QtObject {
    id: jobs

    /// id -> job record; reassigned on every change so bindings update.
    property var records: ({})
    property var _inflight: ({})
    property int _serial: 0

    /// A job finished while somebody was waiting for it: `finished(id, ok, payload)` (the result, or
    /// the {message, code} error). Not emitted for an answer nobody waits for any more.
    signal finished(string id, bool ok, var payload)

    readonly property var idle: ({ running: false, startedMs: 0, done: false, ok: false, result: null,
                                   error: null, cancelled: false, token: 0 })

    /// The record of a job (a quiet one when it never ran).
    function get(id) {
        var r = records[id]
        return r === undefined ? idle : r
    }

    /// Whether the command is still executing, even if the person stopped waiting for it.
    function busy(id) { return _inflight[id] === true }

    function _set(id, record) {
        var copy = {}
        for (var k in records) copy[k] = records[k]
        copy[id] = record
        records = copy
    }

    /// Starts `area verb args` as job `id`. Returns false (and does nothing) while the previous run
    /// of the same job has not answered yet.
    function start(id, area, verb, args) {
        if (busy(id)) return false
        var token = ++_serial
        var inflight = _inflight
        inflight[id] = true
        _inflight = inflight
        _set(id, { running: true, startedMs: Date.now(), done: false, ok: false, result: null,
                   error: null, cancelled: false, token: token })
        var sent = Api.call(area, verb, args,
            function (result) { _answered(id, token, true, result) },
            function (err) { _answered(id, token, false, err) })
        if (sent < 0) _answered(id, token, false, { message: "no command bus", code: "internal" })
        return true
    }

    function _answered(id, token, ok, payload) {
        var inflight = _inflight
        delete inflight[id]
        _inflight = inflight
        var cur = get(id)
        if (cur.token !== token || cur.cancelled) return
        _set(id, { running: false, startedMs: cur.startedMs, done: true, ok: ok,
                   result: ok ? payload : null, error: ok ? null : payload, cancelled: false, token: token })
        finished(id, ok, payload)
    }

    /// Stops waiting for the job. The command keeps running to its end (see above).
    function cancelWaiting(id) {
        var cur = get(id)
        if (!cur.running) return
        _set(id, { running: false, startedMs: cur.startedMs, done: false, ok: false, result: null,
                   error: null, cancelled: true, token: cur.token })
    }

    /// Forgets a finished job's answer (the page shows its quiet state again).
    function clear(id) {
        if (get(id).running) return
        var copy = {}
        for (var k in records) if (k !== id) copy[k] = records[k]
        records = copy
    }
}
