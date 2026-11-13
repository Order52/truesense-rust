// The parts the polish pass added or changed: the gallery grid (equal cards, columns that come back
// after a narrow window, a detail row under the chosen card's row), the select box that is never
// blank, the quiet danger button, the tinted icon button and the slider's value text.
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import QtTest
import org.truesense.app

TestCase {
    id: ptest
    name: "Polish"
    when: windowShown
    visible: true
    width: 900
    height: 900

    // ---- GalleryGrid ---------------------------------------------------------------------------
    ColumnLayout {
        id: holder
        width: 870
        GalleryGrid {
            id: grid
            minCardWidth: 260
            Repeater {
                id: cards
                model: 5
                delegate: Rectangle {
                    required property int index
                    Layout.preferredWidth: grid.cardWidth
                    Layout.fillHeight: true
                    Layout.row: grid.rowOf(index)
                    Layout.column: grid.columnOf(index)
                    implicitHeight: index === 1 ? 120 : 60
                    color: "gray"
                }
            }
            Rectangle {
                id: detail
                Layout.row: Math.max(0, grid.detailRow)
                Layout.column: 0
                Layout.columnSpan: grid.columns
                Layout.fillWidth: true
                visible: grid.detailAfter >= 0
                implicitHeight: 40
                color: "blue"
            }
        }
    }

    function test_cards_share_one_width_and_a_lone_card_is_not_stretched() {
        tryCompare(grid, "columns", 3)
        var w = cards.itemAt(0).width
        verify(Math.abs(w - grid.cardWidth) < 1, "card " + w + " grid " + grid.cardWidth)
        verify(Math.abs(cards.itemAt(4).width - w) < 1, "the last, lone card keeps the width of the others")
        verify(w < 300, "three columns: " + w)
    }

    function test_the_cards_of_a_row_are_as_tall_as_the_tallest() {
        tryCompare(grid, "columns", 3)
        tryVerify(function () { return cards.itemAt(0).height === 120 && cards.itemAt(2).height === 120 })
        compare(cards.itemAt(3).height, 60, "another row keeps its own height")
    }

    function test_the_grid_gets_its_columns_back_after_being_narrow() {
        holder.width = 300
        tryCompare(grid, "columns", 1)
        holder.width = 870
        tryCompare(grid, "columns", 3)
        holder.width = 600
        tryCompare(grid, "columns", 2)
        holder.width = 870
        tryCompare(grid, "columns", 3)
    }

    function test_the_detail_opens_under_the_row_of_its_card() {
        tryCompare(grid, "columns", 3)
        grid.detailAfter = 1
        compare(grid.detailRow, 1)
        compare(grid.rowOf(1), 0)
        compare(grid.rowOf(3), 2, "the next row moves below the detail")
        tryVerify(function () { return detail.visible && detail.y > cards.itemAt(1).y + cards.itemAt(1).height - 1 })
        tryVerify(function () { return cards.itemAt(3).y > detail.y + detail.height - 1 })
        verify(Math.abs(detail.width - grid.width) < 1, "the detail spans the whole grid")
        grid.detailAfter = 4
        compare(grid.detailRow, 2)
        compare(grid.rowOf(3), 1)
        grid.detailAfter = -1
        compare(grid.rowOf(3), 1)
        verify(!detail.visible)
    }

    // ---- SelectRow -----------------------------------------------------------------------------
    SelectRow { id: sel; y: 500; width: 400; label: "Profile"; options: ["a", "b"] }
    SelectRow { id: none; y: 560; width: 400; label: "Mode"; options: [] }
    SelectRow { id: own; y: 620; width: 400; label: "Look"; options: ["x"]; placeholder: "Pick a look" }

    function findText(item, wanted) {
        if (item.text !== undefined && item.text === wanted && item.color !== undefined) return item
        for (var i = 0; i < item.children.length; ++i) {
            var r = findText(item.children[i], wanted)
            if (r) return r
        }
        if (item.contentItem && item.contentItem !== item) return findText(item.contentItem, wanted)
        return null
    }

    // Without the app's catalogue qsTrId gives the id, with it the English text: either will do.
    function shows(item, id, english) { return findText(item, id) !== null || findText(item, english) !== null }

    function test_a_select_without_a_choice_is_never_blank() {
        sel.current = undefined
        verify(shows(sel, "select_choose", "Choose…"), "an unchosen box asks for a choice")
        verify(shows(none, "select_empty", "Nothing to choose"), "an empty box says so")
        verify(findText(own, "Pick a look") !== null, "a page can word it")
        sel.current = "b"
        verify(findText(sel, "b") !== null)
        verify(!shows(sel, "select_choose", "Choose…"))
    }

    // ---- buttons -------------------------------------------------------------------------------
    AppButton { id: quietDanger; y: 700; kind: "danger-quiet"; text: "Delete" }
    AppButton { id: loudDanger; x: 200; y: 700; kind: "danger"; text: "Delete" }
    ListIconButton { id: star; x: 400; y: 700; iconName: "star-filled"; text: "Remove from favourites"; tint: Theme.warn }

    function test_the_quiet_danger_button_is_red_text_on_the_plain_background() {
        compare(quietDanger._fg, Theme.bad)
        compare(quietDanger._bg, Theme.button)
        compare(loudDanger._bg, Theme.bad)
    }

    function test_an_icon_button_has_a_tooltip_and_a_tint() {
        compare(star.ToolTip.text, "Remove from favourites")
        verify(star.tint.a > 0)
        mouseMove(star, 10, 10)
        tryCompare(star, "hovered", true)
    }

    // ---- SliderRow -----------------------------------------------------------------------------
    SliderRow { id: degrees; y: 760; width: 500; label: "Limit"; from: 0; to: 100; value: 90; unit: " °C" }

    function test_a_unit_with_its_own_space_is_not_spaced_twice() {
        verify(findText(degrees, "90 °C") !== null, "one space between value and unit")
    }
}
