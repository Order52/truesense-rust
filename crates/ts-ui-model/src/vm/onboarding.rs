//! The first-run wizard: its steps, and what its health check says in plain terms.
//!
//! The wizard is shown once and remembered in `ui.json` (`onboarded`). It must never trap anybody:
//! Skip works on every step and counts as having seen it, no step is blocked by a failing health
//! check (a laptop without the driver still gets a window to read the explanation in), and nothing
//! here sends a command: the health check only reads three answers the page fetched (`doctor
//! run`, `service check`, `capabilities get`) and decides what each means and which fix, if any,
//! the person is offered.
//!
//! Verbs (pure):
//!
//! * `should_show {ui}`: whether the wizard opens, from the parsed `ui.json`;
//! * `start`, `next {step}`, `back {step}`, `skip {step}`: the state machine, each answering
//!   `{step, index, total, can_back, closed, remember}`;
//! * `health {doctor?, services?, capabilities?}`: `{rows, summary, all_ok, can_repair,
//!   system_command}`;
//! * `repair {steps}`: what a `service repair` answer means: `{ok, failed, needs_system}`.
//!
//! An input that is `null` or absent has not answered yet (state `pending`); one that is
//! `{"error": text}` failed (state `unknown`, with the text as the detail).
#![forbid(unsafe_code)]

use serde_json::{Value, json};

use crate::json::{at, i64_at, list_at, str_at};

/// The steps in order. `done` is the last page, not the end: leaving it closes the wizard.
pub(crate) const STEPS: [&str; 4] = ["welcome", "health", "look", "done"];

/// The command that repairs the user's own services; the window can run it by itself.
pub(crate) const USER_REPAIR: &str = "truesense service repair";

/// The key service's unit, the one the PREDATOR key and the lighting effects need.
const KEYS_UNIT: &str = "truesense-keys.service";
/// The root service's unit.
const DAEMON_UNIT: &str = "truesense.service";

fn position(step: &str) -> Option<usize> {
    STEPS.iter().position(|s| *s == step)
}

fn state(index: usize) -> Value {
    let index = index.min(STEPS.len() - 1);
    json!({
        "step": STEPS[index],
        "index": index,
        "total": STEPS.len(),
        "can_back": index > 0,
        "is_last": index == STEPS.len() - 1,
        "closed": false,
        "remember": false,
    })
}

/// The wizard is over. Both Finish and Skip remember it: a person who skipped has decided, and
/// asking again at every start would be the thing they skipped.
fn closed() -> Value {
    json!({"step": "closed", "index": STEPS.len(), "total": STEPS.len(), "can_back": false,
        "is_last": false, "closed": true, "remember": true})
}

/// Only an explicit `true` counts as seen: a missing, null, text or number value shows it again.
fn should_show(args: &Value) -> bool {
    at(args, "ui.onboarded").and_then(Value::as_bool) != Some(true)
}

fn step_of(args: &Value) -> usize {
    // An unknown step (a bug, or a state from a newer build) restarts at the welcome page rather
    // than failing: the wizard must always be able to continue or be skipped.
    str_at(args, "step").and_then(position).unwrap_or(0)
}

fn next(args: &Value) -> Value {
    let i = step_of(args);
    if i + 1 >= STEPS.len() { closed() } else { state(i + 1) }
}

fn back(args: &Value) -> Value {
    state(step_of(args).saturating_sub(1))
}

// ---- the health check ---------------------------------------------------------------------

/// How one input of the health check stands.
enum Input<'a> {
    /// Not answered yet.
    Pending,
    /// The command failed; the text says why.
    Failed(String),
    /// The answer.
    Ready(&'a Value),
}

fn input<'a>(args: &'a Value, key: &str) -> Input<'a> {
    match args.get(key) {
        None | Some(Value::Null) => Input::Pending,
        Some(v) => match str_at(v, "error") {
            Some(e) => Input::Failed(e.to_owned()),
            None => Input::Ready(v),
        },
    }
}

fn row(id: &str, state: &str, detail: Option<String>, fix: Option<&str>) -> Value {
    let fix = fix.filter(|f| !f.is_empty()).map(|f| json!({"command": f, "auto": f == USER_REPAIR}));
    json!({"id": id, "state": state, "detail": detail, "fix": fix})
}

