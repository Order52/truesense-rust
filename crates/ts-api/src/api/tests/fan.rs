use serde_json::json;
use ts_decision::curves::{Curve, Point};
use ts_platform::firmware::SimFan;
use ts_platform::hw::FanMode;

use super::Fx;
use crate::api::testing::Reply;
use crate::api::{ApiError, CurveEdit, FanRequest, FanRoute, Refusal};

fn refused(e: ApiError) -> Refusal {
    e.refusal().cloned().unwrap_or_else(|| panic!("expected a refusal, got: {e}"))
}

#[test]
fn auto_and_max_go_through_the_daemon_in_one_request() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok("fan", json!("auto"));
    let out = fx.api.fan_auto().unwrap();
    assert_eq!(out.route, FanRoute::Daemon);
    fx.api.fan_max().unwrap();
    let calls = fx.rig.daemon.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].1.get("mode"), Some(&json!("auto")));
    assert_eq!(calls[0].1.get("cpu"), Some(&json!(null)));
    assert_eq!(calls[1].1.get("mode"), Some(&json!("max")));
    // The firmware was not touched: the daemon owns it while it runs.
    assert!(matches!(fx.rig.sim.snapshot().fan, SimFan::Auto));
}

#[test]
fn without_a_daemon_the_firmware_is_written_directly() {
    let fx = Fx::offline();
    assert_eq!(fx.api.fan_max().unwrap().route, FanRoute::Hardware);
    assert!(matches!(fx.rig.sim.snapshot().fan, SimFan::Max));
    fx.api.fan_auto().unwrap();
    assert!(matches!(fx.rig.sim.snapshot().fan, SimFan::Auto));
    fx.api.fan_set(60, Some(40), false).unwrap();
    let snap = fx.rig.sim.snapshot();
    assert!(matches!(snap.fan, SimFan::Manual));
    assert_eq!(snap.duty, [60, 40]);
    // One duty for both fans when the second is not given.
    fx.api.fan_set(35, None, false).unwrap();
    assert_eq!(fx.rig.sim.snapshot().duty, [35, 35]);
    assert!(fx.rig.daemon.calls().is_empty());
}

#[test]
fn the_smart_curve_is_the_daemons() {
    let fx = Fx::offline();
    assert_eq!(refused(fx.api.fan_smart().unwrap_err()), Refusal::SmartNeedsService);
    fx.rig.daemon.set_available(true);
    fx.rig.daemon.answer_ok("fan", json!("smart"));
    fx.api.fan_smart().unwrap();
    assert_eq!(fx.rig.daemon.last_args("fan").unwrap().get("mode"), Some(&json!("smart")));
}

#[test]
fn a_duty_is_zero_or_twenty_to_a_hundred_and_nothing_else_is_sent() {
    let fx = Fx::new();
    for bad in [-5, 1, 10, 19, 101, 255] {
        for confirm in [false, true] {
            assert_eq!(refused(fx.api.fan_set(bad, None, confirm).unwrap_err()), Refusal::FanDuty, "{bad}");
            assert_eq!(
                refused(fx.api.fan_set(50, Some(bad), confirm).unwrap_err()),
                Refusal::FanDuty,
                "{bad}"
            );
        }
    }
    assert!(fx.rig.daemon.calls().is_empty());
    assert!(matches!(fx.rig.sim.snapshot().fan, SimFan::Auto));
    fx.rig.daemon.answer_ok("fan", json!("manual"));
    for ok in [20, 21, 99, 100] {
        fx.api.fan_set(ok, None, false).unwrap();
    }
}

#[test]
fn stopping_a_fan_needs_the_confirmation_before_anything_is_sent() {
    let fx = Fx::new();
    for (cpu, gpu) in [(0, None), (0, Some(0)), (50, Some(0)), (0, Some(50))] {
        assert_eq!(refused(fx.api.fan_set(cpu, gpu, false).unwrap_err()), Refusal::StopNeedsConfirmation);
    }
    assert_eq!(refused(fx.api.fan_off(false).unwrap_err()), Refusal::StopNeedsConfirmation);
    assert!(fx.rig.daemon.calls().is_empty(), "a refused stop must not reach the service");
    assert_eq!(Refusal::StopNeedsConfirmation.to_string(), "stopping a fan needs explicit confirmation");
}

