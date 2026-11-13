// Lighting: the strip that binds a look to each power mode, then one tab at a time (Keyboard,
// Per-key, Rear bar, Looks). The tabs are the one list `tabs` below; each is a file in lighting/
// that is created only while its tab shows, so nothing of the other tabs runs or polls.
//
// ADDING A TAB is one entry in `tabs` plus its file in lighting/ (the Themes tab was added that way).
import QtQuick
import QtQuick.Layouts
import org.truesense.app

PageFrame {
    id: page
    objectName: "lightingPage"

    title: Nav.currentPage ? Nav.titleOf(Nav.currentPage) : ""

    /// The tabs in order: id, translated label, icon and the file of the tab (relative to this one).
    readonly property var tabs: [
        //% "Keyboard"
        { id: "keyboard", label: qsTrId("light_tab_keyboard"), icon: "keyboard", source: "lighting/KeyboardTab.qml" },
        //% "Per-key"
        { id: "perkey", label: qsTrId("light_tab_perkey"), icon: "palette", source: "lighting/PerKeyTab.qml" },
        //% "Rear bar"
        { id: "rear", label: qsTrId("light_tab_rear"), icon: "bar", source: "lighting/RearTab.qml" },
        //% "Themes"
        { id: "themes", label: qsTrId("light_tab_themes"), icon: "sparkle", source: "lighting/ThemesTab.qml" },
        //% "Looks"
        { id: "looks", label: qsTrId("light_tab_looks"), icon: "profiles", source: "lighting/LooksTab.qml" }
    ]
    property string tab: "keyboard"
    readonly property string tabSource: {
        for (var i = 0; i < tabs.length; ++i)
            if (tabs[i].id === tab) return tabs[i].source
        return tabs[0].source
    }

    ModeStrip {
        Layout.fillWidth: true
    }

    TileRow {
        Layout.fillWidth: true
        variant: "outlined"
        tileHeight: 60
        minTileWidth: 88
        current: page.tab
        model: page.tabs.map(function (t) { return { value: t.id, label: t.label, icon: t.icon } })
        onActivated: (id) => page.tab = id
    }

    Loader {
        id: body
        Layout.fillWidth: true
        source: page.tabSource
        // A tab that fails to load must be visible in the log, not a silent empty page.
        onStatusChanged: if (status === Loader.Error) console.warn("Lighting: cannot load " + source)
    }
}
