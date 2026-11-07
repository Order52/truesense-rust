// Audio: sound repair, output, speaker filter, DTS, the 3.5 mm jack, microphones and Purified Voice
// (the port of the web page audio.js). The two operations that can take a while (the sound repair
// and "DTS on every output") run as Jobs: the page shows their progress and never waits for them,
// and leaving the page does not lose their answer.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

PageFrame {
    id: page
    title: Nav.currentPage ? Nav.titleOf(Nav.currentPage) : ""

    property string soundState: "unknown"
    property string soundProblem: ""
    property string outputActive: ""
    property var jackPlugged: null
    property string dtsDevices: ""
    property string jackSetting: "auto"
    property string jackDetected: "none"
    property string voiceHeadset: "off"
    property string voiceArray: "off"
    property bool dtsBusy: false
    property int _ticks: 0
    readonly property var dtsJob: Jobs.get("audio-dts-all")
    readonly property var repairJob: Jobs.get("audio-repair")
    readonly property var life: ({ alive: true })
    Component.onDestruction: life.alive = false

    function guarded(fn) {
        var l = life
        return function (x) { if (l.alive) fn(x) }
    }
    function vm(verb, args) {
        try {
            var r = JSON.parse(Bus.vm("audio", verb, JSON.stringify({ value: args })))
            return r.ok ? r.result : null
        } catch (e) { return null }
    }

    readonly property string nextOutput: {
        var n = vm("next_output", { active: outputActive, jack_plugged: jackPlugged })
        return n === null ? "speakers" : n
    }

    // ---- reading the machine ------------------------------------------------------------------
    // Several status commands at once; the controls are set from their answers together. A section
    // whose command failed is left as it is.
    function load(slow) {
        var acc = {}
        var pending = 0
        function done() { if (--pending === 0) apply(acc) }
        function ask(key, area, verb, args) {
            pending++
            Api.call(area, verb, args, guarded(function (r) { acc[key] = r; done() }),
                     guarded(function () { done() }))
        }
        ask("sound", "audio", "sound", {})
        ask("output", "audio", "output", { action: "status" })
        ask("filter", "audio", "speaker-filter", { action: "status" })
        if (slow) ask("dts", "audio", "dts", { action: "status" })
        ask("jack", "audio", "jack", {})
        ask("mic", "audio", "mic", { action: "status" })
        ask("voice", "audio", "voice", { action: "status" })
    }

    function apply(acc) {
        var s = vm("state", acc)
        if (s === null) return
        if (s.sound) { soundState = s.sound.state; soundProblem = s.sound.problem || "" }
        if (s.output) {
            outputActive = s.output.active || ""
            jackPlugged = s.output.jack_plugged
        }
        if (s.filter !== null) filter.checked = s.filter
        if (s.dts) {
            dts.checked = s.dts.on
            if (s.dts.mode) dtsMode.current = s.dts.mode
            dtsDevices = s.dts.devices_text
        }
        if (s.jack) { jackSetting = s.jack.setting; jackDetected = s.jack.detected }
        if (s.mic) {
            mute.checked = s.mic.muted
            if (s.mic.volume !== null && !micVolume.dragging) micVolume.value = s.mic.volume
            if (s.mic.boost !== null && !boost.dragging) boost.value = s.mic.boost
        }
        if (s.voice) {
            voiceHeadset = s.voice.headset
            voiceArray = s.voice.array
            if (s.voice.notify !== null) notify.checked = s.voice.notify
        }
    }

    /// A command from a control: on success read the state back, on failure say why (verbatim) and read it back too.
    function act(area, verb, args, slow) {
        Api.call(area, verb, args, guarded(function () { load(slow) }),
                 guarded(function (err) { Toasts.error(err.message); load(true) }))
    }

    Component.onCompleted: load(true)

    // Slow status (DTS asks the sound server for every device) only on opening and after a change;
    // the rest every fifth live tick. The stream ticks only while the window is visible.
    Connections {
        target: Live
        function onSerialChanged() {
            page._ticks++
            if (page._ticks % 5 === 0) page.load(false)
        }
    }
    Connections {
        target: Jobs
        function onFinished(id) {
            if (id === "audio-dts-all" || id === "audio-repair") page.load(true)
        }
    }

    CapabilityNote {
        capability: "audio"
        //% "This laptop's sound controls are not available."
        text: qsTrId("audio_missing")
    }

    // ---- sound output -------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Sound output"
        title: qsTrId("audio_sound")
        icon: "audio"

        HelpText {
            text: {
                if (page.soundState === "ok")
                    //% "The sound card's output is present."
                    return qsTrId("audio_sound_ok")
                if (page.soundState === "no-card")
                    //% "No sound card was found."
                    return qsTrId("audio_sound_no_card")
                if (page.soundState === "problem")
                    //% "There is no sound output: %1. Press Fix sound."
                    return qsTrId("audio_sound_problem").arg(page.soundProblem !== "" ? page.soundProblem : "?")
                //% "Checking that the sound card has an output …"
                return qsTrId("audio_sound_checking")
            }
        }
        SettingRow {
            Layout.fillWidth: true
            //% "No sound at all?"
            label: qsTrId("audio_fix_label")
            //% "Restarts the audio session manager (WirePlumber) so it probes the sound card again, and checks that an output exists. Nothing is written to the sound chip."
            sub: qsTrId("audio_fix_sub")
            AppButton {
                //% "Fix sound"
                text: qsTrId("audio_fix")
                enabled: !Jobs.busy("audio-repair")
                onClicked: Jobs.start("audio-repair", "audio", "repair", {})
            }
        }
        LongRunning {
            job: page.repairJob
            //% "Repairing the sound output"
            text: qsTrId("audio_fix_running")
            onStopWaiting: Jobs.cancelWaiting("audio-repair")
        }
        ReadbackNote {
            readonly property var summary: page.repairJob.done && page.repairJob.ok
                ? page.vm("repair_summary", page.repairJob.result) : null
            visible: page.repairJob.done
            kind: !page.repairJob.ok ? "error" : (summary && summary.kind === "failed" ? "error" : "ok")
            text: {
                if (!page.repairJob.done) return ""
                if (!page.repairJob.ok) return page.repairJob.error ? page.repairJob.error.message : ""
                if (summary === null) return ""
                if (summary.kind === "repaired")
                    //% "The sound output was repaired: %1"
                    return qsTrId("audio_fix_done").arg(summary.steps)
                if (summary.kind === "nothing")
                    //% "The sound output is there: nothing to repair."
                    return qsTrId("audio_fix_nothing")
                //% "Could not repair the sound output (%1). Check the kernel log for 'sof' errors and try a reboot."
                return qsTrId("audio_fix_failed").arg(summary.steps)
            }
        }
    }

    // ---- output -------------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Output"
        title: qsTrId("audio_output")
        icon: "wave"

        SettingRow {
            Layout.fillWidth: true
            //% "Play sound on"
            label: qsTrId("audio_play_on")
            Segmented {
                enabled: Caps.usable("audio")
                current: page.outputActive
                model: [
                    //% "Speakers"
                    { value: "speakers", label: qsTrId("audio_speakers") },
                    //% "Headphones"
                    { value: "headphones", label: qsTrId("audio_headphones") }
                ]
                onActivated: (value) => page.act("audio", "output", { action: value }, false)
            }
        }
        SettingRow {
            Layout.fillWidth: true
            //% "Switch output"
            label: qsTrId("audio_switch_label")
            sub: page.jackPlugged === false && page.outputActive === "speakers"
                //% "Nothing is plugged into the 3.5 mm jack, so there is nothing to switch to."
                ? qsTrId("audio_switch_nothing") : ""
            AppButton {
                text: page.nextOutput === "headphones"
                    //% "Switch to headphones"
                    ? qsTrId("audio_switch_headphones")
                    //% "Switch to speakers"
                    : qsTrId("audio_switch_speakers")
                enabled: Caps.usable("audio") && page.outputActive !== "" && page.nextOutput !== page.outputActive
                onClicked: page.act("audio", "output", { action: page.nextOutput }, false)
            }
        }
        ToggleRow {
            id: filter
            Layout.fillWidth: true
            //% "Noise filter on the speakers"
            label: qsTrId("audio_filter")
            //% "Off by default: it also gates music. For calls on the speakers."
            sub: qsTrId("audio_filter_sub")
            onToggled: (on) => page.act("audio", "speaker-filter", { action: on ? "on" : "off" }, false)
        }
    }

    // ---- DTS ----------------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "DTS sound"
        title: qsTrId("audio_dts")
        icon: "sparkle"

        ToggleRow {
            id: dts
            Layout.fillWidth: true
            //% "Acer DTS sound"
            label: qsTrId("audio_dts_toggle")
            //% "This laptop plays on the speakers or the headphone jack, not both at once."
            sub: qsTrId("audio_dts_sub")
            enabled: !page.dtsBusy
            onToggled: (on) => {
                page.dtsBusy = true
                Api.call("audio", "dts", { action: on ? "on" : "off" }, page.guarded(function () {
                    page.dtsBusy = false
                    page.load(true)
                }), page.guarded(function (err) {
                    page.dtsBusy = false
                    Toasts.error(err.message)
                    page.load(true)
                }))
            }
        }
        SelectRow {
            id: dtsMode
            Layout.fillWidth: true
            //% "DTS sound mode"
            label: qsTrId("audio_dts_mode")
            //% "Applies to every output: speakers, the jack, USB and Bluetooth headsets, controllers. Plain is no processing."
            sub: qsTrId("audio_dts_mode_sub")
            options: ["Music", "Movie", "Voice", "Game1", "Game2", "Game3", "Plain"]
            enabled: !page.dtsBusy
            onActivated: (value) => {
                page.dtsBusy = true
                Api.call("audio", "dts", { action: "mode", name: value }, page.guarded(function () {
                    page.dtsBusy = false
                    //% "DTS sound: %1 on every output"
                    Toasts.ok(qsTrId("audio_dts_mode_done").arg(value))
                    page.load(true)
                }), page.guarded(function (err) {
                    page.dtsBusy = false
                    Toasts.error(err.message)
                    page.load(true)
                }))
            }
        }
        HelpText {
            visible: page.dtsDevices !== ""
            //% "Now: %1"
            text: qsTrId("audio_dts_now").arg(page.dtsDevices)
        }
        SettingRow {
            Layout.fillWidth: true
            //% "A headset still sounds flat?"
            label: qsTrId("audio_dts_all_label")
            //% "New outputs get DTS by themselves when you plug them in. This does it again for all of them now (and adds a headphone EQ to headsets)."
            sub: qsTrId("audio_dts_all_sub")
            AppButton {
                //% "Use DTS on every output"
                text: qsTrId("audio_dts_all")
                enabled: !Jobs.busy("audio-dts-all")
                onClicked: Jobs.start("audio-dts-all", "audio", "dts", { action: "all" })
            }
        }
        LongRunning {
            job: page.dtsJob
            //% "Setting up DTS on every output"
            text: qsTrId("audio_dts_all_running")
            onStopWaiting: Jobs.cancelWaiting("audio-dts-all")
        }
        ReadbackNote {
            visible: page.dtsJob.done
            kind: page.dtsJob.ok ? "ok" : "error"
            text: {
                if (!page.dtsJob.done) return ""
                if (!page.dtsJob.ok) return page.dtsJob.error ? page.dtsJob.error.message : ""
                var s = page.vm("dts_all_summary", page.dtsJob.result)
                return s && s.count > 0
                    //% "DTS is on for: %1"
                    ? qsTrId("audio_dts_all_done").arg(s.text)
                    //% "DTS is on for every output."
                    : qsTrId("audio_dts_all_done_plain")
            }
        }
    }

    // ---- 3.5 mm jack --------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "3.5 mm jack"
        title: qsTrId("audio_jack")
        icon: "audio"

        SettingRow {
            Layout.fillWidth: true
            //% "I plugged in"
            label: qsTrId("audio_jack_label")
            Segmented {
                current: page.jackSetting
                model: [
                    //% "Auto"
                    { value: "auto", label: qsTrId("audio_jack_auto") },
                    //% "Headphones"
                    { value: "headphones", label: qsTrId("audio_jack_headphones") },
                    //% "Headset"
                    { value: "headset", label: qsTrId("audio_jack_headset") }
                ]
                onActivated: (value) => {
                    page.jackSetting = value
                    page.act("audio", "jack", { use: value }, false)
                }
            }
        }
        HelpText {
            text: {
                if (page.jackSetting === "headphones")
                    //% "Output only: the headset microphone is never used."
                    return qsTrId("audio_jack_hint_headphones")
                if (page.jackSetting === "headset")
                    //% "Always use the headset microphone, even if the codec has not detected it (try this if it is missing after boot)."
                    return qsTrId("audio_jack_hint_headset")
                //% "Uses what the codec detects when you plug something in."
                return qsTrId("audio_jack_hint_auto")
            }
        }
        HelpText {
            color: Theme.stat
            text: page.jackDetected === "headset"
                //% "Detected now: a headset with a microphone"
                ? qsTrId("audio_jack_detected_headset")
                : (page.jackDetected === "headphones"
                   //% "Detected now: headphones"
                   ? qsTrId("audio_jack_detected_headphones")
                   //% "Detected now: nothing plugged in"
                   : qsTrId("audio_jack_detected_none"))
        }
        SliderRow {
            id: boost
            Layout.fillWidth: true
            //% "Headset microphone boost"
            label: qsTrId("audio_boost")
            from: 0
            to: 3
            stepSize: 1
            onReleased: (v) => Api.write("audio.boost", "audio", "mic", { action: "boost", value: v })
        }
    }

    // ---- microphones --------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Microphone"
        title: qsTrId("audio_mic")
        icon: "wave"

        SliderRow {
            id: micVolume
            Layout.fillWidth: true
            //% "Microphone volume"
            label: qsTrId("audio_mic_volume")
            from: 0
            to: 150
            stepSize: 5
            unit: " %"
            onReleased: (v) => Api.write("audio.micvol", "audio", "mic", { action: "volume", value: v })
        }
        ToggleRow {
            id: mute
            Layout.fillWidth: true
            //% "Mute every microphone"
            label: qsTrId("audio_mute")
            onToggled: (on) => page.act("audio", "mic", { action: on ? "mute" : "unmute" }, false)
        }
        SettingRow {
            Layout.fillWidth: true
            //% "Headset (AI noise reduction)"
            label: qsTrId("audio_voice_headset")
            Segmented {
                current: page.voiceHeadset
                model: page.voiceModel
                onActivated: (value) => {
                    page.voiceHeadset = value
                    page.act("audio", "voice", { action: "set", a: "headset", b: value }, false)
                }
            }
        }
        SettingRow {
            Layout.fillWidth: true
            //% "Built-in (AI noise reduction)"
            label: qsTrId("audio_voice_array")
            Segmented {
                current: page.voiceArray
                model: page.voiceModel
                onActivated: (value) => {
                    page.voiceArray = value
                    page.act("audio", "voice", { action: "set", a: "array", b: value }, false)
                }
            }
        }
        ToggleRow {
            id: notify
            Layout.fillWidth: true
            //% "Notify when the microphone changes"
            label: qsTrId("audio_notify")
            onToggled: (on) => page.act("audio", "voice", { action: "notify", a: on ? "on" : "off" }, false)
        }
    }

    readonly property var voiceModel: [
        //% "Off"
        { value: "off", label: qsTrId("audio_voice_off") },
        //% "Conference"
        { value: "conference", label: qsTrId("audio_voice_conference") },
        //% "Personal"
        { value: "personal", label: qsTrId("audio_voice_personal") }
    ]
}
