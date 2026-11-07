//! Status, power modes, Smart auto and the fans.
#![forbid(unsafe_code)]

use serde_json::{Value, json};
use ts_api::api::{Api, GameRuleAction};

use super::args::{Args, SWITCH, on_off, parse_word};
use super::{CommandError, Ctx, Outcome, lift, names, val, with_builtin};

/// How long a power mode picked in the window is held against the automatic profile.
const HOLD_MINUTES: f64 = 30.0;

/// The handler of one command of this area; `None` when it is not one of ours.
pub(super) fn dispatch(c: &Ctx<'_>, key: &str, a: &Args) -> Option<Outcome> {
    lift(handle(c, key, a))
}

fn handle(c: &Ctx<'_>, key: &str, a: &Args) -> Result<Option<Value>, CommandError> {
    let api = c.api;
    Ok(Some(match key {
        "status/get" => val(api.status_full())?,
        "version/get" => json!({ "truesense": Api::version() }),
        "power/status" => val(api.power_status()?)?,
        "power/set" => set_power(api, a)?,
        "power/auto" => auto(api, a)?,
        "power/release" => json!(api.release_hold()?.to_string()),
        "power/hold" => hold(api, a)?,
        "power/game" => game(api, a)?,
        "power/managers" => managers(api, a)?,
        "power/smart" => smart(api, a)?,
        "fan/auto" => json!(api.fan_auto()?.to_string()),
        "fan/max" => json!(api.fan_max()?.to_string()),
        "fan/smart" => on_off(a, SWITCH, || Ok(api.smart_fan()?), |on| Ok(api.set_smart_fan(on)?))?,
        "fan/off" => fan_off(api, a)?,
        "fan/set" => fan_set(api, a)?,
        "fan/curve" => curve(api, a)?,
        _ => return Ok(None),
    }))
}

fn set_power(api: &Api, a: &Args) -> Outcome {
    let mode = a.parsed("mode")?;
    let now = api.set_power(mode, Some(HOLD_MINUTES))?;
    Ok(json!(now.to_string()))
}

fn auto(api: &Api, a: &Args) -> Outcome {
    let action = parse_word(&a.action("status")?)?;
    let now = api.apply_auto_mode(action)?;
    Ok(json!(now.as_str()))
}

fn hold(api: &Api, a: &Args) -> Outcome {
    let minutes = a.float_opt("minutes")?.unwrap_or(HOLD_MINUTES);
    Ok(json!(api.hold_power(minutes)?.to_string()))
}

fn game(api: &Api, a: &Args) -> Outcome {
    let (name, mode) = (a.str("name")?, a.str("mode")?);
    let action = GameRuleAction::parse(&a.action("list")?, Some(&name), Some(&mode))?;
    val(api.game_rules(&action)?)
}

fn managers(api: &Api, a: &Args) -> Outcome {
    match a.action("list")?.as_str() {
        "list" => val(api.power_managers()?),
        "stop" => val(api.stop_power_managers()?),
        "restore" => val(api.restore_power_managers()?),
        _ => Err(CommandError::failed("action: list | stop | restore")),
    }
}

fn smart(api: &Api, a: &Args) -> Outcome {
    let (name, new_name) = (a.str("name")?, a.str("new")?);
    let settings = a.raw("profile");
    match a.action("current")?.as_str() {
        "list" => Ok(names(api.smart_profiles()?.names)),
        "current" => {
            let in_use = api.smart_profile_in_use()?;
            Ok(json!({ "name": in_use.name, "profile": val(&in_use.profile)? }))
        }
        "show" => {
            let n = if name.is_empty() { "truesense" } else { name.as_str() };
            with_builtin(api.smart_profile(n)?, Api::smart_profile_is_builtin(n))
        }
        "use" => {
            let used = match settings {
                Some(s) => api.use_smart_profile_data(&name, s)?,
                None => api.use_smart_profile(&name)?,
            };
            Ok(json!({ "name": used }))
        }
        "copy" => Ok(json!(api.copy_smart_profile(&name, non_empty(&new_name))?)),
        "save" => {
            let s =
                settings.ok_or_else(|| CommandError::failed("profile: the settings to save are needed"))?;
            val(api.save_smart_profile_data(&name, s)?)
        }
        "delete" => {
            api.delete_smart_profile(&name)?;
            Ok(json!(name))
        }
        _ => Err(CommandError::failed("action: list | current | show | use | copy | save | delete")),
    }
}

