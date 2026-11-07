// Tab "Tuning": CPU and GPU undervolt. Both panels follow the same discipline: nothing is applied
// without a question, a value past the safety line is asked about in red, a value past the danger
// line needs the word APPLY typed, and a reboot always returns to stock. The GPU panel is created
// only when the GPU is chosen, because reading it asks the NVIDIA driver, which wakes a sleeping card.
// There is no CPU temperature ceiling here on purpose: the service has no command for it yet.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

ColumnLayout {
    id: root

    required property var host

    spacing: Theme.spacing + 4

    property string processor: "cpu"

    Card {
        Layout.fillWidth: true
        //% "Undervolt with care"
        title: qsTrId("perf_tune_title")
        icon: "warn"

        HelpText {
            //% "Lowering the CPU or GPU voltage makes the laptop run cooler. Too much can freeze it, reboot it or crash games. Nothing is applied unless you confirm, and a reboot always returns to stock. Test a profile under load before relying on it."
            text: qsTrId("perf_tune_warning")
        }
        Segmented {
            Layout.fillWidth: true
            model: [
                //% "CPU"
                { value: "cpu", label: qsTrId("perf_tune_cpu"), enabled: true },
                //% "GPU"
                { value: "gpu", label: qsTrId("perf_tune_gpu"), enabled: true }
            ]
            current: root.processor
            onActivated: (v) => root.processor = v
        }
    }

    Loader {
        Layout.fillWidth: true
        active: root.processor === "cpu"
        visible: active
        sourceComponent: Component {
            UndervoltPanel {
                kind: "cpu"
                usable: Caps.usable("undervolt") && root.host.capUsable("cpu_undervolt")
                whyNot: root.host.capWhy("cpu_undervolt")
            }
        }
        onLoaded: item.width = Qt.binding(function () { return width })
        Layout.preferredHeight: item ? item.implicitHeight : 0
    }
    Loader {
        Layout.fillWidth: true
        active: root.processor === "gpu"
        visible: active
        sourceComponent: Component {
            UndervoltPanel {
                kind: "gpu"
                usable: Caps.usable("undervolt") && root.host.capUsable("gpu_undervolt")
                whyNot: root.host.capWhy("gpu_undervolt")
            }
        }
        onLoaded: item.width = Qt.binding(function () { return width })
        Layout.preferredHeight: item ? item.implicitHeight : 0
    }
}
