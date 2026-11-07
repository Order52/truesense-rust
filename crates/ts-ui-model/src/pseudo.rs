//! The pseudo-locale: every translatable text accented, padded by about 40 % and bracketed, so
//! that a glance at the window shows (a) which strings do not go through `qsTrId` (they stay
//! plain) and (b) which layouts break when a language needs more room (German).
//!
//! This file depends on `std` only: the Qt app's `build.rs` includes it with `#[path]` to turn
//! `i18n/en.ts` into `pseudo.ts`, and the library tests it. Placeholders (`%1`, `%n`, `%L1`) are
//! kept byte for byte, and XML entities are decoded before and encoded after, so the output is a
//! valid `.ts` file.
#![forbid(unsafe_code)]

fn accent(c: char) -> char {
    match c {
        'a' => 'á',
        'b' => 'ƀ',
        'c' => 'ç',
        'd' => 'ð',
        'e' => 'é',
        'f' => 'ƒ',
        'g' => 'ĝ',
        'h' => 'ĥ',
        'i' => 'í',
        'j' => 'ĵ',
        'k' => 'ķ',
        'l' => 'ļ',
        'n' => 'ñ',
        'o' => 'ö',
        'p' => 'þ',
        'r' => 'ŕ',
        's' => 'š',
        't' => 'ţ',
        'u' => 'ü',
        'w' => 'ŵ',
        'y' => 'ý',
        'z' => 'ž',
        'A' => 'Å',
        'C' => 'Ç',
        'D' => 'Ð',
        'E' => 'É',
        'I' => 'Í',
        'N' => 'Ñ',
        'O' => 'Ö',
        'S' => 'Š',
        'U' => 'Ü',
        'Y' => 'Ý',
        other => other,
    }
}

/// The pseudo-localised form of one source text.
pub fn pseudo(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::from("⟦");
    let mut i = 0;
    let mut letters = 0usize;
    while i < chars.len() {
        if chars[i] == '%' {
            // %1, %n, %L1: copy the whole placeholder untouched.
            let mut j = i + 1;
            if chars.get(j) == Some(&'L') {
                j += 1;
            }
            if chars.get(j).is_some_and(|c| c.is_ascii_digit() || *c == 'n') {
                j += 1;
                out.extend(&chars[i..j]);
                i = j;
                continue;
            }
        }
        letters += usize::from(chars[i].is_alphabetic());
        out.push(accent(chars[i]));
        i += 1;
    }
    out.push_str(&"·".repeat((letters * 2).div_ceil(5)));
    out.push('⟧');
    out
}

fn decode(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn encode(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Turns the text of a Qt Linguist `.ts` file into its pseudo-locale: every `<translation>` after a
/// `<source>` becomes the pseudo form of that source, and the language is set to `pseudo`.
pub fn pseudo_ts(ts: &str) -> String {
    let mut out = String::with_capacity(ts.len() * 2);
    let mut rest = ts;
    while let Some(s_start) = rest.find("<source>") {
        let after = &rest[s_start + "<source>".len()..];
        let Some(s_len) = after.find("</source>") else { break };
        let source = decode(&after[..s_len]);
        let tail = &after[s_len + "</source>".len()..];
        let Some(t_start) = tail.find("<translation") else { break };
        let from_t = &tail[t_start..];
        let t_end = if let Some(end) = from_t.find("</translation>") {
            // `<translation ...>` holds text only when it is not self-closing.
            let open_end = from_t.find('>').unwrap_or(0);
            if from_t[..open_end].ends_with('/') { open_end + 1 } else { end + "</translation>".len() }
        } else {
            from_t.find("/>").map_or(from_t.len(), |e| e + 2)
        };
        out.push_str(&rest[..s_start + "<source>".len()]);
        out.push_str(&encode(&decode(&after[..s_len])));
        out.push_str("</source>");
        out.push_str(&tail[..t_start]);
        out.push_str("<translation>");
        out.push_str(&encode(&pseudo(&source)));
        out.push_str("</translation>");
        rest = &from_t[t_end..];
    }
    out.push_str(rest);
    set_language(&out, "pseudo")
}

/// Sets `language="..."` on the `<TS>` element.
fn set_language(ts: &str, language: &str) -> String {
    let Some(start) = ts.find("<TS ") else { return ts.to_owned() };
    let Some(end) = ts[start..].find('>') else { return ts.to_owned() };
    let header = &ts[start..start + end];
    let new_header = match header.find("language=\"") {
        Some(l) => {
            let value_start = l + "language=\"".len();
            let value_end = header[value_start..].find('"').map_or(header.len(), |q| value_start + q);
            format!("{}{language}{}", &header[..value_start], &header[value_end..])
        }
        None => format!("{header} language=\"{language}\""),
    };
    format!("{}{}{}", &ts[..start], new_header, &ts[start + end..])
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn text_is_accented_padded_and_bracketed() {
        let p = pseudo("Settings");
        assert!(p.starts_with('⟦') && p.ends_with('⟧'), "{p}");
        assert!(p.contains("Šéţţíñĝš"), "{p}");
        assert!(p.chars().count() > "Settings".chars().count() + 2);
    }

    #[test]
    fn placeholders_survive_untouched() {
        let p = pseudo("Hold left: %1 of %n minutes (%L2)");
        for ph in ["%1", "%n", "%L2"] {
            assert!(p.contains(ph), "{p}");
        }
        assert_eq!(pseudo("100%"), "⟦100%⟧");
    }

    #[test]
    fn expansion_is_about_forty_percent_of_the_letters() {
        let long = "abcdefghij".repeat(10);
        let extra = pseudo(&long).matches('·').count();
        assert_eq!(extra, 40);
        assert_eq!(pseudo(""), "⟦⟧");
    }

    const TS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<TS version="2.1" language="en">
<context>
    <name>qml</name>
    <message id="a">
        <source>Fish &amp; chips</source>
        <translation>Fish &amp; chips</translation>
    </message>
    <message id="b">
        <source>Open %1</source>
        <translation type="unfinished"></translation>
    </message>
    <message id="c">
        <source>Done</source>
        <translation type="unfinished"/>
    </message>
</context>
</TS>
"#;

    #[test]
    fn a_ts_file_becomes_a_valid_pseudo_ts_with_sources_kept() {
        let out = pseudo_ts(TS);
        assert!(out.contains(r#"language="pseudo""#), "{out}");
        assert!(out.contains("<source>Fish &amp; chips</source>"), "{out}");
        assert!(out.contains("<translation>⟦Fíšĥ &amp; çĥíþš"), "{out}");
        assert!(out.contains("<translation>⟦Ö"), "{out}");
        assert!(out.contains("Open") || out.contains("<source>Open %1</source>"));
        assert!(out.contains("%1"));
        assert_eq!(out.matches("<translation>").count(), 3);
        assert!(!out.contains("unfinished"), "{out}");
        assert_eq!(out.matches("</message>").count(), 3);
    }

    #[test]
    fn a_file_without_messages_or_header_is_passed_through() {
        assert_eq!(pseudo_ts("nothing here"), "nothing here");
        assert!(pseudo_ts("<TS version=\"2.1\"></TS>").contains("language=\"pseudo\""));
    }
}
