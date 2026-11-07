// What this machine can do (the capability model of ts-ui-model). A control the laptop lacks stays
// on the page, greyed, with one sentence (CapabilityNote); it is never missing and never an error.
pragma Singleton
import QtQuick

QtObject {
    id: caps

    /// {"rear-light": {"state": "unavailable", "why": "..."}, ...}; {} until the first refresh.
    property var model: ({})

    /// "available", "unavailable", "needs-daemon" or "unknown".
    function state(id) {
        var e = model[id]
        return e === undefined ? "unknown" : e.state
    }

    /// Whether a control for `id` should be enabled (unknown counts: the command is the authority).
    function usable(id) {
        var s = state(id)
        return s === "available" || s === "unknown"
    }

    /// The machine's own explanation, if it gave one ("" otherwise).
    function why(id) {
        var e = model[id]
        return (e === undefined || e.why === undefined) ? "" : e.why
    }

    function load(text) {
        try { model = JSON.parse(text) } catch (e) { model = ({}) }
    }
}
