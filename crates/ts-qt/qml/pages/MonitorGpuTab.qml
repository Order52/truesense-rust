// Monitor, GPU: temperature, load, power, video memory, clocks, P-state, PCIe link and the power
// limits, read-only. `view` is the `gpu` view of ts-ui-model (vm monitor); the three history series
// belong to the page. A card that is asleep is said to be asleep and shows no numbers: the page
// does not wake it to take a reading (and never polls it for detail).
import QtQuick
import QtQuick.Layouts
import org.truesense.app

ColumnLayout {
    id: root

    property var view: null
    property var tempHistory: []
    property var utilHistory: []
    property var powerHistory: []
    /// {max, grid} for the power chart, from ts-ui-model `scale`.
    property var powerScale: null

    readonly property bool present: view !== null && !view.absent
    readonly property bool asleep: view !== null && view.asleep

    spacing: Theme.spacing + 4

    /// "of 8.0 GB", or nothing when the whole is not known.
    function ofText(total) {
        //% "of %1"
        return total === Fmt.unknown ? "" : qsTrId("monitor_gpu_of").arg(total)
    }

    function limitLabel(id) {
        switch (id) {
        //% "Current power limit"
        case "current": return qsTrId("monitor_gpu_l_current")
        //% "Default"
        case "default": return qsTrId("monitor_gpu_l_default")
        //% "Lowest allowed"
        case "min": return qsTrId("monitor_gpu_l_min")
        //% "Highest allowed"
        case "max": return qsTrId("monitor_gpu_l_max")
        default: return id
        }
    }

    // ---- no card ----------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        visible: root.view !== null && root.view.absent
        //% "Graphics card"
        title: qsTrId("monitor_gpu_title")
        icon: "gpu"
        HelpText {
            //% "No separate NVIDIA graphics card was found in this computer."
            text: qsTrId("monitor_gpu_absent")
        }
    }

    // ---- asleep -----------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        visible: root.asleep
        //% "The graphics card is asleep"
        title: qsTrId("monitor_gpu_sleep_title")
        icon: "moon"
        HelpText {
            //% "It switches itself off when nothing uses it, which saves power and heat. TrueSense does not wake it to take readings: start a game or a program that uses it and the numbers appear here. The temperature below comes from the firmware and is always available."
            text: qsTrId("monitor_gpu_sleep_text")
        }
    }

    // ---- gauges -----------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        visible: root.present
        title: root.view && root.view.name ? root.view.name : qsTrId("monitor_gpu_title")
        icon: "gpu"
        //% "driver %1"
        rightText: root.view && root.view.driver ? qsTrId("monitor_gpu_driver").arg(root.view.driver) : ""

        Flow {
            Layout.fillWidth: true
            spacing: Theme.spacing
            Repeater {
                model: root.view ? [
                    { id: "temp", g: root.view.temperature, from: 0, to: 110 },
                    { id: "util", g: root.view.util, from: 0, to: 100 },
                    { id: "power", g: root.view.power, from: 0, to: 100 }
                ] : []
                delegate: ColumnLayout {
                    id: cell
                    required property var modelData
                    spacing: 2
                    width: 138
                    Gauge {
                        Layout.alignment: Qt.AlignHCenter
                        implicitWidth: 124
                        implicitHeight: 124
                        value: cell.modelData.id === "power"
                               ? (cell.modelData.g.fraction === null ? 0 : cell.modelData.g.fraction * 100)
                               : (cell.modelData.g.value === null ? 0 : cell.modelData.g.value)
                        from: 0
                        to: cell.modelData.to
                        warnAt: cell.modelData.id === "temp" ? cell.modelData.g.warn_at : NaN
                        critAt: cell.modelData.id === "temp" ? cell.modelData.g.crit_at : NaN
                        text: cell.modelData.g.text
                        caption: ""
                    }
                    Text {
                        Layout.fillWidth: true
                        horizontalAlignment: Text.AlignHCenter
                        color: Theme.text
                        font.pixelSize: Theme.fontSmall + 1
                        text: cell.modelData.id === "temp"
                              //% "Temperature"
                              ? qsTrId("monitor_gpu_g_temp")
                              : cell.modelData.id === "util"
                                //% "Load"
                                ? qsTrId("monitor_gpu_g_util")
                                //% "Power"
                                : qsTrId("monitor_gpu_g_power")
                    }
                    Text {
                        Layout.fillWidth: true
                        horizontalAlignment: Text.AlignHCenter
                        visible: cell.modelData.id === "power" && cell.modelData.g.limit_text !== Fmt.unknown
                        text: root.ofText(cell.modelData.g.limit_text)
                        color: Theme.muted
                        font.pixelSize: Theme.fontSmall
                    }
                }
            }
        }
    }

    // ---- memory and clocks ------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        visible: root.present && !root.asleep
        //% "Memory and clocks"
        title: qsTrId("monitor_gpu_mem")
        icon: "chip"

        BarList {
            Layout.fillWidth: true
            labelWidth: 120
            textWidth: 84
            model: root.view ? [
                //% "Video memory"
                { label: qsTrId("monitor_gpu_vram"), text: root.view.vram.used_text, sub: root.ofText(root.view.vram.total_text),
                  value: root.view.vram.value, level: root.view.vram.level },
                //% "Graphics clock"
                { label: qsTrId("monitor_gpu_gclk"), text: root.view.clocks.graphics.text, sub: root.ofText(root.view.clocks.graphics.max_text),
                  value: root.view.clocks.graphics.value, level: "ok" },
                //% "Memory clock"
                { label: qsTrId("monitor_gpu_mclk"), text: root.view.clocks.memory.text, sub: root.ofText(root.view.clocks.memory.max_text),
                  value: root.view.clocks.memory.value, level: "ok" }
            ] : []
        }
        HelpText {
            visible: root.view && !root.view.has_detail
            text: root.view && root.view.error ? root.view.error
                  //% "Waiting for the first detailed reading."
                  : qsTrId("monitor_gpu_wait_detail")
        }
    }

    // ---- history ----------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        visible: root.present && !root.asleep
        //% "Last two minutes"
        title: qsTrId("monitor_history")
        icon: "wave"

        LineChart {
            Layout.fillWidth: true
            capacity: 120
            chartHeight: 110
            minimum: 0
            maximum: 110
            grid: [ { value: 0, text: "0" }, { value: 50, text: "50" }, { value: 100, text: "100" } ]
            //% "Waiting for the first readings"
            emptyText: qsTrId("monitor_waiting")
            series: [
                //% "Temperature (°C)"
                { values: root.tempHistory, color: Theme.warn, label: qsTrId("monitor_gpu_h_temp") },
                //% "Load (%)"
                { values: root.utilHistory, color: Theme.accent, label: qsTrId("monitor_gpu_h_util"), filled: true }
            ]
        }
        LineChart {
            Layout.fillWidth: true
            capacity: 120
            chartHeight: 90
            minimum: 0
            maximum: root.powerScale ? root.powerScale.max : NaN
            grid: root.powerScale ? root.powerScale.grid : []
            series: [
                //% "Power"
                { values: root.powerHistory, color: Theme.ok, label: qsTrId("monitor_gpu_h_power"), filled: true }
            ]
        }
    }

    // ---- details and limits -----------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        visible: root.present && !root.asleep
        //% "Details"
        title: qsTrId("monitor_gpu_details")
        icon: "info"

        ColumnLayout {
            Layout.fillWidth: true
            spacing: 8
            MonitorFactRow {
                //% "Performance state"
                label: qsTrId("monitor_gpu_pstate")
                value: root.view && root.view.pstate ? root.view.pstate : ""
            }
            MonitorFactRow {
                //% "PCIe link"
                label: qsTrId("monitor_gpu_pcie")
                //% "%1 (up to %2)"
                value: root.view ? (root.view.pcie.downgraded ? qsTrId("monitor_gpu_pcie_fmt").arg(root.view.pcie.text).arg(root.view.pcie.max_text) : root.view.pcie.text) : ""
            }
            MonitorFactRow {
                //% "Memory activity"
                label: qsTrId("monitor_gpu_memact")
                value: root.view ? root.view.memory_util_text : ""
            }
            MonitorFactRow {
                //% "Fan"
                label: qsTrId("monitor_gpu_fan")
                value: root.view ? root.view.fan_text : ""
            }
            MonitorFactRow {
                //% "Video BIOS"
                label: qsTrId("monitor_gpu_vbios")
                value: root.view && root.view.vbios ? root.view.vbios : ""
            }
        }
        HelpText {
            visible: root.view && root.view.has_detail
            //% "Detailed readings are taken every 5 seconds."
            text: qsTrId("monitor_gpu_every5")
        }
    }

    Card {
        Layout.fillWidth: true
        visible: root.present && !root.asleep
        //% "Power limits"
        title: qsTrId("monitor_gpu_limits")
        icon: "lock"

        ColumnLayout {
            Layout.fillWidth: true
            spacing: 8
            Repeater {
                model: root.view ? root.view.limits : []
                delegate: MonitorFactRow {
                    required property var modelData
                    label: root.limitLabel(modelData.id)
                    value: modelData.text
                }
            }
        }
        HelpText {
            //% "These are shown for information only. The total graphics power (TGP) of this laptop is fixed by its firmware: TrueSense cannot raise or lower it, and does not try."
            text: qsTrId("monitor_gpu_tgp_fixed")
        }
    }
}
