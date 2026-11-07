// The shell: window, sidebar (full, icon rail or menu button depending on the width), page area,
// status line, toasts. Pages are loaded from qml/pages by the registry and know nothing of this file.
// Only this file talks to the Rust `Bus` object; it feeds the QML singletons (Ui, Live, Caps, Nav,
// Api) that pages use.
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import QtQuick.Window
import org.truesense.app

ApplicationWindow {
    id: win

    // ---- size, position and what the layout does at each width -------------------------------
    readonly property var saved: JSON.parse(Bus.uiJson)
    /// 2: sidebar with titles, 1: icon rail, 0: menu button.
    readonly property int layoutMode: width >= Theme.breakIcons ? 2 : (width >= Theme.breakMenu ? 1 : 0)
    /// Visible on screen: the live values tick only then (nothing runs while hidden or minimised).
    readonly property bool active: visible && visibility !== Window.Minimized && visibility !== Window.Hidden

    width: saved.window_width
    height: saved.window_height
    minimumWidth: 360
    minimumHeight: 480
    visible: !Bus.startHidden
    title: "TrueSense"
    color: Theme.bg
    LayoutMirroring.enabled: Ui.rtl
    LayoutMirroring.childrenInherit: true

    // ---- the singletons are fed from here -----------------------------------------------------
    Binding { target: Nav; property: "headerShowsTitle"; value: win.layoutMode === 0 }
    Binding { target: Api; property: "transport"; value: Bus }

    Connections {
        target: Bus
        function onLiveJsonChanged() { Live.load(Bus.liveJson) }
        function onLiveOkChanged() { if (!Bus.liveOk) Live.markLost() }
        function onCapsJsonChanged() { Caps.load(Bus.capsJson) }
        function onUiJsonChanged() { Ui.load(Bus.uiJson) }
        function onShowRequested(page) { win.showPage(page) }
    }

    Connections {
        target: Ui
        function onChangeRequested(key, value) {
            var err = Bus.setUi(key, JSON.stringify(value))
            if (err !== "") Toasts.error(err)
        }
    }

    Connections {
        target: Nav
        function onRaiseRequested() { win.bringToFront() }
    }

    onActiveChanged: {
        Bus.setActive(active)
        if (active) Bus.refreshCapabilities()
    }

    Component.onCompleted: {
        Nav.load(Bus.routesJson)
        Ui.load(Bus.uiJson)
        Nav.go(Bus.startPage)
        restorePosition()
        Bus.setActive(active)
        if (active) Bus.refreshCapabilities()
    }

    /// Brings the window to the front, from anywhere (second start, tray, PREDATOR key).
    function bringToFront() {
        if (visibility === Window.Minimized) showNormal()
        else show()
        raise()
        requestActivate()
    }

    /// Goes to `page` (a registry id, "" = stay) and brings the window forward.
    function showPage(page) {
        if (page !== "") Nav.go(page)
        bringToFront()
    }

    function restorePosition() {
        if (saved.x === undefined || saved.y === undefined) return
        if (Qt.platform.pluginName === "wayland" || Qt.platform.pluginName === "offscreen") return
        var w = Screen.desktopAvailableWidth, h = Screen.desktopAvailableHeight
        // Only where at least part of the title bar will be on a screen.
        if (saved.x > -width + 120 && saved.x < w - 120 && saved.y >= 0 && saved.y < h - 80) {
            x = saved.x
            y = saved.y
        }
    }

    Timer {
        id: saveGeometry
        interval: 700
        onTriggered: {
            if (win.visibility !== Window.Windowed) return
            var hasPos = Qt.platform.pluginName !== "wayland" && Qt.platform.pluginName !== "offscreen"
            Bus.saveWindow(win.width, win.height, win.x, win.y, hasPos)
        }
    }
    onWidthChanged: saveGeometry.restart()
    onHeightChanged: saveGeometry.restart()
    onXChanged: saveGeometry.restart()
    onYChanged: saveGeometry.restart()

    onClosing: (close) => {
        saveGeometry.stop()
        if (win.visibility === Window.Windowed)
            Bus.saveWindow(win.width, win.height, win.x, win.y,
                           Qt.platform.pluginName !== "wayland" && Qt.platform.pluginName !== "offscreen")
        if (Ui.minimiseToTray && Ui.trayAvailable) {
            close.accepted = false
            win.hide()
        } else {
            Qt.quit()
        }
    }

    // ---- first frame, test hook ---------------------------------------------------------------
    property bool firstFrame: false
    onFrameSwapped: {
        if (firstFrame) return
        firstFrame = true
        if (Bus.framePresented()) {
            testHook.window = win
            testHook.target = shell
            testHook.config = JSON.parse(Bus.testConfig())
            testHook.begin()
        }
    }
    TestHook { id: testHook }

    // ---- keyboard -----------------------------------------------------------------------------
    Repeater {
        model: Nav.pages
        delegate: Item {
            required property var modelData
            Shortcut {
                enabled: modelData.shortcut > 0
                sequence: "Ctrl+" + modelData.shortcut
                onActivated: Nav.go(modelData.id)
            }
        }
    }
    Shortcut { sequences: [StandardKey.Quit]; onActivated: Qt.quit() }
    Shortcut { sequence: "Ctrl+W"; onActivated: win.close() }

    // ---- the tray (optional) ------------------------------------------------------------------
    Loader {
        id: trayLoader
        // Never under the offscreen platform (tests): it would put an icon on the real desktop's tray.
        source: Qt.platform.pluginName === "offscreen" ? "" : "Tray.qml"
        onLoaded: {
            Ui.trayAvailable = item.available
            item.openRequested.connect(win.bringToFront)
            item.quitRequested.connect(Qt.quit)
        }
    }

    // ---- layout -------------------------------------------------------------------------------
    Item {
        id: shell
        anchors.fill: parent

        // The window's own background does not appear in a grab of this item (tests, screenshots).
        Rectangle { anchors.fill: parent; color: Theme.bg }

    RowLayout {
        id: row
        anchors.fill: parent
        spacing: 0

        Sidebar {
            visible: win.layoutMode >= 1
            compact: win.layoutMode === 1
            Layout.fillHeight: true
            Layout.preferredWidth: compact ? Theme.sidebarNarrow : Theme.sidebarWide
        }

        ColumnLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 0

            Rectangle {
                id: header
                visible: win.layoutMode === 0
                Layout.fillWidth: true
                implicitHeight: 48
                color: Theme.sidebar
                Rectangle { anchors.bottom: parent.bottom; width: parent.width; height: 1; color: Theme.sep }
                RowLayout {
                    anchors.fill: parent
                    anchors.leftMargin: 8
                    anchors.rightMargin: 12
                    spacing: 8
                    AppButton {
                        iconName: "menu"
                        text: ""
                        implicitWidth: 40
                        //% "Pages"
                        Accessible.name: qsTrId("shell_menu")
                        onClicked: narrowMenu.open()
                    }
                    Text {
                        Layout.fillWidth: true
                        text: Nav.currentPage ? Nav.titleOf(Nav.currentPage) : ""
                        color: Theme.text
                        font.pixelSize: Theme.fontSection + 1
                        font.bold: true
                        elide: Text.ElideRight
                    }
                }
            }

            Loader {
                id: pageLoader
                Layout.fillWidth: true
                Layout.fillHeight: true
                source: Nav.currentQml !== "" ? "pages/" + Nav.currentQml + ".qml" : ""
                onStatusChanged: {
                    if (status === Loader.Error)
                        //% "This page could not be loaded."
                        Toasts.error(qsTrId("shell_page_failed")
                        )
                }
            }

            StatusLine {
                Layout.fillWidth: true
                compact: win.width < 600
            }
        }
    }

    // ---- first run (once; remembered in ui.json) -----------------------------------------------
    // Never in a test run: a fresh test home would put the wizard over every screenshot.
    Loader {
        anchors.fill: parent
        active: !Ui.onboarded && !JSON.parse(Bus.testConfig()).enabled
        source: "components/OnboardingWizard.qml"
    }

    ToastHost { anchors.fill: parent }
    }

    NarrowMenu { id: narrowMenu }
}
