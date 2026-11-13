// Lighting > Themes: the gallery of themes (each one look for keyboard, bar and logo), with the
// colourways, the settings that matter for the chosen theme, speed and brightness. Apply starts it
// (the key service draws it, at most 15 frames a second, only while it is the current look), Stop
// puts the static look back, and "Keep my version" saves the colours and values as a user theme.
// While the chosen theme is the running one, every change is applied at once (newest wins).
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import org.truesense.app

ColumnLayout {
    id: tab

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

    readonly property var entry: {
        for (var i = 0; i < themes.length; ++i)
            if (themes[i].id === selected) return themes[i]
        return null
    }
    readonly property bool running: active !== null && entry !== null && active.name === selected
    readonly property var ways: detail !== null ? detail.theme.colourways : []

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
            tab.selected = id
            tab.reload(function () { tab.choose(id) })
        })
    }

    function reload(done) {
        loadStars()
        Api.call("theme", "list", {}, function (r) {
            themes = r.themes || []
            skipped = r.skipped || []
            var st = r.status || {}
            active = st.active ? st.active.spec : null
            halted = st.halted ? String(st.halted) : ""
            if (selected === "" && themes.length > 0) choose(active ? active.name : themes[0].id)
            if (done) done()
        })
    }

    function choose(id) {
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
            if (active !== null && active.name === id) {
                if (active.colourway === "custom") { colourway = "custom"; if (active.custom) custom = active.custom }
                else if (typeof active.colourway === "number") colourway = active.colourway
                var p = active.params || {}
                for (var k in p) v[k] = p[k]
                values = v
                if (active.speed) speed = active.speed
                if (active.brightness) brightness = active.brightness
            }
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

    /// An edit of the running theme shows at once; any other theme waits for Apply.
    function edited() { if (running) apply(false) }

    function setValue(id, v) {
        var o = {}
        for (var k in values) o[k] = values[k]
        o[id] = v
        values = o
        edited()
    }

    Component.onCompleted: {
        LightingVm.loadLayout()
        reload()
    }

    Card {
        Layout.fillWidth: true
        //% "Running theme"
        title: qsTrId("light_theme_running")
        icon: "sparkle"

        Text {
            Layout.fillWidth: true
            text: tab.active !== null
                //% "Running: %1"
                ? qsTrId("light_theme_running_is").arg(tab.active.name)
                //% "No theme is running. The lights show the look you set last."
                : qsTrId("light_theme_none")
            color: Theme.text
            font.pixelSize: Theme.fontBody
            wrapMode: Text.Wrap
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
        SecondaryButton {
            //% "Stop"
            text: qsTrId("light_theme_stop")
            enabled: tab.active !== null
            onClicked: Api.call("theme", "stop", {}, function () { tab.reload() })
        }
        HelpText {
            //% "A theme is drawn by the key service while it runs. It stops when you set the keyboard another way."
            text: qsTrId("light_theme_help")
        }
    }

    Card {
        Layout.fillWidth: true
        //% "Gallery"
        title: qsTrId("light_theme_gallery")
        icon: "palette"

        ThemeGallery {
            Layout.fillWidth: true
            themes: tab.themes
            starred: tab.starred
            active: tab.active !== null ? tab.active.name : ""
            selected: tab.selected
            onPicked: (id) => tab.choose(id)
            onApply: (id, way) => tab.applyCard(id, way)
            onStop: Api.call("theme", "stop", {}, function () { tab.reload() })
            onStarToggled: (id) => tab.toggleStar(id)
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

    Card {
        Layout.fillWidth: true
        visible: tab.detail !== null
        title: tab.entry !== null ? tab.entry.name : ""
        icon: "sparkle"

        HelpText {
            text: tab.entry !== null ? tab.entry.mood : ""
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
                    //% "Custom"
                    Accessible.name: isCustom ? qsTrId("light_theme_custom") : modelData.name
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
                            text: way.isCustom ? qsTrId("light_theme_custom") : way.modelData.name
                            color: way.picked ? Theme.accent : Theme.text
                            font.pixelSize: Theme.fontSmall + 1
                            font.bold: way.picked
                        }
                    }
                    background: Rectangle {
                        radius: Theme.radiusSmall + 1
                        color: way.picked ? Theme.selBg : Theme.button
                        border.width: 1
                        border.color: way.activeFocus ? Theme.focusRing : (way.picked ? Theme.accent : Theme.buttonLine)
                    }
                }
            }
        }
        // The custom colours, one per slot, named by what the slot does in the effect.
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
                        border.width: 1
                        border.color: Theme.swatchLine
                        activeFocusOnTab: true
                        Accessible.role: Accessible.Button
                        Accessible.name: slot.modelData
                        function open() { tab.pickSlot = slot.index; picker.start(tab.custom[slot.index].substring(1)); picker.open() }
                        MouseArea { anchors.fill: parent; onClicked: parent.open() }
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

        Flow {
            Layout.fillWidth: true
            spacing: 8
            PrimaryButton {
                //% "Apply"
                text: qsTrId("light_theme_apply")
                enabled: tab.entry !== null && tab.entry.runnable !== false
                onClicked: tab.apply(true)
            }
            SecondaryButton {
                //% "Stop"
                text: qsTrId("light_theme_stop2")
                enabled: tab.running
                onClicked: Api.call("theme", "stop", {}, function () { tab.reload() })
            }
            AppButton {
                kind: "danger"
                //% "Delete"
                text: qsTrId("light_theme_delete")
                iconName: "trash"
                visible: tab.entry !== null && !tab.entry.builtin
                onClicked: {
                    var id = tab.selected
                    confirm.ask({
                        //% "Delete theme"
                        title: qsTrId("light_theme_delete_title"),
                        //% "Delete your theme '%1'?"
                        text: qsTrId("light_theme_delete_text").arg(id),
                        confirmText: qsTrId("light_theme_delete"),
                        danger: true
                    }, function () {
                        Api.call("theme", "delete", { name: id }, function () { LightingVm.forgetPreview(id); tab.selected = ""; tab.detail = null; tab.reload() })
                    })
                }
            }
        }
        SettingRow {
            Layout.fillWidth: true
            //% "Keep my version as"
            label: qsTrId("light_theme_keep")
            //% "Saves these colours and settings as a theme of your own, in ~/.config/truesense/themes."
            sub: qsTrId("light_theme_keep_sub")
            LightNameField {
                id: keepName
                Layout.preferredWidth: 180
                //% "Theme name"
                placeholderText: qsTrId("light_theme_keep_name")
                onAccepted: keepSave.clicked()
            }
            SecondaryButton {
                id: keepSave
                //% "Save"
                text: qsTrId("light_theme_keep_save")
                iconName: "save"
                onClicked: {
                    var taken = tab.themes.filter(function (t) { return t.builtin }).map(function (t) { return t.id })
                    var theme = LightingVm.call("theme_for_save", {
                        theme: tab.detail.theme, name: keepName.text, colourway: tab.colourway, custom: tab.custom,
                        values: tab.values, taken: taken })
                    if (theme === undefined) {
                        //% "Give the theme a name made of letters and digits that no built-in theme has."
                        Toasts.show(qsTrId("light_theme_keep_bad"), "error")
                        return
                    }
                    Api.call("theme", "save", { theme: theme }, function (made) {
                        LightingVm.forgetPreview(theme.id)
                        //% "Saved theme '%1'"
                        Toasts.show(qsTrId("light_theme_keep_saved").arg(theme.name), "ok")
                        keepName.text = ""
                        tab.selected = ""
                        tab.reload()
                        tab.choose(theme.id)
                    })
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