#[test]
fn a_confirmed_stop_goes_to_the_service_with_the_confirmation() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok("fan", json!("partial-off"));
    let out = fx.api.fan_set(0, Some(50), true).unwrap();
    assert_eq!(out.route, FanRoute::Daemon);
    let args = fx.rig.daemon.last_args("fan").unwrap();
    assert_eq!(args.get("mode"), Some(&json!("manual")));
    assert_eq!(args.get("cpu"), Some(&json!(0)));
    assert_eq!(args.get("gpu"), Some(&json!(50)));
    assert_eq!(args.get("confirm"), Some(&json!(true)));
    fx.api.fan_off(true).unwrap();
    let args = fx.rig.daemon.last_args("fan").unwrap();
    assert_eq!(args.get("mode"), Some(&json!("off")));
    assert_eq!(args.get("confirm"), Some(&json!(true)));
}

#[test]
fn a_stop_is_never_written_to_the_firmware_without_the_service() {
    let fx = Fx::offline();
    assert_eq!(refused(fx.api.fan_set(0, None, true).unwrap_err()), Refusal::StopNeedsService);
    assert_eq!(refused(fx.api.fan_set(40, Some(0), true).unwrap_err()), Refusal::StopNeedsService);
    assert_eq!(refused(fx.api.fan_off(true).unwrap_err()), Refusal::OffNeedsService);
    let snap = fx.rig.sim.snapshot();
    assert!(matches!(snap.fan, SimFan::Auto));
    assert_ne!(snap.duty, [0, 0]);
}

#[test]
fn the_service_refusing_a_stop_is_shown_as_it_said_it() {
    let fx = Fx::new();
    fx.rig.daemon.answer("fan", Reply::Error("too warm to stop a fan (CPU 66 C, GPU 50 C)".into()));
    let e = fx.api.fan_set(0, None, true).unwrap_err();
    assert_eq!(e.to_string(), "too warm to stop a fan (CPU 66 C, GPU 50 C)");
    assert!(matches!(e, ApiError::Daemon(_)));
    // The refusal is not turned into a hardware write.
    assert!(matches!(fx.rig.sim.snapshot().fan, SimFan::Auto));
}

#[test]
fn a_daemon_error_is_not_a_reason_to_fall_back_to_the_firmware() {
    let fx = Fx::new();
    fx.rig.daemon.answer("fan", Reply::Error("device unavailable".into()));
    assert!(fx.api.fan_max().is_err());
    assert!(matches!(fx.rig.sim.snapshot().fan, SimFan::Auto));
}

#[test]
fn an_old_daemon_that_does_not_know_fan_is_handled_the_old_way() {
    let fx = Fx::new();
    fx.rig.daemon.answer("fan", Reply::UnknownCommand);
    fx.rig.daemon.answer_ok("set", json!({"smart_fan": false}));
    let out = fx.api.fan_max().unwrap();
    assert_eq!(out.route, FanRoute::LegacyDaemon);
    assert_eq!(fx.rig.daemon.commands(), ["fan", "set"]);
    let set = fx.rig.daemon.last_args("set").unwrap();
    assert_eq!(set.get("key"), Some(&json!("smart_fan")));
    assert_eq!(set.get("value"), Some(&json!(false)));
    assert_eq!(fx.rig.sleeps.lock().unwrap().as_slice(), [std::time::Duration::from_millis(1200)]);
    assert!(matches!(fx.rig.sim.snapshot().fan, SimFan::Max));
}

