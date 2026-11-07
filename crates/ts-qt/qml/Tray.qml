// The system tray icon (a stub: Open and Quit). It is loaded through a Loader, so a desktop or a Qt
// without Qt.labs.platform just has no tray. `Ui.trayAvailable` tells the shell whether closing the
// window may hide it instead of ending the program.
import QtQuick
import Qt.labs.platform as Platform
import org.truesense.app

Platform.SystemTrayIcon {
    id: tray

    signal openRequested()
    signal quitRequested()

    visible: available
    icon.source: "qrc:/assets/truesense-256.png"
    tooltip: "TrueSense"
    onActivated: (reason) => {
        if (reason === Platform.SystemTrayIcon.Trigger) openRequested()
    }
    menu: Platform.Menu {
        Platform.MenuItem {
            //% "Open TrueSense"
            text: qsTrId("tray_open")
            onTriggered: tray.openRequested()
        }
        Platform.MenuSeparator {}
        Platform.MenuItem {
            //% "Quit"
            text: qsTrId("tray_quit")
            onTriggered: tray.quitRequested()
        }
    }
}
