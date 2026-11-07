//! Mode bindings through the real daemon on a temporary socket: bind with the application layer,
//! change the power mode the way a user does, and watch what the daemon applied.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;
use ts_decision::curves::Curve;
use ts_domain::looks::Look;
use ts_platform::hw::{Coded, PowerMode};

use super::rig::Rig;

fn look(color: &str) -> Look {
    serde_json::from_value(json!({
        "keyboard": {"mode": "effect", "name": "wave", "color": color, "speed": 3, "brightness": 30},
        "rear": {"mode": "static", "color": color, "speed": 3, "brightness": 50}
    }))
    .unwrap()
}

fn mode(rig: &Rig, name: &str) {
    rig.api.set_power(PowerMode::from_name(name).unwrap(), None).unwrap();
    rig.ticks(8);
}

#[test]
fn a_bound_look_and_curve_are_applied_when_the_mode_changes_and_not_otherwise() {
    let rig = Rig::new();
    rig.api.save_look_value("red", &look("ff0000")).unwrap();
    rig.api.save_fan_curve_value("loud", &Curve::builtin("performance").unwrap()).unwrap();
    rig.api.fan_smart().unwrap();

    let bound = rig.api.bind_mode("turbo", Some("red"), Some("loud")).unwrap();
    assert_eq!(
        bound.bindings.get(ts_decision::smartprofile::Mode::Turbo).unwrap().look.as_ref().unwrap().name,
        "red"
    );
    rig.ticks(8);
    assert!(!rig.journal.text().contains("mode binding turbo"), "binding applies nothing by itself");

    mode(&rig, "turbo");
    let journal = rig.journal.text();
    assert!(journal.contains("mode binding turbo: look red: applied"), "{journal}");
    assert!(journal.contains("mode binding turbo: fan curve loud: applied"), "{journal}");
    assert_eq!(rig.state.get_str("fan_curve", ""), "loud");
    assert_eq!(rig.api.fan_curve_in_use().unwrap().name, "loud");
    assert!(rig.api.smart_fan().unwrap(), "Smart stays on");

    // Another mode with no binding: nothing more happens, and the curve stays.
    mode(&rig, "quiet");
    assert_eq!(rig.journal.text().matches("mode binding turbo").count(), 2);
    assert_eq!(rig.state.get_str("fan_curve", ""), "loud");
    assert_eq!(rig.api.mode_bindings().unwrap().mode.as_deref(), Some("quiet"));
}

#[test]
fn a_manual_fan_survives_the_mode_change_and_apply_now_reports_it() {
    let rig = Rig::new();
    rig.api.save_look_value("red", &look("ff0000")).unwrap();
    rig.api.bind_mode("turbo", Some("red"), Some("performance")).unwrap();
    rig.api.fan_set(55, None, false).unwrap();
    let before = rig.state.get_str("fan_curve", "");
    mode(&rig, "turbo");
    assert_eq!(rig.state.get_str("fan_curve", ""), before);
    assert_eq!(rig.sim.snapshot().duty, [55, 55]);
    let applied = rig.api.apply_mode_binding(None).unwrap();
    assert_eq!(applied.mode, "turbo");
    let statuses: Vec<(&str, &str)> =
        applied.results.iter().map(|r| (r.target.as_str(), r.status.as_str())).collect();
    assert_eq!(statuses, [("look", "applied"), ("fan_curve", "skipped")]);
    assert_eq!(rig.sim.snapshot().duty, [55, 55]);
}

#[test]
fn saving_the_look_again_refreshes_the_daemons_copy() {
    let rig = Rig::new();
    rig.api.save_look_value("red", &look("ff0000")).unwrap();
    rig.api.bind_mode("turbo", Some("red"), None).unwrap();
    rig.api.save_look_value("red", &look("0000ff")).unwrap();
    let stored = rig.state.get("mode_bindings").unwrap();
    assert_eq!(stored["turbo"]["look"]["data"], serde_json::to_value(look("0000ff")).unwrap());
    // And it is the new one that applies.
    let applied = rig.api.apply_mode_binding(Some("turbo")).unwrap();
    assert_eq!(applied.results[0].status, "applied");
}

#[test]
fn unbinding_stops_the_application_and_survives_in_the_state_file() {
    let rig = Rig::new();
    rig.api.bind_mode("turbo", Some("native"), None).unwrap();
    assert!(rig.state.get("mode_bindings").unwrap()["turbo"]["look"].is_object());
    let after = rig.api.unbind_mode("turbo", "all").unwrap();
    assert!(after.bindings.is_empty());
    mode(&rig, "turbo");
    assert!(!rig.journal.text().contains("mode binding turbo"));
    assert_eq!(rig.state.get("mode_bindings").unwrap(), json!({}));
}

#[test]
fn the_daemon_refuses_what_the_application_layer_would_have_refused() {
    let rig = Rig::new();
    // A client that skips the application layer's checks meets the same rules in the daemon.
    for args in [
        ts_ipc::wire::Args::new().with("action", "set").with("mode", "turbo"),
        ts_ipc::wire::Args::new()
            .with("action", "set")
            .with("mode", "warp")
            .with("look", json!({"name": "x", "data": {}})),
        ts_ipc::wire::Args::new()
            .with("action", "set")
            .with("mode", "turbo")
            .with("fan_curve", json!({"name": "n", "data": Curve::builtin("native").unwrap().to_value()})),
    ] {
        assert!(rig.client.call("mode_bindings", args.clone()).is_err(), "{args:?}");
    }
    assert!(rig.api.mode_bindings().unwrap().bindings.is_empty());
}
