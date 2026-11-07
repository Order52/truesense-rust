//! `battery`, `display`, `boot` and `touchpad`: the settings that are an on/off switch or a level.
#![forbid(unsafe_code)]

use ts_api::api::{ChargeFullAction, Switch};

use super::{Cx, Fail, Outcome, fail};
use crate::cli::argparse::Matches;

pub(super) fn battery(cx: &mut Cx<'_, '_>, verb: Option<&str>, m: &Matches) -> Outcome {
    let api = cx.api;
    match verb.unwrap_or("status") {
        "status" => {
            let st = api.battery_status()?;
            cx.emit(&st, None);
        }
        "limit" => {
            cx.tri(
                m.str("action"),
                "80 % battery limit",
                || Ok(Some(api.battery_limit()?)),
                |on| Ok(api.set_battery_limit(on)?),
            )?;
        }
        "full" => {
            let action: ChargeFullAction = m.str("action").unwrap_or("status").parse()?;
            let r = api.charge_full(action)?;
            let text = r.message(action);
            cx.emit(&r, Some(&text));
        }
        "calibration" => {
            cx.tri(
                m.str("action"),
                "battery calibration (needs AC, takes hours)",
                || Ok(Some(api.calibration()?)),
                |on| Ok(api.set_calibration(on)?),
            )?;
        }
        "usb-charge" => {
            let level = m.str("level").unwrap_or("status");
            let shown = if level == "status" {
                api.usb_charge()?
            } else {
                api.set_usb_charge(level.parse().map_err(|_| fail(format!("usb-charge level: {level}")))?)?
            };
            cx.emit(&shown, Some(&format!("USB charging while off, stop level: {shown}")));
        }
        other => return Err(fail(format!("unknown battery command {other}"))),
    }
    Ok(0)
}

/// The action word of a switch command, checked.
fn action(m: &Matches) -> Result<Option<&str>, Fail> {
    let word = m.str("action");
    if let Some(w) = word {
        w.parse::<Switch>()?;
    }
    Ok(word)
}

pub(super) fn display(cx: &mut Cx<'_, '_>, m: &Matches) -> Outcome {
    let api = cx.api;
    cx.tri(action(m)?, "LCD overdrive", || Ok(api.overdrive()?), |on| Ok(api.set_overdrive(on)?))
}

pub(super) fn boot(cx: &mut Cx<'_, '_>, m: &Matches) -> Outcome {
    let api = cx.api;
    cx.tri(action(m)?, "boot sound", || Ok(Some(api.boot_sound()?)), |on| Ok(api.set_boot_sound(on)?))
}

pub(super) fn touchpad(cx: &mut Cx<'_, '_>, m: &Matches) -> Outcome {
    let api = cx.api;
    cx.tri(action(m)?, "touchpad", || Ok(api.touchpad()), |on| Ok(api.set_touchpad(on)?))
}