fn non_empty(s: &str) -> Option<&str> {
    (!s.is_empty()).then_some(s)
}

fn fan_off(api: &Api, a: &Args) -> Outcome {
    // The confirmation is passed on, never assumed: the application layer refuses a stop without it.
    Ok(json!(api.fan_off(a.boolean("confirm")?)?.to_string()))
}

fn fan_set(api: &Api, a: &Args) -> Outcome {
    let cpu = a.must_int("cpu")?;
    let gpu = a.int_opt("gpu")?;
    Ok(json!(api.fan_set(cpu, gpu, a.boolean("confirm")?)?.to_string()))
}

/// Builds the typed curve from the page's JSON (`{"cpu": [[t, pct], ...], "gpu": [...], ...}`).
///
/// This crate cannot name the curve type (it does not depend on the layer that defines it), so the
/// curve is made from a copy of a built-in one whose points and parameters are replaced; the
/// application layer then validates it exactly as it validates every curve (ranges, never falling
/// duty, 100 % by the safety temperatures), so nothing unsafe gets through whatever the page sent.
macro_rules! curve_from_json {
    ($api:expr, $json:expr) => {{
        let template = $api.fan_curve("truesense")?;
        let proto =
            *template.cpu.first().ok_or_else(|| CommandError::failed("curve: no template to edit"))?;
        let object = $json.as_object().ok_or_else(|| CommandError::failed("curve: expected an object"))?;
        let points = |side: &str| -> Result<Vec<_>, CommandError> {
            let bad = || {
                CommandError::failed(format!(
                    "curve {side}: expected a list of [temperature, percent] points"
                ))
            };
            let list = object.get(side).and_then(Value::as_array).ok_or_else(bad)?;
            list.iter()
                .map(|p| match p.as_array().map(Vec::as_slice) {
                    Some([t, d]) => {
                        let (t, d) = (t.as_f64().ok_or_else(bad)?, d.as_f64().ok_or_else(bad)?);
                        let mut point = proto;
                        point.temp = t;
                        point.duty = d;
                        Ok(point)
                    }
                    _ => Err(bad()),
                })
                .collect()
        };
        let number = |key: &str, current: f64| -> Result<f64, CommandError> {
            match object.get(key) {
                None | Some(Value::Null) => Ok(current),
                Some(v) => {
                    v.as_f64().ok_or_else(|| CommandError::failed(format!("curve {key}: expected a number")))
                }
            }
        };
        let mut curve = template.clone();
        curve.description = None;
        curve.native = object.get("native").and_then(Value::as_bool).unwrap_or(false);
        curve.cpu = points("cpu")?;
        curve.gpu = points("gpu")?;
        curve.hysteresis = number("hysteresis", curve.hysteresis)?;
        curve.down_rate = number("down_rate", curve.down_rate)?;
        curve.predict = number("predict", curve.predict)?;
        curve.couple = number("couple", curve.couple)?;
        curve.battery_shift = number("battery_shift", curve.battery_shift)?;
        curve
    }};
}

fn curve(api: &Api, a: &Args) -> Outcome {
    let (name, new_name) = (a.str("name")?, a.str("new")?);
    let edited = a.raw("curve");
    let need = || CommandError::failed("curve: the curve to save is needed");
    match a.action("current")?.as_str() {
        "list" => Ok(names(api.fan_curves()?.names)),
        "current" => {
            let in_use = api.fan_curve_in_use()?;
            Ok(json!({ "name": in_use.name, "curve": val(&in_use.curve)? }))
        }
        "show" => {
            let n = if name.is_empty() { "truesense" } else { name.as_str() };
            with_builtin(api.fan_curve(n)?, Api::fan_curve_is_builtin(n))
        }
        "use" => {
            let used = match edited {
                Some(json) => api.use_fan_curve_data(&name, &curve_from_json!(api, json))?,
                None => api.use_fan_curve(&name)?,
            };
            Ok(json!({ "name": used }))
        }
        "copy" => Ok(json!(api.copy_fan_curve(&name, non_empty(&new_name))?)),
        "save" => {
            let json = edited.ok_or_else(need)?;
            val(api.save_fan_curve_value(&name, &curve_from_json!(api, json))?)
        }
        "delete" => {
            api.delete_fan_curve(&name)?;
            Ok(json!(name))
        }
        _ => Err(CommandError::failed("action: list | current | show | use | copy | save | delete")),
    }
}