#[test]
fn an_old_daemon_that_cannot_stop_its_curve_is_not_waited_for() {
    let fx = Fx::new();
    fx.rig.daemon.answer("fan", Reply::UnknownCommand);
    fx.rig.daemon.answer("set", Reply::Error("nope".into()));
    fx.api.fan_auto().unwrap();
    assert!(fx.rig.sleeps.lock().unwrap().is_empty());
}

#[test]
fn an_old_daemon_gets_the_smart_curve_through_set() {
    let fx = Fx::new();
    fx.rig.daemon.answer("fan", Reply::UnknownCommand);
    fx.rig.daemon.answer_ok("set", json!({"smart_fan": true}));
    assert_eq!(fx.api.fan_smart().unwrap().route, FanRoute::LegacyDaemon);
    assert_eq!(fx.rig.daemon.last_args("set").unwrap().get("value"), Some(&json!(true)));
}

#[test]
fn an_old_daemon_never_gets_a_stop_either() {
    let fx = Fx::new();
    fx.rig.daemon.answer("fan", Reply::UnknownCommand);
    // A stop is sent to the service only; an old one that does not know it is an error, not a write.
    let e = fx.api.fan_set(0, None, true).unwrap_err();
    assert!(matches!(e, ApiError::Daemon(_)), "{e}");
    assert!(matches!(fx.rig.sim.snapshot().fan, SimFan::Auto));
}

#[test]
fn request_words_parse_and_unknown_ones_are_refused() {
    assert_eq!("smart".parse::<FanRequest>().unwrap(), FanRequest::Smart);
    assert_eq!("off".parse::<FanRequest>().unwrap(), FanRequest::Off);
    assert!("turbo".parse::<FanRequest>().is_err());
}

#[test]
fn the_outcome_reads_like_the_old_command_did() {
    let fx = Fx::offline();
    assert_eq!(fx.api.fan_auto().unwrap().to_string(), "fans: firmware auto (they stop when cool)");
    assert_eq!(
        fx.api.fan_max().unwrap().to_string(),
        "fans: MAX  (`truesense fan smart on` hands them back)"
    );
    assert_eq!(fx.api.fan_set(60, None, false).unwrap().to_string(), "fans: manual 60 % / 60 %");
    let fx = Fx::new();
    fx.rig.daemon.answer_ok("fan", json!("off"));
    let stop = fx.api.fan_set(0, None, true).unwrap().to_string();
    assert!(stop.starts_with(
        "fans: manual 0 % / 0 %   (a stopped fan: all fans return to Auto at CPU 70 C / GPU 65 C"
    ));
}

#[test]
fn the_status_joins_firmware_speed_and_the_smart_switch() {
    let fx = Fx::offline();
    fx.rig.sim.set_sensors([50, 52, 48], [1800, 1700]);
    let st = fx.api.fan_status().unwrap();
    assert_eq!(st.state.mode, FanMode::Auto);
    assert_eq!((st.cpu_fan_rpm, st.gpu_fan_rpm), (1800, 1700));
    assert_eq!(st.smart, None);
    let json = serde_json::to_value(&st).unwrap();
    assert_eq!(json["smart"], json!(null));
    assert_eq!(json["mode"], json!("auto"));
    assert_eq!(json["cpu_fan_rpm"], json!(1800));
    assert_eq!(json["behavior_raw"], json!("0x41"));
    assert!(st.to_string().ends_with("smart curve: unknown"));
    fx.rig.daemon.set_available(true);
    fx.rig.daemon.answer_ok("state", super::daemon_state(json!({"smart_fan": true}), json!({}), 0));
    assert_eq!(fx.api.fan_status().unwrap().smart, Some(true));
}

#[test]
fn the_smart_switch_reads_and_sets_the_daemons_value() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok("state", super::daemon_state(json!({"smart_fan": true}), json!({}), 0));
    assert!(fx.api.smart_fan().unwrap());
    fx.rig.daemon.answer_ok("set", json!({"smart_fan": false}));
    assert!(!fx.api.set_smart_fan(false).unwrap());
    fx.rig.daemon.answer_ok("set", json!({}));
    assert!(fx.api.set_smart_fan(true).is_err(), "a reply without the key is not taken as set");
}

