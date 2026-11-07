//! Formatting for QML: `Bus.vm("fmt", "temp", '{"v": 54}')` is `"54 °C"`.
//!
//! The arguments are `{"v": number|null}` (and `unit` for `signed`); a missing or `null` value gives
//! the en dash, so QML can pass a live value straight through.
#![forbid(unsafe_code)]

use serde_json::{Value, json};

use crate::format;
use crate::json::{f64_at, str_at};

/// `temp`, `rpm`, `percent`, `watts`, `bytes`, `duration`, `signed`, `capitalize`.
///
/// # Errors
/// The text for an unknown verb.
pub fn call(verb: &str, args: &Value) -> Result<Value, String> {
    let v = f64_at(args, "v");
    let text = match verb {
        "temp" => format::temp(v),
        "rpm" => format::rpm(v),
        "percent" => format::percent(v),
        "watts" => format::watts(v),
        "bytes" => format::bytes(v),
        "duration" => v.map_or_else(|| format::UNKNOWN.to_owned(), format::duration),
        "signed" => v.map_or_else(|| format::UNKNOWN.to_owned(), |n| format::signed(n, str_at(args, "unit").unwrap_or(""))),
        "capitalize" => format::capitalize(str_at(args, "s").unwrap_or("")),
        _ => return Err(format!("unknown fmt verb: {verb}")),
    };
    Ok(json!(text))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn every_verb_formats_and_null_is_a_dash() {
        assert_eq!(call("temp", &json!({"v": 54.4})), Ok(json!("54 °C")));
        assert_eq!(call("rpm", &json!({"v": 2150})), Ok(json!("2150 RPM")));
        assert_eq!(call("percent", &json!({"v": 87})), Ok(json!("87 %")));
        assert_eq!(call("watts", &json!({"v": 28.54})), Ok(json!("28.5 W")));
        assert_eq!(call("bytes", &json!({"v": 1536})), Ok(json!("1.5 KB")));
        assert_eq!(call("duration", &json!({"v": 120})), Ok(json!("2 min")));
        assert_eq!(call("signed", &json!({"v": -25, "unit": "mV"})), Ok(json!("\u{2212}25 mV")));
        assert_eq!(call("capitalize", &json!({"s": "turbo"})), Ok(json!("Turbo")));
        assert_eq!(call("temp", &json!({"v": null})), Ok(json!("–")));
        assert_eq!(call("duration", &json!({})), Ok(json!("–")));
        assert!(call("nope", &json!({})).is_err());
    }
}
