//! What the Lighting page decides, without Qt: the key picture and its geometry, hit testing and
//! paint strokes, colour maths, the effect catalogue with the rules of its options, the keyboard
//! preview model, the rear bar rules and the per-power-mode binding edit state (Changed, Undo).
//!
//! Everything is a pure function of its arguments (no I/O, no clock), reached from QML as
//! `Bus.vm("lighting", verb, args)`; the verbs are listed in [`call`]. The page holds the state
//! objects (selection, binding edit state) and passes them in and out, so the logic stays testable
//! and the page can be destroyed and rebuilt freely.
//!
//! Cells: the keyboard takes 128 colours, `cell = column * 6 + row` (22 columns, 6 rows); only
//! cells 0..125 are keys, 126 and 127 stay black (`docs/PROTOCOL.md`, `keymap`).
#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use ts_domain::keyboard;

use crate::json::{at, f64_at, i64_at, list_at, str_at};

/// Colours the keyboard takes in one upload.
pub(crate) const CELLS: usize = 128;
/// Cells that are keys.
pub(crate) const KEY_CELLS: usize = 126;
/// Rows of the keyboard picture (the cell numbering).
const ROWS: i64 = 6;
/// The stock cyan of the keyboard and the default effect colour.
pub(crate) const STOCK: &str = "00aec7";
/// The colours offered as swatches (the web window's palette).
pub const PALETTE: [&str; 16] = [
    "ff3b30", "ff9500", "ffcc00", "34c759", "00c7be", "32ade6", "007aff", "5856d6", "af52de", "ff2d55", "ffffff",
    "c7c7cc", "8e8e93", "3a3a3c", "00aec7", "000000",
];

#[path = "lighting/colour.rs"]
mod colour;
#[path = "lighting/picture.rs"]
mod picture;
#[path = "lighting/effects.rs"]
mod effects;
#[path = "lighting/looks.rs"]
mod looks;
#[path = "lighting/bindings.rs"]
mod bindings;
#[path = "lighting/themes.rs"]
mod themes;

pub use self::colour::*;
pub use self::picture::*;
pub use self::effects::*;
pub use self::looks::*;
pub use self::bindings::*;
pub use self::themes::*;

// ---- the verbs ----------------------------------------------------------------------------------

fn keys_arg(args: &Value) -> Vec<Key> {
    at(args, "keys").and_then(|k| serde_json::from_value(k.clone()).ok()).unwrap_or_default()
}

fn cells_arg(args: &Value, name: &str) -> Vec<usize> {
    list_at(args, name)
        .map(|l| l.iter().filter_map(Value::as_u64).filter_map(|c| usize::try_from(c).ok()).collect())
        .unwrap_or_default()
}

fn point(args: &Value, name: &str) -> Option<(f64, f64)> {
    Some((f64_at(args, &format!("{name}.x"))?, f64_at(args, &format!("{name}.y"))?))
}

fn bindings_arg(args: &Value) -> Result<Bindings, String> {
    at(args, "state")
        .and_then(|s| serde_json::from_value(s.clone()).ok())
        .ok_or_else(|| "the binding state is missing".to_owned())
}

fn state_out(b: &Bindings) -> Value {
    json!({"state": b, "view": b.view()})
}

/// The verbs of the page:
/// `effects`, `rear_effects`, `fx_modes`, `fx_own_colours`, `palette`, `stock_cells`, `norm_hex`, `hsv_to_hex`,
/// `hex_to_hsv`, `wants_dark_text`, `inks`, `neighbour`, `layout`, `hit`, `select`, `stroke`, `paint`, `tool`,
/// `pick_effect`, `rear_request`, `preview`, `look_summary`, `valid_name`, `theme_settings`, `theme_request`, `theme_for_save`, `clock_text`, and for the strip
/// `bindings_load`, `bindings_select`, `bindings_choose`, `bindings_undo`, `bindings_discard`,
/// `bindings_plan`.
///
/// # Errors
/// The text for an unknown verb or arguments that cannot be used.
pub fn call(verb: &str, args: &Value) -> Result<Value, String> {
    match verb {
        "effects" => Ok(json!(EFFECTS.iter().map(effect_json).collect::<Vec<_>>())),
        "rear_effects" => Ok(json!(
            REAR_EFFECTS
                .iter()
                .map(|(id, icon, speed, direction)| json!({"id": id, "icon": icon, "speed": speed, "direction": direction}))
                .collect::<Vec<_>>()
        )),
        "fx_modes" => Ok(json!(FX_MODES)),
        "fx_own_colours" => Ok(json!(FX_OWN_COLOURS)),
        "palette" => Ok(json!(PALETTE)),
        "stock_cells" => Ok(json!(stock_cells())),
        "norm_hex" => Ok(json!(norm_hex(str_at(args, "s").unwrap_or("")))),
        "hsv_to_hex" => {
            let hsv = Hsv {
                h: f64_at(args, "h").unwrap_or(0.0),
                s: f64_at(args, "s").unwrap_or(0.0),
                v: f64_at(args, "v").unwrap_or(0.0),
            };
            Ok(json!(to_hex(hsv_to_rgb(hsv))))
        }
        "hex_to_hsv" => {
            let rgb = str_at(args, "hex").and_then(parse_rgb).ok_or("not a colour")?;
            let h = rgb_to_hsv(rgb);
            Ok(json!({"h": h.h, "s": h.s, "v": h.v}))
        }
        "inks" => Ok(json!(cells_from(at(args, "cells")).iter().map(|c| wants_dark_text(c)).collect::<Vec<_>>())),
        "neighbour" => {
            let cell = usize::try_from(i64_at(args, "cell").ok_or("neighbour needs a cell")?).map_err(|e| e.to_string())?;
            Ok(json!(neighbour(&keys_arg(args), cell, str_at(args, "dir").unwrap_or(""))))
        }
        "wants_dark_text" => Ok(json!(wants_dark_text(str_at(args, "hex").unwrap_or("")))),
        "layout" => {
            let layout = build_layout(&name_cells(args));
            serde_json::to_value(layout).map_err(|e| e.to_string())
        }
        "hit" => {
            let keys = keys_arg(args);
            let (x, y) = point(args, "at").ok_or("hit needs at.x and at.y")?;
            Ok(json!(key_at(&keys, x, y).map(|k| &k.cells)))
        }
        "select" => select(args),
        "stroke" => {
            let keys = keys_arg(args);
            let a = point(args, "from").ok_or("stroke needs from")?;
            let b = point(args, "to").unwrap_or(a);
            let touched: Vec<usize> = keys_on_segment(&keys, a, b).into_iter().flat_map(|k| k.cells.iter().copied()).collect();
            let mut cells = cells_from(at(args, "cells"));
            let mut changed = false;
            if let Some(hex) = str_at(args, "colour").and_then(norm_hex) {
                for c in &touched {
                    if let Some(slot) = cells.get_mut(*c)
                        && *slot != hex
                    {
                        slot.clone_from(&hex);
                        changed = true;
                    }
                }
            }
            Ok(json!({"touched": touched, "cells": cells, "changed": changed}))
        }
        "paint" => {
            // Paint a list of cells one colour.
            let hex = str_at(args, "colour").and_then(norm_hex).ok_or("paint needs a colour")?;
            let mut cells = cells_from(at(args, "cells"));
            for c in cells_arg(args, "targets") {
                if let Some(slot) = cells.get_mut(c) {
                    slot.clone_from(&hex);
                }
            }
            Ok(json!(cells))
        }
        "tool" => {
            let tool = str_at(args, "tool").ok_or("tool needs a name")?;
            let keys = keys_arg(args);
            let mut cells = cells_from(at(args, "cells"));
            paint_tool(tool, &mut cells, &keys, &cells_arg(args, "selection"), args)?;
            Ok(json!(cells))
        }
        "pick_effect" => pick_effect(str_at(args, "name").unwrap_or(""), at(args, "current").unwrap_or(&Value::Null)),
        "rear_request" => rear_request(args),
        "preview" => Ok(preview(at(args, "look").unwrap_or(&Value::Null), at(args, "cells"))),
        "theme_settings" => Ok(theme_settings(at(args, "theme").unwrap_or(&Value::Null))),
        "theme_request" => theme_request(args),
        "theme_for_save" => {
            let taken: Vec<String> = list_at(args, "taken").unwrap_or_default().iter().filter_map(|t| t.as_str().map(str::to_owned)).collect();
            theme_for_save(args, &taken)
        }
        "clock_text" => Ok(json!(clock_text(f64_at(args, "hours").unwrap_or(0.0)))),
        "look_summary" => Ok(look_summary(args)),
        "valid_name" => {
            let name = str_at(args, "name").unwrap_or("");
            Ok(json!(if str_at(args, "kind") == Some("set") { valid_set_name(name) } else { valid_name(name) }))
        }
        "bindings_load" => {
            let previous: Option<Bindings> = at(args, "state").and_then(|s| serde_json::from_value(s.clone()).ok());
            let result = at(args, "result").unwrap_or(&Value::Null);
            Ok(state_out(&Bindings::load(result, previous.as_ref())))
        }
        "bindings_select" => {
            let mut b = bindings_arg(args)?;
            let mode = str_at(args, "mode").ok_or("select needs a mode")?;
            if !MODES.contains(&mode) {
                return Err(format!("unknown power mode: {mode}"));
            }
            mode.clone_into(&mut b.selected);
            Ok(state_out(&b))
        }
        "bindings_choose" => {
            let mut b = bindings_arg(args)?;
            let mode = str_at(args, "mode").unwrap_or(&b.selected.clone()).to_owned();
            b.choose(&mode, str_at(args, "look").map(str::to_owned))?;
            Ok(state_out(&b))
        }
        "bindings_undo" => {
            let mut b = bindings_arg(args)?;
            let mode = str_at(args, "mode").unwrap_or(&b.selected.clone()).to_owned();
            b.undo(&mode)?;
            Ok(state_out(&b))
        }
        "bindings_discard" => {
            let mut b = bindings_arg(args)?;
            let mode = str_at(args, "mode").unwrap_or(&b.selected.clone()).to_owned();
            b.discard(&mode)?;
            Ok(state_out(&b))
        }
        "bindings_plan" => {
            let b = bindings_arg(args)?;
            let mode = str_at(args, "mode").unwrap_or(&b.selected.clone()).to_owned();
            Ok(json!(b.plan(&mode)?))
        }
        _ => Err(format!("unknown lighting verb: {verb}")),
    }
}

