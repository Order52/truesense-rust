// Lighting > Themes: what runs now, then the gallery of themes (each one look for keyboard, bar and
// logo). Apply on a card starts a theme as designed (the key service draws it, at most 15 frames a
// second, only while it is the current look); Stop puts the static look back. Clicking a card opens
// its editor right under the card's row: colourways, the settings that matter for that theme,
// speed and brightness, then "Save changes" (a theme of your own) or "Save as a new theme" (any
// theme). While the opened theme is the running one, every change is applied at once (newest wins).
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import org.truesense.app

ColumnLayout {
    id: tab
    objectName: "themesTab"

    spacing: Theme.spacing + 2

    property var themes: []
    property var skipped: []
    property var active: null           // spec of the running theme, or null
    property string halted: ""
    property string selected: ""
    property var detail: null           // `theme show`: the entry plus .theme
    property var settings: []
    property var values: ({})
    property var colourway: 0           // index or "custom"
    property var custom: []
    property int speed: 100
    property int brightness: 100
    property int pickSlot: 0
    property var starred: []            // ids of the starred themes
    property var previewCells: []       // one frame of the opened theme as the editor has it

    readonly property var entry: {
        for (var i = 0; i < themes.length; ++i)
            if (themes[i].id === selected) return themes[i]
        return null
    }
    readonly property bool running: active !== null && entry !== null && active.name === selected
    readonly property var ways: detail !== null ? detail.theme.colourways : []
    /// The running theme's list entry (for its display name), or null.
    readonly property var activeEntry: {
        if (active === null) return null
        for (var i = 0; i < themes.length; ++i)
            if (themes[i].id === active.name) return themes[i]
        return null
    }

    /// The star on a card: stars or unstars the theme (the favourites list, which the PREDATOR key
    /// and the Home page use), then shows what is stored.
    function toggleStar(id) {
        Api.call("favourite", "toggle", { kind: "theme", name: id }, function (r) {
            var list = starred.filter(function (n) { return n !== id })
            if (r.starred) list.push(id)
            starred = list
        })
    }

    function loadStars() {
        Api.call("favourite", "list", {}, function (list) {
            starred = (list || []).filter(function (f) { return f.kind === "theme" }).map(function (f) { return f.name })
        }, function () {})
    }

    /// One-click Apply on a card: that theme in the colourway of the card, everything else as designed.
    function applyCard(id, way) {
        Api.call("theme", "apply", { name: id, colourway: way }, function () {
            // An open editor of this theme shows what now runs.
            tab.reload(function () { if (tab.selected === id) tab.choose(id) })
        })
    }

    /// Closes the editor.
    function close() {
        selected = ""
        detail = null
    }

    /// Scrolls the page so that the opened editor shows, with the end of its card's row above it.
    function reveal(item) {
        var f = item
        while (f && (f.contentY === undefined || f.contentItem === undefined)) f = f.parent
        if (!f || !item) return
        var top = item.mapToItem(f.contentItem, 0, 0).y
        if (top >= f.contentY + 40 && top <= f.contentY + f.height * 0.6) return
        f.contentY = Math.max(0, Math.min(top - 140, f.contentHeight - f.height))
    }

    function reload(done) {
        loadStars()
        Api.call("theme", "list", {}, function (r) {
            themes = r.themes || []
            skipped = r.skipped || []
            var st = r.status || {}
            active = st.active ? st.active.spec : null
            halted = st.halted ? String(st.halted) : ""
            if (selected !== "" && entry === null) close()
            if (done) done()
        })
    }

    function choose(id) {
        if (selected === id && detail !== null) { close(); return }
        if (selected !== id) detail = null
        selected = id
        Api.call("theme", "show", { name: id }, function (d) {
            if (selected !== id) return
            detail = d
            settings = LightingVm.call("theme_settings", { theme: d.theme }) || []
            var v = {}
            for (var i = 0; i < settings.length; ++i) v[settings[i].id] = settings[i].value
            values = v
            custom = d.theme.palette.slice()
            colourway = 0
            // A theme that is running shows the choices it runs with.
            previewCells = []
            if (active !== null && active.name === id) {
                if (active.colourway === "custom") { colourway = "custom"; if (active.custom) custom = active.custom }
                else if (typeof active.colourway === "number") colourway = active.colourway
                var p = active.params || {}
                for (var k in p) v[k] = p[k]
                values = v
                if (active.speed) speed = active.speed
                if (active.brightness) brightness = active.brightness
            }
            updatePreview()
        })
    }

    function request() {
        return LightingVm.call("theme_request", {
            name: selected, colourway: colourway, custom: custom, values: values, speed: speed, brightness: brightness })
    }

    function apply(final) {
        var args = request()
        if (args === undefined) return
        if (final) {
            Api.call("theme", "apply", args, function () { tab.reload() })
        } else {
            Api.write("theme", "theme", "apply", args)
        }
    }

    /// An edit of the running theme shows at once; any other theme waits for Apply. The editor's
    /// picture follows every edit (asked once the edits pause, see `previewLater`).
    function edited() {
        if (running) apply(false)
        previewLater.restart()
    }

    /// Asks for one frame of the opened theme with the editor's colours and settings.
    function updatePreview() {
        var args = request()
        if (args === undefined) return
        var name = selected
        Api.call("theme", "preview", { name: name, colourway: args.colourway, custom: args.custom, params: args.params },
            function (r) { if (tab.selected === name) tab.previewCells = r.cells }, function () {})
    }

    function setValue(id, v) {
        var o = {}
        for (var k in values) o[k] = values[k]
        o[id] = v
        values = o
        edited()
    }

    /// Saves the editor's colours and settings as the user's theme `name` (a new one, or the opened
    /// one again for "Save changes"), then opens it.
    function saveAs(name, done) {
        var taken = themes.filter(function (t) { return t.builtin }).map(function (t) { return t.id })
        var theme = LightingVm.call("theme_for_save", {
            theme: detail.theme, name: name, colourway: colourway, custom: custom, values: values, taken: taken })
        if (theme === undefined) {
            //% "Give the theme a name made of letters and digits that no built-in theme has."
            Toasts.show(qsTrId("light_theme_keep_bad"), "error")
            return
        }
        Api.call("theme", "save", { theme: theme }, function () {
            //% "Saved theme '%1'"
            Toasts.show(qsTrId("light_theme_keep_saved").arg(theme.name), "ok")
            if (done) done()
            selected = ""
            reload(function () { choose(theme.id) })
        })
    }

    Component.onCompleted: {
        LightingVm.loadLayout()
        reload()
    }

    // ---- what runs now ------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Running theme"
        title: qsTrId("light_theme_running")
        icon: "sparkle"

        RowLayout {
            Layout.fillWidth: true
            spacing: 10
            Rectangle {
                Layout.preferredWidth: 10
                Layout.preferredHeight: 10
                Layout.alignment: Qt.AlignVCenter
                radius: 5
                color: tab.active !== null ? Theme.ok : Theme.switchOff
            }
            Text {
                Layout.fillWidth: true
                text: tab.active !== null
                    //% "Running: %1"
                    ? qsTrId("light_theme_running_is").arg(tab.activeEntry !== null ? tab.activeEntry.name : tab.active.name)
                    //% "No theme is running. The lights show the look you set last."
                    : qsTrId("light_theme_none")
                color: Theme.text
                font.pixelSize: Theme.fontBody
                font.bold: tab.active !== null
                wrapMode: Text.Wrap
            }
            SecondaryButton {
                visible: tab.activeEntry !== null && tab.selected !== tab.activeEntry.id
                //% "Change it"
                text: qsTrId("light_theme_edit_running")
                iconName: "sliders"
                onClicked: tab.choose(tab.activeEntry.id)
            }
            SecondaryButton {
                visible: tab.active !== null
                //% "Stop"
                text: qsTrId("light_theme_stop")
                iconName: "stop"
                onClicked: Api.call("theme", "stop", {}, function () { tab.reload() })
            }
        }
        Text {
            Layout.fillWidth: true
            visible: tab.halted !== ""
            //% "Stopped: %1"
            text: qsTrId("light_theme_halted").arg(tab.halted)
            color: Theme.warn
            font.pixelSize: Theme.fontSmall + 1
            wrapMode: Text.Wrap
        }
        HelpText {
            //% "A theme is drawn by the key service while it runs. It stops when you set the keyboard another way."
            text: qsTrId("light_theme_help")
        }
    }

    // ---- the gallery, with the editor under the chosen card ------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Gallery"
        title: qsTrId("light_theme_gallery")
        icon: "palette"

        ThemeGallery {
            id: gallery
            objectName: "themeGallery"
            Layout.fillWidth: true
            themes: tab.themes
            starred: tab.starred
            active: tab.active !== null ? tab.active.name : ""
            selected: tab.detail !== null ? tab.selected : ""
            detail: editor
            onPicked: (id) => tab.choose(id)
            onApply: (id, way) => tab.applyCard(id, way)
            onStop: Api.call("theme", "stop", {}, function () { tab.reload() })
            onStarToggled: (id) => tab.toggleStar(id)
            onDetailItemChanged: if (detailItem !== null) revealLater.restart()
        }
        Repeater {
            model: tab.skipped
            delegate: Text {
                required property var modelData
                Layout.fillWidth: true
                //% "Not a theme: %1 (%2)"
                text: qsTrId("light_theme_skipped").arg(modelData.file).arg(modelData.reason)
                color: Theme.warn
                font.pixelSize: Theme.fontSmall + 1
                wrapMode: Text.Wrap
            }
        }
    }
    // Once, a moment after the last edit (a slider drag asks for one picture, not fifty).
    Timer {
        id: previewLater
        interval: 200
        onTriggered: tab.updatePreview()
    }
    // Once, after the editor opened and the grid placed it (not a running timer).
    Timer {
        id: revealLater
        interval: 30
        onTriggered: tab.reveal(gallery.detailItem)
    }

    // The editor of the opened theme; the gallery puts it under the theme's card.
    Component {
        id: editor
        Rectangle {
            id: panel
            implicitHeight: edit.implicitHeight + 2 * Theme.pad
            radius: Theme.radius
            color: Theme.card
            border.width: 1
            border.color: Theme.accentLine

            ColumnLayout {
                id: edit
                x: Theme.pad
                y: Theme.pad
                width: panel.width - 2 * Theme.pad
                spacing: Theme.spacing

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8
                    Icon { name: "sliders"; size: 20; color: Theme.accent }
                    Text {
                        Layout.fillWidth: true
                        //% "Change %1"
                        text: qsTrId("light_theme_edit_title").arg(tab.entry !== null ? tab.entry.name : "")
                        color: Theme.text
                        font.pixelSize: Theme.fontSection
                        font.bold: true
                        elide: Text.ElideRight
                    }
                    Badge {
                        visible: tab.entry !== null && !tab.entry.builtin
                        text: qsTrId("light_theme_yours")
                        kind: "accent"
                    }
                    ListIconButton {
                        iconName: "close"
                        //% "Close"
                        text: qsTrId("light_theme_close")
                        onClicked: tab.close()
                    }
                }
                HelpText {
                    visible: text !== ""
                    text: tab.entry !== null ? tab.entry.mood : ""
                }
                // The theme as the editor has it now: one frame, drawn by the theme engine.
                KeyboardThumb {
                    Layout.fillWidth: true
                    Layout.maximumWidth: 480
                    Layout.preferredHeight: width * LightingVm.layout.height / Math.max(1, LightingVm.layout.width)
                    layout: LightingVm.layout
                    cells: tab.previewCells
                    capColor: Theme.buttonOff
                    //% "Picture of the theme with the colours and settings chosen here"
                    description: qsTrId("light_theme_preview_a11y")
                }
                Text {
                    Layout.fillWidth: true
                    visible: tab.entry !== null && tab.entry.runnable === false
                    //% "This theme flashes in a way that was never measured, so it is listed but cannot be started."
                    text: qsTrId("light_theme_not_runnable")
                    color: Theme.warn
                    font.pixelSize: Theme.fontSmall + 1
                    wrapMode: Text.Wrap
                }

                Text {
                    Layout.fillWidth: true
                    //% "Colours"
                    text: qsTrId("light_theme_colours")
                    color: Theme.text
                    font.pixelSize: Theme.fontBody
                }
                Flow {
                    Layout.fillWidth: true
                    spacing: 8
                    Repeater {
                        model: tab.ways.concat([{ name: "", palette: [], custom: true }])
                        delegate: Button {
                            id: way
                            required property var modelData
                            required property int index
                            readonly property bool isCustom: modelData.custom === true
                            readonly property bool picked: isCustom ? tab.colourway === "custom" : tab.colourway === index
                            height: Theme.controlHeight
                            hoverEnabled: true
                            focusPolicy: Qt.StrongFocus
                            Accessible.role: Accessible.RadioButton
                            Accessible.checked: picked
                            //% "Your colours"
                            Accessible.name: isCustom ? qsTrId("light_theme_custom2") : modelData.name
                            onClicked: { tab.colourway = isCustom ? "custom" : index; tab.edited() }
                            contentItem: RowLayout {
                                spacing: 6
                                Row {
                                    spacing: 2
                                    Repeater {
                                        model: way.isCustom ? tab.custom : way.modelData.palette
                                        delegate: Rectangle {
                                            required property string modelData
                                            width: 12; height: 12; radius: 6
                                            color: modelData
                                        }
                                    }
                                }
                                Text {
                                    text: way.isCustom ? qsTrId("light_theme_custom2") : way.modelData.name
                                    color: way.picked ? Theme.accent : Theme.text
                                    font.pixelSize: Theme.fontSmall + 1
                                    font.bold: way.picked
                                }
                            }
                            background: Rectangle {
                                radius: Theme.radiusSmall + 1
                                color: way.picked ? Theme.selBg : (way.hovered ? Theme.buttonHover : Theme.button)
                                border.width: 1
                                border.color: way.activeFocus ? Theme.focusRing : (way.picked ? Theme.accent : Theme.buttonLine)
                            }
                        }
                    }
                }
                // The custom colours, one per slot, named by what the slot does in the effect.
                HelpText {
                    visible: tab.colourway === "custom"
                    //% "Click a colour to change it."
                    text: qsTrId("light_theme_custom_help")
                }
                Flow {
                    Layout.fillWidth: true
                    visible: tab.colourway === "custom" && tab.detail !== null
                    spacing: 12
                    Repeater {
                        model: tab.detail !== null ? tab.detail.theme.custom : []
                        delegate: RowLayout {
                            id: slot
                            required property string modelData
                            required property int index
                            spacing: 6
                            Rectangle {
                                Layout.preferredWidth: 30; Layout.preferredHeight: 30
                                radius: 8
                                color: tab.custom.length > slot.index ? tab.custom[slot.index] : "transparent"
                                border.width: activeFocus ? 2 : 1
                                border.color: activeFocus ? Theme.focusRing : Theme.swatchLine
                                activeFocusOnTab: true
                                Accessible.role: Accessible.Button
                                Accessible.name: slot.modelData
                                function open() { tab.pickSlot = slot.index; picker.start(tab.custom[slot.index].substring(1)); picker.open() }
                                MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: parent.open() }
                                Keys.onSpacePressed: open()
                                Keys.onReturnPressed: open()
                            }
                            Text {
                                text: slot.modelData
                                color: Theme.muted
                                font.pixelSize: Theme.fontSmall + 1
                            }
                        }
                    }
                }

                Repeater {
                    model: tab.settings
                    delegate: Loader {
                        id: setting
                        required property var modelData
                        Layout.fillWidth: true
                        property var def: modelData
                        sourceComponent: def.kind === "number" ? numberRow : def.kind === "choice" ? choiceRow
                            : def.kind === "toggle" ? toggleRow : keysRow
                    }
                }

                SliderRow {
                    Layout.fillWidth: true
                    //% "Speed"
                    label: qsTrId("light_theme_speed")
                    from: 10
                    to: 100
                    stepSize: 5
                    unit: "%"
                    value: tab.speed
                    coalesceMs: 150
                    onEdited: (v) => { tab.speed = v; tab.edited() }
                }
                SliderRow {
                    Layout.fillWidth: true
                    //% "Brightness"
                    label: qsTrId("light_theme_brightness")
                    from: 1
                    to: 100
                    stepSize: 1
                    unit: "%"
                    value: tab.brightness
                    coalesceMs: 150
                    onEdited: (v) => { tab.brightness = v; tab.edited() }
                }

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8
                    PrimaryButton {
                        visible: !tab.running
                        //% "Apply"
                        text: qsTrId("light_theme_apply")
                        iconName: "play"
                        enabled: tab.entry !== null && tab.entry.runnable !== false
                        onClicked: tab.apply(true)
                    }
                    SecondaryButton {
                        visible: tab.running
                        //% "Stop"
                        text: qsTrId("light_theme_stop2")
                        iconName: "stop"
                        onClicked: Api.call("theme", "stop", {}, function () { tab.reload() })
                    }
                    HelpText {
                        Layout.fillWidth: true
                        text: tab.running
                            //% "This theme is running: your changes show on the keyboard at once."
                            ? qsTrId("light_theme_live_note")
                            //% "Apply shows your changes on the keyboard."
                            : qsTrId("light_theme_apply_note")
                    }
                }

                Rectangle {
                    Layout.fillWidth: true
                    Layout.topMargin: 4
                    implicitHeight: 1
                    color: Theme.sep
                }

                // Keep it: save over the person's own theme, or as a new one.
                Text {
                    Layout.fillWidth: true
                    //% "Keep your version"
                    text: qsTrId("light_theme_keep_title")
                    color: Theme.text
                    font.pixelSize: Theme.fontBody
                    font.bold: true
                }
                RowLayout {
                    Layout.fillWidth: true
                    visible: tab.entry !== null && !tab.entry.builtin
                    spacing: 8
                    SecondaryButton {
                        //% "Save changes"
                        text: qsTrId("light_theme_save_changes")
                        iconName: "save"
                        onClicked: tab.saveAs(tab.entry.name)
                    }
                    Item { Layout.fillWidth: true }
                    SecondaryButton {
                        //% "Delete"
                        text: qsTrId("light_theme_delete")
                        iconName: "trash"
                        onClicked: {
                            var id = tab.selected
                            var name = tab.entry.name
                            confirm.ask({
                                //% "Delete theme"
                                title: qsTrId("light_theme_delete_title"),
                                //% "Delete your theme '%1'?"
                                text: qsTrId("light_theme_delete_text").arg(name),
                                confirmText: qsTrId("light_theme_delete"),
                                danger: true
                            }, function () {
                                Api.call("theme", "delete", { name: id }, function () { tab.close(); tab.reload() })
                            })
                        }
                    }
                }
                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8
                    LightNameField {
                        id: keepName
                        Layout.fillWidth: true
                        Layout.maximumWidth: 320
                        //% "Name of the new theme"
                        placeholderText: qsTrId("light_theme_keep_name2")
                        onAccepted: keepSave.clicked()
                    }
                    SecondaryButton {
                        id: keepSave
                        //% "Save as a new theme"
                        text: qsTrId("light_theme_save_new")
                        iconName: "plus"
                        enabled: keepName.text.trim() !== ""
                        onClicked: tab.saveAs(keepName.text, function () { keepName.text = "" })
                    }
                }
                HelpText {
                    //% "Your themes keep their colours and settings, are listed under Yours and live in ~/.config/truesense/themes. Speed and brightness are chosen each time you apply one."
                    text: qsTrId("light_theme_keep_sub2")
                }
            }
        }
    }

    ConfirmDialog { id: confirm }

    ColorPickerPopup {
        id: picker
        parent: Overlay.overlay
        anchors.centerIn: Overlay.overlay
        onPicked: (hex) => {
            var c = tab.custom.slice()
            c[tab.pickSlot] = "#" + hex.toUpperCase()
            tab.custom = c
            tab.edited()
        }
    }

    // The controls of the settings, one component per kind (the `def` of the Loader is the setting).
    Component {
        id: numberRow
        SliderRow {
            label: def.label
            from: def.min
            to: def.max
            stepSize: def.step
            unit: def.clock ? "" : def.unit
            value: tab.values[def.id] === undefined ? def.min : tab.values[def.id]
            valueText: def.clock ? (LightingVm.call("clock_text", { hours: value }) || "") : ""
            coalesceMs: 150
            onEdited: (v) => tab.setValue(def.id, v)
        }
    }
    Component {
        id: choiceRow
        SelectRow {
            label: def.label
            options: def.options.map(function (o, i) { return { value: i, text: o } })
            current: tab.values[def.id]
            onActivated: (i) => tab.setValue(def.id, i)
        }
    }
    Component {
        id: toggleRow
        ToggleRow {
            label: def.label
            checked: tab.values[def.id] === true
            onToggled: (on) => tab.setValue(def.id, on)
        }
    }
    Component {
        id: keysRow
        SettingRow {
            label: def.label
            //% "Key names separated by commas, for example w, a, s, d"
            sub: qsTrId("light_theme_keys_sub")
            LightNameField {
                Layout.preferredWidth: 200
                maximumLength: 120
                text: Array.isArray(tab.values[def.id]) ? tab.values[def.id].join(", ") : (tab.values[def.id] || "")
                onEditingFinished: {
                    var names = text.split(",").map(function (s) { return s.trim() }).filter(function (s) { return s !== "" })
                    tab.setValue(def.id, def.kind === "key" ? (names.length ? names[0] : "") : names)
                }
            }
        }
    }
}
