// The keyboard as a picture: the 22 x 6 cell keyboard drawn as its real keys (the layout comes
// from the view-model), each key in the colour of its first cell. It is the live preview when
// `interactive` is false and the per-key editor's board when true.
//
// Positions are reported in layout units (one unit = a normal key), so the page asks the
// view-model what is under them. The picture never mirrors in right-to-left layouts: the keys are
// where they are on the real keyboard.
import QtQuick
import org.truesense.app

Item {
    id: root

    /// {keys, width, height} from LightingVm.
    property var layout: LightingVm.layout
    /// 128 colours "rrggbb"; anything shorter shows black keys.
    property var cells: []
    /// Label per key (only legible when the picture is big enough).
    property bool showLabels: false
    /// The board answers the pointer and the keyboard.
    property bool interactive: false
    /// Cells drawn with a selection ring.
    property var selection: []
    /// The cell that has the keyboard focus ring (arrow keys), -1 for none.
    property int focusCell: -1
    /// Dark or light label per cell (from the view-model, one entry per cell), for showLabels.
    property var inks: []
    /// An explanation for screen readers.
    property string description: ""

    /// The pointer went down at `point` (layout units); `button` is Qt.LeftButton or Qt.RightButton.
    signal pressedAt(var point, int button, bool extend)
    /// The pointer moved from `from` to `to` with a button down.
    signal dragged(var from, var to, bool extend)
    signal released()
    /// An arrow key (`dir`: left, right, up, down) or Space/Enter (`dir`: "activate").
    signal navigated(string dir, bool extend)

    readonly property real _unit: layout.width > 0 ? Math.min(width / layout.width, 70) : 1
    readonly property real _ox: (width - layout.width * _unit) / 2
    implicitHeight: layout.height * _unit + 2
    height: implicitHeight
    implicitWidth: 480

    LayoutMirroring.enabled: false
    LayoutMirroring.childrenInherit: true
    activeFocusOnTab: interactive
    Accessible.role: interactive ? Accessible.Graphic : Accessible.StaticText
    Accessible.name: description
    Accessible.ignored: description === ""

    function _selected(cell) { return selection.indexOf(cell) >= 0 }

    Repeater {
        model: root.layout.keys
        delegate: Rectangle {
            id: key
            required property var modelData
            readonly property int cell: modelData.cells[0]
            readonly property string hex: root.cells.length > cell ? root.cells[cell] : "000000"
            readonly property bool selected: root._selected(cell)
            readonly property bool focused: root.interactive && root.activeFocus && root.focusCell >= 0
                && modelData.cells.indexOf(root.focusCell) >= 0

            x: root._ox + modelData.x * root._unit + 1
            y: modelData.y * root._unit + 1
            width: modelData.w * root._unit - 2
            height: modelData.h * root._unit - 2
            radius: Math.max(2, root._unit * 0.12)
            // A dark key shows as a dim cap, not a hole, so the picture still reads as a keyboard.
            color: hex === "000000" ? Theme.tile : LightingVm.css(hex)
            border.width: selected || focused ? 2 : 1
            border.color: focused ? Theme.focusRing : selected ? Theme.accent : (hex === "000000" ? Theme.tileLine : Qt.rgba(0, 0, 0, 0.35))

            Text {
                anchors.centerIn: parent
                visible: root.showLabels && root._unit > 24 && modelData.label !== ""
                text: modelData.label
                font.pixelSize: Math.max(8, Math.min(root._unit * 0.34, 14))
                // Ink over a key colour is data-driven: dark on light keys and the other way round.
                color: key.hex === "000000" ? Theme.muted
                    : (root.inks.length > key.cell && root.inks[key.cell] ? Qt.rgba(0, 0, 0, 0.8) : Qt.rgba(1, 1, 1, 0.92))
            }
            // Selection also lifts the key so it does not rely on the ring colour alone.
            Rectangle {
                anchors.fill: parent
                radius: parent.radius
                visible: key.selected
                color: Qt.rgba(1, 1, 1, 0.18)
            }
        }
    }

    MouseArea {
        id: pointer
        anchors.fill: parent
        enabled: root.interactive
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        property var last: null
        function _at(m) { return { x: (m.x - root._ox) / root._unit, y: m.y / root._unit } }
        onPressed: (m) => {
            root.forceActiveFocus()
            last = _at(m)
            root.pressedAt(last, m.button, (m.modifiers & Qt.ControlModifier) !== 0)
        }
        onPositionChanged: (m) => {
            if (!pressed || last === null) return
            var p = _at(m)
            root.dragged(last, p, (m.modifiers & Qt.ControlModifier) !== 0)
            last = p
        }
        onReleased: { last = null; root.released() }
        onCanceled: { last = null; root.released() }
    }

    Keys.onPressed: (e) => {
        if (!interactive) return
        var extend = (e.modifiers & Qt.ControlModifier) !== 0
        var dir = ""
        if (e.key === Qt.Key_Left) dir = "left"
        else if (e.key === Qt.Key_Right) dir = "right"
        else if (e.key === Qt.Key_Up) dir = "up"
        else if (e.key === Qt.Key_Down) dir = "down"
        else if (e.key === Qt.Key_Space || e.key === Qt.Key_Return || e.key === Qt.Key_Enter) dir = "activate"
        if (dir === "") return
        e.accepted = true
        navigated(dir, extend)
    }
}
