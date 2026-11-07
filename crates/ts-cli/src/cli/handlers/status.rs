//! `status` and `sensors`.
#![forbid(unsafe_code)]

use ts_api::api::describe_sensors;

use super::{Cx, Outcome};

/// `truesense status`: every reading at a glance, one broken part never hiding the others.
pub(super) fn status(cx: &mut Cx<'_, '_>) -> Outcome {
    let status = cx.api.status();
    cx.emit(&status, None);
    Ok(0)
}

/// `truesense sensors`: temperatures and fan speeds.
pub(super) fn sensors(cx: &mut Cx<'_, '_>) -> Outcome {
    let s = cx.api.sensors()?;
    cx.emit(&s, Some(&describe_sensors(&s)));
    Ok(0)
}
