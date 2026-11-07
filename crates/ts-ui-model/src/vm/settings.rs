//! What the Settings page decides: the "features supported on this machine" checklist, the
//! options of the start-up drop-downs, the alert settings with the daemon's own limits, the
//! summaries of an update check and of an installation, the licence text of the About box, and the
//! summary of "reset everything".
//!
//! The validation here **mirrors** the rules of the layers below (`AlertsConfig::validate` in
//! ts-foundation, `UiSettings::set` for the window's own settings) so that the page can refuse a
//! value before sending it and say which range is allowed; the layers below stay the authority, and
//! a value this module accepts may still be refused by them (their text is then shown as it is).
//!
//! Verbs: `checklist`, `alerts_view`, `alerts_validate`, `alerts_ranges`, `ui_validate`,
//! `startup_options`, `update_summary`, `apply_summary`, `licence`, `reset_summary`.
#![forbid(unsafe_code)]

use serde_json::{Map, Value, json};

use crate::json::{at, bool_at, f64_at, list_at, str_at};
use crate::settings::UiSettings;

/// The address updates may link to; anything else in a release's `url` is not offered as a link.
pub(crate) const RELEASE_URL_PREFIX: &str = "https://github.com/";
/// Where the installer writes its log, named in the message when it exits with an error.
pub(crate) const INSTALL_LOG: &str = "/var/log/truesense-install.log";
/// Most characters of the licence file the About box shows (the file is a few kilobytes).
pub(crate) const LICENCE_MAX_CHARS: usize = 60_000;

/// The allowed range of one alert setting, as the daemon enforces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    /// Smallest allowed value.
    pub min: i64,
    /// Largest allowed value.
    pub max: i64,
    /// A value that is allowed besides the range (battery low: 0 means off).
    pub also: Option<i64>,
}

/// The integer alert settings and their ranges: temperature limits 70 to 100 C, repeat 1 to 240
/// minutes, debounce 1 to 30 samples, battery low 5 to 50 % or 0 for off.
pub(crate) const ALERT_FIELDS: &[(&str, Range)] = &[
    ("cpu_limit_c", Range { min: 70, max: 100, also: None }),
    ("gpu_limit_c", Range { min: 70, max: 100, also: None }),
    ("repeat_minutes", Range { min: 1, max: 240, also: None }),
    ("debounce_samples", Range { min: 1, max: 30, also: None }),
    ("battery_low_pct", Range { min: 5, max: 50, also: Some(0) }),
];
/// The yes/no alert settings.
pub(crate) const ALERT_FLAGS: &[&str] = &["enabled", "throttling", "sensor_lost"];

fn alert_default(field: &str) -> Value {
    match field {
        "cpu_limit_c" | "gpu_limit_c" => json!(90),
        "repeat_minutes" => json!(5),
        "debounce_samples" => json!(3),
        "battery_low_pct" => json!(15),
        _ => json!(true),
    }
}

/// Why a value was refused: `unknown` (no such setting), `type` (not a whole number or not a
/// boolean) or `range`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The setting does not exist.
    Unknown,
    /// The value has the wrong type.
    Type,
    /// The value is outside the allowed range.
    Range(Range),
}

/// Checks one alert setting and returns the value to send (a whole number or a boolean).
///
/// # Errors
/// [`Refusal`] with the reason.
pub(crate) fn alert_value(field: &str, value: &Value) -> Result<Value, Refusal> {
    if ALERT_FLAGS.contains(&field) {
        return value.as_bool().map(Value::Bool).ok_or(Refusal::Type);
    }
    let Some((_, range)) = ALERT_FIELDS.iter().find(|(name, _)| *name == field) else {
        return Err(Refusal::Unknown);
    };
    // A whole number, written as 90 or 90.0; a fraction is a typing slip, not a rounding wish.
    let n = value
        .as_i64()
        .or_else(|| value.as_f64().filter(|f| f.is_finite() && f.fract() == 0.0 && f.abs() < 1.0e9).map(|f| f as i64))
        .ok_or(Refusal::Type)?;
    if (range.min..=range.max).contains(&n) || range.also == Some(n) {
        Ok(json!(n))
    } else {
        Err(Refusal::Range(*range))
    }
}

