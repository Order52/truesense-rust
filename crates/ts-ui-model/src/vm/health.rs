//! What the Health page decides: the doctor report as a list with the worst first and one overall
//! level, the service rows, the self-test output as rows with a tally, the daemon log as lines
//! with a severity, and the configuration check.
//!
//! Output is data, not sentences (the page translates), except where the text *is* the data: the
//! doctor's `detail` and `fix`, a service's `fix`, the log and the self-test lines come from the
//! program and are shown as they are.
//!
//! Verbs: `doctor`, `services`, `repair`, `selftest`, `log_tail`, `config`, `capabilities`.
#![forbid(unsafe_code)]

use serde_json::{Value, json};

use crate::json::{bool_at, f64_at, list_at, str_at};

/// Most lines of the daemon log the page keeps (the command already returns about sixty).
pub const LOG_MAX_LINES: usize = 200;
/// Longest log or self-test line kept, in characters; a runaway line must not freeze the layout.
pub const LINE_MAX_CHARS: usize = 600;
/// Most self-test lines parsed.
pub const SELFTEST_MAX_LINES: usize = 500;

fn clip(line: &str) -> String {
    let line = line.trim_end();
    if line.chars().count() <= LINE_MAX_CHARS {
        return line.to_owned();
    }
    let mut out: String = line.chars().take(LINE_MAX_CHARS).collect();
    out.push('\u{2026}');
    out
}

/// A doctor status as the badge the page draws: `ok`, `info`, `warn`, `fail`. Anything unknown is
/// `info` (a newer doctor may add a status; it must not be shown as a pass).
pub fn badge(status: &str) -> &'static str {
    match status {
        "OK" => "ok",
        "WARN" => "warn",
        "FAIL" => "fail",
        _ => "info",
    }
}

fn severity(badge: &str) -> u8 {
    match badge {
        "fail" => 0,
        "warn" => 1,
        "info" => 2,
        _ => 3,
    }
}

/// The overall level of a doctor report: `fail` when anything failed, `warn` when anything warns,
/// otherwise `ok`.
pub fn level(failures: usize, warnings: usize) -> &'static str {
    if failures > 0 {
        "fail"
    } else if warnings > 0 {
        "warn"
    } else {
        "ok"
    }
}

/// A 0 to 100 score for the report: a pass counts one, a warning a half, a failure nothing;
/// informational lines are not scored. `None` when there is nothing to score.
pub fn score(ok: usize, warnings: usize, failures: usize) -> Option<u32> {
    let total = ok + warnings + failures;
    if total == 0 {
        return None;
    }
    // Counts are small (dozens), so the casts are exact.
    let points = ok as f64 + 0.5 * warnings as f64;
    Some((100.0 * points / total as f64).round() as u32)
}

