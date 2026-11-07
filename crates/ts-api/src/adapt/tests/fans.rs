//! `fan`, `fan_curve` and `smart_profile`: the fans, with the rules for stopping one, and the two
//! profiles the daemon keeps.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};
use ts_decision::curves::Curve;
use ts_ipc::client::ClientError;
use ts_ipc::wire::Args;
use ts_platform::firmware::SimFan;

use super::rig::Rig;
use crate::api::{ApiError, CurveEdit, FanRequest, FanRoute, Refusal};

fn daemon_error(e: &ApiError) -> String {
    match e {
        ApiError::Daemon(ClientError::Daemon(text)) => text.clone(),
        other => panic!("not a daemon refusal: {other:?}"),
    }
}

fn raw_fan(rig: &Rig, args: Args) -> Result<Value, String> {
    rig.client.call("fan", args).map_err(|e| e.to_string())
}

// ---- fan -------------------------------------------------------------------------------------

#[test]
fn auto_max_and_manual_reach_the_firmware_through_the_service() {
    let rig = Rig::new();
    let out = rig.api.fan_max().unwrap();
    assert_eq!(out.route, FanRoute::Daemon);
    assert_eq!(rig.sim.snapshot().fan, SimFan::Max);
    let out = rig.api.fan_set(40, Some(60), false).unwrap();
    assert_eq!(out.route, FanRoute::Daemon);
    let snap = rig.sim.snapshot();
    assert_eq!((snap.fan, snap.duty), (SimFan::Manual, [40, 60]));
    // One duty alone is used for both fans (Python: `g = int(c if gpu is None else gpu)`).
    rig.api.fan_set(35, None, false).unwrap();
    assert_eq!(rig.sim.snapshot().duty, [35, 35]);
    let out = rig.api.fan_auto().unwrap();
    assert_eq!(out.route, FanRoute::Daemon);
    assert_eq!(rig.sim.snapshot().fan, SimFan::Auto);
}

#[test]
fn the_raw_reply_of_fan_is_the_mode_as_python_returns_it() {
    let rig = Rig::new();
    assert_eq!(raw_fan(&rig, Args::new().with("mode", "max")).unwrap(), json!("max"));
    assert_eq!(raw_fan(&rig, Args::new().with("mode", "auto")).unwrap(), json!("auto"));
    assert_eq!(raw_fan(&rig, Args::new().with("mode", "smart")).unwrap(), json!("smart"));
    let manual = Args::new().with("mode", "manual").with("cpu", 50).with("gpu", json!(null));
    assert_eq!(raw_fan(&rig, manual).unwrap(), json!("manual"));
    assert_eq!(rig.sim.snapshot().duty, [50, 50]);
}

#[test]
fn a_fan_request_switches_the_smart_curve_off_in_the_same_step() {
    let rig = Rig::new();
    rig.api.fan_smart().unwrap();
    assert!(rig.api.smart_fan().unwrap());
    assert_eq!(rig.api.fan_status().unwrap().smart, Some(true));
    rig.api.fan_max().unwrap();
    // Python `fan`: `self.st.set("smart_fan", False)` under the same lock as the hardware write.
    assert!(!rig.api.smart_fan().unwrap());
    assert_eq!(rig.api.fan_status().unwrap().smart, Some(false));
    rig.api.fan_smart().unwrap();
    assert!(rig.api.smart_fan().unwrap());
}

#[test]
fn a_duty_outside_the_range_is_refused_by_both_halves_in_the_same_words() {
    let rig = Rig::new();
    // The application layer refuses before sending...
    for duty in [10, 19, 101, -5] {
        let err = rig.api.fan_set(duty, None, false).unwrap_err();
        assert_eq!(err.refusal(), Some(&Refusal::FanDuty), "{duty}");
    }
    // ...and the daemon refuses the same with the same text if a client does not.
    let text = "manual fan duty is 20..100 % per fan, or 0 to stop that fan";
    assert_eq!(Refusal::FanDuty.to_string(), text);
    for duty in [10, 19, 101] {
        let err = raw_fan(&rig, Args::new().with("mode", "manual").with("cpu", duty).with("gpu", duty))
            .unwrap_err();
        assert_eq!(err, text, "{duty}");
    }
    assert_eq!(rig.sim.snapshot().fan, SimFan::Auto, "nothing reached the firmware");
}

