#![allow(clippy::unwrap_used, clippy::panic)]
//! The `theme` commands of the bus against the simulated laptop: the list with what runs, showing a
//! theme, starting and stopping, and saving and deleting the user's own.

use serde_json::{Value, json};

use super::harness::Machine;

/// Hearth as the user's own theme.
fn mine(m: &Machine, id: &str) -> Value {
    let mut theme = m.ok("theme", "show", &json!({"name": "hearth"}))["theme"].clone();
    theme["id"] = id.into();
    theme["name"] = "Mine".into();
    theme["top"] = 0.into();
    theme
}

#[test]
fn the_list_has_the_built_in_themes_and_what_runs() {
    let m = Machine::start();
    let list = m.ok("theme", "list", &json!({}));
    assert_eq!(list["themes"].as_array().unwrap().len(), 32);
    assert_eq!(list["themes"][0]["id"], "predator");
    assert_eq!(list["themes"][0]["builtin"], true);
    assert_eq!(list["skipped"], json!([]));
    assert_eq!(list["status"], json!({"active": null, "halted": null}));
}

#[test]
fn show_gives_the_catalogue_schema_and_refuses_an_unknown_name() {
    let m = Machine::start();
    let shown = m.ok("theme", "show", &json!({"name": "hearth"}));
    assert_eq!(shown["id"], "hearth");
    assert!(shown["theme"]["colourways"].is_array() && shown["theme"]["settings"].is_array());
    assert!(m.refused("theme", "show", &json!({"name": "nobody"})).contains("no theme 'nobody'"));
    assert_eq!(m.refused("theme", "show", &json!({})), "name is needed");
}

#[test]
fn apply_starts_the_theme_and_stop_ends_it() {
    let m = Machine::start();
    let status = m.ok("theme", "apply", &json!({"name": "hearth", "speed": 50, "brightness": 30}));
    assert_eq!(status["active"]["spec"]["name"], "hearth");
    assert_eq!(status["active"]["spec"]["speed"], 50);
    assert_eq!(status["halted"], Value::Null);
    let list = m.ok("theme", "list", &json!({}));
    assert_eq!(list["status"]["active"]["spec"]["brightness"], 30);
    assert_eq!(m.ok("theme", "stop", &json!({})), json!({"stopped": true}));
    assert_eq!(m.ok("theme", "stop", &json!({})), json!({"stopped": false}));
}

#[test]
fn apply_takes_the_users_choices_and_refuses_what_the_theme_does_not_have() {
    let m = Machine::start();
    let ok = m.ok("theme", "apply", &json!({"name": "hearth", "colourway": 1}));
    assert_eq!(ok["active"]["spec"]["colourway"], 1);
    let custom =
        m.ok("theme", "apply", &json!({"name": "hearth", "colourway": "custom", "custom": ["#112233"]}));
    assert_eq!(custom["active"]["spec"]["colourway"], "custom");
    for (args, expect) in [
        (json!({"name": "hearth", "colourway": 99}), "colourway 99"),
        (json!({"name": "hearth", "colourway": "other"}), "colourway: wrong kind of value"),
        (json!({"name": "hearth", "params": {"nope": 1}}), "no setting 'nope'"),
        (json!({"name": "hearth", "params": [1]}), "params: wrong kind of value"),
        (json!({"name": "hearth", "speed": 5}), "10 to 100"),
        (json!({"name": "hearth", "brightness": 0}), "1 to 100"),
        (json!({"name": "hearth", "speed": "fast"}), "speed"),
        (json!({"name": "storm"}), "flashing has not been measured"),
        (json!({"name": "nobody"}), "no theme"),
        (json!({}), "name is needed"),
    ] {
        let e = m.refused("theme", "apply", &args);
        assert!(e.contains(expect), "{args}: {e}");
    }
    // Every refusal left the earlier theme running.
    assert_eq!(m.ok("theme", "list", &json!({}))["status"]["active"]["spec"]["colourway"], "custom");
}