/// The doctor result as a page model: checks sorted worst first (the order inside one severity is
/// the doctor's own), the counts, the level, the score, and the plain-text report for the clipboard.
/// A fix is attached to warnings and failures only.
pub fn doctor(result: &Value) -> Value {
    let version = str_at(result, "version").unwrap_or("");
    let kernel = str_at(result, "kernel").unwrap_or("");
    let mut checks: Vec<(u8, Value)> = Vec::new();
    let (mut ok, mut warn, mut fail, mut info) = (0usize, 0usize, 0usize, 0usize);
    for c in list_at(result, "checks").unwrap_or_default().iter().filter(|c| c.is_object()) {
        let status = str_at(c, "status").unwrap_or("");
        let b = badge(status);
        match b {
            "ok" => ok += 1,
            "warn" => warn += 1,
            "fail" => fail += 1,
            _ => info += 1,
        }
        let fix = str_at(c, "fix").unwrap_or("");
        let show_fix = matches!(b, "warn" | "fail") && !fix.is_empty();
        checks.push((
            severity(b),
            json!({
                "check": str_at(c, "check").unwrap_or(""),
                "status": status,
                "badge": b,
                "detail": str_at(c, "detail").unwrap_or(""),
                "fix": if show_fix { fix } else { "" },
                "show_fix": show_fix,
            }),
        ));
    }
    // A report without a checks list still carries its own counts.
    if checks.is_empty() {
        fail = f64_at(result, "failures").map_or(0, |n| n.max(0.0) as usize);
        warn = f64_at(result, "warnings").map_or(0, |n| n.max(0.0) as usize);
    }
    checks.sort_by_key(|(s, _)| *s);
    let checks: Vec<Value> = checks.into_iter().map(|(_, c)| c).collect();
    let mut report = vec![format!("TrueSense {version} ({kernel})")];
    for c in &checks {
        let fix = c["fix"].as_str().unwrap_or("");
        let tail = if fix.is_empty() { String::new() } else { format!("  (fix: {fix})") };
        report.push(format!(
            "[{}] {}: {}{tail}",
            c["status"].as_str().unwrap_or(""),
            c["check"].as_str().unwrap_or(""),
            c["detail"].as_str().unwrap_or("")
        ));
    }
    json!({
        "level": level(fail, warn),
        "failures": fail,
        "warnings": warn,
        "passed": ok,
        "info": info,
        "score": score(ok, warn, fail),
        "version": version,
        "kernel": kernel,
        "checks": checks,
        "report": report.join("\n"),
    })
}

fn word(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(true)) => "yes".to_owned(),
        Some(Value::Bool(false)) => "no".to_owned(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// The `service check` rows: each with a one-line detail (`system · enabled · active`) and the
/// fix text only for a row that is not fine. `all_ok` is true for an empty list too (nothing to run).
pub fn services(rows: &Value) -> Value {
    let list: Vec<Value> = rows
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|r| r.is_object())
        .map(|r| {
            let ok = bool_at(r, "ok").unwrap_or(false);
            let parts: Vec<String> = ["scope", "enabled", "active"]
                .iter()
                .map(|k| word(r.get(*k)))
                .filter(|p| !p.is_empty())
                .collect();
            json!({
                "unit": str_at(r, "unit").unwrap_or(""),
                "ok": ok,
                "detail": parts.join(" \u{b7} "),
                "fix": if ok { "" } else { str_at(r, "fix").unwrap_or("") },
            })
        })
        .collect();
    let bad = list.iter().filter(|r| r["ok"] == false).count();
    json!({ "rows": list, "bad": bad, "all_ok": bad == 0 })
}

/// The `service repair` steps: `{step, ok}` rows and whether every one worked.
pub fn repair(steps: &Value) -> Value {
    let rows: Vec<Value> = steps
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|s| s.is_object())
        .map(|s| json!({ "step": str_at(s, "step").unwrap_or(""), "ok": bool_at(s, "ok").unwrap_or(false) }))
        .collect();
    let failed = rows.iter().filter(|r| r["ok"] == false).count();
    json!({ "rows": rows, "failed": failed, "all_ok": failed == 0, "empty": rows.is_empty() })
}

/// One self-test line as `{status, name, detail}`: `PASS  name  (detail)`, `FAIL  name: why` and
/// `SKIP  name: why`; any other line (the `29/30 passed` footer, blank separators dropped) is `info`.
fn selftest_row(line: &str) -> Option<Value> {
    let line = clip(line);
    let text = line.trim();
    if text.is_empty() {
        return None;
    }
    for (tag, status) in [("PASS", "PASS"), ("FAIL", "FAIL"), ("SKIP", "SKIP")] {
        let Some(rest) = text.strip_prefix(tag).filter(|r| r.starts_with(char::is_whitespace)) else { continue };
        let rest = rest.trim_start();
        let (name, detail) = if status == "PASS" {
            match rest.split_once("  (") {
                Some((n, d)) => (n.trim(), d.strip_suffix(')').unwrap_or(d).trim()),
                None => (rest, ""),
            }
        } else {
            match rest.split_once(": ") {
                Some((n, d)) => (n.trim(), d.trim()),
                None => (rest, ""),
            }
        };
        return Some(json!({ "status": status, "name": name, "detail": detail }));
    }
    Some(json!({ "status": "INFO", "name": text, "detail": "" }))
}

