use serde_json::{Value, json};
use ts_decision::curves::{self, Curve};
use ts_domain::looks::Look;

use super::Fx;

fn look(color: &str) -> Look {
    serde_json::from_value(json!({
        "keyboard": {"mode": "effect", "name": "wave", "color": color, "speed": 3, "brightness": 30},
        "rear": {"mode": "static", "color": color, "speed": 3, "brightness": 50}
    }))
    .unwrap()
}

fn answer(mode: Option<&str>, bindings: Value) -> Value {
    json!({"mode": mode, "bindings": bindings})
}

fn bound(name: &str, data: Value) -> Value {
    json!({"name": name, "data": data})
}

#[test]
fn binding_sends_validated_copies_of_the_saved_look_and_the_curve() {
    let fx = Fx::new();
    fx.api.save_look_value("red", &look("ff0000")).unwrap();
    let copy = serde_json::to_value(look("ff0000")).unwrap();
    let curve = Curve::builtin("performance").unwrap().to_value();
    fx.rig.daemon.answer_ok(
        "mode_bindings",
        answer(Some("eco"), json!({"turbo": {"look": bound("red", copy.clone()), "fan_curve": bound("performance", curve.clone())}})),
    );
    let shown = fx.api.bind_mode("turbo", Some("red"), Some("performance")).unwrap();
    let args = fx.rig.daemon.last_args("mode_bindings").unwrap();
    assert_eq!(args.get("action"), Some(&json!("set")));
    assert_eq!(args.get("mode"), Some(&json!("turbo")));
    assert_eq!(args.get("look"), Some(&bound("red", copy)));
    assert_eq!(args.get("fan_curve"), Some(&bound("performance", curve)));
    assert_eq!(shown.to_string(), "  turbo: look red, fan curve performance");
}

#[test]
fn one_part_only_sends_one_part() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok("mode_bindings", answer(None, json!({})));
    fx.api.bind_mode("quiet", None, Some("quiet")).unwrap();
    let args = fx.rig.daemon.last_args("mode_bindings").unwrap();
    assert!(args.get("look").is_none());
    assert!(args.get("fan_curve").is_some());
}

#[test]
fn a_bad_binding_is_refused_before_anything_is_sent() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok("mode_bindings", answer(None, json!({})));
    let cases: [(&str, Option<&str>, Option<&str>, &str); 5] = [
        ("warp", Some("native"), None, "unknown power mode 'warp'"),
        ("turbo", None, None, "a binding needs a look and/or a fan curve"),
        ("turbo", Some("nosuch"), None, "no look 'nosuch'"),
        ("turbo", None, Some("nosuch"), "nosuch"),
        ("turbo", None, Some("native"), "native curve cannot be bound"),
    ];
    for (mode, look, curve, text) in cases {
        let e = fx.api.bind_mode(mode, look, curve).unwrap_err().to_string();
        assert!(e.contains(text), "{mode} {look:?} {curve:?}: {e}");
    }
    assert!(fx.rig.daemon.last_args("mode_bindings").is_none(), "the service was never asked");
}

#[test]
fn a_look_that_changes_nothing_cannot_be_bound() {
    let fx = Fx::new();
    fx.api.save_look_value("empty", &Look::default()).unwrap();
    let e = fx.api.bind_mode("turbo", Some("empty"), None).unwrap_err().to_string();
    assert!(e.contains("a look needs keyboard and/or rear settings"), "{e}");
}

#[test]
fn without_the_service_binding_says_how_to_start_it() {
    let fx = Fx::offline();
    let e = fx.api.bind_mode("turbo", Some("native"), None).unwrap_err();
    assert!(e.daemon_missing(), "{e}");
    assert!(fx.api.mode_bindings().unwrap_err().daemon_missing());
}

#[test]
fn an_old_service_without_the_command_is_recognised() {
    let fx = Fx::new();
    let e = fx.api.mode_bindings().unwrap_err();
    assert!(e.to_string().contains("older than this app"), "{e}");
}

#[test]
fn unbind_checks_the_mode_and_the_part() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok("mode_bindings", answer(None, json!({})));
    fx.api.unbind_mode("turbo", "").unwrap();
    assert_eq!(fx.rig.daemon.last_args("mode_bindings").unwrap().get("what"), Some(&json!("all")));
    fx.api.unbind_mode("eco", "fan_curve").unwrap();
    assert_eq!(fx.rig.daemon.last_args("mode_bindings").unwrap().get("what"), Some(&json!("fan_curve")));
    assert!(fx.api.unbind_mode("warp", "all").is_err());
    let e = fx.api.unbind_mode("eco", "both").unwrap_err().to_string();
    assert_eq!(e, "what: look | fan_curve | all");
}