/// `alerts_validate`: `{"field", "value"}` gives `{"ok": true, "value"}` or `{"ok": false,
/// "reason": "range"|"type"|"unknown", "min", "max", "also"}` (the range only for `range`).
pub fn alerts_validate(args: &Value) -> Value {
    let field = str_at(args, "field").unwrap_or("");
    match alert_value(field, args.get("value").unwrap_or(&Value::Null)) {
        Ok(value) => json!({ "ok": true, "value": value }),
        Err(Refusal::Unknown) => json!({ "ok": false, "reason": "unknown" }),
        Err(Refusal::Type) => json!({ "ok": false, "reason": "type" }),
        Err(Refusal::Range(r)) => json!({ "ok": false, "reason": "range", "min": r.min, "max": r.max, "also": r.also }),
    }
}

/// `alerts_ranges`: every integer setting with its range, so the page builds its sliders from the
/// same numbers the validation uses.
pub(crate) fn alerts_ranges() -> Value {
    let mut out = Map::new();
    for (name, r) in ALERT_FIELDS {
        out.insert((*name).to_owned(), json!({ "min": r.min, "max": r.max, "also": r.also, "default": alert_default(name) }));
    }
    Value::Object(out)
}

/// `alerts_view`: the `alerts get` answer with every setting present and valid. A member that is
/// missing or out of range reads as its default, because the daemon would be using the default.
/// `battery_low_on` is whether the battery alert is on (the value is not 0), and `battery_low_pct`
/// then is the percentage to show on the slider (the default when the alert is off).
pub fn alerts_view(alerts: &Value) -> Value {
    let mut out = Map::new();
    for flag in ALERT_FLAGS {
        out.insert((*flag).to_owned(), json!(bool_at(alerts, flag).unwrap_or(true)));
    }
    for (name, _) in ALERT_FIELDS {
        let value = at(alerts, name).and_then(|v| alert_value(name, v).ok()).unwrap_or_else(|| alert_default(name));
        out.insert((*name).to_owned(), value);
    }
    let low = out.get("battery_low_pct").and_then(Value::as_i64).unwrap_or(0);
    out.insert("battery_low_on".to_owned(), json!(low != 0));
    if low == 0 {
        out.insert("battery_low_pct".to_owned(), alert_default("battery_low_pct"));
    }
    Value::Object(out)
}

/// `ui_validate`: `{"key", "value"}` for the window's own settings (theme, language, text size,
/// reduced motion, tray), judged by the rules of the settings file itself. Gives `{"ok": true}` or
/// `{"ok": false, "error"}`.
pub(crate) fn ui_validate(args: &Value) -> Value {
    let key = str_at(args, "key").unwrap_or("");
    match UiSettings::default().set(key, args.get("value").unwrap_or(&Value::Null)) {
        Ok(()) => json!({ "ok": true }),
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    }
}

/// The word of a capability tier the page switches on: `supported`, `unverified`, `unsupported`,
/// anything else `unknown`.
fn tier(v: Option<&str>) -> &'static str {
    match v {
        Some("supported") => "supported",
        Some("unverified") => "unverified",
        Some("unsupported") => "unsupported",
        _ => "unknown",
    }
}

