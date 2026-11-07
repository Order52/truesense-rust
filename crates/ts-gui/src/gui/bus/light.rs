//! The lights: keyboard, light bar, looks and what happens at login.
#![forbid(unsafe_code)]

use serde_json::{Value, json};
use ts_api::api::{Api, EffectRequest, LightingProfileAnswer, RearRequest, parse_direction};

use super::args::{Args, SWITCH, SWITCH_NO_TOGGLE, on_off, parse_word};
use super::{CommandError, Ctx, Outcome, lift, names, val, with_builtin};

/// The effect speed when the page sends none.
const DEFAULT_EFFECT_SPEED: i64 = 5;
/// The effect colour when the page sends none (the stock cyan).
const DEFAULT_EFFECT_COLOR: &str = "00aec7";

/// The handler of one command of this area; `None` when it is not one of ours.
pub(super) fn dispatch(c: &Ctx<'_>, key: &str, a: &Args) -> Option<Outcome> {
    lift(handle(c, key, a))
}

fn handle(c: &Ctx<'_>, key: &str, a: &Args) -> Result<Option<Value>, CommandError> {
    let api = c.api;
    Ok(Some(match key {
        "keyboard/status" => val(api.keyboard_status()?)?,
        "keyboard/on" => json!(api.keyboard_power("on")?),
        "keyboard/off" => json!(api.keyboard_power("off")?),
        "keyboard/brightness" => {
            let level = a.must_int("level")?;
            api.set_keyboard_brightness(level)?;
            json!(level)
        }
        "keyboard/effect" => effect(api, a)?,
        "keyboard/color" => {
            api.set_keyboard_color(&a.must_str("color")?)?;
            json!("sent")
        }
        "keyboard/speed" => {
            let level = a.must_int("level")?;
            api.set_keyboard_speed(level)?;
            json!(format!("keyboard effect speed: {level} / 12"))
        }
        "keyboard/operating" => on_off(
            a,
            SWITCH_NO_TOGGLE,
            || Ok(api.operating_follow()?),
            |on| Ok(api.set_operating_follow(on)?),
        )?,
        "keyboard/timeout" => on_off(
            a,
            SWITCH,
            || Ok(api.keyboard_timeout()?.unwrap_or(false)),
            |on| Ok(api.set_keyboard_timeout(on)?),
        )?,
        "keyboard/restore" => {
            api.restore_keyboard()?;
            json!("sent")
        }
        "keyboard/layout" => val(api.key_layout())?,
        "keyboard/cells" => json!({ "cells": api.keyboard_cells() }),
        "keyboard/set-cells" => set_cells(api, a)?,
        "keyboard/fx" => fx(api, a)?,
        "keyboard/design" => design(api, a)?,
        "keyboard/profile" => lighting_profile(api, a)?,
        "rear/status" => val(api.rear_status()?)?,
        "rear/on" => json!(api.rear_power(parse_word("on")?)?),
        "rear/off" => json!(api.rear_power(parse_word("off")?)?),
        "rear/set" => rear_set(api, a)?,
        "look/list" => names(api.looks()),
        "look/show" => {
            let name = a.must_str("name")?;
            with_builtin(api.look(&name)?, Api::look_is_builtin(&name))?
        }
        "look/save" => look_save(api, a)?,
        "look/apply" => names(api.apply_look(&a.must_str("name")?)?),
        "look/delete" => {
            let name = a.must_str("name")?;
            api.delete_look(&name)?;
            json!(name)
        }
        "look/copy" => {
            let (name, new_name) = (a.must_str("name")?, a.str("new")?);
            json!(api.copy_look(&name, (!new_name.is_empty()).then_some(new_name.as_str()))?)
        }
        "startup/show" => val(api.startup())?,
        "startup/set" => startup_set(c, a)?,
        "startup/apply" => startup_apply(c)?,
        "startup/boot" => boot_look(api, a)?,
        _ => return Ok(None),
    }))
}

fn effect(api: &Api, a: &Args) -> Outcome {
    let mut request = EffectRequest::new(&a.must_str("name")?);
    let color = a.str("color")?;
    request.color = if color.is_empty() { DEFAULT_EFFECT_COLOR.to_owned() } else { color };
    request.speed = a.int_opt("speed")?.unwrap_or(DEFAULT_EFFECT_SPEED);
    request.brightness = a.int_opt("brightness")?;
    request.direction = wave_direction(a)?;
    api.set_keyboard_effect(&request)?;
    Ok(json!("sent"))
}

