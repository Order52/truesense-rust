use serde_json::json;
use ts_domain::keymap::{self, LIT_CELLS};

use super::Fx;
use crate::api::{
    AnimationRequest, Api, ApiError, EffectRequest, KeyboardLook, LightingProfileAnswer, PaintItem,
};

fn no_daemon() -> Fx {
    Fx::offline()
}

#[test]
fn the_status_reports_the_remembered_look_and_the_levels() {
    let fx = no_daemon();
    let st = fx.api.keyboard_status().unwrap();
    assert_eq!(st.max, 50);
    assert_eq!(st.brightness, 25);
    let json = serde_json::to_value(&st).unwrap();
    assert_eq!(json["look"], json!({}), "nothing was put on the keyboard yet");
    assert_eq!(json["lighting_profiles"], json!([]));
    assert!(json.get("backlight_timeout").is_some());
    fx.api.set_keyboard_effect(&EffectRequest::new("wave")).unwrap();
    assert_eq!(fx.api.keyboard_status().unwrap().look.name, "wave");
}

#[test]
fn an_effect_without_a_brightness_keeps_the_keyboards_own_level() {
    let fx = no_daemon();
    fx.api.set_keyboard_effect(&EffectRequest::new("breathing")).unwrap();
    assert_eq!(fx.api.keyboard_last().brightness, 25);
    fx.api
        .set_keyboard_effect(&EffectRequest { brightness: Some(40), ..EffectRequest::new("static") })
        .unwrap();
    let last = fx.api.keyboard_last();
    assert_eq!((last.name.as_str(), last.brightness, last.speed), ("static", 40, Some(5)));
    assert_eq!(last.color.as_deref(), Some("00aec7"));
}

#[test]
fn neon_is_full_bright_and_a_dark_keyboard_gets_a_visible_effect() {
    let fx = no_daemon();
    fx.api.set_keyboard_effect(&EffectRequest::new("neon")).unwrap();
    assert_eq!(fx.api.keyboard_last().brightness, 50);
    fx.api.set_keyboard_brightness(0).unwrap();
    fx.api.set_keyboard_effect(&EffectRequest::new("wave")).unwrap();
    assert_eq!(fx.api.keyboard_last().brightness, 30);
    // A brightness of 0 given on purpose is dark.
    fx.api.set_keyboard_effect(&EffectRequest { brightness: Some(0), ..EffectRequest::new("wave") }).unwrap();
    assert_eq!(fx.api.keyboard_last().brightness, 0);
}

#[test]
fn bad_effect_values_are_refused_with_the_keyboards_words() {
    let fx = no_daemon();
    assert!(
        fx.api
            .set_keyboard_effect(&EffectRequest::new("sparkle"))
            .unwrap_err()
            .to_string()
            .starts_with("effect: off, static")
    );
    let e =
        fx.api.set_keyboard_effect(&EffectRequest { speed: 13, ..EffectRequest::new("wave") }).unwrap_err();
    assert_eq!(e.to_string(), "speed 1..12, brightness 0..50");
    assert!(
        fx.api
            .set_keyboard_effect(&EffectRequest { brightness: Some(51), ..EffectRequest::new("wave") })
            .is_err()
    );
    assert!(Api::keyboard_effects().contains(&"fireball"));
}

#[test]
fn colour_speed_and_brightness_change_the_running_look() {
    let fx = no_daemon();
    fx.api.set_keyboard_effect(&EffectRequest::new("breathing")).unwrap();
    fx.api.set_keyboard_color("ff0000").unwrap();
    fx.api.set_keyboard_speed(9).unwrap();
    fx.api.set_keyboard_brightness(10).unwrap();
    let last = fx.api.keyboard_last();
    assert_eq!((last.color.as_deref(), last.speed, last.brightness), (Some("ff0000"), Some(9), 10));
    assert_eq!(fx.api.keyboard_brightness().unwrap(), 10);
    assert!(fx.api.set_keyboard_speed(0).is_err());
    assert!(fx.api.set_keyboard_color("red").is_err());
    assert!(fx.api.set_keyboard_brightness(51).is_err());
}

