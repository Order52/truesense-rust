//! What the Battery and screen page decides: how `battery info` becomes a gauge and a list of
//! values, whether a switch that was just flipped really took (the read-back), the state of
//! "charge to 100 % once", and the session-only charge history.
//!
//! Everything is a pure function of the JSON the page passes in, so the QML stays a layout. The
//! module returns codes (`charging`, `good`, `plug-in`) and already formatted numbers, never
//! sentences: the sentences are translated on the QML side.
//!
//! Verbs: `summary` (a `battery info` result), `readback` (`{requested, result}`), `full_state`
//! (a `battery full get` result or `null`), `history_push` and `history_series`.
#![forbid(unsafe_code)]

use serde_json::{Value, json};

use crate::format::{self, UNKNOWN};
use crate::json::{at, bool_at, f64_at, str_at};

/// Health at or above this is "good": batteries are rated for 80 % of their design capacity at the
/// end of their rated life, so more than that is a healthy cell.
pub(crate) const HEALTH_GOOD_PCT: f64 = 80.0;
/// Health at or above this (and below [`HEALTH_GOOD_PCT`]) is "fair"; under it the cell is "poor".
pub(crate) const HEALTH_FAIR_PCT: f64 = 60.0;
/// A charge at or below this is "low": the page says so next to the gauge.
pub(crate) const LOW_CHARGE_PCT: f64 = 15.0;

/// Seconds between two stored charge samples unless the caller says otherwise.
pub(crate) const DEFAULT_MIN_GAP_S: f64 = 30.0;
/// Samples kept: four hours at the default gap, a few kilobytes.
pub(crate) const DEFAULT_HISTORY_CAP: usize = 480;

/// Classifies the health percentage: `good`, `fair`, `poor`, or `unknown` when the kernel gave none.
pub(crate) fn health_class(pct: Option<f64>) -> &'static str {
    match pct.filter(|p| p.is_finite()) {
        None => "unknown",
        Some(p) if p >= HEALTH_GOOD_PCT => "good",
        Some(p) if p >= HEALTH_FAIR_PCT => "fair",
        Some(_) => "poor",
    }
}

/// The kernel's status text as a code: `charging`, `discharging`, `full`, `not-charging`, `unknown`.
pub(crate) fn status_code(raw: Option<&str>) -> &'static str {
    match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("charging") => "charging",
        Some("discharging") => "discharging",
        Some("full") => "full",
        Some("not charging" | "not-charging") => "not-charging",
        _ => "unknown",
    }
}

/// `15.4 V`.
pub fn volts(v: Option<f64>) -> String {
    v.filter(|n| n.is_finite())
        .map_or_else(|| UNKNOWN.to_owned(), |n| format!("{} V", (n * 100.0).round() / 100.0))
}

/// A current: `850 mA` under one ampere, `1.25 A` above. The kernel reports a magnitude, so the
/// sign comes from the status: `+` while charging, a real minus while discharging, none otherwise.
pub fn current(ma: Option<f64>, status: &str) -> String {
    let Some(ma) = ma.filter(|n| n.is_finite()) else { return UNKNOWN.to_owned() };
    let magnitude = ma.abs();
    let body = if magnitude >= 1000.0 {
        format!("{} A", (magnitude / 10.0).round() / 100.0)
    } else {
        format!("{} mA", magnitude.round())
    };
    match status {
        "charging" if magnitude > 0.0 => format!("+{body}"),
        "discharging" if magnitude > 0.0 => format!("\u{2212}{body}"),
        _ => body,
    }
}

/// The capacity row: full over design, in mAh when the battery reports charge and in Wh when it
/// reports energy (laptops differ: the PH16-71 reports energy). Only what is known is shown.
pub fn capacity(info: &Value) -> String {
    let mah = |path: &str| f64_at(info, path).map(|v| v.round());
    let wh = |path: &str| f64_at(info, path).map(|v| (v * 10.0).round() / 10.0);
    let pair = |full: Option<f64>, design: Option<f64>, unit: &str| match (full, design) {
        (Some(f), Some(d)) => Some(format!("{f} / {d} {unit}")),
        (Some(f), None) => Some(format!("{f} {unit}")),
        (None, Some(d)) => Some(format!("? / {d} {unit}")),
        (None, None) => None,
    };
    pair(mah("charge_full_mah"), mah("charge_full_design_mah"), "mAh")
        .or_else(|| pair(wh("energy_full_wh"), wh("energy_full_design_wh"), "Wh"))
        .unwrap_or_else(|| UNKNOWN.to_owned())
}

