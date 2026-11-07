// Performance: how the laptop chooses its power mode (Smart auto and its profiles), which programs
// raise it (game rules), how much power the CPU may draw (limits, other power managers) and the
// voltage tuning (CPU and GPU undervolt). The page is only the frame and the tab switch; each tab is
// a file in pages/performance and is created when it is first shown (the GPU is never asked about
// until the GPU tab is open: asking wakes a sleeping card). The decisions are in
// ts-ui-model/src/vm/performance.rs.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

PageFrame {
    id: page
    title: Nav.titleOf(Nav.currentPage)

    /// "mode", "rules", "limits" or "tuning".
    property string tab: "mode"
    /// `capabilities get` as {id: {usable, why}} (ids like "cpu_undervolt"); {} until it answers.
    property var capMap: ({})

    function capUsable(id) {
        var e = capMap[id]
        return e === undefined || e.usable === true
    }
    function capWhy(id) {
        var e = capMap[id]
        return (e === undefined || e.usable === true || !e.why) ? "" : e.why
    }

    Component.onCompleted: Api.call("capabilities", "get", {}, function (answer) {
        capMap = PerfVm.val("caps", { capabilities: answer }, ({}))
    }, function () {})

    Segmented {
        Layout.fillWidth: true
        model: [
            //% "Mode and Smart"
            { value: "mode", label: qsTrId("perf_tab_mode") },
            //% "Game rules"
            { value: "rules", label: qsTrId("perf_tab_rules") },
            //% "Power limits"
            { value: "limits", label: qsTrId("perf_tab_limits") },
            //% "Tuning"
            { value: "tuning", label: qsTrId("perf_tab_tuning") }
        ]
        current: page.tab
        onActivated: (v) => page.tab = v
    }

    Loader {
        Layout.fillWidth: true
        active: page.tab === "mode"
        visible: active
        sourceComponent: Component { PerfModeTab { host: page } }
        onLoaded: item.width = Qt.binding(function () { return width })
        Layout.preferredHeight: item ? item.implicitHeight : 0
    }
    Loader {
        Layout.fillWidth: true
        active: page.tab === "rules"
        visible: active
        sourceComponent: Component { PerfGameTab { host: page } }
        onLoaded: item.width = Qt.binding(function () { return width })
        Layout.preferredHeight: item ? item.implicitHeight : 0
    }
    Loader {
        Layout.fillWidth: true
        active: page.tab === "limits"
        visible: active
        sourceComponent: Component { PerfLimitsTab { host: page } }
        onLoaded: item.width = Qt.binding(function () { return width })
        Layout.preferredHeight: item ? item.implicitHeight : 0
    }
    Loader {
        Layout.fillWidth: true
        active: page.tab === "tuning"
        visible: active
        sourceComponent: Component { PerfTuningTab { host: page } }
        onLoaded: item.width = Qt.binding(function () { return width })
        Layout.preferredHeight: item ? item.implicitHeight : 0
    }
}
