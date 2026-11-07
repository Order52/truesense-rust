//! `keys`: what the PREDATOR key and Fn+Esc do, and the key binding profiles.
#![forbid(unsafe_code)]

use serde_json::{Value, json};
use ts_api::api::{Api, KeyBindings, KeyProfileEdit};
use ts_domain::keybinds::Gesture;

use super::{Cx, Fail, Obj, Outcome, fail};
use crate::cli::argparse::Matches;

/// The five gestures and two timings of a binding set as the Python dictionary listed them.
fn binding_rows(b: &KeyBindings) -> Vec<(&'static str, Value)> {
    vec![
        ("tap", json!(b.tap)),
        ("double", json!(b.double)),
        ("triple", json!(b.triple)),
        ("long", json!(b.long)),
        ("fn_esc", json!(b.fn_esc)),
        ("long_ms", json!(b.long_ms)),
        ("multi_ms", json!(b.multi_ms)),
    ]
}

/// The reference text of `keys actions`, its action lines indented by two spaces as they are in
/// the Python edition's docstring. (The constant in the domain layer lost that indentation to a
/// line continuation; a line that already has it is left alone.)
fn catalogue() -> String {
    let text = Api::key_actions();
    let mut out = Vec::new();
    let mut in_list = false;
    for line in text.lines() {
        if in_list && !line.is_empty() && !line.starts_with(' ') {
            out.push(format!("  {line}"));
        } else {
            out.push(line.to_owned());
        }
        if line.is_empty() {
            in_list = true;
        }
    }
    out.join("\n")
}

pub(super) fn keys(cx: &mut Cx<'_, '_>, verb: Option<&str>, m: &Matches) -> Outcome {
    let api = cx.api;
    match verb.unwrap_or("show") {
        "show" => {
            let settings = api.key_settings()?;
            let mut rows = binding_rows(&settings.bindings);
            rows.push(("profile", json!(settings.profile)));
            rows.push(("power_cycle", json!(settings.power_cycle)));
            cx.emit(&Obj(rows), Some(&settings.to_string()));
        }
        "actions" => cx.say(&catalogue()),
        "run" => return Ok(cx.launcher.key_service()),
        "set" => {
            let gesture = Api::parse_gesture(m.str("gesture").unwrap_or_default())?;
            let action = m.str("action").unwrap_or_default();
            api.set_key_action(gesture, action)?;
            cx.emit_text(&format!(
                "{} -> {action}   (the key service reloads by itself; save it as a profile: truesense keys profile save NAME)",
                gesture.key()
            ));
        }
        "reset" => {
            let gesture = m.str("gesture").map(Api::parse_gesture).transpose()?;
            let done = api.reset_keys(gesture)?;
            let names: Vec<&str> = done.iter().map(|g| g.key()).collect();
            cx.emit_text(&format!("reset: {}", names.join(", ")));
        }
        "test" => {
            let said = api.test_action(m.str("action").unwrap_or_default())?;
            cx.emit_text(if said.is_empty() { "ok" } else { &said });
        }
        "profile" => profile(cx, m)?,
        "timing" => timing(cx, m)?,
        other => return Err(fail(format!("unknown keys command {other}"))),
    }
    Ok(0)
}

fn timing(cx: &mut Cx<'_, '_>, m: &Matches) -> Result<(), Fail> {
    // Only a non-zero value counts as given (`if x`).
    let long = m.int("long_ms").filter(|v| *v != 0);
    let multi = m.int("multi_ms").filter(|v| *v != 0);
    if long.is_none() && multi.is_none() {
        let t = cx.api.key_timing()?;
        cx.emit(&t, None);
        return Ok(());
    }
    cx.api.set_key_timing(long, multi)?;
    let mut parts = Vec::new();
    if let Some(v) = long {
        parts.push(format!("'long_ms': {v}"));
    }
    if let Some(v) = multi {
        parts.push(format!("'multi_ms': {v}"));
    }
    cx.emit_text(&format!("timing: {{{}}}", parts.join(", ")));
    Ok(())
}

/// A key profile as the Python dictionary: the description, then the bindings.
fn view_json(view: &ts_api::api::KeyProfileView) -> Obj {
    let mut rows = vec![("description", json!(view.description))];
    rows.extend(binding_rows(&view.bindings));
    Obj(rows)
}

fn profile(cx: &mut Cx<'_, '_>, m: &Matches) -> Result<(), Fail> {
    let api = cx.api;
    let name = m.str("name");
    match m.str("action").unwrap_or("current") {
        "list" => {
            let list = api.key_profiles()?;
            cx.emit(&list.names, Some(&list.to_string()));
        }
        "current" => {
            let (profile, view) = api.key_profile_in_use()?;
            let mut rows = binding_rows(&view.bindings);
            rows.push(("profile", json!(profile)));
            cx.emit(&Obj(rows), Some(&format!("key profile in use: {profile}\n{view}")));
        }
        "show" => {
            let shown = name.unwrap_or("default");
            let view = api.key_profile(shown)?;
            cx.emit(&view_json(&view), Some(&format!("{shown}\n{view}")));
        }
        "use" => {
            let used = name.unwrap_or_default();
            api.use_key_profile(used)?;
            cx.emit(used, Some(&format!("key profile in use: {used}   (the key service reloads by itself)")));
        }
        "copy" => {
            let from = name.unwrap_or_default();
            let made = api.copy_key_profile(from, m.str("new"))?;
            cx.emit(
                &made,
                Some(&format!(
                    "copied '{from}' to '{made}' (edit: truesense keys profile save {made} --tap power:turbo)"
                )),
            );
        }
        "save" => {
            let target = name.unwrap_or_default();
            let mut actions = Vec::new();
            for (flag, gesture) in [
                ("tap", Gesture::Tap),
                ("double", Gesture::Double),
                ("triple", Gesture::Triple),
                ("long", Gesture::Long),
                ("fn_esc", Gesture::FnEsc),
            ] {
                if let Some(a) = m.str(flag) {
                    actions.push((gesture, a.to_owned()));
                }
            }
            let edit = KeyProfileEdit {
                from: m.str("base").map(str::to_owned),
                actions,
                long_ms: m.int("long_ms"),
                multi_ms: m.int("multi_ms"),
            };
            let saved = api.save_key_profile(target, &edit)?;
            cx.emit(
                &view_json(&saved),
                Some(&format!("saved key profile '{target}' (use it: truesense keys profile use {target})")),
            );
        }
        _ => {
            api.delete_key_profile(name.unwrap_or_default())?;
            let shown = name.unwrap_or("None");
            cx.emit(&name, Some(&format!("deleted key profile '{shown}'")));
        }
    }
    Ok(())
}