fn row(id: &str, text: String) -> Value {
    json!({ "id": id, "text": text, "known": text != UNKNOWN })
}

/// The firmware's charge limit as one code and text: `{"active": bool|null, "text": "80 %"}`.
fn limit_summary(info: &Value) -> Value {
    let active = bool_at(info, "charge_limit.limited");
    let end = f64_at(info, "charge_limit.end_threshold_pct");
    let text = match (active, end) {
        (Some(true), Some(end)) => format!("{} %", end.round()),
        (Some(true), None) => "80 %".to_owned(),
        (Some(false), _) => "100 %".to_owned(),
        (None, _) => UNKNOWN.to_owned(),
    };
    json!({ "active": active, "text": text })
}

/// The page model for a `battery info` result.
///
/// A laptop without a battery (`present: false` or `battery: null`) gives `present: false` and no
/// rows, so the page shows one sentence instead of a gauge of nothing.
pub fn summary(info: &Value) -> Value {
    let battery = at(info, "battery").filter(|b| b.is_object());
    let present = bool_at(info, "present").unwrap_or(battery.is_some()) && battery.is_some();
    let ac = match bool_at(info, "ac_online") {
        Some(true) => "online",
        Some(false) => "offline",
        None => "unknown",
    };
    let limit = limit_summary(info);
    let Some(b) = battery.filter(|_| present) else {
        return json!({ "present": false, "ac": ac, "rows": [], "limit": limit });
    };
    let percent = f64_at(b, "percent").map(|p| p.clamp(0.0, 100.0));
    let status = status_code(str_at(b, "status"));
    let health = f64_at(b, "health_pct");
    let rows = vec![
        row("voltage", volts(f64_at(b, "voltage_v"))),
        row("current", current(f64_at(b, "current_ma"), status)),
        row("power", format::watts(f64_at(b, "power_w"))),
        row("cycles", f64_at(b, "cycle_count").map_or_else(|| UNKNOWN.to_owned(), |c| format!("{}", c.round()))),
        row("health", health.map_or_else(|| UNKNOWN.to_owned(), |h| format!("{} %", h.round()))),
        row("capacity", capacity(b)),
        row("temperature", format::temp(f64_at(b, "temperature_c"))),
    ];
    json!({
        "present": true,
        "ac": ac,
        "status": status,
        "gauge": {
            "value": percent.unwrap_or(0.0),
            "known": percent.is_some(),
            "text": format::percent(percent),
            "low": percent.is_some_and(|p| p <= LOW_CHARGE_PCT) && status != "charging",
        },
        "health": { "pct": health, "class": health_class(health) },
        "rows": rows,
        "limit": limit,
    })
}

/// Did the machine do what the person asked? `requested` is what was sent (a boolean for a switch,
/// a number for the USB level) and `result` is what the command read back from the hardware.
///
/// `confirmed` when they agree, `differs` when the firmware answered something else (shown as a
/// warning with the value it kept), `unknown` when it gave no readable answer.
pub fn readback(args: &Value) -> Value {
    let requested = args.get("requested").filter(|v| !v.is_null());
    let result = args.get("result").filter(|v| !v.is_null());
    let state = match (requested, result) {
        (Some(want), Some(got)) if same(want, got) => "confirmed",
        (Some(_), Some(got)) if got.is_boolean() || got.is_number() => "differs",
        _ => "unknown",
    };
    json!({ "state": state, "value": result })
}

fn same(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() < f64::EPSILON,
        _ => a == b,
    }
}

/// The state of "charge to 100 % once" from a `battery full get` result (`null` when the daemon
/// is not running, which this feature needs).
///
/// `unavailable` (no daemon), `active` (charging to full this time), `plug-in` (the charger is not
/// connected), `limit-off` (the 80 % limit is off already) or `ready`. `can_start` and `can_cancel`
/// say which button works: starting needs the limit on and the charger in; cancelling needs it active.
pub fn full_state(full: &Value) -> Value {
    if !full.is_object() {
        return json!({ "state": "unavailable", "can_start": false, "can_cancel": false, "capacity": null });
    }
    let active = bool_at(full, "active").unwrap_or(false);
    let limit = bool_at(full, "limit").unwrap_or(false);
    let ac = bool_at(full, "ac").unwrap_or(false);
    let state = if active {
        "active"
    } else if !ac {
        "plug-in"
    } else if !limit {
        "limit-off"
    } else {
        "ready"
    };
    json!({
        "state": state,
        "can_start": !active && limit && ac,
        "can_cancel": active,
        "capacity": f64_at(full, "capacity").map(f64::round),
    })
}

