//! `look` and `startup`: saved lighting looks and what happens at login or boot.
#![forbid(unsafe_code)]

use ts_foundation::config::{Value, section_table};

use super::{Cx, Fail, Outcome, fail};
use crate::cli::argparse::Matches;

pub(super) fn look(cx: &mut Cx<'_, '_>, verb: Option<&str>, m: &Matches) -> Outcome {
    let api = cx.api;
    let name = m.str("name").unwrap_or_default();
    match verb.unwrap_or("list") {
        "list" => {
            let names = api.looks();
            let text = if names.is_empty() {
                "(no saved looks; `truesense look save NAME`)".to_owned()
            } else {
                names.join("\n")
            };
            cx.emit(&names, Some(&text));
        }
        "save" => {
            let saved = api.save_look(name)?;
            cx.emit(&saved, Some(&format!("saved look '{name}': {}", saved.describe())));
        }
        "show" => {
            let shown = api.look(name)?;
            cx.emit(&shown, Some(&format!("{name}: {}", shown.describe())));
        }
        "apply" => {
            let lines = api.apply_look(name)?;
            cx.emit(&lines, Some(&lines.join("\n")));
        }
        "delete" => {
            api.delete_look(name)?;
            cx.emit(name, Some(&format!("deleted look '{name}'")));
        }
        other => return Err(fail(format!("unknown look command {other}"))),
    }
    Ok(0)
}

pub(super) fn startup(cx: &mut Cx<'_, '_>, verb: Option<&str>, m: &Matches) -> Outcome {
    let api = cx.api;
    match verb.unwrap_or("show") {
        "boot" => boot(cx, m.str("what"))?,
        "show" => {
            let s = api.startup();
            cx.emit(&s, None);
        }
        "apply" => {
            let lines = cx.launcher.startup_apply().map_err(fail)?;
            cx.emit(&lines, Some(&lines.join("\n")));
        }
        "set" => {
            let target = m.str("target").unwrap_or_default();
            let value = set_option(cx, target, m.str("value").unwrap_or_default())?;
            let shown = if value.is_empty() { "none" } else { value.as_str() };
            cx.emit(&value, Some(&format!("at login, {target}: {shown}")));
        }
        other => return Err(fail(format!("unknown startup command {other}"))),
    }
    Ok(0)
}

fn boot(cx: &mut Cx<'_, '_>, what: Option<&str>) -> Result<(), Fail> {
    let api = cx.api;
    let current = api.boot_look()?;
    match what {
        None => {
            let text = match &current {
                Some(look) => format!("at boot and after suspend: {}", look.describe()),
                None => "no look is applied at boot".to_owned(),
            };
            cx.emit(&current, Some(&text));
        }
        Some("clear") => {
            api.clear_boot_look()?;
            cx.emit(&(), Some("nothing is applied at boot any more"));
        }
        Some(name) => {
            let look = api.set_boot_look(name)?;
            cx.emit(&look, Some(&format!("at boot and after suspend: {}", look.describe())));
        }
    }
    Ok(())
}

/// `startup.set_option`: validates a login setting and writes it to the user's configuration.
fn set_option(cx: &mut Cx<'_, '_>, target: &str, value: &str) -> Result<String, Fail> {
    let api = cx.api;
    let val = if target == "profile" && value == "none" { "" } else { value };
    if target == "profile" && !val.is_empty() {
        let names = api.profile_names();
        if !names.iter().any(|n| n == val) {
            return Err(fail(format!("no profile {}: {}", py_repr(val), names.join(", "))));
        }
    }
    if target == "keyboard" {
        if let Some(name) = val.strip_prefix("profile:") {
            let names = api.lighting_profile_names();
            if !names.iter().any(|n| n == name) {
                return Err(fail(format!("no lighting profile {}: {}", py_repr(name), names.join(", "))));
            }
        }
    }
    if let Some(name) = val.strip_prefix("look:") {
        let names = api.looks();
        if !names.iter().any(|n| n == name) {
            return Err(fail(format!("no look {}: {}", py_repr(name), names.join(", "))));
        }
    }
    let mut cfg = api.load_config().map_err(fail)?;
    match target {
        "keyboard" => cfg.startup.keyboard = val.to_owned(),
        "rear" => cfg.startup.rear = val.to_owned(),
        _ => cfg.startup.profile = val.to_owned(),
    }
    if let Some(bad) = cfg.validate().into_iter().find(|p| p.contains("[startup]")) {
        return Err(fail(bad));
    }
    api.services().config.write_user(section_table("startup", [(target, Value::String(val.to_owned()))]))?;
    Ok(val.to_owned())
}

/// Python's `repr()` of a name, as it appears in these messages.
fn py_repr(text: &str) -> String {
    crate::cli::argparse::pyfmt::repr_str(text)
}
