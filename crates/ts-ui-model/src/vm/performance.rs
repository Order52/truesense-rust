//! What the Performance page decides, in Rust and without Qt: the Smart auto profile edit state
//! (ranges, Changed and Undo), the game-rule list, the risk bands of power limits and undervolts
//! and the confirmation each band needs, and the plain-data views of what the daemon answers.
//!
//! Every number here mirrors a constant of the layer that enforces it: `ts_decision::smartprofile`
//! (`MODE_SETTINGS`, `NUM_SETTINGS`), `ts_decision::gamerules` (`clean_name`, `MAX_RULES`),
//! `ts_domain::power` (the PL1/PL2 bands), `ts_domain::undervolt` and `ts_domain::gpuundervolt`
//! (the safety and danger lines), and the web page's `risk.js`. This crate does not depend on
//! those crates, so the mirror is pinned by tests (the setting keys against
//! `ts_api::api::SMART_SETTING_KEYS`, the bands against the Python test vectors). The daemon
//! enforces every limit again, so a drifted mirror can only make the page ask a question too
//! early or too late, never write an unsafe value.
//!
//! Verbs (QML calls `Bus.vm("performance", verb, json)`): `smart_schema`, `smart_set`,
//! `smart_payload`, `changed`, `undo`, `copy_name`, `name_check`, `rules_model`, `rule_check`,
//! `risk_pl`, `risk_cpu_uv`, `risk_gpu_uv`, `confirm_plan`, `uv_cpu_values`, `uv_gpu_values`,
//! `now_line`, `limits_view`, `uv_state`, `caps`. Texts the person reads are chosen and
//! translated in QML; the explanations a risk carries are the daemon's own words, shown in the
//! confirmation exactly as the command line shows them.
#![forbid(unsafe_code)]

use serde_json::{Map, Value, json};

use crate::json::{at, bool_at, f64_at, list_at, str_at};

/// The five power modes, slowest to fastest (the rank order of `ts_decision::smartprofile::Mode`).
const MODES: [&str; 5] = ["eco", "quiet", "balanced", "performance", "turbo"];

/// The mode-valued settings of a Smart auto profile, in display order.
const MODE_KEYS: [&str; 7] = [
    "ac_idle",
    "ac_browsing",
    "ac_busy",
    "battery_idle",
    "battery_browsing",
    "battery_busy",
    "battery_hot",
];

/// The switch of a Smart auto profile.
const TURBO_KEY: &str = "ac_turbo";

/// The numeric settings with their inclusive ranges, in the daemon's `NUM_SETTINGS` order.
const NUMBERS: [(&str, i64, i64); 12] = [
    ("turbo_after_s", 5, 300),
    ("turbo_max_cpu", 60, 95),
    ("turbo_max_gpu", 55, 85),
    ("backoff_hold_s", 10, 600),
    ("idle_after_s", 10, 600),
    ("battery_low_pct", 5, 50),
    ("up_dwell_s", 2, 120),
    ("down_dwell_s", 2, 300),
    ("busy_hold_s", 0, 600),
    ("min_change_s", 2, 600),
    ("turbo_margin_c", 0, 20),
    ("turbo_retry_s", 10, 900),
];

/// A profile name as the library accepts it: letters, digits, `.`, `_`, `-`, at most 40.
const NAME_MAX: usize = 40;
/// A game rule's program name is at most this long (`ts_decision::gamerules`).
const PROGRAM_MAX: usize = 80;
/// At most this many game rules (`ts_decision::gamerules::MAX_RULES`).
const MAX_RULES: usize = 64;

/// Smallest and largest PL1 or PL2 the daemon ever writes, in watts (`ts_domain::power`).
const PL_MIN_W: f64 = 25.0;
const PL_MAX_W: f64 = 120.0;
/// PL1 and PL2 ranges that are "safe": the cooling keeps up (stock is 80 W sustained).
const PL1_SAFE: (f64, f64) = (35.0, 80.0);
const PL2_SAFE: (f64, f64) = (35.0, 100.0);
/// Beyond these the cooling cannot keep up and the CPU throttles hard or overheats.
const PL1_DANGER: f64 = 100.0;
const PL2_DANGER: f64 = 115.0;

/// CPU undervolt lines in millivolts of reduction: past SAFE it may be unstable, past CAUTION
/// (the danger line) freezes and reboots are likely. The two planes have their own lines.
const CPU_SAFE_CORE_MV: u32 = 60;
const CPU_SAFE_CACHE_MV: u32 = 50;
const CPU_DANGER_CORE_MV: u32 = 90;
const CPU_DANGER_CACHE_MV: u32 = 70;

/// GPU undervolt lines (MHz of curve offset) and the lowest clock cap that costs little speed.
const GPU_SAFE_OFFSET_MHZ: i64 = 150;
const GPU_DANGER_OFFSET_MHZ: i64 = 200;
const GPU_LOW_CAP_MHZ: i64 = 1500;
/// Smallest clock cap the driver is asked for; a slider below this means "no cap".
const GPU_CAP_MIN_MHZ: i64 = 600;

/// The word that must be typed to apply a value beyond the danger line.
const DANGER_WORD: &str = "APPLY";

/// The dispatcher of this module (see the module documentation for the verbs).
///
/// # Errors
/// The text for an unknown verb or for arguments of the wrong shape.
pub fn call(verb: &str, args: &Value) -> Result<Value, String> {
    match verb {
        "smart_schema" => Ok(smart_schema()),
        "smart_set" => smart_set(args),
        "smart_payload" => smart_payload(args),
        "changed" => changed(args),
        "undo" => undo(args),
        "copy_name" => Ok(json!(copy_name(args))),
        "name_check" => Ok(json!({ "ok": valid_name(str_at(args, "name").unwrap_or("")) })),
        "rules_model" => Ok(rules_model(args)),
        "rule_check" => Ok(rule_check(args)),
        "risk_pl" => Ok(risk_pl(args)),
        "risk_cpu_uv" => Ok(risk_cpu_uv(args)),
        "risk_gpu_uv" => Ok(risk_gpu_uv(args)),
        "confirm_plan" => Ok(confirm_plan(args)),
        "uv_cpu_values" => Ok(uv_cpu_values(args)),
        "uv_gpu_values" => Ok(uv_gpu_values(args)),
        "now_line" => Ok(now_line(args)),
        "limits_view" => Ok(limits_view(args)),
        "uv_state" => uv_state(args),
        "caps" => Ok(caps(args)),
        _ => Err(format!("unknown performance verb: {verb}")),
    }
}

// ---- the Smart auto profile edit state ---------------------------------------------------------