/// The `[t, percent]` pairs of a stored history; entries of the wrong shape are skipped, so a
/// damaged value can never stop the page.
fn samples(history: Option<&Value>) -> Vec<(f64, f64)> {
    let Some(list) = history.and_then(Value::as_array) else { return Vec::new() };
    list.iter()
        .filter_map(|e| {
            let pair = e.as_array()?;
            let t = pair.first()?.as_f64().filter(|t| t.is_finite())?;
            let p = pair.get(1)?.as_f64().filter(|p| p.is_finite())?;
            Some((t, p))
        })
        .collect()
}

/// Adds one reading to the session history.
///
/// Arguments: `history` (`[[t, pct], ...]`), `t` (seconds), `pct`, and optionally `min_gap_s` and
/// `cap`. A reading without a usable `t` or `pct` is ignored; one that comes sooner than `min_gap_s`
/// after the last is ignored too (the chart needs a point every half minute, not every second); a
/// time that goes backwards (the clock was set) starts the history again rather than drawing a line
/// that runs backwards; the oldest points beyond `cap` are dropped.
pub fn history_push(args: &Value) -> Value {
    let mut history = samples(args.get("history"));
    let min_gap = f64_at(args, "min_gap_s").filter(|g| *g >= 0.0).unwrap_or(DEFAULT_MIN_GAP_S);
    let cap = f64_at(args, "cap")
        .filter(|c| *c >= 2.0)
        .map_or(DEFAULT_HISTORY_CAP, |c| c.min(100_000.0) as usize);
    let reading = f64_at(args, "t").zip(f64_at(args, "pct")).filter(|(_, p)| (0.0..=100.0).contains(p));
    if let Some((t, pct)) = reading {
        match history.last() {
            Some(&(last, _)) if t < last => history = vec![(t, pct)],
            Some(&(last, _)) if t - last < min_gap => {}
            _ => history.push((t, pct)),
        }
    }
    if history.len() > cap {
        history.drain(..history.len() - cap);
    }
    Value::Array(history.into_iter().map(|(t, p)| json!([t, p])).collect())
}

/// The history as a chart: `values` for a `Sparkline` (a gap longer than three steps is one `null`,
/// so a laptop that slept for an hour shows a break, not a straight line across it), the span in
/// seconds, and the lowest and highest charge seen. Arguments: `history` and optionally `step_s`.
pub fn history_series(args: &Value) -> Value {
    let history = samples(args.get("history"));
    let step = f64_at(args, "step_s").filter(|s| *s > 0.0).unwrap_or(DEFAULT_MIN_GAP_S);
    let mut values: Vec<Value> = Vec::with_capacity(history.len());
    let mut previous: Option<f64> = None;
    for &(t, p) in &history {
        if previous.is_some_and(|last| t - last > 3.0 * step) {
            values.push(Value::Null);
        }
        values.push(json!(p));
        previous = Some(t);
    }
    let span = match (history.first(), history.last()) {
        (Some(a), Some(b)) => b.0 - a.0,
        _ => 0.0,
    };
    let low = history.iter().map(|s| s.1).fold(None, |m: Option<f64>, p| Some(m.map_or(p, |m| m.min(p))));
    let high = history.iter().map(|s| s.1).fold(None, |m: Option<f64>, p| Some(m.map_or(p, |m| m.max(p))));
    json!({
        "values": values,
        "points": history.len(),
        "span_s": span,
        "span_text": format::duration(span),
        "low": low,
        "high": high,
    })
}

