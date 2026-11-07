//! Mode bindings: what the service applies when the power mode changes.
#![forbid(unsafe_code)]

use serde_json::Value;
use ts_api::api::Api;

use super::args::Args;
use super::{CommandError, Ctx, Outcome, lift, val};

/// The handler of one command of this area; `None` when it is not one of ours.
pub(super) fn dispatch(c: &Ctx<'_>, key: &str, a: &Args) -> Option<Outcome> {
    lift(handle(c.api, key, a))
}

fn handle(api: &Api, key: &str, a: &Args) -> Result<Option<Value>, CommandError> {
    Ok(Some(match key {
        "mode/bindings" => val(api.mode_bindings()?)?,
        "mode/bind" => bind(api, a)?,
        "mode/unbind" => unbind(api, a)?,
        "mode/apply" => apply(api, a)?,
        _ => return Ok(None),
    }))
}

/// `Some(text)` for a given name, `None` for an absent or empty one.
fn named(a: &Args, key: &str) -> Result<Option<String>, CommandError> {
    Ok(Some(a.str(key)?).filter(|s| !s.is_empty()))
}

fn bind(api: &Api, a: &Args) -> Outcome {
    let mode = a.must_str("mode")?;
    let (look, curve) = (named(a, "look")?, named(a, "curve")?);
    val(api.bind_mode(&mode, look.as_deref(), curve.as_deref())?)
}

fn unbind(api: &Api, a: &Args) -> Outcome {
    let mode = a.must_str("mode")?;
    let what = match a.str("what")?.as_str() {
        "" | "all" => "all",
        "look" => "look",
        "curve" | "fan_curve" | "fan-curve" => "fan_curve",
        _ => return Err(CommandError::failed("what: look | curve | all")),
    };
    val(api.unbind_mode(&mode, what)?)
}

fn apply(api: &Api, a: &Args) -> Outcome {
    let mode = named(a, "mode")?;
    val(api.apply_mode_binding(mode.as_deref())?)
}
