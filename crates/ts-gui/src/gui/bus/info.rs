//! What the machine is and has: the battery's readings and the capability checklist.
//!
//! Both are plain reads, safe to call at any time: the capability probe asks the firmware only the
//! read-only commands and the battery readings come from `/sys` (see `ts_api::api`).
#![forbid(unsafe_code)]

use serde_json::Value;

use super::args::Args;
use super::{CommandError, Ctx, Outcome, lift, val};

/// The handler of one command of this area; `None` when it is not one of ours.
pub(super) fn dispatch(c: &Ctx<'_>, key: &str, _a: &Args) -> Option<Outcome> {
    lift(handle(c, key))
}

fn handle(c: &Ctx<'_>, key: &str) -> Result<Option<Value>, CommandError> {
    Ok(Some(match key {
        "battery/info" => val(c.api.battery_info())?,
        "capabilities/get" => val(c.api.capabilities())?,
        _ => return Ok(None),
    }))
}