#[test]
fn a_users_theme_is_saved_listed_and_deleted() {
    let m = Machine::start();
    let entry = m.ok("theme", "save", &json!({"theme": mine(&m, "calm")}));
    assert_eq!(entry["id"], "calm");
    assert_eq!(entry["builtin"], false);
    let ids: Vec<String> = m.ok("theme", "list", &json!({}))["themes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids.last().map(String::as_str), Some("calm"));
    assert!(m.root.0.join("home/themes/calm.json").exists());
    m.ok("theme", "apply", &json!({"name": "calm"}));
    // The theme as text is accepted too.
    let text = mine(&m, "calm2").to_string();
    assert_eq!(m.ok("theme", "save", &json!({"json": text}))["id"], "calm2");
    assert_eq!(m.ok("theme", "delete", &json!({"name": "calm2"})), json!("calm2"));
    assert_eq!(m.ok("theme", "delete", &json!({"name": "calm"})), json!("calm"));
    assert!(m.refused("theme", "delete", &json!({"name": "calm"})).contains("no theme 'calm'"));
}

#[test]
fn built_in_themes_are_read_only_and_a_bad_theme_is_refused_with_its_reason() {
    let m = Machine::start();
    let hearth = m.ok("theme", "show", &json!({"name": "hearth"}))["theme"].clone();
    assert!(m.refused("theme", "save", &json!({"theme": hearth})).contains("built-in"));
    assert!(m.refused("theme", "delete", &json!({"name": "hearth"})).contains("built-in"));
    let mut bad = mine(&m, "bad");
    bad["keyboard"]["effect"] = "nope".into();
    assert!(m.refused("theme", "save", &json!({"theme": bad})).contains("not an effect"));
    let mut extra = mine(&m, "extra");
    extra["script"] = "x".into();
    assert!(m.refused("theme", "save", &json!({"theme": extra})).contains("unknown field"));
    assert_eq!(m.refused("theme", "save", &json!({})), "theme is needed");
    assert_eq!(m.refused("theme", "save", &json!({"theme": 3})), "theme: expected a theme object");
    assert!(m.refused("theme", "save", &json!({"json": "{not json"})).contains("theme"));
    let mut traversal = mine(&m, "ok");
    traversal["id"] = "../evil".into();
    assert!(m.refused("theme", "save", &json!({"theme": traversal})).contains("lower-case"));
    assert!(!m.root.0.join("evil.json").exists() && !m.root.0.join("home/evil.json").exists());
}

#[test]
fn a_file_that_is_not_a_theme_is_listed_as_skipped_with_the_reason() {
    let m = Machine::start();
    m.ok("theme", "save", &json!({"theme": mine(&m, "good")}));
    std::fs::write(m.root.0.join("home/themes/broken.json"), "{oops").unwrap();
    let list = m.ok("theme", "list", &json!({}));
    assert_eq!(list["skipped"][0]["file"], "broken.json");
    assert!(list["skipped"][0]["reason"].as_str().unwrap().contains("broken"), "{list}");
    assert_eq!(list["themes"].as_array().unwrap().len(), 33);
}

#[test]
fn a_look_with_a_theme_applied_through_the_look_command_starts_it() {
    let m = Machine::start();
    let look = json!({
        "keyboard": {"mode": "effect", "name": "static", "color": "112233", "brightness": 30},
        "theme": {"name": "hearth"}
    });
    m.ok("look", "save", &json!({"name": "fire", "look": look}));
    let done = m.ok("look", "apply", &json!({"name": "fire"}));
    assert_eq!(done.as_array().unwrap().last().unwrap(), "theme: hearth");
    assert_eq!(m.ok("theme", "list", &json!({}))["status"]["active"]["spec"]["name"], "hearth");
    let shown = m.ok("look", "show", &json!({"name": "fire"}));
    assert_eq!(shown["theme"]["name"], "hearth");
}
