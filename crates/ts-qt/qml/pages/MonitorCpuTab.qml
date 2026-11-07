// Monitor, CPU: total load with its history, clocks, load average, power when readable, every
// core as a bar and the busiest processes. `view` is the `cpu` view of ts-ui-model (vm monitor);
// `loadHistory` the page's own series of total load (null = gap).
import QtQuick
import QtQuick.Layouts
import org.truesense.app

ColumnLayout {
    id: root

    property var view: null
    property var loadHistory: []
    /// How many processes are listed now, and the signal to ask for another count.
    property int limit: 8
    signal limitRequested(int limit)

    spacing: Theme.spacing + 4

    Card {
        Layout.fillWidth: true
        //% "Processor"
        title: qsTrId("monitor_cpu_title")
        icon: "chip"
        //% "%1 threads"
        rightText: root.view && root.view.core_count > 0 ? qsTrId("monitor_cpu_threads").arg(root.view.core_count) : ""

        GridLayout {
            Layout.fillWidth: true
            columns: root.width < 560 ? 2 : 4
            columnSpacing: Theme.spacing
            rowSpacing: Theme.spacing
            MonitorStat {
                //% "Load"
                label: qsTrId("monitor_cpu_load")
                value: root.view ? root.view.load.text : ""
                level: root.view ? root.view.load.level : "ok"
                sub: root.view && root.view.governor ? root.view.governor : ""
            }
            MonitorStat {
                //% "Clock"
                label: qsTrId("monitor_cpu_clock")
                value: root.view ? root.view.freq_avg_text : ""
                //% "up to %1"
                sub: root.view ? qsTrId("monitor_cpu_upto").arg(root.view.freq_max_text) : ""
            }
            MonitorStat {
                //% "Temperature"
                label: qsTrId("monitor_cpu_temp")
                value: root.view ? root.view.temp.text : ""
            }
            MonitorStat {
                //% "Power"
                label: qsTrId("monitor_cpu_power")
                value: root.view ? root.view.power.text : ""
                //% "needs the service"
                sub: root.view && !root.view.power.available ? qsTrId("monitor_cpu_power_na") : ""
            }
        }

        LineChart {
            Layout.fillWidth: true
            capacity: 120
            chartHeight: 110
            minimum: 0
            maximum: 100
            grid: [ { value: 0, text: Fmt.percent(0) }, { value: 50, text: Fmt.percent(50) }, { value: 100, text: Fmt.percent(100) } ]
            //% "Waiting for the first readings"
            emptyText: qsTrId("monitor_waiting")
            series: [ { values: root.loadHistory, color: Theme.accent, filled: true } ]
            summary: root.view ? root.view.load.text : ""
        }

        RowLayout {
            Layout.fillWidth: true
            spacing: Theme.spacing
            visible: root.view && root.view.load_avg.available
            Text {
                color: Theme.muted
                font.pixelSize: Theme.fontSmall + 1
                //% "Load average (1, 5, 15 min)"
                text: qsTrId("monitor_cpu_avg")
            }
            Item { Layout.fillWidth: true }
            Text {
                color: Theme.text
                font.pixelSize: Theme.fontBody
                LayoutMirroring.enabled: false
                text: root.view ? root.view.load_avg.one + "  ·  " + root.view.load_avg.five + "  ·  " + root.view.load_avg.fifteen : ""
            }
        }
    }

    Card {
        Layout.fillWidth: true
        //% "Cores"
        title: qsTrId("monitor_cores")
        icon: "bar"

        BarList {
            Layout.fillWidth: true
            columns: root.width >= 640 ? 2 : 1
            labelWidth: 52
            model: root.view ? root.view.cores : []
            //% "No per-core readings."
            emptyText: qsTrId("monitor_cores_none")
        }
    }

    Card {
        Layout.fillWidth: true
        //% "Busiest processes"
        title: qsTrId("monitor_procs")
        icon: "performance"
        //% "%1 running programs"
        rightText: root.view && root.view.processes !== null ? qsTrId("monitor_procs_count").arg(root.view.processes) : ""

        MonitorProcessList {
            Layout.fillWidth: true
            mode: "cpu"
            rows: root.view ? root.view.top : []
            //% "Measuring: percentages appear after the second look."
            emptyText: root.view && !root.view.top_valid ? qsTrId("monitor_procs_measuring")
                       //% "No processes to list."
                       : qsTrId("monitor_procs_none")
        }
        HelpText {
            //% "100 % is one full core, so a busy program can show more."
            text: qsTrId("monitor_procs_hint")
        }
        SecondaryButton {
            Layout.alignment: Qt.AlignHCenter
            text: root.limit > 8
                  //% "Show fewer"
                  ? qsTrId("monitor_procs_fewer")
                  //% "Show more"
                  : qsTrId("monitor_procs_more")
            onClicked: root.limitRequested(root.limit > 8 ? 8 : 20)
        }
    }
}
