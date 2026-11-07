//! The Keys page: what the PREDATOR key and Fn+Esc do, as the editor needs to see it.
//!
//! The daemon-side truth is the action grammar of `ts_domain::actions::parse` (`name` or
//! `name:argument`, checked when a binding is configured, never at key-press time). The page must
//! not let a person compose text that grammar refuses, and must show a hand-edited binding it
//! cannot read as an error instead of silently replacing it. This module therefore owns the
//! *editor state* of one action (`{kind, choice, text, percent, position, modes}`), turns a spec
//! into that state ([`parse`](call) verb `parse`) and back (verb `compose`), and keeps the tables
//! the pickers are built from. It cannot depend on `ts-domain`, so the grammar is mirrored here and
//! the tests prove the mirror: every action text the real parser accepts or refuses (probed through
//! the application layer's `set_key_action`, which runs that parser) gets the same verdict here, and
//! every accepted text survives `parse` then `compose`.
//!
//! Verbs (all pure; arguments and results are JSON):
//!
//! * `catalogue`: the groups, kinds and parameter shapes the action picker is built from, the
//!   gestures in table order, and the timing ranges;
//! * `parse {spec, known?}`: `{valid, error?, state, missing}`;
//! * `compose {state}`: `{valid, spec?, error?}`;
//! * `defaults {kind, known?}`: a sensible first state when the person switches the kind;
//! * `rows {bindings, known?}`: one parsed row per gesture;
//! * `status {loaded, draft, locked}`: what the footer buttons need (changed, invalid gestures);
//! * `save_args {name, draft}`: the arguments of `keys profile save`;
//! * `save_name {name, existing?}`: whether a "save as" name is acceptable and why not;
//! * `timing {long_ms, multi_ms}`: values clamped into the allowed ranges and snapped to the step;
//! * `service {rows}`: the key service read-out from the answer of `service check`.
#![forbid(unsafe_code)]

use serde_json::{Map, Value, json};

use crate::json::{at, f64_at, list_at, str_at};

/// The power modes, in the order the firmware and the pages list them.
pub const POWER_MODES: [&str; 5] = ["eco", "quiet", "balanced", "performance", "turbo"];

/// The gestures of the table, in the order the page shows them (the JSON names of `[keys]`).
pub const GESTURES: [&str; 5] = ["tap", "double", "triple", "long", "fn_esc"];

/// The lowest manual fan duty a key may set: a key can never stop a fan.
pub const FAN_MIN: u64 = 20;
/// The highest fan duty.
pub const FAN_MAX: u64 = 100;
/// How many favourites exist (`favourite:N` counts from 1 to this).
pub const FAVOURITE_MAX: u64 = 24;

/// Long press range, step and default of the built-in profiles (milliseconds).
pub const LONG_MS: Range = Range { min: 300, max: 2000, step: 50, default: 800 };
/// Multi-tap window range, step and default (milliseconds).
pub const MULTI_MS: Range = Range { min: 200, max: 1000, step: 50, default: 450 };

/// The profile names that ship with the program and cannot be overwritten or deleted.
pub const BUILTIN_PROFILES: [&str; 5] = ["native", "default", "gaming", "work", "lights"];

/// The longest profile name the library accepts.
const NAME_MAX: usize = 40;

/// A timing slider's range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    /// Smallest value.
    pub min: i64,
    /// Largest value.
    pub max: i64,
    /// Slider step.
    pub step: i64,
    /// What the built-in profiles use.
    pub default: i64,
}

impl Range {
    /// `value` inside the range and on a step (an out-of-range or off-step value in a hand-edited
    /// profile would otherwise leave the slider on a position no step reaches).
    fn fit(self, value: i64) -> i64 {
        let clamped = value.clamp(self.min, self.max);
        let steps = (clamped - self.min + self.step / 2) / self.step;
        (self.min + steps * self.step).clamp(self.min, self.max)
    }

