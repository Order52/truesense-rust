// The responsiveness script (TRUESENSE_QT_BENCH=1, `tools/qtprofile/qtprofile.py respond`): never
// part of a normal run. It drives the real shell the way a person would, page by page, tab by tab,
// action by action, and prints one `BENCH {json}` line per step (through Bus.benchLine), then quits.
//
// What one step measures, in milliseconds from the moment it acts (the clock is Bus.clockMs):
//   sync_ms    how long the act itself held the GUI thread (a page or tab is built synchronously),
//   frame_ms   the first frame presented after it (what the person sees first),
//   data_ms    every command the step caused has answered (Api.pending is back to 0),
//   shown_ms   the first frame after that (the page with its data),
//   live_ms    for actions: the live state shows the new value (what a tile bound to Live shows),
//   stall_ms   the longest time the GUI thread did not get back to its event loop during the step
//              (a 4 ms timer that notices how late it fires; only runs during the script).
//
// TRUESENSE_QT_BENCH may name the phases to run, comma separated: start, pages, tabs, actions.
import QtQuick
import org.truesense.app

Item {
    id: bench

    /// The window (its frameSwapped marks frames) and the page loader of the shell.
    property var window: null
    property var loader: null
    property string phases: "1"

    property var queue: []
    property var cur: null
    property real lastTick: 0
    property real maxGap: 0

    function now() { return Bus.clockMs() }
    function wants(p) { return phases === "1" || phases.split(",").indexOf(p) >= 0 }
    function emit(o) { Bus.benchLine(JSON.stringify(o)) }

    // ---- the stall monitor ----------------------------------------------------------------------
    Timer {
        id: stallTimer
        interval: 4
        repeat: true
        onTriggered: {
            var t = bench.now()
            if (bench.lastTick > 0) bench.maxGap = Math.max(bench.maxGap, t - bench.lastTick - interval)
            bench.lastTick = t
        }
    }

    // ---- one step -------------------------------------------------------------------------------
    // step = {phase, name, act: function, until?: function -> bool (live_ms), settle?: ms}
    function runStep(step) {
        cur = { step: step, t0: 0, t1: 0, tf: -1, td: -1, ts: -1, tl: -1, wantFrame: true, wantShown: false }
        maxGap = 0
        lastTick = 0
        stallTimer.restart()
        emit({ phase: "reset" })
        cur.t0 = now()
        step.act()
        cur.t1 = now()
        poll.restart()
    }

    function finish() {
        var c = cur, s = c.step
        stallTimer.stop()
        var r = function (t) { return t < 0 ? null : Math.round((t - c.t0) * 10) / 10 }
        emit({ phase: s.phase, name: s.name, sync_ms: r(c.t1), frame_ms: r(c.tf), data_ms: r(c.td),
               shown_ms: r(c.ts), live_ms: r(c.tl), stall_ms: Math.round(Math.max(maxGap, c.t1 - c.t0) * 10) / 10,
               extra: s.extra ? s.extra() : undefined })
        cur = null
        next.restart()
    }

    Connections {
        target: bench.window
        function onFrameSwapped() {
            var c = bench.cur
            if (c === null) return
            var t = bench.now()
            if (c.tf < 0) c.tf = t
            if (c.wantShown && c.ts < 0) c.ts = t
        }
    }

    // Checks the step's conditions every 2 ms (only while a step runs).
    Timer {
        id: poll
        interval: 2
        repeat: true
        onTriggered: {
            var c = bench.cur
            if (c === null) { stop(); return }
            var t = bench.now()
            if (c.td < 0 && Api.pending === 0 && t - c.t1 > 1) { c.td = t; c.wantShown = true }
            if (c.step.until && c.tl < 0 && c.step.until()) c.tl = t
            // An action need not change the picture (a keyboard colour): then 300 ms without a frame.
            var framed = c.tf >= 0 || (c.step.phase === "action" && c.td >= 0 && t - c.td > 300)
            var settled = c.td >= 0 && (c.ts >= 0 || t - c.td > 150) && framed
                          && (!c.step.until || c.tl >= 0)
            var limit = c.step.timeout || 6000
            if ((settled && t - c.t0 >= (c.step.settle || 0)) || t - c.t0 > limit) { stop(); bench.finish() }
        }
    }

    Timer {
        id: next
        interval: 250
        onTriggered: {
            if (bench.queue.length === 0) { Qt.quit(); return }
            var q = bench.queue.slice()
            var s = q.shift()
            bench.queue = q
            bench.runStep(s)
        }
    }

    // ---- the script -----------------------------------------------------------------------------
    function page() { return loader ? loader.item : null }

    function goStep(phase, id) {
        return { phase: phase, name: id, act: function () { Nav.go(id) }, settle: 300 }
    }

    function tabStep(phase, id) {
        return { phase: phase, name: id, act: function () { bench.page().tab = id }, settle: 300 }
    }

    function begin() {
        emit({ phase: "start", name: "start", t_ms: now(), pending: Api.pending })
        var ids = Nav.pages.map(function (p) { return p.id })
        var q = []
        if (wants("pages")) {
            // The start page was built by the cold start: its first visit is that.
            ids.forEach(function (id) { if (id !== Nav.current) q.push(goStep("page-first", id)) })
            ids.forEach(function (id) { q.push(goStep("page-again", id)) })
        }
        if (wants("tabs")) {
            q.push(goStep("tabs", "lighting"))
            var tabs = ["perkey", "rear", "themes", "looks", "keyboard"]
            tabs.forEach(function (t) { q.push(tabStep("tab-first", t)) })
            tabs.forEach(function (t) { q.push(tabStep("tab-again", t)) })
        }
        if (wants("actions")) actions(q)
        queue = q
        // The first step starts at the first frame: until every command the start page asked
        // has answered and the first live frame is in (the window is usable with its data).
        runStep({ phase: "start", name: "interactive", act: function () {}, settle: 0,
                  until: function () { return Live.ok } })
    }

    function actions(q) {
        q.push(goStep("actions", "performance"))
        var modes = ["turbo", "quiet", "balanced"]
        modes.forEach(function (m) {
            q.push({ phase: "action", name: "power set " + m, timeout: 3000,
                     act: function () { Api.call("power", "set", { mode: m }) },
                     until: function () { return Live.value("power_mode") === m } })
        })
        q.push(goStep("actions", "cooling"))
        q.push({ phase: "action", name: "fan max", act: function () { Api.call("fan", "max", {}) } })
        q.push({ phase: "action", name: "fan auto", act: function () { Api.call("fan", "auto", {}) } })
        q.push(goStep("actions", "lighting"))
        q.push({ phase: "action", name: "keyboard color",
                 act: function () { Api.write("kb.color", "keyboard", "color", { color: "ff3300" }) } })
        // A slider drag: one write per pointer move for half a second; how many reached the keyboard.
        q.push(dragStep("keyboard brightness drag", "kb.level", "keyboard", "brightness",
                        function (i) { return { level: 10 + (i % 40) } }))
        q.push(dragStep("rear brightness drag", "rear", "rear", "set",
                        function (i) { return { effect: "static", color: "3723c7", speed: 3, brightness: 20 + i % 80, verify: false } }))
        q.push({ phase: "action", name: "theme apply", act: function () { Api.call("theme", "apply", { name: "hearth" }) } })
        q.push({ phase: "action", name: "theme stop", act: function () { Api.call("theme", "stop", {}) } })
    }

    // 30 writes, one every 16 ms, then waits for the last one.
    function dragStep(name, key, area, verb, args) {
        var stats = { sent: 0, ran: 0, superseded: 0, failed: 0 }
        return {
            phase: "action", name: name, timeout: 8000,
            extra: function () { stats.superseded = stats.sent - stats.ran - stats.failed; return stats },
            until: function () { return stats.sent >= 30 && Api.pending === 0 },
            act: function () {
                var i = 0
                var send = function () {
                    stats.sent++
                    // (a superseded write calls neither callback: Api.qml drops it silently)
                    Api.write(key, area, verb, args(i++), function () { stats.ran++ },
                              function () { stats.failed++ })
                }
                send()
                dragTimer.fn = function () { if (i < 30) send(); else dragTimer.stop() }
                dragTimer.start()
            }
        }
    }
    Timer { id: dragTimer; interval: 16; repeat: true; property var fn: null; onTriggered: fn() }
}
