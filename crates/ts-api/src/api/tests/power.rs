use serde_json::json;
use ts_platform::firmware::Snapshot;
use ts_platform::hw::{Coded, PowerMode};

use super::{Fx, daemon_state};
use crate::api::testing::Reply;
use crate::api::{ApiError, AutoAction, AutoMode, GameRuleAction};

#[test]
fn the_status_adds_the_automatic_profile_only_when_the_service_answers() {
    let fx = Fx::offline();
    let st = fx.api.power_status().unwrap();
    assert_eq!(st.auto_profile, None);
    assert!(st.supported.contains(&PowerMode::Turbo));
    let json = serde_json::to_value(&st).unwrap();
    assert!(json.get("auto_profile").is_none() && json.get("hold_left_s").is_none());
    assert!(json["mode"].is_string());
    assert!(st.to_string().starts_with("power mode: "));
    assert!(!st.to_string().contains("automatic profile"));

    let fx = Fx::new();
    fx.rig.daemon.answer_ok("state", daemon_state(json!({"auto_profile": true}), json!({}), 600));
    let st = fx.api.power_status().unwrap();
    assert_eq!(st.auto_profile, Some(true));
    assert_eq!(st.hold_left_s, Some(600));
    let text = st.to_string();
    assert!(text.ends_with("(automatic profile on, manual choice held for 10 more min)"), "{text}");
}

#[test]
fn the_mode_goes_through_the_daemon_with_its_hold() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok("power", json!("turbo"));
    let mode = fx.api.set_power(PowerMode::Turbo, Some(30.0)).unwrap();
    assert_eq!(mode, PowerMode::Turbo);
    let args = fx.rig.daemon.last_args("power").unwrap();
    assert_eq!(args.get("mode"), Some(&json!("turbo")));
    assert_eq!(args.get("hold_minutes"), Some(&json!(30.0)));
    fx.api.set_power(PowerMode::Quiet, None).unwrap();
    assert_eq!(fx.rig.daemon.last_args("power").unwrap().get("hold_minutes"), Some(&json!(null)));
}

#[test]
fn without_a_daemon_the_firmware_is_written_and_read_back() {
    let fx = Fx::offline();
    let mode = fx.api.set_power(PowerMode::Performance, Some(30.0)).unwrap();
    assert_eq!(mode, PowerMode::Performance);
    let snap: Snapshot = fx.rig.sim.snapshot();
    assert_eq!(i64::from(snap.mode), PowerMode::Performance.code());
    assert!(fx.rig.daemon.calls().is_empty());
}

#[test]
fn an_old_daemon_is_skipped_but_still_asked_to_hold() {
    let fx = Fx::new();
    fx.rig.daemon.answer("power", Reply::UnknownCommand);
    fx.rig.daemon.answer_ok("hold", json!(null));
    fx.api.set_power(PowerMode::Eco, Some(15.0)).unwrap();
    assert_eq!(fx.rig.daemon.commands(), ["power", "hold"]);
    assert_eq!(fx.rig.daemon.last_args("hold").unwrap().get("minutes"), Some(&json!(15.0)));
    assert_eq!(i64::from(fx.rig.sim.snapshot().mode), PowerMode::Eco.code());
}

#[test]
fn a_hold_that_cannot_be_set_does_not_fail_the_mode() {
    let fx = Fx::new();
    fx.rig.daemon.answer("power", Reply::UnknownCommand);
    fx.api.set_power(PowerMode::Quiet, Some(15.0)).unwrap();
    // No hold when none was asked for.
    fx.rig.daemon.clear_calls();
    fx.api.set_power(PowerMode::Balanced, Some(0.0)).unwrap();
    assert_eq!(fx.rig.daemon.commands(), ["power"]);
}

#[test]
fn a_daemon_error_is_shown_and_the_firmware_is_left_alone() {
    let fx = Fx::new();
    fx.rig.daemon.answer("power", Reply::Error("power mode readback mismatch".into()));
    let before = fx.rig.sim.snapshot().mode;
    let e = fx.api.set_power(PowerMode::Turbo, None).unwrap_err();
    assert_eq!(e.to_string(), "power mode readback mismatch");
    assert_eq!(fx.rig.sim.snapshot().mode, before);
}

#[test]
fn the_modes_are_the_ones_the_firmware_accepts() {
    let fx = Fx::offline();
    let modes = fx.api.power_modes().unwrap();
    assert!(modes.contains(&PowerMode::Quiet) && modes.contains(&PowerMode::Eco));
    assert!("warp".parse::<PowerMode>().is_err());
}

