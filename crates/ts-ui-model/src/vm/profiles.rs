//! What the Profiles and favourites page (and the Favourites strip of Home) decide, as pure
//! functions over command answers. No sentences: kinds, icons and areas are ids the QML side
//! translates.
//!
//! Verbs (all arguments are JSON objects):
//!
//! * `favourites` `{list}`: the model of a favourite list (`favourite list` / `favourite move` answer):
//!   kind icon, missing mark, which moves are possible;
//! * `move` `{position, delta, count}`: the `favourite move` arguments for "one up" / "one down";
//! * `profiles` `{list, current, starred?}`: the rows of the whole-setup profile list (`starred`: the
//!   names of the starred profiles);
//! * `parts` `{profile}`: what a profile changes, one entry per setting, as translatable ids;
//! * `name_check` `{name}`: whether a name is acceptable for `profile save`.
#![forbid(unsafe_code)]

use serde_json::{Value, json};

use crate::json::{at, bool_at, i64_at, list_at, str_at};

/// The most favourites the store accepts (`favourite toggle` refuses the next one).
pub const MAX_FAVOURITES: usize = ts_domain::favourites::MAX_FAVOURITES;
/// The longest profile name the command line accepts.
pub(crate) const MAX_NAME_LEN: usize = ts_foundation::names::NAME_MAX;

/// Calls one verb of the Profiles view-model.
///
/// # Errors
/// The text for an unknown verb.
pub fn call(verb: &str, args: &Value) -> Result<Value, String> {
    match verb {
        "favourites" => Ok(favourites(args)),
        "move" => Ok(move_args(args)),
        "profiles" => Ok(profiles(args)),
        "parts" => Ok(parts(args)),
        "name_check" => Ok(name_check(args)),
        _ => Err(format!("unknown profiles verb: {verb}")),
    }
}

/// The canonical kind and its icon. `curve` and `smart` are the spellings `favourite toggle`
/// also accepts; a kind a newer build added is `None`: skipped, never an error (APP-DESIGN 4.6),
/// so a newer favourites file still works in an older window.
fn kind(raw: &str) -> Option<(&'static str, &'static str)> {
    match raw {
        "look" => Some(("look", "palette")),
        "fan_curve" | "curve" => Some(("fan_curve", "fan")),
        "smart_profile" | "smart" => Some(("smart_profile", "smart")),
        "profile" => Some(("profile", "profiles")),
        "theme" => Some(("theme", "sparkle")),
        _ => None,
    }
}

fn favourites(args: &Value) -> Value {
    let list = list_at(args, "list").unwrap_or_default();
    // Positions are the store's (1-based, in the user's order). An entry without one gets the place
    // it has in the list; what is skipped still counts for "first" and "last", because the store
    // moves among all of them.
    let positions: Vec<i64> =
        list.iter().enumerate().map(|(i, e)| i64_at(e, "position").unwrap_or(i as i64 + 1)).collect();
    let first = positions.iter().copied().min();
    let last = positions.iter().copied().max();
    let items: Vec<Value> = list
        .iter()
        .zip(&positions)
        .filter_map(|(entry, &position)| {
            let (kind, icon) = kind(str_at(entry, "kind")?)?;
            let name = str_at(entry, "name").filter(|n| !n.is_empty())?;
            Some(json!({
                "position": position,
                "kind": kind,
                "icon": icon,
                "name": name,
                // A theme is shown by its name ("vision" is "Predator Vision"); the others by the name
                // people typed.
                "title": if kind == "theme" { theme_title(name) } else { name.to_owned() },
                // A missing member is "exists": only the store's explicit false greys an item.
                "exists": bool_at(entry, "exists").unwrap_or(true),
                "can_up": Some(position) != first,
                "can_down": Some(position) != last,
            }))
        })
        .collect();
    json!({
        "count": items.len(),
        "empty": items.is_empty(),
        "full": list.len() >= MAX_FAVOURITES,
        "items": items,
    })
}

