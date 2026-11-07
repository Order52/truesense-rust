// Settings: the app's own preferences (theme, language, tray, motion, text size) are real already,
// because the shell needs them; the rest of the page is a placeholder.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

PageFrame {
    title: Nav.currentPage ? Nav.titleOf(Nav.currentPage) : ""

    Card {
        Layout.fillWidth: true
        //% "Appearance"
        title: qsTrId("settings_appearance")
        icon: "palette"

        TileRow {
            Layout.fillWidth: true
            variant: "outlined"
            current: Ui.theme
            model: [
                //% "Dark"
                { value: "dark", label: qsTrId("settings_theme_dark"), icon: "moon" },
                //% "Light"
                { value: "light", label: qsTrId("settings_theme_light"), icon: "sun" },
                //% "System"
                { value: "system", label: qsTrId("settings_theme_system"), icon: "auto" }
            ]
            onActivated: (value) => Ui.set("theme", value)
        }
        SelectRow {
            Layout.fillWidth: true
            //% "Language"
            label: qsTrId("settings_language")
            current: Ui.languageSetting
            options: [
                //% "Follow the desktop"
                { value: "system", text: qsTrId("settings_lang_system") },
                { value: "en", text: "English" },
                { value: "ar", text: "العربية" }
            ]
            onActivated: (value) => Ui.set("language", value)
        }
        SliderRow {
            Layout.fillWidth: true
            //% "Text size"
            label: qsTrId("settings_text_size")
            from: 80
            to: 160
            stepSize: 10
            unit: "%"
            value: Ui.fontScale * 100
            onReleased: (v) => Ui.set("font_scale", v / 100)
        }
        ToggleRow {
            Layout.fillWidth: true
            //% "Reduce motion"
            label: qsTrId("settings_reduce_motion")
            //% "Turns off easing of gauges and other non-essential animation."
            sub: qsTrId("settings_reduce_motion_sub")
            checked: Ui.reducedMotion
            onToggled: (on) => Ui.set("reduced_motion", on)
        }
        ToggleRow {
            Layout.fillWidth: true
            //% "Keep running in the tray when the window is closed"
            label: qsTrId("settings_tray")
            sub: Ui.trayAvailable ? ""
                //% "This desktop shows no tray, so closing the window ends the app."
                : qsTrId("settings_tray_missing")
            enabled: Ui.trayAvailable
            checked: Ui.minimiseToTray
            onToggled: (on) => Ui.set("minimise_to_tray", on)
        }
        HelpText {
            visible: Ui.problem !== ""
            text: Ui.problem
            color: Theme.warn
        }
    }

    Card {
        Layout.fillWidth: true
        HelpText {
            //% "This page is coming in the next step."
            text: qsTrId("placeholder_next_step")
        }
    }
}
