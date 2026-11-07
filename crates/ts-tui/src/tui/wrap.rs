//! Word wrapping for the dialogs, the health check lines and the help.

/// Breaks `text` into lines of at most `n` cells at spaces; a word longer than a line is cut.
/// Runs of spaces inside a line are kept, spaces at the ends of lines are dropped, and any white
/// space (tabs, newlines) counts as one space.
pub(crate) fn wrap(text: &str, n: usize) -> Vec<String> {
    let n = n.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut cur: Vec<char> = Vec::new();
    let flush = |cur: &mut Vec<char>, lines: &mut Vec<String>| {
        let line: String = cur.iter().collect();
        let line = line.trim_end_matches(' ');
        if !line.is_empty() {
            lines.push(line.to_owned());
        }
        cur.clear();
    };
    for chunk in chunks(text) {
        let mut word: Vec<char> = chunk.chars().collect();
        if word.first() == Some(&' ') {
            if !cur.is_empty() {
                cur.extend(word);
            }
            continue;
        }
        while cur.len() + word.len() > n {
            if !cur.is_empty() {
                flush(&mut cur, &mut lines);
                continue;
            }
            let rest = word.split_off(n);
            cur.extend(word);
            word = rest;
            flush(&mut cur, &mut lines);
        }
        cur.extend(word);
    }
    flush(&mut cur, &mut lines);
    lines
}

/// Alternating runs of spaces and non-spaces; every kind of white space becomes a space.
fn chunks(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut space = false;
    for (i, c) in text.chars().enumerate() {
        let is_space = c.is_whitespace();
        if i > 0 && is_space != space {
            out.push(std::mem::take(&mut cur));
        }
        space = is_space;
        cur.push(if is_space { ' ' } else { c });
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn text_is_wrapped_at_spaces_and_long_words_are_cut() {
        let table: [(&str, usize, &[&str]); 9] = [
            ("", 10, &[]),
            ("hello", 10, &["hello"]),
            ("hello world", 5, &["hello", "world"]),
            ("hello world", 11, &["hello world"]),
            ("aaaaaaaaaa", 4, &["aaaa", "aaaa", "aa"]),
            ("a  b", 10, &["a  b"]),
            ("  \t\n x  y ", 4, &["x  y"]),
            ("  \t\n x  y ", 3, &["x", "y"]),
            ("one two three four", 9, &["one two", "three", "four"]),
        ];
        for (text, n, want) in table {
            assert_eq!(wrap(text, n), want, "{text:?} at {n}");
        }
    }

    #[test]
    fn no_line_is_wider_than_asked_and_no_word_character_is_lost() {
        let samples = [
            "Stop BOTH fans? The service brings them back at CPU 70 °C / GPU 65 °C.",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "  \t\n x  y ",
            "ünï cödé wörds ✓ and more",
        ];
        for text in samples {
            for n in 1..40 {
                let lines = wrap(text, n);
                assert!(lines.iter().all(|l| l.chars().count() <= n), "{text:?} at {n}: {lines:?}");
                let got: String = lines.concat().split_whitespace().collect();
                let want: String = text.split_whitespace().collect();
                assert_eq!(got, want, "{text:?} at {n}");
            }
        }
    }

    #[test]
    fn a_width_of_zero_does_not_loop_or_panic() {
        assert_eq!(wrap("abc", 0), vec!["a", "b", "c"]);
    }
}
