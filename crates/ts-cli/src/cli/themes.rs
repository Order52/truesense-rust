//! `theme`: lighting themes for the keyboard (`theme list`, `theme show NAME`,
//! `theme apply NAME`, `theme stop`).
//!
//! New in 0.5; the Python and Go editions have no such command. A theme is a designed look (a colour
//! palette and an effect with settings) that the key service draws on every key, so the verbs only
//! choose and record: `apply` is the same as applying a look that has a theme part, `stop` hands the
//! keyboard back to its static look. The command tree entry ([`command`]) and the handler ([`run`])
//! of the area live in this one file, added to the program with one line in the tree and one in
//! the dispatch table; the work and every rule are in the application layer.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde_json::Value;
use ts_api::api::{Api, ColourwayPick, ThemeList, ThemeSpec};

use super::argparse::{Arg, Command, Matches};
use super::handlers::{Cx, Fail, Outcome, fail};

/// The `theme` area of the command tree.
pub(super) fn command() -> Command {
    Command::new("theme")
        .help("lighting themes drawn on the keyboard by the key service: list | show NAME | apply NAME | stop")
        .subs(
            "verb",
            vec![
                Command::new("list").help("the built-in themes, yours, and the one that runs"),
                Command::new("show")
                    .help("a theme's colours and settings")
                    .arg(Arg::pos("name")),
                Command::new("apply")
                    .help("make a theme the current look (drawn at most 15 times a second, only until something else sets the keyboard)")
                    .arg(Arg::pos("name"))
                    .arg(Arg::opt("--colourway").metavar("N|NAME|custom").help("a colourway of the theme, by position or name"))
                    .arg(Arg::append("--custom").metavar("#RRGGBB").help("your own colour for the next slot (with --colourway custom)"))
                    .arg(Arg::append("--set").metavar("ID=VALUE").help("a setting of the theme (see `theme show`)"))
                    .arg(Arg::opt("--speed").int().help("percent of the designed speed, 10 to 100"))
                    .arg(Arg::opt("--brightness").int().help("keyboard brightness while it runs, 1 to 100 (default: the keyboard's own)")),
                Command::new("stop").help("stop the running theme and put the keyboard's own look back"),
            ],
        )
}

/// Runs a verb of the area.
pub(super) fn run(cx: &mut Cx<'_, '_>, verb: Option<&str>, m: &Matches) -> Outcome {
    let api = cx.api;
    match verb.unwrap_or("list") {
        "list" => {
            let list = api.themes();
            let text = list_text(&list);
            cx.emit(&list, Some(&text));
        }
        "show" => {
            let detail = api.theme(m.str("name").unwrap_or_default())?;
            let text = show_text(&detail.theme, detail.entry.builtin, detail.entry.runnable);
            cx.emit(&detail, Some(&text));
        }
        "apply" => {
            let spec = spec(api, m)?;
            let status = api.apply_theme(&spec)?;
            cx.emit(&status, Some(&status.describe()));
        }
        "stop" => {
            let stopped = api.stop_theme()?;
            cx.emit(&stopped, Some(if stopped { "theme stopped" } else { "no theme was running" }));
        }
        other => return Err(fail(format!("unknown theme command {other}"))),
    }
    Ok(0)
}

/// One theme per line: `*` marks the one that runs, then the id, the name and what is special.
fn list_text(list: &ThemeList) -> String {
    let running = list.status.active.as_ref().map(|a| a.spec.name.as_str());
    let mut out = String::new();
    for t in &list.themes {
        let mark = if running == Some(t.id.as_str()) { '*' } else { ' ' };
        let mut notes = Vec::new();
        if !t.builtin {
            notes.push("yours");
        }
        if !t.runnable {
            notes.push("not available yet");
        }
        let notes = if notes.is_empty() { String::new() } else { format!("  ({})", notes.join(", ")) };
        let _ = writeln!(out, "{mark} {:<18} {}{notes}", t.id, t.name);
    }
    for s in &list.skipped {
        let _ = writeln!(out, "  skipped {}: {}", s.file, s.reason);
    }
    let _ = write!(out, "{}", list.status.describe());
    out
}

/// A theme in words: mood, colourways with their colours, slots, settings.
fn show_text(theme: &Value, builtin: bool, runnable: bool) -> String {
    let text = |v: &Value| v.as_str().unwrap_or("").to_owned();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} ({}){}",
        text(&theme["name"]),
        text(&theme["id"]),
        if builtin { "" } else { "  your theme" }
    );
    let _ = writeln!(out, "{}", text(&theme["mood"]));
    if !runnable {
        let _ = writeln!(out, "not available yet: its flashing has not been measured on this keyboard");
    }
    let _ = writeln!(out, "colourways (--colourway N, or custom with --custom for each slot):");
    for (i, c) in theme["colourways"].as_array().into_iter().flatten().enumerate() {
        let colours: Vec<String> = c["palette"].as_array().into_iter().flatten().map(&text).collect();
        let _ = writeln!(out, "  {i}  {:<16} {}", text(&c["name"]), colours.join(" "));
    }
    let slots: Vec<String> = theme["custom"].as_array().into_iter().flatten().map(&text).collect();
    let _ = writeln!(out, "custom slots: {}", slots.join(", "));
    let settings = theme["settings"].as_array().map(Vec::as_slice).unwrap_or_default();
    if settings.is_empty() {
        let _ = write!(out, "settings: none");
    } else {
        let _ = write!(out, "settings (--set ID=VALUE):");
        for s in settings {
            let _ = write!(out, "\n  {:<14} {}  [{}]", text(&s["id"]), text(&s["label"]), setting_range(s));
        }
    }
    out
}

