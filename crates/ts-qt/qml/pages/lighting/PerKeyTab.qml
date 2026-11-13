// Lighting > Per-key: paint single keys. Click or drag over keys to paint them with the chosen
// colour (a fast drag paints every key on the line it crossed), right-click picks a key's colour up,
// Select mode collects keys (drag a box, Ctrl adds) for the tools. Every change goes to the keyboard
// at once through Api.write, which keeps only the newest picture while the keyboard (about 50 ms
// per upload) is busy, so a drag never floods it.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

ColumnLayout {
    id: tab
    objectName: "perKeyTab"

    spacing: Theme.spacing + 2

    property var cells: LightingVm.call("stock_cells")
    property var inks: []
    property string colour: "ff3b30"
    property var recent: []
    property string tool: "paint"          // paint | select
    property var selection: []
    property var baseSelection: []
    property var boxStart: null
    property int focusCell: -1
    property var history: []
    property var redo: []
    property bool strokeChanged: false
    property string gradFrom: "ff2d55"
    property string gradTo: "5856d6"
    property string pickTarget: "colour"   // colour | from | to
    property string fxMode: "off"
    property int fxSpeed: 5
    property int brightness: 25
    property var designs: []
    /// The built-in designs, in the order they ship: [{id, name, mood, builtin, top, suits}], the
    /// cards of the Templates section, and the pictures drawn for them so far (by name).
    property var templates: []
    property var templatePictures: ({})
    property string templateQuery: ""
    property string selectedDesign: ""
    readonly property bool usable: Caps.usable("keyboard-light") && Caps.usable("per_key_rgb")

    // ---- state ------------------------------------------------------------------------------

    function setCells(list) {
        cells = list
        var i = LightingVm.call("inks", { cells: list })
        if (i !== undefined) inks = i
    }

    function _snapshot() {
        var h = history.slice(-39)
        h.push(cells)
        history = h
        redo = []
    }

    function upload() {
        if (fxMode !== "off") pushFx()
        else Api.write("kb.cells", "keyboard", "set-cells", { cells: cells })
    }

    function pushFx() {
        var args = { mode: fxMode, speed: fxSpeed, base: cells }
        if (selection.length > 0) args.cells = selection
        Api.write("kb.fx", "keyboard", "fx", args)
    }

    /// Takes new colours for the keyboard as one undoable step and sends them.
    function commit(list) {
        _snapshot()
        setCells(list)
        upload()
    }

    function undo() {
        if (history.length === 0) return
        var r = redo.slice(); r.push(cells); redo = r
        var h = history.slice(); var back = h.pop(); history = h
        setCells(back)
        upload()
    }

    function redoStep() {
        if (redo.length === 0) return
        var h = history.slice(); h.push(cells); history = h
        var r = redo.slice(); var fwd = r.pop(); redo = r
        setCells(fwd)
        upload()
    }

    function setColour(hex, applyToSelection) {
        colour = hex
        var known = LightingVm.call("palette").indexOf(hex) >= 0
        if (!known && recent.indexOf(hex) < 0) recent = [hex].concat(recent).slice(0, 8)
        if (applyToSelection && tool === "select" && selection.length > 0) {
            var out = LightingVm.call("paint", { cells: cells, targets: selection, colour: hex })
            if (out !== undefined) commit(out)
        }
    }

    function applyTool(name, extra) {
        var args = { tool: name, cells: cells, selection: selection }
        for (var k in extra) args[k] = extra[k]
        var out = LightingVm.callKeys("tool", args)
        if (out !== undefined) commit(out)
    }

    function select(op, extra) {
        var args = { op: op, selection: selection }
        for (var k in extra) args[k] = extra[k]
        var out = LightingVm.callKeys("select", args)
        if (out !== undefined) selection = out
    }

    // ---- pointer ----------------------------------------------------------------------------

    function _paint(from, to) {
        var r = LightingVm.callKeys("stroke", { from: from, to: to, colour: colour, cells: cells })
        if (r === undefined || !r.changed) return
        if (!strokeChanged) { _snapshot(); strokeChanged = true }
        setCells(r.cells)
        upload()
    }

    function pressed(p, button, extend) {
        var hit = LightingVm.callKeys("hit", { at: p })
        if (hit !== undefined && hit !== null) focusCell = hit[0]
        if (button === Qt.RightButton) {
            // The eyedropper: take the colour of the key under the pointer.
            if (hit) setColour(cells[hit[0]], false)
            return
        }
        if (tool === "paint") {
            strokeChanged = false
            _paint(p, p)
        } else if (extend && hit && selection.indexOf(hit[0]) >= 0) {
            boxStart = null
            select("toggle", { cell: hit[0] })
        } else {
            baseSelection = extend ? selection : []
            boxStart = p
            select("box", { from: p, to: p, add: extend, selection: baseSelection })
        }
    }

    function dragged(from, to, extend) {
        if (tool === "paint") _paint(from, to)
        else if (boxStart !== null) select("box", { from: boxStart, to: to, add: extend, selection: baseSelection })
    }

    function navigated(dir, extend) {
        if (focusCell < 0 && LightingVm.layout.keys.length > 0) focusCell = LightingVm.layout.keys[0].cells[0]
        if (dir === "activate") {
            if (tool === "paint") {
                var key = null
                var keys = LightingVm.layout.keys
                for (var i = 0; i < keys.length; ++i) if (keys[i].cells.indexOf(focusCell) >= 0) key = keys[i]
                if (key === null) return
                var r = LightingVm.callKeys("stroke", { from: { x: key.x + key.w / 2, y: key.y + key.h / 2 }, to: { x: key.x + key.w / 2, y: key.y + key.h / 2 }, colour: colour, cells: cells })
                if (r !== undefined && r.changed) { _snapshot(); setCells(r.cells); upload() }
            } else {
                select("toggle", { cell: focusCell })
            }
            return
        }
        var next = LightingVm.callKeys("neighbour", { cell: focusCell, dir: dir })
        if (next !== undefined && next !== null) {
            focusCell = next
            if (extend && tool === "select" && selection.indexOf(next) < 0) select("toggle", { cell: next })
        }
    }

    // ---- reading ----------------------------------------------------------------------------

    function reload() {
        LightingVm.loadLayout()
        Api.call("keyboard", "cells", {}, function (c) {
            if (history.length === 0 && Array.isArray(c.cells)) setCells(c.cells)
        }, function () {})
        Api.call("keyboard", "status", {}, function (st) {
            if (typeof st.brightness === "number") brightness = st.brightness
        }, function () {})
        loadDesigns()
    }

    function loadDesigns(pick) {
        Api.call("keyboard", "design", { action: "list" }, function (d) {
            var names = Object.keys(d || {}).sort()
            designs = names.map(function (n) { return { value: n, text: d[n].source ? n + "  (" + d[n].source + ")" : n } })
            templates = Object.keys(d || {}).filter(function (n) { return d[n].source === "built-in" })
                .map(function (n) { return { id: n, name: n, mood: d[n].info || "", builtin: true, top: 0, suits: [] } })
            selectedDesign = pick !== undefined ? pick : (names.indexOf(selectedDesign) >= 0 ? selectedDesign : (names.length ? names[0] : ""))
        }, function () {})
    }

    /// Asks for the colours of a template once; its card draws them.
    function pictureOf(name) {
        if (templatePictures[name] !== undefined) return templatePictures[name]
        var p = {}
        for (var k in templatePictures) p[k] = templatePictures[k]
        p[name] = []
        templatePictures = p
        Api.call("keyboard", "design", { action: "show", name: name }, function (d) {
            var q = {}
            for (var k2 in templatePictures) q[k2] = templatePictures[k2]
            q[name] = d.cells
            templatePictures = q
        }, function () {})
        return []
    }

    /// Puts a template on the keyboard and into the editor, as one step that Undo takes back.
    function applyTemplate(name) {
        Api.call("keyboard", "design", { action: "show", name: name }, function (d) {
            commit(d.cells)
            //% "Template '%1' applied"
            Toasts.show(qsTrId("light_pk_template_applied").arg(name), "ok")
        })
    }

    /// Saves a template as a design of the user's (NAME-copy, numbered until the name is free), so it
    /// can be changed on the board and kept; it is not sent to the keyboard.
    function copyTemplate(name) {
        var taken = designs.map(function (d) { return d.value })
        var copy = name + "-copy"
        for (var i = 2; taken.indexOf(copy) >= 0; ++i) copy = name + "-copy" + i
        var target = copy
        Api.call("keyboard", "design", { action: "show", name: name }, function (d) {
            Api.call("keyboard", "design", { action: "save", name: target, cells: d.cells }, function () {
                //% "Saved a copy as '%1'. Apply it under Designs, then change it on the board."
                Toasts.show(qsTrId("light_pk_template_copied").arg(target), "ok")
                loadDesigns(target)
            })
        })
    }

    Component.onCompleted: {
        setCells(cells)
        reload()
    }

    CapabilityNote {
        capability: "keyboard-light"
        //% "This laptop has no keyboard backlight control."
        text: qsTrId("light_pk_none")
    }
    CapabilityNote { capability: "per_key_rgb" }

    Card {
        Layout.fillWidth: true
        enabled: tab.usable
        //% "Keys"
        title: qsTrId("light_pk_keys")
        icon: "keyboard"

        RowLayout {
            Layout.fillWidth: true
            spacing: 10
            Segmented {
                current: tab.tool
                model: [
                    //% "Paint"
                    { value: "paint", label: qsTrId("light_pk_tool_paint") },
                    //% "Select"
                    { value: "select", label: qsTrId("light_pk_tool_select") }
                ]
                onActivated: (v) => tab.tool = v
            }
            Item { Layout.fillWidth: true }
            Rectangle {
                Layout.preferredWidth: 28; Layout.preferredHeight: 28
                radius: 6
                color: LightingVm.css(tab.colour)
                border.width: 1
                border.color: Theme.swatchLine
                Accessible.role: Accessible.StaticText
                //% "Colour in use"
                Accessible.name: qsTrId("light_pk_colour_in_use")
            }
        }
        KeyboardView {
            id: board
            Layout.fillWidth: true
            interactive: true
            showLabels: true
            cells: tab.cells
            inks: tab.inks
            selection: tab.selection
            focusCell: tab.focusCell
            //% "Keyboard editor. Arrow keys move between keys, Space paints or selects the key."
            description: qsTrId("light_pk_board_a11y")
            onPressedAt: (p, button, extend) => tab.pressed(p, button, extend)
            onDragged: (from, to, extend) => tab.dragged(from, to, extend)
            onReleased: { if (tab.tool === "paint") tab.strokeChanged = false; tab.boxStart = null }
            onNavigated: (dir, extend) => tab.navigated(dir, extend)
        }
        HelpText {
            text: tab.tool === "paint"
                //% "Click a key or drag across keys to paint them with the chosen colour. Right-click a key to pick up its colour."
                ? qsTrId("light_pk_help_paint")
                //% "Drag a box around keys to select them, Ctrl adds more. A colour you choose then paints all selected keys; the tools below work on them."
                : qsTrId("light_pk_help_select")
        }
        RowLayout {
            Layout.fillWidth: true
            spacing: 8
            Text {
                Layout.fillWidth: true
                text: tab.selection.length === 0
                    //% "No keys selected"
                    ? qsTrId("light_pk_none_selected")
                    //% "Selected keys: %1"
                    : qsTrId("light_pk_selected").arg(tab.selection.length)
                color: Theme.muted
                font.pixelSize: Theme.fontSmall + 1
                Accessible.role: Accessible.StaticText
            }
            SelectRow {
                minimumControlWidth: 150
                options: [
                    //% "Select by group"
                    { value: "", text: qsTrId("light_pk_group") },
                    //% "All keys"
                    { value: "all", text: qsTrId("light_pk_group_all") },
                    //% "WASD"
                    { value: "wasd", text: qsTrId("light_pk_group_wasd") },
                    //% "Arrows"
                    { value: "arrows", text: qsTrId("light_pk_group_arrows") },
                    //% "F1 to F12"
                    { value: "fkeys", text: qsTrId("light_pk_group_fkeys") },
                    //% "Number row"
                    { value: "numrow", text: qsTrId("light_pk_group_numrow") },
                    //% "Letters"
                    { value: "letters", text: qsTrId("light_pk_group_letters") },
                    //% "Numpad"
                    { value: "numpad", text: qsTrId("light_pk_group_numpad") }
                ]
                current: ""
                onActivated: (g) => { if (g !== "") { tab.tool = "select"; tab.select("group", { group: g }) } }
            }
            SecondaryButton {
                //% "Invert"
                text: qsTrId("light_pk_invert")
                onClicked: { tab.tool = "select"; tab.select("invert", {}) }
            }
            SecondaryButton {
                //% "Clear selection"
                text: qsTrId("light_pk_deselect")
                enabled: tab.selection.length > 0
                onClicked: tab.select("none", {})
            }
        }
    }

    Card {
        Layout.fillWidth: true
        enabled: tab.usable
        //% "Colour"
        title: qsTrId("light_pk_colour")
        icon: "palette"

        Text {
            Layout.fillWidth: true
            //% "Palette"
            text: qsTrId("light_pk_palette")
            color: Theme.text
            font.pixelSize: Theme.fontBody
        }
        SwatchRow {
            Layout.fillWidth: true
            colors: LightingVm.call("palette").map(LightingVm.css)
            current: LightingVm.css(tab.colour)
            allowCustom: true
            onActivated: (c) => tab.setColour(c.substring(1), true)
            onCustomRequested: { tab.pickTarget = "colour"; picker.start(tab.colour); picker.open() }
        }
        Text {
            Layout.fillWidth: true
            visible: tab.recent.length > 0
            //% "Recent"
            text: qsTrId("light_pk_recent")
            color: Theme.text
            font.pixelSize: Theme.fontBody
        }
        SwatchRow {
            Layout.fillWidth: true
            visible: tab.recent.length > 0
            colors: tab.recent.map(LightingVm.css)
            current: LightingVm.css(tab.colour)
            onActivated: (c) => tab.setColour(c.substring(1), true)
        }
    }

    Card {
        Layout.fillWidth: true
        enabled: tab.usable
        //% "Tools"
        title: qsTrId("light_pk_tools")
        icon: "sliders"

        HelpText {
            //% "The tools work on the selected keys, or on every key when none is selected."
            text: qsTrId("light_pk_tools_help")
        }
        Flow {
            Layout.fillWidth: true
            spacing: 8
            SecondaryButton {
                //% "Fill with the colour"
                text: qsTrId("light_pk_fill")
                onClicked: tab.applyTool("fill", { colour: tab.colour })
            }
            SecondaryButton {
                //% "Rainbow"
                text: qsTrId("light_pk_rainbow")
                onClicked: tab.applyTool("rainbow", {})
            }
            SecondaryButton {
                //% "Clear (dark)"
                text: qsTrId("light_pk_clear")
                onClicked: tab.applyTool("clear", {})
            }
            SecondaryButton {
                //% "Undo"
                text: qsTrId("light_pk_undo")
                iconName: "undo"
                enabled: tab.history.length > 0
                onClicked: tab.undo()
            }
            SecondaryButton {
                //% "Redo"
                text: qsTrId("light_pk_redo")
                enabled: tab.redo.length > 0
                onClicked: tab.redoStep()
            }
        }
        SettingRow {
            Layout.fillWidth: true
            //% "Gradient from left to right"
            label: qsTrId("light_pk_gradient")
            Repeater {
                model: 2
                delegate: Rectangle {
                    id: end
                    required property int index
                    readonly property string hex: index === 0 ? tab.gradFrom : tab.gradTo
                    Layout.preferredWidth: 34; Layout.preferredHeight: 34
                    radius: 8
                    color: LightingVm.css(hex)
                    border.width: 1
                    border.color: Theme.swatchLine
                    activeFocusOnTab: true
                    Accessible.role: Accessible.Button
                    Accessible.name: index === 0
                        //% "Gradient start colour"
                        ? qsTrId("light_pk_grad_from")
                        //% "Gradient end colour"
                        : qsTrId("light_pk_grad_to")
                    function open() {
                        tab.pickTarget = index === 0 ? "from" : "to"
                        picker.start(hex)
                        picker.open()
                    }
                    MouseArea { anchors.fill: parent; onClicked: end.open() }
                    Keys.onSpacePressed: open()
                    Keys.onReturnPressed: open()
                }
            }
            PrimaryButton {
                //% "Apply"
                text: qsTrId("light_pk_grad_apply")
                onClicked: tab.applyTool("gradient", { from: tab.gradFrom, to: tab.gradTo })
            }
        }
    }

    Card {
        Layout.fillWidth: true
        enabled: tab.usable
        //% "Animation"
        title: qsTrId("light_pk_anim")
        icon: "wave"

        HelpText {
            //% "The keyboard cannot animate single keys by itself, so TrueSense draws it, about 5 frames a second, while the key service runs. It animates the selected keys, or every key when none is selected."
            text: qsTrId("light_pk_anim_help")
        }
        SelectRow {
            Layout.fillWidth: true
            //% "Animation"
            label: qsTrId("light_pk_anim_mode")
            options: ["off"].concat(LightingVm.call("fx_modes")).map(function (m) { return { value: m, text: LightingVm.animationName(m) } })
            current: tab.fxMode
            onActivated: (m) => { tab.fxMode = m; tab.pushFx() }
        }
        SliderRow {
            Layout.fillWidth: true
            enabled: tab.fxMode !== "off"
            //% "Speed"
            label: qsTrId("light_pk_anim_speed")
            from: 1
            to: 10
            stepSize: 1
            value: tab.fxSpeed
            coalesceMs: 250
            onEdited: (v) => { tab.fxSpeed = v; tab.pushFx() }
        }
    }

    Card {
        Layout.fillWidth: true
        enabled: tab.usable
        visible: tab.templates.length > 0
        //% "Templates"
        title: qsTrId("light_pk_templates")
        icon: "palette"

        HelpText {
            //% "Ready-made looks for gaming, typing, editing and the evening. Apply one to start from it: it goes to the keyboard and the board above, and Undo takes it back."
            text: qsTrId("light_pk_templates_help")
        }
        AppTextField {
            Layout.fillWidth: true
            //% "Search templates"
            placeholderText: qsTrId("light_pk_templates_search")
            Accessible.name: placeholderText
            text: tab.templateQuery
            onTextEdited: tab.templateQuery = text
            Keys.onEscapePressed: { text = ""; tab.templateQuery = "" }
        }
        GalleryGrid {
            id: templateGrid
            minCardWidth: 250
            Repeater {
                model: LightingVm.gallery("filter", { themes: tab.templates, query: tab.templateQuery, chip: "all" }) || []
                delegate: GalleryCard {
                    id: tcard
                    required property string modelData
                    readonly property var entry: {
                        for (var i = 0; i < tab.templates.length; ++i)
                            if (tab.templates[i].id === modelData) return tab.templates[i]
                        return null
                    }
                    Layout.preferredWidth: templateGrid.cardWidth
                    Layout.fillHeight: true
                    title: LightingVm.prettyName(modelData)
                    subtitle: entry ? entry.mood : ""
                    layout: LightingVm.layout
                    cells: tab.pictureOf(modelData)
                    onActivated: tab.applyTemplate(modelData)
                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 8
                        SecondaryButton {
                            //% "Apply"
                            text: qsTrId("light_pk_template_apply")
                            iconName: "play"
                            onClicked: tab.applyTemplate(tcard.modelData)
                        }
                        SecondaryButton {
                            //% "Save a copy"
                            text: qsTrId("light_pk_template_copy")
                            iconName: "copy"
                            onClicked: tab.copyTemplate(tcard.modelData)
                        }
                    }
                }
            }
        }
        Text {
            Layout.fillWidth: true
            visible: templateGrid.children.length <= 1 && tab.templateQuery !== ""
            //% "No template matches the search."
            text: qsTrId("light_pk_templates_none")
            color: Theme.muted
            font.pixelSize: Theme.fontBody
        }
    }

    Card {
        Layout.fillWidth: true
        enabled: tab.usable
        //% "Designs"
        title: qsTrId("light_pk_designs")
        icon: "star"

        HelpText {
            //% "A design is a still picture of colours. Yours are kept in ~/.config/truesense/designs."
            text: qsTrId("light_pk_designs_help")
        }
        SelectRow {
            Layout.fillWidth: true
            enabled: tab.designs.length > 0
            //% "Design"
            label: qsTrId("light_pk_design")
            options: tab.designs
            current: tab.selectedDesign
            onActivated: (n) => tab.selectedDesign = n
        }
        Flow {
            Layout.fillWidth: true
            spacing: 8
            PrimaryButton {
                //% "Apply design"
                text: qsTrId("light_pk_design_apply")
                enabled: tab.selectedDesign !== ""
                onClicked: {
                    var name = tab.selectedDesign
                    Api.call("keyboard", "design", { action: "show", name: name }, function (d) {
                        tab.commit(d.cells)
                        //% "Design '%1' applied"
                        Toasts.show(qsTrId("light_pk_design_applied").arg(name), "ok")
                    })
                }
            }
        }
        SettingRow {
            Layout.fillWidth: true
            //% "Save the keys as a design"
            label: qsTrId("light_pk_design_save")
            LightNameField {
                id: designName
                Layout.preferredWidth: 180
                //% "Name"
                placeholderText: qsTrId("light_pk_design_name")
                onAccepted: designSave.clicked()
            }
            SecondaryButton {
                id: designSave
                //% "Save"
                text: qsTrId("light_pk_design_save_button")
                iconName: "save"
                onClicked: {
                    var name = designName.text.trim()
                    if (!LightingVm.call("valid_name", { name: name })) {
                        //% "Names use letters, digits, . _ - (up to 40)."
                        Toasts.show(qsTrId("light_pk_bad_name"), "error")
                        return
                    }
                    Api.call("keyboard", "design", { action: "save", name: name, cells: tab.cells }, function () {
                        //% "Saved design '%1'"
                        Toasts.show(qsTrId("light_pk_design_saved").arg(name), "ok")
                        designName.text = ""
                        tab.loadDesigns(name)
                    })
                }
            }
        }
    }

    Card {
        Layout.fillWidth: true
        enabled: tab.usable
        //% "Whole keyboard"
        title: qsTrId("light_pk_whole")
        icon: "sun"

        SliderRow {
            Layout.fillWidth: true
            //% "Brightness"
            label: qsTrId("light_pk_brightness")
            from: 0
            to: 50
            stepSize: 1
            value: tab.brightness
            valueText: value === 0
                //% "Off"
                ? qsTrId("light_pk_level_off") : Math.round(value * 2) + " %"
            coalesceMs: 90
            onEdited: (v) => {
                tab.brightness = v
                Api.write("kb.level", "keyboard", "brightness", { level: v })
            }
        }
        SettingRow {
            Layout.fillWidth: true
            //% "Reset"
            label: qsTrId("light_pk_reset")
            SecondaryButton {
                //% "Reset to the stock look"
                text: qsTrId("light_pk_reset_button")
                onClicked: {
                    tab.commit(LightingVm.call("stock_cells"))
                    Api.call("keyboard", "restore", {}, function () {})
                }
            }
        }
        SettingRow {
            Layout.fillWidth: true
            //% "Save as a look"
            label: qsTrId("light_pk_look")
            //% "Appears under Looks; it remembers these key colours."
            sub: qsTrId("light_pk_look_sub")
            LightNameField {
                id: lookName
                Layout.preferredWidth: 180
                //% "Name"
                placeholderText: qsTrId("light_pk_look_name")
                onAccepted: lookSave.clicked()
            }
            SecondaryButton {
                id: lookSave
                //% "Save"
                text: qsTrId("light_pk_look_save")
                iconName: "save"
                onClicked: {
                    var name = lookName.text.trim()
                    if (!LightingVm.call("valid_name", { name: name })) {
                        Toasts.show(qsTrId("light_pk_bad_name"), "error")
                        return
                    }
                    var look = { keyboard: { mode: "perkey", brightness: tab.brightness > 0 ? tab.brightness : 25, cells: tab.cells }, rear: null }
                    Api.call("look", "save", { name: name, look: look }, function () {
                        //% "Saved look '%1'"
                        Toasts.show(qsTrId("light_pk_look_saved").arg(name), "ok")
                        lookName.text = ""
                        LightingVm.refreshLooks()
                    })
                }
            }
        }
    }

    ColorPickerPopup {
        id: picker
        parent: Overlay.overlay
        anchors.centerIn: Overlay.overlay
        onPicked: (hex) => {
            if (tab.pickTarget === "from") tab.gradFrom = hex
            else if (tab.pickTarget === "to") tab.gradTo = hex
            else tab.setColour(hex, true)
        }
    }
}
