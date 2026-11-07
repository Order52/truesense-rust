//! The PREDATOR key, whole-setup profiles and sound.
#![forbid(unsafe_code)]

use serde_json::{Value, json};
use ts_api::api::{Api, KeyProfileEdit};

use super::args::{Args, SWITCH_NO_TOGGLE, on_off, parse_word};
use super::{CommandError, Ctx, Outcome, lift, names, val, with_builtin};

/// The gestures a key profile binds, as the `[keys]` section and the page name them.
const GESTURES: [&str; 5] = ["tap", "double", "triple", "long", "fn_esc"];

/// The handler of one command of this area; `None` when it is not one of ours.
pub(super) fn dispatch(c: &Ctx<'_>, key: &str, a: &Args) -> Option<Outcome> {
    lift(handle(c, key, a))
}

fn handle(c: &Ctx<'_>, key: &str, a: &Args) -> Result<Option<Value>, CommandError> {
    let api = c.api;
    Ok(Some(match key {
        "keys/profile" => key_profile(api, a)?,
        "keys/test" => json!(api.test_action(&a.must_str("action")?)?),
        "profile/list" => val(api.profiles())?,
        "profile/show" => val(api.profile(&a.must_str("name")?)?)?,
        "profile/apply" => json!(api.apply_profile(&a.must_str("name")?)?),
        "profile/next" => json!(api.next_profile()?),
        "profile/save" => {
            let (name, description) = (a.must_str("name")?, a.str("description")?);
            json!(api.save_profile(&name, &description)?.display().to_string())
        }
        "profile/delete" => {
            let name = a.must_str("name")?;
            api.delete_profile(&name)?;
            json!(format!("deleted {name}"))
        }
        "profile/current" => json!({ "name": api.current_profile() }),
        "audio/output" => output(api, a)?,
        "audio/speaker-filter" => {
            on_off(a, SWITCH_NO_TOGGLE, || Ok(api.speaker_filter()), |on| Ok(api.set_speaker_filter(on)?))?
        }
        "audio/dts" => dts(api, a)?,
        "audio/jack" => jack(api, a)?,
        "audio/mic" => mic(api, a)?,
        "audio/voice" => voice(api, a)?,
        "audio/sound" => sound(api)?,
        "audio/repair" => {
            let r = api.repair_sound()?;
            json!({ "fixed": r.fixed, "changed": r.changed, "steps": r.steps })
        }
        _ => return Ok(None),
    }))
}

fn key_profile(api: &Api, a: &Args) -> Outcome {
    let (name, new_name) = (a.str("name")?, a.str("new")?);
    let action = a.action("current")?;
    match action.as_str() {
        "list" => Ok(names(api.key_profiles()?.names)),
        "current" => {
            let (profile, view) = api.key_profile_in_use()?;
            let mut object = match val(view)? {
                Value::Object(o) => o,
                _ => serde_json::Map::new(),
            };
            object.insert("profile".to_owned(), json!(profile));
            Ok(Value::Object(object))
        }
        "show" => {
            let n = if name.is_empty() { "default" } else { name.as_str() };
            let view = api.key_profile(n)?;
            // Only the built-in profiles have a description in the list.
            let builtin = api.key_profiles()?.descriptions.contains_key(n);
            with_builtin(view, builtin)
        }
        "use" => {
            api.use_key_profile(&name)?;
            Ok(json!(name))
        }
        "copy" => {
            Ok(json!(api.copy_key_profile(&name, (!new_name.is_empty()).then_some(new_name.as_str()))?))
        }
        "save" => {
            let mut edit = KeyProfileEdit {
                from: Some(a.str("from")?).filter(|f| !f.is_empty()),
                ..KeyProfileEdit::default()
            };
            for key in GESTURES {
                let action = a.str(key)?;
                if !action.is_empty() {
                    edit.actions.push((Api::parse_gesture(key)?, action));
                }
            }
            edit.long_ms = a.int_opt("long_ms")?;
            edit.multi_ms = a.int_opt("multi_ms")?;
            val(api.save_key_profile(&name, &edit)?)
        }
        "delete" => {
            api.delete_key_profile(&name)?;
            Ok(json!(name))
        }
        _ => Err(CommandError::failed("action: list | current | show | use | copy | save | delete")),
    }
}

fn output(api: &Api, a: &Args) -> Outcome {
    let action = a.action("status")?;
    match action.as_str() {
        "status" => {}
        "speakers" | "headphones" => {
            api.set_audio_output(parse_word(&action)?)?;
        }
        _ => return Err(CommandError::failed("output: speakers | headphones | status")),
    }
    val(api.audio_output()?)
}

fn dts(api: &Api, a: &Args) -> Outcome {
    let name = a.str("name")?;
    match a.action("status")?.as_str() {
        "status" => {}
        "on" => {
            api.set_dts_power(true)?;
        }
        "off" => {
            api.set_dts_power(false)?;
        }
        "mode" => {
            api.set_dts_mode(&name)?;
        }
        "all" => {
            let done = api.dts_everywhere((!name.is_empty()).then_some(name.as_str()))?;
            return Ok(json!({ "done": done }));
        }
        _ => return Err(CommandError::failed("action: status | on | off | mode | all")),
    }
    val(api.dts_info()?)
}

fn jack(api: &Api, a: &Args) -> Outcome {
    match a.str("use")?.as_str() {
        "" => {}
        use_ @ ("auto" | "headphones" | "headset") => api.set_jack(use_)?,
        _ => return Err(CommandError::failed("jack: auto | headphones | headset")),
    }
    val(api.jack())
}

fn mic(api: &Api, a: &Args) -> Outcome {
    let value = a.int_opt("value")?;
    match a.action("status")?.as_str() {
        "status" => {}
        "mute" => api.set_mic_mute(true, false),
        "unmute" => api.set_mic_mute(false, false),
        "volume" => {
            return match value {
                None => Ok(json!(api.mic_volume())),
                Some(v) => Ok(json!(api.set_mic_volume(whole("value", v)?)?)),
            };
        }
        "boost" => {
            if let Some(v) = value {
                api.set_headset_boost(whole("value", v)?)?;
            }
            return Ok(json!(api.headset_boost()));
        }
        _ => return Err(CommandError::failed("action: status | mute | unmute | volume | boost")),
    }
    val(api.mic_status())
}

/// A number the sound system takes as a small unsigned value.
fn whole<T: TryFrom<i64>>(key: &str, v: i64) -> Result<T, CommandError> {
    T::try_from(v).map_err(|_| CommandError::failed(format!("{key}: {v} is out of range")))
}

fn voice(api: &Api, a: &Args) -> Outcome {
    let (first, second) = (a.str("a")?, a.str("b")?);
    match a.action("status")?.as_str() {
        "status" => {}
        "set" => {
            api.set_voice(parse_word(&first)?, parse_word(&second)?)?;
        }
        "notify" => match first.as_str() {
            "on" => api.set_voice_notify(true)?,
            "off" => api.set_voice_notify(false)?,
            _ => return Err(CommandError::failed("usage: truesense audio voice notify on|off")),
        },
        _ => return Err(CommandError::failed("action: status | set | notify")),
    }
    val(api.voice_status())
}

fn sound(api: &Api) -> Outcome {
    let mut state = val(api.sound_state()?)?;
    // The page lists the output devices; a card without any has an empty list, not a missing one.
    if let Some(object) = state.as_object_mut() {
        object.entry("sinks").or_insert_with(|| json!([]));
    }
    Ok(state)
}