/// What a setting takes, in a few words.
fn setting_range(s: &Value) -> String {
    let n = |k: &str| s[k].as_f64().map_or_else(String::new, |v| format!("{v}"));
    match s["type"].as_str().unwrap_or("") {
        "number" | "time" => {
            let unit = s["unit"].as_str().unwrap_or("");
            format!("{} to {}{}{unit}", n("min"), n("max"), if unit.is_empty() { "" } else { " " })
        }
        "choice" => s["options"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(i, o)| format!("{i}={}", o.as_str().unwrap_or("")))
            .collect::<Vec<_>>()
            .join(", "),
        "toggle" => "on or off".to_owned(),
        "keys" => "key names, separated by commas".to_owned(),
        "key" => "a key name".to_owned(),
        other => other.to_owned(),
    }
}

/// The theme to start from the words on the line. The theme is read first so that a colourway can
/// be named and a setting's value can be read as the kind it is (a number, an option's label, on or
/// off, key names).
fn spec(api: &Api, m: &Matches) -> Result<ThemeSpec, Fail> {
    let name = m.str("name").unwrap_or_default();
    let detail = api.theme(name)?.theme;
    let mut spec = ThemeSpec::new(name);
    spec.colourway = m.str("colourway").map(|c| colourway(&detail, c)).transpose()?;
    spec.custom = m.list("custom");
    spec.speed = m.int("speed");
    spec.brightness = m.int("brightness");
    let mut params = BTreeMap::new();
    for pair in m.list("set") {
        let (id, text) = pair.split_once('=').ok_or_else(|| fail(format!("--set {pair}: use ID=VALUE")))?;
        let setting = detail["settings"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|s| s["id"].as_str() == Some(id))
            .ok_or_else(|| {
                fail(format!("theme {name} has no setting '{id}' (truesense theme show {name})"))
            })?;
        params.insert(
            id.to_owned(),
            setting_value(setting, text).map_err(|why| fail(format!("--set {id}: {why}")))?,
        );
    }
    spec.params = params;
    Ok(spec)
}

/// `N`, a colourway's name (any case) or `custom`.
fn colourway(theme: &Value, word: &str) -> Result<ColourwayPick, Fail> {
    if word.eq_ignore_ascii_case("custom") {
        return Ok(ColourwayPick::CUSTOM);
    }
    if let Ok(n) = word.parse::<u32>() {
        return Ok(ColourwayPick::Index(n));
    }
    let found = theme["colourways"]
        .as_array()
        .into_iter()
        .flatten()
        .position(|c| c["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(word)));
    found
        .and_then(|i| u32::try_from(i).ok())
        .map(ColourwayPick::Index)
        .ok_or_else(|| fail(format!("--colourway {word}: a position, custom, or a name from `theme show`")))
}

/// The value of `--set` as the setting's kind takes it.
fn setting_value(setting: &Value, text: &str) -> Result<Value, String> {
    let kind = setting["type"].as_str().unwrap_or("");
    let number = |t: &str| t.trim().parse::<f64>().ok().filter(|n| n.is_finite());
    match kind {
        "number" | "time" => number(text).map(Value::from).ok_or_else(|| format!("'{text}' is not a number")),
        "choice" => {
            if let Some(n) = number(text) {
                return Ok(Value::from(n));
            }
            let options = setting["options"].as_array().map(Vec::as_slice).unwrap_or_default();
            options
                .iter()
                .position(|o| o.as_str().is_some_and(|o| o.eq_ignore_ascii_case(text.trim())))
                .map(|i| Value::from(i as u64))
                .ok_or_else(|| format!("'{text}' is not one of the options (a number from 0, or its name)"))
        }
        "toggle" => match text.trim().to_ascii_lowercase().as_str() {
            "on" | "true" | "yes" | "1" => Ok(Value::Bool(true)),
            "off" | "false" | "no" | "0" => Ok(Value::Bool(false)),
            _ => Err(format!("'{text}' is not on or off")),
        },
        "keys" => Ok(Value::Array(text.split(',').map(|k| Value::String(k.trim().to_owned())).collect())),
        "key" => Ok(Value::String(text.trim().to_owned())),
        other => Err(format!("a setting of type {other} cannot be set here")),
    }
}
