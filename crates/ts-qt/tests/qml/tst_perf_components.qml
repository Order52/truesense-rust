import QtQuick
import QtTest
import org.truesense.app

TestCase {
    name: "Performance components"
    when: windowShown
    visible: true
    width: 600
    height: 400

    Badge { id: badge; text: "Changed"; kind: "warn" }
    RiskLine { id: risk; level: "caution"; why: "Past the safety line." }
    TextFieldRow { id: field; x: 0; y: 60; width: 480; label: "Save as"; maxLength: 5 }
    SignalSpy { id: typed; target: field; signalName: "edited" }
    ZoneBar { id: zone; x: 0; y: 140; width: 300; from: 0; to: 120; safeUntil: 60; cautionUntil: 90; value: 100 }

    function init() { typed.clear(); field.text = "" }

    function test_a_badge_says_its_word_in_text() {
        compare(badge.Accessible.name, "Changed")
        verify(badge.width > 0)
    }

    function test_a_risk_line_shows_only_for_a_known_level() {
        verify(risk.visible)
        risk.level = ""
        verify(!risk.visible)
        risk.level = "danger"
        verify(risk.visible)
        risk.level = "safe"
        verify(risk.visible)
        risk.level = "nonsense"
        verify(!risk.visible)
    }

    function test_the_text_row_reports_typing_only() {
        field.text = "ab"
        compare(typed.count, 0)
        field.field.forceActiveFocus()
        keyClick("c")
        compare(typed.count, 1)
        compare(field.text, "abc")
    }

    function test_the_text_row_honours_its_maximum_length() {
        field.text = ""
        field.field.forceActiveFocus()
        for (var i = 0; i < 8; ++i) keyClick("x")
        compare(field.text.length, 5)
    }

    function test_the_zone_bar_names_a_different_zone_for_each_range() {
        zone.value = 100
        var danger = zone.Accessible.name
        zone.value = 70
        var caution = zone.Accessible.name
        zone.value = 10
        var safe = zone.Accessible.name
        verify(danger !== caution && caution !== safe && danger !== safe)
    }
}