    fn to_json(self) -> Value {
        json!({"min": self.min, "max": self.max, "step": self.step, "default": self.default})
    }
}

/// What extra control an action kind needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Param {
    /// Nothing: the kind is the whole action.
    None,
    /// One of a fixed list of words.
    Choice(&'static [&'static str]),
    /// `smart`, `auto`, `max` or a manual duty.
    Fan,
    /// An ordered list of power modes.
    Modes,
    /// The name of something in a library (`profile`, `look`, `keyboard-profile`).
    Library,
    /// A favourite position.
    Position,
    /// Free text (`exec`, `notify`).
    Text,
}

impl Param {
    fn code(self) -> &'static str {
        match self {
            Param::None => "none",
            Param::Choice(_) => "choice",
            Param::Fan => "fan",
            Param::Modes => "modes",
            Param::Library => "library",
            Param::Position => "position",
            Param::Text => "text",
        }
    }
}

const ON_OFF_TOGGLE: &[&str] = &["on", "off", "toggle"];
const ON_OFF: &[&str] = &["on", "off"];
const AUTO_MODES: &[&str] = &["off", "native", "smart"];
const OUTPUTS: &[&str] = &["speakers", "headphones"];
const MIC: &[&str] = &["mute", "unmute"];

/// One kind of action: the part before the colon, or the whole text of an action that has no
/// argument.
struct KindDef {
    kind: &'static str,
    param: Param,
}

const fn k(kind: &'static str, param: Param) -> KindDef {
    KindDef { kind, param }
}

/// The picker's groups and kinds, in display order. A kind appears exactly once; the groups are
/// the same on the screen and in the tests.
const GROUPS: &[(&str, &[KindDef])] = &[
    ("none", &[k("none", Param::None)]),
    (
        "power",
        &[
            k("power-next", Param::None),
            k("power-prev", Param::None),
            k("power", Param::Choice(&POWER_MODES)),
            k("power-cycle", Param::Modes),
            k("auto-next", Param::None),
            k("auto", Param::Choice(AUTO_MODES)),
        ],
    ),
    (
        "fans",
        &[k("fan", Param::Fan), k("fan-max-toggle", Param::None), k("smart-fan-toggle", Param::None)],
    ),
    ("profiles", &[k("profile", Param::Library), k("profile-next", Param::None)]),
    ("favourites", &[k("favourite", Param::Position), k("favourite-next", Param::None)]),
    (
        "sound",
        &[
            k("output-toggle", Param::None),
            k("output-cycle", Param::None),
            k("output", Param::Choice(OUTPUTS)),
            k("mic-toggle", Param::None),
            k("mic", Param::Choice(MIC)),
            k("voice-next", Param::None),
        ],
    ),
    ("touchpad", &[k("touchpad-toggle", Param::None), k("touchpad", Param::Choice(ON_OFF))]),
    (
        "lighting",
        &[
            k("keyboard", Param::Choice(ON_OFF_TOGGLE)),
            k("keyboard-effect-next", Param::None),
            k("keyboard-profile", Param::Library),
            k("rear", Param::Choice(ON_OFF_TOGGLE)),
            k("rear-effect-next", Param::None),
            k("look", Param::Library),
        ],
    ),
    ("hardware", &[k("battery-limit-toggle", Param::None), k("overdrive-toggle", Param::None)]),
    ("other", &[k("exec", Param::Text), k("notify", Param::Text)]),
];

fn kind_def(kind: &str) -> Option<&'static KindDef> {
    GROUPS.iter().flat_map(|(_, kinds)| kinds.iter()).find(|d| d.kind == kind)
}

/// The editor state of an action: every member is always present, so QML binds without checks;
/// only the members the kind uses mean anything.
#[derive(Debug, Clone, PartialEq, Eq)]
struct State {
    kind: String,
    choice: String,
    text: String,
    percent: u64,
    position: u64,
    modes: Vec<String>,
}

