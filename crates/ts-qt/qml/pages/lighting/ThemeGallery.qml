// The theme gallery: a search box, filter chips, and one card per theme with a picture of the
// keyboard drawn from one frame of the theme (the Rust side draws it: `theme preview`), its
// colourway chips (they redraw the picture), a star, and one-click Apply and Stop. The card of the
// running theme is marked. Which cards show is decided in Rust (the gallery view-model); this file
// only asks and lays them out.
//
// The page owns what is chosen and what runs; this file reports the person's choices as signals.
// The page's `detail` (the editor of the chosen theme) opens right under the row of the chosen
// card, so a theme is changed where it was picked, not below the whole gallery.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

ColumnLayout {
    id: gallery

    /// `theme list` entries: {id, name, mood, builtin, top, runnable, suits, colourways}.
    property var themes: []
    /// The ids of the starred themes.
    property var starred: []
    /// The id of the running theme, "" for none.
    property string active: ""
    /// The id of the theme the detail below shows.
    property string selected: ""
    property string query: ""
    property string chip: "all"
    /// What opens under the row of the selected card (the page's editor); null for nothing.
    property Component detail: null
    /// The item the detail became (for the page to scroll to), or null.
    readonly property var detailItem: detailLoader.item

    signal picked(string id)
    signal apply(string id, int colourway)
    signal stop()
    signal starToggled(string id)

    // What each card shows: the chosen colourway of each theme. The pictures are LightingVm's
    // (kept between visits, updated once per batch of answers).
    property var ways: ({})

    // Which cards show, in which order. The Repeater's model is only replaced when that list really
    // changes: a new array with the same ids (every reload after Apply, a star, a stop) would destroy
    // and rebuild all 32 cards, the grid would collapse for a frame and the page would lose its
    // scroll position. The running theme is not moved to the front either (it has the bar above).
    readonly property var computedShown: LightingVm.gallery("filter", {
        themes: themes, query: query, chip: chip, starred: starred, active: active,
        pin_active: false }) || []
    property var shown: []
    property string _listKey: ""
    onComputedShownChanged: {
        var k = JSON.stringify(computedShown)
        if (k === _listKey) return
        _listKey = k
        shown = computedShown
    }
    /// How many times the cards were rebuilt (a test reads it: Apply must not rebuild them).
    property int rebuilds: 0
    onShownChanged: rebuilds++
    Component.onCompleted: { _listKey = JSON.stringify(computedShown); shown = computedShown }

    /// The card of theme `id`, or null when it is not shown.
    function cardFor(id) {
        for (var i = 0; i < cards.count; ++i) {
            var c = cards.itemAt(i)
            if (c && c.modelData === id) return c
        }
        return null
    }

    readonly property var chips: LightingVm.gallery("chips", { themes: themes, query: query, starred: starred }) || []
    readonly property var byId: {
        var o = {}
        for (var i = 0; i < themes.length; ++i) o[themes[i].id] = themes[i]
        return o
    }

    spacing: 10

    function wayOf(id) { return ways[id] === undefined ? 0 : ways[id] }

    function setWay(id, index) {
        var o = {}
        for (var k in ways) o[k] = ways[k]
        o[id] = index
        ways = o
    }

    /// Asks Rust for the picture of a theme in a colourway, once per run.
    function ensure(id, way) { LightingVm.ensurePreview(id, way) }

    function isStarred(id) { return starred.indexOf(id) >= 0 }

    RowLayout {
        Layout.fillWidth: true
        spacing: 8
        AppTextField {
            id: search
            Layout.fillWidth: true
            //% "Search themes, moods and uses"
            placeholderText: qsTrId("light_gal_search")
            Accessible.name: placeholderText
            text: gallery.query
            onTextEdited: gallery.query = text
            Keys.onEscapePressed: { text = ""; gallery.query = "" }
        }
    }
    FilterChips {
        Layout.fillWidth: true
        current: gallery.chip
        model: gallery.chips.map(function (c) { return { id: c.id, label: gallery.chipName(c.id), count: c.count } })
        onPicked: (id) => gallery.chip = id
    }

    function chipName(id) {
        switch (id) {
        //% "All"
        case "all": return qsTrId("light_gal_chip_all")
        //% "Top picks"
        case "top": return qsTrId("light_gal_chip_top")
        //% "Starred"
        case "starred": return qsTrId("light_gal_chip_starred")
        //% "Gaming"
        case "gaming": return qsTrId("light_gal_chip_gaming")
        //% "Work and typing"
        case "work": return qsTrId("light_gal_chip_work")
        //% "Night"
        case "night": return qsTrId("light_gal_chip_night")
        //% "Music"
        case "music": return qsTrId("light_gal_chip_music")
        //% "Calm"
        case "calm": return qsTrId("light_gal_chip_calm")
        //% "Yours"
        case "yours": return qsTrId("light_gal_chip_yours")
        default: return String(id)
        }
    }

    Text {
        Layout.fillWidth: true
        visible: gallery.shown.length > 0
        //% "Apply puts a theme on the keyboard. Click a picture to change its colours and settings, and to keep your version."
        text: qsTrId("light_gal_hint2")
        color: Theme.muted
        font.pixelSize: Theme.fontSmall + 1
        wrapMode: Text.Wrap
    }

    // Nothing to show: say why, and what to do, for the filter that is chosen.
    ColumnLayout {
        Layout.fillWidth: true
        Layout.topMargin: 8
        Layout.bottomMargin: 8
        visible: gallery.shown.length === 0
        spacing: 6
        Icon {
            Layout.alignment: Qt.AlignHCenter
            name: gallery.chip === "starred" ? "star" : (gallery.chip === "yours" ? "palette" : "search")
            size: 28
            color: Theme.muted
        }
        Text {
            Layout.fillWidth: true
            horizontalAlignment: Text.AlignHCenter
            text: gallery.themes.length === 0
                //% "Loading the themes."
                ? qsTrId("light_gal_loading")
                : gallery.query !== ""
                    //% "No theme matches. Clear the search or pick another filter."
                    ? qsTrId("light_gal_none")
                    : gallery.chip === "starred"
                        //% "No starred themes yet. Star a theme to keep it here, on Home and on the PREDATOR key."
                        ? qsTrId("light_gal_none_starred")
                        : gallery.chip === "yours"
                            //% "No themes of your own yet. Open a theme, change its colours or settings, then save it as a new theme."
                            ? qsTrId("light_gal_none_yours")
                            : qsTrId("light_gal_none")
            color: Theme.muted
            font.pixelSize: Theme.fontBody
            wrapMode: Text.Wrap
        }
    }

    GalleryGrid {
        id: grid
        detailAfter: gallery.detail !== null ? gallery.shown.indexOf(gallery.selected) : -1

        Repeater {
            id: cards
            model: gallery.shown
            delegate: GalleryCard {
                id: card
                required property string modelData
                required property int index
                readonly property var theme: gallery.byId[modelData]
                readonly property int way: gallery.wayOf(modelData)
                readonly property bool live: gallery.active === modelData
                readonly property var cellsNow: {
                    LightingVm.previewSerial    // re-read when a batch of pictures arrived
                    return LightingVm.previews[LightingVm.previewKey(modelData, way)]
                }

                Layout.preferredWidth: grid.cardWidth
                Layout.fillHeight: true
                Layout.row: grid.rowOf(index)
                Layout.column: grid.columnOf(index)
                visible: theme !== undefined
                title: theme ? theme.name : ""
                subtitle: theme ? theme.mood : ""
                layout: LightingVm.layout
                cells: cellsNow === undefined ? [] : cellsNow
                starrable: true
                starred: gallery.isStarred(modelData)
                //% "Star this theme"
                starName: qsTrId("light_gal_star")
                selected: gallery.selected === modelData
                highlighted: live
                badge: !theme ? "" : live
                    //% "In use"
                    ? qsTrId("light_gal_in_use")
                    : (theme.runnable === false
                        //% "Not available"
                        ? qsTrId("light_gal_unavailable")
                        : (!theme.builtin
                            //% "Yours"
                            ? qsTrId("light_theme_yours") : ""))
                badgeKind: live ? "ok" : (theme && theme.runnable === false ? "warn" : (theme && !theme.builtin ? "accent" : "neutral"))
                onActivated: gallery.picked(modelData)
                onStarToggled: gallery.starToggled(modelData)
                Component.onCompleted: gallery.ensure(modelData, way)
                onWayChanged: gallery.ensure(modelData, way)

                ColourwayChips {
                    Layout.fillWidth: true
                    visible: card.theme !== undefined && card.theme.colourways !== undefined && card.theme.colourways.length > 1
                    ways: card.theme && card.theme.colourways ? card.theme.colourways : []
                    current: card.way
                    onPicked: (i) => gallery.setWay(card.modelData, i)
                }
                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8
                    // The running theme's card stops it; every other card starts its theme. One
                    // quiet button per card: the editor's Apply is the strong one.
                    SecondaryButton {
                        visible: !card.live
                        //% "Apply"
                        text: qsTrId("light_theme_apply")
                        iconName: "play"
                        enabled: card.theme !== undefined && card.theme.runnable !== false
                        onClicked: gallery.apply(card.modelData, card.way)
                    }
                    SecondaryButton {
                        visible: card.live
                        //% "Stop"
                        text: qsTrId("light_theme_stop2")
                        iconName: "stop"
                        onClicked: gallery.stop()
                    }
                    Item { Layout.fillWidth: true }
                    ListIconButton {
                        iconName: "sliders"
                        //% "Change colours and settings"
                        text: qsTrId("light_gal_customise")
                        onClicked: gallery.picked(card.modelData)
                    }
                }
            }
        }

        // The editor of the chosen theme, under its row.
        Loader {
            id: detailLoader
            Layout.row: Math.max(0, grid.detailRow)
            Layout.column: 0
            Layout.columnSpan: grid.columns
            Layout.fillWidth: true
            Layout.topMargin: 2
            Layout.bottomMargin: 6
            visible: grid.detailAfter >= 0
            active: visible
            sourceComponent: gallery.detail
        }
    }
}
