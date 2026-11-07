// A slider with a name and the current value. Dragging fires `edited(value)` at most once every
// `coalesceMs` (the first move at once, then the newest value after each interval), and
// `released(value)` once when the person lets go, after a final `edited` if the last move was not
// reported yet (a keyboard step is throttled like a drag move but has no `released`). Setting
// `value` from code is silent.
//
//   SliderRow {
//       //% "CPU fan"
//       label: qsTrId("fan_cpu")
//       from: 20; to: 100; unit: "%"; value: Live.value("fans.duty_register_pct.cpu") || 20
//       onEdited: (v) => Api.write("fan.cpu", "fan", "set", { cpu: v })
//   }
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import org.truesense.app

SettingRow {
    id: root

    property alias from: slider.from
    property alias to: slider.to
    property alias stepSize: slider.stepSize
    property alias value: slider.value
    /// Appended to the shown number ("%", "W", " ms").
    property string unit: ""
    /// Replaces the shown number when set (for example "Auto" at the lowest position).
    property string valueText: ""
    property int coalesceMs: 60
    readonly property alias control: slider
    readonly property bool dragging: slider.pressed

    /// The value while being dragged, throttled; the page forwards it (usually to Api.write).
    signal edited(real value)
    /// The value when the person let go.
    signal released(real value)

    property real _latest: 0
    property real _sent: NaN
    property bool _dirty: false

    function _moved() {
        _latest = slider.value
        if (!throttle.running) {
            _send()
            throttle.start()
        } else {
            _dirty = true
        }
    }

    function _send() {
        _sent = _latest
        _dirty = false
        edited(_latest)
    }

    Timer {
        id: throttle
        interval: root.coalesceMs
        onTriggered: {
            if (root._dirty) {
                root._send()
                restart()
            }
        }
    }

    RowLayout {
        Layout.fillWidth: true
        spacing: 12

        Slider {
            id: slider
            Layout.fillWidth: true
            Layout.minimumWidth: 140
            Layout.preferredWidth: 260
            from: 0
            to: 100
            stepSize: 1
            snapMode: Slider.SnapAlways
            focusPolicy: Qt.StrongFocus
            hoverEnabled: true
            Accessible.name: root.label
            Accessible.role: Accessible.Slider
            onMoved: root._moved()
            onPressedChanged: {
                if (pressed) return
                throttle.stop()
                if (root._dirty) {
                    root._latest = slider.value
                    root._send()
                }
                root.released(slider.value)
            }

            background: Rectangle {
                x: slider.leftPadding
                y: slider.topPadding + slider.availableHeight / 2 - height / 2
                width: slider.availableWidth
                height: 6
                radius: 3
                color: Theme.groove
                Rectangle {
                    x: slider.mirrored ? parent.width - width : 0
                    width: slider.visualPosition === undefined ? 0 : (slider.mirrored ? 1 - slider.visualPosition : slider.visualPosition) * parent.width
                    height: parent.height
                    radius: 3
                    color: slider.enabled ? Theme.accent : Theme.fill
                }
            }
            handle: Rectangle {
                x: slider.leftPadding + slider.visualPosition * (slider.availableWidth - width)
                y: slider.topPadding + slider.availableHeight / 2 - height / 2
                width: 20
                height: 20
                radius: 10
                color: slider.pressed ? Theme.accent : Theme.knob
                border.width: slider.activeFocus ? 2 : 1
                border.color: slider.activeFocus ? Theme.focusRing : Theme.knobLine
                opacity: slider.enabled ? 1 : 0.5
            }
        }

        Text {
            Layout.minimumWidth: 52
            text: root.valueText !== "" ? root.valueText : (Math.round(slider.value * 100) / 100) + (root.unit !== "" ? " " + root.unit : "")
            color: Theme.stat
            font.pixelSize: Theme.fontBody
            horizontalAlignment: Text.AlignRight
        }
    }
}