fn unresolved(id: &str, input: &Input<'_>) -> Option<Value> {
    match input {
        Input::Pending => Some(row(id, "pending", None, None)),
        Input::Failed(e) => Some(row(id, "unknown", Some(e.clone()), None)),
        Input::Ready(_) => None,
    }
}

/// The service-check row for `unit`.
fn unit<'a>(rows: &'a [Value], unit: &str) -> Option<&'a Value> {
    rows.iter().find(|r| str_at(r, "unit") == Some(unit))
}

fn fix_of(r: &Value) -> Option<&str> {
    str_at(r, "fix").filter(|f| !f.is_empty())
}

fn module_row(services: &Input<'_>) -> Value {
    if let Some(r) = unresolved("module", services) {
        return r;
    }
    let Input::Ready(v) = services else { return row("module", "pending", None, None) };
    let rows = v.as_array().map_or(&[][..], Vec::as_slice);
    match rows.iter().find(|r| str_at(r, "unit").is_some_and(|u| u.starts_with("kernel module"))) {
        Some(r) if str_at(r, "active") == Some("present") => row("module", "ok", None, None),
        Some(r) => row("module", "fail", str_at(r, "active").map(str::to_owned), fix_of(r)),
        None => row("module", "unknown", None, None),
    }
}

fn daemon_row(services: &Input<'_>) -> Value {
    if let Some(r) = unresolved("daemon", services) {
        return r;
    }
    let Input::Ready(v) = services else { return row("daemon", "pending", None, None) };
    let rows = v.as_array().map_or(&[][..], Vec::as_slice);
    let service = unit(rows, DAEMON_UNIT);
    let socket = unit(rows, "control socket");
    let (Some(service), Some(socket)) = (service, socket) else {
        return row("daemon", "unknown", None, None);
    };
    let running = str_at(service, "active") == Some("active");
    let answers = str_at(socket, "active") == Some("answers");
    match (running, answers) {
        (true, true) => row("daemon", "ok", None, None),
        // Running but silent: a stalled daemon, or one still starting.
        (true, false) => row("daemon", "warn", str_at(socket, "active").map(str::to_owned), fix_of(socket)),
        (false, _) => row("daemon", "fail", str_at(service, "active").map(str::to_owned), fix_of(service)),
    }
}

fn keys_row(services: &Input<'_>) -> Value {
    if let Some(r) = unresolved("keys", services) {
        return r;
    }
    let Input::Ready(v) = services else { return row("keys", "pending", None, None) };
    let rows = v.as_array().map_or(&[][..], Vec::as_slice);
    match unit(rows, KEYS_UNIT) {
        Some(r) if str_at(r, "active") == Some("active") => row("keys", "ok", None, None),
        // Not a failure of the machine: the PREDATOR key and the lighting effects are off, and the
        // person's own repair brings them back without a password.
        Some(r) => row("keys", "warn", str_at(r, "active").map(str::to_owned), fix_of(r)),
        None => row("keys", "unknown", None, None),
    }
}

/// The user's other background parts (sound fixes, start-up look, update timer): one row, ok
/// when each is as it should be.
fn background_row(services: &Input<'_>) -> Value {
    if let Some(r) = unresolved("background", services) {
        return r;
    }
    let Input::Ready(v) = services else { return row("background", "pending", None, None) };
    let rows = v.as_array().map_or(&[][..], Vec::as_slice);
    let bad: Vec<&str> = rows
        .iter()
        .filter(|r| str_at(r, "scope") == Some("user") && str_at(r, "unit") != Some(KEYS_UNIT))
        .filter(|r| r.get("ok").and_then(Value::as_bool) != Some(true))
        .filter_map(|r| str_at(r, "unit"))
        .collect();
    if bad.is_empty() {
        row("background", "ok", None, None)
    } else {
        row("background", "warn", Some(bad.join(", ")), Some(USER_REPAIR))
    }
}

