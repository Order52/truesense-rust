//! `keyboard`: backlight effects, per-key colours, animations, designs and lighting profiles.
#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::path::Path;

use ts_api::api::{AnimationRequest, EffectRequest, LightingProfileAnswer, PaintItem};
use ts_domain::keyboard::MAX_BRIGHT;

use super::{Cx, Fail, Outcome, fail};
use crate::cli::argparse::Matches;

/// The answer of the commands that only send something to the keyboard.
fn sent(cx: &mut Cx<'_, '_>, message: &str) {
    cx.emit_text(message);
}

pub(super) fn keyboard(cx: &mut Cx<'_, '_>, verb: Option<&str>, m: &Matches) -> Outcome {
    let api = cx.api;
    let brightness = m.int("brightness");
    match verb.unwrap_or("status") {
        "status" => {
            let st = api.keyboard_status()?;
            cx.emit(&st, None);
        }
        word @ ("on" | "off" | "toggle") => {
            let said = api.keyboard_power(word)?;
            cx.emit_text(&said);
        }
        "restore" => {
            api.restore_keyboard()?;
            sent(cx, "sent");
        }
        "list-keys" => {
            let names = api.key_names();
            cx.emit(&names, None);
        }
        "effect" => {
            let request = EffectRequest {
                name: m.str("name").unwrap_or_default().to_owned(),
                color: m.str("color").unwrap_or("00aec7").to_owned(),
                speed: m.int("speed").unwrap_or(5),
                brightness,
                direction: m.int("direction").unwrap_or(0),
            };
            api.set_keyboard_effect(&request)?;
            sent(cx, "sent");
        }
        "fx" => fx(cx, m)?,
        "color" => {
            api.set_keyboard_color(m.str("color").unwrap_or_default())?;
            sent(cx, "sent");
        }
        "speed" => {
            let level = m.int("level").unwrap_or_default();
            api.set_keyboard_speed(level)?;
            sent(cx, &format!("keyboard effect speed: {level} / 11"));
        }
        "paint" => {
            let items = PaintItem::parse_all(&m.list("items"))?;
            api.paint_keys(&items, brightness)?;
            let distinct: BTreeSet<&str> = items.iter().map(|i| i.key.as_str()).collect();
            sent(cx, &format!("painted {} key(s)", distinct.len()));
        }
        "design" => design(cx, m)?,
        "image" => {
            let file = m.str("file").unwrap_or_default();
            api.image_keyboard(Path::new(file), brightness)?;
            let base = Path::new(file)
                .file_name()
                .map_or_else(|| file.to_owned(), |n| n.to_string_lossy().into_owned());
            sent(
                cx,
                &format!(
                    "painted {base} across the keys (scaled to 22 x 6 keys); look at the keyboard, and `truesense keyboard status` shows the stored colours"
                ),
            );
        }
        "layout" => {
            let layout = api.key_layout();
            cx.emit(&layout, None);
        }
        "fill" => {
            api.fill_keyboard(m.str("color").unwrap_or_default(), brightness)?;
            sent(cx, "sent");
        }
        "gradient" => {
            api.gradient_keyboard(
                m.str("from_color").unwrap_or_default(),
                m.str("to_color").unwrap_or_default(),
                brightness,
            )?;
            sent(cx, "sent");
        }
        "rainbow" => {
            api.rainbow_keyboard(brightness)?;
            sent(cx, "sent");
        }
        "keys" => {
            let items = PaintItem::parse_all(&m.list("items"))?;
            api.set_key_colors(&items, m.str("base").unwrap_or("000000"), brightness)?;
            sent(cx, "sent");
        }
        "brightness" => match m.int("level") {
            None => {
                let b = api.keyboard_brightness()?;
                cx.emit(&b, Some(&format!("keyboard brightness: {b} / {MAX_BRIGHT}")));
            }
            Some(level) => {
                api.set_keyboard_brightness(level)?;
                cx.emit(&level, Some(&format!("keyboard brightness: {level} / {MAX_BRIGHT}")));
            }
        },
        "timeout" => {
            return cx.tri(
                m.str("action"),
                "backlight timeout (off after 30 s)",
                || Ok(api.keyboard_timeout()?),
                |on| Ok(api.set_keyboard_timeout(on)?),
            );
        }
        "operating" => {
            return cx.tri(
                m.str("action"),
                "operating-mode colour follows the power mode",
                || Ok(Some(api.operating_follow()?)),
                |on| Ok(api.set_operating_follow(on)?),
            );
        }
        "profile" => lighting_profile(cx, m)?,
        other => return Err(fail(format!("unknown keyboard command {other}"))),
    }
    Ok(0)
}

fn fx(cx: &mut Cx<'_, '_>, m: &Matches) -> Result<(), Fail> {
    let request = AnimationRequest {
        mode: m.str("mode").unwrap_or_default().to_owned(),
        speed: m.int("speed").unwrap_or(5),
        keys: m.str("keys").map(str::to_owned),
    };
    cx.api.set_keyboard_fx(&request)?;
    cx.emit(request.mode.as_str(), Some(&request.to_string()));
    Ok(())
}

fn design(cx: &mut Cx<'_, '_>, m: &Matches) -> Result<(), Fail> {
    let api = cx.api;
    let action = m.str("action").unwrap_or_default();
    if action == "list" {
        let all = api.designs();
        let mut sorted: Vec<&(String, ts_api::api::DesignInfo)> = all.0.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        let text = sorted
            .iter()
            .map(|(n, d)| format!("  {n:<12} {:<9} {}", d.source, d.info))
            .collect::<Vec<_>>()
            .join("\n");
        cx.emit(&all, Some(&text));
        return Ok(());
    }
    let name = m.str("name").unwrap_or_default();
    if name.is_empty() {
        return Err(fail("give a design name"));
    }
    if action == "apply" {
        api.apply_design(name, m.int("brightness"))?;
        sent(cx, &format!("design {name} applied"));
    } else {
        let path = api.save_design(name)?;
        sent(cx, &format!("saved to {}", path.display()));
    }
    Ok(())
}

fn lighting_profile(cx: &mut Cx<'_, '_>, m: &Matches) -> Result<(), Fail> {
    let spec = m.list("spec");
    let words: Vec<&str> = spec.iter().map(String::as_str).collect();
    let answer = cx.api.lighting_profile(m.str("verb2").unwrap_or_default(), m.str("name"), &words)?;
    let text = answer.to_string();
    match &answer {
        LightingProfileAnswer::Names(names) => cx.emit(names, Some(&text)),
        LightingProfileAnswer::Message(message) => cx.emit(message, Some(&text)),
    }
    Ok(())
}
