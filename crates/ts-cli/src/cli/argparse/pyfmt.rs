//! The few Python text conventions that leak into messages: `repr()` of a string, what
//! `int()` and `float()` accept, and how a float is written.
#![forbid(unsafe_code)]

/// Python's `repr()` of a string: single quotes unless the text has a `'` and no `"`.
pub(crate) fn repr_str(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Removes the underscores Python allows between digits (`1_000`); `None` when one is misplaced.
fn strip_underscores(text: &str) -> Option<String> {
    if !text.contains('_') {
        return Some(text.to_owned());
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (i, c) in chars.iter().enumerate() {
        if *c == '_' {
            let before = i.checked_sub(1).and_then(|j| chars.get(j)).is_some_and(char::is_ascii_digit);
            let after = chars.get(i + 1).is_some_and(char::is_ascii_digit);
            if !(before && after) {
                return None;
            }
        } else {
            out.push(*c);
        }
    }
    Some(out)
}

/// What `int(text)` accepts: optional whitespace, a sign, digits with single underscores.
/// Values beyond 64 bits are refused (Python would take them; no setting needs them).
pub(crate) fn parse_int(text: &str) -> Option<i64> {
    let t = text.trim();
    let digits = t.strip_prefix(['+', '-']).unwrap_or(t);
    if digits.is_empty() || !digits.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    strip_underscores(t)?.parse::<i64>().ok()
}

/// What `float(text)` accepts: whitespace, a sign, digits with a point and an exponent,
/// underscores between digits, and `inf` / `nan`.
pub(crate) fn parse_float(text: &str) -> Option<f64> {
    let t = strip_underscores(text.trim())?;
    let body = t.strip_prefix(['+', '-']).unwrap_or(&t);
    let word = body.to_ascii_lowercase();
    let valid_word = matches!(word.as_str(), "inf" | "infinity" | "nan");
    let numeric = body.starts_with(|c: char| c.is_ascii_digit() || c == '.')
        && body.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'));
    if !(valid_word || numeric) {
        return None;
    }
    t.parse::<f64>().ok()
}

/// A float the way Python's `repr(float)` writes it: the shortest text that reads back, in
/// scientific form below 1e-4 and from 1e16 (`1e-05`, `1e+16`), with `.0` on whole numbers.
pub(crate) fn float_repr(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    if value == 0.0 {
        return if value.is_sign_negative() { "-0.0" } else { "0.0" }.to_owned();
    }
    // `{:e}` gives the shortest digits that round-trip: d.ddde<exp>.
    let sci = format!("{value:e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let negative = mantissa.starts_with('-');
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let sign = if negative { "-" } else { "" };
    if (-4..16).contains(&exp) {
        let point = exp + 1;
        let body = if point <= 0 {
            format!("0.{}{}", "0".repeat(usize::try_from(-point).unwrap_or(0)), digits)
        } else {
            let point = usize::try_from(point).unwrap_or(0);
            if digits.len() <= point {
                format!("{}{}.0", digits, "0".repeat(point - digits.len()))
            } else {
                format!("{}.{}", &digits[..point], &digits[point..])
            }
        };
        return format!("{sign}{body}");
    }
    let (first, rest) = digits.split_at(1);
    let frac = if rest.is_empty() { String::new() } else { format!(".{rest}") };
    let exp_sign = if exp < 0 { '-' } else { '+' };
    format!("{sign}{first}{frac}e{exp_sign}{:02}", exp.abs())
}

/// Python's `format(value, 'g')` for the small numbers the commands print (minutes, watts): six
/// significant digits, no trailing zeros, no `.0` on whole numbers.
pub(crate) fn float_g(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "inf" } else { "-inf" }.to_owned();
    }
    if value == 0.0 {
        return if value.is_sign_negative() { "-0" } else { "0" }.to_owned();
    }
    let sci = format!("{value:.5e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let trim = |s: &str| -> String {
        if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_owned() } else { s.to_owned() }
    };
    if (-4..6).contains(&exp) {
        // Fixed notation with `5 - exp` decimals, then trailing zeros dropped.
        let decimals = usize::try_from(5 - exp).unwrap_or(0);
        return trim(&format!("{value:.decimals$}"));
    }
    let exp_sign = if exp < 0 { '-' } else { '+' };
    format!("{}e{exp_sign}{:02}", trim(mantissa), exp.abs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repr_quotes_like_python() {
        assert_eq!(repr_str("foo"), "'foo'");
        assert_eq!(repr_str("it's"), "\"it's\"");
        assert_eq!(repr_str("a\"b'c"), "'a\"b\\'c'");
        assert_eq!(repr_str("tab\there\n"), "'tab\\there\\n'");
        assert_eq!(repr_str("\u{1}"), "'\\x01'");
        assert_eq!(repr_str(""), "''");
    }

    #[test]
    fn int_accepts_what_python_accepts() {
        for (text, want) in [
            ("5", Some(5)),
            (" 5 ", Some(5)),
            ("+5", Some(5)),
            ("-10", Some(-10)),
            ("1_000", Some(1000)),
            ("007", Some(7)),
            ("", None),
            ("-", None),
            ("1__0", None),
            ("_1", None),
            ("1_", None),
            ("5.0", None),
            ("0x10", None),
            ("abc", None),
            ("99999999999999999999", None),
        ] {
            assert_eq!(parse_int(text), want, "{text:?}");
        }
    }

    #[test]
    fn float_accepts_what_python_accepts() {
        for (text, want) in [
            ("5", Some(5.0)),
            ("5.5", Some(5.5)),
            (".5", Some(0.5)),
            ("5.", Some(5.0)),
            ("-1e3", Some(-1000.0)),
            ("1_0.5", Some(10.5)),
            (" 7 ", Some(7.0)),
            ("", None),
            ("e5", None),
            ("1e", None),
            ("abc", None),
            ("1,5", None),
        ] {
            assert_eq!(parse_float(text), want, "{text:?}");
        }
        assert!(parse_float("inf").is_some_and(f64::is_infinite));
        assert!(parse_float("-Infinity").is_some_and(f64::is_infinite));
        assert!(parse_float("nan").is_some_and(f64::is_nan));
        assert_eq!(parse_float("infx"), None);
    }

    #[test]
    fn float_repr_matches_python() {
        for (value, want) in [
            (1.0, "1.0"),
            (0.5, "0.5"),
            (30.0, "30.0"),
            (123456.789, "123456.789"),
            (1e16, "1e+16"),
            (1.5e16, "1.5e+16"),
            (1e15, "1000000000000000.0"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (1.25e-7, "1.25e-07"),
            (-2.5, "-2.5"),
            (0.1, "0.1"),
            (100.0, "100.0"),
            (0.0, "0.0"),
            (1e100, "1e+100"),
        ] {
            assert_eq!(float_repr(value), want, "{value}");
        }
    }

    #[test]
    fn float_g_matches_python() {
        for (value, want) in [
            (30.0, "30"),
            (0.5, "0.5"),
            (15.0, "15"),
            (1.5, "1.5"),
            (45.25, "45.25"),
            (123456.0, "123456"),
            (1234567.0, "1.23457e+06"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (0.1, "0.1"),
            (2.0 / 3.0, "0.666667"),
            (99.99, "99.99"),
        ] {
            assert_eq!(float_g(value), want, "{value}");
        }
    }
}