#[test]
fn next_and_previous_are_the_key_actions() {
    let fx = Fx::new();
    fx.rig.actions.answer("power-next", Ok("Power mode: Turbo"));
    assert_eq!(fx.api.next_power().unwrap(), "Power mode: Turbo");
    fx.api.prev_power().unwrap();
    assert_eq!(fx.rig.actions.calls(), ["power-next", "power-prev"]);
    fx.rig.actions.answer("power-next", Err("no daemon"));
    assert_eq!(fx.api.next_power().unwrap_err().to_string(), "no daemon");
}

#[test]
fn hold_and_release_send_minutes_or_null() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok("hold", json!(null));
    assert_eq!(fx.api.hold_power(45.0).unwrap().to_string(), "holding the current power mode for 45 min");
    assert_eq!(fx.rig.daemon.last_args("hold").unwrap().get("minutes"), Some(&json!(45.0)));
    assert_eq!(
        fx.api.release_hold().unwrap().to_string(),
        "hold released: the automatic profile is in charge again"
    );
    assert_eq!(fx.rig.daemon.last_args("hold").unwrap().get("minutes"), Some(&json!(null)));
    let off = Fx::offline();
    assert!(off.api.hold_power(5.0).unwrap_err().daemon_missing());
}

#[test]
fn the_automatic_mode_reads_the_new_setting_and_falls_back_to_the_old_switch() {
    let fx = Fx::new();
    fx.rig
        .daemon
        .answer_ok("state", daemon_state(json!({"auto_mode": "native", "auto_profile": true}), json!({}), 0));
    assert_eq!(fx.api.auto_mode().unwrap(), AutoMode::Native);
    fx.rig.daemon.answer_ok("state", daemon_state(json!({"auto_profile": true}), json!({}), 0));
    assert_eq!(fx.api.auto_mode().unwrap(), AutoMode::Smart);
    fx.rig.daemon.answer_ok("state", daemon_state(json!({"auto_profile": false}), json!({}), 0));
    assert_eq!(fx.api.auto_mode().unwrap(), AutoMode::Off);
    fx.rig
        .daemon
        .answer_ok("state", daemon_state(json!({"auto_mode": "bogus", "auto_profile": true}), json!({}), 0));
    assert_eq!(fx.api.auto_mode().unwrap(), AutoMode::Smart);
}

#[test]
fn power_auto_actions_change_only_what_differs() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok("state", daemon_state(json!({"auto_mode": "off"}), json!({}), 0));
    fx.rig.daemon.answer_ok("set", json!({"auto_mode": "smart"}));
    assert_eq!(fx.api.apply_auto_mode(AutoAction::On).unwrap(), AutoMode::Smart);
    assert_eq!(fx.rig.daemon.last_args("set").unwrap().get("value"), Some(&json!("smart")));
    fx.rig.daemon.clear_calls();
    assert_eq!(fx.api.apply_auto_mode(AutoAction::Status).unwrap(), AutoMode::Off);
    assert_eq!(fx.api.apply_auto_mode(AutoAction::Set(AutoMode::Off)).unwrap(), AutoMode::Off);
    assert!(!fx.rig.daemon.commands().contains(&"set".to_owned()));
    assert_eq!(fx.api.apply_auto_mode(AutoAction::Toggle).unwrap(), AutoMode::Smart);
    fx.rig.daemon.answer_ok("state", daemon_state(json!({"auto_mode": "native"}), json!({}), 0));
    assert_eq!(fx.api.apply_auto_mode(AutoAction::Toggle).unwrap(), AutoMode::Off);
}

#[test]
fn auto_mode_words_parse() {
    assert_eq!("on".parse::<AutoAction>().unwrap(), AutoAction::On);
    assert_eq!("native".parse::<AutoAction>().unwrap(), AutoAction::Set(AutoMode::Native));
    assert_eq!("".parse::<AutoAction>().unwrap(), AutoAction::Status);
    assert_eq!("auto_mode: off | native | smart", "blue".parse::<AutoAction>().unwrap_err().to_string());
    assert!(AutoMode::Native.to_string().contains("like PredatorSense"));
}