fn doctor_row(doctor: &Input<'_>) -> Value {
    if let Some(r) = unresolved("doctor", doctor) {
        return r;
    }
    let Input::Ready(v) = doctor else { return row("doctor", "pending", None, None) };
    let failures = i64_at(v, "failures").unwrap_or(0);
    let warnings = i64_at(v, "warnings").unwrap_or(0);
    let state = if failures > 0 {
        "fail"
    } else if warnings > 0 {
        "warn"
    } else {
        "ok"
    };
    // The first check of the worst grade: its own text is what a person can act on.
    let worst = if failures > 0 { "FAIL" } else { "WARN" };
    let first = list_at(v, "checks")
        .and_then(|c| c.iter().find(|c| str_at(c, "status") == Some(worst)))
        .filter(|_| state != "ok");
    let detail = first.map(|c| {
        let name = str_at(c, "check").unwrap_or("");
        match str_at(c, "detail").filter(|d| !d.is_empty()) {
            Some(d) => format!("{name}: {d}"),
            None => name.to_owned(),
        }
    });
    let mut r = row("doctor", state, detail, first.and_then(fix_of));
    if let Some(o) = r.as_object_mut() {
        o.insert("failures".to_owned(), json!(failures));
        o.insert("warnings".to_owned(), json!(warnings));
    }
    r
}

fn features_row(caps: &Input<'_>) -> Value {
    if let Some(r) = unresolved("features", caps) {
        return r;
    }
    let Input::Ready(v) = caps else { return row("features", "pending", None, None) };
    let items = list_at(v, "items").unwrap_or(&[]);
    let supported = items.iter().filter(|i| i.get("supported").and_then(Value::as_bool) == Some(true)).count();
    let state = match str_at(v, "model.tier") {
        Some("supported") => "ok",
        Some("unverified") => "warn",
        Some("unsupported") => "fail",
        _ => "unknown",
    };
    let mut r = row("features", state, str_at(v, "model.message").map(str::to_owned), None);
    if let Some(o) = r.as_object_mut() {
        o.insert("supported".to_owned(), json!(supported));
        o.insert("total".to_owned(), json!(items.len()));
        o.insert("simulated".to_owned(), json!(v.get("simulated").and_then(Value::as_bool) == Some(true)));
    }
    r
}

fn health(args: &Value) -> Value {
    let services = input(args, "services");
    let rows = vec![
        module_row(&services),
        daemon_row(&services),
        keys_row(&services),
        background_row(&services),
        doctor_row(&input(args, "doctor")),
        features_row(&input(args, "capabilities")),
    ];
    let count = |s: &str| rows.iter().filter(|r| r["state"] == json!(s)).count();
    let fixes: Vec<&str> = rows.iter().filter_map(|r| str_at(r, "fix.command")).collect();
    // The window cannot answer a password prompt: a repair that needs root is shown as the command
    // to run in a terminal, never run from here.
    let system_command = fixes.iter().find(|f| f.starts_with("sudo ")).map(|f| (*f).to_owned());
    json!({
        "summary": {"ok": count("ok"), "warn": count("warn"), "fail": count("fail"),
            "unknown": count("unknown"), "pending": count("pending")},
        "all_ok": rows.iter().all(|r| r["state"] == json!("ok")),
        "settled": count("pending") == 0,
        "can_repair": fixes.contains(&USER_REPAIR),
        "system_command": system_command,
        "rows": rows,
    })
}

/// What a `service repair` answer (`[{step, ok}]`, or the older `[[text, ok]]` pairs) means.
fn repair(args: &Value) -> Value {
    let steps = list_at(args, "steps").unwrap_or(&[]);
    let mut failed = Vec::new();
    let mut ok = 0;
    let mut needs_system = false;
    for s in steps {
        let (text, good) = match s {
            Value::Array(p) => (
                p.first().and_then(Value::as_str).unwrap_or(""),
                p.get(1).and_then(Value::as_bool).unwrap_or(false),
            ),
            other => (str_at(other, "step").unwrap_or(""), other.get("ok").and_then(Value::as_bool) == Some(true)),
        };
        if text.contains("sudo truesense service repair") {
            needs_system = true;
        }
        if good {
            ok += 1;
        } else {
            failed.push(text.to_owned());
        }
    }
    json!({"ok": ok, "failed": failed, "needs_system": needs_system})
}

