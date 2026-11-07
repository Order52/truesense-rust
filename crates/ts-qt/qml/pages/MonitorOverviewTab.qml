// Monitor, Overview: the temperature gauges with the lowest and highest of this session, the
// headline of every other tab, two minutes of temperature history and the machine's facts.
// Pure presentation: `view` is the `overview` view of ts-ui-model (vm monitor), `facts` its `facts`
// view; the temperature history comes from the live stream (`Live`).
import QtQuick
import QtQuick.Layouts
import org.truesense.app

ColumnLayout {
    id: root

    property var view: null
    property var facts: null

    spacing: Theme.spacing + 4

    function gaugeName(g) {
        switch (g.id) {
        //% "CPU"
        case "cpu": return qsTrId("monitor_g_cpu")
        //% "GPU"
        case "gpu": return qsTrId("monitor_g_gpu")
        //% "Board"
        case "board": return qsTrId("monitor_g_board")
        //% "NVMe"
        case "nvme": return qsTrId("monitor_g_nvme")
        //% "Wi-Fi"
        case "wifi": return qsTrId("monitor_g_wifi")
        default: return g.id
        }
    }

    function gaugeCaption(g) {
        var name = gaugeName(g)
        if (g.id === "nvme" && g.index > 0) name += " " + (g.index + 1)
        return name
    }

    function factLabel(id) {
        switch (id) {
        //% "Computer"
        case "model": return qsTrId("monitor_f_model")
        //% "BIOS"
        case "bios": return qsTrId("monitor_f_bios")
        //% "Processor"
        case "cpu": return qsTrId("monitor_f_cpu")
        //% "Cores / threads"
        case "cores": return qsTrId("monitor_f_cores")
        //% "Highest clock"
        case "max_clock": return qsTrId("monitor_f_maxclock")
        //% "Memory"
        case "memory": return qsTrId("monitor_f_memory")
        //% "System"
        case "os": return qsTrId("monitor_f_os")
        //% "Kernel"
        case "kernel": return qsTrId("monitor_f_kernel")
        //% "Running for"
        case "uptime": return qsTrId("monitor_f_uptime")
        default:
            if (id.indexOf("gpu") === 0) {
                //% "Graphics"
                var g = qsTrId("monitor_f_gpu")
                var n = parseInt(id.substring(3))
                return n > 0 ? g + " " + (n + 1) : g
            }
            return id
        }
    }

    // ---- temperatures -----------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Temperatures"
        title: qsTrId("monitor_temps")
        icon: "monitor"
        //% "Lowest and highest since the app started"
        rightText: root.width >= 560 ? qsTrId("monitor_minmax_hint") : ""

        Flow {
            Layout.fillWidth: true
            spacing: Theme.spacing
            Repeater {
                model: root.view ? root.view.gauges : []
                delegate: ColumnLayout {
                    id: cell
                    required property var modelData
                    spacing: 2
                    width: 138
                    Gauge {
                        Layout.alignment: Qt.AlignHCenter
                        implicitWidth: 124
                        implicitHeight: 124
                        value: cell.modelData.value === null ? cell.modelData.from : cell.modelData.value
                        from: cell.modelData.from
                        to: cell.modelData.to
                        warnAt: cell.modelData.warn_at
                        critAt: cell.modelData.crit_at
                        text: cell.modelData.text
                        caption: root.gaugeCaption(cell.modelData)
                    }
                    Text {
                        Layout.fillWidth: true
                        visible: cell.modelData.badge === "asleep"
                        horizontalAlignment: Text.AlignHCenter
                        //% "asleep"
                        text: qsTrId("monitor_asleep")
                        color: Theme.warn
                        font.pixelSize: Theme.fontSmall + 1
                    }
                    Text {
                        Layout.fillWidth: true
                        horizontalAlignment: Text.AlignHCenter
                        //% "min %1 · max %2"
                        text: qsTrId("monitor_range").arg(cell.modelData.min_text).arg(cell.modelData.max_text)
                        color: Theme.muted
                        font.pixelSize: Theme.fontSmall
                        elide: Text.ElideRight
                        LayoutMirroring.enabled: false
                    }
                }
            }
        }
        HelpText {
            visible: root.view && root.view.gpu_state === "suspended"
            //% "The graphics card is asleep to save power. Its temperature comes from the firmware, which does not wake it."
            text: qsTrId("monitor_gpu_asleep_temp")
        }
    }

    // ---- headline ---------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Right now"
        title: qsTrId("monitor_now")
        icon: "chip"

        GridLayout {
            Layout.fillWidth: true
            columns: root.width < 560 ? 2 : 4
            columnSpacing: Theme.spacing
            rowSpacing: Theme.spacing
            MonitorStat {
                //% "Processor load"
                label: qsTrId("monitor_s_cpuload")
                value: root.view ? root.view.cpu_load.text : ""
                sub: root.view ? root.view.cpu_freq_text : ""
                level: root.view ? root.view.cpu_load.level : "ok"
            }
            MonitorStat {
                //% "Memory in use"
                label: qsTrId("monitor_s_mem")
                value: root.view ? root.view.memory.text : ""
                //% "swap %1"
                sub: root.view ? qsTrId("monitor_s_swap").arg(root.view.swap.text) : ""
                level: root.view ? root.view.memory.level : "ok"
            }
            MonitorStat {
                //% "Download"
                label: qsTrId("monitor_s_down")
                value: root.view ? root.view.net.rx_text : ""
                sub: root.view && root.view.net.primary ? root.view.net.primary : ""
            }
            MonitorStat {
                //% "Upload"
                label: qsTrId("monitor_s_up")
                value: root.view ? root.view.net.tx_text : ""
                sub: root.view && root.view.net.primary ? root.view.net.primary : ""
            }
        }
    }

    // ---- history ----------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Last two minutes"
        title: qsTrId("monitor_history")
        icon: "wave"

        LineChart {
            Layout.fillWidth: true
            capacity: 120
            chartHeight: 130
            minimum: 20
            maximum: 110
            grid: [ { value: 20, text: Fmt.temp(20) }, { value: 50, text: Fmt.temp(50) },
                    { value: 80, text: Fmt.temp(80) }, { value: 110, text: Fmt.temp(110) } ]
            //% "Waiting for the first readings"
            emptyText: qsTrId("monitor_waiting")
            series: [
                //% "Processor"
                { values: Live.history("sensors.cpu_temp"), color: Theme.accent, label: qsTrId("monitor_h_cpu"), filled: true },
                //% "Graphics"
                { values: Live.history("sensors.gpu_temp"), color: Theme.warn, label: qsTrId("monitor_h_gpu") },
                //% "Board"
                { values: Live.history("sensors.sys_temp"), color: Theme.ok, label: qsTrId("monitor_h_board") }
            ]
        }
    }

    // ---- facts ------------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "This computer"
        title: qsTrId("monitor_computer")
        icon: "info"

        ColumnLayout {
            Layout.fillWidth: true
            spacing: 8
            Repeater {
                model: root.facts ? root.facts.rows : []
                delegate: MonitorFactRow {
                    required property var modelData
                    label: root.factLabel(modelData.id)
                    value: modelData.text
                }
            }
        }
    }
}
