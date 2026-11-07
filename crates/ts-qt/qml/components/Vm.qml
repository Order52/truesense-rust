// Calls the pure view-model modules of ts-ui-model (src/vm) from QML:
//
//   var v = Vm.call("home", "view", { frame: Live.frame })      // the result, or null on an error
//
// The Rust Bus is reached through Api.transport (set by Main.qml), so components and tests that run
// without Rust simply get null and render their empty state; `lastError` says why.
pragma Singleton
import QtQuick
import org.truesense.app

QtObject {
    id: vm

    property string lastError: ""

    /// The last answer of each module.verb, as text and as the parsed object. A view-model call is
    /// made again on every live frame (once a second); almost always the answer is the same text
    /// as the last one, and handing back the SAME object then means nothing that depends on it is
    /// re-evaluated, no Repeater rebuilds its delegates and no canvas repaints. (Before this, the
    /// Home page rebuilt every tile button once a second.) The result must be treated as read-only.
    property var _memo: ({})

    function call(module, verb, args) {
        var t = Api.transport
        if (t === null || t === undefined || typeof t.vm !== "function") {
            lastError = "no view-model available"
            return null
        }
        var answer, text
        try {
            text = t.vm(module, verb, JSON.stringify(args === undefined ? {} : args))
            var key = module + "." + verb
            var seen = _memo[key]
            if (seen !== undefined && seen.text === text) {
                lastError = ""
                return seen.result
            }
            answer = JSON.parse(text)
        } catch (e) {
            lastError = String(e)
            return null
        }
        if (!answer.ok) {
            lastError = String(answer.error)
            console.warn("view-model " + module + "." + verb + ": " + lastError)
            return null
        }
        lastError = ""
        _memo[key] = { text: text, result: answer.result }
        return answer.result
    }
}
