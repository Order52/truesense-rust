// Lighting > Looks: a look remembers the keyboard and the rear bar as they are now. Save the lights
// the way you like (Keyboard, Per-key, Rear bar), then keep them here. Every look is a card with its
// own actions (apply, copy, update, delete); below: save the lights as a new look, the per-key sets
// (the bare key colours, no effect, no bar) and which look is put on at boot.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

ColumnLayout {
    id: tab
    objectName: "looksTab"

    spacing: Theme.spacing + 2

    /// The look applied last from this tab (its card is outlined).
    property string applied: ""
    property var sets: []
    property string selectedSet: ""

    function reload(pick) {
        lookGallery.forget()
        LightingVm.refreshLooks()
        loadSets()
    }

    function loadSets(pick) {
        Api.call("keyboard", "profile", { action: "list" }, function (names) {
            sets = Array.isArray(names) ? names : []
            selectedSet = pick !== undefined ? pick : (sets.indexOf(selectedSet) >= 0 ? selectedSet : (sets.length ? sets[0] : ""))
        }, function () {})
    }

    function _badName(name, kind) {
        if (LightingVm.call("valid_name", { name: name, kind: kind })) return false
        if (kind === "set")
            //% "Key set names use letters, digits, - and _ (up to 40)."
            Toasts.show(qsTrId("light_looks_bad_set_name"), "error")
        else
            //% "Names use letters, digits, . _ - (up to 40)."
            Toasts.show(qsTrId("light_looks_bad_name"), "error")
        return true
    }

    function applyLook(name) {
        Api.call("look", "apply", { name: name }, function () {
            tab.applied = name
            //% "Look '%1' applied"
            Toasts.show(qsTrId("light_looks_applied").arg(LightingVm.lookName(name)), "ok")
        })
    }

    function copyLook(name) {
        Api.call("look", "copy", { name: name }, function (made) {
            //% "Copied to '%1'. Set the lights the way you like, then update it with the save button on its card."
            Toasts.show(qsTrId("light_looks_copied2").arg(made), "ok")
            tab.reload()
        })
    }

    function save(name, done) {
        if (_badName(name, "look")) return
        Api.call("look", "save", { name: name }, function () {
            //% "Saved look '%1'"
            Toasts.show(qsTrId("light_looks_saved").arg(name), "ok")
            if (done) done()
            tab.reload()
        })
    }

    Component.onCompleted: {
        LightingVm.loadLayout()
        reload()
    }

    ConfirmDialog { id: confirm }

    HelpText {
        //% "A look remembers the keyboard and the rear bar as they are now. Set the lights the way you like under Keyboard, Per-key and Rear bar, then save them here."
        text: qsTrId("light_looks_help")
    }

    Card {
        Layout.fillWidth: true
        //% "Your looks"
        title: qsTrId("light_looks_pictures")
        icon: "profiles"

        LookGallery {
            id: lookGallery
            Layout.fillWidth: true
            names: LightingVm.looks
            selected: tab.applied
            onApply: (name) => tab.applyLook(name)
            onCopy: (name) => tab.copyLook(name)
            onUpdate: (name) => confirm.ask({
                //% "Update look"
                title: qsTrId("light_looks_update_title"),
                //% "Replace the look '%1' with the lights as they are now?"
                text: qsTrId("light_looks_update_text").arg(name),
                //% "Update"
                confirmText: qsTrId("light_looks_update_confirm")
            }, function () { tab.save(name) })
            onRemove: (name) => confirm.ask({
                //% "Delete look"
                title: qsTrId("light_looks_delete_title"),
                //% "Delete the look '%1'?"
                text: qsTrId("light_looks_delete_text").arg(name),
                confirmText: qsTrId("light_looks_delete"),
                danger: true
            }, function () {
                Api.call("look", "delete", { name: name }, function () { tab.reload() })
            })
        }
        HelpText {
            visible: LightingVm.looks.length <= 1
            //% "Only the default look so far. Save the lights below to add your own."
            text: qsTrId("light_looks_only_default")
        }
        HelpText {
            //% "A moving effect is drawn as one still frame."
            text: qsTrId("light_looks_still_note")
        }
    }

    Card {
        Layout.fillWidth: true
        //% "Save the lights"
        title: qsTrId("light_looks_save_title")
        icon: "save"

        SettingRow {
            Layout.fillWidth: true
            //% "Save the lights as they are now"
            label: qsTrId("light_looks_save_as2")
            //% "The keyboard and the rear bar, as a new look."
            sub: qsTrId("light_looks_save_sub")
            LightNameField {
                id: nameField
                Layout.preferredWidth: 200
                //% "Name of the look"
                placeholderText: qsTrId("light_looks_name2")
                onAccepted: saveButton.clicked()
            }
            SecondaryButton {
                id: saveButton
                //% "Save"
                text: qsTrId("light_looks_save")
                iconName: "save"
                enabled: nameField.text.trim() !== ""
                onClicked: tab.save(nameField.text.trim(), function () { nameField.text = "" })
            }
        }
        SettingRow {
            Layout.fillWidth: true
            //% "Default"
            label: qsTrId("light_looks_default")
            SecondaryButton {
                //% "Reset to the default look"
                text: qsTrId("light_looks_reset")
                onClicked: Api.call("look", "apply", { name: "native" }, function () {
                    tab.applied = "native"
                    //% "Lighting back to the default look"
                    Toasts.show(qsTrId("light_looks_reset_done"), "ok")
                })
            }
        }
    }

    Card {
        Layout.fillWidth: true
        //% "Per-key sets"
        title: qsTrId("light_sets_title")
        icon: "keyboard"

        HelpText {
            //% "Only the 128 key colours, no effect and no rear bar. Used by the command line and by the login setting."
            text: qsTrId("light_sets_help")
        }
        HelpText {
            visible: tab.sets.length === 0
            //% "No key sets saved yet."
            text: qsTrId("light_sets_none")
            color: Theme.stat
        }
        SettingRow {
            Layout.fillWidth: true
            visible: tab.sets.length > 0
            //% "Key set"
            label: qsTrId("light_sets_set")
            SelectRow {
                Layout.preferredWidth: 220
                options: tab.sets
                current: tab.selectedSet
                onActivated: (name) => tab.selectedSet = name
            }
            SecondaryButton {
                //% "Apply"
                text: qsTrId("light_sets_apply")
                iconName: "play"
                enabled: tab.selectedSet !== ""
                onClicked: Api.call("keyboard", "profile", { action: "load", name: tab.selectedSet }, function () {
                    //% "Key set '%1' applied"
                    Toasts.show(qsTrId("light_sets_applied").arg(tab.selectedSet), "ok")
                })
            }
            ListIconButton {
                iconName: "trash"
                tone: "danger"
                //% "Delete"
                text: qsTrId("light_sets_delete")
                enabled: tab.selectedSet !== ""
                onClicked: {
                    var name = tab.selectedSet
                    confirm.ask({
                        //% "Delete key set"
                        title: qsTrId("light_sets_delete_title"),
                        //% "Delete the key set '%1'?"
                        text: qsTrId("light_sets_delete_text").arg(name),
                        confirmText: qsTrId("light_sets_delete"),
                        danger: true
                    }, function () {
                        Api.call("keyboard", "profile", { action: "delete", name: name }, function () { tab.loadSets() })
                    })
                }
            }
        }
        SettingRow {
            Layout.fillWidth: true
            //% "Save the keys now as"
            label: qsTrId("light_sets_save_as")
            LightNameField {
                id: setField
                Layout.preferredWidth: 200
                //% "Name"
                placeholderText: qsTrId("light_sets_name")
                onAccepted: setSave.clicked()
            }
            SecondaryButton {
                id: setSave
                //% "Save"
                text: qsTrId("light_sets_save")
                iconName: "save"
                enabled: setField.text.trim() !== ""
                onClicked: {
                    var name = setField.text.trim()
                    if (tab._badName(name, "set")) return
                    Api.call("keyboard", "cells", {}, function (c) {
                        Api.call("keyboard", "profile", { action: "save", name: name, cells: c.cells }, function () {
                            //% "Saved key set '%1'"
                            Toasts.show(qsTrId("light_sets_saved").arg(name), "ok")
                            setField.text = ""
                            tab.loadSets(name)
                        })
                    })
                }
            }
        }
    }

    BootLookCard {
        Layout.fillWidth: true
        showPicker: true
    }
}
