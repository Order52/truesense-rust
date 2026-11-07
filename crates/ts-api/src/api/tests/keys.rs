use ts_domain::keybinds::Gesture;

use super::Fx;
use crate::api::{Api, KeyProfileEdit};

#[test]
fn the_bindings_are_the_defaults_until_you_change_them() {
    let fx = Fx::new();
    let k = fx.api.key_settings().unwrap();
    assert_eq!(k.bindings.tap, "power-next");
    assert_eq!(k.bindings.long, "fan-max-toggle");
    assert_eq!((k.bindings.long_ms, k.bindings.multi_ms), (800, 450));
    assert!(!k.power_cycle.is_empty());
    let text = k.to_string();
    assert!(text.starts_with("tap      power-next\n"), "{text}");
    assert!(text.ends_with("(long press after 800 ms; taps within 450 ms count together)"), "{text}");
    let json = serde_json::to_value(&k).unwrap();
    assert_eq!(json["fn_esc"], serde_json::json!("none"));
    assert!(json.get("power_cycle").is_some() && json.get("profile").is_some());
}

#[test]
fn an_action_is_checked_when_it_is_bound_and_the_profile_becomes_custom() {
    let fx = Fx::new();
    fx.api.set_key_action(Gesture::Tap, "power:turbo").unwrap();
    let k = fx.api.key_settings().unwrap();
    assert_eq!(k.bindings.tap, "power:turbo");
    assert_eq!(k.profile, "custom");
    let e = fx.api.set_key_action(Gesture::Double, "no-such-action").unwrap_err();
    assert!(e.to_string().ends_with("   (truesense keys actions)"), "{e}");
    assert_eq!(
        fx.api.key_settings().unwrap().bindings.double,
        "output-toggle",
        "a refused action changes nothing"
    );
    assert!(Api::key_actions().starts_with("An action is `name` or `name:argument`."));
    assert_eq!(Api::parse_gesture("fn_esc").unwrap(), Gesture::FnEsc);
    assert!(Api::parse_gesture("shout").is_err());
}

#[test]
fn reset_puts_the_defaults_back_on_one_gesture_or_all() {
    let fx = Fx::new();
    fx.api.set_key_action(Gesture::Tap, "none").unwrap();
    fx.api.set_key_action(Gesture::Long, "none").unwrap();
    assert_eq!(fx.api.reset_keys(Some(Gesture::Tap)).unwrap(), [Gesture::Tap]);
    let k = fx.api.key_settings().unwrap();
    assert_eq!((k.bindings.tap.as_str(), k.bindings.long.as_str()), ("power-next", "none"));
    assert_eq!(fx.api.reset_keys(None).unwrap().len(), 5);
    assert_eq!(fx.api.key_settings().unwrap().bindings.long, "fan-max-toggle");
}

#[test]
fn the_timing_is_range_checked_and_zero_is_not_a_way_to_skip() {
    let fx = Fx::new();
    let t = fx.api.set_key_timing(Some(900), None).unwrap();
    assert_eq!((t.long_ms, t.multi_ms), (900, 450));
    assert_eq!(fx.api.set_key_timing(None, Some(300)).unwrap().multi_ms, 300);
    assert_eq!(fx.api.set_key_timing(Some(299), None).unwrap_err().to_string(), "long_ms is 300..2000");
    assert_eq!(fx.api.set_key_timing(None, Some(1001)).unwrap_err().to_string(), "multi_ms is 200..1000");
    assert!(fx.api.set_key_timing(Some(0), None).is_err());
    assert_eq!(fx.api.key_timing().unwrap().long_ms, 900);
    assert_eq!(fx.api.set_key_timing(None, None).unwrap().long_ms, 900);
}

#[test]
fn the_action_test_runs_the_same_code_as_the_key() {
    let fx = Fx::new();
    fx.rig.actions.answer("power-next", Ok("Power mode: Quiet"));
    assert_eq!(fx.api.test_action("power-next").unwrap(), "Power mode: Quiet");
    fx.rig.actions.answer("fan:oops", Err("unknown action"));
    assert_eq!(fx.api.test_action("fan:oops").unwrap_err().to_string(), "unknown action");
}

