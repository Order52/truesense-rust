// A small picture of the keyboard for a gallery card: the real keys, each in the colour of its
// first cell, drawn on one Canvas (a card with 100 key items each would make a gallery of thirty
// cards three thousand items). The picture never mirrors in right-to-left layouts.
//
//   KeyboardThumb { layout: LightingVm.layout; cells: preview.cells; description: "Hearth" }
//
// `layout` is {keys: [{cells: [..], x, y, w, h}], width, height} (what the Lighting view-model
// builds); `cells` are up to 128 "rrggbb" strings. A dark key is drawn as a dim cap, so the picture
// still reads as a keyboard. With no cells yet it shows the dim caps only.
import QtQuick
import org.truesense.app

Canvas {
    id: root

    property var layout: ({ keys: [], width: 20.5, height: 6.4 })
    property var cells: []
    /// For screen readers.
    property string description: ""
    /// The colour of a key that is dark.
    property color capColor: Theme.tile
    /// The thin edge of every key.
    property color edgeColor: Qt.rgba(0, 0, 0, 0.35)

    readonly property real unit: layout.width > 0 ? width / layout.width : 1
    /// How many keys the last paint drew (the tests look).
    property int painted: 0

    implicitWidth: 240
    implicitHeight: Math.round(implicitWidth * layout.height / Math.max(1, layout.width))
    renderTarget: Canvas.Image
    LayoutMirroring.enabled: false
    Accessible.role: Accessible.Graphic
    Accessible.name: description
    Accessible.ignored: description === ""

    onCellsChanged: requestPaint()
    onLayoutChanged: requestPaint()
    onWidthChanged: requestPaint()
    onHeightChanged: requestPaint()
    onCapColorChanged: requestPaint()

    onPaint: {
        var ctx = getContext("2d")
        ctx.reset()
        var keys = layout.keys || []
        var u = unit
        var gap = Math.max(0.6, u * 0.07)
        var radius = Math.max(1.5, u * 0.14)
        var cap = String(capColor)
        var edge = String(edgeColor)
        var n = 0
        ctx.lineWidth = 1
        for (var i = 0; i < keys.length; ++i) {
            var k = keys[i]
            var hex = cells.length > k.cells[0] ? cells[k.cells[0]] : "000000"
            var x = k.x * u + gap / 2
            var y = k.y * u + gap / 2
            var w = k.w * u - gap
            var h = k.h * u - gap
            ctx.beginPath()
            ctx.moveTo(x + radius, y)
            ctx.arcTo(x + w, y, x + w, y + h, radius)
            ctx.arcTo(x + w, y + h, x, y + h, radius)
            ctx.arcTo(x, y + h, x, y, radius)
            ctx.arcTo(x, y, x + w, y, radius)
            ctx.closePath()
            ctx.fillStyle = hex === "000000" ? cap : "#" + hex
            ctx.fill()
            ctx.strokeStyle = edge
            ctx.stroke()
            ++n
        }
        painted = n
    }
}
