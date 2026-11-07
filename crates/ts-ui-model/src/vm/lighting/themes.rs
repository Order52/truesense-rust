//! The themes tab: settings, requests and the look a theme makes.
//!
//! A part of [`super`] (the Lighting page's view-model); it shares the parent's names.
#![forbid(unsafe_code)]

use super::*;

// ---- themes -------------------------------------------------------------------------------------

/// `#RRGGBB` (upper case, the catalogue's form) from user input, or `None`.
pub(super) fn theme_colour(text: &str) -> Option<String> {
    norm_hex(text).map(|h| format!("#{}", h.to_ascii_uppercase()))
}

/// The look member a setting target (`keyboard.params.anchor`, `bar.speed`, `keyboard.dir`) writes:
/// the path inside the theme object.
pub(super) fn target_path(target: &str) -> Option<Vec<String>> {
    let parts: Vec<&str> = target.split('.').collect();
    match parts.as_slice() {
        [light @ ("keyboard" | "bar" | "logo"), field @ ("speed" | "dir")] => Some(vec![(*light).to_owned(), (*field).to_owned()]),
        [light @ ("keyboard" | "bar" | "logo"), "params", id] if !id.is_empty() => {
            Some(vec![(*light).to_owned(), "params".to_owned(), (*id).to_owned()])
        }
        _ => None,
    }
}

pub(super) fn path_get<'a>(theme: &'a Value, path: &[String]) -> Option<&'a Value> {
    path.iter().try_fold(theme, |v, key| v.get(key))
}

/// Writes `value` at `path` when the parent object exists (a target that does not fit its look is
/// ignored, as the engine does).
pub(super) fn path_set(theme: &mut Value, path: &[String], value: Value) {
    let Some((last, parents)) = path.split_last() else { return };
    let mut node = theme;
    for key in parents {
        match node.get_mut(key) {
            Some(next) => node = next,
            None => return,
        }
    }
    if let Some(map) = node.as_object_mut() {
        map.insert(last.clone(), value);
    }
}

/// The controls of a theme's settings with the value each has now (read from the theme's looks):
/// `[{id, label, kind: number|choice|toggle|keys|key, min, max, step, unit, clock, options, value}]`.
/// A choice's value is the index of the option (what `theme apply` takes), a toggle's a boolean,
/// and a number in hours (unit `h`) is shown as a clock time (`clock`).
pub fn theme_settings(theme: &Value) -> Value {
    let list = list_at(theme, "settings").unwrap_or_default();
    let out: Vec<Value> = list
        .iter()
        .filter_map(|s| {
            let id = str_at(s, "id")?;
            let kind = str_at(s, "type")?;
            let current = str_at(s, "target").and_then(target_path).and_then(|p| path_get(theme, &p).cloned());
            let num = |v: &Option<Value>| v.as_ref().and_then(Value::as_f64);
            let mut entry = json!({"id": id, "label": str_at(s, "label").unwrap_or(id)});
            let (kind_out, value) = match kind {
                "choice" => {
                    let values: Vec<f64> =
                        list_at(s, "values").unwrap_or_default().iter().filter_map(Value::as_f64).collect();
                    entry["options"] = json!(list_at(s, "options").unwrap_or_default());
                    let idx = num(&current).and_then(|c| values.iter().position(|v| (*v - c).abs() < 1e-9)).unwrap_or(0);
                    ("choice", json!(idx))
                }
                "toggle" => {
                    let on = f64_at(s, "on").unwrap_or(1.0);
                    ("toggle", json!(num(&current).is_some_and(|c| (c - on).abs() < 1e-9)))
                }
                "keys" => ("keys", current.filter(Value::is_array).unwrap_or_else(|| json!([]))),
                "key" => {
                    let first = current.as_ref().and_then(|c| c.as_array().and_then(|a| a.first()).or(Some(c))).and_then(Value::as_str);
                    ("key", json!(first.unwrap_or("")))
                }
                "number" | "time" => {
                    let (min, max) = (f64_at(s, "min").unwrap_or(0.0), f64_at(s, "max").unwrap_or(100.0));
                    entry["min"] = json!(min);
                    entry["max"] = json!(max);
                    entry["step"] = json!(f64_at(s, "step").filter(|st| *st > 0.0).unwrap_or(1.0));
                    let unit = str_at(s, "unit").unwrap_or("");
                    entry["unit"] = json!(unit);
                    entry["clock"] = json!(kind == "time" || unit == "h");
                    ("number", json!(num(&current).unwrap_or(min).clamp(min, max.max(min))))
                }
                _ => return None,
            };
            entry["kind"] = json!(kind_out);
            entry["value"] = value;
            Some(entry)
        })
        .collect();
    json!(out)
}

