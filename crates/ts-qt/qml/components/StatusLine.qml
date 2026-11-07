// The thin bar at the bottom of the window: is the service running, and the few live numbers
// (CPU and GPU temperature, fan speed) so the person sees the machine on every page.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

Rectangle {
    id: root

    /// Show the live numbers (hidden when the window is very narrow).
    property bool compact: false
    readonly property bool up: Live.daemonUp

    implicitHeight: 30
    color: Theme.sidebar
    Rectangle { width: parent.width; height: 1; color: Theme.sep }
    Accessible.role: Accessible.StatusBar

    RowLayout {
        anchors.fill: parent
        anchors.leftMargin: Theme.pad
        anchors.rightMargin: Theme.pad
        spacing: 14

        Rectangle {
            Layout.alignment: Qt.AlignVCenter
            width: 8
            height: 8
            radius: 4
            color: !Live.ok ? Theme.muted : (root.up ? Theme.ok : Theme.warn)
        }
        Text {
            //% "Reading the laptop…"
            text: !Live.ok ? qsTrId("status_reading")
                //% "TrueSense service running"
                : root.up ? qsTrId("status_service_up")
                //% "TrueSense service not running"
                : qsTrId("status_service_down")
            color: Theme.muted
            font.pixelSize: Theme.fontSmall
            elide: Text.ElideRight
            Layout.fillWidth: root.compact
        }
        Item { Layout.fillWidth: !root.compact }
        Text {
            visible: !root.compact && Live.ok
            //% "CPU %1"
            text: qsTrId("status_cpu").arg(Fmt.temp(Live.value("sensors.cpu_temp")))
            color: Theme.stat
            font.pixelSize: Theme.fontSmall
        }
        Text {
            visible: !root.compact && Live.ok
            //% "GPU %1"
            text: qsTrId("status_gpu").arg(Fmt.temp(Live.value("sensors.gpu_temp")))
            color: Theme.stat
            font.pixelSize: Theme.fontSmall
        }
        Text {
            visible: !root.compact && Live.ok
            //% "Fans %1"
            text: qsTrId("status_fans").arg(Fmt.rpm(Live.value("sensors.cpu_fan_rpm")))
            color: Theme.stat
            font.pixelSize: Theme.fontSmall
        }
    }
}
