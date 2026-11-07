// Home: placeholder with the live strip, which also proves the live pipeline end to end.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

PageFrame {
    title: Nav.currentPage ? Nav.titleOf(Nav.currentPage) : ""

    Card {
        Layout.fillWidth: true
        //% "Live"
        title: qsTrId("home_live")
        icon: "chip"
        //% "AC"
        rightText: Live.value("ac") === undefined ? "" : (Live.value("ac") ? qsTrId("home_ac")
                                                                          //% "Battery"
                                                                          : qsTrId("home_battery"))

        Repeater {
            model: [
                //% "CPU temperature"
                { label: qsTrId("home_cpu_temp"),
                  path: "sensors.cpu_temp", fmt: "temp" },
                //% "GPU temperature"
                { label: qsTrId("home_gpu_temp"),
                  path: "sensors.gpu_temp", fmt: "temp" },
                //% "Fans"
                { label: qsTrId("home_fans"),
                  path: "sensors.cpu_fan_rpm", fmt: "rpm" },
                //% "CPU load"
                { label: qsTrId("home_cpu_load"),
                  path: "load.cpu_pct", fmt: "percent" }
            ]
            delegate: RowLayout {
                required property var modelData
                Layout.fillWidth: true
                spacing: 12
                Text {
                    Layout.fillWidth: true
                    text: modelData.label
                    color: Theme.text
                    font.pixelSize: Theme.fontBody
                    elide: Text.ElideRight
                }
                Sparkline {
                    Layout.preferredWidth: 110
                    Layout.preferredHeight: 24
                    values: Live.history(modelData.path)
                }
                Text {
                    Layout.preferredWidth: 78
                    horizontalAlignment: Text.AlignRight
                    text: Fmt[modelData.fmt](Live.value(modelData.path))
                    color: Theme.stat
                    font.pixelSize: Theme.fontBody
                }
            }
        }
    }

    Card {
        Layout.fillWidth: true
        HelpText {
            //% "This page is coming in the next step."
            text: qsTrId("placeholder_next_step")
        }
    }
}