#[test]
fn a_fan_stop_needs_the_confirmation_in_both_halves() {
    let rig = Rig::new();
    assert_eq!(rig.api.fan_off(false).unwrap_err().refusal(), Some(&Refusal::StopNeedsConfirmation));
    assert_eq!(rig.api.fan_set(0, None, false).unwrap_err().refusal(), Some(&Refusal::StopNeedsConfirmation));
    assert_eq!(
        rig.api.fan_set(50, Some(0), false).unwrap_err().refusal(),
        Some(&Refusal::StopNeedsConfirmation)
    );
    let text = Refusal::StopNeedsConfirmation.to_string();
    // The daemon's own check (Python: "stopping a fan needs explicit confirmation").
    for args in [
        Args::new().with("mode", "off"),
        Args::new().with("mode", "manual").with("cpu", 0).with("gpu", 0),
        Args::new().with("mode", "manual").with("cpu", 50).with("gpu", 0),
        Args::new().with("mode", "manual").with("cpu", 0),
    ] {
        assert_eq!(raw_fan(&rig, args.clone()).unwrap_err(), text, "{args:?}");
    }
    let snap = rig.sim.snapshot();
    assert_eq!((snap.fan, snap.duty), (SimFan::Auto, [30, 30]), "the firmware was never touched");
}

#[test]
fn a_confirmed_stop_on_a_cool_machine_reaches_the_firmware_and_arms_the_watchdog() {
    let rig = Rig::new();
    rig.api.fan_off(true).unwrap();
    let snap = rig.sim.snapshot();
    assert_eq!((snap.fan, snap.duty), (SimFan::Manual, [0, 0]));
    // The raw answers are Python's: "off" for both fans, "partial-off" for one.
    assert_eq!(raw_fan(&rig, Args::new().with("mode", "off").with("confirm", true)).unwrap(), json!("off"));
    let one = Args::new().with("mode", "manual").with("cpu", 0).with("gpu", 40).with("confirm", true);
    assert_eq!(raw_fan(&rig, one).unwrap(), json!("partial-off"));
    assert_eq!(rig.sim.snapshot().duty, [0, 40]);
    let live = rig.api.daemon_state().unwrap();
    rig.ticks(2);
    assert_eq!(rig.api.daemon_state().unwrap().live_bool("fan_off"), Some(true), "{live:?}");
    // The watchdog: Python gives the fans back at CPU 70 C / GPU 65 C.
    rig.sim.set_sensors([75, 60, 50], [0, 800]);
    rig.wait_for(|| rig.sim.snapshot().fan == SimFan::Auto, "the watchdog to hand the fans back");
    assert_eq!(rig.api.daemon_state().unwrap().live_bool("fan_off"), Some(false));
}

#[test]
fn a_stop_on_a_warm_machine_is_refused_with_python_s_sentence() {
    let rig = Rig::new();
    rig.heat(65, 55, 50);
    let err = rig.api.fan_off(true).unwrap_err();
    // `FAN_OFF_START_CPU, FAN_OFF_START_GPU = 60, 55`.
    assert_eq!(
        daemon_error(&err),
        "too warm to stop a fan (CPU 65 C, GPU 50 C; a fan may only be stopped below 60 / 55 C)"
    );
    let err = rig.api.fan_set(30, Some(0), true).unwrap_err();
    assert!(daemon_error(&err).starts_with("too warm to stop a fan"), "{err}");
    assert_eq!(rig.sim.snapshot().fan, SimFan::Auto);
}

#[test]
fn an_unknown_fan_mode_and_a_missing_duty_are_refused_by_the_daemon() {
    let rig = Rig::new();
    // Python: `raise hw.HwError(f"fan mode {mode!r}")`.
    assert_eq!(raw_fan(&rig, Args::new().with("mode", "turbo")).unwrap_err(), "fan mode 'turbo'");
    assert!(raw_fan(&rig, Args::new()).is_err());
    assert!(raw_fan(&rig, Args::new().with("mode", "manual")).is_err());
    assert!(raw_fan(&rig, Args::new().with("mode", "manual").with("cpu", "fast")).is_err());
    assert!(raw_fan(&rig, Args::new().with("mode", "manual").with("cpu", true)).is_err());
}

