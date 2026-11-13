// The accent colours as round swatches with their names; the chosen one has a ring and a check.
// `picked(id)` only for the person's own click or key.
//
//   AccentPicker { current: Ui.accent; onPicked: (id) => Ui.set("accent", id) }
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import org.truesense.app

Flow {
    id: root

    property string current: "cyan"
    /// [{id, label}] in display order; the colours come from Theme.accents.
    property var model: []
    signal picked(string id)

    spacing: 10

    Repeater {
        model: root.model
        delegate: Button {
            id: sw
            required property var modelData
            readonly property bool chosen: modelData.id === root.current
            readonly property color tint: Theme.accents[modelData.id] ? Theme.accents[modelData.id][Theme.dark ? 0 : 1] : Theme.accent
            implicitWidth: Math.max(64, nameText.implicitWidth + 12)
            implicitHeight: 64
            hoverEnabled: true
            focusPolicy: Qt.StrongFocus
            Accessible.role: Accessible.RadioButton
            Accessible.name: modelData.label
            Accessible.checked: chosen
            onClicked: root.picked(modelData.id)

            contentItem: ColumnLayout {
                spacing: 6
                Rectangle {
                    Layout.alignment: Qt.AlignHCenter
                    width: 30; height: 30; radius: 15
                    color: sw.tint
                    border.width: sw.chosen ? 3 : (sw.activeFocus ? 2 : 0)
                    border.color: sw.chosen || sw.activeFocus ? Theme.text : "transparent"
                    Icon {
                        anchors.centerIn: parent
                        visible: sw.chosen
                        name: "check"
                        size: 16
                        color: Theme.accentInk
                    }
                }
                Text {
                    id: nameText
                    Layout.alignment: Qt.AlignHCenter
                    text: sw.modelData.label
                    color: sw.chosen ? Theme.text : Theme.muted
                    font.pixelSize: Theme.fontSmall
                    font.bold: sw.chosen
                }
            }
            background: Rectangle {
                radius: Theme.radiusSmall
                color: sw.hovered ? Theme.buttonHover : "transparent"
            }
        }
    }
}