#[test]
fn profiles_list_with_the_one_in_use_and_their_descriptions() {
    let fx = Fx::new();
    let list = fx.api.key_profiles().unwrap();
    assert_eq!(list.current.as_deref(), Some("default"));
    assert!(list.names.contains(&"gaming".to_owned()) && list.names[0] == "native");
    assert!(list.to_string().contains("* default  - "));
    let (name, view) = fx.api.key_profile_in_use().unwrap();
    assert_eq!(name, "default");
    assert_eq!(view.bindings.tap, "power-next");
}

#[test]
fn using_a_profile_writes_its_bindings_and_its_name() {
    let fx = Fx::new();
    fx.api.use_key_profile("gaming").unwrap();
    let k = fx.api.key_settings().unwrap();
    assert_eq!(k.profile, "gaming");
    let gaming = fx.api.key_profile("gaming").unwrap();
    assert_eq!(k.bindings, gaming.bindings);
    assert_eq!(fx.api.key_profiles().unwrap().current.as_deref(), Some("gaming"));
    assert!(fx.user_config().contains("profile = \"gaming\""));
    assert_eq!(
        fx.api.use_key_profile("").unwrap_err().to_string(),
        "which profile? (truesense keys profile list)"
    );
    assert!(fx.api.use_key_profile("nosuch").is_err());
}

#[test]
fn saving_starts_from_a_base_and_applies_the_edit() {
    let fx = Fx::new();
    let edit = KeyProfileEdit {
        from: Some("gaming".into()),
        actions: vec![(Gesture::Triple, "rear:toggle".into())],
        long_ms: Some(1000),
        multi_ms: None,
    };
    let saved = fx.api.save_key_profile("mine", &edit).unwrap();
    assert_eq!(saved.bindings.triple, "rear:toggle");
    assert_eq!(saved.bindings.long_ms, 1000);
    assert_eq!(saved.bindings.tap, fx.api.key_profile("gaming").unwrap().bindings.tap);
    // Editing your own profile by name keeps the rest of it.
    let again = fx
        .api
        .save_key_profile(
            "mine",
            &KeyProfileEdit { actions: vec![(Gesture::Tap, "none".into())], ..KeyProfileEdit::default() },
        )
        .unwrap();
    assert_eq!((again.bindings.tap.as_str(), again.bindings.triple.as_str()), ("none", "rear:toggle"));
    // A new name without a base starts from the defaults.
    let fresh = fx.api.save_key_profile("fresh", &KeyProfileEdit::default()).unwrap();
    assert_eq!(fresh.bindings.tap, "power-next");
    assert_eq!(fx.api.copy_key_profile("mine", None).unwrap(), "mine-copy");
    assert_eq!(fx.api.copy_key_profile("", None).unwrap_err().to_string(), "copy which profile?");
    fx.api.delete_key_profile("mine-copy").unwrap();
    assert!(fx.api.key_profiles().unwrap().names.contains(&"mine".to_owned()));
}

#[test]
fn a_bad_action_a_bad_name_or_a_built_in_is_refused() {
    let fx = Fx::new();
    let bad = KeyProfileEdit { actions: vec![(Gesture::Tap, "bogus".into())], ..KeyProfileEdit::default() };
    assert!(fx.api.save_key_profile("x", &bad).is_err());
    assert_eq!(
        fx.api.save_key_profile("", &KeyProfileEdit::default()).unwrap_err().to_string(),
        "save under which name?"
    );
    assert!(
        fx.api.save_key_profile("gaming", &KeyProfileEdit::default()).is_err(),
        "built-in profiles are read-only"
    );
    assert!(fx.api.delete_key_profile("gaming").is_err());
    assert!(fx.api.save_key_profile("../x", &KeyProfileEdit::default()).is_err());
    let too_long = KeyProfileEdit { long_ms: Some(10), ..KeyProfileEdit::default() };
    assert!(fx.api.save_key_profile("x", &too_long).is_err());
}