/// The `direction` member of `keyboard effect`: a number (the stored 0 to 5 of the wave) or one of
/// the names (`clockwise`, `counter-clockwise`, `left-to-right`, ...); absent is 0.
pub(super) fn wave_direction(a: &Args) -> Result<i64, CommandError> {
    match a.raw("direction") {
        Some(Value::String(text)) => Ok(parse_direction(text)?),
        _ => Ok(a.int_opt("direction")?.unwrap_or(0)),
    }
}

fn set_cells(api: &Api, a: &Args) -> Outcome {
    let cells = a.strings("cells")?.ok_or_else(|| CommandError::failed("cells is needed"))?;
    api.set_keyboard_cells(&cells, a.int_opt("brightness")?)?;
    Ok(json!("sent"))
}

fn fx(api: &Api, a: &Args) -> Outcome {
    let mode = a.must_str("mode")?;
    let speed = a.int_opt("speed")?.unwrap_or(DEFAULT_EFFECT_SPEED);
    let cells: Vec<i64> = a.decode("cells")?.unwrap_or_default();
    let base = a.strings("base")?;
    val(api.set_keyboard_fx_cells(&mode, speed, &cells, base.as_deref())?)
}

fn design(api: &Api, a: &Args) -> Outcome {
    let name = a.str("name")?;
    let cells = a.strings("cells")?;
    match a.action("list")?.as_str() {
        "list" => val(api.designs()),
        "show" => val(api.show_design(&name)?),
        "save" => Ok(json!(api.save_design_cells(&name, cells.as_deref())?.display().to_string())),
        "apply" => {
            api.apply_design(&name, None)?;
            Ok(json!(format!("design {name} applied")))
        }
        _ => Err(CommandError::failed("action: list | show | save | apply")),
    }
}

fn lighting_profile(api: &Api, a: &Args) -> Outcome {
    let name = a.str("name")?;
    let cells = a.strings("cells")?;
    let action = a.action("list")?;
    match action.as_str() {
        "list" => Ok(names(api.lighting_profile_names())),
        "save" => Ok(json!(api.save_lighting_profile_cells(&name, cells.as_deref())?)),
        "load" | "delete" => {
            let answer = api.lighting_profile(&action, Some(&name), &[])?;
            Ok(json!(match answer {
                LightingProfileAnswer::Message(m) => m,
                other @ LightingProfileAnswer::Names(_) => other.to_string(),
            }))
        }
        _ => Err(CommandError::failed("action: list | load | save | delete")),
    }
}

fn rear_set(api: &Api, a: &Args) -> Outcome {
    let effect = a.must_str("effect")?;
    let color = a.str("color")?;
    let zones = a.strings("zones")?.map(|z| z.join(","));
    let request = RearRequest::parse(
        &effect,
        if color.is_empty() { "ffffff" } else { &color },
        a.int_opt("speed")?.unwrap_or(3),
        a.int_opt("brightness")?.unwrap_or(100),
        a.int_opt("direction")?.unwrap_or(1),
        zones.as_deref(),
    )?;
    // `verify: false` is a slider drag: the next write follows at once, so no read-back.
    let verify = a.decode::<bool>("verify")?.unwrap_or(true);
    if !verify {
        return val(api.set_rear_live(&request, false)?);
    }
    api.set_rear(&request)?;
    val(api.rear_status()?)
}

fn look_save(api: &Api, a: &Args) -> Outcome {
    let name = a.must_str("name")?;
    let saved = match a.decode("look")? {
        Some(look) => api.save_look_value(&name, &look)?,
        None => api.save_look(&name)?,
    };
    val(saved)
}

fn startup_set(c: &Ctx<'_>, a: &Args) -> Outcome {
    let (target, value) = (a.must_str("target")?, a.str("value")?);
    let control = c.hooks.startup.as_ref().ok_or_else(|| {
        CommandError::failed("changing the start-up settings is not available in this build")
    })?;
    control.set(&target, &value).map(|v| json!(v)).map_err(CommandError::failed)
}

fn startup_apply(c: &Ctx<'_>) -> Outcome {
    let control = c.hooks.startup.as_ref().ok_or_else(|| {
        CommandError::failed("applying the start-up settings is not available in this build")
    })?;
    control.apply().map(names).map_err(CommandError::failed)
}

fn boot_look(api: &Api, a: &Args) -> Outcome {
    match a.str("what")?.as_str() {
        "" => val(api.boot_look()?),
        "clear" => {
            api.clear_boot_look()?;
            Ok(Value::Null)
        }
        what => val(api.set_boot_look(what)?),
    }
}
