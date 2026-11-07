// A list of labelled horizontal bars: one row per core, per filesystem, per memory part. Each row
// is  label | bar | value text  with an optional second line (`sub`) under the label.
//
//   BarList {
//       columns: 2
//       model: [ { label: "0", text: "37 %", sub: "3.2 GHz", value: 0.37, level: "ok" } ]
//   }
//
// `value` is a fraction 0..1, or null for "not measured": the bar stays empty and the text is
// whatever the page puts there (a dash), never a zero bar that looks like a reading. `level`
// ("ok", "warn", "bad", "unknown") colours the fill; the text always carries the number, so the
// colour is never the only signal. Bars fill from the start edge of the language (they do mirror
// in right-to-left, like any progress bar), the numbers stay left to right.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

GridLayout {
    id: root

    /// [{label, text, sub?, value, level}]
    property var model: []
    property int barHeight: 8
    /// Width of the label column in pixels; 0 sizes it to the longest label.
    property int labelWidth: 0
    property int textWidth: 56
    /// Said when the list is empty.
    property string emptyText: ""

    readonly property int rowCount: model.length

    function levelColor(level) {
        return level === "bad" ? Theme.bad : level === "warn" ? Theme.warn : level === "unknown" ? Theme.fill : Theme.accent
    }

    // `columns` is the GridLayout's own: rows side by side (a long list of cores uses two on a wide window).
    columns: 1
    columnSpacing: 22
    rowSpacing: 8
    Accessible.role: Accessible.List

    Text {
        Layout.columnSpan: Math.max(1, root.columns)
        Layout.fillWidth: true
        visible: root.model.length === 0 && root.emptyText !== ""
        text: root.emptyText
        color: Theme.muted
        font.pixelSize: Theme.fontSmall + 1
        wrapMode: Text.Wrap
    }

    Repeater {
        model: root.model
        delegate: RowLayout {
            id: row
            required property var modelData
            readonly property real fraction: (modelData.value === null || modelData.value === undefined || isNaN(modelData.value))
                ? 0 : Math.max(0, Math.min(1, modelData.value))

            Layout.fillWidth: true
            Layout.preferredWidth: 1
            spacing: 10
            Accessible.role: Accessible.ProgressBar
            Accessible.name: modelData.label
            Accessible.description: modelData.text

            ColumnLayout {
                // A nested layout fills by default when a child does: say no, the bar fills.
                Layout.fillWidth: false
                Layout.preferredWidth: root.labelWidth > 0 ? root.labelWidth : implicitWidth
                Layout.minimumWidth: root.labelWidth
                spacing: 0
                Text {
                    Layout.fillWidth: true
                    text: row.modelData.label
                    color: Theme.text
                    font.pixelSize: Theme.fontSmall + 1
                    elide: Text.ElideRight
                }
                Text {
                    Layout.fillWidth: true
                    visible: (row.modelData.sub || "") !== ""
                    text: row.modelData.sub || ""
                    color: Theme.muted
                    font.pixelSize: Theme.fontSmall - 1
                    elide: Text.ElideRight
                }
            }

            Rectangle {
                Layout.fillWidth: true
                Layout.preferredHeight: root.barHeight
                radius: height / 2
                color: Theme.groove
                Rectangle {
                    width: parent.width * row.fraction
                    height: parent.height
                    radius: parent.radius
                    visible: row.fraction > 0
                    color: root.levelColor(row.modelData.level)
                    Behavior on width { NumberAnimation { duration: Theme.animMs; easing.type: Easing.OutCubic } }
                }
            }

            Text {
                Layout.preferredWidth: root.textWidth
                text: row.modelData.text
                color: Theme.stat
                font.pixelSize: Theme.fontSmall + 1
                horizontalAlignment: Text.AlignRight
                LayoutMirroring.enabled: false
            }
        }
    }
}
