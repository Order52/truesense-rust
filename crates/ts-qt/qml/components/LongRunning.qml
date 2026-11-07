// The progress line of a long command (see Jobs): an indeterminate bar, the seconds it has been
// running and a "Stop waiting" button. Hidden while the job is not running. Its timer runs only
// while a job is running and the item is visible, so a quiet page costs nothing.
//
//   LongRunning { job: Jobs.get("selftest"); text: "Self-test running"; onStopWaiting: Jobs.cancelWaiting("selftest") }
import QtQuick
import QtQuick.Layouts
import org.truesense.app

ColumnLayout {
    id: root

    /// The record from Jobs.get(id).
    property var job: Jobs.idle
    /// What is happening, without the time ("Setting up DTS on every output").
    property string text: ""
    /// Seconds since the job started.
    property int elapsed: 0
    signal stopWaiting()

    visible: job.running
    Layout.fillWidth: true
    spacing: 6
    Accessible.role: Accessible.ProgressBar
    Accessible.name: root.text

    onJobChanged: elapsed = job.running ? Math.max(0, Math.round((Date.now() - job.startedMs) / 1000)) : 0

    Timer {
        interval: 1000
        repeat: true
        running: root.job.running && root.visible
        onTriggered: root.elapsed = Math.max(0, Math.round((Date.now() - root.job.startedMs) / 1000))
    }

    RowLayout {
        Layout.fillWidth: true
        spacing: Theme.spacing
        Text {
            Layout.fillWidth: true
            text: root.text + " … " + Fmt.duration(root.elapsed)
            color: Theme.text
            font.pixelSize: Theme.fontBody
            wrapMode: Text.Wrap
        }
        AppButton {
            //% "Stop waiting"
            text: qsTrId("job_stop_waiting")
            onClicked: root.stopWaiting()
        }
    }
    Rectangle {
        id: track
        Layout.fillWidth: true
        implicitHeight: 6
        radius: 3
        color: Theme.groove
        clip: true
        LayoutMirroring.enabled: false

        Rectangle {
            id: bar
            width: Math.max(24, track.width * 0.25)
            height: parent.height
            radius: 3
            color: Theme.accent
            x: Ui.reducedMotion ? (track.width - width) / 2 : 0
            SequentialAnimation on x {
                running: root.job.running && root.visible && !Ui.reducedMotion
                loops: Animation.Infinite
                NumberAnimation { from: 0; to: Math.max(0, track.width - bar.width); duration: 900; easing.type: Easing.InOutQuad }
                NumberAnimation { from: Math.max(0, track.width - bar.width); to: 0; duration: 900; easing.type: Easing.InOutQuad }
            }
        }
    }
    HelpText {
        //% "The command keeps running on the machine if you stop waiting; nothing is cut off half way."
        text: qsTrId("job_stop_note")
    }
}
