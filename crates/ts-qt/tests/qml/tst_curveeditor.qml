import QtQuick
import QtTest
import org.truesense.app

// The editor draws what `engine` says and sends the pointer and keyboard to it. The engine here is a
// small fake with the same contract as ts-ui-model's cooling view-model (whose rules are tested in
// Rust), so these tests are about the widget: which verb each gesture calls, and the signals.
TestCase {
    name: "CurveEditor"
    when: windowShown
    visible: true
    width: 600
    height: 360

    QtObject {
        id: fake
        property var log: []
        property string refuse: ""
        function plot(args) {
            var m = args.margins
            return { left: m[0], top: m[2], right: args.width - m[1], bottom: args.height - m[3] }
        }
        function x(p, t) { return p.left + (t - 20) / 90 * (p.right - p.left) }
        function y(p, d) { return p.top + (1 - d / 100) * (p.bottom - p.top) }
        function call(verb, args) {
            log.push(verb)
            if (verb !== "geometry" && verb !== "hit" && refuse !== "") return { error: refuse }
            var p = plot(args)
            if (verb === "geometry") {
                var dots = args.points.map(function (q) { return [x(p, q[0]), y(p, q[1])] })
                return { plot: p, x_ticks: [{ value: 20, px: x(p, 20) }], y_ticks: [{ value: 0, px: y(p, 0) }],
                         auto_band: { top: y(p, 20), bottom: y(p, 0) }, limit: { temp: 95, x: x(p, 95) },
                         line: dots, other_line: [], dots: dots,
                         now: args.temp === null || args.temp === undefined ? null
                              : { x: x(p, args.temp), y: y(p, 50), temp: args.temp, duty: 50 } }
            }
            if (verb === "hit") {
                for (var i = 0; i < args.points.length; ++i)
                    if (Math.hypot(x(p, args.points[i][0]) - args.x, y(p, args.points[i][1]) - args.y) <= 14) return i
                return null
            }
            var pts = JSON.parse(JSON.stringify(args.points))
            if (verb === "drag") {
                pts[args.index] = [Math.round(20 + (args.x - p.left) / (p.right - p.left) * 90),
                                   5 * Math.round((1 - (args.y - p.top) / (p.bottom - p.top)) * 20)]
                return { points: pts, index: args.index }
            }
            if (verb === "nudge") {
                pts[args.index][0] += args.dt
                pts[args.index][1] += 5 * args.dd
                return { points: pts, index: args.index }
            }
            if (verb === "insert") {
                var t = args.t !== undefined ? args.t : Math.round(20 + (args.x - p.left) / (p.right - p.left) * 90)
                pts.push([t, 50])
                pts.sort(function (a, b) { return a[0] - b[0] })
                return { points: pts, index: pts.findIndex(function (q) { return q[0] === t }) }
            }
            if (verb === "insert_middle") {
                pts.splice(1, 0, [(pts[0][0] + pts[1][0]) / 2, 40])
                return { points: pts, index: 1 }
            }
            if (verb === "remove") {
                pts.splice(args.index, 1)
                return { points: pts, index: Math.min(args.index, pts.length - 1) }
            }
            return { error: "unknown " + verb }
        }
    }

    CurveEditor {
        id: ed
        width: 560
        height: 300
        engine: fake
        points: [[50, 0], [70, 50], [90, 100]]
    }
    SignalSpy { id: started; target: ed; signalName: "editStarted" }
    SignalSpy { id: edited; target: ed; signalName: "edited" }
    SignalSpy { id: refused; target: ed; signalName: "refused" }

    function init() {
        ed.points = [[50, 0], [70, 50], [90, 100]]
        ed.locked = false
        fake.refuse = ""
        fake.log = []
        started.clear(); edited.clear(); refused.clear()
    }

    function dot(i) {
        var g = ed.geo
        return { x: g.dots[i][0], y: g.dots[i][1] }
    }

    function test_it_draws_one_focusable_dot_per_point() {
        verify(ed.geo !== null)
        compare(ed.geo.dots.length, 3)
        compare(ed.geo.plot.left > 20, true, "room for the axis labels")
    }

    function test_the_live_temperature_moves_the_marker_and_never_rebuilds_the_chart() {
        ed.nowTemp = NaN
        compare(ed.nowAt, null)
        var drawn = ed.drawCount
        for (var t = 55; t <= 85; t += 5) {
            ed.nowTemp = t
            verify(ed.nowAt !== null)
            compare(ed.nowAt.temp, t)
        }
        wait(50)
        compare(ed.drawCount, drawn, "the grid, labels and curves are not redrawn for a new temperature")
        ed.nowTemp = NaN
        compare(ed.nowAt, null)
    }

    function test_a_change_of_the_points_rebuilds_the_chart_once() {
        var drawn = ed.drawCount
        ed.points = [[50, 0], [70, 60], [90, 100]]
        compare(ed.drawCount, drawn + 1)
    }

    function test_a_drag_edits_once_per_gesture_and_reports_every_move() {
        var p = dot(1)
        mousePress(ed, p.x, p.y)
        mouseMove(ed, p.x + 10, p.y - 10)
        mouseMove(ed, p.x + 20, p.y - 20)
        mouseRelease(ed, p.x + 20, p.y - 20)
        compare(started.count, 1, "one undo step for the whole drag")
        verify(edited.count >= 1)
        var last = edited.signalArguments[edited.count - 1][0]
        compare(last.length, 3)
        verify(last[1][0] > 70, "moved right")
    }

    function test_a_press_in_empty_space_does_not_edit() {
        mousePress(ed, 300, 100)
        mouseMove(ed, 320, 120)
        mouseRelease(ed, 320, 120)
        compare(edited.count, 0)
        compare(started.count, 0)
    }

    function test_a_locked_curve_ignores_pointer_and_keys() {
        ed.locked = true
        var p = dot(1)
        mousePress(ed, p.x, p.y)
        mouseMove(ed, p.x + 30, p.y)
        mouseRelease(ed, p.x + 30, p.y)
        compare(edited.count, 0)
        verify(!ed.addMiddle())
        verify(!ed.removeAt(0))
        verify(!ed.insertAt(60))
        compare(fake.log.filter(function (v) { return v === "drag" || v === "insert" || v === "remove" }).length, 0)
    }

    function test_a_right_click_removes_the_point() {
        var p = dot(1)
        mouseClick(ed, p.x, p.y, Qt.RightButton)
        compare(started.count, 1)
        compare(edited.count, 1)
        compare(edited.signalArguments[0][0].length, 2)
    }

    function test_a_double_click_in_empty_space_adds_a_point_and_on_a_point_does_not() {
        var p = dot(1)
        mouseDoubleClickSequence(ed, p.x, p.y)
        compare(edited.count, 0, "on a point nothing is added")
        var g = ed.geo.plot
        mouseDoubleClickSequence(ed, g.left + (g.right - g.left) * 0.2, g.top + 40)
        compare(edited.count, 1)
        compare(edited.signalArguments[0][0].length, 4)
    }

    function test_the_add_button_function_uses_the_middle_of_a_gap() {
        verify(ed.addMiddle())
        compare(edited.signalArguments[0][0].length, 4)
        compare(started.count, 1)
    }

    function test_arrow_keys_on_a_focused_point_nudge_and_delete_removes() {
        var item = null
        var p = dot(1)
        mousePress(ed, p.x, p.y)
        mouseRelease(ed, p.x, p.y)
        keyClick(Qt.Key_Right)
        compare(edited.count >= 1, true)
        compare(edited.signalArguments[edited.count - 1][0][1][0], 71)
        keyClick(Qt.Key_Up)
        keyClick(Qt.Key_Delete)
        compare(edited.signalArguments[edited.count - 1][0].length, 2)
    }

    function test_a_refusal_from_the_engine_becomes_the_refused_signal_and_no_edit() {
        fake.refuse = "A curve keeps at least 2 points."
        verify(!ed.removeAt(0))
        compare(refused.count, 1)
        compare(refused.signalArguments[0][0], "A curve keeps at least 2 points.")
        compare(edited.count, 0)
        compare(started.count, 0)
    }

    function test_without_an_engine_nothing_is_drawn_and_nothing_crashes() {
        ed.engine = null
        compare(ed.geo, null)
        compare(ed.hitAt(10, 10), -1)
        verify(!ed.addMiddle())
        ed.engine = fake
        verify(ed.geo !== null)
    }
}
