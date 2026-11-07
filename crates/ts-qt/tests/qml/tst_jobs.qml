import QtQuick
import QtTest
import org.truesense.app

// Jobs: long commands that outlive their page; stopping to wait never cancels the command.
TestCase {
    name: "Jobs"
    when: windowShown
    visible: true

    QtObject {
        id: fake
        signal commandFinished(int id, bool ok, string json)
        property int next: 1
        property var calls: []
        function run(area, verb, args) { calls.push([area, verb, args]); return next++ }
        function write(key, area, verb, args) { return next++ }
    }
    SignalSpy { id: finished; target: Jobs; signalName: "finished" }

    function init() {
        fake.calls = []
        fake.next = 1
        Api.transport = fake
        finished.clear()
        Jobs.records = ({})
        Jobs._inflight = ({})
    }
    function cleanup() { Api.transport = null }

    function test_a_job_runs_and_keeps_its_answer() {
        verify(Jobs.start("selftest", "selftest", "run", { skip_fans: true }))
        verify(Jobs.get("selftest").running)
        verify(Jobs.busy("selftest"))
        compare(fake.calls[0], ["selftest", "run", '{"skip_fans":true}'])
        fake.commandFinished(1, true, '{"ok":true,"output":"PASS  x"}')
        var job = Jobs.get("selftest")
        verify(!job.running)
        verify(job.done)
        compare(job.result.output, "PASS  x")
        verify(!Jobs.busy("selftest"))
        compare(finished.count, 1)
        compare(finished.signalArguments[0][0], "selftest")
    }

    function test_a_second_start_is_refused_while_the_first_runs() {
        verify(Jobs.start("dts", "audio", "dts", { action: "all" }))
        verify(!Jobs.start("dts", "audio", "dts", { action: "all" }))
        compare(fake.calls.length, 1)
    }

    function test_stopping_to_wait_does_not_free_the_job_and_drops_the_late_answer() {
        Jobs.start("update", "update", "apply", { yes: true })
        Jobs.cancelWaiting("update")
        verify(!Jobs.get("update").running)
        verify(Jobs.get("update").cancelled)
        verify(Jobs.busy("update"), "the command itself is still running")
        verify(!Jobs.start("update", "update", "apply", { yes: true }), "no second installer while the first runs")
        fake.commandFinished(1, true, '{"returncode":0}')
        verify(!Jobs.busy("update"))
        verify(!Jobs.get("update").done, "the late answer is not shown")
        compare(finished.count, 0)
        verify(Jobs.start("update", "update", "apply", { yes: true }))
    }

    function test_an_error_is_kept_verbatim_and_not_toasted() {
        Toasts.items = []
        Jobs.start("doctor", "doctor", "run", {})
        fake.commandFinished(1, false, '{"message":"refused: checksum mismatch","code":"x"}')
        var job = Jobs.get("doctor")
        verify(job.done)
        verify(!job.ok)
        compare(job.error.message, "refused: checksum mismatch")
        compare(Toasts.items.length, 0)
    }

    function test_clear_forgets_a_finished_job_only() {
        Jobs.start("a", "x", "y", {})
        Jobs.clear("a")
        verify(Jobs.get("a").running, "a running job stays")
        fake.commandFinished(1, true, "1")
        Jobs.clear("a")
        verify(!Jobs.get("a").done)
    }
}
