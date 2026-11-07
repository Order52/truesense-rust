use serde_json::json;
use ts_platform::hw::{RearMode, Rgb};

use super::Fx;
use crate::api::{Api, RearPowerAction, RearRequest};

#[test]
fn the_light_bar_is_written_and_read_back() {
    let fx = Fx::offline();
    let req = RearRequest::parse("breathing", "00ff88", 4, 80, 0, None).unwrap();
    let rear = fx.api.set_rear(&req).unwrap();
    assert_eq!(rear.mode.to_string(), "breathing");
    assert_eq!((rear.speed, rear.brightness, rear.direction), (4, 80, Some(0)));
    assert_eq!(rear.color.to_string(), "00ff88");
    let read = fx.api.rear().unwrap();
    assert_eq!(read.brightness, 80);
    let json = serde_json::to_value(&read).unwrap();
    assert_eq!(json["mode"], json!("breathing"));
    assert_eq!(json["color"], json!("00ff88"));
    assert!(json.get("zones").is_none(), "the plain reading has no zones, as in the Python output");
}

#[test]
fn the_static_effect_can_colour_the_three_parts_and_the_window_gets_them_back() {
    let fx = Fx::offline();
    let req = RearRequest::parse("static", "ffffff", 3, 100, 1, Some("ff0000,00ff00,0000ff")).unwrap();
    fx.api.set_rear(&req).unwrap();
    let snap = fx.rig.sim.snapshot();
    assert_eq!(snap.zones.get(&1).map(String::as_str), Some("ff0000"));
    assert_eq!(snap.zones.get(&4).map(String::as_str), Some("0000ff"));
    let st = fx.api.rear_status().unwrap();
    assert_eq!(st.zones.as_ref().map(Vec::len), Some(3));
    let json = serde_json::to_value(&st).unwrap();
    assert_eq!(json["zones"], json!(["ff0000", "00ff00", "0000ff"]));
    assert_eq!(json["mode"], json!("static"));
}

#[test]
fn the_window_reading_has_null_zones_before_any_were_chosen() {
    let fx = Fx::offline();
    let json = serde_json::to_value(fx.api.rear_status().unwrap()).unwrap();
    assert_eq!(json["zones"], json!(null));
    assert!(json["raw"].is_string());
}

#[test]
fn one_colour_colours_all_three_parts_and_other_effects_refuse_zones() {
    let fx = Fx::offline();
    fx.api.set_rear(&RearRequest::parse("static", "ffffff", 3, 100, 1, Some("123456")).unwrap()).unwrap();
    assert_eq!(fx.rig.sim.snapshot().zones.get(&2).map(String::as_str), Some("123456"));
    let wave = RearRequest::parse("wave", "ffffff", 3, 100, 1, Some("ff0000,00ff00,0000ff")).unwrap();
    assert_eq!(
        fx.api.set_rear(&wave).unwrap_err().to_string(),
        "different colours per part only work with the static effect"
    );
    let two = RearRequest::parse("static", "ffffff", 3, 100, 1, Some("ff0000,00ff00")).unwrap();
    assert_eq!(
        fx.api.set_rear(&two).unwrap_err().to_string(),
        "zones: give one colour, or three (left middle right)"
    );
}

#[test]
fn the_numbers_are_checked_and_words_are_parsed() {
    let fx = Fx::offline();
    let before = fx.rig.sim.snapshot().rear;
    for (speed, brightness, direction) in [(0, 50, 1), (6, 50, 1), (3, 101, 1), (3, -1, 1), (3, 50, 2)] {
        let req = RearRequest { speed, brightness, direction, ..RearRequest::new(RearMode::Static) };
        assert_eq!(
            fx.api.set_rear(&req).unwrap_err().to_string(),
            "speed 1..5, brightness 0..100, direction 0|1"
        );
    }
    assert_eq!(fx.rig.sim.snapshot().rear, before);
    assert_eq!(
        RearRequest::parse("glow", "ffffff", 3, 100, 1, None).unwrap_err().to_string(),
        "rear mode: static, breathing, neon, wave, twinkling"
    );
    assert_eq!(
        RearRequest::parse("wave", "12345", 3, 100, 1, None).unwrap_err().to_string(),
        "color must be RRGGBB hex"
    );
    assert!(RearRequest::parse("static", "ffffff", 3, 100, 1, Some("ff0000,zz")).is_err());
    assert_eq!(Api::rear_effects(), ["static", "breathing", "neon", "wave", "twinkling"]);
    assert_eq!(RearRequest::new(RearMode::Neon).color, Rgb([0xff; 3]));
}

#[test]
fn a_live_write_does_not_wait_for_the_firmware_and_can_keep_the_part_colours() {
    let fx = Fx::offline();
    fx.api
        .set_rear(&RearRequest::parse("static", "ffffff", 3, 100, 1, Some("ff0000,00ff00,0000ff")).unwrap())
        .unwrap();
    let dim = RearRequest { brightness: 30, ..RearRequest::new(RearMode::Static) };
    let st = fx.api.set_rear_live(&dim, true).unwrap();
    assert_eq!(st.zones.unwrap().len(), 3);
    let snap = fx.rig.sim.snapshot();
    assert_eq!(
        snap.zones.get(&1).map(String::as_str),
        Some("ff0000"),
        "the parts were not flattened to white"
    );
    assert_eq!(snap.rear[3], 30);
    // Without keep_zones the bar is one colour again.
    fx.api.set_rear_live(&dim, false).unwrap();
    assert_eq!(fx.rig.sim.snapshot().zones.get(&1).map(String::as_str), Some("ffffff"));
    assert!(fx.api.set_rear_live(&RearRequest { speed: 9, ..dim }, false).is_err());
}

#[test]
fn on_off_and_toggle_are_the_key_actions() {
    let fx = Fx::offline();
    fx.rig.actions.answer("rear:toggle", Ok("Rear light: off"));
    assert_eq!(fx.api.rear_power(RearPowerAction::Toggle).unwrap(), "Rear light: off");
    fx.api.rear_power(RearPowerAction::On).unwrap();
    fx.api.rear_power(RearPowerAction::Off).unwrap();
    assert_eq!(fx.rig.actions.calls(), ["rear:toggle", "rear:on", "rear:off"]);
    assert_eq!("off".parse::<RearPowerAction>().unwrap(), RearPowerAction::Off);
    assert!("dim".parse::<RearPowerAction>().is_err());
}
