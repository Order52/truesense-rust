import QtQuick
import QtTest
import org.truesense.app

TestCase {
    name: "Singletons"
    when: windowShown
    visible: true

    // A fake of the Rust Bus: records calls and lets the test answer them.
    QtObject {
        id: fake
        signal commandFinished(int id, bool ok, string json)
        property int next: 1
        property var calls: []
        function run(area, verb, args) { calls.push(["run", area, verb, args]); return next++ }
        function write(key, area, verb, args) { calls.push(["write", key, area, verb, args]); return next++ }
    }

    function init() {
        fake.calls = []
        fake.next = 1
        Api.transport = fake
        Toasts.items = []
        Caps.model = ({})
    }
    function cleanup() { Api.transport = null }

    // ---- Api -------------------------------------------------------------------------------
    function test_api_call_delivers_the_decoded_result() {
        var got = null
        Api.call("power", "set", { mode: "turbo" }, function (r) { got = r })
        compare(fake.calls[0], ["run", "power", "set", '{"mode":"turbo"}'])
        compare(Api.pending, 1)
        fake.commandFinished(1, true, '{"mode":"turbo"}')
        compare(got.mode, "turbo")
        compare(Api.pending, 0)
    }

    function test_api_error_is_a_toast_with_the_text_verbatim() {
        Api.call("fan", "off", {})
        fake.commandFinished(1, false, '{"message":"refused: too hot","code":"x"}')
        compare(Toasts.items.length, 1)
        compare(Toasts.items[0].text, "refused: too hot")
        compare(Toasts.items[0].kind, "error")
    }

    function test_api_fail_callback_replaces_the_toast() {
        var err = null
        Api.call("fan", "off", {}, null, function (e) { err = e })
        fake.commandFinished(1, false, '{"message":"no","code":"needs-daemon"}')
        compare(Toasts.items.length, 0)
        compare(err.message, "no")
        verify(Api.needsDaemon(err))
    }

    function test_superseded_writes_are_silent() {
        Api.write("fan.cpu", "fan", "set", { cpu: 30 })
        compare(fake.calls[0][0], "write")
        compare(fake.calls[0][1], "fan.cpu")
        fake.commandFinished(1, false, '{"message":"replaced","code":"superseded"}')
        compare(Toasts.items.length, 0)
        compare(Api.pending, 0)
    }

    function test_unknown_ids_are_ignored() { fake.commandFinished(99, true, "1") }

    // ---- Live ------------------------------------------------------------------------------
    function test_live_parses_frames_and_histories() {
        Live.load('{"n":3,"frame":{"sensors":{"cpu_temp":54,"gpu_temp":null},"daemon":{}},"history":{"sensors.cpu_temp":[50,null,54]}}')
        compare(Live.serial, 3)
        compare(Live.ok, true)
        compare(Live.value("sensors.cpu_temp"), 54)
        compare(Live.value("sensors.gpu_temp"), undefined)
        compare(Live.value("nope.deeper"), undefined)
        compare(Live.history("sensors.cpu_temp").length, 3)
        compare(Live.history("missing").length, 0)
        compare(Live.daemonUp, true)
        Live.markLost()
        compare(Live.ok, false)
        Live.load("{ broken")
        compare(Live.serial, 3, "a malformed snapshot is ignored")
    }

    // ---- Caps ------------------------------------------------------------------------------
    function test_caps_states() {
        Caps.load('{"rear-light":{"state":"unavailable","why":"no bar"},"fans":{"state":"available"},"undervolt":{"state":"needs-daemon"}}')
        compare(Caps.state("fans"), "available")
        compare(Caps.usable("fans"), true)
        compare(Caps.usable("rear-light"), false)
        compare(Caps.why("rear-light"), "no bar")
        compare(Caps.usable("undervolt"), false)
        compare(Caps.state("never-heard-of"), "unknown")
        compare(Caps.usable("never-heard-of"), true)
        Caps.load("{ broken")
        compare(Caps.state("fans"), "unknown")
    }

    // ---- Nav -------------------------------------------------------------------------------
    function test_nav_navigates_and_falls_back_to_not_found() {
        Nav.load('{"groups":[{"id":"g","titleId":"t","title":"G"}],"pages":[{"id":"home","group":"g","icon":"home","titleId":"nav_home","title":"Home","qml":"HomePage","aliases":[],"shortcut":1}],"notFound":{"qml":"NotFoundPage"}}')
        Nav.go("home")
        compare(Nav.currentQml, "HomePage")
        compare(Nav.inGroup("g").length, 1)
        Nav.go("nope")
        compare(Nav.currentQml, "NotFoundPage")
        compare(Nav.missing, "nope")
        Nav.go("home")
        compare(Nav.missing, "")
    }

    // ---- Ui and Theme ----------------------------------------------------------------------
    function test_ui_load_and_theme_follow_the_setting() {
        Ui.load('{"theme":"light","language":"pseudo","language_effective":"pseudo","font_scale":1.2,"reduced_motion":true,"problem":"bad file"}')
        compare(Ui.theme, "light")
        compare(Ui.languageSetting, "pseudo")
        compare(Ui.language, "pseudo")
        compare(Theme.dark, false)
        compare(Theme.animMs, 0)
        compare(Ui.problem, "bad file")
        compare(Theme.fontBody, Math.round(14 * 1.2))
        Ui.load('{"theme":"dark","font_scale":1,"reduced_motion":false}')
        compare(Theme.dark, true)
        compare(Theme.bg.toString(), "#111317")
        compare(Ui.problem, "")
    }

    function test_ui_set_is_a_request_not_a_change() {
        var seen = []
        function h(k, v) { seen.push([k, v]) }
        Ui.changeRequested.connect(h)
        Ui.set("theme", "light")
        Ui.changeRequested.disconnect(h)
        compare(seen[0], ["theme", "light"])
        compare(Ui.theme, "dark")
    }

    // ---- Toasts ----------------------------------------------------------------------------
    function test_a_new_toast_replaces_the_one_showing() {
        for (var i = 0; i < 6; ++i) Toasts.show("t" + i)
        compare(Toasts.items.length, 1)
        compare(Toasts.items[0].text, "t5")
        Toasts.dismiss(Toasts.items[0].id)
        compare(Toasts.items.length, 0)
    }
}