/// Clock text for hours of the day (`6.5` is `06:30`).
pub fn clock_text(hours: f64) -> String {
    let minutes = (hours.rem_euclid(24.0) * 60.0).round() as i64 % (24 * 60);
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// The values a page holds for a theme: the colourway (`"custom"` or an index), the custom colours
/// and one value per setting id.
pub(super) fn colourway_palette(theme: &Value, choice: &Value, custom: &[String]) -> Vec<String> {
    let ways = list_at(theme, "colourways").unwrap_or_default();
    let first: Vec<String> = ways
        .first()
        .and_then(|w| list_at(w, "palette"))
        .unwrap_or_default()
        .iter()
        .filter_map(|c| c.as_str().map(str::to_owned))
        .collect();
    if choice.as_str() == Some("custom") {
        // A slot without a valid colour keeps the first colourway's (the engine's rule).
        return first.iter().enumerate().map(|(i, c)| custom.get(i).cloned().unwrap_or_else(|| c.clone())).collect();
    }
    let idx = choice.as_u64().and_then(|i| usize::try_from(i).ok()).unwrap_or(0);
    ways.get(idx.min(ways.len().saturating_sub(1)))
        .and_then(|w| list_at(w, "palette"))
        .map(|p| p.iter().filter_map(|c| c.as_str().map(str::to_owned)).collect())
        .unwrap_or(first)
}

pub(super) fn custom_colours(args: &Value) -> Vec<String> {
    list_at(args, "custom").unwrap_or_default().iter().map(|c| c.as_str().and_then(theme_colour).unwrap_or_default()).collect()
}

/// The arguments of `theme apply`: the colourway (an index or `custom` with its colours), the
/// settings by id, the speed (10..100 %) and the brightness (1..100 %). Values out of range are
/// clamped here so that the service never has to refuse a slider.
pub fn theme_request(args: &Value) -> Result<Value, String> {
    let name = str_at(args, "name").filter(|n| !n.is_empty()).ok_or("a theme needs a name")?;
    let mut out = json!({"name": name});
    match at(args, "colourway") {
        Some(Value::String(s)) if s == "custom" => {
            out["colourway"] = json!("custom");
            out["custom"] = json!(custom_colours(args).into_iter().filter(|c| !c.is_empty()).collect::<Vec<_>>());
        }
        Some(v) => {
            if let Some(i) = v.as_u64() {
                out["colourway"] = json!(i);
            }
        }
        None => {}
    }
    if let Some(params) = at(args, "values").and_then(Value::as_object) {
        out["params"] = Value::Object(params.clone());
    }
    if let Some(s) = i64_at(args, "speed") {
        out["speed"] = json!(s.clamp(10, 100));
    }
    if let Some(b) = i64_at(args, "brightness") {
        out["brightness"] = json!(b.clamp(1, 100));
    }
    Ok(out)
}

/// An id for a user theme from the name typed: lower case letters, digits, `-` and `_` (spaces
/// become `-`); `None` when nothing is left.
pub(crate) fn theme_id(name: &str) -> Option<String> {
    let id: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c == ' ' { '-' } else { c })
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'))
        .collect();
    (!id.is_empty() && id.len() <= 40).then_some(id)
}

/// The theme object `theme save` takes for "keep what I chose": a copy of `theme` (as `theme show`
/// gives it) under a new `id` and `name`, whose palette and first colourway are the colours chosen
/// and whose settings hold the chosen values. `top` and `suits` are dropped (a user theme has no
/// rank), and ids of `taken` (the built-in themes) are refused here with a message.
pub fn theme_for_save(args: &Value, taken: &[String]) -> Result<Value, String> {
    let mut theme = at(args, "theme").cloned().filter(Value::is_object).ok_or("no theme to save")?;
    let name = str_at(args, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or("give the theme a name")?;
    let id = theme_id(name).ok_or("the name needs letters or digits")?;
    if taken.contains(&id) {
        return Err(format!("'{id}' is the id of a built-in theme: choose another name"));
    }
    let palette = colourway_palette(&theme, at(args, "colourway").unwrap_or(&Value::Null), &custom_colours(args));
    let way_name = if at(args, "colourway").and_then(Value::as_str) == Some("custom") {
        "Mine".to_owned()
    } else {
        let idx = at(args, "colourway").and_then(Value::as_u64).and_then(|i| usize::try_from(i).ok()).unwrap_or(0);
        list_at(&theme, "colourways").and_then(|w| w.get(idx)).and_then(|w| str_at(w, "name")).unwrap_or("Mine").to_owned()
    };
    if let Some(values) = at(args, "values").and_then(Value::as_object) {
        let defs: Vec<Value> = list_at(&theme, "settings").unwrap_or_default().to_vec();
        for def in &defs {
            let (Some(sid), Some(path)) = (str_at(def, "id"), str_at(def, "target").and_then(target_path)) else { continue };
            let Some(v) = values.get(sid) else { continue };
            let written = match (str_at(def, "type"), v) {
                (Some("choice"), v) => {
                    let values_list: Vec<f64> = list_at(def, "values").unwrap_or_default().iter().filter_map(Value::as_f64).collect();
                    let i = v.as_f64().map_or(0, |x| x as i64).clamp(0, values_list.len() as i64 - 1);
                    usize::try_from(i).ok().and_then(|i| values_list.get(i).copied()).map(|x| json!(x))
                }
                (Some("toggle"), v) => {
                    let (on, off) = (f64_at(def, "on").unwrap_or(1.0), f64_at(def, "off").unwrap_or(0.0));
                    Some(json!(if v.as_bool().unwrap_or(false) { on } else { off }))
                }
                (Some("keys"), Value::Array(a)) => Some(Value::Array(a.clone())),
                (Some("key"), Value::String(k)) => Some(json!([k])),
                (Some("number" | "time"), v) => v.as_f64().map(|x| json!(x)),
                _ => None,
            };
            if let Some(w) = written {
                path_set(&mut theme, &path, w);
            }
        }
    }
    theme["id"] = json!(id);
    theme["name"] = json!(name);
    theme["palette"] = json!(palette);
    let mut ways: Vec<Value> = list_at(&theme, "colourways").unwrap_or_default().to_vec();
    ways.retain(|w| str_at(w, "name") != Some(way_name.as_str()));
    ways.insert(0, json!({"name": way_name, "palette": palette}));
    theme["colourways"] = json!(ways);
    if let Some(map) = theme.as_object_mut() {
        map.remove("top");
        map.remove("suits");
    }
    Ok(theme)
}
