//! Reading the JSON the command bus returns without writing `match` ladders.
//!
//! Every command result and every live frame is a `serde_json::Value` whose shape is documented
//! in `COMMANDS.md`, and members are routinely missing or `null` (a laptop without a rear light,
//! a daemon that is not running). Page view-models therefore read through dotted paths and get
//! `None` instead of a failure; the UI shows a dash for `None`.
#![forbid(unsafe_code)]

use serde_json::{Map, Value, json};

/// The member at a dotted `path` (`"sensors.cpu_temp"`, `"fans.duty_register_pct.cpu"`; a numeric
/// segment indexes an array). A JSON `null` counts as absent, because for the UI "unknown" and
/// "not there" are the same thing.
pub fn at<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    if !path.is_empty() {
        for part in path.split('.') {
            current = match current {
                Value::Object(map) => map.get(part)?,
                Value::Array(items) => items.get(part.parse::<usize>().ok()?)?,
                _ => return None,
            };
        }
    }
    (!current.is_null()).then_some(current)
}

/// A finite number at `path` (integers and floats alike; `NaN` cannot occur in JSON).
pub(crate) fn f64_at(value: &Value, path: &str) -> Option<f64> {
    at(value, path)?.as_f64().filter(|n| n.is_finite())
}

/// An integer at `path`; a float with no fractional part counts.
pub(crate) fn i64_at(value: &Value, path: &str) -> Option<i64> {
    let v = at(value, path)?;
    v.as_i64().or_else(|| v.as_f64().filter(|f| f.fract() == 0.0 && f.abs() < 9.0e15).map(|f| f as i64))
}

/// A string at `path`.
pub(crate) fn str_at<'a>(value: &'a Value, path: &str) -> Option<&'a str> {
    at(value, path)?.as_str()
}

/// A boolean at `path`.
pub(crate) fn bool_at(value: &Value, path: &str) -> Option<bool> {
    at(value, path)?.as_bool()
}

/// An array at `path`.
pub(crate) fn list_at<'a>(value: &'a Value, path: &str) -> Option<&'a [Value]> {
    at(value, path)?.as_array().map(Vec::as_slice)
}

/// Parses the argument text QML sends. Empty text is `{}` (a command with no arguments); anything
/// that is not a JSON object is refused, because the bus contract says arguments are an object.
///
/// # Errors
/// The message to show when the text is not a JSON object.
pub fn parse_args(text: &str) -> Result<Value, String> {
    if text.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    match serde_json::from_str::<Value>(text) {
        Ok(v @ Value::Object(_)) => Ok(v),
        Ok(_) => Err("the arguments of a command must be a JSON object".to_owned()),
        Err(e) => Err(format!("the arguments of a command are not valid JSON: {e}")),
    }
}

/// The `{"message", "code"}` object the QML side decodes for a failed command.
pub(crate) fn error_object(message: &str, code: Option<&str>) -> Value {
    match code {
        Some(code) => json!({ "message": message, "code": code }),
        None => json!({ "message": message }),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn frame() -> Value {
        json!({
            "sensors": {"cpu_temp": 54, "gpu_temp": null, "fans": [2150, 2090]},
            "ac": true, "mode": "balanced", "ratio": 2.0, "name": 3.5,
        })
    }

    #[test]
    fn dotted_paths_read_nested_members_and_array_items() {
        let f = frame();
        assert_eq!(i64_at(&f, "sensors.cpu_temp"), Some(54));
        assert_eq!(f64_at(&f, "sensors.fans.1"), Some(2090.0));
        assert_eq!(str_at(&f, "mode"), Some("balanced"));
        assert_eq!(bool_at(&f, "ac"), Some(true));
        assert_eq!(list_at(&f, "sensors.fans").map(<[Value]>::len), Some(2));
    }

    #[test]
    fn null_missing_and_wrong_type_are_all_none() {
        let f = frame();
        assert_eq!(f64_at(&f, "sensors.gpu_temp"), None);
        assert_eq!(f64_at(&f, "sensors.nope.deeper"), None);
        assert_eq!(f64_at(&f, "sensors.fans.9"), None);
        assert_eq!(f64_at(&f, "sensors.fans.x"), None);
        assert_eq!(str_at(&f, "ac"), None);
        assert_eq!(f64_at(&f, "mode"), None);
        assert_eq!(at(&f, "mode.deeper"), None);
    }

    #[test]
    fn empty_path_is_the_value_itself() {
        let v = json!(7);
        assert_eq!(i64_at(&v, ""), Some(7));
        assert_eq!(at(&Value::Null, ""), None);
    }

    #[test]
    fn integers_accept_whole_floats_only() {
        let f = frame();
        assert_eq!(i64_at(&f, "ratio"), Some(2));
        assert_eq!(i64_at(&f, "name"), None);
    }

    #[test]
    fn args_must_be_an_object() {
        assert_eq!(parse_args(""), Ok(json!({})));
        assert_eq!(parse_args("  "), Ok(json!({})));
        assert_eq!(parse_args(r#"{"mode":"turbo"}"#), Ok(json!({"mode": "turbo"})));
        assert!(parse_args("[1]").is_err());
        assert!(parse_args("3").is_err());
        assert!(parse_args("{nope").is_err());
        assert!(parse_args(&"[".repeat(10_000)).is_err());
    }

    #[test]
    fn error_object_carries_the_code_only_when_there_is_one() {
        assert_eq!(error_object("x", None), json!({"message": "x"}));
        assert_eq!(error_object("x", Some("needs-daemon")), json!({"message": "x", "code": "needs-daemon"}));
    }
}
