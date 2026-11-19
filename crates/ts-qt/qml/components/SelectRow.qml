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

    AppComboBox {
        Layout.minimumWidth: root.minimumControlWidth
        Layout.preferredHeight: Theme.controlHeight
        options: root.options
        current: root.current
        placeholder: root.placeholder
        label: root.label
        onPicked: (value) => root.activated(value)
    }
}
