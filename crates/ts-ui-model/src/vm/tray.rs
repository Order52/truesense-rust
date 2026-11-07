//! The tray menu: which entries it has, which one is checked, and what a click does.
//!
//! The tray lives in the desktop's panel, so it can neither be tested offscreen (the tray must
//! never be created there: it hangs without a StatusNotifier host) nor be allowed to guess. QML
//! only draws what this module returns and sends back the id of the clicked entry; every decision
//! (the checked power mode and fan mode, the first five favourites, which commands an id means)
//! is here, in plain data.
//!
//! Verbs (pure):
//!
//! * `menu {status, favourites?, pages?, supported?}`: `{summary, entries}`. `status` is a live
//!   frame or the answer of `status get` (both carry `power_mode`, `fans.mode` and `daemon`);
//!   `favourites` is the answer of `favourite list`; `pages` is `[{id, title}]` for the "Open
//!   page" submenu; `supported` the power modes the machine has (`power status`);
//! * `command {id}`: what a clicked entry does: `{type: "command", area, verb, args}`,
//!   `{type: "page", page}`, `{type: "open"}` or `{type: "quit"}`.
//!
//! Entry ids: `open`, `quit`, `power:<mode>`, `fan:<smart|auto|max>`, `favourite:<position>`,
//! `page:<id>`; headings are `title:<what>` and a placeholder line is `note:<what>`. The words
//! on static entries are translated in QML by id; names of favourites and pages are data and
//! arrive as `label`.
#![forbid(unsafe_code)]

use serde_json::{Value, json};

use super::keys::{FAVOURITE_MAX, POWER_MODES};
use crate::format;
use crate::json::{at, f64_at, list_at, str_at};

/// How many favourites the menu lists (the Favourites page has all of them).
pub const MENU_FAVOURITES: usize = 5;

/// The fan modes the tray can switch to. `manual` needs a duty, so it is only ever shown as the
/// current state, never offered (a menu click cannot say how fast).
const FAN_CHOICES: [&str; 3] = ["smart", "auto", "max"];

/// The effective fan mode: Smart is the daemon following a curve over firmware Auto, so it wins
/// when the daemon says it is on. `None` when the state is not known.
fn fan_mode(status: &Value) -> Option<&'static str> {
    if at(status, "daemon.settings.smart_fan").and_then(Value::as_bool) == Some(true) {
        return Some("smart");
    }
    match str_at(status, "fans.mode")? {
        "auto" => Some("auto"),
        "max" => Some("max"),
        "manual" => Some("manual"),
        _ => None,
    }
}

/// The power mode, when it is one of the five.
fn power_mode(status: &Value) -> Option<&'static str> {
    let mode = str_at(status, "power_mode")?;
    POWER_MODES.iter().copied().find(|m| *m == mode)
}

fn temp(status: &Value, path: &str) -> String {
    format::temp(f64_at(status, path))
}

fn entry(id: &str, kind: &str) -> Value {
    json!({"id": id, "kind": kind})
}

fn separator() -> Value {
    json!({"id": "", "kind": "separator"})
}

fn radio(id: &str, checked: bool, enabled: bool) -> Value {
    json!({"id": id, "kind": "radio", "checked": checked, "enabled": enabled})
}

fn menu(args: &Value) -> Value {
    let empty = json!({});
    let status = at(args, "status").unwrap_or(&empty);
    let mode = power_mode(status);
    let fan = fan_mode(status);
    // Without a list of supported modes every mode is offered: the command is the authority.
    let supported: Option<Vec<&str>> =
        list_at(args, "supported").map(|l| l.iter().filter_map(Value::as_str).collect());

    let mut entries = vec![entry("open", "action"), separator(), entry("title:power", "title")];
    for m in POWER_MODES {
        let enabled = supported.as_ref().is_none_or(|s| s.contains(&m));
        entries.push(radio(&format!("power:{m}"), mode == Some(m), enabled));
    }
    entries.push(separator());
    entries.push(entry("title:fan", "title"));
    for f in FAN_CHOICES {
        entries.push(radio(&format!("fan:{f}"), fan == Some(f), true));
    }
    if fan == Some("manual") {
        entries.push(radio("fan:manual", true, false));
    }
    entries.push(separator());
    entries.push(entry("title:favourites", "title"));
    let favourites = favourites(args);
    if favourites.is_empty() {
        entries.push(json!({"id": "note:no-favourites", "kind": "note"}));
    }
    entries.extend(favourites);
    entries.push(separator());
    let pages: Vec<Value> = list_at(args, "pages")
        .unwrap_or(&[])
        .iter()
        .filter_map(|p| {
            let (id, title) = (str_at(p, "id")?, str_at(p, "title")?);
            (!id.is_empty()).then(|| json!({"id": format!("page:{id}"), "kind": "action", "label": title}))
        })
        .collect();
    if !pages.is_empty() {
        entries.push(json!({"id": "submenu:pages", "kind": "submenu", "children": pages}));
        entries.push(separator());
    }
    entries.push(entry("quit", "action"));

    json!({
        "summary": {
            "mode": mode,
            "fan": fan,
            "cpu": temp(status, "sensors.cpu_temp"),
            "gpu": temp(status, "sensors.gpu_temp"),
        },
        "entries": entries,
    })
}

