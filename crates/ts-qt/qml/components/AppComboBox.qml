// A drop-down list in the app's look: strings or {value, text} options, the current value, and
// `picked(value)` for the person's pick only (changing `current` from code is silent).
//
//   AppComboBox { options: [{ value: "x", text: "Shown" }]; current: "x"; onPicked: (v) => ... }
import QtQuick
import QtQuick.Controls.Basic
import org.truesense.app

ComboBox {
    id: box

    /// Strings, or objects {value, text}.
    property var options: []
    property var current: undefined
    /// Shown (muted) while nothing is chosen; "" for the default wording.
    property string placeholder: ""
    /// The name screen readers say.
    property string label: ""
    /// The person's pick (not a change of `current` from code).
    signal picked(var value)

    readonly property var _items: options.map(function (o) {
        return (typeof o === "object" && o !== null) ? o : { value: o, text: String(o) }
    })

    function _indexOf(value) {
        for (var i = 0; i < _items.length; ++i)
            if (_items[i].value === value) return i
        return -1
    }

    HoverHandler { cursorShape: parent.enabled ? Qt.PointingHandCursor : Qt.ArrowCursor }
    implicitHeight: Theme.controlHeight
    model: box._items
    textRole: "text"
    valueRole: "value"
    currentIndex: box._indexOf(box.current)
    displayText: currentIndex >= 0 ? currentText
        : box.placeholder !== "" ? box.placeholder
        //% "Choose…"
        : box._items.length > 0 ? qsTrId("select_choose")
        //% "Nothing to choose"
        : qsTrId("select_empty")
    hoverEnabled: true
    focusPolicy: Qt.StrongFocus
    Accessible.name: box.label
    onActivated: (index) => box.picked(box._items[index].value)

    contentItem: Text {
        leftPadding: 12
        rightPadding: 28
        text: box.displayText
        color: !box.enabled ? Theme.disabledText : (box.currentIndex >= 0 ? Theme.text : Theme.muted)
        font.pixelSize: Theme.fontBody
        verticalAlignment: Text.AlignVCenter
        elide: Text.ElideRight
    }
    indicator: Icon {
        x: box.mirrored ? 8 : box.width - width - 8
        y: (box.height - height) / 2
        name: "chevron"
        size: 14
        color: Theme.muted
        rotation: 90
    }
    background: Rectangle {
        radius: Theme.radiusSmall + 1
        color: box.hovered ? Theme.buttonHover : Theme.button
        border.width: 1
        border.color: box.activeFocus ? Theme.focusRing : Theme.buttonLine
        opacity: box.enabled ? 1 : 0.5
    }
    delegate: ItemDelegate {
        id: item
        required property var modelData
        required property int index
        width: box.width
        height: Theme.controlHeight
        highlighted: box.highlightedIndex === index
        contentItem: Text {
            text: item.modelData.text
            color: Theme.text
            font.pixelSize: Theme.fontBody
            verticalAlignment: Text.AlignVCenter
            elide: Text.ElideRight
            font.bold: item.index === box.currentIndex
        }
        background: Rectangle {
            color: item.highlighted ? Theme.selBg : "transparent"
            radius: 5
        }
    }
    popup: Popup {
        y: box.height + 4
        width: box.width
        implicitHeight: Math.min(contentItem.implicitHeight + 12, 320)
        padding: 6
        contentItem: ListView {
            clip: true
            implicitHeight: contentHeight
            model: box.popup.visible ? box.delegateModel : null
            currentIndex: box.highlightedIndex
            ScrollIndicator.vertical: ScrollIndicator {}
        }
        background: Rectangle {
            radius: Theme.radius
            color: Theme.dialog
            border.color: Theme.cardLine
        }
    }
}
