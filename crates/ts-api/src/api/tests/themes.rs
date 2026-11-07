//! Themes through the application layer: the lists, showing, saving the user's own, starting and
//! stopping, and the way a look with a theme part and the keyboard commands interact with the
//! running theme.

use serde_json::{Value, json};
use ts_domain::looks::Look;
use ts_domain::themes::ThemeSpec;

use super::Fx;
use crate::api::AnimationRequest;

fn spec(v: Value) -> ThemeSpec {
    serde_json::from_value(v).unwrap()
}

/// Hearth under another id, as a user would write it into a file.
fn mine(id: &str) -> String {
    let fx = Fx::new();
    let mut v = fx.api.theme("hearth").unwrap().theme;
    v["id"] = id.into();
    v["name"] = "Mine".into();
    v["top"] = 0.into();
    v.to_string()
}

#[test]
fn the_list_has_the_32_built_in_themes_and_nothing_running() {
    let fx = Fx::new();
    let list = fx.api.themes();
    assert_eq!(list.themes.len(), 32);
    assert!(list.themes.iter().all(|t| t.builtin));
    assert!(list.status.active.is_none() && list.status.halted.is_none());
    assert_eq!(list.status.describe(), "no theme is running");
    let value = serde_json::to_value(&list).unwrap();
    assert_eq!(value["themes"][0]["id"], "predator");
    assert_eq!(value["status"]["active"], Value::Null);
}

#[test]
fn show_gives_the_theme_in_the_catalogues_schema() {
    let fx = Fx::new();
    let shown = serde_json::to_value(fx.api.theme("hearth").unwrap()).unwrap();
    assert_eq!(shown["id"], "hearth");
    assert_eq!(shown["builtin"], true);
    assert!(shown["theme"]["colourways"].is_array() && shown["theme"]["keyboard"].is_object());
    assert!(fx.api.theme("nobody").unwrap_err().to_string().contains("no theme 'nobody'"));
}

#[test]
fn a_users_theme_is_saved_listed_shown_copied_and_deleted() {
    let fx = Fx::new();
    let entry = fx.api.save_theme(&mine("calm")).unwrap();
    assert_eq!((entry.id.as_str(), entry.builtin), ("calm", false));
    let ids: Vec<String> = fx.api.themes().themes.into_iter().map(|t| t.id).collect();
    assert_eq!(ids.last().map(String::as_str), Some("calm"));
    assert!(fx.dir.0.join("home/themes/calm.json").exists());
    assert_eq!(fx.api.theme("calm").unwrap().entry.name, "Mine");
    assert_eq!(fx.api.copy_theme("calm", None).unwrap(), "calm-copy");
    fx.api.delete_theme("calm-copy").unwrap();
    fx.api.delete_theme("calm").unwrap();
    assert!(fx.api.theme("calm").is_err());
}

#[test]
fn built_in_themes_cannot_be_overwritten_or_deleted_and_bad_themes_are_refused() {
    let fx = Fx::new();
    let hearth = fx.api.theme("hearth").unwrap().theme.to_string();
    assert!(fx.api.save_theme(&hearth).unwrap_err().to_string().contains("built-in"));
    assert!(fx.api.delete_theme("hearth").unwrap_err().to_string().contains("built-in"));
    assert!(fx.api.save_theme("not json").is_err());
    let mut bad: Value = serde_json::from_str(&mine("bad")).unwrap();
    bad["keyboard"]["effect"] = "nope".into();
    assert!(fx.api.save_theme(&bad.to_string()).unwrap_err().to_string().contains("not an effect"));
    assert!(!fx.dir.0.join("home/themes").exists(), "nothing was written");
}

#[test]
fn applying_a_theme_records_it_and_stopping_ends_it() {
    let fx = Fx::new();
    let status = fx.api.apply_theme(&spec(json!({"name": "hearth", "speed": 70}))).unwrap();
    let active = status.active.unwrap();
    assert_eq!((active.spec.name.as_str(), active.spec.speed), ("hearth", Some(70)));
    assert_eq!(fx.api.theme_status().describe(), "theme hearth is running");
    assert!(fx.api.stop_theme().unwrap());
    assert!(!fx.api.stop_theme().unwrap(), "nothing was running");
    assert!(fx.api.theme_status().active.is_none());
}

