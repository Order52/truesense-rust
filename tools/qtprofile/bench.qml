import QtQuick
import QtQuick.Window
import QtQuick.Layouts
import org.truesense.app
import "old" as Old

Window {
    id: win
    width: 760; height: 520
    visible: true
    color: "#17181c"
    property int frames: 0
    property bool useOld: Qt.application.arguments.indexOf("old") >= 0
    property int tick: 0
    property var rpm: [5000, 4500]
    property var hist: []
    property var net: []
    onFrameSwapped: frames++
    Timer {
        interval: 1000; repeat: true; running: true
        onTriggered: {
            win.tick++
            win.rpm = [4000 + Math.random() * 2000, 4000 + Math.random() * 2000]
            var h = win.hist.slice(-119); h.push(win.rpm[0]); win.hist = h
            var n = win.net.slice(-119); n.push(Math.random() * 1000); win.net = n
        }
    }
    Timer {
        interval: 1000; repeat: true; running: true
        property int last: 0
        onTriggered: { console.log("FRAMES " + (win.frames - last)); last = win.frames }
    }
    Timer { interval: 9000; running: true; onTriggered: Qt.quit() }

    RowLayout {
        anchors.fill: parent
        anchors.margins: 16
        Loader { Layout.fillWidth: true; Layout.fillHeight: true; sourceComponent: win.useOld ? oldComp : newComp }
    }
    Component {
        id: newComp
        ColumnLayout {
            RowLayout {
                Gauge { Layout.preferredWidth: 150; Layout.preferredHeight: 150; value: win.rpm[0]; to: 6500; text: Math.round(win.rpm[0]) + " RPM" }
                Gauge { Layout.preferredWidth: 150; Layout.preferredHeight: 150; value: win.rpm[1]; to: 6500; text: Math.round(win.rpm[1]) + " RPM" }
            }
            Sparkline { Layout.fillWidth: true; Layout.preferredHeight: 26; values: win.hist; minRepaintMs: 3000 }
            LineChart { Layout.fillWidth: true; capacity: 120; series: [ { values: win.net, color: "#58a6ff", label: "Download", filled: true } ] }
        }
    }
    Component {
        id: oldComp
        ColumnLayout {
            RowLayout {
                Old.Gauge { Layout.preferredWidth: 150; Layout.preferredHeight: 150; value: win.rpm[0]; to: 6500; text: Math.round(win.rpm[0]) + " RPM" }
                Old.Gauge { Layout.preferredWidth: 150; Layout.preferredHeight: 150; value: win.rpm[1]; to: 6500; text: Math.round(win.rpm[1]) + " RPM" }
            }
            Old.Sparkline { Layout.fillWidth: true; Layout.preferredHeight: 26; values: win.hist; minRepaintMs: 3000 }
            Old.LineChart { Layout.fillWidth: true; capacity: 120; series: [ { values: win.net, color: "#58a6ff", label: "Download", filled: true } ] }
        }
    }
}
