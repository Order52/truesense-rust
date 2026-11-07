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

    function call(module, verb, args) {
        var t = Api.transport
        if (t === null || t === undefined || typeof t.vm !== "function") {
            lastError = "no view-model available"
            return null
        }
        var answer
        try {
            answer = JSON.parse(t.vm(module, verb, JSON.stringify(args === undefined ? {} : args)))
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
        return answer.result
    }
}
