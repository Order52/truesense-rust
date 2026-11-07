// Health: the system check (doctor) with its fixes, the services, the hardware self-test, the
// daemon's log, the configuration check and the table of what this machine supports. The doctor and
// the self-test run as Jobs (progress shown, never blocking, the answer kept while the person looks
// at another page).
import QtQuick
import QtQuick.Layouts
import org.truesense.app

PageFrame {
    id: page
    title: Nav.currentPage ? Nav.titleOf(Nav.currentPage) : ""

    readonly property var doctorJob: Jobs.get("doctor")
    readonly property var selftestJob: Jobs.get("selftest")
    readonly property var doc: doctorJob.done && doctorJob.ok ? vm("doctor", doctorJob.result) : null
    readonly property var test: selftestJob.done && selftestJob.ok ? vm("selftest", selftestJob.result) : null
    property var services: null
    property bool servicesFailed: false
    property var repair: null
    property bool repairBusy: false
    property var logTail: null
    property string logError: ""
    property bool logBusy: false
    property var config: null
    property bool configBusy: false
    property var caps: ({ available: false, items: [] })
    property bool skipFans: true
    readonly property var life: ({ alive: true })
    Component.onDestruction: life.alive = false

    function guarded(fn) {
        var l = life
        return function (x) { if (l.alive) fn(x) }
    }
    function vm(verb, args) {
        try {
            var r = JSON.parse(Bus.vm("health", verb, JSON.stringify({ value: args })))
            return r.ok ? r.result : null
        } catch (e) { return null }
    }

    function loadServices() {
        Api.call("service", "check", {}, guarded(function (r) {
            page.services = vm("services", r)
            page.servicesFailed = false
        }), guarded(function () { page.servicesFailed = true }))
    }
    function loadCaps() {
        Api.call("capabilities", "get", {}, guarded(function (r) { page.caps = vm("capabilities", r) }),
                 guarded(function () { page.caps = vm("capabilities", null) }))
    }
    function runDoctor() { Jobs.start("doctor", "doctor", "run", {}) }

    Component.onCompleted: {
        if (!Jobs.get("doctor").done && !Jobs.busy("doctor")) runDoctor()
        loadServices()
        loadCaps()
    }

    ConfirmDialog { id: confirm }
    // The clipboard has no QML API of its own; a hidden text field copies for us.
    TextEdit { id: clip; visible: false }

    function copyText(text) {
        clip.text = text
        clip.selectAll()
        clip.copy()
        //% "Copied to the clipboard."
        Toasts.ok(qsTrId("health_copied"))
    }

    function levelText(level) {
        if (level === "fail")
            //% "Something needs fixing"
            return qsTrId("health_level_fail")
        if (level === "warn")
            //% "Works, with warnings"
            return qsTrId("health_level_warn")
        //% "All good"
        return qsTrId("health_level_ok")
    }
    function badgeKind(status) { return status === "PASS" ? "ok" : (status === "FAIL" ? "fail" : (status === "SKIP" ? "warn" : "info")) }

    // ---- system check -------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "System check"
        title: qsTrId("health_doctor")
        icon: "health"
        rightText: page.doc ? page.doc.version : ""

        LongRunning {
            job: page.doctorJob
            //% "Checking the system"
            text: qsTrId("health_doctor_running")
            onStopWaiting: Jobs.cancelWaiting("doctor")
        }
        ReadbackNote {
            visible: page.doctorJob.done && !page.doctorJob.ok
            kind: "error"
            text: page.doctorJob.error ? page.doctorJob.error.message : ""
        }
        RowLayout {
            Layout.fillWidth: true
            visible: page.doc !== null
            spacing: Theme.spacing
            Badge {
                text: page.doc ? page.levelText(page.doc.level) : ""
                kind: page.doc ? (page.doc.level === "ok" ? "ok" : page.doc.level) : "info"
            }
            Text {
                Layout.fillWidth: true
                color: Theme.muted
                font.pixelSize: Theme.fontBody
                wrapMode: Text.Wrap
                text: page.doc
                    //% "%1 problems, %2 warnings. Score %3 of 100. Kernel %4."
                    ? qsTrId("health_doctor_counts").arg(page.doc.failures).arg(page.doc.warnings)
                          .arg(page.doc.score === null ? "–" : page.doc.score).arg(page.doc.kernel)
                    : ""
            }
        }
        Repeater {
            model: page.doc ? page.doc.checks : []
            delegate: ColumnLayout {
                id: check
                required property var modelData
                Layout.fillWidth: true
                spacing: 2
                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8
                    Badge {
                        Layout.alignment: Qt.AlignTop
                        Layout.preferredWidth: 56
                        text: check.modelData.status
                        kind: check.modelData.badge
                    }
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 0
                        Text {
                            Layout.fillWidth: true
                            text: check.modelData.check
                            color: Theme.text
                            font.pixelSize: Theme.fontBody
                            font.bold: true
                            wrapMode: Text.Wrap
                        }
                        HelpText { text: check.modelData.detail; visible: text !== "" }
                        HelpText {
                            visible: check.modelData.show_fix
                            //% "Fix: %1"
                            text: qsTrId("health_fix").arg(check.modelData.fix)
                            color: Theme.warn
                        }
                    }
                }
            }
        }
        RowLayout {
            Layout.fillWidth: true
            spacing: Theme.spacing
            PrimaryButton {
                //% "Run check"
                text: qsTrId("health_run_check")
                enabled: !Jobs.busy("doctor")
                onClicked: page.runDoctor()
            }
            SecondaryButton {
                //% "Copy report"
                text: qsTrId("health_copy_report")
                iconName: "copy"
                enabled: page.doc !== null
                onClicked: page.copyText(page.doc.report)
            }
            Item { Layout.fillWidth: true }
        }
        HelpText {
            //% "Reads only: nothing is changed."
            text: qsTrId("health_doctor_sub")
        }
    }

    // ---- services -----------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Services"
        title: qsTrId("health_services")
        icon: "chip"

        HelpText {
            visible: page.servicesFailed
            //% "The services could not be read."
            text: qsTrId("health_services_failed")
        }
        Repeater {
            model: page.services ? page.services.rows : []
            delegate: RowLayout {
                id: svc
                required property var modelData
                Layout.fillWidth: true
                spacing: 8
                Badge {
                    Layout.alignment: Qt.AlignTop
                    Layout.preferredWidth: 56
                    //% "OK"
                    text: svc.modelData.ok ? qsTrId("health_svc_ok")
                                           //% "Down"
                                           : qsTrId("health_svc_down")
                    kind: svc.modelData.ok ? "ok" : "fail"
                }
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 0
                    Text {
                        Layout.fillWidth: true
                        text: svc.modelData.unit
                        color: Theme.text
                        font.pixelSize: Theme.fontBody
                        font.bold: true
                        elide: Text.ElideRight
                    }
                    HelpText { text: svc.modelData.detail; visible: text !== "" }
                    HelpText {
                        visible: svc.modelData.fix !== ""
                        text: qsTrId("health_fix").arg(svc.modelData.fix)
                        color: Theme.warn
                    }
                }
            }
        }
        HelpText {
            visible: page.services !== null
            text: page.services && page.services.all_ok
                //% "Everything that should run is running."
                ? qsTrId("health_services_ok")
                //% "Something is not running: use Repair services, or run `sudo truesense service repair`."
                : qsTrId("health_services_bad")
        }
        SettingRow {
            Layout.fillWidth: true
            //% "Repair"
            label: qsTrId("health_repair")
            //% "Gets your user services running again. Safe to repeat; touches no setting. The system part needs `sudo truesense service repair` (a window cannot ask for your password)."
            sub: qsTrId("health_repair_sub")
            AppButton {
                //% "Repair services"
                text: qsTrId("health_repair_button")
                busy: page.repairBusy
                onClicked: {
                    page.repairBusy = true
                    Api.call("service", "repair", {}, page.guarded(function (r) {
                        page.repairBusy = false
                        page.repair = page.vm("repair", r)
                        page.loadServices()
                    }), page.guarded(function (err) {
                        page.repairBusy = false
                        Toasts.error(err.message)
                    }))
                }
            }
        }
        Repeater {
            model: page.repair ? page.repair.rows : []
            delegate: ReadbackNote {
                required property var modelData
                kind: modelData.ok ? "ok" : "error"
                text: modelData.step
            }
        }
        HelpText {
            visible: page.repair !== null && page.repair.empty
            //% "There was nothing to repair."
            text: qsTrId("health_repair_nothing")
        }
    }

    // ---- self-test ----------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Hardware self-test"
        title: qsTrId("health_selftest")
        icon: "sliders"

        HelpText {
            //% "Sets each firmware feature, reads it back and puts it back as it was. Takes about a minute."
            text: qsTrId("health_selftest_sub")
        }
        ToggleRow {
            Layout.fillWidth: true
            //% "Leave the fans alone"
            label: qsTrId("health_skip_fans")
            //% "Skips the fan checks. Without this, the fans are held at a fixed speed for a moment and handed back to the firmware afterwards."
            sub: qsTrId("health_skip_fans_sub")
            checked: page.skipFans
            enabled: !Jobs.busy("selftest")
            onToggled: (on) => page.skipFans = on
        }
        RowLayout {
            Layout.fillWidth: true
            PrimaryButton {
                //% "Run self-test…"
                text: qsTrId("health_selftest_run")
                enabled: !Jobs.busy("selftest")
                onClicked: confirm.ask({
                    //% "Hardware self-test"
                    title: qsTrId("health_selftest_title"),
                    text: page.skipFans
                        //% "This briefly changes power mode, lighting, battery and display settings and restores them. The fans are not touched. Run it now?"
                        ? qsTrId("health_selftest_text_skip")
                        //% "This briefly changes power mode, lighting, battery and display settings and restores them. The fans are held at a fixed speed for a moment and handed back to the firmware. Run it now?"
                        : qsTrId("health_selftest_text_fans"),
                    //% "Run it"
                    confirmText: qsTrId("health_selftest_confirm")
                }, function () { Jobs.start("selftest", "selftest", "run", { skip_fans: page.skipFans }) })
            }
            Item { Layout.fillWidth: true }
        }
        LongRunning {
            job: page.selftestJob
            //% "Self-test running"
            text: qsTrId("health_selftest_running")
            onStopWaiting: Jobs.cancelWaiting("selftest")
        }
        HelpText {
            visible: page.selftestJob.running
            //% "The results appear when the self-test has finished. It puts back what it changed by itself."
            text: qsTrId("health_selftest_wait")
        }
        ReadbackNote {
            visible: page.selftestJob.done && !page.selftestJob.ok
            kind: "error"
            text: page.selftestJob.error ? page.selftestJob.error.message : ""
        }
        RowLayout {
            Layout.fillWidth: true
            visible: page.test !== null
            spacing: Theme.spacing
            Badge {
                text: page.test && page.test.ok
                    //% "All passed"
                    ? qsTrId("health_selftest_ok")
                    //% "Some checks failed"
                    : qsTrId("health_selftest_bad")
                kind: page.test && page.test.ok ? "ok" : "fail"
            }
            Text {
                Layout.fillWidth: true
                color: Theme.muted
                font.pixelSize: Theme.fontBody
                wrapMode: Text.Wrap
                text: page.test
                    //% "%1 passed, %2 failed, %3 skipped."
                    ? qsTrId("health_selftest_tally").arg(page.test.passed).arg(page.test.failed).arg(page.test.skipped)
                    : ""
            }
        }
        ReadbackNote {
            visible: page.test !== null && page.test.restore_problem
            kind: "error"
            //% "A setting could not be put back. Check the lines marked RESTORE FAILED below and set it by hand."
            text: qsTrId("health_selftest_restore")
        }
        Repeater {
            model: page.test ? page.test.rows : []
            delegate: RowLayout {
                id: tline
                required property var modelData
                Layout.fillWidth: true
                spacing: 8
                Badge {
                    Layout.alignment: Qt.AlignTop
                    Layout.preferredWidth: 56
                    visible: tline.modelData.status !== "INFO"
                    text: tline.modelData.status
                    kind: page.badgeKind(tline.modelData.status)
                }
                Text {
                    Layout.fillWidth: true
                    text: tline.modelData.name + (tline.modelData.detail !== "" ? " — " + tline.modelData.detail : "")
                    color: tline.modelData.status === "FAIL" ? Theme.bad : (tline.modelData.status === "INFO" ? Theme.muted : Theme.text)
                    font.pixelSize: Theme.fontBody
                    wrapMode: Text.Wrap
                }
            }
        }
    }

    // ---- log ----------------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Service log"
        title: qsTrId("health_log")
        icon: "info"

        RowLayout {
            Layout.fillWidth: true
            AppButton {
                text: page.logTail
                    //% "Refresh log"
                    ? qsTrId("health_log_refresh")
                    //% "Show log"
                    : qsTrId("health_log_show")
                busy: page.logBusy
                onClicked: {
                    page.logBusy = true
                    page.logError = ""
                    Api.call("service", "logs", {}, page.guarded(function (r) {
                        page.logBusy = false
                        page.logTail = page.vm("log_tail", r)
                        logScroll.contentY = Math.max(0, logScroll.contentHeight - logScroll.height)
                    }), page.guarded(function (err) {
                        page.logBusy = false
                        page.logError = err.message
                    }))
                }
            }
            Item { Layout.fillWidth: true }
        }
        ReadbackNote {
            visible: page.logError !== ""
            kind: "error"
            text: page.logError
        }
        HelpText {
            visible: page.logTail !== null && page.logTail.count === 0
            //% "No log lines."
            text: qsTrId("health_log_empty")
        }
        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: 240
            visible: page.logTail !== null && page.logTail.count > 0
            radius: Theme.radiusSmall
            color: Theme.field
            border.color: Theme.cardLine
            LayoutMirroring.enabled: false

            Flickable {
                id: logScroll
                anchors.fill: parent
                anchors.margins: 8
                clip: true
                contentWidth: logColumn.width
                contentHeight: logColumn.height
                boundsBehavior: Flickable.StopAtBounds
                Column {
                    id: logColumn
                    width: Math.max(logScroll.width, 1)
                    Repeater {
                        model: page.logTail ? page.logTail.lines : []
                        delegate: Text {
                            required property var modelData
                            width: logColumn.width
                            text: modelData.text
                            wrapMode: Text.WrapAnywhere
                            font.family: "monospace"
                            font.pixelSize: Theme.fontSmall
                            color: modelData.level === "error" ? Theme.bad : (modelData.level === "warn" ? Theme.warn : Theme.stat)
                        }
                    }
                }
            }
        }
        HelpText {
            visible: page.logTail !== null && page.logTail.truncated
            //% "Only the newest lines are shown."
            text: qsTrId("health_log_truncated")
        }
    }

    // ---- configuration ------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Configuration"
        title: qsTrId("health_config")
        icon: "settings"

        SettingRow {
            Layout.fillWidth: true
            //% "Check the configuration files"
            label: qsTrId("health_config_label")
            //% "Finds mistakes in the settings files without changing anything."
            sub: qsTrId("health_config_sub")
            AppButton {
                //% "Check"
                text: qsTrId("health_config_check")
                busy: page.configBusy
                onClicked: {
                    page.configBusy = true
                    Api.call("config", "validate", {}, page.guarded(function (r) {
                        page.configBusy = false
                        page.config = page.vm("config", r)
                    }), page.guarded(function (err) {
                        page.configBusy = false
                        page.config = { valid: false, problems: [err.message] }
                    }))
                }
            }
        }
        ReadbackNote {
            visible: page.config !== null && page.config.valid
            //% "The configuration is valid."
            text: qsTrId("health_config_valid")
        }
        Repeater {
            model: page.config && !page.config.valid ? page.config.problems : []
            delegate: ReadbackNote {
                required property var modelData
                kind: "error"
                text: modelData
            }
        }
    }

    // ---- capabilities -------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Supported features"
        title: qsTrId("health_caps")
        icon: "check"
        //% "%1 of %2 supported"
        rightText: page.caps.available ? qsTrId("health_caps_count").arg(page.caps.supported).arg(page.caps.total) : ""

        CapabilityChecklist {
            info: page.caps
            detailed: true
        }
        SecondaryButton {
            //% "Read again"
            text: qsTrId("health_caps_reread")
            iconName: "refresh"
            onClicked: page.loadCaps()
        }
    }
}
