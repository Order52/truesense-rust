// "Lighting for [power mode]": choose which look each power mode switches the lights to. The look is
// copied into the daemon, which applies it when the power mode changes (docs/APP-DESIGN.md 4.3).
// Editing is a draft: the Changed badge shows while the draft differs from what the daemon holds,
// Save writes it, Undo takes the last choice back. The fan curve of a mode is shown read-only; the
// Cooling page owns it. The edit state and its rules live in the view-model (bindings_*).
import QtQuick
import QtQuick.Layouts
import org.truesense.app

Card {
    id: root

    /// The edit state of the view-model (a JSON object), null before the first read.
    property var bstate: null
    /// What the view-model says about it: {selected, current, modes:[{mode, look, saved, curve, changed, can_undo, current}]}.
    property var view: null
    property bool busy: false
    property bool failed: false

    readonly property var row: {
        if (view === null) return null
        for (var i = 0; i < view.modes.length; ++i)
            if (view.modes[i].mode === view.selected) return view.modes[i]
        return null
    }
    readonly property bool usable: Caps.usable("daemon") && !failed

    //% "Lighting for each power mode"
    title: qsTrId("light_strip_title")
    icon: "performance"
    //% "Now: %1"
    rightText: view !== null && view.current !== null
        ? qsTrId("light_strip_now").arg(LightingVm.modeName(view.current)) : ""
    Layout.fillWidth: true

    function _apply(out) {
        if (out === undefined) return
        bstate = out.state
        view = out.view
    }

    function reload() {
        Api.call("mode", "bindings", {}, function (result) {
            failed = false
            _apply(LightingVm.call("bindings_load", { state: bstate, result: result }))
        }, function () { failed = true })
    }

    function _edit(verb, extra) {
        var args = { state: bstate }
        for (var k in extra) args[k] = extra[k]
        _apply(LightingVm.call(verb, args))
    }

    function save() {
        var plan = LightingVm.call("bindings_plan", { state: bstate, mode: view.selected })
        if (plan === undefined || plan.length === 0) return
        var mode = view.selected
        var isNow = view.current === mode
        busy = true
        Api.call(plan[0].area, plan[0].verb, plan[0].args, function () {
            busy = false
            reload()
            //% "Saved: %1 will use this look"
            Toasts.show(qsTrId("light_strip_saved").arg(LightingVm.modeName(mode)), "ok")
            // The daemon applies a binding when the mode changes; the mode in use gets it at once.
            if (isNow) Api.call("mode", "apply", { mode: mode }, function () {})
        }, function (err) {
            busy = false
            Toasts.show(err && err.message ? err.message : "", "error")
        })
    }

    Component.onCompleted: {
        LightingVm.refreshLooks()
        reload()
    }
    Connections {
        target: Live
        function onDaemonUpChanged() { if (Live.daemonUp) root.reload() }
    }

    CapabilityNote {
        capability: "daemon"
        //% "Power mode looks are applied by the TrueSense service, which is not running."
        text: qsTrId("light_strip_needs_daemon")
    }

    TileRow {
        Layout.fillWidth: true
        enabled: root.usable && root.view !== null
        tileHeight: 60
        minTileWidth: 78
        variant: "outlined"
        current: root.view === null ? undefined : root.view.selected
        model: {
            var out = []
            var rows = root.view === null ? [] : root.view.modes
            for (var i = 0; i < rows.length; ++i)
                out.push({ value: rows[i].mode, label: LightingVm.modeName(rows[i].mode), icon: rows[i].mode })
            return out
        }
        onActivated: (mode) => root._edit("bindings_select", { mode: mode })
    }

    SelectRow {
        Layout.fillWidth: true
        enabled: root.usable && root.row !== null
        //% "Look"
        label: qsTrId("light_strip_look")
        options: {
            //% "No look (leave the lights as they are)"
            var out = [{ value: "", text: qsTrId("light_strip_none") }]
            for (var i = 0; i < LightingVm.looks.length; ++i)
                out.push({ value: LightingVm.looks[i], text: LightingVm.looks[i] })
            return out
        }
        current: root.row === null ? undefined : (root.row.look === null ? "" : root.row.look)
        onActivated: (name) => root._edit("bindings_choose", { mode: root.view.selected, look: name === "" ? null : name })
    }

    Flow {
        Layout.fillWidth: true
        spacing: 8

        Rectangle {
            visible: root.row !== null && root.row.changed
            height: Theme.controlHeight
            width: badge.implicitWidth + 24
            radius: height / 2
            color: Theme.accentSoft
            border.color: Theme.accentLine
            Text {
                id: badge
                anchors.centerIn: parent
                //% "Changed"
                text: qsTrId("light_strip_changed")
                color: Theme.accent
                font.pixelSize: Theme.fontSmall + 1
                font.bold: true
            }
            Accessible.role: Accessible.StaticText
            //% "Changed: not saved to the power mode yet"
            Accessible.name: qsTrId("light_strip_changed_a11y")
        }
        SecondaryButton {
            //% "Undo"
            text: qsTrId("light_strip_undo")
            iconName: "undo"
            enabled: root.usable && root.row !== null && root.row.can_undo
            onClicked: root._edit("bindings_undo", { mode: root.view.selected })
        }
        PrimaryButton {
            //% "Save"
            text: qsTrId("light_strip_save")
            iconName: "save"
            busy: root.busy
            enabled: root.usable && root.row !== null && root.row.changed
            onClicked: root.save()
        }
        SecondaryButton {
            //% "Show on my laptop"
            text: qsTrId("light_strip_show")
            enabled: root.usable && root.row !== null && root.row.look !== null
            onClicked: Api.call("look", "apply", { name: root.row.look }, function () {
                //% "Look applied to the lights, not to the power mode"
                Toasts.show(qsTrId("light_strip_shown"), "info")
            })
        }
    }

    RowLayout {
        Layout.fillWidth: true
        spacing: 8
        Icon { name: "fan"; size: 16; color: Theme.muted }
        Text {
            Layout.fillWidth: true
            text: root.row === null ? "" : (root.row.curve !== null
                //% "Fan curve for this mode: %1"
                ? qsTrId("light_strip_curve").arg(root.row.curve)
                //% "No fan curve is bound to this mode."
                : qsTrId("light_strip_no_curve"))
            color: Theme.muted
            font.pixelSize: Theme.fontSmall + 1
            wrapMode: Text.Wrap
        }
        SecondaryButton {
            //% "Change in Cooling"
            text: qsTrId("light_strip_cooling")
            onClicked: Nav.go("cooling")
        }
    }

    HelpText {
        //% "The service applies the look when the power mode changes, also at boot and by Smart auto. Moving pictures need the key service."
        text: qsTrId("light_strip_help")
    }
}