/// The checklist of `capabilities get`: the model line, its tier, and one row per feature. Rows of
/// the wrong shape are skipped. `available` is false when the answer is not an object (the command
/// failed), so the page can say so instead of showing an empty list as "nothing is supported".
pub fn checklist(caps: &Value) -> Value {
    if !caps.is_object() {
        return json!({ "available": false, "items": [], "supported": 0, "total": 0 });
    }
    let items: Vec<Value> = list_at(caps, "items")
        .unwrap_or_default()
        .iter()
        .filter_map(|i| {
            let id = str_at(i, "id")?;
            let supported = bool_at(i, "supported")?;
            Some(json!({
                "id": id,
                "label": str_at(i, "label").unwrap_or(id),
                "supported": supported,
                "why": if supported { None } else { str_at(i, "why") },
                "source": str_at(i, "source").unwrap_or(""),
                "detail": str_at(i, "detail"),
            }))
        })
        .collect();
    let supported = items.iter().filter(|i| i["supported"] == true).count();
    let part = |k: &str| str_at(caps, &format!("model.{k}")).map(str::trim).filter(|s| !s.is_empty());
    let title = match (part("vendor"), part("product")) {
        (Some(v), Some(p)) if p.to_lowercase().starts_with(&v.to_lowercase()) => p.to_owned(),
        (Some(v), Some(p)) => format!("{v} {p}"),
        (None, Some(p)) => p.to_owned(),
        (Some(v), None) => v.to_owned(),
        (None, None) => String::new(),
    };
    json!({
        "available": true,
        "model": {
            "title": title,
            "bios": part("bios"),
            "tier": tier(str_at(caps, "model.tier")),
            "message": str_at(caps, "model.message"),
        },
        "simulated": bool_at(caps, "simulated").unwrap_or(false),
        "answered": str_at(caps, "firmware_answered"),
        "items": items,
        "supported": supported,
        "total": items.len(),
    })
}

/// One option of a start-up drop-down: `kind` is `stock` (value is `last`, `default`, `none` or
/// `off`), `look`, `keyset` or `profile` with its `name`, `none` (no profile) or `missing` (the
/// saved choice names something that no longer exists; kept so the drop-down still shows it).
fn option(value: &str, kind: &str, name: &str) -> Value {
    json!({ "value": value, "kind": kind, "name": name })
}

fn names(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|n| n.as_str().or_else(|| str_at(n, "name")))
        .map(str::to_owned)
        .collect()
}

fn with_missing(mut options: Vec<Value>, current: &str) -> (Vec<Value>, String) {
    if !options.iter().any(|o| o["value"] == current) {
        options.push(option(current, "missing", current));
    }
    (options, current.to_owned())
}

/// The three start-up drop-downs (`startup show` plus the lists of looks, key sets and profiles):
/// their options and the current value of each. The stock choices come first; an empty or missing
/// setting reads as `last` (lights) or `none` (profile), as the command line does.
pub fn startup_options(args: &Value) -> Value {
    let looks = names(args.get("looks"));
    let keysets = names(args.get("keysets"));
    let profiles = names(args.get("profiles"));
    let stock = || {
        vec![
            option("last", "stock", "last"),
            option("default", "stock", "default"),
            option("none", "stock", "none"),
            option("off", "stock", "off"),
        ]
    };
    let current = |k: &str, fallback: &str| {
        str_at(args, &format!("show.{k}")).filter(|s| !s.is_empty()).unwrap_or(fallback).to_owned()
    };

    let mut keyboard = stock();
    keyboard.extend(looks.iter().map(|n| option(&format!("look:{n}"), "look", n)));
    keyboard.extend(keysets.iter().map(|n| option(&format!("profile:{n}"), "keyset", n)));
    let (keyboard, keyboard_now) = with_missing(keyboard, &current("keyboard", "last"));

    let mut rear = stock();
    rear.extend(looks.iter().map(|n| option(&format!("look:{n}"), "look", n)));
    let (rear, rear_now) = with_missing(rear, &current("rear", "last"));

    let mut profile = vec![option("none", "none", "")];
    profile.extend(profiles.iter().map(|n| option(n, "profile", n)));
    let (profile, profile_now) = with_missing(profile, &current("profile", "none"));

    json!({
        "keyboard": { "options": keyboard, "current": keyboard_now },
        "rear": { "options": rear, "current": rear_now },
        "profile": { "options": profile, "current": profile_now },
    })
}

