// A picture of the back of the laptop with its light bar lit as set: the glass strip with its seven
// circuit-shaped light panels and HELIOS in the middle, the glowing edge line under it, and the
// vents and ports below (drawn after the PH16-71's rear, not a photo).
//
//   RearBarPicture { effect: "static"; zones: ["ff0000", "00ff00", "0000ff"]; brightness: 80 }
//
// It is scene-graph geometry (QtQuick.Shapes and Rectangles in a 1000 x 200 drawing scaled to the
// width), redrawn only when a setting changes: no Canvas and no running animation. A moving effect
// is shown as one still frame of it (breathing at its peak, neon as its colour ramp, wave as a fade
// toward the way it travels, twinkling as scattered panels).
//
// In the static effect a click on a panel asks for that part's colour (`partClicked(0..2)`).
import QtQuick
import QtQuick.Shapes
import org.truesense.app

Item {
    id: root

    /// The rear effect id: static, breathing, neon, wave, twinkling.
    property string effect: "static"
    /// "rrggbb" of the effects with one colour.
    property string colour: "00aec7"
    /// Three "rrggbb": left, middle and right part (static).
    property var zones: [colour, colour, colour]
    /// 0..100; 0 (or `on` false) shows the bar unlit.
    property int brightness: 100
    property bool on: true
    /// 0 left to right, 1 right to left (wave).
    property int direction: 1
    /// Whether a click on a part asks for its colour.
    property bool partsClickable: false

    signal partClicked(int part)

    /// Which part (left, middle, right) each of the seven panels belongs to.
    readonly property var partOf: [0, 0, 1, 1, 1, 2, 2]
    readonly property real level: on && brightness > 0 ? 0.4 + 0.6 * Math.min(100, brightness) / 100 : 0
    /// [{c: color, k: 0..1}] for the seven panels, left to right.
    readonly property var lights: {
        var out = []
        for (var i = 0; i < 7; ++i) {
            var t = i / 6
            var hex = colour
            var k = level
            if (effect === "static") {
                var z = zones && zones.length === 3 ? zones[partOf[i]] : colour
                hex = z || colour
            } else if (effect === "neon") {
                out.push({ c: Qt.hsva(0.83 * t, 0.85, 1, 1), k: k })
                continue
            } else if (effect === "wave") {
                var head = direction === 0 ? t : 1 - t
                k = level * (0.3 + 0.7 * head)
            } else if (effect === "twinkling") {
                k = level * [1, 0.35, 0.85, 0.5, 1, 0.3, 0.8][i]
            }
            out.push({ c: Qt.color("#" + hex), k: k })
        }
        return out
    }

    readonly property color etch: "#2a323a"
    readonly property color etchFaint: "#222930"

    function lit(i, alpha) {
        var l = lights[i]
        if (l.k <= 0) return alpha >= 1 ? etch : "transparent"
        return Qt.rgba(l.c.r, l.c.g, l.c.b, alpha * l.k)
    }
    /// The colour of a panel's light, a little whiter at its core (as a lit LED reads).
    function core(i) {
        var l = lights[i]
        if (l.k <= 0) return etch
        var w = 0.18
        return Qt.rgba(l.c.r + (1 - l.c.r) * w, l.c.g + (1 - l.c.g) * w, l.c.b + (1 - l.c.b) * w, 0.35 + 0.65 * l.k)
    }

    // ---- the drawing (design units, 1000 x 200) --------------------------------------------------

    function _p(x, y, m) { return Qt.point(m ? 1000 - x : x, y) }
    function _poly(list, m) { return list.map(function (q) { return _p(q[0], q[1], m) }) }
    function _closed(poly) { return poly.length ? poly.concat([poly[0]]) : poly }

    /// One of the long panels: a stepped light ribbon, a thin trace above it and a short dash.
    function _long(x0, w, m, ticks) {
        var ribbon = _poly([[x0 + .04 * w, 85], [x0 + .50 * w, 85], [x0 + .60 * w, 73], [x0 + .97 * w, 73],
                            [x0 + .97 * w, 83], [x0 + .645 * w, 83], [x0 + .545 * w, 96], [x0 + .04 * w, 96]], m)
        var lines = [_poly([[x0 + .10 * w, 66], [x0 + .58 * w, 66], [x0 + .645 * w, 58], [x0 + .92 * w, 58]], m)]
        if (ticks)
            for (var j = 0; j < 6; ++j)
                lines.push(_poly([[x0 + .08 * w + j * 5, 101], [x0 + .08 * w + j * 5, 108]], m))
        return { fills: [ribbon], bars: [_poly([[x0 + .70 * w, 67], [x0 + .87 * w, 67]], m)], lines: lines }
    }
    function _end(m) {
        return {
            fills: [_poly([[22, 80], [66, 80], [74, 88], [74, 93], [22, 93]], m)],
            bars: [],
            lines: [_poly([[14, 59], [44, 59]], m), _poly([[14, 66], [56, 66]], m), _poly([[14, 73], [36, 73]], m)]
        }
    }
    readonly property var panels: [
        _end(false),
        _long(94, 168, false, false),
        _long(276, 116, false, true),
        {
            fills: [_poly([[493, 111], [507, 111], [500, 103]], false)],
            bars: [_poly([[408, 52], [432, 96], [568, 96], [592, 52]], false)],
            lines: [_poly([[446, 89], [554, 89]], false)]
        },
        _long(276, 116, true, true),
        _long(94, 168, true, false),
        _end(true)
    ]
    /// Where a click picks each panel's part.
    readonly property var hitBoxes: [[8, 86], [86, 268], [268, 400], [400, 600], [600, 732], [732, 914], [914, 992]]

    implicitWidth: 640
    implicitHeight: Math.round(width * 0.2)
    LayoutMirroring.enabled: false
    Accessible.role: Accessible.Graphic

    Item {
        id: drawing
        width: 1000
        height: 200
        transform: Scale { xScale: root.width / 1000; yScale: root.height / 200 }

        // The lid's lower edge and its hinges.
        Rectangle {
            x: 24; y: 0; width: 952; height: 46
            gradient: Gradient {
                GradientStop { position: 0; color: "#0d0f12" }
                GradientStop { position: 1; color: "#181b20" }
            }
        }
        Repeater {
            model: [128, 812]
            delegate: Rectangle {
                required property int modelData
                x: modelData; y: 30; width: 60; height: 20; radius: 2
                color: "#0a0b0d"
                border.width: 1
                border.color: "#23272d"
            }
        }

        // The chassis under the glass, with the vents and the ports.
        Shape {
            anchors.fill: parent
            ShapePath {
                strokeColor: "#30353c"
                strokeWidth: 1
                fillColor: "#1d2024"
                PathPolyline { path: [Qt.point(0, 116), Qt.point(1000, 116), Qt.point(994, 192), Qt.point(6, 192), Qt.point(0, 116)] }
            }
        }
        Repeater {
            model: [{ x: 46, part: 0 }, { x: 674, part: 6 }]
            delegate: Rectangle {
                id: vent
                required property var modelData
                x: modelData.x; y: 134; width: 280; height: 40; radius: 3
                color: "#08090b"
                border.width: 1
                border.color: "#2b3036"
                clip: true
                // Light from the bar leaks into the vent.
                Rectangle {
                    anchors.fill: parent
                    gradient: Gradient {
                        GradientStop { position: 0; color: root.lit(vent.modelData.part, 0.28) }
                        GradientStop { position: 1; color: "transparent" }
                    }
                }
                Shape {
                    anchors.fill: parent
                    ShapePath {
                        strokeColor: "#2a2f35"
                        strokeWidth: 5
                        fillColor: "transparent"
                        PathMultiline {
                            paths: {
                                var out = []
                                for (var fx = 20; fx < 280; fx += 46)
                                    out.push([Qt.point(fx, 42), Qt.point(fx + 26, -2)])
                                return out
                            }
                        }
                    }
                }
            }
        }
        Rectangle {
            x: 356; y: 128; width: 288; height: 52; radius: 4
            color: "#131518"
            border.width: 1
            border.color: "#2b3036"
            Shape {
                anchors.fill: parent
                ShapePath {
                    strokeColor: "#5b636d"
                    strokeWidth: 1.6
                    fillColor: "#0a0b0d"
                    // DC in, HDMI, two USB-C
                    PathMultiline {
                        paths: [
                            [Qt.point(104, 18), Qt.point(150, 18), Qt.point(150, 29), Qt.point(144, 34), Qt.point(110, 34), Qt.point(104, 29), Qt.point(104, 18)],
                            [Qt.point(196, 22), Qt.point(218, 22), Qt.point(222, 26), Qt.point(218, 30), Qt.point(196, 30), Qt.point(192, 26), Qt.point(196, 22)],
                            [Qt.point(242, 22), Qt.point(264, 22), Qt.point(268, 26), Qt.point(264, 30), Qt.point(242, 30), Qt.point(238, 26), Qt.point(242, 22)]
                        ]
                    }
                }
            }
            Rectangle { x: 38; y: 19; width: 14; height: 14; radius: 7; color: "#0a0b0d"; border.width: 1.6; border.color: "#5b636d" }
        }

        // The glass strip.
        Shape {
            anchors.fill: parent
            ShapePath {
                strokeColor: "#3d4853"
                strokeWidth: 1.2
                fillColor: "#161b20"
                PathPolyline { path: [Qt.point(8, 44), Qt.point(992, 44), Qt.point(1000, 52), Qt.point(1000, 116), Qt.point(0, 116), Qt.point(0, 52), Qt.point(8, 44)] }
            }
            // The unlit frame of the panels, etched into the glass.
            ShapePath {
                strokeColor: root.etchFaint
                strokeWidth: 1.4
                fillColor: "transparent"
                PathMultiline {
                    paths: {
                        var out = [[Qt.point(14, 50), Qt.point(986, 50)],
                                   [Qt.point(14, 106), Qt.point(392, 106)], [Qt.point(608, 106), Qt.point(986, 106)]]
                        var cuts = [86, 268, 732, 914]
                        for (var i = 0; i < cuts.length; ++i)
                            out.push([Qt.point(cuts[i] + 4, 50), Qt.point(cuts[i] - 4, 106)])
                        out.push([Qt.point(398, 50), Qt.point(424, 106)], [Qt.point(602, 50), Qt.point(576, 106)])
                        out.push([Qt.point(442, 57), Qt.point(558, 57), Qt.point(554, 64), Qt.point(446, 64), Qt.point(442, 57)])
                        return out
                    }
                }
            }
        }
        // A sheen across the top of the glass.
        Rectangle {
            x: 8; y: 45; width: 984; height: 14
            gradient: Gradient {
                GradientStop { position: 0; color: Qt.rgba(1, 1, 1, 0.05) }
                GradientStop { position: 1; color: "transparent" }
            }
        }

        // The seven light panels.
        Repeater {
            model: root.panels
            delegate: Shape {
                id: panel
                required property var modelData
                required property int index
                anchors.fill: parent
                // glow around the ribbons and bars: a wide faint halo, then a narrower brighter one
                ShapePath {
                    strokeColor: root.lit(panel.index, 0.07)
                    strokeWidth: 16
                    fillColor: "transparent"
                    joinStyle: ShapePath.RoundJoin
                    capStyle: ShapePath.RoundCap
                    PathMultiline { paths: panel.modelData.fills.map(root._closed).concat(panel.modelData.bars) }
                }
                ShapePath {
                    strokeColor: root.lit(panel.index, 0.16)
                    strokeWidth: 7
                    fillColor: "transparent"
                    joinStyle: ShapePath.RoundJoin
                    capStyle: ShapePath.RoundCap
                    PathMultiline { paths: panel.modelData.fills.map(root._closed).concat(panel.modelData.bars) }
                }
                ShapePath {
                    strokeColor: root.lit(panel.index, 0.12)
                    strokeWidth: 5
                    fillColor: "transparent"
                    joinStyle: ShapePath.RoundJoin
                    capStyle: ShapePath.RoundCap
                    PathMultiline { paths: panel.modelData.lines }
                }
                ShapePath {
                    strokeColor: "transparent"
                    fillColor: root.core(panel.index)
                    PathMultiline { paths: panel.modelData.fills }
                }
                ShapePath {
                    strokeColor: root.core(panel.index)
                    strokeWidth: 4.5
                    fillColor: "transparent"
                    joinStyle: ShapePath.MiterJoin
                    capStyle: ShapePath.FlatCap
                    PathMultiline { paths: panel.modelData.bars }
                }
                ShapePath {
                    strokeColor: root.core(panel.index)
                    strokeWidth: 1.8
                    fillColor: "transparent"
                    capStyle: ShapePath.FlatCap
                    PathMultiline { paths: panel.modelData.lines }
                }
            }
        }
        Text {
            x: 440; y: 66; width: 120
            horizontalAlignment: Text.AlignHCenter
            text: "HELIOS"
            font.pixelSize: 13
            font.letterSpacing: 3
            color: root.lights[3].k > 0 ? Qt.rgba(1, 1, 1, 0.25 + 0.45 * root.lights[3].k) : "#3a434c"
        }

        // The light line along the glass edge, and its glow on the chassis.
        Rectangle {
            x: 6; y: 110; width: 988; height: 12
            opacity: root.level > 0 ? 0.5 : 0
            gradient: Gradient {
                orientation: Gradient.Horizontal
                GradientStop { position: 0.0; color: root.lit(0, 0.45) }
                GradientStop { position: 0.17; color: root.lit(1, 0.45) }
                GradientStop { position: 0.33; color: root.lit(2, 0.45) }
                GradientStop { position: 0.5; color: root.lit(3, 0.45) }
                GradientStop { position: 0.67; color: root.lit(4, 0.45) }
                GradientStop { position: 0.83; color: root.lit(5, 0.45) }
                GradientStop { position: 1.0; color: root.lit(6, 0.45) }
            }
        }
        Rectangle {
            x: 4; y: 114; width: 992; height: 2.5
            gradient: Gradient {
                orientation: Gradient.Horizontal
                GradientStop { position: 0.0; color: root.core(0) }
                GradientStop { position: 0.17; color: root.core(1) }
                GradientStop { position: 0.33; color: root.core(2) }
                GradientStop { position: 0.5; color: root.core(3) }
                GradientStop { position: 0.67; color: root.core(4) }
                GradientStop { position: 0.83; color: root.core(5) }
                GradientStop { position: 1.0; color: root.core(6) }
            }
        }

        // Clicks on the parts (static).
        Repeater {
            model: root.partsClickable ? root.hitBoxes : []
            delegate: MouseArea {
                required property var modelData
                required property int index
                x: modelData[0]; y: 44; width: modelData[1] - modelData[0]; height: 72
                cursorShape: Qt.PointingHandCursor
                onClicked: root.partClicked(root.partOf[index])
            }
        }
    }
}