fn curve_reply(name: &str) -> serde_json::Value {
    let c = Curve::builtin(name).unwrap();
    json!({"name": name, "curve": c.to_value(), "names": ["native", "truesense"]})
}

#[test]
fn the_curve_list_shows_the_one_in_use_when_the_service_runs() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok("fan_curve", curve_reply("truesense"));
    let list = fx.api.fan_curves().unwrap();
    assert_eq!(list.current.as_deref(), Some("truesense"));
    assert!(list.names.starts_with(&["native".to_owned(), "truesense".to_owned()]));
    assert!(list.descriptions.contains_key("native"));
    let text = list.to_string();
    assert!(text.contains("* truesense  - "), "{text}");
    assert!(text.contains("  native  - "), "{text}");
    let off = Fx::offline();
    assert_eq!(off.api.fan_curves().unwrap().current, None);
}

#[test]
fn the_curve_in_use_is_parsed_and_checked() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok("fan_curve", curve_reply("native"));
    let in_use = fx.api.fan_curve_in_use().unwrap();
    assert_eq!(in_use.name, "native");
    assert!(in_use.curve.native);
    fx.rig.daemon.answer_ok("fan_curve", json!({"name": "x", "curve": {"cpu": [[50, 0]], "gpu": []}}));
    assert!(fx.api.fan_curve_in_use().is_err());
    fx.rig.daemon.answer_ok("fan_curve", json!("garbage"));
    assert!(fx.api.fan_curve_in_use().is_err());
}

#[test]
fn a_built_in_curve_goes_by_name_and_a_saved_one_travels_as_data() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok("fan_curve", json!({"name": "quiet", "curve": {}}));
    assert_eq!(fx.api.use_fan_curve("quiet").unwrap(), "quiet");
    let args = fx.rig.daemon.last_args("fan_curve").unwrap();
    assert_eq!(args.get("action"), Some(&json!("use")));
    assert_eq!(args.get("data"), Some(&json!(null)));
    fx.api.save_fan_curve("mine", &CurveEdit { from: Some("quiet".into()), ..CurveEdit::default() }).unwrap();
    fx.rig.daemon.answer_ok("fan_curve", json!({"name": "mine", "curve": {}}));
    assert_eq!(fx.api.use_fan_curve("mine").unwrap(), "mine");
    let data = fx.rig.daemon.last_args("fan_curve").unwrap().get("data").cloned().unwrap();
    assert!(data["cpu"].is_array() && data["gpu"].is_array(), "{data}");
    assert!(fx.api.use_fan_curve("").unwrap_err().to_string().starts_with("which curve?"));
    assert!(fx.api.use_fan_curve("nosuch").is_err());
}

#[test]
fn a_curve_that_is_too_cool_is_refused_before_it_is_sent() {
    let fx = Fx::new();
    let cool = Curve::new(
        vec![Point::new(40.0, 20.0), Point::new(90.0, 60.0)],
        vec![Point::new(40.0, 20.0), Point::new(80.0, 100.0)],
    );
    let e = fx.api.use_fan_curve_data("editor", &cool).unwrap_err();
    assert!(e.to_string().contains("95"), "{e}");
    assert!(fx.rig.daemon.calls().is_empty());
}

#[test]
fn an_edited_curve_is_sent_with_a_label() {
    let fx = Fx::new();
    let mut c = Curve::builtin("truesense").unwrap();
    c.description = None;
    fx.rig.daemon.answer_ok("fan_curve", json!({"name": "custom"}));
    assert_eq!(fx.api.use_fan_curve_data("", &c).unwrap(), "custom");
    assert_eq!(fx.rig.daemon.last_args("fan_curve").unwrap().get("name"), Some(&json!("custom")));
}