#[test]
fn paint_keeps_the_other_keys_and_a_bad_name_uploads_nothing() {
    let fx = no_daemon();
    fx.api.fill_keyboard("101010", Some(20)).unwrap();
    let items = PaintItem::parse_all(&["w=ff0000", "arrows=00ff00"]).unwrap();
    assert_eq!(fx.api.paint_keys(&items, None).unwrap(), 2);
    let cells = fx.api.keyboard_cells();
    assert_eq!(cells.len(), 128);
    let w = fx.api.services().keymap.cells_for("w").unwrap();
    assert!(w.iter().all(|c| cells[*c] == "ff0000"));
    assert!(cells.iter().any(|c| c == "101010"), "the other keys keep their colour");
    let frames = fx.rig.hid.frames();
    let bad = PaintItem::parse_all(&["w=00ff00", "nosuchkey=ffffff"]).unwrap();
    assert!(fx.api.paint_keys(&bad, None).is_err());
    assert_eq!(fx.rig.hid.frames(), frames, "nothing was uploaded");
    assert!(fx.api.paint_keys(&PaintItem::parse_all(&["w=zzz"]).unwrap(), None).is_err());
}

#[test]
fn paint_items_need_a_colour() {
    for bad in ["w", "w=", "=ff0000x"] {
        if bad == "=ff0000x" {
            assert!(PaintItem::parse(bad).is_ok(), "the key name is checked when it is used");
        } else {
            assert_eq!(PaintItem::parse(bad).unwrap_err().to_string(), format!("'{bad}': use key=RRGGBB"));
        }
    }
    assert_eq!(
        PaintItem::parse("esc=00ff88").unwrap(),
        PaintItem { key: "esc".into(), color: "00ff88".into() }
    );
}

#[test]
fn key_colours_are_painted_over_a_base() {
    let fx = no_daemon();
    let items = PaintItem::parse_all(&["wasd=ff0000"]).unwrap();
    fx.api.set_key_colors(&items, "000010", Some(30)).unwrap();
    let cells = fx.api.keyboard_cells();
    assert_eq!(cells[0], "000010");
    let wasd = fx.api.services().keymap.cells_for("wasd").unwrap();
    assert!(wasd.iter().all(|c| cells[*c] == "ff0000"));
    assert!(fx.api.set_key_colors(&items, "xyz", None).is_err());
    assert!(
        fx.api.set_key_colors(&PaintItem::parse_all(&["bogus=ff0000"]).unwrap(), "000000", None).is_err()
    );
}

#[test]
fn fill_gradient_and_rainbow_make_the_expected_pictures() {
    let fx = no_daemon();
    fx.api.fill_keyboard("0a0b0c", None).unwrap();
    let cells = fx.api.keyboard_cells();
    assert!(cells[..LIT_CELLS].iter().all(|c| c == "0a0b0c"));
    assert_eq!(&cells[LIT_CELLS..], ["000000", "000000"]);
    fx.api.gradient_keyboard("000000", "ffffff", Some(20)).unwrap();
    let g = fx.api.keyboard_cells();
    assert_eq!(g[0], "000000");
    assert_eq!(
        g[LIT_CELLS - 1],
        "f3f3f3",
        "the last of the 21 columns is 20/21 of the way, as in the Python edition"
    );
    assert_eq!(g, keymap::gradient([0; 3], [255; 3]).iter().map(|c| keymap::hex(*c)).collect::<Vec<_>>());
    fx.api.rainbow_keyboard(None).unwrap();
    assert_eq!(
        fx.api.keyboard_cells(),
        keymap::rainbow().iter().map(|c| keymap::hex(*c)).collect::<Vec<_>>()
    );
    assert!(fx.api.fill_keyboard("nope", None).is_err());
    assert!(fx.api.gradient_keyboard("000000", "bad", None).is_err());
}

