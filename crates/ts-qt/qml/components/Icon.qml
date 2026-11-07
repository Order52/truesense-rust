// A tintable icon. The icons are plain SVG files in qml/icons drawn with stroke="currentColor";
// Qt cannot tint an SVG Image, so this reads the file once (cached), swaps currentColor for the
// requested colour and hands the Image a data: URL. That works on every Qt Quick backend (no
// shader effect, so also the software and offscreen ones) and keeps the files standard SVG.
//
//   Icon { name: "fan"; size: 20; color: Theme.muted }
import QtQuick
import org.truesense.app

Item {
    id: root

    property string name: ""
    property int size: 20
    property color color: Theme.text

    implicitWidth: size
    implicitHeight: size


    readonly property string _source: {
        if (name === "") return ""
        var svg = IconCache.text(name, Qt.resolvedUrl("../icons/" + name + ".svg"))
        if (svg === "") return ""
        var c = root.color
        var hex = "#" + [c.r, c.g, c.b].map(function (v) {
            var h = Math.round(v * 255).toString(16)
            return h.length < 2 ? "0" + h : h
        }).join("")
        return "data:image/svg+xml;utf8," + encodeURIComponent(svg.replace(/currentColor/g, hex))
    }

    Image {
        anchors.fill: parent
        source: root._source
        sourceSize.width: Math.round(root.size * Screen.devicePixelRatio)
        sourceSize.height: Math.round(root.size * Screen.devicePixelRatio)
        fillMode: Image.PreserveAspectFit
        smooth: true
        opacity: root.enabled ? 1 : 0.5
    }

    Accessible.ignored: true
}