#[test]
fn saving_a_curve_starts_from_a_base_and_changes_only_what_is_given() {
    let fx = Fx::new();
    let saved = fx
        .api
        .save_fan_curve(
            "mine",
            &CurveEdit {
                from: Some("truesense".into()),
                cpu: Some("52:0,60:35,70:55,80:75,89:100".into()),
                predict: Some(7.0),
                ..CurveEdit::default()
            },
        )
        .unwrap();
    assert_eq!(saved.cpu.len(), 5);
    assert_eq!(saved.predict, 7.0);
    assert_eq!(saved.gpu, Curve::builtin("truesense").unwrap().gpu);
    assert_eq!(saved.description, None);
    // Editing your own curve by name keeps the rest of it.
    let again =
        fx.api.save_fan_curve("mine", &CurveEdit { hysteresis: Some(6.0), ..CurveEdit::default() }).unwrap();
    assert_eq!(again.cpu.len(), 5);
    assert_eq!(again.hysteresis, 6.0);
    fx.rig.daemon.set_available(false);
    assert!(fx.api.fan_curves().unwrap().names.contains(&"mine".to_owned()));
}

#[test]
fn saving_needs_both_sides_and_a_valid_name_and_never_overwrites_a_built_in() {
    let fx = Fx::new();
    let e = fx
        .api
        .save_fan_curve("mine", &CurveEdit { cpu: Some("52:0,89:100".into()), ..CurveEdit::default() })
        .unwrap_err();
    assert_eq!(e.to_string(), "give --cpu and --gpu points (or --from an existing curve)");
    assert_eq!(
        fx.api.save_fan_curve("", &CurveEdit::default()).unwrap_err().to_string(),
        "save under which name?"
    );
    assert!(
        fx.api
            .save_fan_curve("truesense", &CurveEdit { from: Some("quiet".into()), ..CurveEdit::default() })
            .is_err()
    );
    assert!(
        fx.api
            .save_fan_curve("a/b", &CurveEdit { from: Some("quiet".into()), ..CurveEdit::default() })
            .is_err()
    );
    assert!(
        fx.api
            .save_fan_curve(
                "bad",
                &CurveEdit { from: Some("quiet".into()), cpu: Some("junk".into()), ..CurveEdit::default() }
            )
            .is_err()
    );
}

#[test]
fn copy_and_delete_follow_the_old_rules() {
    let fx = Fx::new();
    assert_eq!(fx.api.copy_fan_curve("quiet", None).unwrap(), "quiet-copy");
    assert_eq!(fx.api.copy_fan_curve("quiet", Some("mine")).unwrap(), "mine");
    assert!(fx.api.copy_fan_curve("", None).is_err());
    fx.api.delete_fan_curve("mine").unwrap();
    assert!(fx.api.delete_fan_curve("mine").is_err());
    assert!(fx.api.delete_fan_curve("quiet").is_err(), "built-in curves are read-only");
    assert!(crate::api::Api::fan_curve_is_builtin("quiet"));
    assert!(!crate::api::Api::fan_curve_is_builtin("quiet-copy"));
    assert_eq!(fx.api.fan_curve("quiet-copy").unwrap().cpu, Curve::builtin("quiet").unwrap().cpu);
}

#[test]
fn a_message_only_answer_prints_as_its_message_under_json() {
    let fx = Fx::offline();
    let out = fx.api.fan_auto().unwrap();
    assert_eq!(serde_json::to_value(out).unwrap(), json!("fans: firmware auto (they stop when cool)"));
    fx.rig.daemon.set_available(true);
    fx.rig.daemon.answer_ok("hold", json!(null));
    let hold = fx.api.hold_power(30.0).unwrap();
    assert_eq!(serde_json::to_value(hold).unwrap(), json!("holding the current power mode for 30 min"));
}

#[test]
fn a_curve_prints_both_sides_and_its_parameters() {
    let c = Curve::builtin("truesense").unwrap();
    let text = crate::api::describe_curve("truesense", &c);
    assert!(text.starts_with("truesense\n  cpu [("), "{text}");
    assert!(text.contains("\n  gpu [(") && text.ends_with("battery_shift=3"), "{text}");
}
