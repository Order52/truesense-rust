// The grid of a gallery: as many columns as cards of at least `minCardWidth` fit, every card the
// same width (`cardWidth`, also in a row that is not full, so a gallery of one card does not
// stretch it over the whole page) and every card of a row as tall as the tallest, so that their
// buttons line up.
//
//   GalleryGrid {
//       id: grid
//       Repeater {
//           model: names
//           delegate: GalleryCard { Layout.preferredWidth: grid.cardWidth; Layout.fillHeight: true }
//       }
//   }
//
// A detail row (an editor that opens under the row of the chosen card) is placed with `rowOf()`:
// give the cards `Layout.row: grid.rowOf(index)` and `Layout.column: grid.columnOf(index)`, and the
// detail `Layout.row: grid.detailRow`, `Layout.columnSpan: grid.columns`; `detailAfter` is the
// index of the card it belongs to (-1: no detail row).
import QtQuick
import QtQuick.Layouts
import org.truesense.app

GridLayout {
    id: grid

    /// The narrowest a card may be.
    property int minCardWidth: 260
    /// The index of the card the detail row belongs to, -1 for none.
    property int detailAfter: -1

    readonly property int cardWidth: Math.max(0, Math.floor((width - (columns - 1) * columnSpacing) / columns))
    /// The grid row of the detail: the one under the row of the card `detailAfter`.
    readonly property int detailRow: detailAfter < 0 ? -1 : Math.floor(detailAfter / columns) + 1

    function rowOf(index) {
        var r = Math.floor(index / columns)
        return detailAfter >= 0 && r >= detailRow ? r + 1 : r
    }
    function columnOf(index) { return index % columns }

    Layout.fillWidth: true
    // A layout takes the sum of its children's widths as its own largest width; a grid that once had
    // one column would then never be given the room for a second one. It may always grow.
    Layout.maximumWidth: 100000
    // And the other way round: the cards' widths must not hold the grid wide when the window narrows.
    Layout.minimumWidth: 0
    columns: Math.max(1, Math.floor((width + columnSpacing) / (minCardWidth + columnSpacing)))
    columnSpacing: 10
    rowSpacing: 10
}