/// The first [`MENU_FAVOURITES`] favourites as menu entries. One that was deleted since it was
/// starred stays in its place, greyed, so the positions the PREDATOR key counts are the ones shown.
fn favourites(args: &Value) -> Vec<Value> {
    list_at(args, "favourites")
        .unwrap_or(&[])
        .iter()
        .filter_map(|f| {
            let position = f64_at(f, "position").filter(|p| p.fract() == 0.0 && *p >= 1.0)? as u64;
            let name = str_at(f, "name")?;
            Some((position, name, str_at(f, "kind").unwrap_or(""), f.get("exists").and_then(Value::as_bool)))
        })
        .filter(|(position, ..)| *position <= MENU_FAVOURITES as u64)
        .map(|(position, name, kind, exists)| {
            json!({
                "id": format!("favourite:{position}"),
                "kind": "action",
                "label": name,
                "favourite_kind": kind,
                "enabled": exists != Some(false),
            })
        })
        .collect()
}

fn command(args: &Value) -> Result<Value, String> {
    let id = str_at(args, "id").ok_or("which entry?")?;
    let (head, rest) = id.split_once(':').unwrap_or((id, ""));
    match (head, rest) {
        ("open", "") => Ok(json!({"type": "open"})),
        ("quit", "") => Ok(json!({"type": "quit"})),
        ("power", mode) if POWER_MODES.contains(&mode) => {
            Ok(json!({"type": "command", "area": "power", "verb": "set", "args": {"mode": mode}}))
        }
        ("fan", "smart") => {
            Ok(json!({"type": "command", "area": "fan", "verb": "smart", "args": {"action": "on"}}))
        }
        ("fan", "auto") => Ok(json!({"type": "command", "area": "fan", "verb": "auto", "args": {}})),
        ("fan", "max") => Ok(json!({"type": "command", "area": "fan", "verb": "max", "args": {}})),
        ("favourite", position) => match position.parse::<u64>() {
            Ok(n) if (1..=FAVOURITE_MAX).contains(&n) && position.bytes().all(|b| b.is_ascii_digit()) => {
                Ok(json!({"type": "command", "area": "favourite", "verb": "apply", "args": {"position": n}}))
            }
            _ => Err(format!("not a favourite: {id}")),
        },
        ("page", page) if !page.is_empty() => Ok(json!({"type": "page", "page": page})),
        _ => Err(format!("not a tray entry: {id}")),
    }
}

