// The Performance page's gateway to its Rust view-model (crates/ts-ui-model/src/vm/performance.rs)
// and the words the page shares between its tabs (power mode names, the Smart auto setting labels).
// Everything that decides something lives in Rust; this file only calls it and translates names.
pragma Singleton
import QtQuick
import org.truesense.app

QtObject {
    /// The answer of a view-model verb, or `fallback` when it refused (a refusal is a text, never
    /// something to show: the page decides what to say).
    function val(verb, args, fallback) {
        var r
        try { r = JSON.parse(Bus.vm("performance", verb, JSON.stringify(args))) } catch (e) { return fallback }
        return r.ok ? r.result : fallback
    }

    /// The refusal text of a verb ("" when it worked): for the one place that shows it, a payload
    /// the daemon would refuse.
    function problem(verb, args) {
        var r
        try { r = JSON.parse(Bus.vm("performance", verb, JSON.stringify(args))) } catch (e) { return String(e) }
        return r.ok ? "" : String(r.error)
    }

    readonly property var modeNames: ["eco", "quiet", "balanced", "performance", "turbo"]

    function modeText(mode) {
        switch (mode) {
        //% "Eco"
        case "eco": return qsTrId("perf_mode_eco")
        //% "Quiet"
        case "quiet": return qsTrId("perf_mode_quiet")
        //% "Balanced"
        case "balanced": return qsTrId("perf_mode_balanced")
        //% "Performance"
        case "performance": return qsTrId("perf_mode_performance")
        //% "Turbo"
        case "turbo": return qsTrId("perf_mode_turbo")
        default: return mode === undefined || mode === null ? Fmt.unknown : String(mode)
        }
    }

    function modeIcon(mode) {
        return mode === "eco" ? "eco" : mode === "quiet" ? "quiet" : mode === "balanced" ? "balanced"
            : mode === "performance" ? "performance" : "turbo"
    }

    /// [{value, text}] of the five modes for a drop-down.
    function modeOptions() {
        return modeNames.map(function (m) { return { value: m, text: modeText(m) } })
    }

    function activityText(activity) {
        switch (activity) {
        //% "idle"
        case "idle": return qsTrId("perf_act_idle")
        //% "everyday use"
        case "browsing": return qsTrId("perf_act_browsing")
        //% "heavy work"
        case "heavy": return qsTrId("perf_act_heavy")
        //% "gaming"
        case "gaming": return qsTrId("perf_act_gaming")
        //% "too hot"
        case "hot": return qsTrId("perf_act_hot")
        default: return activity === undefined || activity === null ? Fmt.unknown : String(activity)
        }
    }

    /// The label of a mode setting of a Smart auto profile.
    function modeSettingLabel(key) {
        switch (key) {
        //% "On power, idle"
        case "ac_idle": return qsTrId("perf_set_ac_idle")
        //% "On power, everyday use"
        case "ac_browsing": return qsTrId("perf_set_ac_browsing")
        //% "On power, gaming or heavy work"
        case "ac_busy": return qsTrId("perf_set_ac_busy")
        //% "On battery, idle"
        case "battery_idle": return qsTrId("perf_set_battery_idle")
        //% "On battery, everyday use"
        case "battery_browsing": return qsTrId("perf_set_battery_browsing")
        //% "On battery, gaming or heavy work"
        case "battery_busy": return qsTrId("perf_set_battery_busy")
        //% "On battery, too hot"
        case "battery_hot": return qsTrId("perf_set_battery_hot")
        default: return key
        }
    }

    /// The label of a numeric setting (the unit is shown by the slider).
    function numberSettingLabel(key) {
        switch (key) {
        //% "Heavy load for this long before Turbo"
        case "turbo_after_s": return qsTrId("perf_set_turbo_after_s")
        //% "No Turbo above this CPU temperature"
        case "turbo_max_cpu": return qsTrId("perf_set_turbo_max_cpu")
        //% "No Turbo above this GPU temperature"
        case "turbo_max_gpu": return qsTrId("perf_set_turbo_max_gpu")
        //% "After overheating, wait before Turbo again"
        case "backoff_hold_s": return qsTrId("perf_set_backoff_hold_s")
        //% "Idle for this long before the idle mode"
        case "idle_after_s": return qsTrId("perf_set_idle_after_s")
        //% "Below this battery level, force Eco"
        case "battery_low_pct": return qsTrId("perf_set_battery_low_pct")
        //% "Wait before moving to a faster mode"
        case "up_dwell_s": return qsTrId("perf_set_up_dwell_s")
        //% "Wait before moving to a slower mode"
        case "down_dwell_s": return qsTrId("perf_set_down_dwell_s")
        //% "Stay in the gaming or heavy mode this long after the load drops"
        case "busy_hold_s": return qsTrId("perf_set_busy_hold_s")
        //% "At least this long between automatic mode changes"
        case "min_change_s": return qsTrId("perf_set_min_change_s")
        //% "Enter Turbo only this far below its temperature limits"
        case "turbo_margin_c": return qsTrId("perf_set_turbo_margin_c")
        //% "After Turbo reaches its limit, wait before trying it again"
        case "turbo_retry_s": return qsTrId("perf_set_turbo_retry_s")
        default: return key
        }
    }

    /// The unit a numeric setting is measured in, from its key.
    function numberUnit(key) {
        if (key === "turbo_max_cpu" || key === "turbo_max_gpu" || key === "turbo_margin_c") return "°C"
        if (key === "battery_low_pct") return "%"
        return "s"
    }
}
