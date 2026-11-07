// A question before something that matters. The safe button has the focus when it opens (Cancel),
// Esc cancels, and with `typeWord` the confirm button stays off until the person has typed that word
// (the danger zone: "APPLY"). The page asks, but the command layer refuses unconfirmed dangerous
// commands anyway (COMMANDS.md), so this is the polite half of the rule.
//
//   ConfirmDialog { id: confirm }
//   ...
//   //% "Turn the fans off?"
//   confirm.ask({ title: qsTrId("x_title"), text: "...", confirmText: "Turn off", danger: true },
//               function () { Api.call("fan", "off", { confirm: true }) })
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import org.truesense.app

Popup {
    id: root

    property string title: ""
    property string text: ""
    //% "OK"
    property string confirmText: qsTrId("dialog_confirm")
    //% "Cancel"
    property string cancelText: qsTrId("dialog_cancel")
    property bool danger: false
    /// When set, the confirm button works only after this word was typed.
    property string typeWord: ""
    property var _onAccept: null

    signal accepted()
    signal rejected()

    /// Opens the dialog with the given options (title, text, confirmText, cancelText, danger,
    /// typeWord) and calls `onAccept` if the person confirms.
    function ask(options, onAccept) {
        title = options.title === undefined ? "" : options.title
        text = options.text === undefined ? "" : options.text
        confirmText = options.confirmText === undefined ? qsTrId("dialog_confirm") : options.confirmText
        cancelText = options.cancelText === undefined ? qsTrId("dialog_cancel") : options.cancelText
        danger = options.danger === true
        typeWord = options.typeWord === undefined ? "" : options.typeWord
        _onAccept = onAccept === undefined ? null : onAccept
        open()
    }

    /// The two buttons, for tests and for pages that want to move the focus.
    readonly property alias confirmButton: confirm
    readonly property alias cancelButton: cancel
    readonly property bool confirmEnabled: typeWord === "" || typed.text.trim() === typeWord

    parent: Overlay.overlay
    anchors.centerIn: Overlay.overlay
    modal: true
    focus: true
    padding: Theme.pad + 4
    width: Math.min(440, (parent ? parent.width : 440) - 32)
    closePolicy: Popup.CloseOnEscape
    Overlay.modal: Rectangle { color: Theme.scrim }

    onAboutToShow: typed.text = ""
    onOpened: cancel.forceActiveFocus()
    onClosed: if (!_decided) rejected()

    property bool _decided: false
    function _decide(yes) {
        _decided = true
        close()
        if (yes) {
            accepted()
            if (_onAccept) _onAccept()
        } else {
            rejected()
        }
        _decided = false
    }

    background: Rectangle {
        radius: Theme.radiusLarge
        color: Theme.dialog
        border.color: root.danger ? Theme.bad : Theme.cardLine
        border.width: 1
    }

    contentItem: ColumnLayout {
        spacing: Theme.spacing

        Text {
            Layout.fillWidth: true
            text: root.title
            color: Theme.text
            font.pixelSize: Theme.fontSection + 2
            font.bold: true
            wrapMode: Text.Wrap
            Accessible.role: Accessible.Heading
        }
        Text {
            Layout.fillWidth: true
            visible: root.text !== ""
            text: root.text
            color: Theme.stat
            font.pixelSize: Theme.fontBody
            wrapMode: Text.Wrap
        }
        Text {
            Layout.fillWidth: true
            visible: root.typeWord !== ""
            //% "Type %1 to go ahead:"
            text: qsTrId("dialog_type_word").arg(root.typeWord)
            color: Theme.muted
            font.pixelSize: Theme.fontSmall + 1
            wrapMode: Text.Wrap
        }
        TextField {
            id: typed
            Layout.fillWidth: true
            visible: root.typeWord !== ""
            color: Theme.text
            font.pixelSize: Theme.fontBody
            placeholderText: root.typeWord
            Accessible.name: qsTrId("dialog_type_word").arg(root.typeWord)
            onAccepted: if (root.confirmEnabled) root._decide(true)
            background: Rectangle {
                radius: Theme.radiusSmall
                color: Theme.field
                border.color: typed.activeFocus ? Theme.focusRing : Theme.buttonLine
            }
        }
        RowLayout {
            Layout.fillWidth: true
            Layout.topMargin: 4
            spacing: 10
            Item { Layout.fillWidth: true }
            AppButton {
                id: cancel
                text: root.cancelText
                kind: "secondary"
                onClicked: root._decide(false)
            }
            AppButton {
                id: confirm
                text: root.confirmText
                kind: root.danger ? "danger" : "primary"
                enabled: root.confirmEnabled
                onClicked: root._decide(true)
            }
        }
    }
}