/// The wizard's view-model entry point.
///
/// # Errors
/// The text for an unknown verb.
pub fn call(verb: &str, args: &Value) -> Result<Value, String> {
    match verb {
        "should_show" => Ok(json!(should_show(args))),
        "start" => Ok(state(0)),
        "next" => Ok(next(args)),
        "back" => Ok(back(args)),
        "skip" => Ok(closed()),
        "health" => Ok(health(args)),
        "repair" => Ok(repair(args)),
        _ => Err(format!("unknown onboarding verb: {verb}")),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn step(verb: &str, from: &str) -> Value {
        call(verb, &json!({"step": from})).unwrap()
    }

    fn unit_row(unit: &str, scope: &str, active: &str, ok: bool, fix: &str) -> Value {
        json!({"unit": unit, "scope": scope, "enabled": "enabled", "active": active, "ok": ok, "fix": fix})
    }

    /// A healthy `service check`.
    fn services_ok() -> Value {
        json!([
            unit_row("truesense.service", "system", "active", true, ""),
            unit_row("truesense-health.timer", "system", "active", true, ""),
            unit_row("truesense-keys.service", "user", "active", true, ""),
            unit_row("truesense-audio.service", "user", "active", true, ""),
            unit_row("kernel module (/dev/truesense)", "system", "present", true, ""),
            unit_row("control socket", "system", "answers", true, ""),
        ])
    }

    fn doctor(failures: i64, warnings: i64) -> Value {
        let mut checks = vec![json!({"check": "Model", "status": "OK", "detail": "PH16-71", "fix": ""})];
        if warnings > 0 {
            checks.push(json!({"check": "Secure Boot", "status": "WARN", "detail": "on", "fix": "disable it"}));
        }
        if failures > 0 {
            checks.push(json!({"check": "Kernel module", "status": "FAIL", "detail": "not loaded",
                "fix": "sudo truesense service repair"}));
        }
        json!({"ok": failures == 0, "failures": failures, "warnings": warnings, "checks": checks})
    }

    fn caps(tier: &str) -> Value {
        json!({"model": {"tier": tier, "message": "a message"}, "simulated": false, "firmware_answered": "12/14",
            "items": [{"id": "fan_control", "supported": true}, {"id": "dts", "supported": false, "why": "x"}]})
    }

    fn row_of<'a>(h: &'a Value, id: &str) -> &'a Value {
        h["rows"].as_array().unwrap().iter().find(|r| r["id"] == json!(id)).unwrap()
    }

    #[test]
    fn the_wizard_walks_forward_back_and_closes_after_the_last_step() {
        let s = call("start", &json!({})).unwrap();
        assert_eq!((s["step"].as_str(), s["index"].as_u64(), s["can_back"].as_bool()), (Some("welcome"), Some(0), Some(false)));
        let mut cur = "welcome".to_owned();
        let mut seen = vec![cur.clone()];
        loop {
            let n = step("next", &cur);
            if n["closed"] == json!(true) {
                assert_eq!(n["remember"], json!(true));
                break;
            }
            cur = n["step"].as_str().unwrap().to_owned();
            seen.push(cur.clone());
        }
        assert_eq!(seen, STEPS);
        assert_eq!(step("back", "health")["step"], json!("welcome"));
        assert_eq!(step("back", "welcome")["step"], json!("welcome"));
        assert_eq!(step("back", "done")["step"], json!("look"));
        assert_eq!(step("next", "done")["closed"], json!(true));
        assert_eq!(step("next", "look")["is_last"], json!(true));
    }

    #[test]
    fn skip_closes_from_every_step_and_is_remembered() {
        for s in STEPS.iter().copied().chain(["", "nonsense"]) {
            let r = step("skip", s);
            assert_eq!(r["closed"], json!(true), "{s}");
            assert_eq!(r["remember"], json!(true), "{s}");
        }
        assert_eq!(call("skip", &json!({})).unwrap()["closed"], json!(true));
    }

    #[test]
    fn an_unknown_step_restarts_instead_of_trapping_the_person() {
        assert_eq!(step("next", "nonsense")["step"], json!("health"));
        assert_eq!(step("back", "nonsense")["step"], json!("welcome"));
        assert_eq!(call("next", &json!({"step": 7})).unwrap()["step"], json!("health"));
    }

    #[test]
    fn it_shows_until_ui_json_says_it_was_seen() {
        let show = |ui: Value| call("should_show", &json!({"ui": ui})).unwrap();
        assert_eq!(show(json!({})), json!(true));
        assert_eq!(show(json!({"onboarded": false})), json!(true));
        assert_eq!(show(json!({"onboarded": true})), json!(false));
        for odd in [json!("yes"), json!(1), Value::Null, json!([true])] {
            assert_eq!(show(json!({"onboarded": odd})), json!(true), "{odd}");
        }
        assert_eq!(call("should_show", &json!({})).unwrap(), json!(true));
    }

    #[test]
    fn a_healthy_machine_is_all_ok_with_nothing_to_fix() {
        let h = call("health", &json!({"doctor": doctor(0, 0), "services": services_ok(), "capabilities": caps("supported")}))
            .unwrap();
        assert_eq!(h["all_ok"], json!(true));
        assert_eq!(h["settled"], json!(true));
        assert_eq!(h["can_repair"], json!(false));
        assert!(h["system_command"].is_null());
        assert_eq!(h["summary"], json!({"ok": 6, "warn": 0, "fail": 0, "unknown": 0, "pending": 0}));
        assert_eq!(row_of(&h, "features")["supported"], json!(1));
        assert_eq!(row_of(&h, "features")["total"], json!(2));
        let ids: Vec<&str> = h["rows"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["module", "daemon", "keys", "background", "doctor", "features"]);
    }

    #[test]
    fn a_stopped_key_service_is_fixed_by_the_users_own_repair_without_a_password() {
        let mut services = services_ok();
        services[2] = unit_row("truesense-keys.service", "user", "inactive", false, USER_REPAIR);
        let h = call("health", &json!({"doctor": doctor(0, 0), "services": services, "capabilities": caps("supported")}))
            .unwrap();
        let keys = row_of(&h, "keys");
        assert_eq!(keys["state"], json!("warn"));
        assert_eq!(keys["detail"], json!("inactive"));
        assert_eq!(keys["fix"], json!({"command": USER_REPAIR, "auto": true}));
        assert_eq!(h["can_repair"], json!(true));
        assert!(h["system_command"].is_null());
        assert_eq!(h["all_ok"], json!(false));
    }

    #[test]
    fn a_missing_module_or_stopped_daemon_needs_root_and_is_shown_as_a_command_never_run() {
        let mut services = services_ok();
        services[0] = unit_row("truesense.service", "system", "inactive", false, "sudo truesense service repair");
        services[4] = unit_row("kernel module (/dev/truesense)", "system", "missing", false, "sudo truesense service repair");
        services[5] = unit_row("control socket", "system", "missing", false, "sudo truesense service repair");
        let h = call("health", &json!({"doctor": doctor(1, 0), "services": services, "capabilities": caps("supported")}))
            .unwrap();
        assert_eq!(row_of(&h, "module")["state"], json!("fail"));
        assert_eq!(row_of(&h, "daemon")["state"], json!("fail"));
        assert_eq!(row_of(&h, "module")["fix"]["auto"], json!(false));
        assert_eq!(h["system_command"], json!("sudo truesense service repair"));
        assert_eq!(h["can_repair"], json!(false));
        assert_eq!(row_of(&h, "doctor")["state"], json!("fail"));
        assert_eq!(row_of(&h, "doctor")["detail"], json!("Kernel module: not loaded"));
        assert_eq!(row_of(&h, "doctor")["failures"], json!(1));
    }

    #[test]
    fn a_running_but_silent_daemon_is_a_warning() {
        let mut services = services_ok();
        services[5] = unit_row("control socket", "system", "silent", false, "sudo truesense service repair");
        let h = call("health", &json!({"services": services})).unwrap();
        assert_eq!(row_of(&h, "daemon")["state"], json!("warn"));
        assert_eq!(row_of(&h, "daemon")["detail"], json!("silent"));
    }

    #[test]
    fn other_background_parts_are_one_row_and_the_keys_unit_is_not_counted_twice() {
        let mut services = services_ok();
        services[3] = unit_row("truesense-audio.service", "user", "failed", false, USER_REPAIR);
        let h = call("health", &json!({"services": services})).unwrap();
        assert_eq!(row_of(&h, "background")["state"], json!("warn"));
        assert_eq!(row_of(&h, "background")["detail"], json!("truesense-audio.service"));
        assert_eq!(row_of(&h, "keys")["state"], json!("ok"));
        assert_eq!(h["can_repair"], json!(true));
    }

    #[test]
    fn the_model_tier_grades_the_features_row() {
        for (tier, state) in [("supported", "ok"), ("unverified", "warn"), ("unsupported", "fail"), ("odd", "unknown")] {
            let h = call("health", &json!({"capabilities": caps(tier)})).unwrap();
            assert_eq!(row_of(&h, "features")["state"], json!(state), "{tier}");
        }
        let h = call("health", &json!({"capabilities": caps("unverified")})).unwrap();
        assert_eq!(row_of(&h, "features")["detail"], json!("a message"));
    }

    #[test]
    fn doctor_warnings_without_failures_are_a_warning_with_the_checks_own_fix() {
        let h = call("health", &json!({"doctor": doctor(0, 1)})).unwrap();
        let d = row_of(&h, "doctor");
        assert_eq!(d["state"], json!("warn"));
        assert_eq!(d["detail"], json!("Secure Boot: on"));
        assert_eq!(d["fix"], json!({"command": "disable it", "auto": false}));
    }

    #[test]
    fn answers_not_in_yet_are_pending_and_failed_ones_are_unknown_with_the_reason() {
        let h = call("health", &json!({})).unwrap();
        assert_eq!(h["settled"], json!(false));
        assert_eq!(h["all_ok"], json!(false));
        assert_eq!(h["summary"]["pending"], json!(6));
        let h = call(
            "health",
            &json!({"services": {"error": "no daemon"}, "doctor": null, "capabilities": {"error": "timeout"}}),
        )
        .unwrap();
        assert_eq!(row_of(&h, "module")["state"], json!("unknown"));
        assert_eq!(row_of(&h, "module")["detail"], json!("no daemon"));
        assert_eq!(row_of(&h, "doctor")["state"], json!("pending"));
        assert_eq!(row_of(&h, "features")["detail"], json!("timeout"));
        assert_eq!(h["can_repair"], json!(false));
    }

    #[test]
    fn hostile_answers_never_panic_and_never_look_healthy() {
        let junk = [json!("x"), json!(5), json!([1, null, "a"]), json!({"items": 3}), json!(true), json!({})];
        for a in &junk {
            for b in &junk {
                for c in &junk {
                    let h = call("health", &json!({"services": a, "doctor": b, "capabilities": c})).unwrap();
                    assert_eq!(h["all_ok"], json!(false));
                    assert_eq!(h["rows"].as_array().unwrap().len(), 6);
                }
            }
        }
    }

    #[test]
    fn a_repair_answer_is_read_in_both_shapes_and_flags_the_part_that_needs_root() {
        let r = call(
            "repair",
            &json!({"steps": [{"step": "started truesense-keys.service", "ok": true},
                {"step": "the system part needs sudo truesense service repair", "ok": false}]}),
        )
        .unwrap();
        assert_eq!(r["ok"], json!(1));
        assert_eq!(r["needs_system"], json!(true));
        assert_eq!(r["failed"], json!(["the system part needs sudo truesense service repair"]));
        let old = call("repair", &json!({"steps": [["did it", true], ["broke", false]]})).unwrap();
        assert_eq!(old, json!({"ok": 1, "failed": ["broke"], "needs_system": false}));
        assert_eq!(call("repair", &json!({})).unwrap(), json!({"ok": 0, "failed": [], "needs_system": false}));
        assert_eq!(call("repair", &json!({"steps": [1, null, "x"]})).unwrap()["failed"], json!(["", "", ""]));
    }

    #[test]
    fn unknown_verbs_are_errors() {
        assert!(call("nope", &json!({})).unwrap_err().contains("nope"));
    }
}
