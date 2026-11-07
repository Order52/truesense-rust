// What an unbuilt page shows: its title and an honest "coming in the next step". Page agents
// replace the page file, not this component.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

PageFrame {
    title: Nav.currentPage ? Nav.titleOf(Nav.currentPage) : ""

    Card {
        Layout.fillWidth: true
        HelpText {
            //% "This page is coming in the next step."
            text: qsTrId("placeholder_next_step")
        }
    }
}