/// The `update check` answer: `state` is `available`, `current` (the newest version is installed)
/// or `unknown` (no release could be found); `error` carries the check's own text; `url` is the
/// release page, only when it is on GitHub (the page offers "What's new" for it, never another host).
pub fn update_summary(check: &Value) -> Value {
    let available = bool_at(check, "available").unwrap_or(false);
    let latest = str_at(check, "latest").filter(|s| !s.is_empty());
    let state = if available {
        "available"
    } else if latest.is_some() {
        "current"
    } else {
        "unknown"
    };
    let url = str_at(check, "info.url").filter(|u| u.starts_with(RELEASE_URL_PREFIX));
    json!({
        "state": state,
        "current": str_at(check, "current").unwrap_or(""),
        "latest": latest,
        "error": str_at(check, "error").filter(|s| !s.is_empty()),
        "url": url,
    })
}

/// The `update apply` answer: `ok` when the installer exited with 0. `verified` is whether the
/// download matched the release's published checksum (`null`: this release predates them). When the
/// installer failed `log` names its log file.
pub fn apply_summary(result: &Value) -> Value {
    let code = f64_at(result, "returncode").map(f64::round);
    let ok = code == Some(0.0);
    json!({
        "ok": ok,
        "returncode": code,
        "version": str_at(result, "version").unwrap_or(""),
        "verified": bool_at(result, "verified"),
        "log": if ok { None } else { Some(INSTALL_LOG) },
    })
}

/// The licence and attribution text for the About box, from the contents of `THIRD_PARTY.md`
/// (or what stands in for it). `available` is false for missing or blank text. Control characters
/// other than newline and tab are removed (the text is shown as plain text, never as markup), and
/// the text is cut at [`LICENCE_MAX_CHARS`]; `truncated` says so.
pub fn licence(arg: &Value) -> Value {
    let raw = str_at(arg, "text").or_else(|| arg.as_str()).unwrap_or("");
    let clean: String =
        raw.replace("\r\n", "\n").chars().filter(|c| *c == '\n' || *c == '\t' || !c.is_control()).collect();
    let trimmed = clean.trim();
    if trimmed.is_empty() {
        return json!({ "available": false, "text": "", "truncated": false });
    }
    let truncated = trimmed.chars().count() > LICENCE_MAX_CHARS;
    let text: String = trimmed.chars().take(LICENCE_MAX_CHARS).collect();
    json!({ "available": true, "text": text, "truncated": truncated })
}

/// The `config reset-all` answer: which steps failed (with their own error text) and how many
/// worked. Every step runs even when one fails, so a partial result is normal and is listed.
pub fn reset_summary(result: &Value) -> Value {
    let steps = list_at(result, "steps").unwrap_or_default();
    let failed: Vec<Value> = steps
        .iter()
        .filter(|s| bool_at(s, "ok") != Some(true))
        .map(|s| json!({ "step": str_at(s, "step").unwrap_or(""), "error": str_at(s, "error").unwrap_or("") }))
        .collect();
    json!({ "ok_count": steps.len() - failed.len(), "failed": failed, "all_ok": failed.is_empty() })
}

/// The bus accepts only JSON *objects* as arguments, but several commands answer an array or `null`
/// (a list of services, a failed read). QML therefore wraps what it passes as `{"value": x}`, and
/// every view-model of this page set unwraps it here; an object with any other members is the
/// argument itself.
pub(crate) fn unwrap_value(args: &Value) -> &Value {
    match args.as_object() {
        Some(map) if map.len() == 1 => map.get("value").unwrap_or(args),
        _ => args,
    }
}