#[test]
fn the_bindings_read_back_with_the_current_mode_marked() {
    let fx = Fx::new();
    let data = serde_json::to_value(look("00ff00")).unwrap();
    fx.rig.daemon.answer_ok(
        "mode_bindings",
        answer(Some("turbo"), json!({"turbo": {"look": bound("green", data.clone()), "fan_curve": null}, "eco": {"look": null, "fan_curve": bound("quiet", Curve::builtin("quiet").unwrap().to_value())}})),
    );
    let got = fx.api.mode_bindings().unwrap();
    assert_eq!(got.mode.as_deref(), Some("turbo"));
    assert_eq!(got.to_string(), "  eco: look -, fan curve quiet\n* turbo: look green, fan curve -");
    assert_eq!(serde_json::to_value(&got).unwrap()["bindings"]["turbo"]["look"]["name"], json!("green"));
    fx.rig.daemon.answer_ok("mode_bindings", answer(None, json!({})));
    assert_eq!(
        fx.api.mode_bindings().unwrap().to_string(),
        "no power mode is bound to a look or a fan curve"
    );
}

#[test]
fn applying_asks_for_the_current_or_a_named_mode_and_reports_every_part() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok(
        "mode_bindings",
        json!({"mode": "turbo", "results": [
            {"target": "look", "name": "red", "status": "applied", "detail": "keyboard: wave"},
            {"target": "fan_curve", "name": "loud", "status": "skipped", "detail": "a manual fan setting is held"}
        ]}),
    );
    let done = fx.api.apply_mode_binding(None).unwrap();
    assert!(fx.rig.daemon.last_args("mode_bindings").unwrap().get("mode").is_none());
    assert_eq!(
        done.to_string(),
        "turbo look red: applied (keyboard: wave)\nturbo fan curve loud: skipped (a manual fan setting is held)"
    );
    fx.api.apply_mode_binding(Some("eco")).unwrap();
    assert_eq!(fx.rig.daemon.last_args("mode_bindings").unwrap().get("mode"), Some(&json!("eco")));
    assert!(fx.api.apply_mode_binding(Some("warp")).is_err());
    fx.rig.daemon.answer_ok("mode_bindings", json!({"mode": "eco", "results": []}));
    assert_eq!(fx.api.apply_mode_binding(None).unwrap().to_string(), "eco has no binding");
}

#[test]
fn saving_a_look_sends_the_new_data_so_the_copy_is_not_stale() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok("mode_bindings", json!({"mode": null, "bindings": {}, "updated": 2}));
    fx.api.save_look_value("red", &look("aa0000")).unwrap();
    let args = fx.rig.daemon.last_args("mode_bindings").unwrap();
    assert_eq!(args.get("action"), Some(&json!("refresh")));
    assert_eq!(args.get("kind"), Some(&json!("look")));
    assert_eq!(args.get("name"), Some(&json!("red")));
    assert_eq!(args.get("data"), Some(&serde_json::to_value(look("aa0000")).unwrap()));
    // The saved file and the data sent agree.
    assert_eq!(fx.api.look("red").unwrap(), look("aa0000"));
    assert_eq!(fx.api.refresh_binding_copies(ts_decision::bindings::Target::Look, "red").unwrap(), 2);
}

#[test]
fn saving_a_curve_sends_the_new_data_too() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok("mode_bindings", json!({"mode": null, "bindings": {}, "updated": 1}));
    let mut curve = Curve::builtin("quiet").unwrap();
    curve.hysteresis = 6.0;
    fx.api.save_fan_curve_value("mine", &curve).unwrap();
    let args = fx.rig.daemon.last_args("mode_bindings").unwrap();
    assert_eq!(args.get("kind"), Some(&json!("fan_curve")));
    assert_eq!(args.get("name"), Some(&json!("mine")));
    let sent = curves::validate(args.get("data").unwrap()).unwrap();
    assert_eq!(sent.hysteresis, 6.0);
}

#[test]
fn a_save_never_fails_because_of_the_bindings() {
    // No service: nothing is sent. An old service: the unknown command is ignored. A service that
    // errors: the save still succeeded.
    let offline = Fx::offline();
    offline.api.save_look_value("red", &look("ff0000")).unwrap();
    assert!(offline.rig.daemon.calls().is_empty());
    let old = Fx::new();
    old.api.save_look_value("red", &look("ff0000")).unwrap();
    old.api.save_fan_curve_value("mine", &Curve::builtin("quiet").unwrap()).unwrap();
    let broken = Fx::new();
    broken.rig.daemon.answer("mode_bindings", crate::api::testing::Reply::Error("disk full".to_owned()));
    broken.api.save_look_value("red", &look("ff0000")).unwrap();
    assert_eq!(broken.api.look("red").unwrap(), look("ff0000"));
}
