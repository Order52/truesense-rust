import QtQuick
import QtTest
import QtQuick.Controls.Basic
import org.truesense.app

TestCase {
    id: gtest
    name: "Gallery"
    when: windowShown
    visible: true
    width: 700
    height: 700

    readonly property var layout: ({
        width: 6, height: 2,
        keys: [
            { name: "a", cells: [0], x: 0, y: 0, w: 1, h: 1 },
            { name: "b", cells: [1], x: 1, y: 0, w: 1, h: 1 },
            { name: "space", cells: [2], x: 0, y: 1, w: 4, h: 1 }
        ]
    })

    KeyboardThumb {
        id: thumb
        width: 240
        height: 80
        layout: gtest.layout
    }

    GalleryCard {
        id: card
        y: 100
        width: 260
        title: "Hearth"
        subtitle: "A calm fire"
        layout: gtest.layout
        cells: ["ff0000", "00ff00", "0000ff"]
        starrable: true
        starName: "Star this theme"
        badge: "Running"
        Button { id: extraButton; text: "Apply" }
    }
    SignalSpy { id: activated; target: card; signalName: "activated" }
    SignalSpy { id: starSpy; target: card; signalName: "starToggled" }

    FilterChips {
        id: chips
        y: 330
        width: 400
        model: [
            { id: "all", label: "All", count: 32 },
            { id: "gaming", label: "Gaming", count: 9 },
            { id: "yours", label: "Yours", count: 0 }
        ]
        current: "all"
    }
    SignalSpy { id: picked; target: chips; signalName: "picked" }

    ColourwayChips {
        id: ways
        y: 400
        width: 300
        ways: [
            { name: "Gas flame", palette: ["#2A1AFF", "#0050FF", "#00E5FF"] },
            { name: "Embers", palette: ["#FF2200", "#FF8800"] },
            { name: "Mono", palette: ["#FFFFFF"] }
        ]
        current: 0
    }
    SignalSpy { id: wayPicked; target: ways; signalName: "picked" }

    function init() {
        activated.clear(); starSpy.clear(); picked.clear(); wayPicked.clear()
        chips.current = "all"; ways.current = 0
    }

    function test_the_thumbnail_draws_every_key_of_the_layout() {
        thumb.cells = ["112233", "000000"]
        tryVerify(function () { return thumb.painted === 3 })
        compare(thumb.unit, 40)
    }

    function test_the_thumbnail_keeps_working_without_cells_or_keys() {
        thumb.cells = []
        thumb.layout = ({ width: 6, height: 2, keys: [] })
        tryVerify(function () { return thumb.painted === 0 })
        thumb.layout = gtest.layout
        tryVerify(function () { return thumb.painted === 3 })
    }

    function test_a_click_on_the_picture_selects_the_card() {
        mouseClick(card, 100, 30)
        compare(activated.count, 1)
        compare(starSpy.count, 0)
    }

    function test_the_star_toggles_without_selecting() {
        // The star sits at the right end of the title row.
        var hit = false
        for (var x = 200; x < 256 && !hit; x += 4) {
            starSpy.clear(); activated.clear()
            mouseClick(card, x, 10 + 6 + card.width * 2 / 6 + 6 + 15)
            hit = starSpy.count > 0
        }
        verify(hit, "the star button answers a click at the end of the title row")
        compare(activated.count, 0, "the star does not select the card")
    }

    function test_extra_controls_go_under_the_text_and_the_badge_shows() {
        var below = extraButton.mapToItem(card, 0, 0).y
        verify(below > 80, "below the picture and the title: " + below)
        compare(card.badge, "Running")
        verify(card.implicitHeight > below)
    }

    function test_a_filter_chip_picks_and_an_empty_one_cannot_be_clicked() {
        mouseClick(chips, 110, 15)
        compare(picked.count, 1)
        compare(picked.signalArguments[0][0], "gaming")
        mouseClick(chips, 230, 15)
        verify(picked.count <= 2)
        for (var i = 0; i < picked.count; ++i) verify(picked.signalArguments[i][0] !== "yours", "an empty chip is disabled")
    }

    function test_changing_the_chip_from_code_never_picks() {
        chips.current = "gaming"
        chips.current = "all"
        compare(picked.count, 0)
    }

    function test_colourway_chips_report_the_index_only_for_a_click() {
        mouseClick(ways, 60, 10)
        compare(wayPicked.count, 1)
        compare(wayPicked.signalArguments[0][0], 1)
        ways.current = 2
        compare(wayPicked.count, 1)
    }
}