#[test]
fn fan_status_combines_the_firmware_and_the_service() {
    let rig = Rig::new();
    rig.api.fan_set(45, Some(45), false).unwrap();
    rig.sim.set_sensors([50, 52, 48], [2100, 1900]);
    let status = rig.api.fan_status().unwrap();
    assert_eq!((status.cpu_fan_rpm, status.gpu_fan_rpm), (2100, 1900));
    assert_eq!(status.state.mode.to_string(), "manual");
    assert_eq!(status.state.duty_register_pct.cpu, 45);
    assert_eq!(status.smart, Some(false));
    let json = serde_json::to_value(&status).unwrap();
    assert_eq!(json["mode"], json!("manual"));
}

#[test]
fn the_legacy_path_is_never_taken_by_this_daemon() {
    let rig = Rig::new();
    for request in [FanRequest::Auto, FanRequest::Max, FanRequest::Smart] {
        let out = rig.api.set_fans(request, false).unwrap();
        assert_eq!(out.route, FanRoute::Daemon, "{request:?}");
    }
}

// ---- fan_curve -------------------------------------------------------------------------------

#[test]
fn fan_curve_get_has_python_s_three_keys_and_the_api_reads_them() {
    let rig = Rig::new();
    let raw = rig.client.call("fan_curve", Args::new().with("action", "get")).unwrap();
    let object = raw.as_object().unwrap();
    let mut keys = object.keys().cloned().collect::<Vec<_>>();
    keys.sort();
    assert_eq!(keys, ["curve", "name", "names"]);
    assert_eq!(object["name"], json!("native"));
    let names: Vec<&str> = object["names"].as_array().unwrap().iter().map(|n| n.as_str().unwrap()).collect();
    assert_eq!(names, rig.api.fan_curves().unwrap().names);
    assert!(names.contains(&"native") && names.contains(&"truesense"));
    // The curve carries what the follower needs, in Python's field names.
    for key in ["cpu", "gpu", "hysteresis", "down_rate", "predict", "couple", "battery_shift"] {
        assert!(object["curve"].get(key).is_some(), "curve.{key}: {}", object["curve"]);
    }
    let in_use = rig.api.fan_curve_in_use().unwrap();
    assert_eq!(in_use.name, "native");
    assert_eq!(in_use.names, names);
    let list = rig.api.fan_curves().unwrap();
    assert_eq!(list.current.as_deref(), Some("native"));
}

#[test]
fn a_built_in_curve_comes_back_as_python_s_pairs_and_parameters() {
    let rig = Rig::new();
    let args = Args::new().with("action", "use").with("name", "truesense").with("data", json!(null));
    let raw = rig.client.call("fan_curve", args).unwrap();
    assert_eq!(raw["name"], json!("truesense"));
    let curve = &raw["curve"];
    // `curves.BUILTIN["truesense"]`: points are `[temperature, percent]` pairs, the parameters are
    // `DEFAULTS` of `curves.py`.
    let pairs = |side: &str| -> Vec<(f64, f64)> {
        curve[side]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p[0].as_f64().unwrap(), p[1].as_f64().unwrap()))
            .collect()
    };
    assert_eq!(
        pairs("cpu"),
        [
            (52.0, 0.0),
            (56.0, 30.0),
            (62.0, 38.0),
            (68.0, 48.0),
            (74.0, 58.0),
            (80.0, 70.0),
            (85.0, 85.0),
            (89.0, 100.0)
        ]
    );
    assert_eq!(
        pairs("gpu"),
        [(48.0, 0.0), (54.0, 30.0), (60.0, 38.0), (65.0, 48.0), (70.0, 60.0), (74.0, 78.0), (78.0, 100.0)]
    );
    for (key, value) in
        [("hysteresis", 4.0), ("down_rate", 1.5), ("predict", 5.0), ("couple", 15.0), ("battery_shift", 3.0)]
    {
        assert_eq!(curve[key].as_f64(), Some(value), "{key}");
    }
    // The native one is flagged, as in `BUILTIN["native"]`.
    let raw = rig.client.call("fan_curve", Args::new().with("action", "use").with("name", "native")).unwrap();
    assert_eq!(raw["name"], json!("native"));
    assert_eq!(raw["curve"]["native"], json!(true));
}

