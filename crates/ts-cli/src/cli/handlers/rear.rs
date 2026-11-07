//! `rear`: the light bar on the back.
#![forbid(unsafe_code)]

use ts_api::api::{RearPowerAction, RearRequest};

use super::{Cx, Outcome, fail};
use crate::cli::argparse::Matches;

pub(super) fn rear(cx: &mut Cx<'_, '_>, verb: Option<&str>, m: &Matches) -> Outcome {
    match verb.unwrap_or("status") {
        "status" => {
            let r = cx.api.rear()?;
            cx.emit(&r, None);
        }
        "set" => {
            let request = RearRequest::parse(
                m.str("effect").unwrap_or_default(),
                m.str("color").unwrap_or("ffffff"),
                m.int("speed").unwrap_or(3),
                m.int("brightness").unwrap_or(100),
                m.int("direction").unwrap_or(1),
                m.str("zones"),
            )?;
            let r = cx.api.set_rear(&request)?;
            cx.emit(&r, None);
        }
        word @ ("on" | "off" | "toggle") => {
            let action: RearPowerAction = word.parse()?;
            let said = cx.api.rear_power(action)?;
            cx.emit_text(&said);
        }
        other => return Err(fail(format!("unknown rear command {other}"))),
    }
    Ok(0)
}
