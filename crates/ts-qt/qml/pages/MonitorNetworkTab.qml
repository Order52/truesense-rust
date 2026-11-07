// Monitor, Network: download and upload as a chart with the peaks and the totals of this session,
// the interface in use and every other interface. `view` is the `network` view of ts-ui-model (vm
// monitor); `rxHistory` and `txHistory` are the page's series in bytes per second (null = gap).
import QtQuick
import QtQuick.Layouts
import org.truesense.app

ColumnLayout {
    id: root

    property var view: null
    property var rxHistory: []
    property var txHistory: []

    spacing: Theme.spacing + 4

    function kindLabel(kind) {
        switch (kind) {
        //% "Wi-Fi"
        case "wifi": return qsTrId("monitor_n_wifi")
        //% "Ethernet"
        case "ethernet": return qsTrId("monitor_n_eth")
        //% "Virtual"
        case "virtual": return qsTrId("monitor_n_virtual")
        //% "Other"
        default: return qsTrId("monitor_n_other")
        }
    }

    Card {
        Layout.fillWidth: true
        //% "Traffic"
        title: qsTrId("monitor_net_title")
        icon: "globe"
        rightText: root.view && root.view.primary
                   ? root.kindLabel(root.view.primary_kind) + " · " + root.view.primary : ""

        GridLayout {
            Layout.fillWidth: true
            columns: 2
            columnSpacing: Theme.spacing
            rowSpacing: Theme.spacing
            MonitorStat {
                //% "Download"
                label: qsTrId("monitor_s_down")
                value: root.view ? root.view.rx.text : ""
                //% "highest %1"
                sub: root.view ? qsTrId("monitor_net_peak").arg(root.view.rx.peak_text) : ""
            }
            MonitorStat {
                //% "Upload"
                label: qsTrId("monitor_s_up")
                value: root.view ? root.view.tx.text : ""
                sub: root.view ? qsTrId("monitor_net_peak").arg(root.view.tx.peak_text) : ""
            }
            MonitorStat {
                //% "Downloaded this session"
                label: qsTrId("monitor_net_sessdown")
                value: root.view ? root.view.rx.session_text : ""
                valueSize: Theme.fs(16)
            }
            MonitorStat {
                //% "Uploaded this session"
                label: qsTrId("monitor_net_sessup")
                value: root.view ? root.view.tx.session_text : ""
                valueSize: Theme.fs(16)
            }
        }

        LineChart {
            Layout.fillWidth: true
            capacity: 120
            chartHeight: 150
            minimum: 0
            maximum: root.view ? root.view.chart.max : NaN
            grid: root.view ? root.view.chart.grid : []
            //% "Waiting for the first readings"
            emptyText: qsTrId("monitor_waiting")
            series: [
                //% "Download"
                { values: root.rxHistory, color: Theme.accent, label: qsTrId("monitor_s_down"), filled: true },
                //% "Upload"
                { values: root.txHistory, color: Theme.ok, label: qsTrId("monitor_s_up") }
            ]
            summary: root.view ? root.view.rx.text + ", " + root.view.tx.text : ""
        }
        HelpText {
            visible: root.view && root.view.has_data && root.view.primary === null
            //% "No network connection is in use."
            text: qsTrId("monitor_net_none")
        }
        HelpText {
            //% "Totals and highest rates count from the start of the app or the last reset, over the real network cards only."
            text: qsTrId("monitor_net_hint")
        }
    }

    Card {
        Layout.fillWidth: true
        //% "Connections"
        title: qsTrId("monitor_ifaces")
        icon: "wave"

        Text {
            Layout.fillWidth: true
            visible: root.view && root.view.interfaces.length === 0
            //% "No network interfaces."
            text: qsTrId("monitor_ifaces_none")
            color: Theme.muted
            font.pixelSize: Theme.fontSmall + 1
        }
        Repeater {
            model: root.view ? root.view.interfaces : []
            delegate: ColumnLayout {
                id: iface
                required property var modelData
                required property int index
                Layout.fillWidth: true
                spacing: 2
                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8
                    Text {
                        text: iface.modelData.name
                        color: Theme.text
                        font.pixelSize: Theme.fontBody
                        font.bold: true
                        LayoutMirroring.enabled: false
                    }
                    Text {
                        Layout.fillWidth: true
                        text: root.kindLabel(iface.modelData.kind) + (iface.modelData.state ? " · " + iface.modelData.state : "")
                              + (iface.modelData.speed_text !== Fmt.unknown ? " · " + iface.modelData.speed_text : "")
                        color: Theme.muted
                        font.pixelSize: Theme.fontSmall
                        elide: Text.ElideRight
                    }
                    Text {
                        text: "↓ " + iface.modelData.rx_text + "   ↑ " + iface.modelData.tx_text
                        color: Theme.stat
                        font.pixelSize: Theme.fontSmall + 1
                        LayoutMirroring.enabled: false
                    }
                }
                Text {
                    Layout.fillWidth: true
                    visible: iface.modelData.addresses.length > 0 || iface.modelData.mac
                    text: iface.modelData.addresses.join(", ") + (iface.modelData.mac ? (iface.modelData.addresses.length ? " · " : "") + iface.modelData.mac : "")
                    color: Theme.muted
                    font.pixelSize: Theme.fontSmall
                    elide: Text.ElideRight
                    textFormat: Text.PlainText
                    LayoutMirroring.enabled: false
                }
                Rectangle {
                    Layout.fillWidth: true
                    Layout.preferredHeight: 1
                    Layout.topMargin: 6
                    color: Theme.sep
                    visible: index < (root.view.interfaces.length - 1)
                }
            }
        }
    }
}