#[test]
fn a_picture_is_a_list_of_at_most_128_colours() {
    let fx = no_daemon();
    let mut cells = vec!["ff0000".to_owned(); 130];
    let e = fx.api.set_keyboard_cells(&cells, None).unwrap_err();
    assert_eq!(e.to_string(), "a picture has at most 128 cells, not 130");
    cells.truncate(3);
    cells[1] = "red".into();
    assert!(fx.api.set_keyboard_cells(&cells, None).unwrap_err().to_string().starts_with("cell 1: "));
    cells[1] = "00ff00".into();
    fx.api.set_keyboard_cells(&cells, Some(25)).unwrap();
    let now = fx.api.keyboard_cells();
    assert_eq!(&now[..3], ["ff0000", "00ff00", "ff0000"]);
    assert_eq!(now[3], "000000", "the rest is black");
}

#[test]
fn a_picture_file_needs_a_decoder() {
    let fx = no_daemon();
    let e = fx.api.image_keyboard(std::path::Path::new("/nonexistent.png"), None).unwrap_err();
    assert!(matches!(e, ApiError::Unavailable(_)), "{e}");
}

#[test]
fn designs_are_listed_applied_and_saved() {
    let fx = no_daemon();
    let list = fx.api.designs();
    assert!(!list.0.is_empty());
    let (name, first) = &list.0[0];
    assert_eq!(first.source, "built-in");
    fx.api.apply_design(name, Some(20)).unwrap();
    let shown = fx.api.show_design(name).unwrap();
    assert_eq!(shown.cells.len(), 128);
    assert_eq!(fx.api.keyboard_cells(), shown.cells);
    let file = fx.api.save_design("mine").unwrap();
    assert!(file.exists());
    let after = fx.api.designs();
    assert!(after.0.iter().any(|(n, d)| n == "mine" && d.source == "yours"));
    assert_eq!(fx.api.show_design("mine").unwrap().cells[..LIT_CELLS], shown.cells[..LIT_CELLS]);
    assert_eq!(fx.api.apply_design("", None).unwrap_err().to_string(), "give a design name");
    assert!(fx.api.apply_design("nosuch", None).unwrap_err().to_string().contains("no design 'nosuch'"));
    assert!(fx.api.save_design("a/b").is_err());
    let own = vec!["123456".to_owned(); 5];
    let saved = fx.api.save_design_cells("blue", Some(&own)).unwrap();
    assert!(saved.exists());
    assert_eq!(fx.api.save_design_cells("", None).unwrap_err().to_string(), "give a design name");
    assert!(fx.api.save_design_cells("again", None).is_ok());
}

#[test]
fn animations_are_recorded_with_the_picture_they_start_from() {
    let fx = no_daemon();
    let req = AnimationRequest { mode: "wave".into(), speed: 7, keys: Some("wasd, arrows".into()) };
    assert_eq!(
        req.to_string(),
        "per-key animation: wave on wasd, arrows (needs the key service: systemctl --user status truesense-keys)"
    );
    let st = fx.api.set_keyboard_fx(&req).unwrap();
    assert_eq!((st.mode.as_str(), st.speed), ("wave", 7));
    let keys = st.keys.unwrap();
    assert!(keys.windows(2).all(|w| w[0] < w[1]), "sorted, no duplicates");
    assert!(keys.len() > 4);
    assert_eq!(st.base.len(), 128);
    assert!(fx.rig.hid.frames() > 0, "the picture is uploaded first");
    let st = fx.api.set_keyboard_fx(&AnimationRequest { mode: "off".into(), speed: 0, keys: None }).unwrap();
    assert_eq!((st.mode.as_str(), st.speed), ("off", 1));
    assert_eq!(
        AnimationRequest { mode: "off".into(), speed: 5, keys: None }.to_string(),
        "per-key animation: off"
    );
}