impl State {
    fn blank(kind: &str) -> Self {
        Self {
            kind: kind.to_owned(),
            choice: String::new(),
            text: String::new(),
            percent: 50,
            position: 1,
            modes: Vec::new(),
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "kind": self.kind,
            "choice": self.choice,
            "text": self.text,
            "percent": self.percent,
            "position": self.position,
            "modes": self.modes,
        })
    }

    /// Reads the state QML holds. Missing members take their blank value; wrong types are an error
    /// (QML never sends them, so one that arrives is a bug worth a message).
    fn from_json(value: &Value) -> Result<Self, String> {
        let kind = str_at(value, "kind").ok_or("the action has no kind")?;
        let mut s = Self::blank(kind);
        if let Some(v) = at(value, "choice") {
            s.choice = v.as_str().ok_or("choice must be text")?.to_owned();
        }
        if let Some(v) = at(value, "text") {
            s.text = v.as_str().ok_or("text must be text")?.to_owned();
        }
        if let Some(n) = f64_at(value, "percent") {
            s.percent = whole(n).ok_or("percent must be a whole number")?;
        }
        if let Some(n) = f64_at(value, "position") {
            s.position = whole(n).ok_or("position must be a whole number")?;
        }
        if let Some(v) = at(value, "modes") {
            s.modes = v
                .as_array()
                .ok_or("modes must be a list of text")?
                .iter()
                .map(|m| m.as_str().map(str::to_owned).ok_or("modes must be a list of text"))
                .collect::<Result<_, _>>()?;
        }
        Ok(s)
    }
}

/// A whole, non-negative number from a JSON number (QML sends sliders as floats).
fn whole(n: f64) -> Option<u64> {
    (n.is_finite() && n >= 0.0 && n.fract() == 0.0 && n < 1.0e9).then_some(n as u64)
}

/// Splits `name:argument` at the first colon and trims both parts, like the real parser.
fn split(spec: &str) -> (&str, &str) {
    let (name, arg) = spec.trim().split_once(':').unwrap_or((spec.trim(), ""));
    (name.trim(), arg.trim())
}

/// ASCII digits only, as the parser requires (it refuses `+5`, `-5`, `5.0` and non-ASCII digits).
fn digits(arg: &str) -> Option<u64> {
    if arg.is_empty() || !arg.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    arg.parse().ok()
}

fn mode_ok(m: &str) -> bool {
    POWER_MODES.contains(&m)
}

fn modes_sentence() -> String {
    // The parser lists the modes in the firmware's order, which starts with `quiet`.
    ["quiet", "balanced", "performance", "turbo", "eco"].join(", ")
}

/// The result of reading an action text.
struct Parsed {
    state: State,
    error: Option<String>,
}

/// Reads an action text into editor state, or says why it is not a valid action.
fn parse_spec(spec: &str) -> Parsed {
    let (name, arg) = split(spec);
    let invalid = |message: String| Parsed { state: State::blank("none"), error: Some(message) };
    let Some(def) = kind_def(name) else { return invalid(format!("unknown action '{name}'")) };
    let mut state = State::blank(name);
    if def.param == Param::None {
        return if arg.is_empty() {
            Parsed { state, error: None }
        } else {
            invalid(format!("'{name}' takes no argument"))
        };
    }
    if arg.is_empty() {
        return invalid(format!("'{name}' needs an argument (e.g. {name}:...)"));
    }
    match def.param {
        Param::None => {}
        Param::Choice(words) => {
            if !words.contains(&arg) {
                let message = match name {
                    "power" => format!("power mode must be one of {}", modes_sentence()),
                    other => format!("{other}: {}", words.join(" | ")),
                };
                return invalid(message);
            }
            arg.clone_into(&mut state.choice);
        }
        Param::Fan => match arg {
            "smart" | "auto" | "max" => arg.clone_into(&mut state.choice),
            _ => match digits(arg).filter(|n| (FAN_MIN..=FAN_MAX).contains(n)) {
                Some(n) => {
                    "manual".clone_into(&mut state.choice);
                    state.percent = n;
                }
                None => return invalid("fan: smart | auto | max | 20..100".to_owned()),
            },
        },
        Param::Modes => {
            let modes: Vec<String> = arg.split(',').map(str::to_owned).collect();
            if !modes.iter().all(|m| mode_ok(m)) {
                return invalid("power-cycle needs a comma list of power modes".to_owned());
            }
            state.modes = modes;
        }
        Param::Library | Param::Text => arg.clone_into(&mut state.text),
        Param::Position => match digits(arg).filter(|n| (1..=FAVOURITE_MAX).contains(n)) {
            Some(n) => state.position = n,
            None => return invalid(format!("favourite: a position from 1 to {FAVOURITE_MAX}")),
        },
    }
    Parsed { state, error: None }
}

