// Shown for a page name that is not in registry.json.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

PageFrame {
    //% "Page not found"
    title: qsTrId("notfound_title")

    Card {
        Layout.fillWidth: true
        HelpText {
            //% "There is no page called \"%1\"."
            text: qsTrId("notfound_text").arg(Nav.missing)
        }
        SecondaryButton {
            //% "Go to Home"
            text: qsTrId("notfound_home")
            onClicked: Nav.go("home")
        }
    }
}