#[test]
fn animations_refuse_unknown_modes_speeds_and_keys() {
    let fx = no_daemon();
    let bad = |mode: &str, speed: i64, keys: Option<&str>| {
        fx.api.set_keyboard_fx(&AnimationRequest { mode: mode.into(), speed, keys: keys.map(str::to_owned) })
    };
    assert!(bad("explode", 5, None).unwrap_err().to_string().starts_with("animation: off, "));
    assert_eq!(bad("wave", 11, None).unwrap_err().to_string(), "speed 1..10");
    assert!(bad("wave", 5, Some("nosuchkey")).is_err());
    assert!(
        fx.api.set_keyboard_fx_cells("wave", 5, &[200], None).unwrap_err().to_string().contains("0..127")
    );
    let base = vec!["ff0000".to_owned(); 128];
    let st = fx.api.set_keyboard_fx_cells("pulse", 5, &[], Some(&base)).unwrap();
    assert_eq!(st.base[0], "ff0000");
}

#[test]
fn lighting_profiles_are_saved_listed_applied_and_deleted() {
    let fx = no_daemon();
    let msg = fx.api.lighting_profile("save", Some("red"), &["fill", "ff0000"]).unwrap();
    assert_eq!(msg, LightingProfileAnswer::Message("saved lighting profile 'red'".into()));
    assert_eq!(fx.api.lighting_profile_names(), ["red"]);
    assert_eq!(fx.api.lighting_profile("list", None, &[]).unwrap().to_string(), "red");
    fx.api.apply_lighting_profile("red").unwrap();
    assert_eq!(fx.api.keyboard_cells()[0], "ff0000");
    assert_eq!(
        fx.api.lighting_profile("load", Some("red"), &[]).unwrap().to_string(),
        "loaded lighting profile 'red'"
    );
    assert!(fx.api.lighting_profile("save", Some("bad"), &["fill"]).is_err());
    assert!(fx.api.lighting_profile("save", None, &[]).is_err());
    assert!(fx.api.apply_lighting_profile("nosuch").is_err());
    let message = fx.api.save_lighting_profile_cells("now", None).unwrap();
    assert_eq!(message, "saved lighting profile 'now'");
    assert_eq!(
        fx.api.lighting_profile("delete", Some("red"), &[]).unwrap().to_string(),
        "deleted lighting profile 'red'"
    );
    assert_eq!(
        fx.api.lighting_profile("list", None, &[]).unwrap(),
        LightingProfileAnswer::Names(vec!["now".into()])
    );
    let empty = Fx::offline();
    assert_eq!(empty.api.lighting_profile("list", None, &[]).unwrap().to_string(), "(no lighting profiles)");
}

#[test]
fn the_light_is_switched_by_the_same_actions_as_the_key() {
    let fx = no_daemon();
    fx.rig.actions.answer("keyboard:toggle", Ok("Keyboard light: off"));
    assert_eq!(fx.api.keyboard_power("toggle").unwrap(), "Keyboard light: off");
    fx.api.keyboard_power("on").unwrap();
    assert_eq!(fx.rig.actions.calls(), ["keyboard:toggle", "keyboard:on"]);
    assert!(fx.api.keyboard_power("dim").is_err());
}

#[test]
fn off_and_on_remember_the_light_and_keep_the_look() {
    let fx = no_daemon();
    fx.api.set_keyboard_effect(&EffectRequest::new("wave")).unwrap();
    fx.api
        .apply_keyboard_look(&KeyboardLook {
            name: "off".into(),
            color: None,
            speed: None,
            brightness: 0,
            direction: 0,
        })
        .unwrap();
    assert_eq!(fx.rig.hid.brightness(), 0);
    assert_eq!(fx.api.keyboard_last().name, "off");
    let back = KeyboardLook {
        name: "breathing".into(),
        color: Some("00ff00".into()),
        speed: Some(4),
        brightness: 20,
        direction: 0,
    };
    fx.api.apply_keyboard_look(&back).unwrap();
    let last = fx.api.keyboard_last();
    assert_eq!(
        (last.name.as_str(), last.color.as_deref(), last.speed),
        ("breathing", Some("00ff00"), Some(4))
    );
    // A look with nothing known uses the keyboard's defaults.
    fx.api
        .apply_keyboard_look(&KeyboardLook {
            name: "static".into(),
            color: None,
            speed: None,
            brightness: 10,
            direction: 0,
        })
        .unwrap();
    assert_eq!(fx.api.keyboard_last().color.as_deref(), Some("ffffff"));
}