/// The settings the editor shows: modes, the switch and the numbers with their ranges.
fn smart_schema() -> Value {
    json!({
        "modes": MODES,
        "mode_keys": MODE_KEYS,
        "turbo_key": TURBO_KEY,
        "numbers": NUMBERS.iter().map(|(key, min, max)| json!({"key": key, "min": min, "max": max})).collect::<Vec<_>>(),
    })
}

/// The object under `key` of `args`, or the error naming it.
fn object_arg<'a>(args: &'a Value, key: &str) -> Result<&'a Map<String, Value>, String> {
    args.get(key).and_then(Value::as_object).ok_or_else(|| format!("{key}: an object is needed"))
}

/// Sets one setting of the edited profile. A number is rounded and held inside its range (a
/// slider can only produce such values; the clamp is the guard for everything else); a mode must
/// be one of the five; anything unknown is refused, so a typo in the page cannot add a key.
fn smart_set(args: &Value) -> Result<Value, String> {
    let mut edited = object_arg(args, "edited")?.clone();
    let key = str_at(args, "key").ok_or("key: the setting's name is needed")?;
    let value = args.get("value").ok_or("value: the new value is needed")?;
    let new = if MODE_KEYS.contains(&key) {
        match value.as_str().filter(|m| MODES.contains(m)) {
            Some(mode) => json!(mode),
            None => return Err(format!("{key} must be one of {}", MODES.join(", "))),
        }
    } else if key == TURBO_KEY {
        json!(value.as_bool().ok_or_else(|| format!("{TURBO_KEY} is true or false"))?)
    } else if let Some((_, min, max)) = NUMBERS.iter().find(|(k, ..)| *k == key) {
        let n = value.as_f64().filter(|n| n.is_finite()).ok_or_else(|| format!("{key}: {min}..{max}"))?;
        json!((n.round() as i64).clamp(*min, *max))
    } else {
        return Err(format!("unknown Smart auto setting: {key}"));
    };
    edited.insert(key.to_owned(), new);
    Ok(Value::Object(edited))
}

/// The profile to send for `save` and `use`: exactly the twenty settings, checked against the same
/// ranges the daemon applies (it refuses a bad one with the same words). The description is not
/// part of it, so saving never overwrites a built-in's text with the editor's copy.
fn smart_payload(args: &Value) -> Result<Value, String> {
    let edited = object_arg(args, "edited")?;
    let mut out = Map::new();
    for key in MODE_KEYS {
        match edited.get(key).and_then(Value::as_str).filter(|m| MODES.contains(m)) {
            Some(mode) => out.insert(key.to_owned(), json!(mode)),
            None => return Err(format!("{key} must be one of {}", MODES.join(", "))),
        };
    }
    let turbo = edited.get(TURBO_KEY).and_then(Value::as_bool).ok_or_else(|| format!("{TURBO_KEY} is true or false"))?;
    out.insert(TURBO_KEY.to_owned(), json!(turbo));
    for (key, min, max) in NUMBERS {
        let n = edited.get(key).and_then(Value::as_f64).filter(|n| n.fract() == 0.0 && (min as f64..=max as f64).contains(n));
        match n {
            Some(n) => out.insert(key.to_owned(), json!(n as i64)),
            None => return Err(format!("{key}: {min}..{max}")),
        };
    }
    Ok(Value::Object(out))
}

/// Which members differ between the loaded copy and the edited one: the "Changed" badge and the
/// per-setting undo marker. Only keys present in `edited` count (the description is never edited).
fn changed(args: &Value) -> Result<Value, String> {
    let original = object_arg(args, "original")?;
    let edited = object_arg(args, "edited")?;
    let keys: Vec<&String> = edited.iter().filter(|(k, v)| original.get(*k) != Some(*v)).map(|(k, _)| k).collect();
    let flags: Map<String, Value> = keys.iter().map(|k| ((*k).clone(), json!(true))).collect();
    Ok(json!({ "keys": keys, "count": keys.len(), "flags": flags }))
}

/// Puts one setting (`key`) or all of them (no `key`) back to the loaded copy.
fn undo(args: &Value) -> Result<Value, String> {
    let original = object_arg(args, "original")?;
    match str_at(args, "key") {
        None => Ok(Value::Object(original.clone())),
        Some(key) => {
            let mut edited = object_arg(args, "edited")?.clone();
            let old = original.get(key).ok_or_else(|| format!("{key}: not in the loaded profile"))?;
            edited.insert(key.to_owned(), old.clone());
            Ok(Value::Object(edited))
        }
    }
}

