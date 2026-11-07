//! What a command prints: the standard streams, and the Python-compatible JSON writer.
//!
//! `--json` output has been read by scripts for years, so it keeps the Python spelling:
//! two-space indent, `ensure_ascii` escapes (`é`), floats as `repr` writes them (`1.0`,
//! `1e-05`) and keys in the order the value lists them.
#![forbid(unsafe_code)]

use std::io::{self, BufRead, Write};

use serde::Serialize;
use serde_json::ser::{Formatter, PrettyFormatter};

use super::argparse::pyfmt::float_repr;

/// The standard streams a run reads and writes. The binary passes the real ones; tests pass
/// buffers.
pub struct Io<'a> {
    /// Standard output.
    pub out: &'a mut dyn Write,
    /// Standard error.
    pub err: &'a mut dyn Write,
    /// Standard input (the one question the command line asks is the confirmation of
    /// `update apply`).
    pub input: &'a mut dyn BufRead,
    /// The width of the terminal, which the help pages are laid out for (80 when unknown).
    pub columns: usize,
    /// Whether standard output is a terminal (the health check colours its marks then).
    pub colour: bool,
}

impl<'a> Io<'a> {
    /// Streams with an 80 column, colourless terminal.
    pub fn new(out: &'a mut dyn Write, err: &'a mut dyn Write, input: &'a mut dyn BufRead) -> Self {
        Self { out, err, input, columns: 80, colour: false }
    }
}

/// `json.dumps` formatting: `PrettyFormatter` with `ensure_ascii` strings and Python floats.
struct PyFormatter {
    inner: PrettyFormatter<'static>,
}

impl PyFormatter {
    fn new() -> Self {
        Self { inner: PrettyFormatter::with_indent(b"  ") }
    }
}

macro_rules! forward {
    ($($name:ident($($arg:ident: $ty:ty),*);)+) => {$(
        fn $name<W: ?Sized + Write>(&mut self, writer: &mut W $(, $arg: $ty)*) -> io::Result<()> {
            self.inner.$name(writer $(, $arg)*)
        }
    )+};
}

impl Formatter for PyFormatter {
    forward! {
        begin_array();
        end_array();
        begin_array_value(first: bool);
        end_array_value();
        begin_object();
        end_object();
        begin_object_key(first: bool);
        begin_object_value();
        end_object_value();
    }

    fn write_f64<W: ?Sized + Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        writer.write_all(float_repr(value).as_bytes())
    }

    fn write_f32<W: ?Sized + Write>(&mut self, writer: &mut W, value: f32) -> io::Result<()> {
        writer.write_all(float_repr(f64::from(value)).as_bytes())
    }

    fn write_string_fragment<W: ?Sized + Write>(&mut self, writer: &mut W, fragment: &str) -> io::Result<()> {
        let mut plain = String::new();
        for c in fragment.chars() {
            if (c as u32) < 0x7f {
                plain.push(c);
                continue;
            }
            writer.write_all(plain.as_bytes())?;
            plain.clear();
            let mut units = [0u16; 2];
            for unit in c.encode_utf16(&mut units) {
                write!(writer, "\\u{unit:04x}")?;
            }
        }
        writer.write_all(plain.as_bytes())
    }
}

/// `json.dumps(value, indent=2)` of anything serialisable.
pub(super) fn dumps<T: Serialize + ?Sized>(value: &T) -> String {
    let mut buffer = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, PyFormatter::new());
    match value.serialize(&mut serializer) {
        Ok(()) => String::from_utf8(buffer).unwrap_or_default(),
        // A value that cannot be written (it never happens for the typed results): keep the
        // output valid JSON and say so.
        Err(e) => format!("{{\"error\": {}}}", dumps(&e.to_string())),
    }
}

/// `json.dumps(value)` on one line (the default separators `, ` and `: `), used by the one
/// machine-readable answer of the command line that is not `--json` output.
pub(super) fn dumps_inline<T: Serialize + ?Sized>(value: &T) -> String {
    let pretty = dumps(value);
    // Collapse the layout of the pretty form: only whitespace outside strings is touched.
    let mut out = String::with_capacity(pretty.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut chars = pretty.chars().peekable();
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '\n' => {
                while chars.peek() == Some(&' ') {
                    chars.next();
                }
                // After a comma the single space of `, ` is wanted; after a bracket nothing.
                if out.ends_with(',') {
                    out.push(' ');
                }
            }
            c => out.push(c),
        }
    }
    out
}

/// The text `print()` shows for a value that is not a list or a dictionary, given its JSON form;
/// `None` for a value `emit` prints nothing for (`null`).
pub(super) fn plain_text(json: &str) -> Option<String> {
    let trimmed = json.trim();
    match trimmed {
        "null" => None,
        "true" => Some("on".to_owned()),
        "false" => Some("off".to_owned()),
        _ if trimmed.starts_with('"') => serde_json::from_str::<String>(trimmed).ok(),
        _ => Some(trimmed.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn json_is_written_like_python_dumps_with_indent_two() {
        let value = json!({"a": 1, "b": [], "c": {}, "d": [1, 2], "e": null, "f": true});
        // serde_json sorts the keys of a `json!` object; the layout is what is checked here.
        assert_eq!(
            dumps(&value),
            "{\n  \"a\": 1,\n  \"b\": [],\n  \"c\": {},\n  \"d\": [\n    1,\n    2\n  ],\n  \"e\": null,\n  \"f\": true\n}"
        );
    }

    #[test]
    fn non_ascii_is_escaped_as_python_does() {
        assert_eq!(dumps("caf\u{e9}"), "\"caf\\u00e9\"");
        assert_eq!(dumps("\u{1f980}"), "\"\\ud83e\\udd80\"");
        assert_eq!(dumps("a\u{7f}b"), "\"a\\u007fb\"");
        assert_eq!(dumps("tab\t\"q\"\\\n"), "\"tab\\t\\\"q\\\"\\\\\\n\"");
    }

    #[test]
    fn floats_are_written_like_repr() {
        assert_eq!(dumps(&1.0_f64), "1.0");
        assert_eq!(dumps(&0.00001_f64), "1e-05");
        assert_eq!(dumps(&45.5_f64), "45.5");
        assert_eq!(dumps(&json!([80.0, 1e16])), "[\n  80.0,\n  1e+16\n]");
    }

    #[test]
    fn the_inline_form_has_python_separators() {
        let steps = vec![("restarted the daemon".to_owned(), true), ("a, \"b\"".to_owned(), false)];
        assert_eq!(dumps_inline(&steps), "[[\"restarted the daemon\", true], [\"a, \\\"b\\\"\", false]]");
        assert_eq!(dumps_inline(&Vec::<String>::new()), "[]");
    }

    #[test]
    fn plain_text_follows_print() {
        assert_eq!(plain_text("null"), None);
        assert_eq!(plain_text("true").as_deref(), Some("on"));
        assert_eq!(plain_text("false").as_deref(), Some("off"));
        assert_eq!(plain_text("\"hi\\nthere\"").as_deref(), Some("hi\nthere"));
        assert_eq!(plain_text("30").as_deref(), Some("30"));
        assert_eq!(plain_text("1.5").as_deref(), Some("1.5"));
        assert_eq!(plain_text("\"\"").as_deref(), Some(""));
    }
}