#[test]
fn the_stock_look_comes_back() {
    let fx = no_daemon();
    fx.api.fill_keyboard("ff00ff", Some(30)).unwrap();
    fx.api.restore_keyboard().unwrap();
    let cells = fx.api.keyboard_cells();
    assert_eq!(cells[0], "00aec7");
    assert_eq!(cells[127], "000000");
}

#[test]
fn key_names_and_layout_agree() {
    let fx = no_daemon();
    let names = fx.api.key_names();
    assert!(names.keys.windows(2).all(|w| w[0] <= w[1]));
    assert!(names.keys.contains(&"esc".to_owned()) && names.groups.contains(&"wasd".to_owned()));
    let layout = fx.api.key_layout();
    assert_eq!(layout.groups, names.groups);
    assert_eq!(layout.keys.0.len(), names.keys.len());
    let json = serde_json::to_value(&layout).unwrap();
    let (_, cells) = &layout.keys.0[0];
    assert_eq!(cells[0].col, cells[0].cell / 6);
    assert_eq!(cells[0].row, cells[0].cell % 6);
    assert!(json["keys"].is_object() && json["groups"].is_array());
}

#[test]
fn the_wave_directions_are_listed_and_parsed_and_reach_the_keyboard() {
    use crate::api::{parse_direction, wave_directions};
    let list = wave_directions();
    let ids: Vec<&str> = list.iter().map(|d| d.id).collect();
    assert_eq!(
        ids,
        [
            "left-to-right",
            "right-to-left",
            "bottom-to-top",
            "top-to-bottom",
            "clockwise",
            "counter-clockwise"
        ]
    );
    assert_eq!(list.iter().map(|d| d.wire).collect::<Vec<_>>(), [1, 2, 3, 4, 5, 6]);
    assert_eq!(parse_direction("clockwise").unwrap(), 4);
    assert_eq!(parse_direction("ccw").unwrap(), 5);
    assert_eq!(parse_direction("3").unwrap(), 3);
    assert!(parse_direction("sideways").unwrap_err().to_string().contains("0..5"));
    assert_eq!(parse_direction("200").unwrap(), 200, "numbers go on: the effect decides");
    let fx = no_daemon();
    fx.api.set_keyboard_effect(&EffectRequest { direction: 4, ..EffectRequest::new("wave") }).unwrap();
    assert_eq!(fx.api.keyboard_last().direction, 4);
    let err = fx
        .api
        .set_keyboard_effect(&EffectRequest { direction: 6, ..EffectRequest::new("wave") })
        .unwrap_err();
    assert!(err.to_string().starts_with("wave direction 6"), "{err}");
}

#[test]
fn a_hardware_effect_switches_a_running_per_key_animation_off() {
    // The bug: a per-key animation kept drawing over Wave picked on the Keyboard tab.
    let fx = no_daemon();
    fx.api.set_keyboard_fx(&AnimationRequest { mode: "pulse".into(), speed: 5, keys: None }).unwrap();
    assert_eq!(fx.api.services().fx.get().mode, "pulse");
    fx.api.set_keyboard_effect(&EffectRequest::new("wave")).unwrap();
    assert_eq!(fx.api.services().fx.get().mode, "off");
    assert_eq!(fx.api.keyboard_last().name, "wave");
}
