//! Python's `textwrap.wrap` (default settings), which `argparse` uses to fit a help text into its
//! column. The way it breaks after a hyphen inside a word ("PredatorSense-" / "like") is visible in
//! the help pages, so the word splitting follows `TextWrapper.wordsep_re` exactly.
#![forbid(unsafe_code)]

/// `[\w!"'&.,?]`: a character a word may end in before a dash.
fn is_word_punct(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || matches!(c, '!' | '"' | '\'' | '&' | '.' | ',' | '?')
}

/// `[^\d\W]`: a letter (or underscore).
fn is_letter(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Splits the text into chunks of whitespace and word pieces, breaking words after a hyphen where
/// `textwrap` allows it.
fn split_chunks(text: &str) -> Vec<String> {
    let s: Vec<char> = text.chars().collect();
    let n = s.len();
    let mut chunks = Vec::new();
    let mut i = 0;
    while i < n {
        if s[i].is_whitespace() {
            let start = i;
            while i < n && s[i].is_whitespace() {
                i += 1;
            }
            chunks.push(s[start..i].iter().collect());
            continue;
        }
        // An em-dash between words: `(?<=wp) -{2,} (?=\w)`.
        if s[i] == '-' && i > 0 && is_word_punct(s[i - 1]) {
            let mut j = i;
            while j < n && s[j] == '-' {
                j += 1;
            }
            if j - i >= 2 && j < n && is_word(s[j]) {
                chunks.push(s[i..j].iter().collect());
                i = j;
                continue;
            }
        }
        // A word: extend one character at a time until one of the endings applies.
        let start = i;
        let mut e = i + 1;
        loop {
            if e >= n {
                break;
            }
            // Hyphenated word: a hyphen next, after two letters or letter-hyphen-letter.
            if s[e] == '-' {
                let behind_two = e >= 2 && is_letter(s[e - 1]) && is_letter(s[e - 2]);
                let behind_pattern = e >= 3 && is_letter(s[e - 1]) && s[e - 2] == '-' && is_letter(s[e - 3]);
                if (behind_two || behind_pattern) && hyphen_lookahead(&s, e + 1) {
                    e += 1;
                    break;
                }
            }
            // End of word.
            if s[e].is_whitespace() {
                break;
            }
            // Em-dash after the word: `(?<=wp) (?=-{2,}\w)`.
            if is_word_punct(s[e - 1]) && s[e] == '-' {
                let mut j = e;
                while j < n && s[j] == '-' {
                    j += 1;
                }
                if j - e >= 2 && j < n && is_word(s[j]) {
                    break;
                }
            }
            e += 1;
        }
        chunks.push(s[start..e].iter().collect());
        i = e;
    }
    chunks
}

/// `(?= [^\d\W] -? [^\d\W])` at `at`.
fn hyphen_lookahead(s: &[char], at: usize) -> bool {
    let Some(first) = s.get(at) else { return false };
    if !is_letter(*first) {
        return false;
    }
    let mut next = at + 1;
    if s.get(next) == Some(&'-') {
        next += 1;
    }
    s.get(next).is_some_and(|c| is_letter(*c))
}

fn is_blank(chunk: &str) -> bool {
    chunk.trim().is_empty()
}

fn len(chunk: &str) -> usize {
    chunk.chars().count()
}

/// Wraps `text` (already whitespace-normalised) to `width` columns.
pub(crate) fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut chunks = split_chunks(text);
    chunks.reverse();
    let mut lines: Vec<String> = Vec::new();
    while !chunks.is_empty() {
        let mut cur: Vec<String> = Vec::new();
        let mut cur_len = 0usize;
        if !lines.is_empty() && chunks.last().is_some_and(|c| is_blank(c)) {
            chunks.pop();
        }
        while let Some(last) = chunks.last() {
            let l = len(last);
            if cur_len + l <= width {
                cur_len += l;
                if let Some(c) = chunks.pop() {
                    cur.push(c);
                }
            } else {
                break;
            }
        }
        if chunks.last().is_some_and(|c| len(c) > width) {
            handle_long_word(&mut chunks, &mut cur, cur_len, width);
        }
        if cur.last().is_some_and(|c| is_blank(c)) {
            cur.pop();
        }
        if !cur.is_empty() {
            lines.push(cur.concat());
        }
    }
    lines
}

/// Breaks a word that is longer than a whole line (never the case for the help texts, but the
/// behaviour is `textwrap`'s).
fn handle_long_word(chunks: &mut Vec<String>, cur: &mut Vec<String>, cur_len: usize, width: usize) {
    let space_left = if width < 1 { 1 } else { width.saturating_sub(cur_len) };
    let Some(chunk) = chunks.pop() else { return };
    let chars: Vec<char> = chunk.chars().collect();
    let mut end = space_left.min(chars.len());
    if chars.len() > space_left {
        // Prefer the last hyphen inside the space that is left.
        let hyphen = chars[..space_left.min(chars.len())].iter().rposition(|c| *c == '-');
        if let Some(h) = hyphen.filter(|h| *h > 0 && chars[..*h].iter().any(|c| *c != '-')) {
            end = h + 1;
        }
    }
    cur.push(chars[..end].iter().collect());
    let rest: String = chars[end..].iter().collect();
    if !rest.is_empty() {
        chunks.push(rest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breaks_after_a_hyphen_inside_a_word() {
        let lines = wrap(
            "automatic power mode: off | native (PredatorSense-like: power source only) | smart (heat, load and battery aware); on = smart",
            52,
        );
        assert_eq!(lines[0], "automatic power mode: off | native (PredatorSense-");
        assert_eq!(lines[1], "like: power source only) | smart (heat, load and");
    }

    #[test]
    fn short_text_is_one_line() {
        assert_eq!(
            wrap("firmware control (0 rpm when cool)", 54),
            vec!["firmware control (0 rpm when cool)"]
        );
        assert_eq!(wrap("", 20), Vec::<String>::new());
    }

    #[test]
    fn breaks_a_very_long_word() {
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn does_not_start_a_line_with_a_space() {
        assert_eq!(wrap("aaa bbb ccc", 7), vec!["aaa bbb", "ccc"]);
    }
}
