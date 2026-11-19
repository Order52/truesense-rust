// The keyboard as a picture of the PH16-71's deck: the speaker grille with the mode button above,
// the recessed keyboard well, and its real keys (the layout comes from the view-model) as black
// caps whose legends and edges glow in the colour of the key's first cell, the way the backlight
// shines through the real keyboard. It is the live preview when `interactive` is false and the
// per-key editor's board when true.
//
// Scene-graph items only (Rectangles, Text and one small Shape for the PREDATOR key, whose emblem is
// Acer's own artwork from PredatorSense), redrawn only when the colours change: no Canvas, no
// running animation.
//
// Positions are reported in layout units (one unit = a normal key, origin at the first key), so the
// page asks the view-model what is under them. The picture never mirrors in right-to-left layouts:
// the keys are where they are on the real keyboard.
import QtQuick
import QtQuick.Shapes
import org.truesense.app

Item {
    id: root

    /// {keys, width, height} from LightingVm.
    property var layout: LightingVm.layout
    /// 128 colours "rrggbb"; anything shorter shows unlit keys.
    property var cells: []
    /// Legends on the caps (only drawn when the picture is big enough to read them).
    property bool showLabels: true
    /// The board answers the pointer and the keyboard.
    property bool interactive: false
    /// Cells drawn with a selection ring.
    property var selection: []
    /// The cell that has the keyboard focus ring (arrow keys), -1 for none.
    property int focusCell: -1
    /// Kept for the callers that pass it; the legends take their ink from the key's own light.
    property var inks: []
    /// An explanation for screen readers.
    property string description: ""
    /// The box being dragged out to select keys, {x, y, w, h} in layout units, or null.
    property var band: null

    /// The pointer went down at `point` (layout units); `button` is Qt.LeftButton or Qt.RightButton.
    signal pressedAt(var point, int button, bool extend)
    /// The pointer moved from `from` to `to` with a button down.
    signal dragged(var from, var to, bool extend)
    signal released()
    /// An arrow key (`dir`: left, right, up, down) or Space/Enter (`dir`: "activate").
    signal navigated(string dir, bool extend)

    // The deck around the keys, in key units: the sides, the grille band above, the edge below.
    readonly property real _side: 0.42
    readonly property real _top: 0.95
    readonly property real _bottom: 0.38
    readonly property real _unit: layout.width > 0 ? Math.min(width / (layout.width + 2 * _side), 64) : 1
    readonly property real _deckW: (layout.width + 2 * _side) * _unit
    readonly property real _ox: (width - layout.width * _unit) / 2
    readonly property real _oy: _top * _unit
    readonly property bool _legible: showLabels && _unit > 22

    implicitHeight: Math.ceil((layout.height + _top + _bottom) * _unit)
    height: implicitHeight
    implicitWidth: 480

    LayoutMirroring.enabled: false
    LayoutMirroring.childrenInherit: true
    activeFocusOnTab: interactive
    Accessible.role: interactive ? Accessible.Graphic : Accessible.StaticText
    Accessible.name: description
    Accessible.ignored: description === ""

    function _selected(cell) { return selection.indexOf(cell) >= 0 }

    // The aluminium of the deck (the same in light and dark themes: it is the laptop).
    Rectangle {
        id: deck
        x: (root.width - root._deckW) / 2
        width: root._deckW
        height: root.height
        radius: root._unit * 0.32
        gradient: Gradient {
            GradientStop { position: 0; color: "#1d2026" }
            GradientStop { position: 1; color: "#16181c" }
        }
        border.width: 1
        border.color: "#2b2f37"

        // The speaker grille across the top, with the mode (turbo) button at its left and the
        // DTS:X mark at its right, as on the PH16-71.
        Rectangle {
            id: grille
            x: root._unit * 2.1
            y: root._unit * 0.26
            width: parent.width - x - root._unit * 1.5
            height: root._unit * 0.34
            radius: height / 2
            color: "#101216"
            border.width: 1
            border.color: "#262a31"
            Column {
                anchors.centerIn: parent
                spacing: Math.max(1, root._unit * 0.06)
                Repeater {
                    model: 3
                    delegate: Rectangle {
                        width: grille.width - root._unit * 0.4
                        height: 1
                        color: "#22262d"
                    }
                }
            }
        }
        Rectangle {
            // The mode button, lit blue.
            x: root._unit * 0.95
            y: grille.y + grille.height / 2 - height / 2
            width: root._unit * 0.75
            height: root._unit * 0.2
            radius: height / 2
            color: "#0d1a24"
            border.width: 1
            border.color: "#2f8fd8"
            Rectangle {
                anchors.centerIn: parent
                width: parent.width - 4
                height: Math.max(1, parent.height * 0.35)
                radius: height / 2
                color: "#3aa8ff"
                opacity: 0.85
            }
        }
        Text {
            anchors.right: parent.right
            anchors.rightMargin: root._unit * 0.45
            y: grille.y + grille.height / 2 - height / 2
            visible: root._unit > 18
            text: "dts:x"
            color: "#5a606b"
            font.pixelSize: Math.max(7, root._unit * 0.2)
            font.bold: true
        }

        // The recessed well the keys sit in.
        Rectangle {
            x: root._ox - deck.x - root._unit * 0.16
            y: root._oy - root._unit * 0.16
            width: root.layout.width * root._unit + root._unit * 0.32
            height: root.layout.height * root._unit + root._unit * 0.32
            radius: root._unit * 0.16
            color: "#0c0d10"
            border.width: 1
            border.color: "#05060a"
            // The well's lit lower lip.
            Rectangle {
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.bottom: parent.bottom
                anchors.margins: 2
                height: 1
                color: "#2c3038"
            }
        }
    }

    Repeater {
        model: root.layout.keys
        delegate: Item {
            id: key
            required property var modelData
            readonly property int cell: modelData.cells[0]
            readonly property string hex: root.cells.length > cell ? root.cells[cell] : "000000"
            readonly property bool dark: hex === "000000"
            readonly property color light: dark ? "#000000" : LightingVm.css(hex)
            /// How strongly the key glows: a dim colour glows less.
            readonly property real level: dark ? 0 : Math.max(light.r, light.g, light.b)
            readonly property color ink: dark ? "#4a4f59"
                : Qt.rgba(light.r + (1 - light.r) * 0.3, light.g + (1 - light.g) * 0.3, light.b + (1 - light.b) * 0.3, 0.6 + 0.4 * level)
            readonly property color capColour: dark ? "#16181c"
                : Qt.rgba(0.075 + light.r * 0.16, 0.08 + light.g * 0.16, 0.09 + light.b * 0.16, 1)
            readonly property bool selected: root._selected(cell)
            readonly property bool focused: root.interactive && root.activeFocus && root.focusCell >= 0
                && modelData.cells.indexOf(root.focusCell) >= 0
            readonly property real u: root._unit
            readonly property real gap: Math.max(1.5, u * 0.09)

            x: root._ox + modelData.x * u
            y: root._oy + modelData.y * u
            width: modelData.w * u
            height: modelData.h * u

            // The light around the cap's edges (the backlight leaking between the keys).
            Rectangle {
                visible: !key.dark
                x: key.gap / 2 - key.u * 0.05
                y: key.gap / 2 - key.u * 0.04
                width: parent.width - key.gap + key.u * 0.1
                height: parent.height - key.gap + key.u * 0.1
                radius: key.u * 0.2
                color: key.light
                opacity: 0.18 + 0.4 * key.level
            }

            // The cap: black plastic with a lighter top edge.
            Rectangle {
                id: cap
                x: key.gap / 2
                y: key.gap / 2
                width: parent.width - key.gap
                height: parent.height - key.gap
                radius: Math.max(2, key.u * 0.11)
                color: key.capColour
                Rectangle {
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.top: parent.top
                    anchors.leftMargin: parent.radius
                    anchors.rightMargin: parent.radius
                    height: 1
                    color: Qt.rgba(1, 1, 1, 0.09)
                }
            }

            // ---- legends ----
            readonly property bool twoLine: modelData.shift !== "" || modelData.sub !== ""
            // The main legend's size: as big as a letter key's, smaller on the low function row and
            // on twin-legend keys, and never wider than the cap.
            readonly property real fontMain: Math.max(6, Math.min(
                u * (media ? 0.24 : twoLine ? 0.25 : modelData.h < 0.7 ? 0.25 : 0.3),
                (legendW - 2 * gap) * 0.85 / (Math.max(1, modelData.label.length) * 0.6), 15))
            readonly property real fontSmall: Math.max(6, Math.min(u * 0.22, 12))
            // Where the legends go: the whole cap.
            readonly property real legendX: 0
            readonly property real legendW: width
            readonly property real legendY: 0
            readonly property real legendH: height
            /// The media keys print small symbols.
            readonly property bool media: ["backward", "playpause", "forward"].indexOf(modelData.name) >= 0

            Text {
                visible: root._legible && key.modelData.shift !== ""
                x: key.legendX
                width: key.legendW
                y: key.legendY + key.legendH * 0.14
                horizontalAlignment: Text.AlignHCenter
                text: key.modelData.shift
                color: key.ink
                font.pixelSize: key.fontSmall
            }
            Text {
                visible: root._legible && key.modelData.label !== ""
                x: key.legendX + key.gap
                width: key.legendW - 2 * key.gap
                y: key.legendY + (key.modelData.shift !== "" ? key.legendH * 0.46
                    : key.modelData.sub !== "" ? key.legendH * 0.14
                    : (key.legendH - height) / 2)
                horizontalAlignment: Text.AlignHCenter
                text: key.modelData.label
                color: key.ink
                font.pixelSize: key.fontMain
                font.weight: Font.DemiBold
            }
            Text {
                visible: root._legible && key.modelData.sub !== ""
                x: key.legendX
                width: key.legendW
                y: key.legendY + key.legendH * 0.56
                horizontalAlignment: Text.AlignHCenter
                text: key.modelData.sub
                color: key.ink
                font.pixelSize: key.fontSmall - 1
            }
            // The space bar's printed line.
            Rectangle {
                visible: key.modelData.name === "space"
                anchors.horizontalCenter: parent.horizontalCenter
                y: parent.height * 0.68
                width: parent.width * 0.42
                height: Math.max(1, key.u * 0.035)
                radius: height / 2
                color: key.ink
            }
            // The Windows key's four panes.
            Grid {
                visible: key.modelData.icon === "windows"
                anchors.centerIn: parent
                columns: 2
                spacing: Math.max(1, key.u * 0.03)
                Repeater {
                    model: 4
                    delegate: Rectangle {
                        width: Math.max(2, key.u * 0.1)
                        height: width
                        color: key.ink
                    }
                }
            }
            Loader {
                active: key.modelData.icon === "menu" || key.modelData.name === "power"
                anchors.centerIn: parent
                sourceComponent: Icon {
                    name: key.modelData.icon === "menu" ? "menu" : "power"
                    size: Math.max(8, Math.round(key.u * 0.3))
                    color: key.ink
                }
            }
            // The PREDATOR key: Acer's emblem, lit like a legend.
            Loader {
                active: key.modelData.icon === "predator"
                anchors.centerIn: parent
                sourceComponent: PredatorEmblem { height: key.u * 0.46; color: key.ink }
            }

            // Selection lifts the key; focus and selection draw a ring.
            Rectangle {
                anchors.fill: cap
                radius: cap.radius
                visible: key.selected
                color: Qt.rgba(1, 1, 1, 0.16)
            }
            Rectangle {
                anchors.fill: cap
                anchors.margins: -1
                radius: cap.radius + 1
                visible: key.selected || key.focused
                color: "transparent"
                border.width: 2
                border.color: key.focused ? Theme.focusRing : Theme.accent
            }
        }
    }

    // The box a selection drag draws.
    Rectangle {
        visible: root.band !== null
        x: root.band === null ? 0 : root._ox + root.band.x * root._unit
        y: root.band === null ? 0 : root._oy + root.band.y * root._unit
        width: root.band === null ? 0 : root.band.w * root._unit
        height: root.band === null ? 0 : root.band.h * root._unit
        color: Qt.rgba(Theme.accent.r, Theme.accent.g, Theme.accent.b, 0.14)
        border.width: 1
        border.color: Theme.accent
        radius: 3
    }

    MouseArea {
        id: pointer
        anchors.fill: parent
        enabled: root.interactive
        // A drag on the board paints or selects keys; the page must not take it to scroll.
        preventStealing: true
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        property var last: null
        function _at(m) { return { x: (m.x - root._ox) / root._unit, y: (m.y - root._oy) / root._unit } }
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