#[test]
fn a_theme_that_is_refused_changes_nothing() {
    let fx = Fx::new();
    fx.api.apply_theme(&spec(json!({"name": "hearth"}))).unwrap();
    for bad in [
        json!({"name": "nobody"}),
        json!({"name": "storm"}),
        json!({"name": "hearth", "speed": 5}),
        json!({"name": "hearth", "params": {"nope": 1}}),
        json!({"name": "hearth", "colourway": 40}),
    ] {
        assert!(fx.api.apply_theme(&spec(bad.clone())).is_err(), "{bad}");
    }
    assert_eq!(fx.api.theme_status().active.unwrap().spec.name, "hearth");
}

#[test]
fn a_user_theme_runs_like_a_built_in_one() {
    let fx = Fx::new();
    fx.api.save_theme(&mine("calm")).unwrap();
    fx.api.apply_theme(&spec(json!({"name": "calm"}))).unwrap();
    assert_eq!(fx.api.theme_status().active.unwrap().spec.name, "calm");
}

#[test]
fn applying_a_theme_switches_a_per_key_animation_off() {
    let fx = Fx::new();
    fx.api.set_keyboard_fx(&AnimationRequest { mode: "rainbow".into(), speed: 5, keys: None }).unwrap();
    assert_eq!(fx.api.services().fx.get().mode, "rainbow");
    fx.api.apply_theme(&spec(json!({"name": "hearth"}))).unwrap();
    assert_eq!(fx.api.services().fx.get().mode, "off");
}

#[test]
fn starting_an_animation_ends_the_running_theme() {
    let fx = Fx::new();
    fx.api.apply_theme(&spec(json!({"name": "hearth"}))).unwrap();
    fx.api.set_keyboard_fx(&AnimationRequest { mode: "rainbow".into(), speed: 5, keys: None }).unwrap();
    assert!(fx.api.theme_status().active.is_none());
}

#[test]
fn a_look_with_a_theme_part_starts_it_and_one_without_ends_it() {
    let fx = Fx::new();
    let themed: Look = serde_json::from_value(json!({
        "keyboard": {"mode": "effect", "name": "static", "color": "112233", "brightness": 30},
        "theme": {"name": "hearth", "speed": 60}
    }))
    .unwrap();
    fx.api.save_look_value("fire", &themed).unwrap();
    let done = fx.api.apply_look("fire").unwrap();
    assert_eq!(done.last().map(String::as_str), Some("theme: hearth"));
    assert_eq!(fx.api.theme_status().active.unwrap().spec.speed, Some(60));
    // The native look has a keyboard part and no theme: it ends the theme.
    let done = fx.api.apply_look("native").unwrap();
    assert!(done.contains(&"theme: stopped".to_owned()), "{done:?}");
    assert!(fx.api.theme_status().active.is_none());
}

#[test]
fn a_look_whose_theme_cannot_run_is_refused_before_the_lights_change() {
    let fx = Fx::new();
    let broken: Look = serde_json::from_value(json!({
        "keyboard": {"mode": "effect", "name": "static", "color": "112233", "brightness": 30},
        "theme": {"name": "storm"}
    }))
    .unwrap();
    fx.api.save_look_value("stormy", &broken).unwrap();
    let before = fx.api.services().keyboard.last();
    let e = fx.api.apply_look("stormy").unwrap_err().to_string();
    assert!(e.contains("flashing has not been measured"), "{e}");
    assert_eq!(fx.api.services().keyboard.last(), before, "the keyboard was not touched");
    assert!(fx.api.theme_status().active.is_none());
}

#[test]
fn saving_the_current_lights_keeps_the_running_theme_in_the_look() {
    let fx = Fx::new();
    fx.api.apply_look("native").unwrap();
    fx.api.apply_theme(&spec(json!({"name": "hearth", "brightness": 40}))).unwrap();
    let saved = fx.api.save_look("with-theme").unwrap();
    assert_eq!(saved.theme.unwrap().brightness, Some(40));
    assert_eq!(fx.api.look("with-theme").unwrap().theme.unwrap().name, "hearth");
}

#[test]
fn without_a_home_only_the_built_in_themes_are_there() {
    let fx = Fx::build(|mut s| {
        s.config = crate::api::ConfigFiles::at(None, None);
        s
    });
    assert_eq!(fx.api.themes().themes.len(), 32);
    assert!(fx.api.save_theme(&mine("x")).unwrap_err().to_string().contains("HOME"));
    assert!(fx.api.apply_theme(&spec(json!({"name": "hearth"}))).is_ok());
}
