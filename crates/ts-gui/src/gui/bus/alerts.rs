//! The desktop alerts' settings (`[alerts]` of the configuration). The alerts themselves are
//! raised by the root daemon, so they work with no window; this area only reads and changes
//! the settings, which the daemon picks up within about ten seconds.
#![forbid(unsafe_code)]

use serde_json::Value;
use ts_api::api::AlertsEdit;

use super::args::Args;
use super::{CommandError, Ctx, Outcome, lift, val};

/// The handler of one command of this area; `None` when it is not one of ours.
pub(super) fn dispatch(c: &Ctx<'_>, key: &str, a: &Args) -> Option<Outcome> {
    lift(handle(c, key, a))
}

fn handle(c: &Ctx<'_>, key: &str, a: &Args) -> Result<Option<Value>, CommandError> {
    let api = c.api;
    Ok(Some(match key {
        "alerts/get" => val(api.alerts()?)?,
        "alerts/set" => {
            let edit = AlertsEdit {
                enabled: a.decode("enabled")?,
                cpu_limit_c: a.int_opt("cpu_limit_c")?,
                gpu_limit_c: a.int_opt("gpu_limit_c")?,
                repeat_minutes: a.int_opt("repeat_minutes")?,
                debounce_samples: a.int_opt("debounce_samples")?,
                battery_low_pct: a.int_opt("battery_low_pct")?,
                throttling: a.decode("throttling")?,
                sensor_lost: a.decode("sensor_lost")?,
            };
            val(c.api.set_alerts(&edit)?)?
        }
        _ => return Ok(None),
    }))
}