/// A self-test result `{ok, output}` as rows with a tally. `restore_problem` is true when any line
/// says a setting could not be put back (`RESTORE FAILED`, `NOT BACK ON AUTO`): the page then tells
/// the person to check that setting, whatever the verdict says.
pub fn selftest(result: &Value) -> Value {
    let output = str_at(result, "output").unwrap_or("");
    let rows: Vec<Value> = output.lines().take(SELFTEST_MAX_LINES).filter_map(selftest_row).collect();
    let count = |status: &str| rows.iter().filter(|r| r["status"] == status).count();
    let restore_problem = output.contains("RESTORE FAILED") || output.contains("NOT BACK ON AUTO");
    // The self-test's own verdict wins; a missing one is derived from the rows.
    let ok = bool_at(result, "ok").unwrap_or_else(|| count("FAIL") == 0 && count("PASS") > 0);
    json!({
        "ok": ok && !restore_problem,
        "passed": count("PASS"),
        "failed": count("FAIL"),
        "skipped": count("SKIP"),
        "restore_problem": restore_problem,
        "rows": rows,
        "empty": rows.is_empty(),
    })
}

fn log_level(line: &str) -> &'static str {
    let lower = line.to_ascii_lowercase();
    if ["error", "fail", "panic", "fatal", "denied", "traceback"].iter().any(|w| lower.contains(w)) {
        "error"
    } else if ["warn", "retry", "timeout", "stale"].iter().any(|w| lower.contains(w)) {
        "warn"
    } else {
        "info"
    }
}

/// The daemon log as lines with a severity: the last [`LOG_MAX_LINES`], each clipped, blank lines
/// dropped. `truncated` says older lines were left out. Accepts `{text}` or a bare string.
pub fn log_tail(arg: &Value) -> Value {
    let text = str_at(arg, "text").or_else(|| arg.as_str()).unwrap_or("");
    let all: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let skip = all.len().saturating_sub(LOG_MAX_LINES);
    let lines: Vec<Value> =
        all[skip..].iter().map(|l| json!({ "text": clip(l), "level": log_level(l) })).collect();
    json!({ "lines": lines, "count": lines.len(), "truncated": skip > 0 })
}

/// The `config validate` answer: an array of problems, empty when the configuration is fine.
pub fn config(problems: &Value) -> Value {
    let list: Vec<String> = problems
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_str)
        .map(clip)
        .collect();
    json!({ "valid": list.is_empty() && problems.is_array(), "problems": list })
}

