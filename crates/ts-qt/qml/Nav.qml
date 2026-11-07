// Which page is showing, and the registry of pages (qml/pages/registry.json, loaded by Main.qml).
// Page agents never edit this file: a new page is a line in registry.json and a <Name>Page.qml.
pragma Singleton
import QtQuick

QtObject {
    id: nav

    /// [{id, group, icon, titleId, title, qml, aliases, shortcut}] in sidebar order.
    property var pages: []
    /// [{id, titleId, title}] in sidebar order.
    property var groups: []
    property string notFoundQml: "NotFoundPage"
    /// The id of the page showing ("home", "cooling", ...), or "" before the first navigation.
    property string current: ""
    /// The text the person typed for a page that does not exist (shown by the not-found page).
    property string missing: ""
    /// Set by the shell when it shows the page title in its own header (narrow windows), so pages
    /// do not repeat it.
    property bool headerShowsTitle: false
    /// A request to bring the window to the front (second launch, tray, PREDATOR key).
    signal raiseRequested()

    function load(text) {
        var o = JSON.parse(text)
        pages = o.pages
        groups = o.groups
        notFoundQml = o.notFound.qml
    }

    function find(id) {
        for (var i = 0; i < pages.length; ++i)
            if (pages[i].id === id) return pages[i]
        return null
    }

    /// The pages of one group, in order.
    function inGroup(groupId) {
        return pages.filter(function (p) { return p.group === groupId })
    }

    /// Goes to a page by its id. An unknown id shows the not-found page.
    function go(id) {
        if (find(id) !== null) { missing = ""; current = id }
        else { missing = id; current = "__notfound" }
    }

    /// The component file of the current page ("HomePage"), "" before the first navigation.
    readonly property string currentQml: {
        if (current === "") return ""
        var p = find(current)
        return p === null ? notFoundQml : p.qml
    }

    readonly property var currentPage: find(current)

    /// The page title for the sidebar and header, translated now (and again when the language changes).
    function titleOf(page) { return qsTrId(page.titleId) }
    function groupTitle(group) { return qsTrId(group.titleId) }
}