/// The arguments of `favourite move` for a step of `delta` places from `position`, or `null` when
/// it would leave the list (the buttons are disabled then, this is the guard behind them).
/// A name made of lower-case words and dashes ("battery-saver") as a title ("Battery saver"): for
/// the things that ship with TrueSense and the themes, whose ids are written that way. A person's
/// own profile keeps the name they typed.
pub(crate) fn title_of(id: &str) -> String {
    let words = id.replace(['-', '_'], " ");
    let mut chars = words.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// What a theme is called: a built-in one by its catalogue name, the person's own one (whose id was
/// made from the name they typed) by its id as a title.
fn theme_title(id: &str) -> String {
    ts_fx::themes::find(id).map_or_else(|| title_of(id), |t| t.name.clone())
}

fn move_args(args: &Value) -> Value {
    let (Some(position), Some(delta), Some(count)) =
        (i64_at(args, "position"), i64_at(args, "delta"), i64_at(args, "count"))
    else {
        return Value::Null;
    };
    let Some(to) = position.checked_add(delta) else { return Value::Null };
    if delta == 0 || position < 1 || position > count || to < 1 || to > count {
        return Value::Null;
    }
    json!({ "from": position, "to": to })
}

fn profiles(args: &Value) -> Value {
    let current = str_at(args, "current");
    let starred: Vec<&str> = list_at(args, "starred").unwrap_or_default().iter().filter_map(Value::as_str).collect();
    let rows: Vec<Value> = list_at(args, "list")
        .unwrap_or_default()
        .iter()
        .filter_map(|p| {
            let name = str_at(p, "name").filter(|n| !n.is_empty())?;
            let yours = str_at(p, "source") == Some("yours");
            Some(json!({
                "name": name,
                "title": if yours { name.to_owned() } else { title_of(name) },
                "starred": starred.contains(&name),
                "source": if yours { "yours" } else { "shipped" },
                "description": str_at(p, "description").unwrap_or(""),
                "current": current == Some(name),
                // Shipped profiles are read-only: `profile delete` would refuse them anyway.
                "deletable": yours,
            }))
        })
        .collect();
    json!({ "count": rows.len(), "empty": rows.is_empty(), "rows": rows })
}

fn entry(label: &str, value: Option<String>, on: Option<bool>) -> Value {
    json!({ "label": label, "value": value, "on": on })
}

fn text(profile: &Value, path: &str) -> Option<String> {
    let v = at(profile, path)?;
    v.as_str().map(str::to_owned).or_else(|| v.as_i64().map(|n| n.to_string())).or_else(|| v.as_f64().map(|n| n.to_string()))
}

fn mode_word(profile: &Value, path: &str) -> Option<String> {
    let s = text(profile, path)?;
    let mut chars = s.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect())
}

/// What a whole-setup profile changes, one entry per setting it lists (and only those: a profile
/// leaves everything else alone). `label` is an id from a fixed set; `value` is data (a mode, a
/// curve, a colour) and `on` a switch, both formatted and translated by the page.
fn parts(args: &Value) -> Value {
    let p = args.get("profile").unwrap_or(&Value::Null);
    let mut out = Vec::new();
    let mut add = |label: &str, value: Option<String>, on: Option<bool>| {
        if value.is_some() || on.is_some() {
            out.push(entry(label, value, on));
        }
    };
    add("power_mode", mode_word(p, "power.mode"), None);
    add("power_auto", text(p, "power.auto"), None);
    add("fan_mode", mode_word(p, "fan.mode"), None);
    if let (Some(cpu), Some(gpu)) = (text(p, "fan.cpu"), text(p, "fan.gpu")) {
        add("fan_duty", Some(format!("{cpu} / {gpu}")), None);
    }
    add("fan_curve", text(p, "fan.curve"), None);
    add("kb_effect", text(p, "keyboard.effect"), None);
    add("kb_profile", text(p, "keyboard.profile"), None);
    add("kb_backlight", None, bool_at(p, "keyboard.on"));
    add("kb_timeout", None, bool_at(p, "keyboard.timeout"));
    if at(p, "rear").is_some_and(|r| r.as_object().is_some_and(|o| !o.is_empty())) {
        match text(p, "rear.effect") {
            Some(effect) => add("rear_effect", Some(effect), None),
            None => add("rear_state", None, bool_at(p, "rear.on")),
        }
    }
    add("battery_limit", None, bool_at(p, "battery.limit"));
    add("battery_usb", text(p, "battery.usb_charge"), None);
    add("screen_overdrive", None, bool_at(p, "display.overdrive"));
    add("boot_sound", None, bool_at(p, "boot.sound"));
    add("audio_output", text(p, "audio.output"), None);
    add("audio_voice_headset", text(p, "audio.voice_headset"), None);
    add("audio_voice_array", text(p, "audio.voice_array"), None);
    json!({ "entries": out, "empty": out.is_empty() })
}

