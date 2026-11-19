//! "Copy as TrueSense CLI": the one command that puts a lighting setting on the laptop, as the
//! page shows it. A part of [`super`] (the Lighting page's view-model).
//!
//! The answer is the words (for a test to run them) and the text to paste in a shell, quoted so
//! that every word arrives as it is. The `ts-cli` tests run every kind through the real command
//! line against the simulated laptop and read the result back, so a copied command always does
//! what the page showed.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use ts_api::api::wave_directions;

use super::*;

/// The command for `args.what`:
///
/// * `keyboard`: `{effect, colour, speed, brightness, direction}` -> `keyboard effect ...` (only the
///   options the effect uses);
/// * `keyboard_off` -> `keyboard off`;
/// * `rear`: `{on, effect, colour, zones, speed, brightness, direction}` -> `rear set ...` or `rear off`;
/// * `perkey`: `{cells: [128 x hex], keys: [layout keys], brightness?}` -> `keyboard keys NAME=HEX ...
///   --base HEX` (the most common colour is the base; a key whose cells differ is given per cell);
///   `keyboard fill HEX` when every key has the same colour;
/// * `theme`: what `theme_request` answers (`{name, colourway?, custom?, params?, speed?,
///   brightness?}`) -> `theme apply ...` with `--colourway`, `--custom` per slot and `--set` per
///   setting;
/// * `look`: `{name}` -> `look apply NAME`.
///
/// Answers `{words, text}`.
///
/// # Errors
/// An unknown kind or effect, or a missing name.
pub fn cli_command(args: &Value) -> Result<Value, String> {
    let mut w: Vec<String> = vec!["truesense".into()];
    let mut push = |items: &[&str]| w.extend(items.iter().map(|s| (*s).to_owned()));
    match str_at(args, "what").unwrap_or("") {
        "keyboard" => {
            let name = str_at(args, "effect").unwrap_or("");
            let e = effect(name).ok_or_else(|| format!("unknown keyboard effect: {name}"))?;
            push(&["keyboard", "effect", e.id]);
            if e.colour {
                let colour = str_at(args, "colour").and_then(norm_hex).unwrap_or_else(|| STOCK.to_owned());
                push(&["--color", &colour]);
            }
            if e.speed {
                let speed = i64_at(args, "speed").unwrap_or(DEFAULT_SPEED).clamp(SPEED_RANGE.0, SPEED_RANGE.1);
                push(&["--speed", &speed.to_string()]);
            }
            let level = i64_at(args, "brightness").unwrap_or(KB_MAX).clamp(0, KB_MAX);
            push(&["--brightness", &level.to_string()]);
            if e.direction {
                let d = i64_at(args, "direction").unwrap_or(0);
                let id = wave_directions().into_iter().find(|x| x.value == d).map(|x| x.id);
                push(&["--direction", id.unwrap_or("left-to-right")]);
            }
        }
        "keyboard_off" => push(&["keyboard", "off"]),
        "rear" => {
            if args.get("on").and_then(Value::as_bool) == Some(false) {
                push(&["rear", "off"]);
            } else {
                let req = rear_request(args)?;
                let a = &req["args"];
                let effect = a["effect"].as_str().unwrap_or("static").to_owned();
                push(&["rear", "set", &effect]);
                if let Some(zones) = a["zones"].as_array() {
                    let z: Vec<&str> = zones.iter().filter_map(Value::as_str).collect();
                    push(&["--zones", &z.join(",")]);
                }
                if let Some(c) = a["color"].as_str() {
                    push(&["--color", c]);
                }
                let (_, _, has_speed, has_direction) =
                    REAR_EFFECTS.iter().copied().find(|(id, ..)| *id == effect).unwrap_or(("", "", false, false));
                if has_speed {
                    push(&["--speed", &a["speed"].as_i64().unwrap_or(3).to_string()]);
                }
                push(&["--brightness", &a["brightness"].as_i64().unwrap_or(100).to_string()]);
                if has_direction {
                    push(&["--direction", &a["direction"].as_i64().unwrap_or(1).to_string()]);
                }
            }
        }
        "perkey" => perkey(args, &mut w)?,
        "theme" => theme(args, &mut w)?,
        "look" => {
            let name = str_at(args, "name").filter(|n| !n.is_empty()).ok_or("which look?")?;
            push(&["look", "apply", name]);
        }
        other => return Err(format!("no command for '{other}'")),
    }
    let text = w.iter().map(|s| shell_word(s)).collect::<Vec<_>>().join(" ");
    Ok(json!({"words": w, "text": text}))
}

