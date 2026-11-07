//! Battery, display, boot sound, GPU, CPU limits and undervolts, touchpad.
#![forbid(unsafe_code)]

use serde_json::{Value, json};
use ts_api::api::{Api, UndervoltRequest};

use super::args::{Args, SWITCH, on_off, parse_word};
use super::{CommandError, Ctx, Outcome, lift, names, val, with_builtin};

/// The handler of one command of this area; `None` when it is not one of ours.
pub(super) fn dispatch(c: &Ctx<'_>, key: &str, a: &Args) -> Option<Outcome> {
    lift(handle(c, key, a))
}

fn handle(c: &Ctx<'_>, key: &str, a: &Args) -> Result<Option<Value>, CommandError> {
    let api = c.api;
    Ok(Some(match key {
        "battery/status" => val(api.battery_status()?)?,
        "battery/limit" => {
            on_off(a, SWITCH, || Ok(api.battery_limit()?), |on| Ok(api.set_battery_limit(on)?))?
        }
        "battery/calibration" => {
            on_off(a, SWITCH, || Ok(api.calibration()?), |on| Ok(api.set_calibration(on)?))?
        }
        "battery/usb-charge" => json!(api.set_usb_charge(a.must_int("level")?)?),
        "battery/full" => full(api, a)?,
        "display/overdrive" => {
            on_off(a, SWITCH, || Ok(api.overdrive()?.unwrap_or(false)), |on| Ok(api.set_overdrive(on)?))?
        }
        "boot/sound" => on_off(a, SWITCH, || Ok(api.boot_sound()?), |on| Ok(api.set_boot_sound(on)?))?,
        "gpu/set" => gpu_set(api, a)?,
        "gpu/undervolt" => gpu_undervolt(api, a)?,
        "cpu/info" => val(api.cpu_limits()?)?,
        "cpu/limits" => cpu_limits(api, a)?,
        "cpu/undervolt" => cpu_undervolt(api, a)?,
        "touchpad/get" => api.touchpad().map_or(Value::Null, Value::Bool),
        "touchpad/set" => touchpad(api, a)?,
        _ => return Ok(None),
    }))
}

fn full(api: &Api, a: &Args) -> Outcome {
    let action = parse_word(&a.action("get")?)?;
    val(api.charge_full(action)?)
}

fn gpu_set(api: &Api, a: &Args) -> Outcome {
    let mode = Api::parse_gpu_mode(&a.must_str("mode")?)?;
    val(api.set_gpu_mode(mode)?.to_string())
}

fn touchpad(api: &Api, a: &Args) -> Outcome {
    let action = a.action("toggle")?;
    let on = match action.as_str() {
        "toggle" => !api.touchpad().unwrap_or(false),
        "on" => true,
        "off" => false,
        _ => return Err(CommandError::failed("action: on | off | toggle")),
    };
    Ok(json!(api.set_touchpad(on)?))
}

fn cpu_limits(api: &Api, a: &Args) -> Outcome {
    if a.boolean("auto")? {
        let now = api.reset_cpu_limits()?;
        return Ok(json!({ "pl1_w": now.info.pl1_w, "pl2_w": now.info.pl2_w }));
    }
    let pl1 = a.float_opt("pl1")?.ok_or_else(|| CommandError::failed("pl1 is needed (or auto)"))?;
    let pl2 = a.float_opt("pl2")?.unwrap_or(pl1);
    val(api.set_cpu_limits(pl1, pl2, a.boolean("force")?)?)
}

/// The members every undervolt command may carry.
struct Undervolt {
    action: String,
    name: String,
    new_name: String,
    request: UndervoltRequest,
}

impl Undervolt {
    fn read(a: &Args) -> Result<Self, CommandError> {
        Ok(Self {
            action: a.action("get")?,
            name: a.str("name")?,
            new_name: a.str("new")?,
            request: UndervoltRequest { confirm: a.boolean("confirm")?, force: a.boolean("force")? },
        })
    }