#[test]
fn using_a_built_in_curve_sends_its_name_and_no_data() {
    let rig = Rig::new();
    let name = rig.api.use_fan_curve("truesense").unwrap();
    assert_eq!(name, "truesense");
    let st = rig.api.daemon_state().unwrap();
    assert_eq!(st.setting_str("fan_curve"), Some("truesense"));
    // Python: `fan_curve_data` is None for a built-in.
    assert_eq!(st.settings["fan_curve_data"], Value::Null);
    let in_use = rig.api.fan_curve_in_use().unwrap();
    assert_eq!(in_use.name, "truesense");
    assert_eq!(in_use.curve, rig.api.fan_curve("truesense").unwrap());
    rig.ticks(2);
    assert_eq!(rig.api.daemon_state().unwrap().live_str("fan_curve"), Some("truesense"));
}

#[test]
fn a_saved_curve_travels_with_the_request_and_the_service_keeps_a_copy() {
    let rig = Rig::new();
    let edit =
        CurveEdit { from: Some("truesense".to_owned()), hysteresis: Some(3.0), ..CurveEdit::default() };
    let saved = rig.api.save_fan_curve("mine", &edit).unwrap();
    assert_eq!(rig.api.use_fan_curve("mine").unwrap(), "mine");
    let st = rig.api.daemon_state().unwrap();
    assert_eq!(st.setting_str("fan_curve"), Some("mine"));
    // The daemon (root) cannot read the user's directory: it stored the curve itself.
    assert_eq!(st.settings["fan_curve_data"], saved.to_value());
    assert_eq!(rig.api.fan_curve_in_use().unwrap().curve, saved);
    // The window's editor sends a curve that was never saved: the label is only a label.
    let curve: Curve = rig.api.fan_curve("truesense").unwrap();
    assert_eq!(rig.api.use_fan_curve_data("scratch", &curve).unwrap(), "scratch");
    assert_eq!(rig.api.daemon_state().unwrap().setting_str("fan_curve"), Some("scratch"));
    assert_eq!(rig.api.use_fan_curve_data("", &curve).unwrap(), "custom");
    assert_eq!(rig.api.daemon_state().unwrap().setting_str("fan_curve"), Some("custom"));
}

#[test]
fn the_native_curve_hands_the_fans_back_to_the_firmware() {
    let rig = Rig::new();
    rig.api.fan_set(50, Some(50), false).unwrap();
    rig.api.fan_smart().unwrap();
    rig.api.use_fan_curve("truesense").unwrap();
    rig.api.fan_max().unwrap();
    assert_eq!(rig.sim.snapshot().fan, SimFan::Max);
    // Python: the native curve "is no curve": smart_fan off, firmware auto.
    let name = rig.api.use_fan_curve("native").unwrap();
    assert_eq!(name, "native");
    assert_eq!(rig.sim.snapshot().fan, SimFan::Auto);
    assert!(!rig.api.smart_fan().unwrap());
}

#[test]
fn fan_curve_refuses_in_python_s_words() {
    let rig = Rig::new();
    let err = rig.client.call("fan_curve", Args::new().with("action", "delete")).unwrap_err();
    assert!(matches!(&err, ClientError::Daemon(t) if t == "fan_curve: get | use"), "{err:?}");
    let err = rig.api.use_fan_curve("no-such-curve").unwrap_err();
    assert!(daemon_or_domain(&err).contains("no-such-curve"), "{err}");
    // A curve that is not a curve: the validator of the daemon refuses it.
    let bad = Args::new().with("action", "use").with("name", "x").with("data", json!({"cpu": [], "gpu": []}));
    assert!(rig.client.call("fan_curve", bad).is_err());
    let bad = Args::new().with("action", "use").with("name", "x").with("data", json!("a string"));
    assert!(rig.client.call("fan_curve", bad).is_err());
    assert_eq!(
        rig.api.daemon_state().unwrap().setting_str("fan_curve"),
        Some("native"),
        "nothing was changed"
    );
}

fn daemon_or_domain(e: &ApiError) -> String {
    e.to_string()
}

#[test]
fn a_curve_without_a_name_is_called_custom_like_python() {
    let rig = Rig::new();
    let curve = rig.api.fan_curve("truesense").unwrap();
    let args = Args::new().with("action", "use").with("name", json!(null)).with("data", curve.to_value());
    let raw = rig.client.call("fan_curve", args).unwrap();
    assert_eq!(raw["name"], json!("custom"));
    assert_eq!(raw["curve"], curve.to_value());
}

// ---- smart_profile ---------------------------------------------------------------------------

