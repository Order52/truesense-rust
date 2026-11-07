//! The one rule for the names of things people save: fan curves, smart profiles, undervolt
//! profiles, looks, bindings.
//!
//! A saved thing is a file named after it (`<name>.json`), so a name must never be able to build
//! a path: no separator, no `..` made of anything but dots that the file extension turns into an
//! ordinary name, no control character, no non-ASCII look-alike. The rule used to be written out
//! in eight places; a copy that drifted would have been a path-traversal bug, so there is one.
#![forbid(unsafe_code)]

/// The longest name, in characters.
pub const NAME_MAX: usize = 40;

/// What a name may contain, for messages: the same words everywhere.
pub const NAME_RULE: &str = "letters, digits, . _ - (max 40)";

/// Whether `name` is usable: one to [`NAME_MAX`] ASCII letters, digits, `.`, `_` or `-`.
#[must_use]
pub fn is_valid(name: &str) -> bool {
    (1..=NAME_MAX).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_names_are_valid() {
        for ok in ["a", "quiet", "my-curve_2", "v1.2", &"x".repeat(NAME_MAX)] {
            assert!(is_valid(ok), "{ok}");
        }
    }

    #[test]
    fn nothing_that_could_leave_the_directory_or_confuse_a_file_system_is() {
        for bad in [
            "",
            "a/b",
            "../x",
            "/etc/passwd",
            "a\\b",
            "a b",
            "a\n",
            "a\0b",
            "\u{e9}",
            "\u{ff21}",
            "x:y",
            &"x".repeat(NAME_MAX + 1),
        ] {
            assert!(!is_valid(bad), "{bad:?}");
        }
    }

    #[test]
    fn the_message_names_the_real_limit() {
        assert!(NAME_RULE.contains(&NAME_MAX.to_string()));
    }
}
