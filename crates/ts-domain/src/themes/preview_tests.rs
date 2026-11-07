#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! The gallery's pictures: deterministic, bounded, and different where the choices differ.

use std::sync::Arc;

use ts_foundation::userstate::Store;

use super::*;
use crate::keymap::testutil::TempDir;
use crate::keymap::{LIT_CELLS, NUM_CELLS};

fn store(dir: &TempDir) -> ThemeStore {
    ThemeStore::beside(Some(&dir.join("config.toml")), Arc::new(Store::new(dir.join("user.json"))))
}

fn lit(cells: &[crate::keymap::Color]) -> usize {
    cells.iter().take(LIT_CELLS).filter(|c| c.iter().any(|v| *v > 0)).count()
}

#[test]
fn every_built_in_theme_has_a_picture_of_128_cells_that_shows_something() {
    let dir = TempDir::new("preview-all");
    let store = store(&dir);
    let names: Vec<String> = ts_fx::themes::builtin().iter().map(|t| t.id.clone()).collect();
    assert_eq!(names.len(), 32);
    for name in names {
        let cells = preview(&store, &ThemeSpec::new(&name), None).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(cells.len(), NUM_CELLS, "{name}");
        assert!(lit(&cells) >= 3, "{name}: only {} lit keys", lit(&cells));
        assert_eq!(cells[126..], [[0; 3]; 2], "{name}: cells 126 and 127 are not keys");
    }
}

#[test]
fn a_picture_is_the_same_every_time_and_for_any_speed_or_brightness() {
    let dir = TempDir::new("preview-det");
    let store = store(&dir);
    for name in ["hearth", "aurora", "neon-rain", "storm", "synthwave"] {
        let a = preview(&store, &ThemeSpec::new(name), None).unwrap();
        let b = preview(&store, &ThemeSpec::new(name), None).unwrap();
        assert_eq!(a, b, "{name}");
        let slow = ThemeSpec { speed: Some(20), brightness: Some(30), ..ThemeSpec::new(name) };
        assert_eq!(
            preview(&store, &slow, None).unwrap(),
            a,
            "{name}: speed and brightness are not part of a picture"
        );
    }
}

#[test]
fn the_chosen_colourway_changes_the_picture() {
    let dir = TempDir::new("preview-ways");
    let store = store(&dir);
    let theme = ts_fx::themes::find("hearth").unwrap();
    assert!(theme.colourways.len() >= 2);
    let first = preview(&store, &ThemeSpec::new("hearth"), None).unwrap();
    let other = ThemeSpec { colourway: Some(ColourwayPick::Index(1)), ..ThemeSpec::new("hearth") };
    assert_ne!(preview(&store, &other, None).unwrap(), first);
    let explicit = ThemeSpec { colourway: Some(ColourwayPick::Index(0)), ..ThemeSpec::new("hearth") };
    assert_eq!(preview(&store, &explicit, None).unwrap(), first, "no choice is the first colourway");
    let custom = ThemeSpec {
        colourway: Some(ColourwayPick::CUSTOM),
        custom: vec!["#FF0000".to_owned(), "#00FF00".to_owned(), "#0000FF".to_owned(), "#FFFFFF".to_owned()],
        ..ThemeSpec::new("hearth")
    };
    assert_ne!(preview(&store, &custom, None).unwrap(), first);
}

#[test]
fn the_moment_matters_for_a_moving_theme_and_is_bounded() {
    let dir = TempDir::new("preview-t");
    let store = store(&dir);
    let spec = ThemeSpec::new("synthwave");
    let early = preview(&store, &spec, Some(0.5)).unwrap();
    let late = preview(&store, &spec, Some(7.0)).unwrap();
    assert_ne!(early, late);
    assert_eq!(preview(&store, &spec, Some(0.0)).unwrap().len(), NUM_CELLS);
    assert_eq!(preview(&store, &spec, Some(PREVIEW_MAX_T)).unwrap().len(), NUM_CELLS);
    for bad in [-0.1, PREVIEW_MAX_T + 0.1, f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e300] {
        assert!(preview(&store, &spec, Some(bad)).is_err(), "{bad}");
    }
}

#[test]
fn a_theme_that_cannot_be_started_can_still_be_pictured() {
    let dir = TempDir::new("preview-storm");
    let store = store(&dir);
    // Whether it runs is the key service's business (its flashing was never measured): refused to start,
    // listed as not runnable, but the picture is only a still.
    assert!(!store.list().themes.iter().find(|t| t.id == "storm").unwrap().runnable);
    assert!(preview(&store, &ThemeSpec::new("storm"), None).is_ok());
}

#[test]
fn hostile_requests_are_errors_not_panics() {
    let dir = TempDir::new("preview-bad");
    let store = store(&dir);
    for name in ["", "nosuch", "../etc/passwd", "HEARTH", "a b"] {
        assert!(preview(&store, &ThemeSpec::new(name), None).is_err(), "{name:?}");
    }
    let bad_way = ThemeSpec { colourway: Some(ColourwayPick::Index(99)), ..ThemeSpec::new("hearth") };
    assert!(preview(&store, &bad_way, None).is_err());
    let bad_custom = ThemeSpec {
        colourway: Some(ColourwayPick::CUSTOM),
        custom: vec!["red".to_owned()],
        ..ThemeSpec::new("hearth")
    };
    assert!(preview(&store, &bad_custom, None).is_err());
}

#[test]
fn a_users_theme_is_pictured_like_a_built_in_one() {
    let dir = TempDir::new("preview-user");
    let store = store(&dir);
    let mut v = serde_json::to_value(ts_fx::themes::find("hearth").unwrap()).unwrap();
    v["id"] = "mine".into();
    v["name"] = "Mine".into();
    v["top"] = 0.into();
    store.save_json(&v.to_string()).unwrap();
    assert_eq!(
        preview(&store, &ThemeSpec::new("mine"), None).unwrap(),
        preview(&store, &ThemeSpec::new("hearth"), None).unwrap()
    );
}
