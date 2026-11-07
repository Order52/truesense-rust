// A colour chooser: a saturation/value square, a hue bar and a hex field. `colorChanged(hex)` fires
// for the person's own changes only (dragging, typing a full colour), so the page can send it to
// the lights through Api.write; opening it with `start(hex)` is silent. The colour maths is the
// view-model's (lighting.rs), not duplicated here.
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import org.truesense.app

Popup {
    id: root

    /// The colour being edited, "rrggbb".
    property string hex: "00aec7"
    property real _h: 190
    property real _s: 1
    property real _v: 0.78
    property bool _silent: false

    signal picked(string hex)

    modal: false
    focus: true
    padding: 14
    closePolicy: Popup.CloseOnEscape | Popup.CloseOnPressOutside
    width: Math.min(300, (parent ? parent.width : 300) - 16)

    /// Opens the chooser on `startHex` (without announcing it as a pick).
    function start(startHex) {
        var h = LightingVm.call("norm_hex", { s: startHex })
        hex = h === null || h === undefined ? "00aec7" : h
        var hsv = LightingVm.call("hex_to_hsv", { hex: hex })
        if (hsv !== undefined) {
            // A grey keeps the last hue so that dragging saturation up does not jump to red.
            if (hsv.s > 0) _h = hsv.h
            _s = hsv.s
            _v = hsv.v
        }
        field.text = hex
    }

    function _update(announce) {
        var h = LightingVm.call("hsv_to_hex", { h: _h, s: _s, v: _v })
        if (h === undefined) return
        hex = h
        field.text = h
        if (announce) picked(h)
    }

    background: Rectangle {
        radius: Theme.radius
        color: Theme.dialog
        border.color: Theme.cardLine
    }

    contentItem: ColumnLayout {
        spacing: 12

        // Saturation (left to right) and value (top to bottom) of the current hue.
        Item {
            id: square
            Layout.fillWidth: true
            Layout.preferredHeight: 150
            Rectangle {
                anchors.fill: parent
                radius: Theme.radiusSmall
                border.width: 1
                border.color: Theme.cardLine
                gradient: Gradient {
                    orientation: Gradient.Horizontal
                    GradientStop { position: 0; color: Qt.hsva(0, 0, 1, 1) }
                    GradientStop { position: 1; color: Qt.hsva(root._h / 360, 1, 1, 1) }
                }
            }
            Rectangle {
                anchors.fill: parent
                radius: Theme.radiusSmall
                gradient: Gradient {
                    GradientStop { position: 0; color: Qt.hsva(0, 0, 0, 0) }
                    GradientStop { position: 1; color: Qt.hsva(0, 0, 0, 1) }
                }
            }
            Rectangle {
                x: root._s * square.width - width / 2
                y: (1 - root._v) * square.height - height / 2
                width: 16; height: 16; radius: 8
                color: LightingVm.css(root.hex)
                border.width: 2
                border.color: Theme.focusRing
            }
            MouseArea {
                anchors.fill: parent
                function _set(m) {
                    root._s = Math.max(0, Math.min(1, m.x / width))
                    root._v = 1 - Math.max(0, Math.min(1, m.y / height))
                    root._update(true)
                }
                onPressed: (m) => _set(m)
                onPositionChanged: (m) => { if (pressed) _set(m) }
            }
            Accessible.role: Accessible.Slider
            //% "Saturation and brightness of the colour"
            Accessible.name: qsTrId("light_picker_square")
        }

        // Hue.
        Item {
            id: hueBar
            Layout.fillWidth: true
            Layout.preferredHeight: 22
            Rectangle {
                anchors.fill: parent
                radius: 11
                gradient: Gradient {
                    orientation: Gradient.Horizontal
                    GradientStop { position: 0.0; color: Qt.hsva(0 / 6, 1, 1, 1) }
                    GradientStop { position: 1 / 6; color: Qt.hsva(1 / 6, 1, 1, 1) }
                    GradientStop { position: 2 / 6; color: Qt.hsva(2 / 6, 1, 1, 1) }
                    GradientStop { position: 3 / 6; color: Qt.hsva(3 / 6, 1, 1, 1) }
                    GradientStop { position: 4 / 6; color: Qt.hsva(4 / 6, 1, 1, 1) }
                    GradientStop { position: 5 / 6; color: Qt.hsva(5 / 6, 1, 1, 1) }
                    GradientStop { position: 1.0; color: Qt.hsva(0, 1, 1, 1) }
                }
            }
            Rectangle {
                x: root._h / 360 * (hueBar.width - width)
                y: 0
                width: 14; height: parent.height; radius: 7
                color: Qt.hsva(root._h / 360, 1, 1, 1)
                border.width: 2
                border.color: Theme.focusRing
            }
            MouseArea {
                anchors.fill: parent
                function _set(m) {
                    root._h = Math.max(0, Math.min(1, m.x / width)) * 360
                    root._update(true)
                }
                onPressed: (m) => _set(m)
                onPositionChanged: (m) => { if (pressed) _set(m) }
            }
            Accessible.role: Accessible.Slider
            //% "Hue"
            Accessible.name: qsTrId("light_picker_hue")
        }

        RowLayout {
            spacing: 10
            Rectangle {
                Layout.preferredWidth: 34; Layout.preferredHeight: 34
                radius: Theme.radiusSmall
                color: LightingVm.css(root.hex)
                border.width: 1
                border.color: Theme.swatchLine
            }
            LightNameField {
                id: field
                Layout.fillWidth: true
                maximumLength: 7
                //% "Colour as hex, for example 00aec7"
                placeholderText: qsTrId("light_picker_hex")
                inputMethodHints: Qt.ImhNoPredictiveText
                onTextEdited: {
                    // Only a complete six digit colour counts: no half-typed colour reaches the lights.
                    if (text.replace("#", "").length !== 6) return
                    var h = LightingVm.call("norm_hex", { s: text })
                    if (h === null || h === undefined) return
                    var hsv = LightingVm.call("hex_to_hsv", { hex: h })
                    if (hsv === undefined) return
                    if (hsv.s > 0) root._h = hsv.h
                    root._s = hsv.s
                    root._v = hsv.v
                    root.hex = h
                    root.picked(h)
                }
            }
            SecondaryButton {
                //% "Done"
                text: qsTrId("light_picker_done")
                onClicked: root.close()
            }
        }
    }
}