/// A profile name as the command line accepts it: letters, digits, `.`, `_`, `-`, up to 40; the
/// answer carries the trimmed name to send.
fn name_check(args: &Value) -> Value {
    let name = str_at(args, "name").unwrap_or("").trim();
    let ok = !name.is_empty()
        && name.chars().count() <= MAX_NAME_LEN
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    json!({ "ok": ok, "name": name })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn run(verb: &str, args: Value) -> Value {
        call(verb, &args).unwrap()
    }

    fn list() -> Value {
        json!([
            {"position": 1, "kind": "look", "name": "calm", "exists": true},
            {"position": 2, "kind": "fan_curve", "name": "quiet", "exists": false},
            {"position": 3, "kind": "smart_profile", "name": "gaming", "exists": true},
            {"position": 4, "kind": "profile", "name": "battery-saver", "exists": true},
        ])
    }

    #[test]
    fn unknown_verb_names_itself() {
        assert!(call("nope", &json!({})).unwrap_err().contains("nope"));
    }

    #[test]
    fn favourites_get_kind_icons_missing_marks_and_possible_moves() {
        let v = run("favourites", json!({"list": list()}));
        assert_eq!(v["count"], json!(4));
        assert_eq!(v["empty"], json!(false));
        let items = v["items"].as_array().unwrap();
        let icons: Vec<&str> = items.iter().map(|i| i["icon"].as_str().unwrap()).collect();
        assert_eq!(icons, ["palette", "fan", "smart", "profiles"]);
        assert_eq!(items[1]["exists"], json!(false));
        assert_eq!(items[0]["exists"], json!(true));
        assert_eq!((items[0]["can_up"].clone(), items[0]["can_down"].clone()), (json!(false), json!(true)));
        assert_eq!((items[3]["can_up"].clone(), items[3]["can_down"].clone()), (json!(true), json!(false)));
        assert_eq!((items[1]["can_up"].clone(), items[1]["can_down"].clone()), (json!(true), json!(true)));
    }

    #[test]
    fn the_aliases_of_toggle_are_understood_and_unknown_kinds_are_skipped() {
        let v = run(
            "favourites",
            json!({"list": [
                {"position": 1, "kind": "curve", "name": "a"},
                {"position": 2, "kind": "hologram", "name": "from-the-future"},
                {"position": 3, "kind": "smart", "name": "b"},
            ]}),
        );
        let items = v["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["kind"], json!("fan_curve"));
        assert_eq!(items[1]["kind"], json!("smart_profile"));
        assert_eq!(items[1]["position"], json!(3), "positions stay the store's");
        assert_eq!(items[1]["can_down"], json!(false));
    }

    #[test]
    fn an_empty_or_broken_list_is_just_empty() {
        for args in [json!({}), json!({"list": []}), json!({"list": null}), json!({"list": "x"}), json!({"list": [1, null, {"kind": "look"}, {"name": "x"}, {"kind": "look", "name": ""}]})] {
            let v = run("favourites", args);
            assert_eq!(v["empty"], json!(true));
            assert_eq!(v["items"], json!([]));
        }
    }

    #[test]
    fn a_list_at_the_limit_is_full() {
        let many: Vec<Value> = (1..=24).map(|i| json!({"position": i, "kind": "look", "name": format!("l{i}")})).collect();
        assert_eq!(run("favourites", json!({"list": many}))["full"], json!(true));
        assert_eq!(run("favourites", json!({"list": list()}))["full"], json!(false));
    }

    #[test]
    fn entries_without_positions_take_their_place_in_the_list() {
        let v = run("favourites", json!({"list": [{"kind": "look", "name": "a"}, {"kind": "look", "name": "b"}]}));
        assert_eq!(v["items"][1]["position"], json!(2));
        assert_eq!(v["items"][1]["can_down"], json!(false));
    }

    #[test]
    fn a_step_stays_inside_the_list() {
        assert_eq!(run("move", json!({"position": 2, "delta": -1, "count": 4})), json!({"from": 2, "to": 1}));
        assert_eq!(run("move", json!({"position": 2, "delta": 1, "count": 4})), json!({"from": 2, "to": 3}));
        assert_eq!(run("move", json!({"position": 1, "delta": -1, "count": 4})), Value::Null);
        assert_eq!(run("move", json!({"position": 4, "delta": 1, "count": 4})), Value::Null);
        assert_eq!(run("move", json!({"position": 2, "delta": 0, "count": 4})), Value::Null);
        assert_eq!(run("move", json!({"position": 9, "delta": -1, "count": 4})), Value::Null);
        assert_eq!(run("move", json!({"position": 0, "delta": 1, "count": 4})), Value::Null);
        assert_eq!(run("move", json!({"position": 2, "delta": 1})), Value::Null);
        assert_eq!(run("move", json!({"position": i64::MAX, "delta": 1, "count": i64::MAX})), Value::Null);
    }

    #[test]
    fn profiles_mark_the_current_one_and_only_yours_can_be_deleted() {
        let v = run(
            "profiles",
            json!({"current": "gaming", "list": [
                {"name": "battery-saver", "source": "shipped", "description": "Longest battery life"},
                {"name": "gaming", "source": "yours"},
                {"name": "", "source": "yours"},
                {"source": "yours"},
            ]}),
        );
        assert_eq!(v["count"], json!(2));
        let rows = v["rows"].as_array().unwrap();
        assert_eq!(rows[0]["deletable"], json!(false));
        assert_eq!(rows[0]["current"], json!(false));
        assert_eq!(rows[0]["description"], json!("Longest battery life"));
        assert_eq!(rows[1]["deletable"], json!(true));
        assert_eq!(rows[1]["current"], json!(true));
        assert_eq!(rows[1]["description"], json!(""));
    }

    #[test]
    fn shipped_profiles_get_a_title_and_the_starred_ones_are_marked() {
        let v = run(
            "profiles",
            json!({"starred": ["gaming", 7], "list": [
                {"name": "battery-saver", "source": "shipped"},
                {"name": "my-gaming_2", "source": "yours"},
                {"name": "gaming", "source": "shipped"},
            ]}),
        );
        let rows = v["rows"].as_array().unwrap();
        let titles: Vec<&str> = rows.iter().map(|r| r["title"].as_str().unwrap()).collect();
        assert_eq!(titles, ["Battery saver", "my-gaming_2", "Gaming"], "a name the person typed is kept");
        let stars: Vec<bool> = rows.iter().map(|r| r["starred"].as_bool().unwrap()).collect();
        assert_eq!(stars, [false, false, true]);
        assert_eq!(run("profiles", json!({"list": [{"name": "a"}]}))["rows"][0]["starred"], json!(false));
    }

    #[test]
    fn starred_themes_are_favourites_too() {
        // The theme gallery stars themes (`favourite toggle {kind: "theme"}`): they must show.
        let v = run("favourites", json!({"list": [{"position": 1, "kind": "theme", "name": "vision"}, {"position": 3, "kind": "theme", "name": "night-sky"}, {"position": 2, "kind": "look", "name": "my-look"}]}));
        let items = v["items"].as_array().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!((items[0]["kind"].as_str(), items[0]["icon"].as_str()), (Some("theme"), Some("sparkle")));
        assert_eq!(items[0]["title"], json!("Predator Vision"), "a built-in theme by its catalogue name");
        assert_eq!(items[1]["title"], json!("Night sky"), "a theme of the person's own by its id");
        assert_eq!(items[2]["title"], json!("my-look"));
        assert_eq!(title_of(""), "");
    }

    #[test]
    fn an_unknown_source_counts_as_shipped_so_it_cannot_be_deleted() {
        let v = run("profiles", json!({"list": [{"name": "x", "source": "elsewhere"}]}));
        assert_eq!(v["rows"][0]["source"], json!("shipped"));
        assert_eq!(v["rows"][0]["deletable"], json!(false));
        assert_eq!(run("profiles", json!({}))["empty"], json!(true));
    }

    fn labels(v: &Value) -> Vec<String> {
        v["entries"].as_array().unwrap().iter().map(|e| e["label"].as_str().unwrap().to_owned()).collect()
    }

    #[test]
    fn parts_list_only_what_a_profile_sets() {
        let v = run(
            "parts",
            json!({"profile": {
                "power": {"mode": "turbo", "auto": "smart"},
                "fan": {"mode": "manual", "cpu": 60, "gpu": 55},
                "keyboard": {"effect": "wave", "on": true},
                "rear": {"effect": "neon", "color": "ff0000"},
                "battery": {"limit": false, "usb_charge": 10},
                "display": {"overdrive": true},
                "boot": {"sound": false},
                "audio": {"output": "speakers", "voice_headset": "off"},
                "description": "x",
            }}),
        );
        assert_eq!(
            labels(&v),
            ["power_mode", "power_auto", "fan_mode", "fan_duty", "kb_effect", "kb_backlight", "rear_effect",
             "battery_limit", "battery_usb", "screen_overdrive", "boot_sound", "audio_output", "audio_voice_headset"]
        );
        let e = v["entries"].as_array().unwrap();
        assert_eq!(e[0]["value"], json!("Turbo"));
        assert_eq!(e[3]["value"], json!("60 / 55"));
        assert_eq!(e[5]["on"], json!(true));
        assert_eq!(e[7]["on"], json!(false), "an explicit off is still a setting");
        assert_eq!(e[8]["value"], json!("10"));
        assert_eq!(v["empty"], json!(false));
    }

    #[test]
    fn a_rear_light_that_is_only_on_or_off_and_an_empty_profile() {
        let v = run("parts", json!({"profile": {"rear": {"on": false}}}));
        assert_eq!(labels(&v), ["rear_state"]);
        assert_eq!(v["entries"][0]["on"], json!(false));
        for p in [json!({"profile": {}}), json!({"profile": {"rear": {}, "power": {}}}), json!({}), json!({"profile": 3}), json!({"profile": {"power": 4}})] {
            assert_eq!(run("parts", p)["empty"], json!(true));
        }
    }

    #[test]
    fn names_follow_the_command_line_rule() {
        for ok in ["gaming", "my-gaming_2.0", "A", &"x".repeat(40)] {
            assert_eq!(run("name_check", json!({"name": ok}))["ok"], json!(true), "{ok}");
        }
        for bad in ["", "  ", "has space", "slash/name", "../etc", "ünï", &"x".repeat(41), "a\nb", "a\tb"] {
            assert_eq!(run("name_check", json!({"name": bad}))["ok"], json!(false), "{bad:?}");
        }
        assert_eq!(run("name_check", json!({"name": "  padded  "})), json!({"ok": true, "name": "padded"}));
        assert_eq!(run("name_check", json!({}))["ok"], json!(false));
        assert_eq!(run("name_check", json!({"name": 3}))["ok"], json!(false));
    }
}