/// `doctor`, `services`, `repair`, `selftest`, `log_tail`, `config`, `capabilities`.
///
/// # Errors
/// The text for an unknown verb.
pub fn call(verb: &str, args: &Value) -> Result<Value, String> {
    let args = super::settings::unwrap_value(args);
    match verb {
        "doctor" => Ok(doctor(args)),
        "services" => Ok(services(args)),
        "repair" => Ok(repair(args)),
        "selftest" => Ok(selftest(args)),
        "log_tail" => Ok(log_tail(args)),
        "config" => Ok(config(args)),
        "capabilities" => Ok(super::settings::checklist(args)),
        _ => Err(format!("unknown health verb: {verb}")),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn report() -> Value {
        json!({
            "version": "0.5.0", "kernel": "7.3.0", "ok": false, "failures": 1, "warnings": 1,
            "checks": [
                {"check": "Kernel module", "status": "OK", "detail": "loaded", "fix": "modprobe"},
                {"check": "Service", "status": "WARN", "detail": "timer off", "fix": "systemctl enable x"},
                {"check": "Secure Boot", "status": "INFO", "detail": "off", "fix": ""},
                {"check": "Device", "status": "FAIL", "detail": "no /dev/truesense", "fix": "reinstall"},
                {"check": "Exotic", "status": "MAYBE", "detail": "new status", "fix": "x"}
            ]
        })
    }

    #[test]
    fn the_report_is_sorted_worst_first_and_counted() {
        let d = doctor(&report());
        let names: Vec<_> = d["checks"].as_array().unwrap().iter().map(|c| c["check"].as_str().unwrap()).collect();
        assert_eq!(names, ["Device", "Service", "Secure Boot", "Exotic", "Kernel module"]);
        assert_eq!(d["level"], "fail");
        assert_eq!(d["failures"], 1);
        assert_eq!(d["warnings"], 1);
        assert_eq!(d["passed"], 1);
        assert_eq!(d["info"], 2, "an unknown status is informational, never a pass");
        assert_eq!(d["score"], 50, "1 pass + half a warning of 3 scored checks");
        assert_eq!(d["version"], "0.5.0");
    }

    #[test]
    fn a_fix_is_shown_for_warnings_and_failures_only() {
        let d = doctor(&report());
        let by = |name: &str| d["checks"].as_array().unwrap().iter().find(|c| c["check"] == name).unwrap().clone();
        assert_eq!(by("Device")["fix"], "reinstall");
        assert_eq!(by("Device")["show_fix"], true);
        assert_eq!(by("Service")["show_fix"], true);
        assert_eq!(by("Kernel module")["show_fix"], false);
        assert_eq!(by("Kernel module")["fix"], "");
        assert_eq!(by("Exotic")["show_fix"], false);
    }

    #[test]
    fn the_clipboard_report_lists_every_check_with_its_fix_where_shown() {
        let text = doctor(&report())["report"].as_str().unwrap().to_owned();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines[0], "TrueSense 0.5.0 (7.3.0)");
        assert_eq!(lines[1], "[FAIL] Device: no /dev/truesense  (fix: reinstall)");
        assert_eq!(lines[2], "[WARN] Service: timer off  (fix: systemctl enable x)");
        assert!(lines.contains(&"[OK] Kernel module: loaded"));
        assert_eq!(lines.len(), 6);
    }

    #[test]
    fn levels_and_scores_have_their_boundaries() {
        assert_eq!(level(0, 0), "ok");
        assert_eq!(level(0, 2), "warn");
        assert_eq!(level(1, 0), "fail");
        assert_eq!(level(3, 5), "fail");
        assert_eq!(score(10, 0, 0), Some(100));
        assert_eq!(score(0, 0, 4), Some(0));
        assert_eq!(score(0, 2, 0), Some(50));
        assert_eq!(score(0, 0, 0), None);
        assert_eq!(score(2, 1, 0), Some(83));
    }

    #[test]
    fn an_all_good_report_and_an_empty_one() {
        let good = doctor(&json!({"checks": [{"check": "a", "status": "OK", "detail": "d", "fix": ""}]}));
        assert_eq!(good["level"], "ok");
        assert_eq!(good["score"], 100);
        let empty = doctor(&json!({}));
        assert_eq!(empty["level"], "ok");
        assert_eq!(empty["score"], Value::Null);
        assert_eq!(empty["checks"], json!([]));
        let counted = doctor(&json!({"failures": 2, "warnings": 1}));
        assert_eq!(counted["level"], "fail");
        assert_eq!(counted["failures"], 2);
    }

    #[test]
    fn hostile_doctor_shapes_do_not_panic() {
        for v in [json!(null), json!(7), json!("x"), json!({"checks": "no"}), json!({"checks": [1, null, "x", {}]})] {
            assert!(doctor(&v).is_object());
        }
        let odd = doctor(&json!({"checks": [{"check": 5, "status": null, "detail": []}]}));
        assert_eq!(odd["checks"][0]["badge"], "info");
    }

    #[test]
    fn service_rows_show_their_scope_state_and_fix_only_when_broken() {
        let s = services(&json!([
            {"unit": "truesense.service", "scope": "system", "enabled": "enabled", "active": "active", "ok": true, "fix": "x"},
            {"unit": "truesense-startup.service", "scope": "user", "enabled": false, "active": "inactive", "ok": false, "fix": "systemctl --user enable x"}
        ]));
        assert_eq!(s["all_ok"], false);
        assert_eq!(s["bad"], 1);
        assert_eq!(s["rows"][0]["detail"], "system \u{b7} enabled \u{b7} active");
        assert_eq!(s["rows"][0]["fix"], "");
        assert_eq!(s["rows"][1]["detail"], "user \u{b7} no \u{b7} inactive");
        assert_eq!(s["rows"][1]["fix"], "systemctl --user enable x");
        assert_eq!(services(&json!([]))["all_ok"], true);
        assert_eq!(services(&json!(null))["rows"], json!([]));
    }

    #[test]
    fn repair_steps_report_failures() {
        let r = repair(&json!([{"step": "start daemon", "ok": true}, {"step": "enable timer", "ok": false}, 5]));
        assert_eq!(r["failed"], 1);
        assert_eq!(r["all_ok"], false);
        assert_eq!(r["rows"].as_array().unwrap().len(), 2);
        let none = repair(&json!([]));
        assert_eq!(none["empty"], true);
        assert_eq!(none["all_ok"], true);
    }

    const OUTPUT: &str = "PASS  power modes  (cycled eco,quiet, restored balanced)\nPASS  keyboard brightness\nFAIL  rear light: reads 3, wanted 4\nSKIP  fans: skipped by request\n\n2/3 passed";

    #[test]
    fn selftest_lines_become_rows_with_a_tally() {
        let s = selftest(&json!({"ok": false, "output": OUTPUT}));
        assert_eq!(s["passed"], 2);
        assert_eq!(s["failed"], 1);
        assert_eq!(s["skipped"], 1);
        assert_eq!(s["ok"], false);
        assert_eq!(s["rows"][0], json!({"status": "PASS", "name": "power modes", "detail": "cycled eco,quiet, restored balanced"}));
        assert_eq!(s["rows"][1], json!({"status": "PASS", "name": "keyboard brightness", "detail": ""}));
        assert_eq!(s["rows"][2], json!({"status": "FAIL", "name": "rear light", "detail": "reads 3, wanted 4"}));
        assert_eq!(s["rows"][3]["status"], "SKIP");
        assert_eq!(s["rows"][4], json!({"status": "INFO", "name": "2/3 passed", "detail": ""}));
        assert_eq!(s["rows"].as_array().unwrap().len(), 5, "the blank line is dropped");
    }

    #[test]
    fn a_failed_restore_makes_the_verdict_bad_whatever_ok_says() {
        let out = "PASS  x\nFAIL  y: RESTORE FAILED: boost reads 1, wanted 2";
        let s = selftest(&json!({"ok": true, "output": out}));
        assert_eq!(s["restore_problem"], true);
        assert_eq!(s["ok"], false);
        let fans = selftest(&json!({"ok": true, "output": "FAIL  fans: FANS NOT BACK ON AUTO: x"}));
        assert_eq!(fans["restore_problem"], true);
        let fine = selftest(&json!({"ok": true, "output": "PASS  x"}));
        assert_eq!(fine["ok"], true);
        // A missing verdict is derived from the rows.
        assert_eq!(selftest(&json!({"output": "PASS  x"}))["ok"], true);
        assert_eq!(selftest(&json!({"output": "FAIL  x: y"}))["ok"], false);
        assert_eq!(selftest(&json!({}))["empty"], true);
    }

    #[test]
    fn selftest_tags_must_stand_alone_and_lines_are_clipped_and_capped() {
        let s = selftest(&json!({"output": "PASSED all\nFAILURE mode\nPASS\tx"}));
        assert_eq!(s["rows"][0]["status"], "INFO");
        assert_eq!(s["rows"][1]["status"], "INFO");
        assert_eq!(s["rows"][2], json!({"status": "PASS", "name": "x", "detail": ""}));
        let long = format!("PASS  {}", "a".repeat(5000));
        let name = selftest(&json!({"output": long}))["rows"][0]["name"].as_str().unwrap().chars().count();
        assert!(name <= LINE_MAX_CHARS);
        let many = "PASS  a\n".repeat(2000);
        assert_eq!(selftest(&json!({"output": many}))["rows"].as_array().unwrap().len(), SELFTEST_MAX_LINES);
    }

    #[test]
    fn the_log_keeps_the_tail_with_severities() {
        let text = "Nov 07 truesense[1]: started\nNov 07 truesense[1]: WARN: retry in 5 s\nNov 07 truesense[1]: error: cannot open /dev/truesense\n\n";
        let l = log_tail(&json!({"text": text}));
        assert_eq!(l["count"], 3);
        assert_eq!(l["truncated"], false);
        assert_eq!(l["lines"][0]["level"], "info");
        assert_eq!(l["lines"][1]["level"], "warn");
        assert_eq!(l["lines"][2]["level"], "error");
        let many: String = (0..500).map(|i| format!("line {i}\n")).collect();
        let l = log_tail(&json!({"text": many}));
        assert_eq!(l["count"], LOG_MAX_LINES);
        assert_eq!(l["truncated"], true);
        assert_eq!(l["lines"][LOG_MAX_LINES - 1]["text"], "line 499");
        assert_eq!(l["lines"][0]["text"], "line 300");
        assert_eq!(log_tail(&json!("bare string\n"))["count"], 1);
        assert_eq!(log_tail(&json!(null))["count"], 0);
        let huge = log_tail(&json!({"text": "x".repeat(100_000)}));
        assert!(huge["lines"][0]["text"].as_str().unwrap().chars().count() <= LINE_MAX_CHARS + 1);
    }

    #[test]
    fn the_config_check_is_valid_only_for_an_empty_list() {
        assert_eq!(config(&json!([])), json!({"valid": true, "problems": []}));
        assert_eq!(
            config(&json!(["[alerts] cpu_limit_c must be 70..100", 5])),
            json!({"valid": false, "problems": ["[alerts] cpu_limit_c must be 70..100"]})
        );
        assert_eq!(config(&json!(null))["valid"], false, "no answer is not a clean bill of health");
    }

    #[test]
    fn arrays_arrive_wrapped_because_the_bus_takes_objects_only() {
        let rows = json!({"value": [{"unit": "a.service", "scope": "user", "ok": false, "fix": "f"}]});
        assert_eq!(call("services", &rows).unwrap()["bad"], 1);
        assert_eq!(call("repair", &json!({"value": [{"step": "s", "ok": true}]})).unwrap()["all_ok"], true);
        assert_eq!(call("config", &json!({"value": ["bad"]})).unwrap()["valid"], false);
        assert_eq!(call("capabilities", &json!({"value": null})).unwrap()["available"], false);
    }

    #[test]
    fn dispatch_reaches_every_verb() {
        assert!(call("doctor", &report()).unwrap().is_object());
        assert!(call("services", &json!([])).unwrap().is_object());
        assert!(call("repair", &json!([])).unwrap().is_object());
        assert!(call("selftest", &json!({})).unwrap().is_object());
        assert!(call("log_tail", &json!({})).unwrap().is_object());
        assert!(call("config", &json!([])).unwrap().is_object());
        assert!(call("capabilities", &json!({})).unwrap().is_object());
        assert!(call("nope", &json!({})).unwrap_err().contains("nope"));
    }
}
