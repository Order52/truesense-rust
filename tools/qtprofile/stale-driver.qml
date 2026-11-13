// The app's own window (Main) plus a command poller, for tools/qtprofile/stale.py only: the program
// loads it through TRUESENSE_QT_QML_URL from a private copy next to `cmd.js`. Every 120 ms it reads
// cmd.js ("<serial>\n<javascript>"); a new serial runs the script as `run(w, done)` (w: the window,
// done(result): prints `REPORT {"seq":..,"result":..}`). Never shipped, never in the real app.
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Window
import org.truesense.app

Main {
    id: w
    property int seq: 0
    Timer {
        interval: 120; repeat: true; running: true
        onTriggered: {
            var x = new XMLHttpRequest()
            x.open("GET", Qt.resolvedUrl("cmd.js"), false)
            try { x.send() } catch (e) { return }
            var t = x.responseText
            var nl = t.indexOf("\n")
            if (nl < 0) return
            var n = parseInt(t.substring(0, nl))
            if (!(n > w.seq)) return
            w.seq = n
            try {
                var o = Qt.createQmlObject("import QtQuick\nimport QtQuick.Controls.Basic\nimport org.truesense.app\n"
                    + "QtObject { function run(w, done) {\n" + t.substring(nl + 1) + "\n} }", w, "cmd" + n)
                o.run(w, function (r) { Bus.report(JSON.stringify({ seq: n, result: r === undefined ? null : r })) })
            } catch (e) {
                Bus.report(JSON.stringify({ seq: n, error: String(e) }))
            }
        }
    }
}