/// The action text of an editor state, or why the state is not an action.
fn compose_state(state: &State) -> Result<String, String> {
    let Some(def) = kind_def(&state.kind) else { return Err(format!("unknown action '{}'", state.kind)) };
    let kind = def.kind;
    let argument = match def.param {
        Param::None => return Ok(kind.to_owned()),
        Param::Choice(words) => {
            if !words.contains(&state.choice.as_str()) {
                return Err(format!("{kind}: {}", words.join(" | ")));
            }
            state.choice.clone()
        }
        Param::Fan => match state.choice.as_str() {
            "smart" | "auto" | "max" => state.choice.clone(),
            "manual" if (FAN_MIN..=FAN_MAX).contains(&state.percent) => state.percent.to_string(),
            _ => return Err("fan: smart | auto | max | 20..100".to_owned()),
        },
        Param::Modes => {
            if state.modes.is_empty() || !state.modes.iter().all(|m| mode_ok(m)) {
                return Err("power-cycle needs a comma list of power modes".to_owned());
            }
            state.modes.join(",")
        }
        Param::Library | Param::Text => {
            let text = state.text.trim();
            if text.is_empty() {
                return Err(format!("'{kind}' needs an argument (e.g. {kind}:...)"));
            }
            text.to_owned()
        }
        Param::Position => {
            if !(1..=FAVOURITE_MAX).contains(&state.position) {
                return Err(format!("favourite: a position from 1 to {FAVOURITE_MAX}"));
            }
            state.position.to_string()
        }
    };
    Ok(format!("{kind}:{argument}"))
}

/// Whether a library kind names something that is not in the list the page knows. An unknown
/// (not yet loaded) list never counts as missing.
fn is_missing(state: &State, known: &Value) -> bool {
    let Some(def) = kind_def(&state.kind) else { return false };
    if def.param != Param::Library {
        return false;
    }
    match list_at(known, &state.kind) {
        Some(names) => !names.iter().any(|n| n.as_str() == Some(state.text.as_str())),
        None => false,
    }
}

/// The first state shown when the person switches the kind of an action.
fn defaults_for(kind: &str, known: &Value) -> Result<State, String> {
    let def = kind_def(kind).ok_or_else(|| format!("unknown action '{kind}'"))?;
    let mut s = State::blank(kind);
    match def.param {
        Param::None => {}
        Param::Choice(words) => {
            // A sensible first pick: balanced for the power modes, native for the mode control,
            // toggle where there is one (a key that only ever switches on is rarely wanted).
            s.choice = match kind {
                "power" => "balanced",
                "auto" => "native",
                _ if words.contains(&"toggle") => "toggle",
                _ => words.first().copied().unwrap_or(""),
            }
            .to_owned();
        }
        Param::Fan => {
            "smart".clone_into(&mut s.choice);
            s.percent = 60;
        }
        Param::Modes => s.modes = ["eco", "balanced", "turbo"].map(str::to_owned).to_vec(),
        Param::Library => {
            s.text = list_at(known, kind)
                .and_then(|l| l.first())
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
        }
        Param::Position => s.position = 1,
        Param::Text => {}
    }
    Ok(s)
}

