//! Choosing what happens at login, validated the same way for the command line and the window.
#![forbid(unsafe_code)]

use ts_api::api::{Api, ApiError, ApiResult};
use ts_foundation::config::{Value, section_table};

/// The things that can be chosen, in the order the help lists them.
pub const TARGETS: [&str; 3] = ["keyboard", "rear", "profile"];

/// A name quoted the way Python's `repr` of a string does (single quotes, unless the name holds a
/// single quote and no double quote), because the messages of the earlier editions used `{x!r}`
/// and scripts match on them.
pub fn py_repr(text: &str) -> String {
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
            c if c.is_control() => out.push_str(&format!("\\x{:02x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// A list of names for a message, `a, b, c`.
fn list(names: &[String]) -> String {
    names.join(", ")
}

/// Sets what happens at login: `target` is `keyboard`, `rear` or `profile` (the value `none`
/// clears the profile). The profile, lighting profile or look must exist and the resulting
/// configuration must be valid; nothing is written otherwise. Answers the stored value.
///
/// # Errors
///
/// [`ApiError::Invalid`] with the message the earlier editions printed when the request is wrong,
/// or the configuration's own error when the file cannot be read or written.
pub fn set_option(api: &Api, target: &str, value: &str) -> ApiResult<String> {
    if !TARGETS.contains(&target) {
        return Err(ApiError::invalid("target: keyboard | rear | profile"));
    }
    let services = api.services();
    let val = if target == "profile" && value == "none" { "" } else { value };
    if target == "profile" && !val.is_empty() {
        let names = services.bundles.names();
        if !names.iter().any(|n| n == val) {
            return Err(ApiError::invalid(format!("no profile {}: {}", py_repr(val), list(&names))));
        }
    }
    if target == "keyboard"
        && let Some(name) = val.strip_prefix("profile:")
        && !services.lighting.exists(name)
    {
        return Err(ApiError::invalid(format!(
            "no lighting profile {}: {}",
            py_repr(name),
            list(&services.lighting.names())
        )));
    }
    if let Some(name) = val.strip_prefix("look:") {
        let names = services.looks.names();
        if !names.iter().any(|n| n == name) {
            return Err(ApiError::invalid(format!("no look {}: {}", py_repr(name), list(&names))));
        }
    }
    let mut cfg = api.load_config()?;
    match target {
        "keyboard" => cfg.startup.keyboard = val.to_owned(),
        "rear" => cfg.startup.rear = val.to_owned(),
        _ => cfg.startup.profile = val.to_owned(),
    }
    if let Some(problem) = cfg.validate().into_iter().find(|p| p.contains("[startup]")) {
        return Err(ApiError::invalid(problem));
    }
    services.config.write_user(section_table("startup", [(target, Value::String(val.to_owned()))]))?;
    Ok(val.to_owned())
}
