//! `cpu`: power limits and the guarded undervolt.
#![forbid(unsafe_code)]

use ts_api::api::{PowerLimitsState, UndervoltPlane, UndervoltRequest};

use super::{Cx, Fail, Outcome, fail, narrow};
use crate::cli::argparse::Matches;
use crate::cli::argparse::pyfmt::{float_g, float_repr};

/// `PL1 45.0 W, PL2 157.0 W   (automatic: ...)`: the limits as the Python dictionary printed them
/// (floats keep their `.0`, an unknown limit is `None`).
fn limits_text(state: &PowerLimitsState) -> String {
    let watts = |w: Option<f64>| w.map_or_else(|| "None".to_owned(), float_repr);
    let tail = match &state.user {
        Some(u) => format!("(yours: {}/{} W)", float_g(u.pl1), float_g(u.pl2)),
        None => format!("(automatic: {} W, {} W when gaming)", state.auto.normal_w, state.auto.gaming_w),
    };
    format!("PL1 {} W, PL2 {} W   {tail}", watts(state.info.pl1_w), watts(state.info.pl2_w))
}

pub(super) fn cpu(cx: &mut Cx<'_, '_>, verb: Option<&str>, m: &Matches) -> Outcome {
    match verb.unwrap_or("info") {
        "info" => {
            let info = cx.api.cpu_info();
            cx.emit(&info, None);
        }
        "limits" => limits(cx, m)?,
        "undervolt" => undervolt(cx, m)?,
        other => return Err(fail(format!("unknown cpu command {other}"))),
    }
    Ok(0)
}

fn limits(cx: &mut Cx<'_, '_>, m: &Matches) -> Result<(), Fail> {
    let api = cx.api;
    if m.flag("auto") {
        let state = api.reset_cpu_limits()?;
        cx.emit(&state, Some(&format!("automatic power limits: {}", limits_text(&state))));
    } else if let Some(pl1) = m.float("pl1") {
        let pl2 = m.float("pl2").unwrap_or(pl1);
        let r = api.set_cpu_limits(pl1, pl2, m.flag("force"))?;
        let text = format!(
            "power limits set: PL1 {} W, PL2 {} W   (the service keeps them; `truesense cpu limits --auto` goes back)",
            float_g(r.pl1_w),
            float_g(r.pl2_w)
        );
        cx.emit(&r, Some(&text));
    } else {
        let state = api.cpu_limits()?;
        cx.emit(&state, Some(&limits_text(&state)));
    }
    Ok(())
}

fn undervolt(cx: &mut Cx<'_, '_>, m: &Matches) -> Result<(), Fail> {
    let api = cx.api;
    let name = m.str("name");
    let request = UndervoltRequest { confirm: m.flag("confirm"), force: m.flag("force") };
    match m.str("action").unwrap_or_default() {
        "get" => {
            let r = api.cpu_undervolt_state()?;
            cx.emit(&r, Some(&r.to_string()));
        }
        "list" => {
            let list = api.cpu_undervolts()?;
            let mut lines = Vec::new();
            for n in &list.names {
                let p = api.cpu_undervolt_profile(n)?;
                let mut line = format!("  {n:12} core {:>4} mV  cache {:>4} mV", p.core, p.cache);
                if let Some(d) = list.descriptions.get(n) {
                    line.push_str(&format!("  - {d}"));
                }
                lines.push(line);
            }
            cx.emit(&list.names, Some(&lines.join("\n")));
        }
        "show" => {
            let shown = name.unwrap_or("truesense");
            let p = api.cpu_undervolt_profile(shown)?;
            let text = format!("{shown}: core {} mV, cache {} mV   {}", p.core, p.cache, p.description);
            cx.emit(&p, Some(&text));
        }
        "reset" => {
            let r = api.reset_cpu_undervolt()?;
            cx.emit(&r, Some(&format!("undervolt reset: {r}")));
        }
        "use" => {
            let used = name.unwrap_or_default();
            let r = api.apply_cpu_undervolt(used, request)?;
            let text = format!(
                "applied '{used}': {r}   (a reboot returns to stock; `truesense cpu undervolt reset` does it now)"
            );
            cx.emit(&r, Some(&text));
        }
        "set" => {
            let Some(mv) = m.int("mv") else {
                return Err(fail("usage: truesense cpu undervolt set -10 --confirm   (or use a profile)"));
            };
            let plane = UndervoltPlane::from_number(m.int("plane").unwrap_or(0))?;
            let r = api.set_cpu_undervolt(plane, narrow(mv, "mV")?, request)?;
            cx.emit(&r, Some(&r.to_string()));
        }
        "copy" => {
            let from = name.unwrap_or_default();
            let made = api.copy_cpu_undervolt(from, m.str("new"))?;
            cx.emit(&made, Some(&format!("copied '{from}' to '{made}'")));
        }
        "save" => {
            let target = name.unwrap_or_default();
            let core = m.int("core").map(|v| narrow(v, "--core")).transpose()?;
            let cache = m.int("cache").map(|v| narrow(v, "--cache")).transpose()?;
            let saved = api.save_cpu_undervolt(target, core, cache)?;
            let text = format!(
                "saved undervolt profile '{target}' (use it: truesense cpu undervolt use {target} --confirm)"
            );
            cx.emit(&saved, Some(&text));
        }
        "delete" => {
            api.delete_cpu_undervolt(name.unwrap_or_default())?;
            let shown = name.unwrap_or("None");
            cx.emit(&name, Some(&format!("deleted undervolt profile '{shown}'")));
        }
        "boot" => match name {
            None | Some("off") => {
                api.set_cpu_undervolt_boot(None, request)?;
                cx.emit(&(), Some("no undervolt is applied at boot"));
            }
            Some(profile) => {
                let r = api.set_cpu_undervolt_boot(Some(profile), request)?;
                let text = format!(
                    "'{profile}' will be applied at boot (guarded: skipped and switched off if the previous boot did not stay up 10 minutes)"
                );
                cx.emit(&r, Some(&text));
            }
        },
        other => return Err(fail(format!("unknown cpu undervolt action {other}"))),
    }
    Ok(())
}
