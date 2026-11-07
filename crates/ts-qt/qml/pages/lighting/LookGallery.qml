// The saved looks as cards with a picture of the keyboard each: the colours of a per-key look, the
// theme's own frame for a look that carries a theme, and the view-model's approximation of a moving
// effect (marked, since a still frame is not the effect). The selected look is the one the Looks tab
// below acts on; Apply puts it on at once.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

GridLayout {
    id: gallery

    /// The names of the looks, built-in first.
    property var names: []
    property string selected: ""
    signal picked(string name)
    signal apply(string name)

    // What each card shows, by look name: {cells, summary}.
    property var shown: ({})
    property var asked: ({})

    columns: Math.max(1, Math.floor((width + columnSpacing) / (270 + columnSpacing)))
    columnSpacing: 10
    rowSpacing: 10

    function _set(name, value) {
        var o = {}
        for (var k in shown) o[k] = shown[k]
        o[name] = value
        shown = o
    }

    /// Reads a look and works out its picture; asked once per name until `forget()`.
    function ensure(name) {
        if (asked[name] === true) return
        var a = {}
        for (var k in asked) a[k] = asked[k]
        a[name] = true
        asked = a
        Api.call("look", "show", { name: name }, function (look) {
            var summary = LightingVm.describeLook(look)
            var kb = look.keyboard
            if (look.theme && look.theme.name) {
                var t = look.theme
                Api.call("theme", "preview", {
                    name: t.name, colourway: t.colourway, custom: t.custom, params: t.params }, function (r) {
                    gallery._set(name, { cells: r.cells, summary: summary, exact: true })
                }, function () { gallery._set(name, { cells: [], summary: summary, exact: false }) })
                return
            }
            if (kb) {
                var p = LightingVm.call("preview", { look: kb, cells: kb.cells })
                gallery._set(name, { cells: p ? p.cells : [], summary: summary, exact: p ? p.exact : false })
            } else {
                gallery._set(name, { cells: [], summary: summary, exact: true })
            }
        }, function () { gallery._set(name, { cells: [], summary: "", exact: true }) })
    }

    function forget() {
        shown = ({})
        asked = ({})
    }

    Repeater {
        model: gallery.names
        delegate: GalleryCard {
            id: card
            required property string modelData
            readonly property var info: gallery.shown[modelData]

            Layout.fillWidth: true
            Layout.preferredWidth: 1
            Layout.maximumWidth: 420
            Layout.alignment: Qt.AlignTop
            title: modelData
            subtitle: info ? info.summary : ""
            layout: LightingVm.layout
            cells: info ? info.cells : []
            selected: gallery.selected === modelData
            onActivated: gallery.picked(modelData)
            Component.onCompleted: gallery.ensure(modelData)

            SecondaryButton {
                //% "Apply"
                text: qsTrId("light_looks_apply")
                onClicked: gallery.apply(card.modelData)
            }
        }
    }
}