#[test]
fn smart_profile_get_has_python_s_three_keys_and_the_api_reads_them() {
    let rig = Rig::new();
    let raw = rig.client.call("smart_profile", Args::new().with("action", "get")).unwrap();
    let object = raw.as_object().unwrap();
    let mut keys = object.keys().cloned().collect::<Vec<_>>();
    keys.sort();
    assert_eq!(keys, ["name", "names", "profile"]);
    assert_eq!(object["name"], json!("native"));
    assert!(object["profile"].get("ac_idle").is_some(), "{}", object["profile"]);
    let in_use = rig.api.smart_profile_in_use().unwrap();
    assert_eq!(in_use.name, "native");
    assert_eq!(rig.api.smart_profiles().unwrap().current.as_deref(), Some("native"));
    assert_eq!(in_use.names, rig.api.smart_profiles().unwrap().names);
}

#[test]
fn smart_profiles_are_chosen_by_name_or_sent_as_data() {
    let rig = Rig::new();
    assert_eq!(rig.api.use_smart_profile("truesense").unwrap(), "truesense");
    let st = rig.api.daemon_state().unwrap();
    assert_eq!(st.setting_str("smart_profile"), Some("truesense"));
    assert_eq!(st.settings["smart_profile_data"], Value::Null);
    // A saved one travels as data (the daemon cannot read the user's directory).
    rig.api.save_smart_profile("mine", Some("truesense"), &["ac_idle=eco".to_owned()]).unwrap();
    assert_eq!(rig.api.use_smart_profile("mine").unwrap(), "mine");
    let st = rig.api.daemon_state().unwrap();
    assert_eq!(st.setting_str("smart_profile"), Some("mine"));
    assert_eq!(st.settings["smart_profile_data"]["ac_idle"], json!("eco"));
    assert_eq!(rig.api.smart_profile_in_use().unwrap().profile.to_json()["ac_idle"], json!("eco"));
    // The window's editor: settings that are not saved at all.
    let settings = rig.api.smart_profile("truesense").unwrap().to_json();
    assert_eq!(rig.api.use_smart_profile_data("scratch", &settings).unwrap(), "scratch");
    assert_eq!(rig.api.daemon_state().unwrap().setting_str("smart_profile"), Some("scratch"));
    rig.ticks(2);
    assert_eq!(rig.api.daemon_state().unwrap().live_str("smart_profile"), Some("scratch"));
}

#[test]
fn smart_profile_refuses_in_python_s_words() {
    let rig = Rig::new();
    let err = rig.client.call("smart_profile", Args::new().with("action", "frob")).unwrap_err();
    assert!(matches!(&err, ClientError::Daemon(t) if t == "smart_profile: get | use"), "{err:?}");
    assert!(rig.api.use_smart_profile("no-such-profile").is_err());
    let bad = Args::new().with("action", "use").with("name", "x").with("data", json!({"ac_idle": "warp"}));
    assert!(rig.client.call("smart_profile", bad).is_err());
    assert_eq!(rig.api.daemon_state().unwrap().setting_str("smart_profile"), Some("native"));
}

// ---- the daemon going away ---------------------------------------------------------------------

#[test]
fn stopping_the_service_hands_the_fans_back_and_the_clients_notice() {
    let rig = Rig::new();
    rig.api.fan_set(50, Some(50), false).unwrap();
    assert_eq!(rig.sim.snapshot().fan, SimFan::Manual);
    rig.shutdown().unwrap();
    // Python's `shutdown`: the fans go back to the firmware on any way out.
    assert_eq!(rig.sim.snapshot().fan, SimFan::Auto);
    assert!(!rig.sock.exists(), "the socket file is removed");
    assert!(!rig.api.daemon_available());
    assert!(rig.api.daemon_state().unwrap_err().daemon_missing());
    // A stop is refused without the service, whatever the confirmation.
    assert_eq!(rig.api.fan_set(0, None, true).unwrap_err().refusal(), Some(&Refusal::StopNeedsService));
    // What is safe without it falls back to the firmware.
    assert_eq!(rig.api.fan_max().unwrap().route, FanRoute::Hardware);
    assert_eq!(rig.sim.snapshot().fan, SimFan::Max);
}

#[test]
fn a_service_that_stops_while_a_fan_is_stopped_gives_it_back_too() {
    let rig = Rig::new();
    rig.api.fan_off(true).unwrap();
    assert_eq!(rig.sim.snapshot().duty, [0, 0]);
    rig.shutdown().unwrap();
    assert_eq!(rig.sim.snapshot().fan, SimFan::Auto);
}
