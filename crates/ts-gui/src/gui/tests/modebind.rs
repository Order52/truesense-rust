//! The `mode` commands of the bus: the arguments the window sends, what reaches the service, and
//! the refusals.
#![allow(clippy::unwrap_used)]

use serde_json::{Value, json};

use super::harness::Machine;

fn asked(m: &Machine) -> Value {
    m.rig
        .daemon
        .last_args("mode_bindings")
        .map_or(Value::Null, |a| serde_json::to_value(a).unwrap_or(Value::Null))
}

fn empty() -> Value {
    json!({"mode": "balanced", "bindings": {}})
}

#[test]
fn bindings_shows_what_the_service_holds() {
    let m = Machine::start();
    m.rig.daemon.answer_ok("mode_bindings", empty());
    assert_eq!(m.ok("mode", "bindings", &json!({})), empty());
    assert_eq!(asked(&m)["action"], json!("get"));
}

#[test]
fn bind_sends_copies_of_the_named_look_and_curve() {
    let m = Machine::start();
    m.rig.daemon.answer_ok("mode_bindings", empty());
    m.ok("mode", "bind", &json!({"mode": "turbo", "look": "native", "curve": "performance"}));
    let args = asked(&m);
    assert_eq!((&args["action"], &args["mode"]), (&json!("set"), &json!("turbo")));
    assert_eq!(args["look"]["name"], json!("native"));
    assert!(args["look"]["data"]["keyboard"].is_object(), "the data travels with the request: {args}");
    assert_eq!(args["fan_curve"]["name"], json!("performance"));
    assert!(args["fan_curve"]["data"]["cpu"].is_array());
    // One part only.
    m.ok("mode", "bind", &json!({"mode": "quiet", "curve": "quiet"}));
    let args = asked(&m);
    assert!(args.get("look").is_none() && args["fan_curve"]["name"] == json!("quiet"), "{args}");
}

#[test]
fn bind_refuses_what_cannot_be_bound() {
    let m = Machine::start();
    m.rig.daemon.answer_ok("mode_bindings", empty());
    assert_eq!(m.refused("mode", "bind", &json!({"look": "native"})), "mode is needed");
    assert!(m.refused("mode", "bind", &json!({"mode": "turbo"})).contains("a look and/or a fan curve"));
    assert!(
        m.refused("mode", "bind", &json!({"mode": "warp", "look": "native"})).contains("unknown power mode")
    );
    assert!(
        m.refused("mode", "bind", &json!({"mode": "turbo", "look": "nosuch"})).contains("no look 'nosuch'")
    );
    assert!(m.refused("mode", "bind", &json!({"mode": "turbo", "curve": "native"})).contains("native curve"));
    assert!(m.refused("mode", "bind", &json!({"mode": "turbo", "look": 5})).contains("look: wrong kind"));
    assert_eq!(asked(&m), Value::Null, "nothing was sent");
}

#[test]
fn unbind_maps_the_words_of_the_window_to_the_services() {
    let m = Machine::start();
    m.rig.daemon.answer_ok("mode_bindings", empty());
    for (what, sent) in [
        (json!({}), "all"),
        (json!({"what": "all"}), "all"),
        (json!({"what": "look"}), "look"),
        (json!({"what": "curve"}), "fan_curve"),
    ] {
        let mut args = json!({"mode": "eco"});
        args.as_object_mut().unwrap().extend(what.as_object().unwrap().clone());
        m.ok("mode", "unbind", &args);
        let got = asked(&m);
        assert_eq!(
            (&got["action"], &got["what"], &got["mode"]),
            (&json!("clear"), &json!(sent), &json!("eco"))
        );
    }
    assert_eq!(
        m.refused("mode", "unbind", &json!({"mode": "eco", "what": "both"})),
        "what: look | curve | all"
    );
    assert_eq!(m.refused("mode", "unbind", &json!({})), "mode is needed");
}

#[test]
fn apply_reports_each_part() {
    let m = Machine::start();
    let reply = json!({"mode": "turbo", "results": [{"target": "look", "name": "red", "status": "applied", "detail": "keyboard: wave"}]});
    m.rig.daemon.answer_ok("mode_bindings", reply.clone());
    assert_eq!(m.ok("mode", "apply", &json!({})), reply);
    assert!(asked(&m).get("mode").is_none());
    m.ok("mode", "apply", &json!({"mode": "eco"}));
    assert_eq!(asked(&m)["mode"], json!("eco"));
    assert!(m.refused("mode", "apply", &json!({"mode": "warp"})).contains("unknown power mode"));
}

#[test]
fn without_the_service_the_page_is_told_to_start_it() {
    let m = Machine::start();
    m.rig.daemon.set_available(false);
    let (status, body) = m.run("mode", "bindings", &json!({}));
    assert_eq!(status, 422);
    assert_eq!(body["error"]["code"], json!("needs-daemon"), "{body}");
}
