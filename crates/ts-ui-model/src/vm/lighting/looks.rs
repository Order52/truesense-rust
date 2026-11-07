//! The keyboard preview model and the names and summaries of looks.
//!
//! A part of [`super`] (the Lighting page's view-model); it shares the parent's names.
#![forbid(unsafe_code)]

use super::*;

// ---- the preview --------------------------------------------------------------------------------

/// Colours of the 128 cells that stand for the lights described by `look` (the `look` member of
/// `keyboard status`), with `cells` the per-key colours when known. `exact` is true when the
/// picture is the real thing (a still colour or the per-key state) and false for an
/// approximation of a moving effect, which the page says so.
pub fn preview(look: &Value, cells: Option<&Value>) -> Value {
    let mode = str_at(look, "mode").unwrap_or("");
    let level = i64_at(look, "brightness").unwrap_or(KB_MAX).clamp(0, KB_MAX);
    let dim = |hex: &str| -> String {
        if level == 0 {
            return "000000".to_owned();
        }
        let f = 0.35 + 0.65 * (level as f64 / KB_MAX as f64);
        parse_rgb(hex).map_or_else(|| "000000".to_owned(), |c| to_hex(c.map(|v| clamp_byte(f64::from(v) * f))))
    };
    let (kind, exact, base): (&str, bool, Vec<String>) = match mode {
        "perkey" => match cells.filter(|c| c.is_array()) {
            Some(c) => ("perkey", true, cells_from(Some(c))),
            None => ("unknown", false, vec!["000000".to_owned(); CELLS]),
        },
        "off" => ("off", true, vec!["000000".to_owned(); CELLS]),
        "effect" => {
            let name = str_at(look, "name").unwrap_or("static");
            let colour = str_at(look, "color").and_then(norm_hex).unwrap_or_else(|| STOCK.to_owned());
            let uses_colour = effect(name).is_none_or(|e| e.colour);
            let still = name == "static";
            let pattern: Vec<String> = (0..CELLS)
                .map(|cell| {
                    if !uses_colour {
                        // The firmware ignores the colour: a hue ramp across the columns.
                        return to_hex(hue_ramp((cell as f64 / ROWS as f64) / 22.0));
                    }
                    if still || name == "breathing" {
                        return colour.clone();
                    }
                    // Sparse bright keys on a dim ground: a still frame of a moving effect.
                    if (cell * 7) % 5 == 0 { colour.clone() } else { dim_to(&colour, 0.3) }
                })
                .collect();
            ("effect", still, pattern)
        }
        _ => ("unknown", false, vec!["000000".to_owned(); CELLS]),
    };
    let shown: Vec<String> = base
        .iter()
        .enumerate()
        .map(|(i, hex)| if i >= KEY_CELLS { "000000".to_owned() } else { dim(hex) })
        .collect();
    json!({"cells": shown, "kind": kind, "exact": exact})
}

pub(super) fn dim_to(hex: &str, f: f64) -> String {
    parse_rgb(hex).map_or_else(|| "000000".to_owned(), |c| to_hex(c.map(|v| clamp_byte(f64::from(v) * f))))
}

// ---- names and look summaries -------------------------------------------------------------------

/// Whether `name` is a usable look or design name (the rule of the web window and the stores).
pub fn valid_name(name: &str) -> bool {
    ts_foundation::names::is_valid(name)
}

/// Whether `name` is a usable per-key set name (no dots).
pub(crate) fn valid_set_name(name: &str) -> bool {
    ts_foundation::names::is_valid(name) && !name.contains('.')
}

/// A look split into the facts the page words in its own language: the keyboard part (`effect`
/// name, `perkey`, or a theme) and the rear part.
pub fn look_summary(look: &Value) -> Value {
    let keyboard = at(look, "keyboard").map(|k| {
        let mode = str_at(k, "mode").unwrap_or("");
        json!({
            "kind": if mode == "perkey" { "perkey" } else if mode == "off" { "off" } else { "effect" },
            "name": str_at(k, "name"), "brightness": i64_at(k, "brightness"), "speed": i64_at(k, "speed"),
        })
    });
    let rear = at(look, "rear").map(|r| {
        json!({"mode": str_at(r, "mode").or_else(|| str_at(r, "effect")), "brightness": i64_at(r, "brightness")})
    });
    let theme = at(look, "theme").map(|t| json!({"id": str_at(t, "id")}));
    json!({"keyboard": keyboard, "rear": rear, "theme": theme})
}