fn parsed_json(spec: &str, known: &Value) -> Value {
    let p = parse_spec(spec);
    json!({
        "valid": p.error.is_none(),
        "error": p.error,
        "state": p.state.to_json(),
        "missing": p.error.is_none() && is_missing(&p.state, known),
    })
}

fn catalogue() -> Value {
    let groups: Vec<Value> = GROUPS
        .iter()
        .map(|(id, kinds)| {
            let kinds: Vec<Value> = kinds
                .iter()
                .map(|d| {
                    let mut o = Map::new();
                    o.insert("kind".to_owned(), json!(d.kind));
                    o.insert("param".to_owned(), json!(d.param.code()));
                    if let Param::Choice(words) = d.param {
                        o.insert("options".to_owned(), json!(words));
                    }
                    Value::Object(o)
                })
                .collect();
            json!({"id": id, "kinds": kinds})
        })
        .collect();
    json!({
        "groups": groups,
        "gestures": GESTURES,
        "modes": POWER_MODES,
        "fan": {"min": FAN_MIN, "max": FAN_MAX, "step": 5, "choices": ["smart", "auto", "max", "manual"]},
        "favourites": {"max": FAVOURITE_MAX},
        "long_ms": LONG_MS.to_json(),
        "multi_ms": MULTI_MS.to_json(),
        "builtin_profiles": BUILTIN_PROFILES,
    })
}

/// The action text of one gesture in a bindings object; a missing member reads as `none`.
fn action_of<'a>(bindings: &'a Value, gesture: &str) -> &'a str {
    str_at(bindings, gesture).unwrap_or("none")
}

fn rows(bindings: &Value, known: &Value) -> Value {
    let rows: Vec<Value> = GESTURES
        .iter()
        .map(|g| {
            let spec = action_of(bindings, g);
            let mut row = parsed_json(spec, known);
            if let Some(o) = row.as_object_mut() {
                o.insert("gesture".to_owned(), json!(g));
                o.insert("spec".to_owned(), json!(spec));
            }
            row
        })
        .collect();
    json!(rows)
}

/// A timing value from a bindings object, fitted into its range; a missing one is the default.
fn timing_of(bindings: &Value, key: &str, range: Range) -> i64 {
    f64_at(bindings, key).map_or(range.default, |n| range.fit(n.round() as i64))
}

fn timing(args: &Value) -> Value {
    json!({
        "long_ms": timing_of(args, "long_ms", LONG_MS),
        "multi_ms": timing_of(args, "multi_ms", MULTI_MS),
    })
}

/// Whether two bindings objects mean the same thing (actions as text, timing as numbers).
fn same_bindings(a: &Value, b: &Value) -> bool {
    GESTURES.iter().all(|g| action_of(a, g).trim() == action_of(b, g).trim())
        && timing_of(a, "long_ms", LONG_MS) == timing_of(b, "long_ms", LONG_MS)
        && timing_of(a, "multi_ms", MULTI_MS) == timing_of(b, "multi_ms", MULTI_MS)
}

fn status(args: &Value) -> Value {
    let empty = json!({});
    let loaded = at(args, "loaded").unwrap_or(&empty);
    let draft = at(args, "draft").unwrap_or(&empty);
    let locked = args.get("locked").and_then(Value::as_bool).unwrap_or(false);
    let invalid: Vec<&str> =
        GESTURES.iter().copied().filter(|g| parse_spec(action_of(draft, g)).error.is_some()).collect();
    let changed = !same_bindings(loaded, draft);
    json!({
        "changed": changed,
        "invalid": invalid,
        "locked": locked,
        // A locked (built-in) profile is never saved; an invalid binding is never saved.
        "can_save": !locked && invalid.is_empty(),
        "can_use": invalid.is_empty(),
    })
}

