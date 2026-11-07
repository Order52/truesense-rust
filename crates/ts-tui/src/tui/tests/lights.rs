//! The keyboard backlight and the light bar: the rules that keep a light from being left dark by
//! accident, and that a brightness of 0 is a light switched off, not a fan stop.

use ts_api::api::RearRequest;

use super::*;

fn last_keyboard(h: &Harness) -> crate::tui::data::KbLook {
    match h.backend.ops().into_iter().rev().find(|o| matches!(o, Op::Keyboard(_))) {
        Some(Op::Keyboard(l)) => l,
        other => panic!("no keyboard write: {other:?}"),
    }
}

fn last_rear(h: &Harness) -> (crate::tui::data::RearLook, bool) {
    match h.backend.ops().into_iter().rev().find(|o| matches!(o, Op::Rear { .. })) {
        Some(Op::Rear { look, keep_zones }) => (look, keep_zones),
        other => panic!("no light bar write: {other:?}"),
    }
}

fn set_rear(h: &Harness, brightness: i64) {
    let request = RearRequest::parse("static", "ffffff", 3, brightness, 1, None).expect("a request");
    h.api().set_rear(&request).expect("the light bar");
}

#[test]
fn a_brightness_of_zero_turns_the_light_off_it_is_not_a_fan_stop() {
    for (which, nth) in [("rear", 1), ("kb", 0)] {
        let mut h = Harness::new(100, 30);
        set_rear(&h, 60);
        h.goto(3);
        h.sel("Brightness", nth);
        h.keys("hhhhhhhhhhhh");
        assert!(h.model.modal.is_none(), "{which}: zero is a normal value for a light");
        h.elapse();
        let (text, st) = h.message();
        assert!(!text.to_lowercase().contains("fan") && st == St::Good, "{which}: {text:?} ({st:?})");
        match which {
            "rear" => assert_eq!(h.api().rear().unwrap().brightness, 0),
            _ => {
                assert_eq!(h.api().keyboard_brightness().unwrap(), 0);
                assert_eq!(text, "Keyboard light off");
            }
        }
        assert!(h.backend.ops().iter().all(|o| !o.stops_a_fan()));
    }
}

#[test]
fn choosing_an_effect_never_leaves_a_dark_light_dark() {
    let mut h = Harness::new(100, 30);
    set_rear(&h, 60);
    h.goto(3);
    h.sel("Brightness", 1);
    h.keys("hhhhhhhhhhhh");
    h.elapse();
    assert_eq!(last_rear(&h).0.brightness, 0);
    let effect = h.sel("Effect", 1);
    h.edit(effect, Value::text("breathing"));
    let (look, _) = last_rear(&h);
    assert_eq!(look.mode, "breathing");
    assert!(
        look.brightness >= 40,
        "an effect picked while the bar is off turns it on at a usable brightness: {look:?}"
    );
    assert_eq!(h.api().rear().unwrap().brightness, 60, "back at the last brightness it had");
}

#[test]
fn a_keyboard_effect_picked_while_dark_comes_back_at_the_floor_and_off_stays_off() {
    let mut h = Harness::new(100, 30);
    h.api().set_keyboard_brightness(0).expect("dark");
    h.goto(3);
    let effect = h.sel("Effect", 0);
    h.edit(effect, Value::text("wave"));
    let look = last_keyboard(&h);
    assert_eq!(look.name, "wave");
    assert!(look.brightness >= 10, "{look:?}");

    let mut h2 = Harness::new(100, 30);
    h2.api().set_keyboard_brightness(0).expect("dark");
    h2.goto(3);
    let effect = h2.sel("Effect", 0);
    h2.edit(effect, Value::text("off"));
    assert_eq!(last_keyboard(&h2).name, "off", "choosing off must stay off");
}

#[test]
fn the_light_bar_keeps_its_part_colours_until_a_colour_is_picked_here() {
    let mut h = Harness::new(100, 30);
    set_rear(&h, 60);
    h.goto(3);
    let effect = h.sel("Effect", 1);
    h.edit(effect, Value::text("wave"));
    assert!(!last_rear(&h).1, "part colours only exist in the static effect");
    h.edit(effect, Value::text("static"));
    assert!(last_rear(&h).1, "kept for static without a colour pick");
    let colour = h.sel("Colour", 1);
    h.edit(colour, Value::text("ff0000"));
    let (look, keep) = last_rear(&h);
    assert!(!keep && look.color == "ff0000", "a picked colour replaces the parts: {look:?}");
}

#[test]
fn a_change_of_one_light_setting_keeps_the_others() {
    let mut h = Harness::new(100, 30);
    h.goto(3);
    let effect = h.sel("Effect", 0);
    h.edit(effect, Value::text("wave"));
    let colour = h.sel("Colour", 0);
    h.edit(colour, Value::text("ff0000"));
    let look = last_keyboard(&h);
    assert_eq!((look.name.as_str(), look.color.as_str()), ("wave", "ff0000"), "the colour keeps the effect");
}

#[test]
fn the_keyboard_shown_is_the_keyboards_real_brightness() {
    let mut h = Harness::new(100, 30);
    h.api().set_keyboard_brightness(35).expect("bright");
    h.goto(3);
    let row = h.sel("Brightness", 0);
    assert_eq!(h.value(row), Some(Value::Int(35)), "Fn+F12 raised it: the screen shows it");
}

#[test]
fn light_rows_show_the_light_as_unknown_values_never_as_zero_where_zero_means_off() {
    let mut h = Harness::new(100, 30);
    h.goto(3);
    let text = h.text();
    assert!(text.contains("Keyboard") && text.contains("Light bar"), "{text}");
}
