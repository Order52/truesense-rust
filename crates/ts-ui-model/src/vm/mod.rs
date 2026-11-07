//! View-model modules: the decisions of each page, in Rust, tested without Qt.
//!
//! A page agent adds `src/vm/<page>.rs` with
//!
//! ```ignore
//! //! What the page decides ...
//! pub fn call(verb: &str, args: &serde_json::Value) -> Result<serde_json::Value, String>
//! ```
//!
//! and QML calls it as `Bus.vm("<page>", "<verb>", JSON.stringify(args))`, which returns the JSON
//! text of the result (synchronous, pure: no I/O, no clocks; the page passes in everything it
//! needs, usually the live frame or a command result). `build.rs` finds the file by name, so
//! neither this file nor `lib.rs` needs an edit. [`fmt`] is the shared formatting module.
//!
//! A view-model must be a pure function of its arguments so that its tests are plain
//! `assert_eq!` over JSON, and so that the same page logic can be reused by a future terminal UI.
#![forbid(unsafe_code)]

include!(concat!(env!("OUT_DIR"), "/vm_gen.rs"));

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn modules_are_discovered_and_dispatch_reaches_them() {
        assert!(MODULES.contains(&"fmt"));
        assert_eq!(dispatch("fmt", "temp", &json!({"v": 54})), Ok(json!("54 °C")));
    }

    #[test]
    fn unknown_modules_and_verbs_are_errors_with_text() {
        assert!(dispatch("nope", "x", &json!({})).unwrap_err().contains("nope"));
        assert!(dispatch("fmt", "nope", &json!({})).unwrap_err().contains("nope"));
    }
}