fn save_args(args: &Value) -> Result<Value, String> {
    let name = str_at(args, "name").ok_or("which name?")?;
    let empty = json!({});
    let draft = at(args, "draft").unwrap_or(&empty);
    let mut o = Map::new();
    o.insert("action".to_owned(), json!("save"));
    o.insert("name".to_owned(), json!(name));
    for g in GESTURES {
        let spec = action_of(draft, g).trim();
        if let Some(e) = parse_spec(spec).error {
            return Err(format!("{g}: {e}"));
        }
        o.insert(g.to_owned(), json!(spec));
    }
    o.insert("long_ms".to_owned(), json!(timing_of(draft, "long_ms", LONG_MS)));
    o.insert("multi_ms".to_owned(), json!(timing_of(draft, "multi_ms", MULTI_MS)));
    Ok(Value::Object(o))
}

/// Why a "save as" name is not acceptable (`""` when it is): the library's own rule (ASCII
/// letters, digits, `.`, `_`, `-`, one to forty characters) plus "not a built-in name". The code
/// is translated on the QML side.
fn name_problem(name: &str, existing_builtin: bool) -> &'static str {
    if name.is_empty() {
        "empty"
    } else if name.len() > NAME_MAX {
        "length"
    } else if !name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')) {
        "chars"
    } else if existing_builtin || BUILTIN_PROFILES.contains(&name) {
        "builtin"
    } else {
        ""
    }
}

fn save_name(args: &Value) -> Value {
    let name = str_at(args, "name").unwrap_or("").trim();
    let code = name_problem(name, false);
    let exists = list_at(args, "existing").is_some_and(|l| l.iter().any(|n| n.as_str() == Some(name)));
    json!({"ok": code.is_empty(), "name": name, "code": code, "overwrites": code.is_empty() && exists})
}

/// The key service's state from the rows of `service check`.
fn service(args: &Value) -> Value {
    const UNIT: &str = "truesense-keys.service";
    let row = list_at(args, "rows")
        .and_then(|rows| rows.iter().find(|r| str_at(r, "unit") == Some(UNIT)));
    let Some(row) = row else {
        return json!({"state": "unknown", "unit": UNIT, "fix": null, "detail": null});
    };
    let active = str_at(row, "active").unwrap_or("unknown");
    let enabled = str_at(row, "enabled").unwrap_or("unknown");
    let state = match active {
        "active" | "activating" | "reloading" => "running",
        "unknown" => "unknown",
        _ => "stopped",
    };
    let fix = str_at(row, "fix").filter(|f| !f.is_empty());
    json!({"state": state, "unit": UNIT, "enabled": enabled, "active": active, "fix": fix, "detail": null})
}

/// The Keys page's view-model entry point.
///
/// # Errors
/// The text for an unknown verb or arguments that cannot be read.
pub fn call(verb: &str, args: &Value) -> Result<Value, String> {
    let known = args.get("known").cloned().unwrap_or(Value::Null);
    match verb {
        "catalogue" => Ok(catalogue()),
        "parse" => Ok(parsed_json(str_at(args, "spec").unwrap_or(""), &known)),
        "compose" => {
            let state = State::from_json(at(args, "state").ok_or("compose needs a state")?)?;
            Ok(match compose_state(&state) {
                Ok(spec) => json!({"valid": true, "spec": spec}),
                Err(error) => json!({"valid": false, "error": error}),
            })
        }
        "defaults" => {
            let kind = str_at(args, "kind").ok_or("defaults needs a kind")?;
            Ok(defaults_for(kind, &known)?.to_json())
        }
        "rows" => Ok(rows(at(args, "bindings").unwrap_or(&Value::Null), &known)),
        "status" => Ok(status(args)),
        "save_args" => save_args(args),
        "save_name" => Ok(save_name(args)),
        "timing" => Ok(timing(args)),
        "service" => Ok(service(args)),
        _ => Err(format!("unknown keys verb: {verb}")),
    }
}

// The tests are long (they probe the real action parser through the application layer), so they
// live beside this file; `build.rs` lists only `*.rs` files of this directory, never `keys/`.
#[cfg(test)]
#[path = "keys/tests.rs"]
mod tests;
