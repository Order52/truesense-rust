// The second polish round: accent colours and density, the drawing choice, the dashboard tile, the
// accent picker and the key caps of the shortcut list.
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import QtTest
import org.truesense.app

TestCase {
    id: rtest
    name: "Round2"
    when: windowShown
    visible: true
    width: 900
    height: 900

    function cleanup() {
        Ui.accent = "cyan"
        Ui.density = "comfortable"
        Ui.theme = "dark"
    }

    // ---- Theme follows Ui ----------------------------------------------------------------------
    function test_the_accent_and_its_derived_colours_follow_the_setting() {
        Ui.accent = "cyan"
        compare(String(Theme.accent), "#2ed3e6")
        Ui.accent = "red"
        compare(String(Theme.accent), "#ff4d5e")
        compare(String(Theme.selBg), "#38191f")
        Ui.theme = "light"
        compare(String(Theme.accent), "#c0283a")
        Ui.accent = "nonsense"
        compare(String(Theme.accent), "#007585", "an unknown accent falls back to cyan")
    }

    function test_compact_density_takes_less_room() {
        var h = Theme.controlHeight, p = Theme.pad, sp = Theme.spacing
        Ui.density = "compact"
        verify(Theme.controlHeight < h && Theme.pad < p && Theme.spacing < sp)
        verify(Theme.controlHeight >= 24, "still a comfortable target (WCAG 2.5.8 asks 24)")
        Ui.density = "comfortable"
        compare(Theme.controlHeight, h)
    }

    // ---- RendererChoice ------------------------------------------------------------------------
    RendererChoice { id: drawing; width: 700; current: "software" }
    SignalSpy { id: drawPicked; target: drawing; signalName: "picked" }

    function test_the_drawing_choice_reports_a_pick_and_says_when_it_applies() {
        drawPicked.clear()
        compare(drawing.startedWith, "software")
        verify(!drawing.pending)
        drawing.current = "software"
        compare(drawPicked.count, 0, "setting it from code says nothing")
        // The second segment is GPU.
        mouseClick(drawing, drawing.width - 40, 18)
        tryCompare(drawPicked, "count", 1)
        compare(drawPicked.signalArguments[0][0], "opengl")
        drawing.current = "opengl"     // what the page does with Ui.set → ui.json → Ui.renderer
        verify(drawing.pending, "a restart note shows")
        drawing.current = "software"
        verify(!drawing.pending)
    }

    // ---- AccentPicker --------------------------------------------------------------------------
    AccentPicker {
        id: accents
        y: 200
        width: 600
        current: "cyan"
        model: [{ id: "cyan", label: "Cyan" }, { id: "red", label: "Red" }]
    }
    SignalSpy { id: accentPicked; target: accents; signalName: "picked" }

    function test_an_accent_swatch_picks_its_colour() {
        accentPicked.clear()
        mouseClick(accents, 64 + 10 + 30, 30)
        compare(accentPicked.count, 1)
        compare(accentPicked.signalArguments[0][0], "red")
    }

    // ---- DashTile ------------------------------------------------------------------------------
    DashTile {
        id: dial
        y: 300; width: 200
        label: "CPU"; icon: "chip"; kind: "dial"; value: 90; to: 105; warnAt: 85; critAt: 95
        valueText: "90 °C"; hot: true; values: [50, 60, 90]
    }
    DashTile {
        id: modeTile
        x: 220; y: 300; width: 200
        label: "Power mode"; icon: "turbo"; kind: "mode"; modeIcon: "turbo"; valueText: "Turbo"
        hasHistory: false; clickable: true
    }
    SignalSpy { id: modeClicked; target: modeTile; signalName: "clicked" }
    SignalSpy { id: dialClicked; target: dial; signalName: "clicked" }

    function test_a_hot_dial_is_outlined_and_says_its_reading() {
        compare(String(dial.border.color), String(Theme.bad))
        compare(dial.Accessible.name, "CPU 90 °C")
        verify(dial.implicitHeight > 150)
    }

    function test_only_the_mode_tile_answers_a_click() {
        modeClicked.clear(); dialClicked.clear()
        mouseClick(modeTile, 100, 100)
        compare(modeClicked.count, 1)
        mouseClick(dial, 100, 100)
        compare(dialClicked.count, 0)
    }

    // ---- KeyCap --------------------------------------------------------------------------------
    KeyCap { id: cap; y: 600; text: "Ctrl+Q" }

    function test_a_key_cap_fits_its_text() {
        verify(cap.implicitWidth > 40)
        compare(cap.Accessible.name, "Ctrl+Q")
    }
}