    fn name_or<'a>(&'a self, default: &'a str) -> &'a str {
        if self.name.is_empty() { default } else { &self.name }
    }

    fn new_name(&self) -> Option<&str> {
        (!self.new_name.is_empty()).then_some(self.new_name.as_str())
    }

    /// Whether the boot profile is being cleared (`name` empty or `off`).
    fn boot_off(&self) -> bool {
        self.name.is_empty() || self.name == "off"
    }
}

const UNDERVOLT_ACTIONS: &str =
    "action: get | list | show | use | apply | reset | copy | save | delete | boot";

fn gpu_undervolt(api: &Api, a: &Args) -> Outcome {
    let u = Undervolt::read(a)?;
    let offset = a.i32_opt("offset")?;
    let cap = a.i32_opt("cap")?.unwrap_or(0);
    match u.action.as_str() {
        "get" => val(api.gpu_undervolt_state()?),
        "list" => Ok(names(api.gpu_undervolts()?.names)),
        "show" => {
            let n = u.name_or("truesense");
            with_builtin(api.gpu_undervolt_profile(n)?, Api::gpu_undervolt_is_builtin(n))
        }
        "use" => val(api.apply_gpu_undervolt(&u.name, u.request)?),
        "apply" => {
            let offset = offset.ok_or_else(|| CommandError::failed("offset is needed"))?;
            val(api.apply_gpu_undervolt_values(u.name_or("manual"), offset, cap, u.request)?)
        }
        "reset" => val(api.reset_gpu_undervolt()?),
        "copy" => Ok(json!(api.copy_gpu_undervolt(&u.name, u.new_name())?)),
        "save" => val(api.save_gpu_undervolt(&u.name, offset, cap)?),
        "delete" => {
            api.delete_gpu_undervolt(&u.name)?;
            Ok(json!(u.name))
        }
        "boot" if u.boot_off() => val(api.set_gpu_undervolt_boot(None, u.request)?),
        "boot" => match offset {
            Some(offset) => val(api.set_gpu_undervolt_boot_values(&u.name, offset, cap, u.request)?),
            None => val(api.set_gpu_undervolt_boot(Some(&u.name), u.request)?),
        },
        _ => Err(CommandError::failed(UNDERVOLT_ACTIONS)),
    }
}

fn cpu_undervolt(api: &Api, a: &Args) -> Outcome {
    let u = Undervolt::read(a)?;
    let (core, cache) = (a.i32_opt("core")?, a.i32_opt("cache")?);
    match u.action.as_str() {
        "get" => val(api.cpu_undervolt_state()?),
        "list" => Ok(names(api.cpu_undervolts()?.names)),
        "show" => {
            let n = u.name_or("truesense");
            with_builtin(api.cpu_undervolt_profile(n)?, Api::cpu_undervolt_is_builtin(n))
        }
        "use" => val(api.apply_cpu_undervolt(&u.name, u.request)?),
        "apply" => {
            let (Some(core), Some(cache)) = (core, cache) else {
                return Err(CommandError::failed("core and cache are needed"));
            };
            val(api.apply_cpu_undervolt_values(u.name_or("manual"), core, cache, u.request)?)
        }
        "reset" => val(api.reset_cpu_undervolt()?),
        "copy" => Ok(json!(api.copy_cpu_undervolt(&u.name, u.new_name())?)),
        "save" => val(api.save_cpu_undervolt(&u.name, core, cache)?),
        "delete" => {
            api.delete_cpu_undervolt(&u.name)?;
            Ok(json!(u.name))
        }
        "boot" if u.boot_off() => val(api.set_cpu_undervolt_boot(None, u.request)?),
        "boot" => match (core, cache) {
            (Some(core), Some(cache)) => {
                val(api.set_cpu_undervolt_boot_values(&u.name, core, cache, u.request)?)
            }
            _ => val(api.set_cpu_undervolt_boot(Some(&u.name), u.request)?),
        },
        _ => Err(CommandError::failed(UNDERVOLT_ACTIONS)),
    }
}
