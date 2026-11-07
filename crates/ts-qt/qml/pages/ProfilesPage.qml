// Profiles and favourites: the whole-setup profiles (list, apply, save the current setup, delete)
// and the Favourites list (reorder, remove, apply). Which rows exist, what a profile changes, which
// moves are possible: ts-ui-model/src/vm/profiles.rs. The star toggle that adds a favourite lives
// on the pages that own the starred things (looks, curves, Smart profiles, this page's profiles).
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import org.truesense.app

PageFrame {
    id: page

    title: Nav.currentPage ? Nav.titleOf(Nav.currentPage) : ""

    // ---- state ---------------------------------------------------------------------------------
    property var rawProfiles: []
    property string currentName: ""
    property var favRaw: []
    property string expanded: ""            // the profile whose details are open
    property var partsOf: ({})              // name -> vm `parts` answer
    property bool savingBusy: false

    readonly property var profiles: Vm.call("profiles", "profiles", { list: rawProfiles, current: currentName })
        || ({ count: 0, empty: true, rows: [] })
    readonly property var favs: Vm.call("profiles", "favourites", { list: favRaw })
        || ({ count: 0, empty: true, full: false, items: [] })
    readonly property bool narrow: width < 560

    function quiet(err) { /* a failed background read leaves the list as it was */ }

    function loadProfiles() {
        Api.call("profile", "list", {}, function (r) { rawProfiles = Array.isArray(r) ? r : [] }, quiet)
        Api.call("profile", "current", {}, function (r) { currentName = (r && r.name) ? r.name : "" }, quiet)
    }
    function loadFavourites() {
        Api.call("favourite", "list", {}, function (r) { favRaw = Array.isArray(r) ? r : [] }, quiet)
    }
    Component.onCompleted: { loadProfiles(); loadFavourites() }

    // ---- profile actions -----------------------------------------------------------------------
    function messageOr(r, fallback) { return (typeof r === "string" && r !== "") ? r : fallback }

    function applyProfile(name) {
        Api.call("profile", "apply", { name: name }, function (r) {
            //% "Profile '%1' applied"
            Toasts.ok(messageOr(r, qsTrId("profiles_toast_applied").arg(name)))
            loadProfiles()
        })
    }
    function nextProfile() {
        Api.call("profile", "next", {}, function (r) {
            //% "Next profile applied"
            Toasts.ok(messageOr(r, qsTrId("profiles_toast_next")))
            loadProfiles()
        })
    }
    function showParts(name) {
        if (expanded === name) { expanded = ""; return }
        expanded = name
        if (partsOf[name] !== undefined) return
        Api.call("profile", "show", { name: name }, function (r) {
            var copy = Object.assign({}, partsOf)
            copy[name] = Vm.call("profiles", "parts", { profile: r }) || ({ entries: [], empty: true })
            partsOf = copy
        })
    }
    function askDelete(name) {
        confirm.ask({
            //% "Delete profile"
            title: qsTrId("profiles_delete_title"),
            //% "Delete the profile '%1'? Favourites that point to it are kept and shown as missing."
            text: qsTrId("profiles_delete_text").arg(name),
            //% "Delete"
            confirmText: qsTrId("profiles_delete_confirm"),
            danger: true
        }, function () {
            Api.call("profile", "delete", { name: name }, function () {
                //% "Deleted profile '%1'"
                Toasts.ok(qsTrId("profiles_toast_deleted").arg(name))
                if (expanded === name) expanded = ""
                loadProfiles()
                loadFavourites()
            })
        })
    }
    function saveCurrent() {
        var check = Vm.call("profiles", "name_check", { name: nameField.text })
        if (!check || !check.ok) {
            //% "Give the profile a name: letters, digits, . _ - (up to 40)."
            Toasts.error(qsTrId("profiles_name_invalid"))
            nameField.forceActiveFocus()
            return
        }
        var description = descField.text.trim()
        savingBusy = true
        var args = { name: check.name }
        //% "saved from the window"
        args.description = description !== "" ? description : qsTrId("profiles_default_description")
        Api.call("profile", "save", args, function () {
            savingBusy = false
            //% "Saved profile '%1' (the current power, fans, rear light, battery, screen, boot sound and audio)"
            Toasts.ok(qsTrId("profiles_toast_saved").arg(check.name))
            nameField.text = ""
            descField.text = ""
            loadProfiles()
        }, function (err) {
            savingBusy = false
            Toasts.error(err && err.message ? err.message : "")
        })
    }

    // ---- favourite actions ---------------------------------------------------------------------
    function moveFavourite(position, delta) {
        var step = Vm.call("profiles", "move", { position: position, delta: delta, count: favs.count })
        if (!step) return
        Api.call("favourite", "move", step, function (r) { favRaw = Array.isArray(r) ? r : favRaw })
    }
    function removeFavourite(item) {
        Api.call("favourite", "remove", { kind: item.kind, name: item.name }, function () { loadFavourites() })
    }
    function applyFavourite(item) {
        Api.call("favourite", "apply", { position: item.position }, function (r) {
            Toasts.ok(messageOr(r, item.name))
        })
    }

    function kindName(kind) {
        switch (kind) {
        //% "Look"
        case "look": return qsTrId("profiles_kind_look")
        //% "Fan curve"
        case "fan_curve": return qsTrId("profiles_kind_curve")
        //% "Smart profile"
        case "smart_profile": return qsTrId("profiles_kind_smart")
        //% "Profile"
        case "profile": return qsTrId("profiles_kind_profile")
        //% "Theme"
        case "theme": return qsTrId("profiles_kind_theme")
        }
        return ""
    }

    function partLabel(id) {
        switch (id) {
        //% "Power mode"
        case "power_mode": return qsTrId("profiles_part_power_mode")
        //% "Mode control"
        case "power_auto": return qsTrId("profiles_part_power_auto")
        //% "Fans"
        case "fan_mode": return qsTrId("profiles_part_fan_mode")
        //% "Fan speed (CPU / GPU, %)"
        case "fan_duty": return qsTrId("profiles_part_fan_duty")
        //% "Fan curve"
        case "fan_curve": return qsTrId("profiles_part_fan_curve")
        //% "Keyboard effect"
        case "kb_effect": return qsTrId("profiles_part_kb_effect")
        //% "Key set"
        case "kb_profile": return qsTrId("profiles_part_kb_profile")
        //% "Keyboard backlight"
        case "kb_backlight": return qsTrId("profiles_part_kb_backlight")
        //% "Keyboard light off after 30 s"
        case "kb_timeout": return qsTrId("profiles_part_kb_timeout")
        //% "Rear light effect"
        case "rear_effect": return qsTrId("profiles_part_rear_effect")
        //% "Rear light"
        case "rear_state": return qsTrId("profiles_part_rear_state")
        //% "80 % battery limit"
        case "battery_limit": return qsTrId("profiles_part_battery_limit")
        //% "USB charging while off"
        case "battery_usb": return qsTrId("profiles_part_battery_usb")
        //% "Screen overdrive"
        case "screen_overdrive": return qsTrId("profiles_part_overdrive")
        //% "Boot sound"
        case "boot_sound": return qsTrId("profiles_part_boot_sound")
        //% "Audio output"
        case "audio_output": return qsTrId("profiles_part_audio_output")
        //% "Headset noise reduction"
        case "audio_voice_headset": return qsTrId("profiles_part_voice_headset")
        //% "Built-in noise reduction"
        case "audio_voice_array": return qsTrId("profiles_part_voice_array")
        }
        return id
    }
    function partText(entry) {
        //% "on"
        if (entry.on === true) return partLabel(entry.label) + ": " + qsTrId("profiles_on")
        //% "off"
        if (entry.on === false) return partLabel(entry.label) + ": " + qsTrId("profiles_off")
        return partLabel(entry.label) + ": " + entry.value
    }

    ConfirmDialog { id: confirm }

    // A one-line field in the look of the app.
    component Field: TextField {
        Layout.fillWidth: true
        implicitHeight: Theme.controlHeight
        color: Theme.text
        placeholderTextColor: Theme.muted
        font.pixelSize: Theme.fontBody
        selectByMouse: true
        leftPadding: 12
        rightPadding: 12
        background: Rectangle {
            radius: Theme.radiusSmall + 1
            color: Theme.field
            border.width: 1
            border.color: parent.activeFocus ? Theme.focusRing : Theme.buttonLine
        }
    }

    HelpText {
        //% "A profile is a one-click bundle: it sets the power mode, fans, rear light, battery limit, screen overdrive, boot sound and audio in one go. It applies once (the power mode is then held like when you pick a tile). A profile changes only what it lists: anything it does not list stays as it is."
        text: qsTrId("profiles_intro")
    }

    // ---- profiles ------------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Profiles"
        title: qsTrId("profiles_title")
        icon: "profiles"
        //% "Last applied: %1"
        rightText: page.currentName !== "" ? qsTrId("profiles_last").arg(page.currentName) : ""

        HelpText {
            visible: page.profiles.empty
            //% "No profiles yet. Set things the way you like and save them below."
            text: qsTrId("profiles_none")
        }

        Repeater {
            model: page.profiles.rows
            delegate: Rectangle {
                id: row
                required property var modelData
                readonly property bool open: page.expanded === modelData.name
                Layout.fillWidth: true
                implicitHeight: rowCol.implicitHeight + 24
                radius: Theme.radius
                color: modelData.current ? Theme.accentSoft : Theme.tile
                border.width: 1
                border.color: modelData.current ? Theme.accentLine : Theme.tileLine

                ColumnLayout {
                    id: rowCol
                    anchors.fill: parent
                    anchors.margins: 12
                    spacing: 8

                    GridLayout {
                        Layout.fillWidth: true
                        columns: page.narrow ? 1 : 2
                        columnSpacing: Theme.spacing
                        rowSpacing: 8

                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 2
                            RowLayout {
                                spacing: 8
                                Text {
                                    text: row.modelData.name
                                    color: Theme.text
                                    font.pixelSize: Theme.fontSection
                                    font.bold: true
                                    elide: Text.ElideRight
                                }
                                Text {
                                    text: row.modelData.source === "yours"
                                        //% "yours"
                                        ? qsTrId("profiles_yours")
                                        //% "shipped"
                                        : qsTrId("profiles_shipped")
                                    color: Theme.muted
                                    font.pixelSize: Theme.fontSmall
                                }
                                RowLayout {
                                    visible: row.modelData.current
                                    spacing: 4
                                    Icon { name: "check"; size: 14; color: Theme.ok }
                                    Text {
                                        //% "last applied"
                                        text: qsTrId("profiles_current")
                                        color: Theme.ok
                                        font.pixelSize: Theme.fontSmall
                                    }
                                }
                                Item { Layout.fillWidth: true }
                            }
                            Text {
                                Layout.fillWidth: true
                                visible: row.modelData.description !== ""
                                text: row.modelData.description
                                color: Theme.stat
                                font.pixelSize: Theme.fontBody
                                wrapMode: Text.Wrap
                            }
                        }
                        RowLayout {
                            Layout.alignment: Qt.AlignVCenter | Qt.AlignRight
                            spacing: 8
                            SecondaryButton {
                                text: row.open
                                    //% "Hide details"
                                    ? qsTrId("profiles_hide")
                                    //% "Details"
                                    : qsTrId("profiles_details")
                                onClicked: page.showParts(row.modelData.name)
                            }
                            ListIconButton {
                                visible: row.modelData.deletable
                                iconName: "trash"
                                tone: "danger"
                                //% "Delete"
                                text: qsTrId("profiles_delete")
                                onClicked: page.askDelete(row.modelData.name)
                            }
                            PrimaryButton {
                                //% "Apply"
                                text: qsTrId("profiles_apply")
                                onClicked: page.applyProfile(row.modelData.name)
                            }
                        }
                    }

                    // What it changes (and only this).
                    Flow {
                        Layout.fillWidth: true
                        Layout.preferredHeight: childrenRect.height
                        visible: row.open && page.partsOf[row.modelData.name] !== undefined
                        spacing: 6
                        Repeater {
                            model: row.open && page.partsOf[row.modelData.name] ? page.partsOf[row.modelData.name].entries : []
                            delegate: Rectangle {
                                required property var modelData
                                implicitWidth: partText.implicitWidth + 20
                                implicitHeight: 28
                                radius: 14
                                color: Theme.card
                                border.color: Theme.cardLine
                                Text {
                                    id: partText
                                    anchors.centerIn: parent
                                    text: page.partText(parent.modelData)
                                    color: Theme.stat
                                    font.pixelSize: Theme.fontSmall + 1
                                }
                            }
                        }
                    }
                    HelpText {
                        visible: row.open && page.partsOf[row.modelData.name] !== undefined && page.partsOf[row.modelData.name].empty
                        //% "This profile does not change anything."
                        text: qsTrId("profiles_nothing")
                    }
                }
            }
        }

        RowLayout {
            Layout.fillWidth: true
            spacing: 8
            visible: !page.profiles.empty
            Item { Layout.fillWidth: true }
            SecondaryButton {
                //% "Apply next profile"
                text: qsTrId("profiles_next")
                onClicked: page.nextProfile()
            }
        }
    }

    // ---- save the current setup ----------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Save the current setup"
        title: qsTrId("profiles_save_title")
        icon: "save"

        HelpText {
            //% "Set everything the way you like first, then save: TrueSense records the current power mode, fans, rear light, battery, screen, boot sound and audio. Keyboard colours are saved under Lighting."
            text: qsTrId("profiles_save_help")
        }
        GridLayout {
            Layout.fillWidth: true
            columns: page.narrow ? 1 : 3
            columnSpacing: 8
            rowSpacing: 8
            Field {
                id: nameField
                maximumLength: 40
                //% "Name, e.g. my-gaming"
                placeholderText: qsTrId("profiles_name_placeholder")
                Accessible.name: qsTrId("profiles_name_placeholder")
                onAccepted: page.saveCurrent()
            }
            Field {
                id: descField
                maximumLength: 120
                //% "Description (optional)"
                placeholderText: qsTrId("profiles_desc_placeholder")
                Accessible.name: qsTrId("profiles_desc_placeholder")
                onAccepted: page.saveCurrent()
            }
            PrimaryButton {
                Layout.alignment: Qt.AlignRight
                busy: page.savingBusy
                //% "Save"
                text: qsTrId("profiles_save")
                onClicked: page.saveCurrent()
            }
        }
        HelpText {
            //% "Names use letters, digits, . _ - (up to 40)."
            text: qsTrId("profiles_name_help")
        }
    }

    // ---- favourites ----------------------------------------------------------------------------
    Card {
        Layout.fillWidth: true
        //% "Favourites"
        title: qsTrId("profiles_fav_title")
        icon: "star"
        rightText: page.favs.count + " / 24"

        RowLayout {
            Layout.fillWidth: true
            spacing: 12
            visible: page.favs.empty
            HelpText {
                //% "Nothing is starred yet. Use the star on a look, a fan curve, a Smart profile or a profile to keep it here for one-click use on Home and on the keys."
                text: qsTrId("profiles_fav_empty")
            }
        }

        Repeater {
            model: page.favs.items
            delegate: Rectangle {
                id: fav
                required property var modelData
                Layout.fillWidth: true
                implicitHeight: favRow.implicitHeight + 16
                radius: Theme.radius
                color: Theme.tile
                border.width: 1
                border.color: Theme.tileLine
                opacity: modelData.exists ? 1 : 0.9

                RowLayout {
                    id: favRow
                    anchors.fill: parent
                    anchors.margins: 8
                    anchors.leftMargin: 12
                    spacing: 8

                    Icon {
                        name: fav.modelData.exists ? fav.modelData.icon : "warn"
                        size: 20
                        color: fav.modelData.exists ? Theme.accent : Theme.warn
                    }
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 0
                        Text {
                            Layout.fillWidth: true
                            text: fav.modelData.name
                            color: fav.modelData.exists ? Theme.text : Theme.disabledText
                            font.pixelSize: Theme.fontBody
                            font.strikeout: !fav.modelData.exists
                            elide: Text.ElideRight
                        }
                        Text {
                            Layout.fillWidth: true
                            text: fav.modelData.exists
                                ? page.kindName(fav.modelData.kind)
                                //% "%1, no longer exists"
                                : qsTrId("profiles_fav_missing").arg(page.kindName(fav.modelData.kind))
                            color: fav.modelData.exists ? Theme.muted : Theme.warn
                            font.pixelSize: Theme.fontSmall
                            elide: Text.ElideRight
                        }
                    }
                    ListIconButton {
                        iconName: "chevron"
                        iconRotation: -90
                        enabled: fav.modelData.can_up
                        //% "Move up"
                        text: qsTrId("profiles_fav_up")
                        onClicked: page.moveFavourite(fav.modelData.position, -1)
                    }
                    ListIconButton {
                        iconName: "chevron"
                        iconRotation: 90
                        enabled: fav.modelData.can_down
                        //% "Move down"
                        text: qsTrId("profiles_fav_down")
                        onClicked: page.moveFavourite(fav.modelData.position, 1)
                    }
                    SecondaryButton {
                        visible: !page.narrow
                        enabled: fav.modelData.exists
                        //% "Apply"
                        text: qsTrId("profiles_fav_apply")
                        onClicked: page.applyFavourite(fav.modelData)
                    }
                    ListIconButton {
                        visible: page.narrow
                        enabled: fav.modelData.exists
                        iconName: "check"
                        text: qsTrId("profiles_fav_apply")
                        onClicked: page.applyFavourite(fav.modelData)
                    }
                    ListIconButton {
                        iconName: "close"
                        tone: "danger"
                        //% "Remove from favourites"
                        text: qsTrId("profiles_fav_remove")
                        onClicked: page.removeFavourite(fav.modelData)
                    }
                }
            }
        }
    }

    HelpText {
        //% "Smart auto profiles decide how the power mode follows load and heat. Fan curves are only the fan shape. Lighting looks are only lights. Key bindings are only the PREDATOR key. A profile here sets all of it once; it does not keep watching."
        text: qsTrId("profiles_differs")
    }
}
