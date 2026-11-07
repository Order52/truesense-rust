// Monitor, Memory and disks: RAM and swap, the biggest programs by memory, mounted filesystems and
// the physical drives with their temperature. `view` is the `memory` view of ts-ui-model (vm
// monitor); `ramHistory` is the page's own series of memory in use (null = gap).
import QtQuick
import QtQuick.Layouts
import org.truesense.app

ColumnLayout {
    id: root

    property var view: null
    property var ramHistory: []

    spacing: Theme.spacing + 4

    function kindLabel(kind) {
        switch (kind) {
        //% "NVMe drive"
        case "nvme": return qsTrId("monitor_k_nvme")
        //% "SSD"
        case "ssd": return qsTrId("monitor_k_ssd")
        //% "Hard disk"
        case "hdd": return qsTrId("monitor_k_hdd")
        //% "Memory card"
        case "mmc": return qsTrId("monitor_k_mmc")
        //% "Drive"
        default: return qsTrId("monitor_k_other")
        }
    }

    Card {
        Layout.fillWidth: true
        //% "Memory"
        title: qsTrId("monitor_mem_title")
        icon: "chip"
        rightText: root.view && root.view.ram ? root.view.ram.total_text : ""

        HelpText {
            visible: root.view && !root.view.has_memory
            //% "Memory use cannot be read on this computer."
            text: qsTrId("monitor_mem_none")
        }

        GridLayout {
            Layout.fillWidth: true
            visible: root.view && root.view.has_memory
            columns: root.width < 560 ? 2 : 4
            columnSpacing: Theme.spacing
            rowSpacing: Theme.spacing
            MonitorStat {
                //% "In use"
                label: qsTrId("monitor_mem_used")
                value: root.view && root.view.ram ? root.view.ram.used_text : ""
                sub: root.view && root.view.ram ? root.view.ram.pct_text : ""
                level: root.view && root.view.ram ? root.view.ram.level : "ok"
            }
            MonitorStat {
                //% "Available"
                label: qsTrId("monitor_mem_avail")
                value: root.view && root.view.ram ? root.view.ram.available_text : ""
            }
            MonitorStat {
                //% "Cached"
                label: qsTrId("monitor_mem_cache")
                value: root.view && root.view.ram ? root.view.ram.cache_text : ""
            }
            MonitorStat {
                //% "Free"
                label: qsTrId("monitor_mem_free")
                value: root.view && root.view.ram ? root.view.ram.free_text : ""
            }
        }

        LineChart {
            Layout.fillWidth: true
            visible: root.view && root.view.has_memory
            capacity: 120
            chartHeight: 100
            minimum: 0
            maximum: 100
            grid: [ { value: 0, text: Fmt.percent(0) }, { value: 50, text: Fmt.percent(50) }, { value: 100, text: Fmt.percent(100) } ]
            //% "Waiting for the first readings"
            emptyText: qsTrId("monitor_waiting")
            series: [ { values: root.ramHistory, color: Theme.accent, filled: true } ]
            summary: root.view && root.view.ram ? root.view.ram.pct_text : ""
        }

        BarList {
            Layout.fillWidth: true
            visible: root.view && root.view.swap_present
            labelWidth: 70
            textWidth: 130
            model: root.view && root.view.swap ? [
                //% "Swap"
                { label: qsTrId("monitor_mem_swap"), text: root.view.swap.used_text + " / " + root.view.swap.total_text,
                  value: root.view.swap.value, level: root.view.swap.level }
            ] : []
        }
        HelpText {
            visible: root.view && root.view.has_memory && !root.view.swap_present
            //% "This computer has no swap space."
            text: qsTrId("monitor_mem_noswap")
        }
    }

    Card {
        Layout.fillWidth: true
        //% "Biggest programs"
        title: qsTrId("monitor_mem_procs")
        icon: "performance"
        MonitorProcessList {
            Layout.fillWidth: true
            mode: "mem"
            rows: root.view ? root.view.top : []
            //% "No programs to list."
            emptyText: qsTrId("monitor_mem_procs_none")
        }
    }

    Card {
        Layout.fillWidth: true
        //% "Storage"
        title: qsTrId("monitor_disks_title")
        icon: "save"

        BarList {
            Layout.fillWidth: true
            labelWidth: 140
            textWidth: 52
            model: root.view ? root.view.filesystems : []
            //% "No mounted drives were found."
            emptyText: root.view && root.view.has_disks ? qsTrId("monitor_disks_none")
                       //% "Reading the drives..."
                       : qsTrId("monitor_disks_wait")
        }

        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: 1
            color: Theme.sep
            visible: root.view && root.view.drives.length > 0
        }
        Repeater {
            model: root.view ? root.view.drives : []
            delegate: RowLayout {
                id: drive
                required property var modelData
                Layout.fillWidth: true
                spacing: 10
                Icon { name: "save"; size: 18; color: Theme.stat }
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 0
                    Text {
                        Layout.fillWidth: true
                        text: drive.modelData.model ? drive.modelData.model : drive.modelData.name
                        color: Theme.text
                        font.pixelSize: Theme.fontBody
                        elide: Text.ElideRight
                        textFormat: Text.PlainText
                    }
                    Text {
                        Layout.fillWidth: true
                        text: (drive.modelData.model ? drive.modelData.name + " · " : "") + root.kindLabel(drive.modelData.kind)
                        color: Theme.muted
                        font.pixelSize: Theme.fontSmall
                        elide: Text.ElideRight
                        textFormat: Text.PlainText
                    }
                }
                ColumnLayout {
                    spacing: 0
                    Text {
                        Layout.alignment: Qt.AlignRight
                        text: drive.modelData.size_text
                        color: Theme.text
                        font.pixelSize: Theme.fontBody
                        font.bold: true
                    }
                    Text {
                        Layout.alignment: Qt.AlignRight
                        visible: drive.modelData.temp_text !== Fmt.unknown
                        text: drive.modelData.temp_text
                        color: Theme.muted
                        font.pixelSize: Theme.fontSmall
                    }
                }
            }
        }
    }
}
