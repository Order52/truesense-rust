//! Painting strokes and the effect catalogue with the rules of its options.
//!
//! A part of [`super`] (the Lighting page's view-model); it shares the parent's names.
#![forbid(unsafe_code)]

use super::*;

// ---- painting -----------------------------------------------------------------------------------

/// Cells ordered left to right by their key's centre (ties by cell number), for gradients.
pub(super) fn by_x(keys: &[Key], cells: &[usize]) -> Vec<usize> {
    let mut v = tidy(cells.iter().copied());
    v.sort_by(|a, b| x_of_cell(keys, *a).total_cmp(&x_of_cell(keys, *b)).then(a.cmp(b)));
    v
}

/// The tools that rewrite colours of several cells at once.
pub(super) fn paint_tool(tool: &str, cells: &mut [String], keys: &[Key], selection: &[usize], args: &Value) -> Result<(), String> {
    let colour = str_at(args, "colour").and_then(norm_hex);
    // No selection means every key.
    let targets: Vec<usize> = if selection.is_empty() { (0..KEY_CELLS).collect() } else { tidy(selection.iter().copied()) };
    let set = |cells: &mut [String], list: &[usize], colours: &[String]| {
        for (c, hex) in list.iter().zip(colours) {
            if let Some(slot) = cells.get_mut(*c) {
                slot.clone_from(hex);
            }
        }
    };
    match tool {
        "fill" => {
            let hex = colour.ok_or("fill needs a colour")?;
            set(cells, &targets, &vec![hex; targets.len()]);
        }
        "clear" => set(cells, &targets, &vec!["000000".to_owned(); targets.len()]),
        "gradient" => {
            let from = str_at(args, "from").and_then(parse_rgb).ok_or("gradient needs a start colour")?;
            let to = str_at(args, "to").and_then(parse_rgb).ok_or("gradient needs an end colour")?;
            let ordered = by_x(keys, &targets);
            let xs: Vec<f64> = ordered.iter().map(|c| x_of_cell(keys, *c)).collect();
            set(cells, &ordered, &gradient_at(&xs, from, to));
        }
        "rainbow" => {
            let ordered = by_x(keys, &targets);
            let xs: Vec<f64> = ordered.iter().map(|c| x_of_cell(keys, *c)).collect();
            set(cells, &ordered, &rainbow_at(&xs));
        }
        other => return Err(format!("unknown lighting tool: {other}")),
    }
    Ok(())
}

// ---- effects ------------------------------------------------------------------------------------

/// One hardware effect of the keyboard and what its options mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effect {
    /// The name `keyboard effect` takes.
    pub id: &'static str,
    /// Icon file under `qml/icons`.
    pub icon: &'static str,
    /// The firmware uses the colour (wave and neon ignore it: the command sends black).
    pub colour: bool,
    /// The effect has a speed (a static colour has none).
    pub speed: bool,
    /// The effect travels, so a direction makes sense. This only enables the control: the byte is
    /// always sent, and the keyboard ignores it where it means nothing. [U: which effects honour
    /// it was never measured on the laptop.]
    pub direction: bool,
    /// Lowest brightness at which picking this effect shows anything (neon was invisible at 25 and
    /// shows at 50: `docs/verification-log.md`; the others need only a visible level).
    pub pick_floor: i64,
}

pub(super) const fn fx(id: &'static str, icon: &'static str, colour: bool, speed: bool, direction: bool, pick_floor: i64) -> Effect {
    Effect { id, icon, colour, speed, direction, pick_floor }
}

/// The 13 effects in display order (`off` is the backlight switch, not a tile).
pub const EFFECTS: [Effect; 13] = [
    fx("static", "lighting/static", true, false, false, 10),
    fx("breathing", "lighting/breathing", true, true, false, 10),
    fx("wave", "lighting/wave", false, true, true, 10),
    fx("snake", "lighting/snake", true, true, true, 10),
    fx("ripple", "lighting/ripple", true, true, true, 10),
    fx("neon", "lighting/rainbow", false, true, true, 50),
    fx("raindrop", "lighting/rain", true, true, true, 10),
    fx("lightning", "lighting/lightning", true, true, false, 10),
    fx("spot", "lighting/spot", true, true, false, 10),
    fx("star", "lighting/stars", true, true, false, 10),
    fx("fireball", "lighting/fireball", true, true, true, 10),
    fx("snow", "lighting/snow", true, true, true, 10),
    fx("heartbeat", "lighting/heartbeat", true, true, false, 10),
];

