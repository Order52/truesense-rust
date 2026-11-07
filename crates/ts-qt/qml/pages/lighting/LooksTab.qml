// Lighting > Looks: a look remembers the keyboard and the rear bar as they are now. Save the lights
// the way you like (Keyboard, Per-key, Rear bar), then keep them here; apply, copy, delete, and
// choose which one is put on at boot. Per-key sets are the bare key colours (no effect, no bar).
import QtQuick
import QtQuick.Layouts
import org.truesense.app

ColumnLayout {
    id: tab

    spacing: Theme.spacing + 2

    property string selected: "native"
    property bool builtin: true
    property string details: ""
    property var sets: []
    property string selectedSet: ""

    function reload(pick) {
        LightingVm.refreshLooks(function () {
            var names = LightingVm.looks
            if (pick !== undefined) selected = pick
            else if (names.indexOf(selected) < 0) selected = "native"
            show()
        })
        loadSets()
    }

    function show() {
        var name = selected
        Api.call("look", "show", { name: name }, function (look) {
            if (name !== selected) return
            builtin = look.builtin === true
            details = LightingVm.describeLook(look)
        }, function () { details = "" })
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

    function save() {
        var name = nameField.text.trim() !== "" ? nameField.text.trim() : selected
        if (_badName(name, "look")) return
        Api.call("look", "save", { name: name }, function () {
            //% "Saved look '%1'"
            Toasts.show(qsTrId("light_looks_saved").arg(name), "ok")
            nameField.text = ""
            reload(name)
        })
    }

    Component.onCompleted: reload()

    ConfirmDialog { id: confirm }

    HelpText {
        //% "A look remembers the keyboard and the rear bar as they are now. Set the lights the way you like under Keyboard, Per-key and Rear bar, then save them here."
        text: qsTrId("light_looks_help")
    }

    Card {
        Layout.fillWidth: true
        //% "Looks"
        title: qsTrId("light_looks_title")
        icon: "profiles"

        SelectRow {
            Layout.fillWidth: true
            //% "Look"
            label: qsTrId("light_looks_look")
            options: LightingVm.looks
            current: tab.selected
            onActivated: (name) => { tab.selected = name; tab.show() }
        }
        Text {
            Layout.fillWidth: true
            visible: text !== ""
            text: (tab.builtin
                //% "Built in and read-only. Copy it to make your own."
                ? qsTrId("light_looks_builtin") + "  " : "") + tab.details
            color: Theme.muted
            font.pixelSize: Theme.fontSmall + 1
            wrapMode: Text.Wrap
        }
        Flow {
            Layout.fillWidth: true
            spacing: 8
            PrimaryButton {
                //% "Apply"
                text: qsTrId("light_looks_apply")
                onClicked: Api.call("look", "apply", { name: tab.selected }, function () {
                    //% "Look '%1' applied"
                    Toasts.show(qsTrId("light_looks_applied").arg(tab.selected), "ok")
                })
            }
            SecondaryButton {
                //% "Copy to edit"
                text: qsTrId("light_looks_copy")
                onClicked: {
                    var n = nameField.text.trim()
                    if (n !== "" && tab._badName(n, "look")) return
                    var src = tab.selected
                    Api.call("look", "copy", n !== "" ? { name: src, new: n } : { name: src }, function (made) {
                        //% "Copied '%1' to '%2'. Change the lights, then Save to update it."
                        Toasts.show(qsTrId("light_looks_copied").arg(src).arg(made), "ok")
                        nameField.text = ""
                        tab.reload(typeof made === "string" ? made : undefined)
                    })
                }
            }
            AppButton {
                kind: "danger"
                //% "Delete"
                text: qsTrId("light_looks_delete")
                iconName: "trash"
                enabled: !tab.builtin
                onClicked: {
                    var name = tab.selected
                    confirm.ask({
                        //% "Delete look"
                        title: qsTrId("light_looks_delete_title"),
                        //% "Delete the look '%1'?"
                        text: qsTrId("light_looks_delete_text").arg(name),
                        confirmText: qsTrId("light_looks_delete"),
                        danger: true
                    }, function () {
                        Api.call("look", "delete", { name: name }, function () { tab.reload("native") })
                    })
                }
            }
        }
        SettingRow {
            Layout.fillWidth: true
            //% "Save the lights as"
            label: qsTrId("light_looks_save_as")
            LightNameField {
                id: nameField
                Layout.preferredWidth: 180
                //% "Name"
                placeholderText: qsTrId("light_looks_name")
                onAccepted: tab.save()
            }
            SecondaryButton {
                //% "Save"
                text: qsTrId("light_looks_save")
                iconName: "save"
                onClicked: tab.save()
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
                    //% "Lighting back to the default look"
                    Toasts.show(qsTrId("light_looks_reset_done"), "ok")
                    tab.reload("native")
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
        SelectRow {
            Layout.fillWidth: true
            enabled: tab.sets.length > 0
            //% "Key set"
            label: qsTrId("light_sets_set")
            options: tab.sets
            current: tab.selectedSet
            onActivated: (name) => tab.selectedSet = name
        }
        Flow {
            Layout.fillWidth: true
            spacing: 8
            PrimaryButton {
                //% "Apply"
                text: qsTrId("light_sets_apply")
                enabled: tab.selectedSet !== ""
                onClicked: Api.call("keyboard", "profile", { action: "load", name: tab.selectedSet }, function () {
                    //% "Key set '%1' applied"
                    Toasts.show(qsTrId("light_sets_applied").arg(tab.selectedSet), "ok")
                })
            }
            AppButton {
                kind: "danger"
                //% "Delete"
                text: qsTrId("light_sets_delete")
                iconName: "trash"
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
                Layout.preferredWidth: 180
                //% "Name"
                placeholderText: qsTrId("light_sets_name")
                onAccepted: setSave.clicked()
            }
            SecondaryButton {
                id: setSave
                //% "Save"
                text: qsTrId("light_sets_save")
                iconName: "save"
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