#[test]
fn game_rules_parse_the_command_line_and_decode_the_answer() {
    assert_eq!(GameRuleAction::parse("list", None, None).unwrap(), GameRuleAction::List);
    assert_eq!(
        GameRuleAction::parse("add", Some("eldenring.exe"), Some("turbo")).unwrap(),
        GameRuleAction::Add { program: "eldenring.exe".into(), mode: PowerMode::Turbo }
    );
    for bad in [("add", None, None), ("add", Some("x"), None), ("add", None, Some("turbo"))] {
        let e = GameRuleAction::parse(bad.0, bad.1, bad.2).unwrap_err();
        assert!(e.to_string().starts_with("usage: truesense power game add PROGRAM MODE"), "{e}");
    }
    assert_eq!(
        GameRuleAction::parse("remove", None, None).unwrap_err().to_string(),
        "usage: truesense power game remove PROGRAM"
    );
    assert!(GameRuleAction::parse("add", Some("x"), Some("warp")).is_err());
    assert!(GameRuleAction::parse("explode", None, None).is_err());

    let fx = Fx::new();
    fx.rig.daemon.answer_ok(
        "game_rules",
        json!({"rules": {"eldenring.exe": "turbo", "blender": "performance"}, "running": "turbo"}),
    );
    let rules = fx
        .api
        .game_rules(&GameRuleAction::Add { program: "blender".into(), mode: PowerMode::Performance })
        .unwrap();
    let args = fx.rig.daemon.last_args("game_rules").unwrap();
    assert_eq!(args.get("action"), Some(&json!("add")));
    assert_eq!(args.get("name"), Some(&json!("blender")));
    assert_eq!(args.get("mode"), Some(&json!("performance")));
    assert_eq!(rules.running.as_deref(), Some("turbo"));
    let text = rules.to_string();
    assert!(text.contains("  blender                      performance"), "{text}");
    assert!(text.ends_with("running now: turbo mode"), "{text}");
    fx.api.game_rules(&GameRuleAction::Clear).unwrap();
    assert_eq!(fx.rig.daemon.last_args("game_rules").unwrap().get("name"), Some(&json!(null)));
    fx.rig.daemon.answer_ok("game_rules", json!({"rules": {}, "running": null}));
    assert_eq!(fx.api.game_rules(&GameRuleAction::List).unwrap().to_string(), "  (no game rules)");
}

#[test]
fn the_other_power_managers_are_listed_stopped_and_restored() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok(
        "power_managers",
        json!({"on": false, "found": [{"unit": "tlp", "active": true, "enabled": true}], "recorded": false}),
    );
    let state = fx.api.power_managers().unwrap();
    assert_eq!(fx.rig.daemon.last_args("power_managers").unwrap().get("action"), Some(&json!("status")));
    assert_eq!(state.found.len(), 1);
    assert_eq!(
        state.to_string(),
        "TrueSense manages power: off\n  tlp                      running, enabled"
    );

    fx.rig.daemon.answer_ok(
        "power_managers",
        json!({"found": ["tlp"], "stopped": ["tlp"], "failed": ["auto-cpufreq"], "cpu_not_set": [],
               "cpu": {"no_turbo": "0", "governor": "powersave", "epp": "balance_performance"}, "ceiling_fixed": false, "on": true}),
    );
    let stopped = fx.api.stop_power_managers().unwrap();
    assert_eq!(fx.rig.daemon.last_args("power_managers").unwrap().get("action"), Some(&json!("takeover")));
    assert!(stopped.on);
    let text = stopped.to_string();
    assert!(text.starts_with("stopped: tlp\nCOULD NOT STOP: auto-cpufreq\nCPU now: governor powersave, energy preference balance_performance, turbo on"), "{text}");
    assert!(text.ends_with("(undo: truesense power managers restore)"));

    fx.rig
        .daemon
        .answer_ok("power_managers", json!({"restored": ["tlp"], "cpu_not_set": [], "cpu": {}, "on": false}));
    let restored = fx.api.restore_power_managers().unwrap();
    assert_eq!(restored.to_string(), "restored: tlp");
    fx.rig.daemon.answer_ok(
        "power_managers",
        json!({"restored": [], "cpu": {}, "note": "nothing recorded", "on": false}),
    );
    assert_eq!(
        fx.api.restore_power_managers().unwrap().to_string(),
        "restored: nothing (TrueSense had not stopped anything)"
    );
}

#[test]
fn a_daemon_reply_of_the_wrong_shape_is_an_error_not_a_panic() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok("power_managers", json!([1, 2, 3]));
    let e = fx.api.power_managers().unwrap_err();
    assert!(matches!(e, ApiError::Daemon(_)));
    assert!(e.to_string().contains("unexpected reply"), "{e}");
    fx.rig.daemon.answer("game_rules", Reply::Unreachable("timed out".into()));
    assert_eq!(
        fx.api.game_rules(&GameRuleAction::List).unwrap_err().to_string(),
        "daemon not answering: timed out"
    );
}
