//! Formatting of measured values for display: pure functions, no locale, no Qt.
//!
//! An unknown value (`None`) is always an en dash, so a missing sensor never shows as `0 °C`.
//! The texts match the web window's `format.js`, so the two front ends read the same.
#![forbid(unsafe_code)]

/// The text for a value that is not known.
pub(crate) const UNKNOWN: &str = "–";

fn known(v: Option<f64>) -> Option<f64> {
    v.filter(|n| n.is_finite())
}

/// `54 °C` (rounded to whole degrees).
pub fn temp(v: Option<f64>) -> String {
    known(v).map_or_else(|| UNKNOWN.to_owned(), |n| format!("{} °C", n.round()))
}

/// `2150 RPM`.
pub fn rpm(v: Option<f64>) -> String {
    known(v).map_or_else(|| UNKNOWN.to_owned(), |n| format!("{} RPM", n.round()))
}

/// `87 %`.
pub fn percent(v: Option<f64>) -> String {
    known(v).map_or_else(|| UNKNOWN.to_owned(), |n| format!("{} %", n.round()))
}

/// `28.5 W` (one decimal, none when it is a whole number).
pub fn watts(v: Option<f64>) -> String {
    known(v).map_or_else(|| UNKNOWN.to_owned(), |n| format!("{} W", (n * 10.0).round() / 10.0))
}

/// `+10 mV`, `−25 mV`, `0 mV`: a signed number with a real minus sign and an optional unit.
pub fn signed(v: f64, unit: &str) -> String {
    let sign = if v > 0.0 {
        "+"
    } else if v < 0.0 {
        "\u{2212}"
    } else {
        ""
    };
    let number = v.abs();
    if unit.is_empty() { format!("{sign}{number}") } else { format!("{sign}{number} {unit}") }
}

/// How long a hold or a delay still lasts: `45 s`, `2 min`, `1.5 h`.
pub fn duration(seconds: f64) -> String {
    if !seconds.is_finite() {
        return UNKNOWN.to_owned();
    }
    let s = seconds.max(0.0).round();
    if s < 90.0 {
        return format!("{s} s");
    }
    let m = (s / 60.0).round();
    if m < 90.0 { format!("{m} min") } else { format!("{} h", (m / 6.0).round() / 10.0) }
}

/// `1.5 GB`, `512 MB`: binary multiples (1024) under the common names.
pub fn bytes(v: Option<f64>) -> String {
    let Some(mut n) = known(v).filter(|n| *n >= 0.0) else { return UNKNOWN.to_owned() };
    for unit in ["B", "KB", "MB", "GB", "TB"] {
        if n < 1024.0 || unit == "TB" {
            return if unit == "B" || n >= 100.0 {
                format!("{} {unit}", n.round())
            } else {
                format!("{:.1} {unit}", n)
            };
        }
        n /= 1024.0;
    }
    UNKNOWN.to_owned()
}

/// Capitalises the first letter: `balanced` becomes `Balanced`.
pub fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn measurements_round_and_carry_units() {
        assert_eq!(temp(Some(54.4)), "54 °C");
        assert_eq!(temp(Some(54.5)), "55 °C");
        assert_eq!(rpm(Some(2149.6)), "2150 RPM");
        assert_eq!(percent(Some(87.0)), "87 %");
        assert_eq!(watts(Some(28.54)), "28.5 W");
        assert_eq!(watts(Some(6.0)), "6 W");
    }

    #[test]
    fn unknown_and_non_finite_are_a_dash_never_zero() {
        for f in [temp, rpm, percent, watts, bytes] {
            assert_eq!(f(None), UNKNOWN);
            assert_eq!(f(Some(f64::NAN)), UNKNOWN);
        }
        assert_eq!(temp(Some(f64::INFINITY)), UNKNOWN);
        assert_eq!(duration(f64::NAN), UNKNOWN);
    }

    #[test]
    fn signed_uses_a_real_minus() {
        assert_eq!(signed(10.0, "mV"), "+10 mV");
        assert_eq!(signed(-25.0, "mV"), "\u{2212}25 mV");
        assert_eq!(signed(0.0, "mV"), "0 mV");
        assert_eq!(signed(-3.0, ""), "\u{2212}3");
    }

    #[test]
    fn durations_switch_unit_where_the_web_window_does() {
        assert_eq!(duration(0.0), "0 s");
        assert_eq!(duration(45.0), "45 s");
        assert_eq!(duration(89.0), "89 s");
        assert_eq!(duration(90.0), "2 min");
        assert_eq!(duration(1800.0), "30 min");
        assert_eq!(duration(5340.0), "89 min");
        assert_eq!(duration(5400.0), "1.5 h");
        assert_eq!(duration(-5.0), "0 s");
    }

    #[test]
    fn byte_sizes() {
        assert_eq!(bytes(Some(512.0)), "512 B");
        assert_eq!(bytes(Some(1536.0)), "1.5 KB");
        assert_eq!(bytes(Some(512.0 * 1024.0 * 1024.0)), "512 MB");
        assert_eq!(bytes(Some(1.5 * 1024.0 * 1024.0 * 1024.0)), "1.5 GB");
        assert_eq!(bytes(Some(-1.0)), UNKNOWN);
    }

    /// The same table the QML component test (`Fmt.qml`) runs: the two implementations must agree.
    #[test]
    fn matches_the_vectors_shared_with_the_qml_twin() {
        let vectors: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../tests/format-vectors.json")).unwrap();
        assert!(vectors.len() > 20);
        for v in vectors {
            let f = v["fn"].as_str().unwrap();
            let arg = |i: usize| v["args"][i].as_f64();
            let got = match f {
                "temp" => temp(arg(0)),
                "rpm" => rpm(arg(0)),
                "percent" => percent(arg(0)),
                "watts" => watts(arg(0)),
                "bytes" => bytes(arg(0)),
                "duration" => arg(0).map_or_else(|| UNKNOWN.to_owned(), duration),
                "signed" => signed(arg(0).unwrap(), v["args"][1].as_str().unwrap()),
                "capitalize" => capitalize(v["args"][0].as_str().unwrap()),
                other => panic!("unknown function {other}"),
            };
            assert_eq!(got, v["expect"].as_str().unwrap(), "{v}");
        }
    }

    #[test]
    fn capitalize_handles_empty_and_unicode() {
        assert_eq!(capitalize("balanced"), "Balanced");
        assert_eq!(capitalize(""), "");
        assert_eq!(capitalize("élan"), "Élan");
    }
}