/// See the module documentation for the verbs.
///
/// # Errors
/// The text for an unknown verb.
pub fn call(verb: &str, args: &Value) -> Result<Value, String> {
    let args = unwrap_value(args);
    match verb {
        "checklist" => Ok(checklist(args)),
        "alerts_view" => Ok(alerts_view(args)),
        "alerts_validate" => Ok(alerts_validate(args)),
        "alerts_ranges" => Ok(alerts_ranges()),
        "ui_validate" => Ok(ui_validate(args)),
        "startup_options" => Ok(startup_options(args)),
        "update_summary" => Ok(update_summary(args)),
        "apply_summary" => Ok(apply_summary(args)),
        "licence" => Ok(licence(args)),
        "reset_summary" => Ok(reset_summary(args)),
        _ => Err(format!("unknown settings verb: {verb}")),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use ts_foundation::config::AlertsConfig;

    fn caps() -> Value {
        json!({
            "model": {"vendor": "Acer", "product": "Predator PH16-71", "bios": "V1.12", "tier": "supported", "message": "verified"},
            "simulated": false, "firmware_answered": "12/14",
            "items": [
                {"id": "power_modes", "label": "Power modes", "supported": true, "why": null, "source": "firmware", "detail": null},
                {"id": "gpu_mode", "label": "GPU mode switch", "supported": false, "why": "TGP is fixed", "source": "dmi", "detail": "x"},
                {"id": "bad"},
                "junk"
            ]
        })
    }

    #[test]
    fn the_checklist_lists_features_and_counts_the_supported_ones() {
        let c = checklist(&caps());
        assert_eq!(c["available"], true);
        assert_eq!(c["total"], 2, "rows of the wrong shape are skipped");
        assert_eq!(c["supported"], 1);
        assert_eq!(c["items"][0]["id"], "power_modes");
        assert_eq!(c["items"][1]["why"], "TGP is fixed");
        assert_eq!(c["items"][0]["why"], Value::Null);
        assert_eq!(c["model"]["title"], "Acer Predator PH16-71");
        assert_eq!(c["model"]["tier"], "supported");
        assert_eq!(c["answered"], "12/14");
    }

    #[test]
    fn the_model_title_does_not_repeat_the_vendor_and_survives_gaps() {
        let t = |v: Value| checklist(&json!({"model": v, "items": []}))["model"]["title"].clone();
        assert_eq!(t(json!({"vendor": "Acer", "product": "Acer Nitro"})), "Acer Nitro");
        assert_eq!(t(json!({"vendor": "Acer", "product": "unknown"})), "Acer unknown");
        assert_eq!(t(json!({"product": "PH16"})), "PH16");
        assert_eq!(t(json!({"vendor": " Acer "})), "Acer");
        assert_eq!(t(json!({})), "");
        assert_eq!(checklist(&json!({"model": {"tier": "weird"}, "items": []}))["model"]["tier"], "unknown");
    }

    #[test]
    fn a_failed_probe_is_not_shown_as_nothing_supported() {
        for v in [json!(null), json!("ERROR"), json!([])] {
            let c = checklist(&v);
            assert_eq!(c["available"], false);
            assert_eq!(c["total"], 0);
        }
        let empty = checklist(&json!({}));
        assert_eq!(empty["available"], true);
        assert_eq!(empty["total"], 0);
    }

    #[test]
    fn an_unsupported_feature_with_no_reason_has_none_and_a_supported_one_never_has_one() {
        let c = checklist(&json!({"items": [
            {"id": "a", "supported": false},
            {"id": "b", "supported": true, "why": "ignored"}
        ]}));
        assert_eq!(c["items"][0]["why"], Value::Null);
        assert_eq!(c["items"][0]["label"], "a", "the id stands in for a missing label");
        assert_eq!(c["items"][1]["why"], Value::Null);
    }

    #[test]
    fn alert_ranges_match_the_daemons_validation_exactly() {
        // The daemon's own validator is the oracle: every value in and around each range gets the same verdict.
        for (field, range) in ALERT_FIELDS {
            for n in (range.min - 3)..=(range.max + 3) {
                let mut cfg = AlertsConfig::default();
                match *field {
                    "cpu_limit_c" => cfg.cpu_limit_c = n,
                    "gpu_limit_c" => cfg.gpu_limit_c = n,
                    "repeat_minutes" => cfg.repeat_minutes = n,
                    "debounce_samples" => cfg.debounce_samples = n,
                    _ => cfg.battery_low_pct = n,
                }
                let daemon_ok = cfg.validate().is_empty();
                assert_eq!(alert_value(field, &json!(n)).is_ok(), daemon_ok, "{field} = {n}");
            }
        }
        // 0 is off for the battery alert and the only value below the range that is allowed.
        assert!(alert_value("battery_low_pct", &json!(0)).is_ok());
        assert!(alert_value("battery_low_pct", &json!(4)).is_err());
        let off = AlertsConfig { battery_low_pct: 0, ..AlertsConfig::default() };
        assert!(off.validate().is_empty());
    }

    #[test]
    fn alert_values_are_whole_numbers_and_flags_are_booleans() {
        assert_eq!(alert_value("cpu_limit_c", &json!(90.0)), Ok(json!(90)));
        assert_eq!(alert_value("cpu_limit_c", &json!(90.5)), Err(Refusal::Type));
        assert_eq!(alert_value("cpu_limit_c", &json!("90")), Err(Refusal::Type));
        assert_eq!(alert_value("cpu_limit_c", &Value::Null), Err(Refusal::Type));
        assert!(matches!(alert_value("cpu_limit_c", &json!(101)), Err(Refusal::Range(_))));
        assert_eq!(alert_value("enabled", &json!(false)), Ok(json!(false)));
        assert_eq!(alert_value("enabled", &json!(1)), Err(Refusal::Type));
        assert_eq!(alert_value("sound", &json!(1)), Err(Refusal::Unknown));
        assert_eq!(alert_value("cpu_limit_c", &json!(1.0e300)), Err(Refusal::Type));
    }

    #[test]
    fn the_validate_verb_reports_the_allowed_range() {
        assert_eq!(alerts_validate(&json!({"field": "repeat_minutes", "value": 5})), json!({"ok": true, "value": 5}));
        assert_eq!(
            alerts_validate(&json!({"field": "repeat_minutes", "value": 500})),
            json!({"ok": false, "reason": "range", "min": 1, "max": 240, "also": null})
        );
        assert_eq!(alerts_validate(&json!({"field": "battery_low_pct", "value": 2}))["also"], 0);
        assert_eq!(alerts_validate(&json!({"field": "x", "value": 1}))["reason"], "unknown");
        assert_eq!(alerts_validate(&json!({}))["reason"], "unknown");
        assert_eq!(alerts_ranges()["cpu_limit_c"], json!({"min": 70, "max": 100, "also": null, "default": 90}));
    }

    #[test]
    fn the_defaults_equal_the_daemons_defaults() {
        let d = AlertsConfig::default();
        let view = alerts_view(&json!({}));
        assert_eq!(view["cpu_limit_c"], d.cpu_limit_c);
        assert_eq!(view["gpu_limit_c"], d.gpu_limit_c);
        assert_eq!(view["repeat_minutes"], d.repeat_minutes);
        assert_eq!(view["debounce_samples"], d.debounce_samples);
        assert_eq!(view["battery_low_pct"], d.battery_low_pct);
        assert_eq!(view["enabled"], d.enabled);
        assert_eq!(view["throttling"], d.throttling);
        assert_eq!(view["sensor_lost"], d.sensor_lost);
    }

    #[test]
    fn the_alerts_view_repairs_what_is_missing_or_out_of_range() {
        let v = alerts_view(&json!({"enabled": false, "cpu_limit_c": 85, "gpu_limit_c": 500, "repeat_minutes": "x", "battery_low_pct": 0}));
        assert_eq!(v["enabled"], false);
        assert_eq!(v["cpu_limit_c"], 85);
        assert_eq!(v["gpu_limit_c"], 90, "an invalid value reads as the default the daemon would use");
        assert_eq!(v["repeat_minutes"], 5);
        assert_eq!(v["battery_low_on"], false);
        assert_eq!(v["battery_low_pct"], 15, "the slider keeps a sensible value while the alert is off");
        let on = alerts_view(&json!({"battery_low_pct": 20}));
        assert_eq!(on["battery_low_on"], true);
        assert_eq!(on["battery_low_pct"], 20);
        assert!(alerts_view(&json!(null)).is_object());
    }

    #[test]
    fn the_windows_own_settings_are_judged_by_the_settings_file_rules() {
        let ok = |k: &str, v: Value| ui_validate(&json!({"key": k, "value": v}))["ok"] == true;
        assert!(ok("theme", json!("light")));
        assert!(!ok("theme", json!("blue")));
        assert!(ok("language", json!("pseudo")));
        assert!(ok("language", json!("ar")));
        assert!(!ok("language", json!("fr")));
        assert!(ok("font_scale", json!(1.4)));
        assert!(!ok("font_scale", json!("big")));
        assert!(ok("reduced_motion", json!(true)));
        assert!(!ok("reduced_motion", json!("yes")));
        assert!(ok("minimise_to_tray", json!(false)));
        assert!(!ok("width", json!(5)), "the window geometry is not a setting of this page");
        assert!(ui_validate(&json!({"key": "theme", "value": "x"}))["error"].as_str().is_some());
    }

    #[test]
    fn startup_drop_downs_list_stock_choices_first_then_what_the_person_has() {
        let o = startup_options(&json!({
            "show": {"profile": "gaming", "keyboard": "look:calm", "rear": "off"},
            "looks": ["calm", "work"], "keysets": ["mine"], "profiles": [{"name": "gaming"}, {"name": "quiet"}]
        }));
        let values = |k: &str| -> Vec<String> {
            o[k]["options"].as_array().unwrap().iter().map(|x| x["value"].as_str().unwrap().to_owned()).collect()
        };
        assert_eq!(values("keyboard"), ["last", "default", "none", "off", "look:calm", "look:work", "profile:mine"]);
        assert_eq!(values("rear"), ["last", "default", "none", "off", "look:calm", "look:work"]);
        assert_eq!(values("profile"), ["none", "gaming", "quiet"]);
        assert_eq!(o["keyboard"]["current"], "look:calm");
        assert_eq!(o["rear"]["current"], "off");
        assert_eq!(o["profile"]["current"], "gaming");
        assert_eq!(o["keyboard"]["options"][4], json!({"value": "look:calm", "kind": "look", "name": "calm"}));
        assert_eq!(o["keyboard"]["options"][6]["kind"], "keyset");
    }

    #[test]
    fn startup_defaults_and_a_choice_that_no_longer_exists() {
        let o = startup_options(&json!({}));
        assert_eq!(o["keyboard"]["current"], "last");
        assert_eq!(o["rear"]["current"], "last");
        assert_eq!(o["profile"]["current"], "none");
        let gone = startup_options(&json!({"show": {"keyboard": "look:deleted", "profile": "old"}, "looks": [], "profiles": []}));
        assert_eq!(gone["keyboard"]["current"], "look:deleted");
        assert_eq!(gone["keyboard"]["options"].as_array().unwrap().last().unwrap()["kind"], "missing");
        assert_eq!(gone["profile"]["options"].as_array().unwrap().last().unwrap()["value"], "old");
        let blank = startup_options(&json!({"show": {"keyboard": ""}}));
        assert_eq!(blank["keyboard"]["current"], "last");
    }

    #[test]
    fn update_states() {
        let avail = update_summary(&json!({"current": "0.4.0", "latest": "0.5.0", "available": true, "error": null,
            "info": {"tag": "v0.5.0", "url": "https://github.com/Order52/truesense/releases/tag/v0.5.0"}}));
        assert_eq!(avail["state"], "available");
        assert_eq!(avail["url"], "https://github.com/Order52/truesense/releases/tag/v0.5.0");
        let cur = update_summary(&json!({"current": "0.5.0", "latest": "0.5.0", "available": false}));
        assert_eq!(cur["state"], "current");
        assert_eq!(cur["latest"], "0.5.0");
        let offline = update_summary(&json!({"current": "0.5.0", "latest": null, "available": false, "error": "cannot reach GitHub"}));
        assert_eq!(offline["state"], "unknown");
        assert_eq!(offline["error"], "cannot reach GitHub");
        assert_eq!(update_summary(&json!(null))["state"], "unknown");
    }

    #[test]
    fn only_github_release_links_are_offered() {
        for url in [
            "http://github.com/x",
            "https://evil.example/github.com/",
            "https://github.com.evil.example/",
            "javascript:alert(1)",
            "",
        ] {
            let s = update_summary(&json!({"latest": "1", "info": {"url": url}}));
            assert_eq!(s["url"], Value::Null, "{url}");
        }
        assert_eq!(update_summary(&json!({"latest": "1"}))["url"], Value::Null);
    }

    #[test]
    fn installer_results_say_whether_it_worked_and_where_the_log_is() {
        let ok = apply_summary(&json!({"version": "0.5.0", "returncode": 0, "sha256": "ab", "verified": true}));
        assert_eq!(ok, json!({"ok": true, "returncode": 0.0, "version": "0.5.0", "verified": true, "log": null}));
        let bad = apply_summary(&json!({"version": "0.5.0", "returncode": 1, "verified": null}));
        assert_eq!(bad["ok"], false);
        assert_eq!(bad["log"], INSTALL_LOG);
        assert_eq!(bad["verified"], Value::Null);
        assert_eq!(apply_summary(&json!({}))["ok"], false, "no return code is not a success");
    }

    #[test]
    fn licence_text_is_cleaned_and_capped() {
        let l = licence(&json!({"text": "  # Credits\r\n\r\nSome\u{0}thing \u{7} by someone\tx\n\n"}));
        assert_eq!(l["available"], true);
        assert_eq!(l["text"], "# Credits\n\nSomething  by someone\tx");
        assert_eq!(licence(&json!({"text": "   \n\t"}))["available"], false);
        assert_eq!(licence(&json!(null))["available"], false);
        assert_eq!(licence(&json!("bare"))["text"], "bare");
        let big = licence(&json!({"text": "a".repeat(LICENCE_MAX_CHARS + 10)}));
        assert_eq!(big["truncated"], true);
        assert_eq!(big["text"].as_str().unwrap().chars().count(), LICENCE_MAX_CHARS);
        let multi = licence(&json!({"text": "\u{627}\u{644}".repeat(10)}));
        assert_eq!(multi["text"].as_str().unwrap().chars().count(), 20, "characters, not bytes");
    }

    #[test]
    fn reset_lists_the_failed_steps_with_their_errors() {
        let r = reset_summary(&json!({"steps": [
            {"step": "fan curve", "ok": true},
            {"step": "undervolt", "ok": false, "error": "needs the service"},
            {"step": "odd"}
        ]}));
        assert_eq!(r["ok_count"], 1);
        assert_eq!(r["all_ok"], false);
        assert_eq!(r["failed"][0], json!({"step": "undervolt", "error": "needs the service"}));
        assert_eq!(r["failed"][1], json!({"step": "odd", "error": ""}));
        assert_eq!(reset_summary(&json!({"steps": []}))["all_ok"], true);
        assert_eq!(reset_summary(&json!(null))["ok_count"], 0);
    }

    #[test]
    fn wrapped_arguments_are_unwrapped_and_other_objects_are_left_alone() {
        assert_eq!(unwrap_value(&json!({"value": [1, 2]})), &json!([1, 2]));
        assert_eq!(unwrap_value(&json!({"value": null})), &json!(null));
        assert_eq!(unwrap_value(&json!({"value": 1, "x": 2})), &json!({"value": 1, "x": 2}));
        assert_eq!(unwrap_value(&json!({"other": 1})), &json!({"other": 1}));
        assert_eq!(unwrap_value(&json!([1])), &json!([1]));
        assert_eq!(call("checklist", &json!({"value": null})).unwrap()["available"], false);
        assert_eq!(
            call("alerts_validate", &json!({"value": {"field": "enabled", "value": true}})).unwrap()["ok"],
            true
        );
    }

    #[test]
    fn dispatch_reaches_every_verb() {
        for verb in ["checklist", "alerts_view", "alerts_validate", "alerts_ranges", "ui_validate", "startup_options", "update_summary", "apply_summary", "licence", "reset_summary"] {
            assert!(call(verb, &json!({})).unwrap().is_object(), "{verb}");
        }
        assert!(call("nope", &json!({})).unwrap_err().contains("nope"));
    }
}