/// `select {op, keys, selection, ...}`: the selection after `op` = `box` (`from`, `to`, `add`),
/// `group` (`group`), `toggle` (`cell`), `invert`, `all`, `none`.
fn select(args: &Value) -> Result<Value, String> {
    let keys = keys_arg(args);
    let current = tidy(cells_arg(args, "selection"));
    let out = match str_at(args, "op").unwrap_or("") {
        "none" => Vec::new(),
        "all" => (0..KEY_CELLS).collect(),
        "invert" => (0..KEY_CELLS).filter(|c| !current.contains(c)).collect(),
        "group" => tidy(group_cells(str_at(args, "group").unwrap_or(""), &keys)),
        "toggle" => {
            let cell = usize::try_from(i64_at(args, "cell").ok_or("toggle needs a cell")?).map_err(|e| e.to_string())?;
            // A key can own several cells: toggle them all together.
            let owner: Vec<usize> = keys.iter().find(|k| k.cells.contains(&cell)).map_or_else(|| vec![cell], |k| k.cells.clone());
            if owner.iter().all(|c| current.contains(c)) {
                current.into_iter().filter(|c| !owner.contains(c)).collect()
            } else {
                tidy(current.into_iter().chain(owner))
            }
        }
        "box" => {
            let a = point(args, "from").ok_or("box needs from")?;
            let b = point(args, "to").unwrap_or(a);
            let inside = cells_in_box(&keys, a, b);
            if args.get("add").and_then(Value::as_bool).unwrap_or(false) {
                tidy(current.into_iter().chain(inside))
            } else {
                tidy(inside)
            }
        }
        other => return Err(format!("unknown selection operation: {other}")),
    };
    Ok(json!(out))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn real_layout() -> Layout {
        let v: Value = serde_json::from_str(include_str!("../../tests/lighting-keys.json")).unwrap();
        build_layout(&name_cells(&v))
    }

    fn cells_all(hex: &str) -> Vec<String> {
        vec![hex.to_owned(); CELLS]
    }

    // ---- colour ----

    #[test]
    fn hex_input_is_normalised_and_junk_refused() {
        assert_eq!(norm_hex("#FF3B30").as_deref(), Some("ff3b30"));
        assert_eq!(norm_hex("  f30 ").as_deref(), Some("ff3300"));
        for bad in ["", "#", "12345", "1234567", "gggggg", "ff 3b30", "ünï", "#12345g", "-12345"] {
            assert_eq!(norm_hex(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn hsv_round_trips_primary_and_grey_colours() {
        for (hex, h, s, v) in [("ff0000", 0.0, 1.0, 1.0), ("00ff00", 120.0, 1.0, 1.0), ("0000ff", 240.0, 1.0, 1.0), ("808080", 0.0, 0.0, 128.0 / 255.0)] {
            let got = rgb_to_hsv(parse_rgb(hex).unwrap());
            assert!((got.h - h).abs() < 1e-9 && (got.s - s).abs() < 1e-9 && (got.v - v).abs() < 1e-9, "{hex}: {got:?}");
            assert_eq!(to_hex(hsv_to_rgb(got)), hex);
        }
        // Every hue survives a round trip within one step of rounding.
        for deg in (0..360).step_by(7) {
            let c = hsv_to_rgb(Hsv { h: f64::from(deg), s: 0.8, v: 0.9 });
            let back = hsv_to_rgb(rgb_to_hsv(c));
            for i in 0..3 {
                assert!(c[i].abs_diff(back[i]) <= 1, "hue {deg}");
            }
        }
    }

    #[test]
    fn hsv_wraps_hue_and_clamps_the_rest() {
        assert_eq!(hsv_to_rgb(Hsv { h: 360.0, s: 1.0, v: 1.0 }), [255, 0, 0]);
        assert_eq!(hsv_to_rgb(Hsv { h: -120.0, s: 1.0, v: 1.0 }), [0, 0, 255]);
        assert_eq!(hsv_to_rgb(Hsv { h: 10.0, s: 5.0, v: -1.0 }), [0, 0, 0]);
        assert_eq!(hsv_to_rgb(Hsv { h: 10.0, s: -5.0, v: 2.0 }), [255, 255, 255]);
    }

    #[test]
    fn hue_ramp_matches_the_keymap_ramp_at_the_ends_and_wraps() {
        assert_eq!(hue_ramp(0.0), [255, 0, 0]);
        assert_eq!(hue_ramp(1.0), [255, 0, 0]);
        assert_eq!(hue_ramp(1.0 / 3.0), [0, 255, 0]);
        assert_eq!(hue_ramp(2.0 / 3.0), [0, 0, 255]);
        assert_eq!(hue_ramp(-1.0 / 3.0), [0, 0, 255]);
    }

    #[test]
    fn text_colour_follows_luminance() {
        assert!(wants_dark_text("ffffff"));
        assert!(wants_dark_text("ffcc00"));
        assert!(!wants_dark_text("000000"));
        assert!(!wants_dark_text("007aff"));
        assert!(!wants_dark_text("nonsense"));
    }

    #[test]
    fn gradient_and_rainbow_by_x() {
        let xs = [0.0, 5.0, 10.0];
        assert_eq!(gradient_at(&xs, [0, 0, 0], [200, 100, 50]), ["000000", "643219", "c86432"]);
        assert_eq!(gradient_at(&[3.0, 3.0], [9, 9, 9], [200, 0, 0]), ["090909", "090909"], "equal x: the start colour");
        let r = rainbow_at(&xs);
        assert_eq!(r[0], "ff0000");
        assert_ne!(r[2], r[0], "85 % of the circle: the ends differ");
        assert!(gradient_at(&[], [0; 3], [0; 3]).is_empty());
    }

    #[test]
    fn stock_cells_are_cyan_with_two_dark_spares() {
        let s = stock_cells();
        assert_eq!((s.len(), s[0].as_str(), s[125].as_str(), s[126].as_str(), s[127].as_str()), (128, "00aec7", "00aec7", "000000", "000000"));
    }

    #[test]
    fn cells_from_pads_and_sanitises() {
        let c = cells_from(Some(&json!(["FF0000", "zz", 5, "#00ff00"])));
        assert_eq!((c.len(), c[0].as_str(), c[1].as_str(), c[2].as_str(), c[3].as_str(), c[127].as_str()), (128, "ff0000", "000000", "000000", "00ff00", "000000"));
        assert_eq!(cells_from(None).len(), 128);
        assert_eq!(cells_from(Some(&json!({"not": "a list"}))).len(), 128);
    }

    // ---- the picture ----

    #[test]
    fn the_real_layout_draws_every_cell_once() {
        let l = real_layout();
        let mut seen = BTreeSet::new();
        for k in &l.keys {
            for c in &k.cells {
                assert!(seen.insert(*c), "cell {c} drawn twice ({})", k.name);
            }
        }
        assert_eq!(seen.len(), 103, "105 names, two of them aliases: 103 distinct cells");
        assert_eq!(l.height, 6.4, "no extra row: the picture knows every key");
        for name in ["esc", "space", "return", "windows", "numenter", "predator", "up"] {
            assert!(l.keys.iter().any(|k| k.name == name), "{name}");
        }
        assert!(!l.keys.iter().any(|k| k.name == "enter" || k.name == "win"), "aliases are drawn once");
    }

    #[test]
    fn rows_end_flush_and_nothing_overlaps() {
        let l = real_layout();
        for (i, a) in l.keys.iter().enumerate() {
            for b in &l.keys[i + 1..] {
                let overlap = a.x + a.w - 1e-9 > b.x && b.x + b.w - 1e-9 > a.x && a.y + a.h - 1e-9 > b.y && b.y + b.h - 1e-9 > a.y;
                assert!(!overlap, "{} overlaps {}", a.name, b.name);
            }
        }
        let right = |n: &str| l.keys.iter().find(|k| k.name == n).map(|k| k.x + k.w).unwrap();
        assert!((right("backspace") - 15.0).abs() < 1e-9);
        assert!((right("return") - 15.0).abs() < 1e-9);
        assert!((right("up") - 15.0).abs() < 1e-9);
        assert!((right("right") - 16.0).abs() < 1e-9);
        assert!((right("num.") - 20.5 + 1.0).abs() < 1e-9 || right("numenter") <= l.width + 1e-9);
        assert!(l.keys.iter().all(|k| k.x + k.w <= l.width + 1e-9));
    }

    #[test]
    fn unknown_keys_get_an_extra_row_and_junk_is_skipped() {
        let v = json!({"keys": {"zzz": [{"cell": 100}], "a": [20], "bad": "x", "far": [999], "mix": [1, {"cell": 2}]}});
        let names = name_cells(&v);
        assert_eq!(names.get("zzz"), Some(&vec![100]));
        assert!(!names.contains_key("bad") && !names.contains_key("far"));
        let l = build_layout(&names);
        let z = l.keys.iter().find(|k| k.name == "zzz").unwrap();
        assert_eq!((z.y, l.height), (6.4, 7.5));
        assert!(l.keys.iter().any(|k| k.name == "a" && k.y == 3.3));
        assert_eq!(build_layout(&BTreeMap::new()).keys.len(), 0);
    }

    #[test]
    fn labels() {
        assert_eq!(key_label("a"), "A");
        assert_eq!(key_label("lshift"), "Shift");
        assert_eq!(key_label("num7"), "7");
        assert_eq!(key_label("space"), "");
        assert_eq!(key_label("f5"), "F5");
        assert_eq!(key_label("zzzzz"), "Zzz");
    }

    #[test]
    fn hit_testing_uses_the_inset_rectangles() {
        let l = real_layout();
        let a = l.keys.iter().find(|k| k.name == "a").unwrap();
        assert_eq!(key_at(&l.keys, a.x + 0.5, a.y + 0.5).map(|k| k.name.as_str()), Some("a"));
        assert!(key_at(&l.keys, a.x + 0.01, a.y + 0.5).is_none(), "inside the gap between keys");
        assert!(key_at(&l.keys, -3.0, -3.0).is_none());
        assert!(key_at(&l.keys, f64::NAN, 1.0).is_none());
        let space = l.keys.iter().find(|k| k.name == "space").unwrap();
        assert_eq!(key_at(&l.keys, space.x + 2.0, space.y + 0.5).map(|k| k.name.as_str()), Some("space"));
    }

    #[test]
    fn a_box_selects_every_key_it_touches_in_any_corner_order() {
        let l = real_layout();
        let cells = cells_in_box(&l.keys, (2.0, 3.5), (0.2, 2.4));
        let names: BTreeSet<&str> = l.keys.iter().filter(|k| k.cells.iter().any(|c| cells.contains(c))).map(|k| k.name.as_str()).collect();
        assert!(["tab", "q", "caps", "a"].iter().all(|n| names.contains(n)), "{names:?}");
        assert!(!names.contains("w") || l.keys.iter().find(|k| k.name == "w").is_some_and(|w| w.x < 1.5));
        assert_eq!(cells_in_box(&l.keys, (50.0, 50.0), (60.0, 60.0)), Vec::<usize>::new());
    }

    #[test]
    fn a_fast_drag_paints_every_key_on_the_line_in_order() {
        let l = real_layout();
        let row = l.keys.iter().filter(|k| k.y == 3.3 && k.x < 15.0).collect::<Vec<_>>();
        let y = 3.3 + 0.5;
        // From the left edge of the home row to the right edge in one jump.
        let hits = keys_on_segment(&l.keys, (0.1, y), (14.9, y));
        assert_eq!(hits.len(), row.len(), "every home row key, none skipped");
        assert!(hits.windows(2).all(|w| w[0].x < w[1].x), "ordered along the drag");
        // The same, dragged right to left, reverses the order.
        let back = keys_on_segment(&l.keys, (14.9, y), (0.1, y));
        assert!(back.windows(2).all(|w| w[0].x > w[1].x));
        // A click (no movement) is one key; a click in a gap is none.
        let a = l.keys.iter().find(|k| k.name == "a").unwrap();
        assert_eq!(keys_on_segment(&l.keys, (a.x + 0.5, y), (a.x + 0.5, y)).len(), 1);
        assert!(keys_on_segment(&l.keys, (a.x + 0.01, y), (a.x + 0.01, y)).is_empty());
        // A diagonal crossing three rows.
        let diag = keys_on_segment(&l.keys, (3.8, 1.8), (3.8, 5.8));
        assert_eq!(diag.len(), 5, "{:?}", diag.iter().map(|k| &k.name).collect::<Vec<_>>());
        assert!(keys_on_segment(&l.keys, (f64::NAN, 0.0), (1.0, 1.0)).is_empty());
    }

    #[test]
    fn arrow_navigation_moves_to_the_nearest_key_in_that_direction() {
        let l = real_layout();
        let cell = |n: &str| l.keys.iter().find(|k| k.name == n).unwrap().cells[0];
        assert_eq!(neighbour(&l.keys, cell("g"), "right"), Some(cell("h")));
        assert_eq!(neighbour(&l.keys, cell("g"), "left"), Some(cell("f")));
        assert_eq!(neighbour(&l.keys, cell("g"), "up"), Some(cell("t")));
        assert!(["b", "v"].iter().any(|n| neighbour(&l.keys, cell("g"), "down") == Some(cell(n))));
        assert_eq!(neighbour(&l.keys, cell("esc"), "left"), None, "no key to the left of the corner");
        assert_eq!(neighbour(&l.keys, cell("g"), "sideways"), None);
        assert_eq!(neighbour(&l.keys, 500, "left"), None);
        assert_eq!(call("neighbour", &json!({"keys": serde_json::to_value(&l.keys).unwrap(), "cell": cell("g"), "dir": "right"})).unwrap(), json!(cell("h")));
        assert_eq!(call("inks", &json!({"cells": ["ffffff", "000000"]})).unwrap()[0], json!(true));
    }

    #[test]
    fn groups_pick_the_documented_keys() {
        let l = real_layout();
        assert_eq!(group_cells("all", &l.keys).len(), 126);
        assert_eq!(group_cells("wasd", &l.keys).len(), 4);
        assert_eq!(group_cells("arrows", &l.keys).len(), 4);
        assert_eq!(group_cells("fkeys", &l.keys).len(), 12);
        assert_eq!(group_cells("numrow", &l.keys).len(), 10);
        assert_eq!(group_cells("letters", &l.keys).len(), 26);
        assert_eq!(group_cells("numpad", &l.keys).len(), 14, "num0..9, num/, num., numenter, numlock");
        assert!(group_cells("nope", &l.keys).is_empty());
    }

    // ---- painting and selection through the verbs ----

    fn layout_json() -> Value {
        serde_json::to_value(real_layout()).unwrap()
    }

    #[test]
    fn stroke_paints_the_crossed_keys_and_reports_whether_anything_changed() {
        let layout = layout_json();
        let args = json!({"keys": layout["keys"], "from": {"x": 0.1, "y": 3.8}, "to": {"x": 14.9, "y": 3.8}, "colour": "FF0000", "cells": cells_all("000000")});
        let r = call("stroke", &args).unwrap();
        assert_eq!(r["touched"].as_array().unwrap().len(), 13, "caps a s d f g h j k l ; ' return");
        assert_eq!(r["changed"], true);
        let painted = r["cells"].as_array().unwrap().iter().filter(|c| *c == "ff0000").count();
        assert_eq!(painted, 13);
        // Painting the same colour again changes nothing: no upload needed.
        let again = call("stroke", &json!({"keys": layout["keys"], "from": {"x": 0.1, "y": 3.8}, "to": {"x": 14.9, "y": 3.8}, "colour": "ff0000", "cells": r["cells"]})).unwrap();
        assert_eq!(again["changed"], false);
        // No colour: only the keys are reported (the eyedropper and hover).
        let dry = call("stroke", &json!({"keys": layout["keys"], "from": {"x": 0.1, "y": 3.8}, "to": {"x": 0.1, "y": 3.8}})).unwrap();
        assert_eq!((dry["changed"].clone(), dry["touched"].as_array().unwrap().len()), (json!(false), 1));
    }

    #[test]
    fn tools_fill_gradient_rainbow_clear_and_their_selections() {
        let layout = layout_json();
        let base = |extra: Value| {
            let mut a = json!({"keys": layout["keys"], "cells": cells_all("111111")});
            a.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            a
        };
        let all_fill = call("tool", &base(json!({"tool": "fill", "colour": "abcdef"}))).unwrap();
        let v = all_fill.as_array().unwrap();
        assert_eq!((v[0].as_str(), v[125].as_str(), v[126].as_str(), v[127].as_str()), (Some("abcdef"), Some("abcdef"), Some("111111"), Some("111111")), "spare cells untouched");
        let sel = json!([0, 1, 2]);
        let part = call("tool", &base(json!({"tool": "clear", "selection": sel}))).unwrap();
        assert_eq!(part.as_array().unwrap().iter().filter(|c| *c == "000000").count(), 3);
        // A gradient runs across the selected keys by x.
        let wasd: Vec<usize> = call("select", &json!({"op": "group", "group": "wasd", "keys": layout["keys"]})).unwrap().as_array().unwrap().iter().map(|c| c.as_u64().unwrap() as usize).collect();
        let g = call("tool", &base(json!({"tool": "gradient", "from": "000000", "to": "ffffff", "selection": wasd}))).unwrap();
        let g = g.as_array().unwrap();
        let shade = |name: &str| {
            let cell = real_layout().keys.iter().find(|k| k.name == name).unwrap().cells[0];
            u8::from_str_radix(&g[cell].as_str().unwrap()[0..2], 16).unwrap()
        };
        assert_eq!(shade("a"), 0, "leftmost key gets the start");
        assert!(shade("s") < shade("d"), "left to right");
        assert_eq!(shade("d"), 255, "wasd: a is leftmost and d rightmost");
        let rb = call("tool", &base(json!({"tool": "rainbow", "selection": [0, 6, 12]}))).unwrap();
        assert_eq!(rb.as_array().unwrap().iter().filter(|c| *c != "111111").count(), 3);
        for bad in [json!({"tool": "nope"}), json!({"tool": "fill"}), json!({"tool": "gradient"}), json!({})] {
            assert!(call("tool", &base(bad)).is_err());
        }
    }

    #[test]
    fn selection_operations() {
        let layout = layout_json();
        let sel = |args: Value| -> Vec<u64> {
            let mut a = json!({"keys": layout["keys"]});
            a.as_object_mut().unwrap().extend(args.as_object().unwrap().clone());
            call("select", &a).unwrap().as_array().unwrap().iter().map(|c| c.as_u64().unwrap()).collect()
        };
        assert_eq!(sel(json!({"op": "none"})), Vec::<u64>::new());
        assert_eq!(sel(json!({"op": "all"})).len(), 126);
        assert_eq!(sel(json!({"op": "invert", "selection": [0, 1]})).len(), 124);
        assert_eq!(sel(json!({"op": "toggle", "cell": 5, "selection": [1, 2]})), vec![1, 2, 5]);
        assert_eq!(sel(json!({"op": "toggle", "cell": 5, "selection": [1, 5]})), vec![1]);
        assert_eq!(sel(json!({"op": "toggle", "cell": 500})), Vec::<u64>::new(), "an out of range cell is no key");
        let boxed = sel(json!({"op": "box", "from": {"x": 0.1, "y": 3.5}, "to": {"x": 0.4, "y": 3.9}}));
        assert_eq!(boxed.len(), 1);
        let added = sel(json!({"op": "box", "add": true, "selection": [100], "from": {"x": 0.1, "y": 3.5}, "to": {"x": 0.4, "y": 3.9}}));
        assert_eq!(added.len(), 2);
        assert!(call("select", &json!({"op": "wat"})).is_err());
    }

    // ---- effects ----

    #[test]
    fn the_animations_that_make_their_own_colours_are_real_animations_and_ignore_the_base() {
        let own = call("fx_own_colours", &json!({})).unwrap();
        let own: Vec<&str> = own.as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        for m in &own {
            assert!(FX_MODES.contains(m), "{m} is an animation");
        }
        // Drawn over two very different bases, these modes give the same frame: the base is ignored.
        // The base-coloured ones (pulse, breathing, scanner...) do not.
        let red = vec![[255u8, 0, 0]; 128];
        let blue = vec![[0u8, 0, 255]; 128];
        for m in FX_MODES {
            // (Only the animated keys: cells past the lit ones keep their base by design.)
            let a = ts_domain::perkeyfx::render(m, 1.3, &red, &[], 5)[..120].to_vec();
            let b = ts_domain::perkeyfx::render(m, 1.3, &blue, &[], 5)[..120].to_vec();
            assert_eq!(a == b, own.contains(&m), "{m}: own colours = {}", a == b);
        }
    }

    #[test]
    fn the_catalogue_has_thirteen_effects_with_the_hardware_rules() {
        assert_eq!(EFFECTS.len(), 13);
        let ids: BTreeSet<&str> = EFFECTS.iter().map(|e| e.id).collect();
        assert_eq!(ids.len(), 13);
        for e in EFFECTS {
            assert!(e.icon.starts_with("lighting/"));
        }
        // keyboard.py NO_COLOR = off, wave, neon; static has no speed.
        let no_colour: Vec<&str> = EFFECTS.iter().filter(|e| !e.colour).map(|e| e.id).collect();
        assert_eq!(no_colour, ["wave", "neon"]);
        assert!(!effect("static").unwrap().speed);
        assert!(effect("breathing").unwrap().speed);
        assert_eq!(effect("neon").unwrap().pick_floor, 50);
        let with_direction: Vec<&str> = EFFECTS.iter().filter(|e| e.direction).map(|e| e.id).collect();
        assert_eq!(with_direction, ["wave"], "only the wave honours a direction (reference, PH16-71)");
        assert_eq!(call("effects", &json!({})).unwrap().as_array().unwrap().len(), 13);
    }

    #[test]
    fn the_wave_preview_follows_its_direction() {
        let shot = |direction: i64| {
            let p = preview(&json!({"mode": "effect", "name": "wave", "brightness": 50, "direction": direction}), None);
            p["cells"].as_array().unwrap().iter().map(|c| c.as_str().unwrap().to_owned()).collect::<Vec<_>>()
        };
        let cell = |col: usize, row: usize| col * 6 + row;
        let (ltr, rtl, up, down) = (shot(0), shot(1), shot(2), shot(3));
        // Left to right: same hue down a column, different along a row; right to left mirrors it.
        assert_eq!(ltr[cell(3, 0)], ltr[cell(3, 5)]);
        assert_ne!(ltr[cell(3, 0)], ltr[cell(15, 0)]);
        assert_eq!(rtl[cell(3, 2)], ltr[cell(18, 2)]);
        // Vertical directions: the same hue along a row, different down a column, opposite to each other.
        assert_eq!(up[cell(2, 1)], up[cell(17, 1)]);
        assert_ne!(up[cell(2, 0)], up[cell(2, 5)]);
        assert_eq!(up[cell(2, 0)], down[cell(2, 5)]);
        // Around the middle: the two rotations are mirror images and differ from the straight ones.
        let (cw, ccw) = (shot(4), shot(5));
        assert_ne!(cw, ccw);
        assert_ne!(cw, ltr);
        assert_ne!(cw[cell(11, 0)], ccw[cell(11, 0)], "above the middle the angle has opposite signs");
        assert_eq!(cw[cell(11, 0)], ccw[cell(11, 5)], "the mirror image: above and below swap");
    }

    #[test]
    fn only_the_wave_keeps_a_direction_when_picked() {
        for d in 0..=5 {
            let r = pick_effect("wave", &json!({"direction": d})).unwrap();
            assert_eq!(r["direction"], d);
        }
        assert_eq!(pick_effect("wave", &json!({"direction": 6})).unwrap()["direction"], 0);
        assert_eq!(pick_effect("wave", &json!({"direction": -1})).unwrap()["direction"], 0);
        for e in EFFECTS.iter().filter(|e| e.id != "wave") {
            assert_eq!(pick_effect(e.id, &json!({"direction": 4})).unwrap()["direction"], 0, "{}", e.id);
        }
    }

    #[test]
    fn the_rear_bar_keeps_the_direction_control_on_the_wave_only() {
        let rows = call("rear_effects", &json!({})).unwrap();
        let with: Vec<&str> =
            rows.as_array().unwrap().iter().filter(|r| r["direction"] == true).map(|r| r["id"].as_str().unwrap()).collect();
        assert_eq!(with, ["wave"]);
    }

    #[test]
    fn picking_an_effect_keeps_the_options_and_makes_it_visible() {
        let r = pick_effect("wave", &json!({"colour": "FF0000", "speed": 9, "brightness": 30, "direction": 1})).unwrap();
        assert_eq!(r, json!({"name": "wave", "color": "ff0000", "speed": 9, "brightness": 30, "direction": 1}));
        let dark = pick_effect("snake", &json!({"brightness": 0})).unwrap();
        assert_eq!((dark["brightness"].clone(), dark["color"].clone(), dark["speed"].clone()), (json!(10), json!("00aec7"), json!(5)));
        assert_eq!(pick_effect("neon", &json!({"brightness": 25})).unwrap()["brightness"], 50, "neon needs 50 to show");
        assert_eq!(pick_effect("neon", &json!({"brightness": 50})).unwrap()["brightness"], 50);
        // Junk in the current state falls back to defaults and stays in the hardware's ranges.
        let junk = pick_effect("static", &json!({"colour": "zz", "speed": 99, "brightness": 999, "direction": 7})).unwrap();
        assert_eq!(junk, json!({"name": "static", "color": "00aec7", "speed": 5, "brightness": 50, "direction": 0}));
        assert!(pick_effect("operatingmode", &json!({})).is_err());
        assert!(call("pick_effect", &json!({"name": "nope"})).is_err());
    }

    #[test]
    fn rear_requests_follow_the_zone_and_brightness_rules() {
        let s = rear_request(&json!({"effect": "static", "colour": "112233", "zones": ["ff0000", "00ff00", "0000ff"], "speed": 3, "brightness": 70})).unwrap();
        assert_eq!(s["args"], json!({"effect": "static", "speed": 3, "brightness": 70, "direction": 1, "zones": ["ff0000", "00ff00", "0000ff"]}));
        assert_eq!(s["turned_on"], false);
        let w = rear_request(&json!({"effect": "wave", "colour": "#AABBCC", "speed": 9, "brightness": 400, "direction": 0})).unwrap();
        assert_eq!(w["args"], json!({"effect": "wave", "speed": 5, "brightness": 100, "direction": 0, "color": "aabbcc"}));
        // Missing zones copy the single colour.
        let z = rear_request(&json!({"effect": "static", "colour": "010203", "zones": [null, "ffffff"]})).unwrap();
        assert_eq!(z["args"]["zones"], json!(["010203", "ffffff", "010203"]));
        // Brightness 0 means off: picking an effect turns the bar back on.
        let on = rear_request(&json!({"effect": "breathing", "brightness": 0, "last_brightness": 40})).unwrap();
        assert_eq!((on["brightness"].clone(), on["turned_on"].clone()), (json!(40), json!(true)));
        assert_eq!(rear_request(&json!({"brightness": 0})).unwrap()["brightness"], json!(100));
        assert!(rear_request(&json!({"effect": "strobe"})).is_err());
    }

    // ---- preview ----

    #[test]
    fn preview_shows_the_current_look() {
        let still = preview(&json!({"mode": "effect", "name": "static", "color": "ff0000", "brightness": 50}), None);
        assert_eq!((still["kind"].clone(), still["exact"].clone()), (json!("effect"), json!(true)));
        let c = still["cells"].as_array().unwrap();
        assert_eq!((c.len(), c[0].as_str(), c[126].as_str()), (128, Some("ff0000"), Some("000000")));
        // Dim at low brightness but not black; black at 0.
        let low = preview(&json!({"mode": "effect", "name": "static", "color": "ffffff", "brightness": 5}), None);
        let first = low["cells"][0].as_str().unwrap();
        assert!(first != "ffffff" && first != "000000", "{first}");
        let zero = preview(&json!({"mode": "effect", "name": "static", "color": "ffffff", "brightness": 0}), None);
        assert!(zero["cells"].as_array().unwrap().iter().all(|c| c == "000000"));
        // A colour-less effect draws the hue ramp, approximately.
        let neon = preview(&json!({"mode": "effect", "name": "neon", "color": "ff0000", "brightness": 50}), None);
        assert_eq!(neon["exact"], false);
        assert_ne!(neon["cells"][0], neon["cells"][60]);
        let spot = preview(&json!({"mode": "effect", "name": "spot", "color": "00ff00", "brightness": 50}), None);
        assert_eq!(spot["exact"], false);
        // The per-key state is exact when the cells are known and unknown otherwise.
        let mut known = cells_all("123456");
        known[3] = "ff00ff".into();
        let pk = preview(&json!({"mode": "perkey", "brightness": 50}), Some(&json!(known)));
        assert_eq!((pk["kind"].clone(), pk["exact"].clone(), pk["cells"][3].clone()), (json!("perkey"), json!(true), json!("ff00ff")));
        assert_eq!(preview(&json!({"mode": "perkey"}), None)["kind"], "unknown");
        for look in [json!({}), json!(null), json!({"mode": 5}), json!({"mode": "mystery"})] {
            assert_eq!(preview(&look, None)["kind"], "unknown");
        }
        assert_eq!(preview(&json!({"mode": "off"}), None)["kind"], "off");
    }

    // ---- names and summaries ----

    #[test]
    fn name_rules() {
        for ok in ["a", "late-night", "my.look_2", &"x".repeat(40)] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", " ", "a b", "ä", "../x", "a/b", &"x".repeat(41)] {
            assert!(!valid_name(bad), "{bad:?}");
        }
        assert!(valid_set_name("my_set-1") && !valid_set_name("my.set"));
        assert_eq!(call("valid_name", &json!({"name": "a.b", "kind": "set"})).unwrap(), json!(false));
    }

    #[test]
    fn look_summary_splits_the_parts() {
        let look = json!({"keyboard": {"mode": "effect", "name": "wave", "brightness": 20, "speed": 4}, "rear": {"mode": "static", "brightness": 80}});
        let s = look_summary(&look);
        assert_eq!(s["keyboard"]["name"], "wave");
        assert_eq!(s["rear"]["mode"], "static");
        assert!(s["theme"].is_null());
        let pk = look_summary(&json!({"keyboard": {"mode": "perkey"}, "rear": null}));
        assert_eq!((pk["keyboard"]["kind"].clone(), pk["rear"].clone()), (json!("perkey"), Value::Null));
        assert!(look_summary(&json!({}))["keyboard"].is_null());
    }

    // ---- bindings ----

    fn answer(binds: Value) -> Value {
        json!({"mode": "turbo", "bindings": binds})
    }

    fn loaded() -> Bindings {
        Bindings::load(
            &answer(json!({
                "turbo": {"look": {"name": "red", "data": {}}, "fan_curve": {"name": "loud", "data": {}}},
                "eco": {"look": {"name": "calm", "data": {}}, "fan_curve": null},
            })),
            None,
        )
    }

    #[test]
    fn loading_reads_looks_curves_and_selects_the_current_mode() {
        let b = loaded();
        assert_eq!((b.selected.as_str(), b.current.as_deref()), ("turbo", Some("turbo")));
        assert_eq!((b.modes["turbo"].saved.as_deref(), b.modes["turbo"].curve.as_deref()), (Some("red"), Some("loud")));
        assert_eq!((b.modes["eco"].saved.as_deref(), b.modes["eco"].curve.as_deref()), (Some("calm"), None));
        assert_eq!(b.modes["quiet"], ModeEntry::default());
        assert!(b.modes.values().all(|e| !e.changed()));
        assert_eq!(Bindings::load(&json!({"mode": "warp"}), None).selected, "balanced");
        assert_eq!(Bindings::load(&Value::Null, None).modes.len(), 5);
    }

    #[test]
    fn choosing_marks_changed_and_undo_walks_back() {
        let mut b = loaded();
        b.choose("eco", Some("night".into())).unwrap();
        assert!(b.modes["eco"].changed());
        b.choose("eco", Some("night".into())).unwrap();
        assert_eq!(b.modes["eco"].steps.len(), 1, "choosing the same thing is not a step");
        b.choose("eco", None).unwrap();
        assert!(b.undo("eco").unwrap());
        assert_eq!(b.modes["eco"].draft.as_deref(), Some("night"));
        assert!(b.undo("eco").unwrap());
        assert_eq!(b.modes["eco"].draft.as_deref(), Some("calm"));
        assert!(!b.modes["eco"].changed(), "back at the saved look: not changed");
        assert!(!b.undo("eco").unwrap());
        assert!(b.choose("warp", None).is_err() && b.undo("warp").is_err() && b.discard("warp").is_err());
        // The history is bounded.
        for i in 0..100 {
            b.choose("quiet", Some(format!("l{i}"))).unwrap();
        }
        assert_eq!(b.modes["quiet"].steps.len(), 40);
    }

    #[test]
    fn discard_restores_the_saved_look() {
        let mut b = loaded();
        b.choose("turbo", None).unwrap();
        b.choose("turbo", Some("x".into())).unwrap();
        b.discard("turbo").unwrap();
        assert_eq!((b.modes["turbo"].draft.as_deref(), b.modes["turbo"].steps.len()), (Some("red"), 0));
    }

    #[test]
    fn the_save_plan_binds_unbinds_or_does_nothing() {
        let mut b = loaded();
        assert!(b.plan("turbo").unwrap().is_empty());
        b.choose("turbo", Some("blue".into())).unwrap();
        assert_eq!(b.plan("turbo").unwrap(), vec![json!({"area": "mode", "verb": "bind", "args": {"mode": "turbo", "look": "blue"}})]);
        b.choose("eco", None).unwrap();
        assert_eq!(b.plan("eco").unwrap(), vec![json!({"area": "mode", "verb": "unbind", "args": {"mode": "eco", "what": "look"}})]);
        b.choose("quiet", Some("a".into())).unwrap();
        b.choose("quiet", None).unwrap();
        assert!(b.plan("quiet").unwrap().is_empty(), "nothing bound and nothing chosen");
        assert!(b.plan("warp").is_err());
    }

    #[test]
    fn a_reload_keeps_unsaved_choices_and_follows_saved_ones() {
        let mut b = loaded();
        b.choose("eco", Some("night".into())).unwrap();
        // The daemon now says turbo has another look (set from the CLI); eco's draft is still being edited.
        let fresh = answer(json!({"turbo": {"look": {"name": "green"}}, "eco": {"look": {"name": "calm"}}}));
        let r = Bindings::load(&fresh, Some(&b));
        assert_eq!(r.modes["eco"].draft.as_deref(), Some("night"));
        assert_eq!(r.modes["eco"].steps.len(), 1);
        assert_eq!(r.modes["turbo"].draft.as_deref(), Some("green"), "untouched drafts follow the daemon");
        assert_eq!(r.selected, "turbo");
        // After saving, the daemon answers with the new look and the badge goes away.
        let saved = Bindings::load(&answer(json!({"eco": {"look": {"name": "night"}}})), Some(&r));
        assert!(!saved.modes["eco"].changed() && saved.modes["eco"].steps.is_empty());
    }

    #[test]
    fn the_bindings_verbs_round_trip_the_state_through_json() {
        let first = call("bindings_load", &json!({"result": answer(json!({"eco": {"look": {"name": "calm"}, "fan_curve": {"name": "quiet"}}}))})).unwrap();
        let state = first["state"].clone();
        let sel = call("bindings_select", &json!({"state": state, "mode": "eco"})).unwrap();
        assert_eq!(sel["view"]["selected"], "eco");
        let ch = call("bindings_choose", &json!({"state": sel["state"], "look": "night"})).unwrap();
        let eco = &ch["view"]["modes"][0];
        assert_eq!((eco["mode"].clone(), eco["changed"].clone(), eco["can_undo"].clone(), eco["curve"].clone()), (json!("eco"), json!(true), json!(true), json!("quiet")));
        let plan = call("bindings_plan", &json!({"state": ch["state"]})).unwrap();
        assert_eq!(plan[0]["args"]["look"], "night");
        let undone = call("bindings_undo", &json!({"state": ch["state"], "mode": "eco"})).unwrap();
        assert_eq!(undone["view"]["modes"][0]["changed"], false);
        let discarded = call("bindings_discard", &json!({"state": ch["state"]})).unwrap();
        assert_eq!(discarded["view"]["modes"][0]["look"], "calm");
        assert!(call("bindings_choose", &json!({"look": "x"})).is_err(), "no state");
        assert!(call("bindings_select", &json!({"state": ch["state"], "mode": "warp"})).is_err());
    }

    // ---- themes ----

    fn sample_theme() -> Value {
        json!({
            "id": "predator", "name": "Predator", "top": 1, "mood": "m", "suits": ["x"],
            "colourways": [{"name": "Predator", "palette": ["#00E5FF", "#0090FF"]}, {"name": "Crimson", "palette": ["#FF1A3C", "#FF0080"]}],
            "custom": ["a", "b"], "palette": ["#00E5FF", "#0090FF"],
            "keyboard": {"effect": "beacon", "params": {"anchor": 2, "interval": 4, "wide": 1}},
            "bar": {"hw": "ripple", "colors": [0, 1, 0], "speed": 2},
            "logo": {"hw": "static", "colors": [0]},
            "settings": [
                {"id": "anchor", "label": "Start", "type": "choice", "target": "keyboard.params.anchor", "options": ["WASD", "Arrows", "Centre"], "values": [0, 1, 2]},
                {"id": "interval", "label": "Every", "type": "number", "target": "keyboard.params.interval", "min": 1, "max": 9, "step": 1, "unit": "s"},
                {"id": "wide", "label": "Wide", "type": "toggle", "target": "keyboard.params.wide", "on": 1, "off": 0},
                {"id": "barspeed", "label": "Bar", "type": "number", "target": "bar.speed", "min": 0, "max": 5, "step": 1},
                {"id": "wake", "label": "Wake", "type": "number", "target": "keyboard.params.wake", "min": 0, "max": 24, "step": 0.5, "unit": "h"},
                {"id": "keys", "label": "Keys", "type": "keys", "target": "keyboard.params.keys"},
                {"id": "nope", "label": "Odd", "type": "mystery", "target": "keyboard.params.x"}
            ]
        })
    }

    #[test]
    fn theme_settings_read_the_values_the_looks_hold() {
        let s = theme_settings(&sample_theme());
        let s = s.as_array().unwrap();
        assert_eq!(s.len(), 6, "the mystery type is skipped");
        assert_eq!((s[0]["kind"].clone(), s[0]["value"].clone()), (json!("choice"), json!(2)), "the index of 2 in values");
        assert_eq!((s[1]["value"].clone(), s[1]["max"].clone(), s[1]["clock"].clone()), (json!(4.0), json!(9.0), json!(false)));
        assert_eq!((s[2]["kind"].clone(), s[2]["value"].clone()), (json!("toggle"), json!(true)));
        assert_eq!(s[3]["value"], json!(2.0), "a bar speed target");
        assert_eq!((s[4]["clock"].clone(), s[4]["value"].clone()), (json!(true), json!(0.0)), "missing: the minimum");
        assert_eq!((s[5]["kind"].clone(), s[5]["value"].clone()), (json!("keys"), json!([])));
        assert!(theme_settings(&Value::Null).as_array().unwrap().is_empty());
        assert_eq!(clock_text(6.5), "06:30");
        assert_eq!(clock_text(24.0), "00:00");
        assert_eq!(clock_text(-1.0), "23:00");
    }

    #[test]
    fn theme_requests_clamp_and_carry_the_colours() {
        let r = theme_request(&json!({"name": "predator", "colourway": 1, "values": {"anchor": 1}, "speed": 500, "brightness": 0})).unwrap();
        assert_eq!(r, json!({"name": "predator", "colourway": 1, "params": {"anchor": 1}, "speed": 100, "brightness": 1}));
        let c = theme_request(&json!({"name": "x", "colourway": "custom", "custom": ["abc", "zz", "#00ff00"]})).unwrap();
        assert_eq!(c["custom"], json!(["#AABBCC", "#00FF00"]));
        assert_eq!(c["colourway"], "custom");
        assert_eq!(theme_request(&json!({"name": "x", "speed": 3})).unwrap()["speed"], json!(10));
        assert!(theme_request(&json!({})).is_err());
    }

    #[test]
    fn theme_ids_come_from_names() {
        assert_eq!(theme_id("My Night 2").as_deref(), Some("my-night-2"));
        assert_eq!(theme_id("  ÄÖ  "), None);
        assert_eq!(theme_id("a_b-c").as_deref(), Some("a_b-c"));
        assert_eq!(theme_id(&"x".repeat(41)), None);
    }

    #[test]
    fn saving_a_theme_keeps_the_colours_and_values_chosen() {
        let args = json!({"theme": sample_theme(), "name": "Night Wave", "colourway": 1, "values": {"anchor": 0, "interval": 7, "wide": false, "barspeed": 4, "keys": ["w", "a"]}});
        let t = theme_for_save(&args, &["predator".to_owned()]).unwrap();
        assert_eq!((t["id"].clone(), t["name"].clone()), (json!("night-wave"), json!("Night Wave")));
        assert_eq!(t["palette"], json!(["#FF1A3C", "#FF0080"]));
        assert_eq!(t["colourways"][0], json!({"name": "Crimson", "palette": ["#FF1A3C", "#FF0080"]}));
        assert_eq!(t["colourways"].as_array().unwrap().len(), 2, "no duplicate of the chosen way");
        assert_eq!(t["keyboard"]["params"]["anchor"], json!(0.0));
        assert_eq!(t["keyboard"]["params"]["interval"], json!(7.0));
        assert_eq!(t["keyboard"]["params"]["wide"], json!(0.0));
        assert_eq!(t["bar"]["speed"], json!(4.0));
        assert_eq!(t["keyboard"]["params"]["keys"], json!(["w", "a"]), "a target the look lacks is not invented");
        assert!(t.get("top").is_none() && t.get("suits").is_none());
        // Custom colours: the unset slot keeps the first colourway's.
        let c = theme_for_save(&json!({"theme": sample_theme(), "name": "Mine2", "colourway": "custom", "custom": ["#112233"]}), &[]).unwrap();
        assert_eq!(c["palette"], json!(["#112233", "#0090FF"]));
        assert_eq!(c["colourways"][0]["name"], "Mine");
        // Refusals say why.
        assert!(theme_for_save(&json!({"theme": sample_theme(), "name": "Predator"}), &["predator".to_owned()]).unwrap_err().contains("built-in"));
        assert!(theme_for_save(&json!({"theme": sample_theme(), "name": ""}), &[]).is_err());
        assert!(theme_for_save(&json!({"name": "x"}), &[]).is_err());
        assert!(theme_for_save(&json!({"theme": sample_theme(), "name": "!!"}), &[]).is_err());
    }

    // ---- the dispatcher ----

    #[test]
    fn dispatch_reaches_the_page_and_unknown_verbs_say_so() {
        assert_eq!(crate::vm::dispatch("lighting", "norm_hex", &json!({"s": "#ABC"})), Ok(json!("aabbcc")));
        assert!(crate::vm::dispatch("lighting", "nope", &json!({})).unwrap_err().contains("nope"));
        assert_eq!(call("hsv_to_hex", &json!({"h": 120, "s": 1, "v": 1})).unwrap(), json!("00ff00"));
        assert!(call("hex_to_hsv", &json!({"hex": "x"})).is_err());
        assert_eq!(call("palette", &json!({})).unwrap().as_array().unwrap().len(), 16);
        let modes = call("fx_modes", &json!({})).unwrap();
        assert_eq!(modes.as_array().unwrap().len(), 20);
        assert_eq!(modes[18], "wave-clockwise");
        assert_eq!(modes[19], "wave-counter-clockwise");
        assert_eq!(call("rear_effects", &json!({})).unwrap().as_array().unwrap().len(), 5);
        assert_eq!(call("stock_cells", &json!({})).unwrap().as_array().unwrap().len(), 128);
        assert!(call("layout", &json!({"keys": 5})).unwrap()["keys"].as_array().unwrap().is_empty());
    }

    #[test]
    fn hostile_arguments_never_panic() {
        let hostile = [json!(null), json!([]), json!("x"), json!({"keys": [{"cells": "x"}], "from": {"x": "a"}, "cells": 5, "state": 3, "selection": "x", "cell": -4, "targets": [-1, 1e30]})];
        for verb in ["layout", "hit", "select", "stroke", "paint", "tool", "pick_effect", "rear_request", "preview", "look_summary", "valid_name", "bindings_load", "bindings_select", "bindings_choose", "bindings_undo", "bindings_discard", "bindings_plan", "theme_settings", "theme_request", "theme_for_save", "clock_text", "hsv_to_hex", "hex_to_hsv", "norm_hex"] {
            for args in &hostile {
                let _ = call(verb, args);
            }
        }
    }
}
