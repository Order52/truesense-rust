// The first-run wizard: welcome, a health check with plain-language fixes, theme, done.
// It covers the window it is placed in (the shell puts it over everything) and emits `finished()`
// when the person finishes or skips; the shell then stops showing it, and this file has already
// remembered that in ui.json (`Ui.set("onboarded", true)`), so it appears once.
//
// Skip is on every step and counts as having seen it. No step is blocked by a failing check: a
// laptop without the driver still gets a window to read the explanation in. The wizard never
// answers a password prompt: a repair that needs administrator rights is shown as the command to
// copy and run in a terminal. Steps, grading and fixes are decided in ts-ui-model
// (vm/onboarding.rs, tested); this file draws them.
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import org.truesense.app

Item {
    id: root

    /// The wizard is over (finished or skipped); it has been remembered already.
    signal finished()

    property var wiz: vm("start", {})
    property var doctor: null
    property var services: null
    property var capabilities: null
    property bool checking: false
    property bool repairing: false
    property bool started: false
    readonly property var health: vm("health", { doctor: doctor, services: services, capabilities: capabilities })

    function vm(verb, args) {
        var r = JSON.parse(Bus.vm("onboarding", verb, JSON.stringify(args)))
        return r.ok ? r.result : null
    }

    function close() {
        // Remembered before anything else: a crash after this must not show the wizard again.
        Ui.set("onboarded", true)
        finished()
    }

    function go(verb) {
        var s = vm(verb, { step: wiz.step })
        if (s.closed) { close(); return }
        wiz = s
        if (s.step === "health" && !started) runChecks()
    }

    function failure(e) { return { error: e && e.message ? e.message : "" } }

    /// Reads the three answers the health step needs; each fills in as it arrives (the doctor is the
    /// slow one). Nothing here changes the machine.
    function runChecks() {
        started = true
        checking = true
        doctor = null
        services = null
        capabilities = null
        var waiting = 3
        var one = function () { if (--waiting === 0) checking = false }
        Api.call("doctor", "run", {}, function (r) { doctor = r; one() }, function (e) { doctor = failure(e); one() })
        Api.call("service", "check", {}, function (r) { services = r; one() }, function (e) { services = failure(e); one() })
        Api.call("capabilities", "get", {}, function (r) { capabilities = r; one() }, function (e) { capabilities = failure(e); one() })
    }

    /// The user part of the repair: starts the person's own services, no password needed.
    function repair() {
        repairing = true
        Api.call("service", "repair", {}, function (steps) {
            repairing = false
            var r = vm("repair", { steps: steps })
            if (r.failed.length === 0) {
                //% "Done. Checking again."
                Toasts.ok(qsTrId("onb_repaired"))
            } else {
                Toasts.error(r.failed[0])
            }
            runChecks()
        }, function (e) {
            repairing = false
            Toasts.error(e.message)
        })
    }

    function copyText(text) {
        clip.text = text
        clip.selectAll()
        clip.copy()
        //% "Copied."
        Toasts.ok(qsTrId("onb_copied"))
    }

    TextEdit { id: clip; visible: false }

    // ---- the words for a check row ------------------------------------------------------------
    function rowTitle(id) {
        switch (id) {
        //% "Driver"
        case "module": return qsTrId("onb_row_module")
        //% "System service"
        case "daemon": return qsTrId("onb_row_daemon")
        //% "Key service"
        case "keys": return qsTrId("onb_row_keys")
        //% "Background helpers"
        case "background": return qsTrId("onb_row_background")
        //% "Health check"
        case "doctor": return qsTrId("onb_row_doctor")
        //% "This laptop"
        case "features": return qsTrId("onb_row_features")
        default: return id
        }
    }

    function rowText(r) {
        if (r.state === "pending")
            //% "Checking…"
            return qsTrId("onb_checking")
        if (r.state === "unknown")
            //% "Could not be checked."
            return qsTrId("onb_unknown")
        var ok = r.state === "ok"
        switch (r.id) {
        case "module":
            //% "The TrueSense driver is loaded."
            return ok ? qsTrId("onb_module_ok")
                //% "The TrueSense driver is not loaded, so fans, power modes and keys cannot be controlled."
                : qsTrId("onb_module_bad")
        case "daemon":
            //% "The TrueSense system service is running."
            return ok ? qsTrId("onb_daemon_ok")
                : r.state === "warn"
                    //% "The system service is running but does not answer."
                    ? qsTrId("onb_daemon_silent")
                    //% "The TrueSense system service is not running, so the app cannot change anything."
                    : qsTrId("onb_daemon_bad")
        case "keys":
            //% "The key service is running: the PREDATOR key works."
            return ok ? qsTrId("onb_keys_ok")
                //% "The key service is not running, so the PREDATOR key does nothing."
                : qsTrId("onb_keys_bad")
        case "background":
            //% "The background helpers are running."
            return ok ? qsTrId("onb_bg_ok")
                //% "Some background helpers are not running: %1."
                : qsTrId("onb_bg_bad").arg(r.detail === null ? "" : r.detail)
        case "doctor":
            //% "The health check found nothing wrong."
            return ok ? qsTrId("onb_doctor_ok")
                : r.state === "warn"
                    //% "The health check has warnings (%1)."
                    ? qsTrId("onb_doctor_warn").arg(r.warnings)
                    //% "The health check found problems (%1)."
                    : qsTrId("onb_doctor_bad").arg(r.failures)
        case "features":
            //% "This laptop is supported: %1 of %2 features are available."
            return ok ? qsTrId("onb_features_ok").arg(r.supported).arg(r.total)
                : r.state === "warn"
                    //% "This model has not been verified: some features may not work."
                    ? qsTrId("onb_features_warn")
                    //% "This model is not supported by TrueSense."
                    : qsTrId("onb_features_bad")
        default: return ""
        }
    }

    function stateColor(state) {
        return state === "ok" ? Theme.ok : state === "warn" ? Theme.warn : state === "fail" ? Theme.bad : Theme.muted
    }

    function stateIcon(state) {
        return state === "ok" ? "check" : state === "warn" ? "warn" : state === "fail" ? "close" : "info"
    }

    // ---- layout -------------------------------------------------------------------------------
    Rectangle {
        anchors.fill: parent
        color: Theme.scrim
        // Absorbs clicks so nothing behind the wizard is touched.
        MouseArea { anchors.fill: parent; hoverEnabled: true; onWheel: (w) => w.accepted = true }
    }

    Rectangle {
        id: card
        anchors.centerIn: parent
        width: Math.min(640, root.width - 32)
        height: Math.min(root.height - 32, 640, column.implicitHeight + 2 * (Theme.pad + 4))
        radius: Theme.radiusLarge
        color: Theme.dialog
        border.color: Theme.cardLine
        border.width: 1
        Accessible.role: Accessible.Dialog
        //% "Welcome to TrueSense"
        Accessible.name: qsTrId("onb_title")

        ColumnLayout {
            id: column
            anchors.fill: parent
            anchors.margins: Theme.pad + 4
            spacing: Theme.spacing

            // Progress and Skip: always visible.
            RowLayout {
                Layout.fillWidth: true
                spacing: 6
                Repeater {
                    model: root.wiz.total
                    delegate: Rectangle {
                        required property int index
                        Layout.preferredWidth: index === root.wiz.index ? 22 : 8
                        Layout.preferredHeight: 8
                        radius: 4
                        color: index <= root.wiz.index ? Theme.accent : Theme.groove
                        Behavior on Layout.preferredWidth { NumberAnimation { duration: Theme.animMs } }
                    }
                }
                Item { Layout.fillWidth: true }
                SecondaryButton {
                    //% "Skip"
                    text: qsTrId("onb_skip")
                    //% "Skip the welcome tour"
                    Accessible.name: qsTrId("onb_skip_named")
                    onClicked: root.close()
                }
            }

            Flickable {
                id: scroll
                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.preferredHeight: body.implicitHeight
                contentWidth: width
                contentHeight: body.implicitHeight
                clip: true
                boundsBehavior: Flickable.StopAtBounds
                flickableDirection: Flickable.VerticalFlick
                ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

                StackLayout {
                    id: body
                    width: scroll.width
                    currentIndex: root.wiz.index

                    // ---- welcome ----
                    ColumnLayout {
                        spacing: Theme.spacing
                        Image {
                            Layout.alignment: Qt.AlignHCenter
                            source: "qrc:/assets/truesense-256.png"
                            sourceSize.width: 96
                            sourceSize.height: 96
                            Layout.preferredWidth: 96
                            Layout.preferredHeight: 96
                            smooth: true
                        }
                        Text {
                            Layout.fillWidth: true
                            //% "Welcome to TrueSense"
                            text: qsTrId("onb_title")
                            color: Theme.text
                            font.pixelSize: Theme.fontTitle
                            font.bold: true
                            horizontalAlignment: Text.AlignHCenter
                            wrapMode: Text.Wrap
                            Accessible.role: Accessible.Heading
                        }
                        Text {
                            Layout.fillWidth: true
                            //% "TrueSense controls the fans, power modes, lighting and PREDATOR key of this laptop. This short tour checks that everything it needs is in place. It takes a minute, and you can skip it at any time."
                            text: qsTrId("onb_welcome")
                            color: Theme.stat
                            font.pixelSize: Theme.fontBody
                            horizontalAlignment: Text.AlignHCenter
                            wrapMode: Text.Wrap
                        }
                    }

                    // ---- health ----
                    ColumnLayout {
                        spacing: Theme.spacing
                        Text {
                            Layout.fillWidth: true
                            //% "Is everything in place?"
                            text: qsTrId("onb_health_title")
                            color: Theme.text
                            font.pixelSize: Theme.fontSection + 3
                            font.bold: true
                            wrapMode: Text.Wrap
                            Accessible.role: Accessible.Heading
                        }
                        Repeater {
                            model: root.health.rows
                            delegate: RowLayout {
                                id: checkRow
                                required property var modelData
                                Layout.fillWidth: true
                                spacing: 12
                                Accessible.name: root.rowTitle(modelData.id) + ": " + root.rowText(modelData)

                                Rectangle {
                                    Layout.alignment: Qt.AlignTop
                                    Layout.preferredWidth: 28
                                    Layout.preferredHeight: 28
                                    radius: 14
                                    color: Theme.tile
                                    border.width: 1
                                    border.color: root.stateColor(checkRow.modelData.state)
                                    Icon {
                                        anchors.centerIn: parent
                                        name: root.stateIcon(checkRow.modelData.state)
                                        size: 16
                                        color: root.stateColor(checkRow.modelData.state)
                                    }
                                }
                                ColumnLayout {
                                    Layout.fillWidth: true
                                    spacing: 2
                                    Text {
                                        text: root.rowTitle(checkRow.modelData.id)
                                        color: Theme.text
                                        font.pixelSize: Theme.fontBody
                                        font.bold: true
                                    }
                                    Text {
                                        Layout.fillWidth: true
                                        text: root.rowText(checkRow.modelData)
                                        color: Theme.stat
                                        font.pixelSize: Theme.fontSmall + 1
                                        wrapMode: Text.Wrap
                                    }
                                    Text {
                                        Layout.fillWidth: true
                                        // The words of the service states ("inactive") mean nothing to most
                                        // people: only the health check's and the model's own text is shown.
                                        visible: checkRow.modelData.state !== "ok" && checkRow.modelData.state !== "pending"
                                            && checkRow.modelData.detail !== null
                                            && (checkRow.modelData.id === "doctor" || checkRow.modelData.id === "features")
                                        text: checkRow.modelData.detail === null ? "" : checkRow.modelData.detail
                                        color: Theme.muted
                                        font.pixelSize: Theme.fontSmall
                                        wrapMode: Text.Wrap
                                    }
                                    // A fix the check names that is neither the user repair nor a
                                    // root command is shown as text to follow.
                                    Text {
                                        Layout.fillWidth: true
                                        visible: checkRow.modelData.fix !== null && checkRow.modelData.fix.auto !== true
                                            && checkRow.modelData.fix.command.indexOf("sudo ") !== 0
                                        //% "To fix: %1"
                                        text: checkRow.modelData.fix === null ? "" : qsTrId("onb_fix_text").arg(checkRow.modelData.fix.command)
                                        color: Theme.warn
                                        font.pixelSize: Theme.fontSmall
                                        wrapMode: Text.Wrap
                                    }
                                }
                            }
                        }

                        // The one repair the window can do alone.
                        RowLayout {
                            Layout.fillWidth: true
                            visible: root.health.can_repair
                            spacing: 10
                            Text {
                                Layout.fillWidth: true
                                //% "Some of your own services are not running. This starts them; no password is needed."
                                text: qsTrId("onb_repair_text")
                                color: Theme.stat
                                font.pixelSize: Theme.fontSmall + 1
                                wrapMode: Text.Wrap
                            }
                            PrimaryButton {
                                //% "Fix it"
                                text: qsTrId("onb_repair")
                                iconName: "refresh"
                                busy: root.repairing
                                onClicked: root.repair()
                            }
                        }

                        // What needs administrator rights is never run from here.
                        Rectangle {
                            Layout.fillWidth: true
                            visible: root.health.system_command !== null
                            implicitHeight: sudoColumn.implicitHeight + 24
                            radius: Theme.radius
                            color: Theme.tile
                            border.width: 1
                            border.color: Theme.warn
                            ColumnLayout {
                                id: sudoColumn
                                anchors.fill: parent
                                anchors.margins: 12
                                spacing: 8
                                Text {
                                    Layout.fillWidth: true
                                    //% "Fixing the rest needs administrator rights. Open a terminal and run:"
                                    text: qsTrId("onb_sudo_text")
                                    color: Theme.text
                                    font.pixelSize: Theme.fontBody
                                    wrapMode: Text.Wrap
                                }
                                RowLayout {
                                    Layout.fillWidth: true
                                    spacing: 8
                                    Text {
                                        Layout.fillWidth: true
                                        text: root.health.system_command === null ? "" : root.health.system_command
                                        color: Theme.text
                                        font.family: "monospace"
                                        font.pixelSize: Theme.fontBody
                                        wrapMode: Text.WrapAnywhere
                                    }
                                    SecondaryButton {
                                        //% "Copy"
                                        text: qsTrId("onb_copy")
                                        iconName: "copy"
                                        onClicked: root.copyText(root.health.system_command)
                                    }
                                }
                            }
                        }

                        RowLayout {
                            Layout.fillWidth: true
                            SecondaryButton {
                                //% "Check again"
                                text: qsTrId("onb_recheck")
                                iconName: "refresh"
                                busy: root.checking
                                onClicked: root.runChecks()
                            }
                            Item { Layout.fillWidth: true }
                        }
                        HelpText {
                            //% "None of this stops you: you can continue and fix it later on the Health page."
                            text: qsTrId("onb_health_later")
                        }
                    }

                    // ---- theme ----
                    ColumnLayout {
                        spacing: Theme.spacing
                        Text {
                            Layout.fillWidth: true
                            //% "Make it yours"
                            text: qsTrId("onb_look_title")
                            color: Theme.text
                            font.pixelSize: Theme.fontSection + 3
                            font.bold: true
                            wrapMode: Text.Wrap
                            Accessible.role: Accessible.Heading
                        }
                        SettingRow {
                            Layout.fillWidth: true
                            //% "Theme"
                            label: qsTrId("onb_theme")
                            Segmented {
                                model: [
                                    //% "Dark"
                                    { value: "dark", label: qsTrId("onb_theme_dark") },
                                    //% "Light"
                                    { value: "light", label: qsTrId("onb_theme_light") },
                                    //% "System"
                                    { value: "system", label: qsTrId("onb_theme_system") }
                                ]
                                current: Ui.theme
                                onActivated: (v) => Ui.set("theme", v)
                            }
                        }
                        HelpText {
                            //% "You can change both later in Settings."
                            text: qsTrId("onb_look_later")
                        }
                    }

                    // ---- done ----
                    ColumnLayout {
                        spacing: Theme.spacing
                        Icon {
                            Layout.alignment: Qt.AlignHCenter
                            name: "check"
                            size: 48
                            color: Theme.ok
                        }
                        Text {
                            Layout.fillWidth: true
                            //% "You are all set"
                            text: qsTrId("onb_done_title")
                            color: Theme.text
                            font.pixelSize: Theme.fontTitle
                            font.bold: true
                            horizontalAlignment: Text.AlignHCenter
                            wrapMode: Text.Wrap
                            Accessible.role: Accessible.Heading
                        }
                        Text {
                            Layout.fillWidth: true
                            //% "Pick a power mode on Home, set up the PREDATOR key on the Keys page, and find this laptop's health check on the Health page. TrueSense keeps running in the tray when you close the window."
                            text: qsTrId("onb_done_text")
                            color: Theme.stat
                            font.pixelSize: Theme.fontBody
                            horizontalAlignment: Text.AlignHCenter
                            wrapMode: Text.Wrap
                        }
                    }
                }
            }

            RowLayout {
                Layout.fillWidth: true
                spacing: 8
                SecondaryButton {
                    visible: root.wiz.can_back
                    //% "Back"
                    text: qsTrId("onb_back")
                    onClicked: root.go("back")
                }
                Item { Layout.fillWidth: true }
                PrimaryButton {
                    text: root.wiz.is_last
                        //% "Finish"
                        ? qsTrId("onb_finish")
                        //% "Continue"
                        : qsTrId("onb_next")
                    onClicked: root.go("next")
                }
            }
        }
    }
}
