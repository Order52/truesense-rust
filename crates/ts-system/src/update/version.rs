//! Release numbers and the shape of the repository name.
#![forbid(unsafe_code)]

use std::fmt;

/// A release number: `major.minor.patch`, nothing else (no pre-releases, no build metadata),
/// because the release pipeline only ever tags plain versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    /// The first number.
    pub major: u64,
    /// The second number.
    pub minor: u64,
    /// The third number.
    pub patch: u64,
}

impl Version {
    /// Reads `0.3.17` or `v0.3.17` (surrounding blanks are ignored). `None` for anything else,
    /// including a number too large for a release.
    ///
    /// Only these plain tags are ever downloaded, so what is parsed here is also what may end up
    /// in a URL: ASCII digits and dots, nothing that could climb out of the release path.
    pub fn parse(tag: &str) -> Option<Self> {
        let text = tag.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let mut numbers = text.split('.').map(|part| {
            (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
                .then(|| part.parse::<u64>().ok())
                .flatten()
        });
        let version = Self { major: numbers.next()??, minor: numbers.next()??, patch: numbers.next()?? };
        numbers.next().is_none().then_some(version)
    }
}

impl fmt::Display for Version {
    /// The canonical form without a `v` (`0.3.17`), the one used in file names.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Whether `repo` looks like `owner/name`. The repository ends up in URLs, so nothing else
/// passes: no second slash, no query, and neither part may be `.` or `..` (`../..` fits the
/// character set but would climb out of `/repos/`).
pub fn valid_repo(repo: &str) -> bool {
    let plain = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    };
    repo.split_once('/').is_some_and(|(owner, name)| plain(owner) && plain(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(major: u64, minor: u64, patch: u64) -> Version {
        Version { major, minor, patch }
    }

    #[test]
    fn versions_parse_with_or_without_v_and_blanks() {
        assert_eq!(Version::parse("v1.2.3"), Some(v(1, 2, 3)));
        assert_eq!(Version::parse("1.2.3"), Some(v(1, 2, 3)));
        assert_eq!(Version::parse("  v0.3.17\n"), Some(v(0, 3, 17)));
        assert_eq!(Version::parse("v0.10.0"), Some(v(0, 10, 0)));
        assert_eq!(Version::parse("007.01.0"), Some(v(7, 1, 0)));
    }

    #[test]
    fn anything_but_a_plain_version_is_refused() {
        for bad in [
            "",
            "main",
            "v",
            "1.2",
            "1.2.3.4",
            "v1.2.3-rc1",
            "1.2.3+build",
            "V1.2.3",
            "vv1.2.3",
            "1..3",
            ".1.2",
            "1.2.",
            "1.2.x",
            "-1.2.3",
            "+1.2.3",
            "1.2.3 4",
            "1. 2.3",
            "../../x",
            "1.2.3/../..",
            "v1.2.3\u{0}",
            "١.٢.٣",
            "99999999999999999999.0.0",
            "1_0.2.3",
        ] {
            assert_eq!(Version::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn versions_compare_numerically_not_textually() {
        assert!(Version::parse("v0.10.0") > Version::parse("v0.9.9"));
        assert!(v(1, 0, 0) > v(0, 99, 99));
        assert!(v(0, 3, 17) > v(0, 3, 9));
        assert_eq!(v(0, 5, 0), v(0, 5, 0));
        assert!(v(0, 3, 16) <= v(0, 3, 16));
        assert_eq!(v(0, 3, 17).to_string(), "0.3.17");
    }

    #[test]
    fn repository_names_are_checked_because_they_end_up_in_urls() {
        for ok in ["Order52/truesense", "o/r", "a-b/c_d.e", "owner/name.git", "1/2"] {
            assert!(valid_repo(ok), "{ok}");
        }
        for bad in [
            "",
            "owner",
            "/name",
            "owner/",
            "a/b/c",
            "../..",
            "./x",
            "x/..",
            "../x",
            "ow ner/name",
            "o/r?x=1",
            "o/r#f",
            "o/r\n",
            "o\\x/r",
            "ö/r",
            "o/r%2f",
            "o:x/r",
            "o@x/r",
            "https://evil/x",
        ] {
            assert!(!valid_repo(bad), "{bad:?}");
        }
    }
}