/// The tray's view-model entry point.
///
/// # Errors
/// The text for an unknown verb or an entry id that is not one of the menu's.
pub fn call(verb: &str, args: &Value) -> Result<Value, String> {
    match verb {
        "menu" => Ok(menu(args)),
        "command" => command(args),
        _ => Err(format!("unknown tray verb: {verb}")),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn ids(menu: &Value) -> Vec<String> {
        menu["entries"].as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap().to_owned()).collect()
    }

    fn find<'a>(menu: &'a Value, id: &str) -> &'a Value {
        menu["entries"].as_array().unwrap().iter().find(|e| e["id"] == json!(id)).unwrap()
    }

    fn live() -> Value {
        json!({
            "power_mode": "turbo",
            "sensors": {"cpu_temp": 54.4, "gpu_temp": null},
            "fans": {"mode": "auto"},
            "daemon": {"settings": {"smart_fan": true}},
        })
    }

    #[test]
    fn the_menu_has_the_documented_sections_in_order() {
        let m = call("menu", &json!({"status": live()})).unwrap();
        assert_eq!(
            ids(&m),
            [
                "open", "", "title:power", "power:eco", "power:quiet", "power:balanced", "power:performance",
                "power:turbo", "", "title:fan", "fan:smart", "fan:auto", "fan:max", "", "title:favourites",
                "note:no-favourites", "", "quit"
            ]
        );
    }

    #[test]
    fn the_current_power_mode_is_the_one_checked_radio() {
        let m = call("menu", &json!({"status": live()})).unwrap();
        let checked: Vec<&str> = m["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["id"].as_str().unwrap().starts_with("power:") && e["checked"] == json!(true))
            .map(|e| e["id"].as_str().unwrap())
            .collect();
        assert_eq!(checked, ["power:turbo"]);
        assert_eq!(m["summary"]["mode"], json!("turbo"));
        // An unknown or hostile mode checks nothing instead of inventing one.
        for odd in [json!("warp"), json!(5), Value::Null] {
            let m = call("menu", &json!({"status": {"power_mode": odd}})).unwrap();
            assert!(m["summary"]["mode"].is_null());
            assert!(!ids(&m).is_empty());
            assert!(m["entries"].as_array().unwrap().iter().all(|e| e["checked"] != json!(true)));
        }
    }

    #[test]
    fn smart_wins_over_the_firmware_mode_and_manual_is_shown_but_never_offered() {
        let m = call("menu", &json!({"status": live()})).unwrap();
        assert_eq!(m["summary"]["fan"], json!("smart"));
        assert_eq!(find(&m, "fan:smart")["checked"], json!(true));
        assert_eq!(find(&m, "fan:auto")["checked"], json!(false));
        let table = [
            (json!({"fans": {"mode": "auto"}}), Some("auto")),
            (json!({"fans": {"mode": "max"}}), Some("max")),
            (json!({"fans": {"mode": "auto"}, "daemon": {"settings": {"smart_fan": false}}}), Some("auto")),
            (json!({"fans": {"mode": "unknown"}}), None),
            // The daemon is not running: `status get` gives its error as text.
            (json!({"fans": {"mode": "max"}, "daemon": "ERROR: no daemon"}), Some("max")),
            (json!({}), None),
        ];
        for (status, want) in table {
            let m = call("menu", &json!({"status": status})).unwrap();
            assert_eq!(m["summary"]["fan"], json!(want), "{status}");
        }
        let m = call("menu", &json!({"status": {"fans": {"mode": "manual"}}})).unwrap();
        let manual = find(&m, "fan:manual");
        assert_eq!(manual["checked"], json!(true));
        assert_eq!(manual["enabled"], json!(false));
        // Nothing the menu offers can reach manual, and a stop is not in the vocabulary at all.
        assert!(call("command", &json!({"id": "fan:manual"})).is_err());
        assert!(call("command", &json!({"id": "fan:off"})).is_err());
    }

    #[test]
    fn modes_the_machine_lacks_are_greyed_not_removed() {
        let m = call("menu", &json!({"status": live(), "supported": ["quiet", "balanced", "turbo"]})).unwrap();
        assert_eq!(find(&m, "power:eco")["enabled"], json!(false));
        assert_eq!(find(&m, "power:turbo")["enabled"], json!(true));
        assert_eq!(find(&m, "power:performance")["enabled"], json!(false));
        let all = call("menu", &json!({"status": live()})).unwrap();
        assert_eq!(find(&all, "power:eco")["enabled"], json!(true));
    }

    #[test]
    fn only_the_first_five_favourites_are_listed_and_a_deleted_one_keeps_its_place_greyed() {
        let favs: Vec<Value> = (1..=8)
            .map(|n| json!({"position": n, "kind": "look", "name": format!("look{n}"), "exists": n != 2}))
            .collect();
        let m = call("menu", &json!({"status": live(), "favourites": favs})).unwrap();
        let listed: Vec<String> = ids(&m).into_iter().filter(|i| i.starts_with("favourite:")).collect();
        assert_eq!(listed, ["favourite:1", "favourite:2", "favourite:3", "favourite:4", "favourite:5"]);
        assert_eq!(find(&m, "favourite:1")["label"], json!("look1"));
        assert_eq!(find(&m, "favourite:2")["enabled"], json!(false));
        assert_eq!(find(&m, "favourite:3")["enabled"], json!(true));
        assert!(!ids(&m).contains(&"note:no-favourites".to_owned()));
        // Hostile entries are skipped, not trusted.
        let junk = json!([null, 3, {"position": "x", "name": "a"}, {"position": 0, "name": "a"},
            {"position": 1.5, "name": "a"}, {"position": 1}, {"position": -1, "name": "a"}]);
        let m = call("menu", &json!({"status": live(), "favourites": junk})).unwrap();
        assert!(ids(&m).iter().all(|i| !i.starts_with("favourite:")));
    }

    #[test]
    fn page_shortcuts_form_one_submenu_and_skip_nameless_pages() {
        let pages = json!([{"id": "home", "title": "Home"}, {"id": "keys", "title": "Keys"}, {"id": "", "title": "x"},
            {"id": "y"}, 7]);
        let m = call("menu", &json!({"status": live(), "pages": pages})).unwrap();
        let sub = find(&m, "submenu:pages");
        let children: Vec<&str> =
            sub["children"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
        assert_eq!(children, ["page:home", "page:keys"]);
        assert_eq!(sub["children"][1]["label"], json!("Keys"));
        // No pages, no submenu and no stray separator.
        let m = call("menu", &json!({"status": live()})).unwrap();
        assert!(!ids(&m).contains(&"submenu:pages".to_owned()));
        let all = ids(&m);
        assert_eq!(all[all.len() - 2], "");
        assert_ne!(all[all.len() - 3], "");
    }

    #[test]
    fn the_summary_is_what_the_tooltip_needs() {
        let m = call("menu", &json!({"status": live()})).unwrap();
        assert_eq!(m["summary"]["cpu"], json!("54 °C"));
        assert_eq!(m["summary"]["gpu"], json!("–"));
        let empty = call("menu", &json!({})).unwrap();
        assert_eq!(empty["summary"]["cpu"], json!("–"));
        assert!(empty["summary"]["mode"].is_null());
    }

    #[test]
    fn every_entry_of_a_full_menu_maps_to_a_command_a_page_or_nothing_for_labels() {
        let favs = json!([{"position": 1, "kind": "profile", "name": "gaming", "exists": true}]);
        let pages = json!([{"id": "cooling", "title": "Cooling"}]);
        let m = call("menu", &json!({"status": live(), "favourites": favs, "pages": pages})).unwrap();
        let mut seen = 0;
        let mut walk = |e: &Value| {
            let id = e["id"].as_str().unwrap();
            match e["kind"].as_str().unwrap() {
                "title" | "note" | "separator" | "submenu" => {}
                _ => {
                    if id == "fan:manual" {
                        return;
                    }
                    let c = call("command", &json!({"id": id})).unwrap();
                    assert!(c["type"].is_string(), "{id}");
                    seen += 1;
                }
            }
        };
        for e in m["entries"].as_array().unwrap() {
            walk(e);
            for child in e["children"].as_array().into_iter().flatten() {
                walk(child);
            }
        }
        // open, 5 modes, 3 fan modes, 1 favourite, 1 page, quit.
        assert_eq!(seen, 12);
    }

    #[test]
    fn commands_are_exactly_the_documented_bus_calls() {
        let c = |id: &str| call("command", &json!({"id": id})).unwrap();
        assert_eq!(c("open"), json!({"type": "open"}));
        assert_eq!(c("quit"), json!({"type": "quit"}));
        assert_eq!(c("power:eco"), json!({"type": "command", "area": "power", "verb": "set", "args": {"mode": "eco"}}));
        assert_eq!(
            c("fan:smart"),
            json!({"type": "command", "area": "fan", "verb": "smart", "args": {"action": "on"}})
        );
        assert_eq!(c("fan:auto"), json!({"type": "command", "area": "fan", "verb": "auto", "args": {}}));
        assert_eq!(c("fan:max"), json!({"type": "command", "area": "fan", "verb": "max", "args": {}}));
        assert_eq!(
            c("favourite:3"),
            json!({"type": "command", "area": "favourite", "verb": "apply", "args": {"position": 3}})
        );
        assert_eq!(c("page:cooling"), json!({"type": "page", "page": "cooling"}));
    }

    #[test]
    fn hostile_ids_are_refused() {
        for id in [
            "", ":", "power", "power:", "power:warp", "power:Turbo", "fan", "fan:0", "fan:stop", "favourite:0",
            "favourite:25", "favourite:+1", "favourite:1x", "favourite:", "page:", "open:1", "quit:now", "exec:rm",
            "title:power", "\u{0}",
        ] {
            assert!(call("command", &json!({"id": id})).is_err(), "{id:?}");
        }
        assert!(call("command", &json!({})).is_err());
        assert!(call("command", &json!({"id": 5})).is_err());
        assert!(call("nope", &json!({})).unwrap_err().contains("nope"));
    }
}
