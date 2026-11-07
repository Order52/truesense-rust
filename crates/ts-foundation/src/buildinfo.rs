//! Build identity: the release number, and optionally the git commit and build date.
//!
//! The version is the workspace version (`CARGO_PKG_VERSION`), so `truesense --version`, the
//! daemon's `version` reply, the update check and the doctor's "service and app are the same
//! version" test all read one number that `Cargo.toml` owns.
//!
//! The commit and the date cannot come from Cargo itself, and a build script would make the
//! build depend on a `git` binary and on the clock (breaking reproducible builds). They are
//! therefore injected from the outside through two environment variables that are read at
//! **compile time**:
//!
//! ```text
//! TRUESENSE_COMMIT=$(git rev-parse --short=12 HEAD) \
//! TRUESENSE_BUILD_DATE=$(date -u -d "@${SOURCE_DATE_EPOCH:-$(date +%s)}" +%Y-%m-%d) \
//!     cargo build --release
//! ```
//!
//! Deriving the date from `SOURCE_DATE_EPOCH` keeps the binary reproducible. Cargo notices a
//! change of either variable and rebuilds this crate. A plain `cargo build` leaves both unset
//! and they read as `None`; an empty variable counts as unset.
#![forbid(unsafe_code)]

/// The TrueSense release this source tree is (the workspace version).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The git commit of the build, or `None` for a build that did not set `TRUESENSE_COMMIT`.
pub(crate) const COMMIT: Option<&str> = non_empty(option_env!("TRUESENSE_COMMIT"));

/// The build date (UTC, `YYYY-MM-DD` by convention) or `None` when `TRUESENSE_BUILD_DATE` was not set.
pub(crate) const DATE: Option<&str> = non_empty(option_env!("TRUESENSE_BUILD_DATE"));

/// Treats an empty variable like an unset one, so `TRUESENSE_COMMIT= cargo build` (what a
/// failing `$(git ...)` in a script produces) does not print "commit ".
const fn non_empty(value: Option<&'static str>) -> Option<&'static str> {
    match value {
        Some(v) if !v.is_empty() => Some(v),
        _ => None,
    }
}

/// The one line `truesense --version` prints: `truesense 0.5.0`. It matches the Python
/// edition's output byte for byte because scripts compare it.
pub fn version_line() -> String {
    format!("truesense {VERSION}")
}

/// The detailed form for `truesense version --long` and bug reports:
/// `0.5.0 (commit abc123, built 2026-11-07)`, leaving out whichever part is unknown.
pub fn long_version() -> String {
    describe(VERSION, COMMIT, DATE)
}

/// Pure formatting behind [`long_version`], separate so it can be tested with every
/// combination of known and unknown parts.
fn describe(version: &str, commit: Option<&str>, date: Option<&str>) -> String {
    match (commit, date) {
        (None, None) => version.to_owned(),
        (Some(c), None) => format!("{version} (commit {c})"),
        (None, Some(d)) => format!("{version} (built {d})"),
        (Some(c), Some(d)) => format!("{version} (commit {c}, built {d})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_the_workspace_version() {
        assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
        assert!(VERSION.split('.').count() == 3 && VERSION.split('.').all(|p| p.parse::<u32>().is_ok()));
        assert_eq!(version_line(), format!("truesense {VERSION}"));
    }

    #[test]
    fn long_form_names_only_what_is_known() {
        assert_eq!(describe("1.2.3", None, None), "1.2.3");
        assert_eq!(describe("1.2.3", Some("abc"), None), "1.2.3 (commit abc)");
        assert_eq!(describe("1.2.3", None, Some("2026-11-07")), "1.2.3 (built 2026-11-07)");
        assert_eq!(
            describe("1.2.3", Some("abc"), Some("2026-11-07")),
            "1.2.3 (commit abc, built 2026-11-07)"
        );
    }

    #[test]
    fn empty_counts_as_unset() {
        assert_eq!(non_empty(None), None);
        assert_eq!(non_empty(Some("")), None);
        assert_eq!(non_empty(Some("abc")), Some("abc"));
    }
}
