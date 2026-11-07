//! `truesense mode ...`: the words of the command line, what reaches the service, the text and the
//! JSON a person or a script reads.

use serde_json::{Value, json};

use super::Fx;

fn reply(mode: &str, bindings: Value) -> Value {
    json!({"mode": mode, "bindings": bindings})
}

fn asked(fx: &Fx) -> Value {
    fx.daemon()
        .last_args("mode_bindings")
        .map_or(Value::Null, |a| serde_json::to_value(a).unwrap_or(Value::Null))
}

#[test]
fn bind_sends_the_look_and_the_curve_by_copy_and_prints_the_bindings() {
    let fx = Fx::new();
    fx.daemon().answer_ok(
        "mode_bindings",
        reply(
            "eco",
            json!({"turbo": {"look": {"name": "native", "data": {"keyboard": {"mode": "perkey"}}}, "fan_curve": {"name": "performance", "data": {}}}}),
        ),
    );
    let ran = fx.run("mode bind turbo --look native --curve performance");
    assert_eq!(ran.code, 0, "{}", ran.err);
    assert_eq!(ran.out, "  turbo: look native, fan curve performance\n");
    let args = asked(&fx);
    assert_eq!(args["action"], json!("set"));
    assert_eq!(args["mode"], json!("turbo"));
    assert_eq!(args["look"]["name"], json!("native"));
    assert!(args["look"]["data"]["keyboard"].is_object());
    assert_eq!(args["fan_curve"]["name"], json!("performance"));
    assert!(args["fan_curve"]["data"]["cpu"].is_array());
}

#[test]
fn bind_needs_a_look_or_a_curve_and_refuses_what_cannot_be_bound() {
    let fx = Fx::new();
    fx.daemon().answer_ok("mode_bindings", reply("eco", json!({})));
    let ran = fx.run("mode bind turbo");
    assert_eq!((ran.code, ran.err.as_str()), (1, "error: give --look NAME and/or --curve NAME\n"));
    let ran = fx.run("mode bind turbo --look nosuch");
    assert_eq!(ran.code, 1);
    assert!(ran.err.contains("no look 'nosuch'"), "{}", ran.err);
    let ran = fx.run("mode bind turbo --curve native");
    assert_eq!(ran.code, 1);
    assert!(ran.err.contains("native curve cannot be bound"), "{}", ran.err);
    let ran = fx.run("mode bind warp --look native");
    assert_eq!(ran.code, 2, "the parser refuses a mode that is not one of the five");
    assert!(ran.err.contains("invalid choice: 'warp'"), "{}", ran.err);
    assert!(fx.daemon().last_args("mode_bindings").is_none(), "nothing was sent");
}

#[test]
fn unbind_removes_a_part_or_all() {
    let fx = Fx::new();
    fx.daemon().answer_ok("mode_bindings", reply("eco", json!({})));
    for (line, what) in [
        ("mode unbind turbo", "all"),
        ("mode unbind turbo --look", "look"),
        ("mode unbind turbo --curve", "fan_curve"),
        ("mode unbind turbo --look --curve", "all"),
    ] {
        let ran = fx.run(line);
        assert_eq!(ran.code, 0, "{line}: {}", ran.err);
        assert_eq!(ran.out, "no power mode is bound to a look or a fan curve\n");
        assert_eq!(asked(&fx)["what"], json!(what), "{line}");
        assert_eq!(asked(&fx)["action"], json!("clear"));
    }
}

#[test]
fn bindings_prints_text_and_json() {
    let fx = Fx::new();
    let stored = json!({"eco": {"look": {"name": "dim", "data": {"keyboard": {"mode": "perkey"}}}, "fan_curve": null}});
    fx.daemon().answer_ok("mode_bindings", reply("eco", stored));
    let ran = fx.run("mode bindings");
    assert_eq!(ran.out, "* eco: look dim, fan curve -\n");
    let ran = fx.run("--json mode bindings");
    let json: Value = serde_json::from_str(&ran.out).unwrap();
    assert_eq!(json["mode"], json!("eco"));
    assert_eq!(json["bindings"]["eco"]["look"]["name"], json!("dim"));
    // The bare area is the same listing.
    assert_eq!(fx.run("mode").code, 0);
}

#[test]
fn apply_reports_each_part_and_takes_an_optional_mode() {
    let fx = Fx::new();
    fx.daemon().answer_ok(
        "mode_bindings",
        json!({"mode": "turbo", "results": [
            {"target": "look", "name": "red", "status": "applied", "detail": "keyboard: wave"},
            {"target": "fan_curve", "name": "loud", "status": "skipped", "detail": "a manual fan setting is held"}
        ]}),
    );
    let ran = fx.run("mode apply");
    assert_eq!(
        ran.out,
        "turbo look red: applied (keyboard: wave)\nturbo fan curve loud: skipped (a manual fan setting is held)\n"
    );
    assert!(asked(&fx).get("mode").is_none());
    fx.run("mode apply eco");
    assert_eq!(asked(&fx)["mode"], json!("eco"));
}

#[test]
fn without_the_service_or_with_an_old_one_the_error_says_so() {
    let fx = Fx::offline();
    let ran = fx.run("mode bindings");
    assert_eq!(ran.code, 1);
    assert!(ran.err.starts_with("error: "), "{}", ran.err);
    let old = Fx::new();
    let ran = old.run("mode bindings");
    assert_eq!(ran.code, 1);
    assert!(ran.err.contains("older than this app"), "{}", ran.err);
}