/// `summary`, `readback`, `full_state`, `history_push`, `history_series`.
///
/// # Errors
/// The text for an unknown verb.
pub fn call(verb: &str, args: &Value) -> Result<Value, String> {
    let args = super::settings::unwrap_value(args);
    match verb {
        "summary" => Ok(summary(args)),
        "readback" => Ok(readback(args)),
        "full_state" => Ok(full_state(args)),
        "history_push" => Ok(history_push(args)),
        "history_series" => Ok(history_series(args)),
        "health_class" => Ok(json!(health_class(f64_at(args, "pct")))),
        _ => Err(format!("unknown battery verb: {verb}")),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn info() -> Value {
        json!({
            "present": true, "ac_online": true,
            "battery": {
                "name": "BAT1", "present": true, "percent": 87.4, "status": "Charging",
                "voltage_v": 16.123, "current_ma": 1250.0, "power_w": 20.16,
                "charge_now_mah": null, "charge_full_mah": null, "charge_full_design_mah": null,
                "energy_now_wh": 74.0, "energy_full_wh": 81.26, "energy_full_design_wh": 90.0,
                "cycle_count": 112, "health_pct": 90.3, "temperature_c": 33.6
            },
            "others": [],
            "charge_limit": {"end_threshold_pct": 80, "limited": true, "firmware_limit_80": true}
        })
    }

    fn row_text(s: &Value, id: &str) -> String {
        s["rows"].as_array().unwrap().iter().find(|r| r["id"] == id).unwrap()["text"].as_str().unwrap().to_owned()
    }

    #[test]
    fn health_classes_have_their_boundaries() {
        assert_eq!(health_class(Some(100.0)), "good");
        assert_eq!(health_class(Some(112.0)), "good");
        assert_eq!(health_class(Some(80.0)), "good");
        assert_eq!(health_class(Some(79.9)), "fair");
        assert_eq!(health_class(Some(60.0)), "fair");
        assert_eq!(health_class(Some(59.9)), "poor");
        assert_eq!(health_class(Some(0.0)), "poor");
        assert_eq!(health_class(None), "unknown");
        assert_eq!(health_class(Some(f64::NAN)), "unknown");
    }

    #[test]
    fn status_text_becomes_a_code() {
        assert_eq!(status_code(Some("Charging")), "charging");
        assert_eq!(status_code(Some("Not charging")), "not-charging");
        assert_eq!(status_code(Some(" discharging ")), "discharging");
        assert_eq!(status_code(Some("Full")), "full");
        assert_eq!(status_code(Some("Unknown")), "unknown");
        assert_eq!(status_code(Some("")), "unknown");
        assert_eq!(status_code(None), "unknown");
    }

    #[test]
    fn numbers_are_formatted_for_reading() {
        assert_eq!(volts(Some(16.123)), "16.12 V");
        assert_eq!(volts(None), "–");
        assert_eq!(current(Some(850.0), "discharging"), "\u{2212}850 mA");
        assert_eq!(current(Some(1250.0), "charging"), "+1.25 A");
        assert_eq!(current(Some(1250.0), "full"), "1.25 A");
        assert_eq!(current(Some(0.0), "charging"), "0 mA");
        assert_eq!(current(Some(-900.0), "unknown"), "900 mA", "a magnitude, whatever sign the kernel used");
        assert_eq!(current(None, "charging"), "–");
        assert_eq!(current(Some(f64::INFINITY), "charging"), "–");
    }

    #[test]
    fn capacity_prefers_charge_then_energy_and_shows_what_is_known() {
        let b = &info()["battery"];
        assert_eq!(capacity(b), "81.3 / 90 Wh");
        assert_eq!(capacity(&json!({"charge_full_mah": 5301.4, "charge_full_design_mah": 5800.0})), "5301 / 5800 mAh");
        assert_eq!(capacity(&json!({"charge_full_mah": 5301.4})), "5301 mAh");
        assert_eq!(capacity(&json!({"energy_full_design_wh": 90.0})), "? / 90 Wh");
        assert_eq!(capacity(&json!({})), "–");
    }

    #[test]
    fn summary_of_a_charging_battery() {
        let s = summary(&info());
        assert_eq!(s["present"], true);
        assert_eq!(s["ac"], "online");
        assert_eq!(s["status"], "charging");
        assert_eq!(s["gauge"]["text"], "87 %");
        assert_eq!(s["gauge"]["low"], false);
        assert_eq!(s["health"]["class"], "good");
        assert_eq!(row_text(&s, "voltage"), "16.12 V");
        assert_eq!(row_text(&s, "current"), "+1.25 A");
        assert_eq!(row_text(&s, "power"), "20.2 W");
        assert_eq!(row_text(&s, "cycles"), "112");
        assert_eq!(row_text(&s, "health"), "90 %");
        assert_eq!(row_text(&s, "capacity"), "81.3 / 90 Wh");
        assert_eq!(row_text(&s, "temperature"), "34 °C");
        assert_eq!(s["limit"], json!({"active": true, "text": "80 %"}));
        let ids: Vec<_> = s["rows"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["voltage", "current", "power", "cycles", "health", "capacity", "temperature"]);
    }

    #[test]
    fn a_low_battery_is_flagged_only_while_it_is_not_charging() {
        let mut v = info();
        v["battery"]["percent"] = json!(9);
        v["battery"]["status"] = json!("Discharging");
        assert_eq!(summary(&v)["gauge"]["low"], true);
        v["battery"]["status"] = json!("Charging");
        assert_eq!(summary(&v)["gauge"]["low"], false);
    }

    #[test]
    fn missing_fields_are_dashes_never_zeros_or_failures() {
        let v = json!({"present": true, "ac_online": null, "battery": {"percent": null, "status": null}});
        let s = summary(&v);
        assert_eq!(s["present"], true);
        assert_eq!(s["ac"], "unknown");
        assert_eq!(s["gauge"]["text"], "–");
        assert_eq!(s["gauge"]["known"], false);
        assert_eq!(s["health"]["class"], "unknown");
        for r in s["rows"].as_array().unwrap() {
            assert_eq!(r["text"], "–", "{r}");
            assert_eq!(r["known"], false);
        }
        assert_eq!(s["limit"]["text"], "–");
    }

    #[test]
    fn no_battery_and_hostile_shapes_do_not_panic() {
        let none = summary(&json!({"present": false, "ac_online": true, "battery": null}));
        assert_eq!(none["present"], false);
        assert_eq!(none["ac"], "online");
        assert_eq!(none["rows"], json!([]));
        for hostile in [json!(null), json!([]), json!("x"), json!({"battery": 5}), json!({"battery": {"percent": "full"}})] {
            let s = summary(&hostile);
            assert!(s.is_object());
        }
        let clamped = summary(&json!({"battery": {"percent": 140}}));
        assert_eq!(clamped["gauge"]["value"], 100.0);
    }

    #[test]
    fn the_charge_limit_text_follows_the_threshold() {
        let v = json!({"charge_limit": {"limited": true, "end_threshold_pct": 60}});
        assert_eq!(limit_summary(&v)["text"], "60 %");
        let v = json!({"charge_limit": {"limited": false, "end_threshold_pct": 100}});
        assert_eq!(limit_summary(&v), json!({"active": false, "text": "100 %"}));
        assert_eq!(limit_summary(&json!({"charge_limit": {"limited": true}}))["text"], "80 %");
    }

    #[test]
    fn readback_compares_what_was_asked_with_what_the_hardware_says() {
        assert_eq!(readback(&json!({"requested": true, "result": true}))["state"], "confirmed");
        assert_eq!(readback(&json!({"requested": false, "result": false}))["state"], "confirmed");
        let d = readback(&json!({"requested": true, "result": false}));
        assert_eq!(d["state"], "differs");
        assert_eq!(d["value"], false);
        assert_eq!(readback(&json!({"requested": 20, "result": 20}))["state"], "confirmed");
        assert_eq!(readback(&json!({"requested": 20, "result": 20.0}))["state"], "confirmed");
        assert_eq!(readback(&json!({"requested": 20, "result": 30}))["state"], "differs");
        assert_eq!(readback(&json!({"requested": true, "result": null}))["state"], "unknown");
        assert_eq!(readback(&json!({"requested": true, "result": "yes"}))["state"], "unknown");
        assert_eq!(readback(&json!({"result": true}))["state"], "unknown");
        assert_eq!(readback(&json!(null))["state"], "unknown");
    }

    #[test]
    fn charge_to_full_follows_the_charger_the_limit_and_the_daemon() {
        let s = |v: Value| full_state(&v);
        assert_eq!(s(json!(null))["state"], "unavailable");
        assert_eq!(s(json!("ERROR"))["can_start"], false);
        let ready = s(json!({"active": false, "limit": true, "capacity": 71.6, "ac": true}));
        assert_eq!(ready["state"], "ready");
        assert_eq!(ready["can_start"], true);
        assert_eq!(ready["capacity"], 72.0);
        let unplugged = s(json!({"active": false, "limit": true, "capacity": 71, "ac": false}));
        assert_eq!(unplugged["state"], "plug-in");
        assert_eq!(unplugged["can_start"], false);
        let off = s(json!({"active": false, "limit": false, "capacity": 71, "ac": true}));
        assert_eq!(off["state"], "limit-off");
        assert_eq!(off["can_start"], false);
        let active = s(json!({"active": true, "limit": false, "capacity": 71, "ac": false}));
        assert_eq!(active["state"], "active");
        assert_eq!(active["can_cancel"], true, "it can always be cancelled, even unplugged");
        assert_eq!(active["can_start"], false);
    }

    #[test]
    fn history_adds_a_point_at_most_every_gap_and_caps_its_length() {
        let mut h = json!([]);
        for t in 0..100 {
            h = history_push(&json!({"history": h, "t": f64::from(t) * 10.0, "pct": 50.0, "min_gap_s": 30}));
        }
        // 0, 30, 60, ... 990: a point every 30 s for 990 s.
        assert_eq!(h.as_array().unwrap().len(), 34);
        assert_eq!(h[0], json!([0.0, 50.0]));
        assert_eq!(h[1], json!([30.0, 50.0]));
        let capped = history_push(&json!({"history": h, "t": 2000, "pct": 49, "cap": 10}));
        assert_eq!(capped.as_array().unwrap().len(), 10);
        assert_eq!(capped[9], json!([2000.0, 49.0]), "the newest survives");
    }

    #[test]
    fn history_ignores_unusable_readings_and_restarts_when_the_clock_goes_back() {
        let h = json!([[100.0, 80.0], [200.0, 79.0]]);
        for bad in [json!({"t": 300}), json!({"pct": 70}), json!({"t": 300, "pct": 101}), json!({"t": 300, "pct": -1}), json!({"t": null, "pct": null}), json!({"t": 300, "pct": "x"})] {
            let mut args = bad.clone();
            args["history"] = h.clone();
            assert_eq!(history_push(&args), h, "{bad}");
        }
        assert_eq!(history_push(&json!({"history": h, "t": 210, "pct": 70})), json!([[100.0, 80.0], [200.0, 79.0]]), "10 s is under the gap");
        assert_eq!(history_push(&json!({"history": h, "t": 230, "pct": 70})), json!([[100.0, 80.0], [200.0, 79.0], [230.0, 70.0]]));
        assert_eq!(history_push(&json!({"history": h, "t": 50, "pct": 70})), json!([[50.0, 70.0]]));
    }

    #[test]
    fn a_damaged_history_is_tolerated() {
        let junk = json!([[1], "x", null, [null, 5], [10, "a"], [20, 60]]);
        let out = history_push(&json!({"history": junk, "t": 100, "pct": 55}));
        let list = out.as_array().unwrap();
        assert!(list.contains(&json!([20.0, 60.0])));
        assert_eq!(list.last().unwrap(), &json!([100.0, 55.0]));
        assert_eq!(history_push(&json!({"history": 5, "t": 1, "pct": 1})), json!([[1.0, 1.0]]));
        assert_eq!(history_push(&json!(null)), json!([]));
    }

    #[test]
    fn the_series_breaks_at_long_gaps_and_reports_span_and_range() {
        let h = json!([[0, 80], [30, 78], [60, 75], [3600, 70], [3630, 90]]);
        let s = history_series(&json!({"history": h, "step_s": 30}));
        assert_eq!(s["values"], json!([80.0, 78.0, 75.0, null, 70.0, 90.0]));
        assert_eq!(s["points"], 5);
        assert_eq!(s["span_s"], 3630.0);
        assert_eq!(s["span_text"], "61 min");
        assert_eq!(s["low"], 70.0);
        assert_eq!(s["high"], 90.0);
        let empty = history_series(&json!({}));
        assert_eq!(empty["values"], json!([]));
        assert_eq!(empty["low"], Value::Null);
        assert_eq!(empty["span_s"], 0.0);
    }

    #[test]
    fn a_wrapped_null_reaches_full_state_as_no_daemon() {
        assert_eq!(call("full_state", &json!({"value": null})).unwrap()["state"], "unavailable");
        assert_eq!(call("full_state", &json!({"value": {"active": true}})).unwrap()["state"], "active");
    }

    #[test]
    fn dispatch_reaches_every_verb_and_refuses_unknown_ones() {
        assert_eq!(call("health_class", &json!({"pct": 70})), Ok(json!("fair")));
        assert!(call("summary", &info()).unwrap()["present"].as_bool().unwrap());
        assert_eq!(call("readback", &json!({"requested": true, "result": true})).unwrap()["state"], "confirmed");
        assert_eq!(call("full_state", &json!(null)).unwrap()["state"], "unavailable");
        assert!(call("history_push", &json!({})).unwrap().is_array());
        assert!(call("history_series", &json!({})).unwrap().is_object());
        assert!(call("nope", &json!({})).unwrap_err().contains("nope"));
    }
}