/// The per-key picture as `keyboard keys`: the most common key colour is `--base`, every other key
/// is `NAME=HEX` (by cell when the cells of one key differ).
fn perkey(args: &Value, w: &mut Vec<String>) -> Result<(), String> {
    let cells = cells_from(at(args, "cells"));
    let keys = at(args, "keys")
        .and_then(|k| serde_json::from_value::<Vec<Key>>(k.clone()).ok())
        .filter(|k| !k.is_empty())
        .ok_or("the key picture is missing")?;
    let mut count: BTreeMap<&str, usize> = BTreeMap::new();
    for k in &keys {
        for c in &k.cells {
            *count.entry(cells[*c].as_str()).or_default() += 1;
        }
    }
    // The most common colour; on a tie the first in colour order, so the answer is stable.
    let base = count.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))).map_or("000000", |(c, _)| c).to_owned();
    let mut items = Vec::new();
    for k in &keys {
        let colours: Vec<&str> = k.cells.iter().map(|c| cells[*c].as_str()).collect();
        if colours.iter().all(|c| *c == colours[0]) {
            if colours[0] != base {
                items.push(format!("{}={}", k.name, colours[0]));
            }
        } else {
            for (c, colour) in k.cells.iter().zip(&colours) {
                if *colour != base {
                    items.push(format!("cell{c}={colour}"));
                }
            }
        }
    }
    let brightness = i64_at(args, "brightness").map(|b| b.clamp(0, KB_MAX));
    if items.is_empty() {
        w.extend(["keyboard".into(), "fill".into(), base]);
    } else {
        w.extend(["keyboard".into(), "keys".into()]);
        w.extend(items);
        w.extend(["--base".into(), base]);
    }
    if let Some(b) = brightness {
        w.extend(["--brightness".into(), b.to_string()]);
    }
    Ok(())
}

/// `theme apply` with every choice of the theme editor.
fn theme(args: &Value, w: &mut Vec<String>) -> Result<(), String> {
    let name = str_at(args, "name").filter(|n| !n.is_empty()).ok_or("which theme?")?;
    w.extend(["theme".into(), "apply".into(), name.to_owned()]);
    match args.get("colourway") {
        Some(Value::String(s)) if s == "custom" => {
            w.extend(["--colourway".into(), "custom".into()]);
            for c in list_at(args, "custom").unwrap_or_default().iter().filter_map(Value::as_str).filter_map(norm_hex) {
                w.extend(["--custom".into(), format!("#{c}")]);
            }
        }
        Some(v) => {
            if let Some(n) = v.as_u64() {
                w.extend(["--colourway".into(), n.to_string()]);
            }
        }
        None => {}
    }
    if let Some(params) = at(args, "params").and_then(Value::as_object) {
        for (id, v) in params {
            let text = match v {
                Value::Bool(b) => if *b { "on".to_owned() } else { "off".to_owned() },
                Value::Array(keys) => keys.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(","),
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            w.extend(["--set".into(), format!("{id}={text}")]);
        }
    }
    for flag in ["speed", "brightness"] {
        if let Some(n) = i64_at(args, flag) {
            w.extend([format!("--{flag}"), n.to_string()]);
        }
    }
    Ok(())
}

/// A word as a POSIX shell reads it back unchanged: as it is when it has only safe characters,
/// else in single quotes (a quote inside becomes `'\''`).
pub fn shell_word(s: &str) -> String {
    let safe = !s.is_empty()
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-./=,:+@%".contains(&b));
    if safe { s.to_owned() } else { format!("'{}'", s.replace('\'', "'\\''")) }
}
