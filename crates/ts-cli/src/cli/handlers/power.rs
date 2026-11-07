//! `power`: the power mode, the automatic profile, holds, other power managers and game rules.
#![forbid(unsafe_code)]

use ts_api::api::{AutoAction, GameRuleAction, SMART_SETTING_KEYS, describe_smart_profile};
use ts_platform::hw::PowerMode;

use super::{Cx, Outcome, fail};
use crate::cli::argparse::Matches;
use crate::cli::argparse::pyfmt::float_g;

/// How long a mode picked from the command line is held against the automatic profile.
const HOLD_MINUTES: f64 = 30.0;

pub(super) fn power(cx: &mut Cx<'_, '_>, verb: Option<&str>, m: &Matches) -> Outcome {
    match verb.unwrap_or("status") {
        "status" => {
            let st = cx.api.power_status()?;
            cx.emit(&st, Some(&st.to_string()));
        }
        "list" => {
            let modes = cx.api.power_modes()?;
            let text = modes.iter().map(PowerMode::to_string).collect::<Vec<_>>().join("  ");
            cx.emit(&modes, Some(&text));
        }
        "set" => {
            let mode: PowerMode = m.str("mode").unwrap_or_default().parse().map_err(fail)?;
            let new = cx.api.set_power(mode, Some(HOLD_MINUTES))?;
            let text = format!(
                "power mode: {new}   (held 30 min against the automatic profile; `truesense power release` to end)"
            );
            cx.emit(&new, Some(&text));
        }
        "next" | "prev" => {
            let said = if verb == Some("next") { cx.api.next_power()? } else { cx.api.prev_power()? };
            cx.emit_text(&said);
        }
        "auto" => {
            let action: AutoAction = m.str("action").unwrap_or("status").parse()?;
            let now = cx.api.apply_auto_mode(action)?;
            cx.emit(now.as_str(), Some(&now.to_string()));
        }
        "smart" => smart(cx, m)?,
        "hold" => {
            let minutes = m.float("minutes").unwrap_or(HOLD_MINUTES);
            cx.api.hold_power(minutes)?;
            cx.emit_text(&format!("holding the current power mode for {} min", float_g(minutes)));
        }
        "managers" => managers(cx, m.str("action").unwrap_or("list"))?,
        "game" => {
            let action =
                GameRuleAction::parse(m.str("action").unwrap_or("list"), m.str("name"), m.str("mode"))?;
            let rules = cx.api.game_rules(&action)?;
            cx.emit(&rules, Some(&rules.to_string()));
        }
        "release" => {
            cx.api.release_hold()?;
            cx.emit_text("hold released: the automatic profile is in charge again");
        }
        other => return Err(fail(format!("unknown power command {other}"))),
    }
    Ok(0)
}

fn managers(cx: &mut Cx<'_, '_>, action: &str) -> Result<(), super::Fail> {
    match action {
        "list" => {
            let r = cx.api.power_managers()?;
            cx.emit(&r, Some(&r.to_string()));
        }
        "stop" => {
            let r = cx.api.stop_power_managers()?;
            cx.emit(&r, Some(&r.to_string()));
        }
        _ => {
            let r = cx.api.restore_power_managers()?;
            cx.emit(&r, Some(&r.to_string()));
        }
    }
    Ok(())
}

/// A Smart auto profile as the Python dictionary printed: the settings in their declared order.
fn profile_json(object: &serde_json::Value) -> super::Obj {
    let mut rows = Vec::new();
    for key in SMART_SETTING_KEYS {
        if let Some(v) = object.get(key) {
            rows.push((key, v.clone()));
        }
    }
    if let Some(d) = object.get("description") {
        rows.push(("description", d.clone()));
    }
    super::Obj(rows)
}

fn smart(cx: &mut Cx<'_, '_>, m: &Matches) -> Result<(), super::Fail> {
    let name = m.str("name");
    match m.str("action").unwrap_or("current") {
        "list" => {
            let list = cx.api.smart_profiles()?;
            cx.emit(&list.names, Some(&list.to_string()));
        }
        "current" => {
            let d = cx.api.smart_profile_in_use()?;
            let text = d.to_string();
            cx.emit(&d, Some(&text));
        }
        "show" => {
            let shown = name.unwrap_or("truesense");
            let p = cx.api.smart_profile(shown)?;
            let text = format!("{shown}\n{}", describe_smart_profile(&p));
            cx.emit(&profile_json(&p.to_json()), Some(&text));
        }
        "use" => {
            let used = cx.api.use_smart_profile(name.unwrap_or_default())?;
            cx.emit(&used, Some(&format!("Smart auto profile in use: {used}")));
        }
        "copy" => {
            let from = name.unwrap_or_default();
            let made = cx.api.copy_smart_profile(from, m.str("new"))?;
            let text = format!(
                "copied '{from}' to '{made}' (edit: truesense power smart save {made} --set ac_busy=turbo)"
            );
            cx.emit(&made, Some(&text));
        }
        "save" => {
            let target = name.unwrap_or_default();
            let saved = cx.api.save_smart_profile(target, m.str("base"), &m.list("set"))?;
            let text = format!("saved smart profile '{target}' (use it: truesense power smart use {target})");
            cx.emit(&profile_json(&saved.to_json()), Some(&text));
        }
        _ => {
            cx.api.delete_smart_profile(name.unwrap_or_default())?;
            let shown = name.unwrap_or("None");
            cx.emit(&name, Some(&format!("deleted smart profile '{shown}'")));
        }
    }
    Ok(())
}