/// A profile name the CLI accepts: letters, digits, `.`, `_`, `-`, one to forty characters.
fn valid_name(name: &str) -> bool {
    (1..=NAME_MAX).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// The name for a copy: `base`, or `base-2`, `base-3`, ... when the library already has it.
fn copy_name(args: &Value) -> String {
    let base = str_at(args, "base").unwrap_or("profile");
    let names: Vec<&str> = list_at(args, "names").map(|l| l.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let mut name = base.to_owned();
    let mut n = 2;
    while names.contains(&name.as_str()) {
        name = format!("{base}-{n}");
        n += 1;
    }
    name
}

// ---- game rules --------------------------------------------------------------------------------

/// The rule list for display, sorted by program. `running` is the mode of the highest-ranked
/// program running now (the daemon answers a mode, not a program), so a row is `running` when it
/// is the only rule with that mode, `maybe` when several share it (one of them runs), and `idle`
/// otherwise: a rule above that mode certainly is not running, and one below may be, but the
/// daemon does not say, so the page does not claim it.
fn rules_model(args: &Value) -> Value {
    let running = str_at(args, "running").filter(|m| MODES.contains(m));
    let mut rules: Vec<(String, String)> = at(args, "rules")
        .and_then(Value::as_object)
        .map(|o| o.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect())
        .unwrap_or_default();
    rules.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    let sharing = running.map_or(0, |m| rules.iter().filter(|(_, mode)| mode == m).count());
    let rows: Vec<Value> = rules
        .iter()
        .map(|(program, mode)| {
            let state = match running {
                Some(m) if m == mode && sharing == 1 => "running",
                Some(m) if m == mode => "maybe",
                _ => "idle",
            };
            json!({ "program": program, "mode": mode, "state": state })
        })
        .collect();
    json!({ "rows": rows, "count": rows.len(), "running": running, "full": rows.len() >= MAX_RULES })
}

/// Checks a rule before it is sent, with the daemon's cleaning (trim, lower case; letters, digits,
/// `. _ + -` and spaces, one to eighty characters). `problem` is a code QML turns into a sentence:
/// `empty`, `chars`, `too_long`, `mode`, `full`; `replaces` says the program already has a rule.
fn rule_check(args: &Value) -> Value {
    let program = str_at(args, "program").unwrap_or("").trim().to_lowercase();
    let mode = str_at(args, "mode").unwrap_or("");
    let rules = at(args, "rules").and_then(Value::as_object);
    let replaces = rules.is_some_and(|r| r.contains_key(&program));
    let count = rules.map_or(0, Map::len);
    let allowed = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'+' | b' ' | b'-');
    let problem = if program.is_empty() {
        Some("empty")
    } else if program.len() > PROGRAM_MAX {
        Some("too_long")
    } else if !program.bytes().all(allowed) {
        Some("chars")
    } else if !MODES.contains(&mode) {
        Some("mode")
    } else if !replaces && count >= MAX_RULES {
        Some("full")
    } else {
        None
    };
    json!({ "ok": problem.is_none(), "program": program, "mode": mode, "problem": problem, "replaces": replaces })
}

// ---- risk bands --------------------------------------------------------------------------------

fn rating(level: &str, why: impl Into<String>) -> Value {
    json!({ "level": level, "why": why.into() })
}

/// Rates a PL1/PL2 pair in watts: the daemon's bands and wording, plus what the page needs to
/// refuse before asking (`problem`: `order` when PL2 is below PL1, `range` outside 25..120 W,
/// `number` when not a number).
fn risk_pl(args: &Value) -> Value {
    let (Some(pl1), Some(pl2)) = (f64_at(args, "pl1"), f64_at(args, "pl2")) else {
        let mut r = rating("danger", "The limits are not numbers.");
        r["problem"] = json!("number");
        return r;
    };
    let mut r = if pl2 < pl1 {
        rating("danger", "PL2 (burst) must not be below PL1 (sustained).")
    } else if pl1 > PL1_DANGER || pl2 > PL2_DANGER {
        rating(
            "danger",
            format!(
                "Beyond the danger line ({PL1_DANGER} W sustained / {PL2_DANGER} W burst): the cooling cannot keep up and the CPU will throttle hard or overheat."
            ),
        )
    } else if pl1 > PL1_SAFE.1 || pl2 > PL2_SAFE.1 {
        rating(
            "caution",
            format!(
                "Above stock ({} W sustained / {} W burst): hotter and louder, and it may throttle.",
                PL1_SAFE.1, PL2_SAFE.1
            ),
        )
    } else if pl1 < PL1_SAFE.0 || pl2 < PL2_SAFE.0 {
        rating("caution", "Very low limits make the laptop feel slow.")
    } else {
        rating("safe", "Inside the safe range.")
    };
    let problem = if pl2 < pl1 {
        Some("order")
    } else if !(PL_MIN_W..=PL_MAX_W).contains(&pl1) || !(PL_MIN_W..=PL_MAX_W).contains(&pl2) {
        Some("range")
    } else {
        None
    };
    r["problem"] = json!(problem);
    r
}

/// Rates CPU undervolt offsets in millivolts (negative or positive; only the size counts).
fn risk_cpu_uv(args: &Value) -> Value {
    let core = f64_at(args, "core").unwrap_or(0.0).abs();
    let cache = f64_at(args, "cache").unwrap_or(0.0).abs();
    if core > f64::from(CPU_DANGER_CORE_MV) || cache > f64::from(CPU_DANGER_CACHE_MV) {
        rating(
            "danger",
            format!(
                "Beyond the danger line (core {CPU_DANGER_CORE_MV} mV / cache {CPU_DANGER_CACHE_MV} mV). Freezes and reboots are likely; unsaved work can be lost."
            ),
        )
    } else if core > f64::from(CPU_SAFE_CORE_MV) || cache > f64::from(CPU_SAFE_CACHE_MV) {
        rating(
            "caution",
            format!(
                "Past the safety line (core {CPU_SAFE_CORE_MV} mV / cache {CPU_SAFE_CACHE_MV} mV). It may be unstable: test under load before trusting it."
            ),
        )
    } else if core != 0.0 || cache != 0.0 {
        rating("safe", "Inside the safe range.")
    } else {
        rating("safe", "Stock voltage.")
    }
}

/// Rates a GPU curve offset (MHz) and clock cap (MHz, 0 = none).
fn risk_gpu_uv(args: &Value) -> Value {
    let offset = f64_at(args, "offset").unwrap_or(0.0).round() as i64;
    let cap = f64_at(args, "cap").unwrap_or(0.0).round() as i64;
    if offset > GPU_DANGER_OFFSET_MHZ {
        rating(
            "danger",
            format!("Beyond the danger line (+{GPU_DANGER_OFFSET_MHZ} MHz). Games and the driver can crash; the screen can glitch."),
        )
    } else if offset > GPU_SAFE_OFFSET_MHZ {
        rating(
            "caution",
            format!("Past the safety line (+{GPU_SAFE_OFFSET_MHZ} MHz). It may crash in some games: test under load."),
        )
    } else if offset > 0 && cap == 0 {
        rating(
            "caution",
            "A positive offset without a clock cap can push the clock above stock: set a cap (2400 MHz is typical) or keep the offset small.",
        )
    } else if offset < -100 {
        rating(
            "caution",
            "A large negative offset only slows the GPU; it does not save voltage the way a positive offset with a cap does.",
        )
    } else if cap != 0 && cap < GPU_LOW_CAP_MHZ {
        rating("caution", "A cap this low will cost a lot of speed in games.")
    } else if offset != 0 || cap != 0 {
        rating("safe", "Inside the safe range.")
    } else {
        rating("safe", "Stock.")
    }
}

/// What the confirmation for a risk level looks like. A value beyond the danger line is sent with
/// `force` and asks for the word APPLY to be typed; a caution is a plain red-button question; a
/// safe value asks only when `always` (every undervolt is asked about, a safe power limit is not).
fn confirm_plan(args: &Value) -> Value {
    let level = str_at(args, "level").unwrap_or("danger");
    let always = bool_at(args, "always").unwrap_or(false);
    match level {
        "safe" => json!({ "ask": always, "danger": false, "force": false, "type_word": "" }),
        "caution" => json!({ "ask": true, "danger": true, "force": false, "type_word": "" }),
        _ => json!({ "ask": true, "danger": true, "force": true, "type_word": DANGER_WORD }),
    }
}

// ---- undervolt values --------------------------------------------------------------------------

/// The page shows a CPU undervolt as a reduction (0 to 120 mV); the command takes negative offsets.
fn uv_cpu_values(args: &Value) -> Value {
    let cut = |key: &str| -> i64 { -(f64_at(args, key).unwrap_or(0.0).abs().round() as i64) };
    json!({ "core": cut("core_cut"), "cache": cut("cache_cut") })
}

/// The GPU clock-cap slider has no "none" position, so everything below the driver's smallest
/// cap (600 MHz) means no cap.
fn uv_gpu_values(args: &Value) -> Value {
    let offset = f64_at(args, "offset").unwrap_or(0.0).round() as i64;
    let slider = f64_at(args, "cap_slider").unwrap_or(0.0).round() as i64;
    json!({ "offset": offset, "cap": if slider >= GPU_CAP_MIN_MHZ { slider } else { 0 } })
}

/// A readable view of `cpu undervolt get` or `gpu undervolt get`. `readable` is false when the
/// daemon could not read the voltage (the MSR or the helper is not usable) and then `error` holds
/// its sentence.
fn uv_state(args: &Value) -> Result<Value, String> {
    let kind = str_at(args, "kind").ok_or("kind: cpu or gpu")?;
    let state = args.get("state").unwrap_or(&Value::Null);
    let name = |path: &str| str_at(state, path).map(str::to_owned);
    let error = str_at(state, "error").map(str::to_owned);
    match kind {
        "cpu" => Ok(json!({
            "readable": f64_at(state, "core_mv").is_some() && f64_at(state, "cache_mv").is_some(),
            "core_mv": f64_at(state, "core_mv"),
            "cache_mv": f64_at(state, "cache_mv"),
            "applied": name("applied.name"),
            "boot": name("boot.name"),
            "pending": bool_at(state, "pending").unwrap_or(false),
            "error": error,
        })),
        "gpu" => Ok(json!({
            "readable": f64_at(state, "offset_mhz").is_some(),
            "offset_mhz": f64_at(state, "offset_mhz"),
            "range_mhz": [f64_at(state, "range_mhz.0"), f64_at(state, "range_mhz.1")],
            "applied": name("applied.name"),
            "applied_cap": f64_at(state, "applied.cap"),
            "boot": name("boot.name"),
            "error": error,
        })),
        _ => Err(format!("kind: cpu or gpu (got '{kind}')")),
    }
}

// ---- the live "now" line, limits and capabilities ----------------------------------------------

/// The daemon's part of the live frame as the "now" line wants it: which mode control is in
/// force (`off`, `native`, `smart`; older daemons only had `auto_profile`), what the machine is
/// doing and why the mode is what it is, and how long a hand-picked mode is still held.
fn now_line(args: &Value) -> Value {
    let daemon = args.get("daemon").unwrap_or(&Value::Null);
    if !daemon.is_object() {
        return json!({ "available": false, "control": "off", "hold_left_s": 0, "hold_active": false });
    }
    let control = str_at(daemon, "settings.auto_mode")
        .filter(|c| ["off", "native", "smart"].contains(c))
        .unwrap_or(if bool_at(daemon, "settings.auto_profile").unwrap_or(false) { "smart" } else { "off" });
    let hold = f64_at(daemon, "hold_left_s").unwrap_or(0.0).max(0.0);
    json!({
        "available": true,
        "control": control,
        "activity": str_at(daemon, "live.activity"),
        "reason": str_at(daemon, "live.mode_reason").filter(|r| !r.is_empty()),
        "profile": str_at(daemon, "live.smart_profile"),
        "hold_left_s": hold,
        "hold_active": hold > 0.0 && control != "off",
    })
}

/// A watts value out of nvidia-smi's text (`"80.00"`, `"80.00 W"`; `[N/A]` is nothing).
fn parse_watts(v: Option<&Value>) -> Option<f64> {
    let text = v?.as_str()?.trim().trim_end_matches('W').trim();
    text.parse::<f64>().ok().filter(|w| w.is_finite())
}

/// `cpu info` made plain: the two CPU limits, whether the firmware locked them (`locked` is null
/// when the register cannot be read, then `lock_note` says why), what the user set and what the
/// automatic mode uses, and the GPU limits as numbers (a driver may print `[N/A]`).
fn limits_view(args: &Value) -> Value {
    let info = args.get("info").unwrap_or(&Value::Null);
    let lock = at(info, "msr_lock_bit");
    let gpu = at(info, "nvidia_power_limit_w");
    let gpu_value = |key: &str| parse_watts(gpu.and_then(|g| g.get(key)));
    let gpu_known = ["current", "default", "min", "max"].iter().any(|k| gpu_value(k).is_some());
    let user = at(info, "user").map(|u| json!({ "pl1": f64_at(u, "pl1"), "pl2": f64_at(u, "pl2") }));
    let auto = at(info, "auto").map(|a| json!({ "normal_w": f64_at(a, "normal_w"), "gaming_w": f64_at(a, "gaming_w") }));
    json!({
        "readable": f64_at(info, "pl1_w").is_some() && f64_at(info, "pl2_w").is_some(),
        "pl1": f64_at(info, "pl1_w"),
        "pl2": f64_at(info, "pl2_w"),
        "rapl_enabled": str_at(info, "rapl_enabled"),
        "locked": lock.and_then(Value::as_bool),
        "lock_note": lock.and_then(Value::as_str),
        "user": user,
        "auto": auto,
        "gpu_known": gpu_known,
        "gpu": { "current": gpu_value("current"), "default": gpu_value("default"), "min": gpu_value("min"), "max": gpu_value("max") },
    })
}

/// `capabilities get` as `{id: {usable, why}}`. An id the answer does not mention is simply absent
/// (QML treats absent as usable: the command is the authority), and `why` is null for a feature
/// that is supported.
fn caps(args: &Value) -> Value {
    let items = args.get("capabilities").map(|c| c.get("items").unwrap_or(c));
    let mut out = Map::new();
    for item in items.and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default() {
        let (Some(id), Some(supported)) = (str_at(item, "id"), bool_at(item, "supported")) else { continue };
        let why = if supported { None } else { str_at(item, "why") };
        out.insert(id.to_owned(), json!({ "usable": supported, "why": why }));
    }
    Value::Object(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn run(verb: &str, args: Value) -> Value {
        call(verb, &args).unwrap()
    }

    fn truesense_profile() -> Value {
        json!({
            "ac_idle": "balanced", "ac_browsing": "balanced", "ac_busy": "performance", "ac_turbo": true,
            "turbo_after_s": 30, "turbo_max_cpu": 85, "turbo_max_gpu": 75, "backoff_hold_s": 90,
            "idle_after_s": 60, "battery_idle": "eco", "battery_browsing": "eco", "battery_busy": "balanced",
            "battery_hot": "quiet", "battery_low_pct": 20, "up_dwell_s": 5, "down_dwell_s": 60,
            "busy_hold_s": 60, "min_change_s": 45, "turbo_margin_c": 6, "turbo_retry_s": 180,
            "description": "the default",
        })
    }

    #[test]
    fn the_schema_covers_exactly_the_settings_the_api_names() {
        let schema = run("smart_schema", json!({}));
        let mut mine: Vec<String> = schema["mode_keys"].as_array().unwrap().iter().map(|k| k.as_str().unwrap().to_owned()).collect();
        mine.push(schema["turbo_key"].as_str().unwrap().to_owned());
        mine.extend(schema["numbers"].as_array().unwrap().iter().map(|n| n["key"].as_str().unwrap().to_owned()));
        mine.sort();
        let mut theirs: Vec<String> = ts_api::api::SMART_SETTING_KEYS.iter().map(|k| (*k).to_owned()).collect();
        theirs.sort();
        assert_eq!(mine, theirs, "a setting was added or renamed in ts-decision: mirror it here");
    }

    #[test]
    fn the_ranges_are_the_daemons() {
        // ts_decision::smartprofile::NUM_SETTINGS, written out once more on purpose.
        let want = [
            ("turbo_after_s", 5, 300), ("turbo_max_cpu", 60, 95), ("turbo_max_gpu", 55, 85), ("backoff_hold_s", 10, 600),
            ("idle_after_s", 10, 600), ("battery_low_pct", 5, 50), ("up_dwell_s", 2, 120), ("down_dwell_s", 2, 300),
            ("busy_hold_s", 0, 600), ("min_change_s", 2, 600), ("turbo_margin_c", 0, 20), ("turbo_retry_s", 10, 900),
        ];
        assert_eq!(NUMBERS, want);
    }

    #[test]
    fn setting_a_number_rounds_and_holds_it_inside_its_range() {
        let base = truesense_profile();
        let set = |key: &str, value: Value| run("smart_set", json!({"edited": base, "key": key, "value": value}));
        assert_eq!(set("turbo_after_s", json!(44.6))["turbo_after_s"], json!(45));
        assert_eq!(set("turbo_after_s", json!(1))["turbo_after_s"], json!(5));
        assert_eq!(set("turbo_after_s", json!(9999))["turbo_after_s"], json!(300));
        assert_eq!(set("busy_hold_s", json!(-4))["busy_hold_s"], json!(0));
        // The rest of the profile, description included, is untouched.
        assert_eq!(set("turbo_margin_c", json!(7))["description"], json!("the default"));
        assert_eq!(set("turbo_margin_c", json!(7))["ac_idle"], json!("balanced"));
    }

    #[test]
    fn setting_modes_and_the_switch_is_checked() {
        let base = truesense_profile();
        let ok = run("smart_set", json!({"edited": base, "key": "battery_hot", "value": "eco"}));
        assert_eq!(ok["battery_hot"], json!("eco"));
        assert_eq!(run("smart_set", json!({"edited": base, "key": "ac_turbo", "value": false}))["ac_turbo"], json!(false));
        for (key, value) in [
            ("battery_hot", json!("Turbo")),
            ("battery_hot", json!("ludicrous")),
            ("battery_hot", json!(3)),
            ("ac_turbo", json!("yes")),
            ("turbo_after_s", json!("30")),
            ("turbo_after_s", json!(null)),
            ("nonsense", json!(1)),
        ] {
            assert!(call("smart_set", &json!({"edited": base, "key": key, "value": value})).is_err(), "{key} {value}");
        }
        assert!(call("smart_set", &json!({"key": "ac_idle", "value": "eco"})).is_err(), "no edited object");
        assert!(call("smart_set", &json!({"edited": base, "value": "eco"})).is_err(), "no key");
        assert!(call("smart_set", &json!({"edited": base, "key": "ac_idle"})).is_err(), "no value");
    }

    #[test]
    fn changed_lists_what_differs_and_undo_puts_it_back() {
        let original = truesense_profile();
        let mut edited = run("smart_set", json!({"edited": original, "key": "ac_idle", "value": "quiet"}));
        edited = run("smart_set", json!({"edited": edited, "key": "turbo_after_s", "value": 60}));
        let c = run("changed", json!({"original": original, "edited": edited}));
        assert_eq!(c["count"], json!(2));
        let mut keys: Vec<&str> = c["keys"].as_array().unwrap().iter().map(|k| k.as_str().unwrap()).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["ac_idle", "turbo_after_s"]);
        assert_eq!(c["flags"]["ac_idle"], json!(true));
        assert!(c["flags"].get("ac_busy").is_none());

        let one = run("undo", json!({"original": original, "edited": edited, "key": "ac_idle"}));
        assert_eq!(one["ac_idle"], json!("balanced"));
        assert_eq!(one["turbo_after_s"], json!(60), "the other change stays");
        let all = run("undo", json!({"original": original, "edited": edited}));
        assert_eq!(all, original);
        assert_eq!(run("changed", json!({"original": original, "edited": all}))["count"], json!(0));
        // Setting a value back by hand is not a change.
        let back = run("smart_set", json!({"edited": edited, "key": "turbo_after_s", "value": 30}));
        assert_eq!(run("changed", json!({"original": original, "edited": back}))["count"], json!(1));
        assert!(call("undo", &json!({"original": original, "edited": edited, "key": "nope"})).is_err());
    }

    #[test]
    fn the_payload_has_the_twenty_settings_and_no_description() {
        let p = run("smart_payload", json!({"edited": truesense_profile()}));
        assert_eq!(p.as_object().unwrap().len(), 20);
        assert!(p.get("description").is_none());
        assert_eq!(p["turbo_max_cpu"], json!(85));
        assert_eq!(p["ac_busy"], json!("performance"));
    }

    #[test]
    fn the_payload_refuses_what_the_daemon_would_refuse() {
        let mut bad = truesense_profile();
        bad["turbo_max_cpu"] = json!(96);
        assert_eq!(call("smart_payload", &json!({"edited": bad})).unwrap_err(), "turbo_max_cpu: 60..95");
        let mut bad = truesense_profile();
        bad["turbo_after_s"] = json!(30.5);
        assert_eq!(call("smart_payload", &json!({"edited": bad})).unwrap_err(), "turbo_after_s: 5..300");
        let mut bad = truesense_profile();
        bad["ac_idle"] = json!("fast");
        assert!(call("smart_payload", &json!({"edited": bad})).unwrap_err().starts_with("ac_idle must be one of eco, quiet"));
        let mut bad = truesense_profile();
        bad.as_object_mut().unwrap().remove("ac_turbo");
        assert_eq!(call("smart_payload", &json!({"edited": bad})).unwrap_err(), "ac_turbo is true or false");
        let mut bad = truesense_profile();
        bad.as_object_mut().unwrap().remove("idle_after_s");
        assert!(call("smart_payload", &json!({"edited": bad})).is_err());
        assert!(call("smart_payload", &json!({})).is_err());
        assert!(call("smart_payload", &json!({"edited": [1]})).is_err());
    }

    #[test]
    fn names_are_checked_like_the_library_does() {
        for ok in ["a", "my-profile", "x.y_z", "A1", &"n".repeat(40)] {
            assert_eq!(run("name_check", json!({"name": ok})), json!({"ok": true}), "{ok}");
        }
        for bad in ["", "has space", "slash/x", "ünï", &"n".repeat(41), "a;b", ".. /"] {
            assert_eq!(run("name_check", json!({"name": bad})), json!({"ok": false}), "{bad}");
        }
        assert_eq!(run("name_check", json!({})), json!({"ok": false}));
    }

    #[test]
    fn a_copy_gets_the_first_free_name() {
        let names = json!(["native", "gaming", "gaming-2"]);
        assert_eq!(run("copy_name", json!({"base": "quiet", "names": names})), json!("quiet"));
        assert_eq!(run("copy_name", json!({"base": "gaming", "names": names})), json!("gaming-3"));
        assert_eq!(run("copy_name", json!({"base": "native-copy", "names": []})), json!("native-copy"));
        assert_eq!(run("copy_name", json!({})), json!("profile"));
    }

    #[test]
    fn rules_are_sorted_and_the_running_marker_is_only_claimed_when_certain() {
        let rules = json!({"Zelda.exe": "turbo", "eldenring.exe": "turbo", "hl2_linux": "performance", "blender": "performance", "slow": "eco"});
        let v = run("rules_model", json!({"rules": rules, "running": "turbo"}));
        let rows = v["rows"].as_array().unwrap();
        let order: Vec<&str> = rows.iter().map(|r| r["program"].as_str().unwrap()).collect();
        assert_eq!(order, ["blender", "eldenring.exe", "hl2_linux", "slow", "Zelda.exe"], "case-insensitive order");
        let state = |p: &str| rows.iter().find(|r| r["program"] == p).unwrap()["state"].as_str().unwrap().to_owned();
        assert_eq!(state("eldenring.exe"), "maybe", "two turbo rules: one of them runs");
        assert_eq!(state("Zelda.exe"), "maybe");
        assert_eq!(state("hl2_linux"), "idle", "a lower rule is not claimed");
        assert_eq!(v["running"], json!("turbo"));
        assert_eq!(v["count"], json!(5));

        let single = run("rules_model", json!({"rules": {"a": "performance", "b": "turbo"}, "running": "performance"}));
        assert_eq!(single["rows"][0]["state"], json!("running"));
        assert_eq!(single["rows"][1]["state"], json!("idle"));
    }

    #[test]
    fn no_running_mode_means_every_row_is_idle_and_junk_is_ignored() {
        let none = run("rules_model", json!({"rules": {"a": "turbo"}, "running": null}));
        assert_eq!(none["rows"][0]["state"], json!("idle"));
        assert_eq!(none["running"], json!(null));
        let junk = run("rules_model", json!({"rules": {"a": 3, "b": "turbo"}, "running": "warp"}));
        assert_eq!(junk["count"], json!(1), "a rule whose mode is not text is dropped");
        assert_eq!(junk["running"], json!(null));
        assert_eq!(run("rules_model", json!({}))["count"], json!(0));
        assert_eq!(run("rules_model", json!({"rules": [1]}))["count"], json!(0));
    }

    #[test]
    fn a_rule_is_cleaned_and_checked_like_the_daemon_does() {
        let check = |program: &str, mode: &str| run("rule_check", json!({"program": program, "mode": mode, "rules": {"old.exe": "turbo"}}));
        let ok = check("  EldenRing.EXE ", "turbo");
        assert_eq!(ok["ok"], json!(true));
        assert_eq!(ok["program"], json!("eldenring.exe"));
        assert_eq!(check("old.exe", "eco")["replaces"], json!(true));
        assert_eq!(check("", "turbo")["problem"], json!("empty"));
        assert_eq!(check("   ", "turbo")["problem"], json!("empty"));
        assert_eq!(check("/usr/bin/game", "turbo")["problem"], json!("chars"));
        assert_eq!(check("a\\b", "turbo")["problem"], json!("chars"));
        assert_eq!(check(&"g".repeat(81), "turbo")["problem"], json!("too_long"));
        assert_eq!(check(&"g".repeat(80), "turbo")["ok"], json!(true));
        assert_eq!(check("game", "Turbo")["problem"], json!("mode"));
        assert_eq!(check("game", "")["problem"], json!("mode"));
        assert_eq!(check("wine64 preloader+x", "quiet")["ok"], json!(true), "space, plus and dash are allowed");
        let many: Map<String, Value> = (0..MAX_RULES).map(|i| (format!("g{i}"), json!("turbo"))).collect();
        let full = run("rule_check", json!({"program": "new", "mode": "turbo", "rules": many}));
        assert_eq!(full["problem"], json!("full"));
        let replace = run("rule_check", json!({"program": "g1", "mode": "eco", "rules": many}));
        assert_eq!(replace["ok"], json!(true), "replacing a rule is allowed when the list is full");
    }

    fn level_of(v: &Value) -> &str {
        v["level"].as_str().unwrap()
    }

    #[test]
    fn power_limit_bands_match_the_python_vectors() {
        // test_core: cpu.pl_risk(65, 80) safe, (90, 100) caution, (110, 115) danger, plus the edges
        // pinned by ts_domain::power's own table.
        for (pl1, pl2, want) in [
            (65.0, 80.0, "safe"), (90.0, 100.0, "caution"), (110.0, 115.0, "danger"), (80.0, 100.0, "safe"),
            (80.5, 100.0, "caution"), (80.0, 100.5, "caution"), (100.0, 115.0, "caution"), (100.5, 115.0, "danger"),
            (100.0, 115.5, "danger"), (35.0, 35.0, "safe"), (34.0, 80.0, "caution"), (60.0, 34.0, "danger"),
        ] {
            let v = run("risk_pl", json!({"pl1": pl1, "pl2": pl2}));
            assert_eq!(level_of(&v), want, "risk_pl({pl1}, {pl2}) = {v}");
        }
    }

    #[test]
    fn power_limit_wording_is_the_daemons() {
        assert_eq!(run("risk_pl", json!({"pl1": 65, "pl2": 80}))["why"], json!("Inside the safe range."));
        assert_eq!(
            run("risk_pl", json!({"pl1": 90, "pl2": 100}))["why"],
            json!("Above stock (80 W sustained / 100 W burst): hotter and louder, and it may throttle.")
        );
        assert_eq!(run("risk_pl", json!({"pl1": 30, "pl2": 80}))["why"], json!("Very low limits make the laptop feel slow."));
        assert_eq!(
            run("risk_pl", json!({"pl1": 110, "pl2": 115}))["why"],
            json!("Beyond the danger line (100 W sustained / 115 W burst): the cooling cannot keep up and the CPU will throttle hard or overheat.")
        );
        assert_eq!(run("risk_pl", json!({"pl1": 70, "pl2": 60}))["why"], json!("PL2 (burst) must not be below PL1 (sustained)."));
    }

    #[test]
    fn power_limit_problems_stop_the_request_before_the_question() {
        assert_eq!(run("risk_pl", json!({"pl1": 70, "pl2": 60}))["problem"], json!("order"));
        assert_eq!(run("risk_pl", json!({"pl1": 20, "pl2": 80}))["problem"], json!("range"));
        assert_eq!(run("risk_pl", json!({"pl1": 80, "pl2": 125}))["problem"], json!("range"));
        assert_eq!(run("risk_pl", json!({"pl1": 25, "pl2": 120}))["problem"], json!(null));
        for bad in [json!({}), json!({"pl1": null, "pl2": 80}), json!({"pl1": "80", "pl2": 80})] {
            let v = run("risk_pl", bad);
            assert_eq!(level_of(&v), "danger");
            assert_eq!(v["problem"], json!("number"));
        }
    }

    #[test]
    fn cpu_undervolt_lines_are_60_50_and_90_70() {
        for (core, cache, want) in [
            (0, 0, "safe"), (-60, -50, "safe"), (-61, 0, "caution"), (0, -51, "caution"), (-90, -70, "caution"),
            (-91, 0, "danger"), (0, -71, "danger"), (-120, -120, "danger"), (60, 50, "safe"), (91, 0, "danger"),
        ] {
            let v = run("risk_cpu_uv", json!({"core": core, "cache": cache}));
            assert_eq!(level_of(&v), want, "core {core} cache {cache}");
        }
        assert_eq!(run("risk_cpu_uv", json!({"core": 0, "cache": 0}))["why"], json!("Stock voltage."));
        assert_eq!(run("risk_cpu_uv", json!({"core": -20, "cache": 0}))["why"], json!("Inside the safe range."));
        assert!(run("risk_cpu_uv", json!({"core": -100, "cache": 0}))["why"].as_str().unwrap().starts_with("Beyond the danger line (core 90 mV / cache 70 mV)."));
        assert!(run("risk_cpu_uv", json!({"core": -70, "cache": 0}))["why"].as_str().unwrap().starts_with("Past the safety line (core 60 mV / cache 50 mV)."));
    }

    #[test]
    fn gpu_undervolt_bands_follow_the_driver_rules() {
        for (offset, cap, want) in [
            (0, 0, "safe"), (100, 2400, "safe"), (150, 2400, "safe"), (151, 2400, "caution"), (200, 2400, "caution"),
            (201, 2400, "danger"), (50, 0, "caution"), (-100, 0, "safe"), (-101, 0, "caution"), (0, 1400, "caution"),
            (0, 1500, "safe"), (-50, 1499, "caution"), (0, 3300, "safe"),
        ] {
            let v = run("risk_gpu_uv", json!({"offset": offset, "cap": cap}));
            assert_eq!(level_of(&v), want, "offset {offset} cap {cap} = {v}");
        }
        assert_eq!(run("risk_gpu_uv", json!({"offset": 0, "cap": 0}))["why"], json!("Stock."));
        assert_eq!(run("risk_gpu_uv", json!({"offset": 10, "cap": 2400}))["why"], json!("Inside the safe range."));
    }

    #[test]
    fn the_confirmation_gets_stricter_with_the_risk() {
        let plan = |level: &str, always: bool| run("confirm_plan", json!({"level": level, "always": always}));
        assert_eq!(plan("safe", false), json!({"ask": false, "danger": false, "force": false, "type_word": ""}));
        assert_eq!(plan("safe", true), json!({"ask": true, "danger": false, "force": false, "type_word": ""}));
        assert_eq!(plan("caution", false), json!({"ask": true, "danger": true, "force": false, "type_word": ""}));
        assert_eq!(plan("danger", false), json!({"ask": true, "danger": true, "force": true, "type_word": "APPLY"}));
        assert_eq!(plan("danger", true)["type_word"], json!("APPLY"));
        // An unknown level is treated as the strictest.
        assert_eq!(plan("???", false)["force"], json!(true));
        assert_eq!(run("confirm_plan", json!({}))["type_word"], json!("APPLY"));
    }

    #[test]
    fn undervolt_sliders_become_the_values_the_command_takes() {
        assert_eq!(run("uv_cpu_values", json!({"core_cut": 50, "cache_cut": 35})), json!({"core": -50, "cache": -35}));
        assert_eq!(run("uv_cpu_values", json!({"core_cut": 0, "cache_cut": 0})), json!({"core": 0, "cache": 0}));
        assert_eq!(run("uv_cpu_values", json!({"core_cut": -50})), json!({"core": -50, "cache": 0}), "a sign cannot flip it");
        assert_eq!(run("uv_gpu_values", json!({"offset": 120, "cap_slider": 2400})), json!({"offset": 120, "cap": 2400}));
        assert_eq!(run("uv_gpu_values", json!({"offset": -50, "cap_slider": 550})), json!({"offset": -50, "cap": 0}));
        assert_eq!(run("uv_gpu_values", json!({"offset": 0, "cap_slider": 600})), json!({"offset": 0, "cap": 600}));
        assert_eq!(run("uv_gpu_values", json!({})), json!({"offset": 0, "cap": 0}));
    }

    #[test]
    fn the_cpu_undervolt_state_is_readable_or_says_why_not() {
        let ok = run("uv_state", json!({"kind": "cpu", "state": {
            "core_mv": -50, "cache_mv": -40, "applied": {"name": "mild", "core": -50, "cache": -40},
            "boot": {"name": "mild", "core": -50, "cache": -40}, "pending": true}}));
        assert_eq!(ok["readable"], json!(true));
        assert_eq!(ok["core_mv"], json!(-50.0));
        assert_eq!(ok["applied"], json!("mild"));
        assert_eq!(ok["boot"], json!("mild"));
        assert_eq!(ok["pending"], json!(true));
        let bad = run("uv_state", json!({"kind": "cpu", "state": {"error": "msr module not loaded", "applied": null, "boot": null, "pending": false}}));
        assert_eq!(bad["readable"], json!(false));
        assert_eq!(bad["error"], json!("msr module not loaded"));
        assert_eq!(bad["applied"], json!(null));
        assert_eq!(run("uv_state", json!({"kind": "cpu"}))["readable"], json!(false));
    }

    #[test]
    fn the_gpu_undervolt_state_carries_the_driver_range() {
        let ok = run("uv_state", json!({"kind": "gpu", "state": {
            "offset_mhz": 100, "range_mhz": [-300, 250], "applied": {"name": "m", "offset": 100, "cap": 2400}, "boot": null}}));
        assert_eq!(ok["readable"], json!(true));
        assert_eq!(ok["range_mhz"], json!([-300.0, 250.0]));
        assert_eq!(ok["applied_cap"], json!(2400.0));
        assert_eq!(ok["boot"], json!(null));
        let bad = run("uv_state", json!({"kind": "gpu", "state": {"error": "helper missing"}}));
        assert_eq!(bad["readable"], json!(false));
        assert_eq!(bad["range_mhz"], json!([null, null]));
        assert!(call("uv_state", &json!({"kind": "fpga", "state": {}})).is_err());
        assert!(call("uv_state", &json!({"state": {}})).is_err());
    }

    #[test]
    fn the_now_line_reads_the_daemons_live_part() {
        let daemon = json!({
            "settings": {"auto_mode": "smart", "auto_profile": true, "smart_fan": true},
            "live": {"activity": "gaming", "mode_reason": "gaming for 40 s: turbo", "smart_profile": "gaming"},
            "hold_left_s": 125.0,
        });
        let v = run("now_line", json!({"daemon": daemon}));
        assert_eq!(v["available"], json!(true));
        assert_eq!(v["control"], json!("smart"));
        assert_eq!(v["activity"], json!("gaming"));
        assert_eq!(v["reason"], json!("gaming for 40 s: turbo"));
        assert_eq!(v["profile"], json!("gaming"));
        assert_eq!(v["hold_active"], json!(true));
        assert_eq!(v["hold_left_s"], json!(125.0));
    }

    #[test]
    fn the_now_line_falls_back_for_old_daemons_and_a_missing_one() {
        let old = run("now_line", json!({"daemon": {"settings": {"auto_profile": true}, "live": {"mode_reason": ""}, "hold_left_s": 0}}));
        assert_eq!(old["control"], json!("smart"));
        assert_eq!(old["reason"], json!(null), "an empty reason is no reason");
        assert_eq!(old["hold_active"], json!(false));
        let off = run("now_line", json!({"daemon": {"settings": {"auto_mode": "off"}, "live": {}, "hold_left_s": 90}}));
        assert_eq!(off["control"], json!("off"));
        assert_eq!(off["hold_active"], json!(false), "no hold is shown while the control is off");
        let junk = run("now_line", json!({"daemon": {"settings": {"auto_mode": "turbo"}, "hold_left_s": -5}}));
        assert_eq!(junk["control"], json!("off"));
        assert_eq!(junk["hold_left_s"], json!(0.0));
        for gone in [json!({}), json!({"daemon": null}), json!({"daemon": "ERROR: the daemon is not running"})] {
            let v = run("now_line", gone);
            assert_eq!(v["available"], json!(false));
            assert_eq!(v["hold_active"], json!(false));
        }
    }

    #[test]
    fn the_limits_view_reads_watts_the_lock_and_the_gpu_text() {
        let v = run("limits_view", json!({"info": {
            "pl1_w": 65.0, "pl2_w": 80.0, "rapl_enabled": "1", "msr_lock_bit": false,
            "nvidia_power_limit_w": {"current": "80.00", "default": "80.00 W", "min": "[N/A]", "max": "100.00"},
            "user": {"pl1": 65.0, "pl2": 80.0}, "auto": {"normal_w": 65, "gaming_w": 80}}}));
        assert_eq!(v["readable"], json!(true));
        assert_eq!(v["pl1"], json!(65.0));
        assert_eq!(v["locked"], json!(false));
        assert_eq!(v["lock_note"], json!(null));
        assert_eq!(v["user"]["pl2"], json!(80.0));
        assert_eq!(v["auto"]["gaming_w"], json!(80.0));
        assert_eq!(v["gpu_known"], json!(true));
        assert_eq!(v["gpu"]["current"], json!(80.0));
        assert_eq!(v["gpu"]["default"], json!(80.0));
        assert_eq!(v["gpu"]["min"], json!(null), "[N/A] is no number");
        assert_eq!(v["gpu"]["max"], json!(100.0));
    }

    #[test]
    fn an_unreadable_lock_is_unknown_and_automatic_has_no_user_limits() {
        let v = run("limits_view", json!({"info": {
            "pl1_w": 80.0, "pl2_w": 80.0, "msr_lock_bit": "needs root", "nvidia_power_limit_w": null,
            "user": null, "auto": {"normal_w": 65, "gaming_w": 80}}}));
        assert_eq!(v["locked"], json!(null));
        assert_eq!(v["lock_note"], json!("needs root"));
        assert_eq!(v["user"], json!(null));
        assert_eq!(v["gpu_known"], json!(false));
        let locked = run("limits_view", json!({"info": {"pl1_w": 80.0, "pl2_w": 80.0, "msr_lock_bit": true}}));
        assert_eq!(locked["locked"], json!(true));
        let nothing = run("limits_view", json!({}));
        assert_eq!(nothing["readable"], json!(false));
        assert_eq!(nothing["pl1"], json!(null));
        let half = run("limits_view", json!({"info": {"pl1_w": 80.0, "pl2_w": null}}));
        assert_eq!(half["readable"], json!(false));
    }

    #[test]
    fn capabilities_are_keyed_by_id_with_the_reason_only_when_unsupported() {
        let answer = json!({"model": {}, "simulated": false, "items": [
            {"id": "cpu_undervolt", "supported": false, "why": "The msr kernel module is not loaded (sudo modprobe msr)."},
            {"id": "gpu_power_limit", "supported": false, "why": "The GPU power limit is fixed by this laptop's firmware."},
            {"id": "power_limits", "supported": true, "why": null},
            {"id": 5, "supported": true}, {"id": "x"}, "junk"]});
        let v = run("caps", json!({"capabilities": answer}));
        assert_eq!(v["cpu_undervolt"], json!({"usable": false, "why": "The msr kernel module is not loaded (sudo modprobe msr)."}));
        assert_eq!(v["gpu_power_limit"]["usable"], json!(false));
        assert_eq!(v["power_limits"], json!({"usable": true, "why": null}));
        assert!(v.get("x").is_none() && v.get("gpu_undervolt").is_none());
        assert_eq!(v.as_object().unwrap().len(), 3);
        // A bare list works too, and nothing at all is an empty map (every control stays usable).
        let list = run("caps", json!({"capabilities": [{"id": "a", "supported": true}]}));
        assert_eq!(list["a"]["usable"], json!(true));
        assert_eq!(run("caps", json!({})), json!({}));
        assert_eq!(run("caps", json!({"capabilities": "no"})), json!({}));
    }

    #[test]
    fn unknown_verbs_are_errors_with_text() {
        assert!(call("nope", &json!({})).unwrap_err().contains("nope"));
    }
}