/// Lowest and highest effect speed (1 slowest .. 12 fastest, the UI scale of `keyboard`).
pub(super) const SPEED_RANGE: (i64, i64) = (keyboard::SPEED_MIN, keyboard::SPEED_MAX);
/// The keyboard's brightness range.
pub(super) const KB_MAX: i64 = keyboard::MAX_BRIGHT;
/// The effect speed used when none is known.
pub(super) const DEFAULT_SPEED: i64 = keyboard::DEFAULT_SPEED;

pub(super) fn effect(id: &str) -> Option<&'static Effect> {
    EFFECTS.iter().find(|e| e.id == id)
}

pub(super) fn effect_json(e: &Effect) -> Value {
    json!({"id": e.id, "icon": e.icon, "colour": e.colour, "speed": e.speed, "direction": e.direction})
}

/// The arguments of `keyboard effect` for picking `name` while the lights are as `current` says
/// (`colour`, `speed`, `brightness`, `direction`, all optional). Picking an effect while the
/// backlight is dark or dim raises the brightness to the effect's floor so that the pick is
/// visible; the brightness slider itself never does this (0 stays 0).
pub fn pick_effect(name: &str, current: &Value) -> Result<Value, String> {
    let e = effect(name).ok_or_else(|| format!("unknown keyboard effect: {name}"))?;
    let colour = str_at(current, "colour").and_then(norm_hex).unwrap_or_else(|| STOCK.to_owned());
    let speed = i64_at(current, "speed").filter(|s| (SPEED_RANGE.0..=SPEED_RANGE.1).contains(s)).unwrap_or(DEFAULT_SPEED);
    let level = i64_at(current, "brightness").unwrap_or(0).clamp(0, KB_MAX);
    let direction = i64_at(current, "direction").filter(|d| matches!(d, 0 | 1)).unwrap_or(0);
    Ok(json!({
        "name": e.id, "color": colour, "speed": speed,
        "brightness": level.max(e.pick_floor), "direction": direction,
    }))
}

/// The software animations of the key service (`keyboard fx`): id and whether it needs the colours
/// under it. Speeds are 1..10.
pub(crate) const FX_MODES: [&str; 18] = [
    "pulse", "wave", "cycle", "twinkle", "rainbow", "breathing", "scanner", "comet", "ripple", "rain", "matrix", "fire",
    "plasma", "aurora", "lightning", "confetti", "starlight", "mood",
];

/// The rear bar's effects: id, icon, whether it has a speed and whether it travels (a direction
/// only makes sense for the moving ones; [U] not measured, like the keyboard's table).
pub(super) const REAR_EFFECTS: [(&str, &str, bool, bool); 5] = [
    ("static", "lighting/static", false, false),
    ("breathing", "lighting/breathing", true, false),
    ("neon", "lighting/rainbow", true, true),
    ("wave", "lighting/wave", true, true),
    ("twinkling", "lighting/stars", true, false),
];

/// The arguments of `rear set` for what the controls show. The static effect colours three parts
/// (`zones`), every other effect one colour. A brightness of 0 is "off" for the bar, so an effect
/// picked while it is off turns it back on at the last brightness (or 100); `turned_on` tells the
/// page to move its switch.
pub fn rear_request(state: &Value) -> Result<Value, String> {
    let name = str_at(state, "effect").unwrap_or("static");
    if !REAR_EFFECTS.iter().any(|(id, ..)| *id == name) {
        return Err(format!("unknown rear light effect: {name}"));
    }
    let colour = str_at(state, "colour").and_then(norm_hex).unwrap_or_else(|| STOCK.to_owned());
    let zones: Vec<String> = (0..3)
        .map(|i| {
            list_at(state, "zones")
                .and_then(|z| z.get(i))
                .and_then(Value::as_str)
                .and_then(norm_hex)
                .unwrap_or_else(|| colour.clone())
        })
        .collect();
    let speed = i64_at(state, "speed").unwrap_or(3).clamp(1, 5);
    let direction = i64_at(state, "direction").filter(|d| matches!(d, 0 | 1)).unwrap_or(1);
    let mut brightness = i64_at(state, "brightness").unwrap_or(100).clamp(0, 100);
    let turned_on = brightness == 0;
    if turned_on {
        brightness = i64_at(state, "last_brightness").filter(|b| *b > 0).unwrap_or(100).clamp(1, 100);
    }
    let mut args = json!({"effect": name, "speed": speed, "brightness": brightness, "direction": direction});
    if name == "static" {
        args["zones"] = json!(zones);
    } else {
        args["color"] = json!(colour);
    }
    Ok(json!({"args": args, "brightness": brightness, "turned_on": turned_on}))
}
