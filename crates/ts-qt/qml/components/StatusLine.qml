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
    readonly property string mode: Live.value("power_mode") || ""

    function modeName(m) {
        switch (m) {
        //% "Eco"
        case "eco": return qsTrId("status_mode_eco")
        //% "Quiet"
        case "quiet": return qsTrId("status_mode_quiet")
        //% "Balanced"
        case "balanced": return qsTrId("status_mode_balanced")
        //% "Performance"
        case "performance": return qsTrId("status_mode_performance")
        //% "Turbo"
        case "turbo": return qsTrId("status_mode_turbo")
        }
        return m
    }

    // One reading: icon and number, at a fixed width.
    component Reading: RowLayout {
        id: reading
        property string icon: ""
        property string text: ""
        property string name: ""
        property bool hot: false
        property bool wide: false
        spacing: 5
        Accessible.role: Accessible.StaticText
        Accessible.name: name + " " + text
        Icon { name: reading.icon; size: 14; color: reading.hot ? Theme.bad : Theme.muted }
        Text {
            Layout.minimumWidth: reading.wide ? 70 : 42
            text: reading.text
            color: reading.hot ? Theme.bad : Theme.stat
            font.pixelSize: Theme.fontSmall
            font.features: { "tnum": 1 }
        }
    }

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
        // The power mode, then the readings, each with its icon and a fixed width so that the row
        // does not shift when a number changes (1 digit more or less every few seconds).
        Badge {
            visible: !root.compact && root.width >= 560 && Live.ok && root.mode !== ""
            Layout.alignment: Qt.AlignVCenter
            text: root.modeName(root.mode)
            kind: root.mode === "turbo" ? "warn" : "accent"
        }
        Reading { visible: !root.compact && Live.ok; icon: "chip"; text: Fmt.temp(Live.value("sensors.cpu_temp")); hot: (Live.value("sensors.cpu_temp") || 0) >= 85
            //% "CPU temperature"
            name: qsTrId("status_cpu_a11y") }
        Reading { visible: !root.compact && Live.ok; icon: "gpu"; text: Fmt.temp(Live.value("sensors.gpu_temp")); hot: (Live.value("sensors.gpu_temp") || 0) >= 75
            //% "GPU temperature"
            name: qsTrId("status_gpu_a11y") }
        Reading { visible: !root.compact && Live.ok; icon: "fan"; text: Fmt.rpm(Live.value("sensors.cpu_fan_rpm")); wide: true
            //% "Fan speed"
            name: qsTrId("status_fans_a11y") }
        Reading { icon: "battery"; text: Fmt.percent(Live.value("battery_pct")); visible: !root.compact && root.width >= 700 && Live.ok && Live.value("battery_pct") !== undefined && Live.value("battery_pct") !== null
            //% "Battery"
            name: qsTrId("status_battery_a11y") }
    }
}
