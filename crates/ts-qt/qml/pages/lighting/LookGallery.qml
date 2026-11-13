// The saved looks as cards with a picture of the keyboard each: the colours of a per-key look, the
// theme's own frame for a look that carries a theme, and the view-model's approximation of a moving
// effect. Each card has its own actions: Apply (also a click on the picture), Copy, and for the
// person's own looks "Update with the lights now" and Delete. The page does the commands.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

GalleryGrid {
    id: gallery

    /// The names of the looks, built-in first.
    property var names: []
    /// The look applied last from here (outlined), or "".
    property string selected: ""
    signal apply(string name)
    signal copy(string name)
    signal update(string name)
    signal remove(string name)

    // What each card shows, by look name: {cells, summary}.
    property var shown: ({})
    property var asked: ({})

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
            readonly property bool builtin: modelData === "native"

            Layout.preferredWidth: gallery.cardWidth
            Layout.fillHeight: true
            title: LightingVm.lookName(modelData)
            subtitle: info ? info.summary : ""
            layout: LightingVm.layout
            cells: info ? info.cells : []
            highlighted: gallery.selected === modelData
            //% "Built in"
            badge: builtin ? qsTrId("light_looks_builtin_badge") : ""
            badgeKind: "neutral"
            onActivated: gallery.apply(modelData)
            Component.onCompleted: gallery.ensure(modelData)

            RowLayout {
                Layout.fillWidth: true
                spacing: 4
                SecondaryButton {
                    //% "Apply"
                    text: qsTrId("light_looks_apply")
                    iconName: "play"
                    onClicked: gallery.apply(card.modelData)
                }
                Item { Layout.fillWidth: true }
                ListIconButton {
                    iconName: "copy"
                    //% "Copy to make your own"
                    text: qsTrId("light_looks_copy2")
                    onClicked: gallery.copy(card.modelData)
                }
                ListIconButton {
                    visible: !card.builtin
                    iconName: "save"
                    //% "Update with the lights as they are now"
                    text: qsTrId("light_looks_update")
                    onClicked: gallery.update(card.modelData)
                }
                ListIconButton {
                    visible: !card.builtin
                    iconName: "trash"
                    tone: "danger"
                    //% "Delete"
                    text: qsTrId("light_looks_delete")
                    onClicked: gallery.remove(card.modelData)
                }
            }
        }
    }
}
