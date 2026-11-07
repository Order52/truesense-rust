// The colourways of a theme as small striped chips (the colours side by side); the chosen one is
// outlined. Each chip is named for screen readers by its colourway's name. `picked(index)` fires
// only for the person's own click.
//
//   ColourwayChips { ways: [{ name: "Gas flame", palette: ["#2A1AFF", ...] }]; current: 0; onPicked: (i) => ... }
import QtQuick
import QtQuick.Controls.Basic
import org.truesense.app

Flow {
    id: root

    /// [{name, palette: ["#RRGGBB", ...]}]
    property var ways: []
    property int current: 0
    signal picked(int index)

    spacing: 5

    Repeater {
        model: root.ways
        delegate: Button {
            id: chip
            required property var modelData
            required property int index
            readonly property bool chosen: index === root.current

            implicitWidth: 38
            implicitHeight: 20
            padding: 3
            leftPadding: 3
            rightPadding: 3
            topPadding: 3
            bottomPadding: 3
            hoverEnabled: true
            focusPolicy: Qt.StrongFocus
            Accessible.role: Accessible.RadioButton
            Accessible.name: modelData.name
            Accessible.checkable: true
            Accessible.checked: chosen
            onClicked: root.picked(index)

            contentItem: Row {
                spacing: 0
                Repeater {
                    model: chip.modelData.palette.slice(0, 5)
                    delegate: Rectangle {
                        required property string modelData
                        width: (chip.width - 6) / Math.min(5, chip.modelData.palette.length)
                        height: chip.height - 6
                        color: modelData
                    }
                }
            }
            background: Rectangle {
                radius: 6
                color: Theme.tile
                border.width: chip.chosen || chip.activeFocus ? 2 : 1
                border.color: chip.activeFocus ? Theme.focusRing : (chip.chosen ? Theme.accent : Theme.buttonLine)
            }
            ToolTip.visible: hovered
            ToolTip.text: modelData.name
            ToolTip.delay: 400
        }
    }
}
