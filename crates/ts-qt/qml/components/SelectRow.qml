// A name with a drop-down list (Smart profile, curve, output device).
//
//   SelectRow { label: "..."; options: ["a", "b"]; current: "a"; onActivated: (v) => ... }
//   SelectRow { options: [{ value: "x", text: "Shown text" }, ...] }
//
// `activated(value)` is the person's pick only; changing `current` from code is silent.
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import org.truesense.app

SettingRow {
    id: root

    /// Strings, or objects {value, text}.
    property var options: []
    property var current: undefined
    property int minimumControlWidth: 180
    /// Shown (muted) while nothing is chosen, so the box never looks empty or broken; "" for the
    /// default wording ("Choose…", or "Nothing to choose" without options).
    property string placeholder: ""
    signal activated(var value)

    readonly property var _items: options.map(function (o) {
        return (typeof o === "object" && o !== null) ? o : { value: o, text: String(o) }
    })

    function _indexOf(value) {
        for (var i = 0; i < _items.length; ++i)
            if (_items[i].value === value) return i
        return -1
    }

    ComboBox {
        id: box
        HoverHandler { cursorShape: parent.enabled ? Qt.PointingHandCursor : Qt.ArrowCursor }
        Layout.minimumWidth: root.minimumControlWidth
        Layout.preferredHeight: Theme.controlHeight
        model: root._items
        textRole: "text"
        valueRole: "value"
        currentIndex: root._indexOf(root.current)
        displayText: currentIndex >= 0 ? currentText
            : root.placeholder !== "" ? root.placeholder
            //% "Choose…"
            : root._items.length > 0 ? qsTrId("select_choose")
            //% "Nothing to choose"
            : qsTrId("select_empty")
        hoverEnabled: true
        focusPolicy: Qt.StrongFocus
        Accessible.name: root.label
        onActivated: (index) => root.activated(root._items[index].value)

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
}
