// How the window draws: "Light" (Qt Quick's software renderer, about 100 MB) or "GPU" (OpenGL,
// smoother on some desktops, about 300 MB). The program reads the choice (ui.json `renderer`) before
// Qt starts, so a change applies at the next start; the line under the choice says so once it was
// changed. `picked(value)` only for the person's own click.
//
//   RendererChoice { current: Ui.renderer; onPicked: (v) => Ui.set("renderer", v) }
import QtQuick
import QtQuick.Layouts
import org.truesense.app

ColumnLayout {
    id: root

    /// "software" or "opengl".
    property string current: "software"
    /// What the window started with (the choice when it was created); a different `current` means a
    /// restart is pending.
    property string startedWith: ""
    readonly property bool pending: startedWith !== "" && current !== startedWith
    signal picked(string value)

    spacing: 6
    Component.onCompleted: startedWith = current

    SettingRow {
        Layout.fillWidth: true
        //% "Drawing"
        label: qsTrId("settings_renderer")
        Segmented {
            current: root.current
            model: [
                //% "Light (software, about 100 MB)"
                { value: "software", label: qsTrId("settings_renderer_software") },
                //% "GPU (smoother on some desktops, about 300 MB)"
                { value: "opengl", label: qsTrId("settings_renderer_gpu") }
            ]
            onActivated: (v) => root.picked(v)
        }
    }
    HelpText {
        //% "If parts of the window look stale or only update under the pointer, try GPU."
        text: qsTrId("settings_renderer_help")
    }
    Text {
        Layout.fillWidth: true
        visible: root.pending
        //% "Applies the next time TrueSense starts: close it (Ctrl+Q) and open it again."
        text: qsTrId("settings_renderer_restart")
        color: Theme.warn
        font.pixelSize: Theme.fontSmall + 1
        wrapMode: Text.Wrap
    }
}
