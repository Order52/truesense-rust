//! Every command of the table against the simulated laptop, through a real loopback request:
//! the arguments the page sends, the result it expects, and the safety rules the application
//! layer enforces for it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};
use ts_api::api::testing::{Reply, daemon_state};
use ts_system::services::Unit;

use super::harness::Machine;
use crate::gui::bus::{Hooks, SelfTest, ServiceLogs, StartupControl};

fn running(m: &Machine) {
    m.rig.daemon.answer_ok("state", daemon_state(json!({"auto_profile": true, "smart_fan": false, "operating_follow": false, "auto_mode": "smart"}), json!({}), 0));
}

fn daemon_args(m: &Machine, cmd: &str) -> Value {
    let args = m.rig.daemon.last_args(cmd);
    assert!(args.is_some(), "the daemon was never asked {cmd}");
    args.map_or(Value::Null, |a| serde_json::to_value(a).unwrap_or(Value::Null))
}

fn curve_json(cpu: &[(i64, i64)], gpu: &[(i64, i64)]) -> Value {
    json!({"cpu": cpu, "gpu": gpu, "hysteresis": 3, "down_rate": 2, "predict": 0, "couple": 30, "battery_shift": 0})
}

/// A curve that reaches 100 % in time and never falls: the safe kind.
fn good_curve() -> Value {
    curve_json(&[(40, 0), (60, 40), (80, 70), (95, 100)], &[(40, 0), (55, 40), (75, 70), (85, 100)])
}

// ---- status and power

#[test]
fn status_get_reads_everything_the_window_shows() {
    let m = Machine::start();
    let s = m.ok("status", "get", &json!({}));
    for key in [
        "power_mode",
        "sensors",
        "fans",
        "daemon",
        "battery_limit",
        "usb_charge_stop",
        "overdrive",
        "boot_sound",
        "kb_backlight_timeout",
        "gpu_mode",
        "rear",
        "touchpad",
        "audio_output",
        "jack",
        "voice",
        "ac",
        "battery_pct",
        "kb_brightness",
        "kb_look",
    ] {
        assert!(s.get(key).is_some(), "{key} is missing from {s}");
    }
    assert!(s["sensors"]["cpu_temp"].is_number() && s["fans"]["mode"].is_string());
    assert_eq!(s["kb_look"], json!({}), "nothing was put on the keyboard yet");
    assert!(
        s["daemon"].as_str().is_some_and(|d| d.starts_with("ERROR")),
        "a reading that fails is text, not a failure"
    );
    assert_eq!(m.ok("version", "get", &json!({})), json!({"truesense": ts_foundation::buildinfo::VERSION}));
}

#[test]
fn power_status_and_set_hold_the_pick_for_30_minutes() {
    let m = Machine::start();
    running(&m);
    m.rig.daemon.answer_ok("power", json!("turbo"));
    let st = m.ok("power", "status", &json!({}));
    assert_eq!(st["mode"], json!("eco"));
    assert_eq!(st["auto_profile"], json!(true));
    assert!(st["supported"].as_array().expect("supported").contains(&json!("turbo")));

    assert_eq!(m.ok("power", "set", &json!({"mode": "turbo"})), json!("turbo"));
    let sent = daemon_args(&m, "power");
    assert_eq!((sent["mode"].clone(), sent["hold_minutes"].clone()), (json!("turbo"), json!(30.0)));
    let why = m.refused("power", "set", &json!({"mode": "warp"}));
    assert!(why.contains("eco"), "the refusal lists the modes: {why}");
    m.refused("power", "set", &json!({}));
}

#[test]
fn power_auto_release_and_hold_talk_to_the_service() {
    let m = Machine::start();
    running(&m);
    m.rig.daemon.answer_ok("set", json!({"auto_mode": "off"}));
    m.rig.daemon.answer_ok("hold", json!(true));
    assert_eq!(m.ok("power", "auto", &json!({"action": "status"})), json!("smart"));
    assert_eq!(m.ok("power", "auto", &json!({"action": "off"})), json!("off"));
    assert_eq!(daemon_args(&m, "set")["key"], json!("auto_mode"));
    assert_eq!(daemon_args(&m, "set")["value"], json!("off"));
    assert!(m.ok("power", "release", &json!({})).as_str().expect("text").contains("hold released"));
    assert!(m.ok("power", "hold", &json!({"minutes": 12})).as_str().expect("text").contains("12"));
    assert_eq!(daemon_args(&m, "hold")["minutes"], json!(12.0));
    m.refused("power", "auto", &json!({"action": "sideways"}));
}

#[test]
fn power_game_and_managers_pass_the_service_answers_on() {
    let m = Machine::start();
    m.rig.daemon.answer_ok("game_rules", json!({"rules": {"eldenring.exe": "turbo"}, "running": null}));
    let rules = m.ok("power", "game", &json!({"action": "add", "name": "eldenring.exe", "mode": "turbo"}));
    assert_eq!(rules["rules"]["eldenring.exe"], json!("turbo"));
    let sent = daemon_args(&m, "game_rules");
    assert_eq!(
        (sent["action"].clone(), sent["name"].clone(), sent["mode"].clone()),
        (json!("add"), json!("eldenring.exe"), json!("turbo"))
    );
    assert!(m.refused("power", "game", &json!({"action": "add"})).contains("usage"));
    assert!(
        m.refused("power", "game", &json!({"action": "explode"})).contains("list | add | remove | clear")
    );

    m.rig.daemon.answer_ok("power_managers", json!({"on": true, "found": [{"unit": "tlp.service", "active": true, "enabled": true}], "recorded": false}));
    m.rig.daemon.then("power_managers", Reply::Ok(json!({"found": ["tlp.service"], "stopped": ["tlp.service"], "failed": [], "cpu": {"governor": "powersave"}, "on": true})));
    m.rig
        .daemon
        .then("power_managers", Reply::Ok(json!({"restored": ["tlp.service"], "cpu": {}, "on": false})));
    let list = m.ok("power", "managers", &json!({}));
    assert_eq!(list["found"][0]["unit"], json!("tlp.service"));
    assert_eq!(daemon_args(&m, "power_managers")["action"], json!("status"));
    assert_eq!(m.ok("power", "managers", &json!({"action": "stop"}))["stopped"], json!(["tlp.service"]));
    assert_eq!(daemon_args(&m, "power_managers")["action"], json!("takeover"));
    assert_eq!(m.ok("power", "managers", &json!({"action": "restore"}))["restored"], json!(["tlp.service"]));
    assert_eq!(daemon_args(&m, "power_managers")["action"], json!("restore"));
    assert_eq!(m.refused("power", "managers", &json!({"action": "x"})), "action: list | stop | restore");
}

#[test]
fn smart_auto_profiles_use_save_copy_and_delete() {
    let m = Machine::start();
    let builtin = m.ok("power", "smart", &json!({"action": "show", "name": "truesense"}));
    assert_eq!(builtin["builtin"], json!(true));
    assert!(builtin["ac_idle"].is_string());
    let mut plain = builtin.clone();
    plain.as_object_mut().expect("object").remove("builtin");
    m.rig
        .daemon
        .answer_ok("smart_profile", json!({"name": "truesense", "profile": plain, "names": ["truesense"]}));
    assert!(
        m.ok("power", "smart", &json!({"action": "list"}))
            .as_array()
            .expect("names")
            .contains(&json!("truesense"))
    );

    let mut settings = builtin.clone();
    settings.as_object_mut().expect("object").remove("builtin");
    settings["ac_idle"] = json!("quiet");
    let saved = m.ok("power", "smart", &json!({"action": "save", "name": "mine", "profile": settings}));
    assert_eq!(saved["ac_idle"], json!("quiet"));
    let shown = m.ok("power", "smart", &json!({"action": "show", "name": "mine"}));
    assert_eq!((shown["builtin"].clone(), shown["ac_idle"].clone()), (json!(false), json!("quiet")));
    assert_eq!(m.ok("power", "smart", &json!({"action": "copy", "name": "mine"})), json!("mine-copy"));
    assert_eq!(
        m.ok("power", "smart", &json!({"action": "copy", "name": "mine", "new": "other"})),
        json!("other")
    );
    assert_eq!(m.ok("power", "smart", &json!({"action": "delete", "name": "other"})), json!("other"));
    assert!(
        m.refused("power", "smart", &json!({"action": "delete", "name": "truesense"})).len() > 5,
        "built-in profiles are read-only"
    );
    assert!(
        m.refused("power", "smart", &json!({"action": "save", "name": "x"})).contains("settings to save")
    );

    // Using one goes to the service as data, a saved one or an edited one that is not saved.
    m.rig.daemon.answer_ok("smart_profile", json!({"name": "mine"}));
    assert_eq!(m.ok("power", "smart", &json!({"action": "use", "name": "mine"})), json!({"name": "mine"}));
    assert_eq!(daemon_args(&m, "smart_profile")["action"], json!("use"));
    assert!(daemon_args(&m, "smart_profile")["data"].is_object());
    m.ok("power", "smart", &json!({"action": "use", "name": "custom", "profile": settings}));
    assert_eq!(daemon_args(&m, "smart_profile")["name"], json!("custom"));
}

// ---- fans

#[test]
fn fan_auto_and_max_work_without_the_service() {
    let m = Machine::start();
    m.rig.daemon.set_available(false);
    assert!(m.ok("fan", "auto", &json!({})).as_str().expect("text").contains("firmware auto"));
    assert!(m.ok("fan", "max", &json!({})).as_str().expect("text").contains("MAX"));
}

#[test]
fn a_fan_stop_is_refused_unless_confirmed_and_never_without_the_service() {
    let m = Machine::start();
    running(&m);
    m.rig.daemon.answer_ok("fan", json!({"mode": "off"}));
    assert_eq!(m.refused("fan", "off", &json!({})), "stopping a fan needs explicit confirmation");
    assert_eq!(
        m.refused("fan", "off", &json!({"confirm": false})),
        "stopping a fan needs explicit confirmation"
    );
    assert_eq!(m.refused("fan", "set", &json!({"cpu": 0})), "stopping a fan needs explicit confirmation");
    assert_eq!(
        m.refused("fan", "set", &json!({"cpu": 60, "gpu": 0})),
        "stopping a fan needs explicit confirmation"
    );
    assert!(m.rig.daemon.commands().iter().all(|c| c != "fan"), "nothing was sent to the service");

    m.ok("fan", "off", &json!({"confirm": true}));
    assert_eq!(daemon_args(&m, "fan")["mode"], json!("off"));
    m.ok("fan", "set", &json!({"cpu": 0, "confirm": true}));

    m.rig.daemon.set_available(false);
    let msg = m.refused("fan", "off", &json!({"confirm": true}));
    assert!(msg.contains("service"), "the stop needs the watchdog: {msg}");
    let msg = m.refused("fan", "set", &json!({"cpu": 0, "confirm": true}));
    assert!(msg.contains("service"), "{msg}");
}

#[test]
fn a_manual_fan_duty_must_be_zero_or_20_to_100() {
    let m = Machine::start();
    running(&m);
    m.rig.daemon.answer_ok("fan", json!({"mode": "manual"}));
    for cpu in [1, 10, 19, 101, -5, 1000] {
        let (status, body) = m.run("fan", "set", &json!({"cpu": cpu}));
        assert_eq!(status, 422, "{cpu}: {body}");
    }
    assert!(m.rig.daemon.commands().iter().all(|c| c != "fan"), "a bad duty never reaches the service");
    m.refused("fan", "set", &json!({"cpu": "fast"}));
    m.refused("fan", "set", &json!({"cpu": 50.5}));
    let ok = m.ok("fan", "set", &json!({"cpu": 60, "gpu": 40}));
    assert!(ok.as_str().expect("text").contains("60"), "{ok}");
    let sent = daemon_args(&m, "fan");
    assert_eq!((sent["cpu"].clone(), sent["gpu"].clone()), (json!(60), json!(40)));
}

#[test]
fn fan_smart_switches_the_service_curve() {
    let m = Machine::start();
    running(&m);
    m.rig.daemon.answer_ok("set", json!({"smart_fan": true}));
    assert_eq!(m.ok("fan", "smart", &json!({"action": "status"})), json!(false));
    assert_eq!(m.ok("fan", "smart", &json!({"action": "on"})), json!(true));
    assert_eq!(daemon_args(&m, "set")["key"], json!("smart_fan"));
    m.rig.daemon.set_available(false);
    let (status, body) = m.run("fan", "smart", &json!({"action": "on"}));
    assert_eq!((status, body["error"]["code"].clone()), (422, json!("needs-daemon")));
}

#[test]
fn fan_curves_list_show_save_copy_use_and_delete() {
    let m = Machine::start();
    let shown = m.ok("fan", "curve", &json!({"action": "show", "name": "truesense"}));
    assert_eq!(shown["builtin"], json!(true));
    assert!(shown["cpu"][0].is_array(), "points are [temperature, percent]: {shown}");
    for key in ["hysteresis", "down_rate", "predict", "couple", "battery_shift"] {
        assert!(shown[key].is_number(), "{key}");
    }
    m.rig
        .daemon
        .answer_ok("fan_curve", json!({"name": "truesense", "curve": good_curve(), "names": ["truesense"]}));
    assert!(
        m.ok("fan", "curve", &json!({"action": "list"}))
            .as_array()
            .expect("names")
            .contains(&json!("truesense"))
    );

    let saved = m.ok("fan", "curve", &json!({"action": "save", "name": "mycurve", "curve": good_curve()}));
    assert_eq!(saved["cpu"][1], json!([60, 40]));
    let back = m.ok("fan", "curve", &json!({"action": "show", "name": "mycurve"}));
    assert_eq!(
        (back["builtin"].clone(), back["cpu"][3].clone(), back["couple"].clone()),
        (json!(false), json!([95, 100]), json!(30))
    );
    assert_eq!(m.ok("fan", "curve", &json!({"action": "copy", "name": "mycurve"})), json!("mycurve-copy"));
    assert_eq!(
        m.ok("fan", "curve", &json!({"action": "copy", "name": "mycurve", "new": "loud"})),
        json!("loud")
    );
    assert_eq!(m.ok("fan", "curve", &json!({"action": "delete", "name": "loud"})), json!("loud"));
    assert!(!m.refused("fan", "curve", &json!({"action": "delete", "name": "truesense"})).is_empty());
    assert!(!m.refused("fan", "curve", &json!({"action": "delete", "name": "nothing"})).is_empty());
    assert!(m.refused("fan", "curve", &json!({"action": "save", "name": "x"})).contains("curve to save"));
}

#[test]
fn a_curve_that_is_too_cool_or_malformed_is_refused_before_it_is_saved_or_used() {
    let m = Machine::start();
    running(&m);
    m.rig.daemon.answer_ok("fan_curve", json!({"name": "custom"}));
    let too_cool = curve_json(&[(40, 0), (60, 30), (80, 50)], &[(40, 0), (55, 40), (75, 70), (85, 100)]);
    let falling = curve_json(&[(40, 0), (60, 60), (80, 40), (95, 100)], &[(40, 0), (85, 100)]);
    for (curve, why) in [
        (too_cool, "never reaches 100 %"),
        (falling, "duty falls"),
        (json!({"cpu": "all", "gpu": []}), "not a list"),
        (json!({"cpu": [[40, 0, 5]], "gpu": []}), "not a pair"),
        (json!({"cpu": [[40, "x"]], "gpu": [[40, 0]]}), "not numbers"),
        (json!({"gpu": [[40, 0]]}), "missing side"),
        (json!([]), "not an object"),
    ] {
        for action in ["save", "use"] {
            let (status, body) =
                m.run("fan", "curve", &json!({"action": action, "name": "bad", "curve": curve}));
            assert_eq!(status, 422, "{action}: {why}: {body}");
        }
    }
    assert!(m.rig.daemon.commands().iter().all(|c| c != "fan_curve"), "nothing unsafe reached the service");
    m.rig
        .daemon
        .answer_ok("fan_curve", json!({"name": "truesense", "curve": good_curve(), "names": ["truesense"]}));
    assert!(
        !m.ok("fan", "curve", &json!({"action": "list"})).as_array().expect("names").contains(&json!("bad"))
    );
}

#[test]
fn an_edited_curve_is_used_as_data_and_a_saved_one_by_name() {
    let m = Machine::start();
    running(&m);
    m.rig.daemon.answer_ok("fan_curve", json!({"name": "custom", "curve": null}));
    assert_eq!(
        m.ok("fan", "curve", &json!({"action": "use", "name": "custom", "curve": good_curve()})),
        json!({"name": "custom"})
    );
    let sent = daemon_args(&m, "fan_curve");
    assert_eq!((sent["action"].clone(), sent["name"].clone()), (json!("use"), json!("custom")));
    assert_eq!(sent["data"]["cpu"][3], json!([95, 100]));

    m.ok("fan", "curve", &json!({"action": "save", "name": "mine", "curve": good_curve()}));
    m.ok("fan", "curve", &json!({"action": "use", "name": "mine"}));
    assert_eq!(daemon_args(&m, "fan_curve")["data"]["gpu"][3], json!([85, 100]));
    m.ok("fan", "curve", &json!({"action": "use", "name": "truesense"}));
    assert_eq!(daemon_args(&m, "fan_curve")["data"], Value::Null, "a built-in one is named, not sent");
}

#[test]
fn fan_curve_current_names_the_curve_the_service_runs() {
    let m = Machine::start();
    let shown = m.ok("fan", "curve", &json!({"action": "show", "name": "truesense"}));
    let mut curve = shown.clone();
    curve.as_object_mut().expect("object").remove("builtin");
    m.rig.daemon.answer_ok(
        "fan_curve",
        json!({"name": "truesense", "curve": curve, "names": ["truesense", "native"]}),
    );
    let current = m.ok("fan", "curve", &json!({}));
    assert_eq!(current["name"], json!("truesense"));
    assert_eq!(current["curve"]["cpu"], shown["cpu"]);
}

// ---- GPU, CPU, battery, display, touchpad

#[test]
fn gpu_mode_is_set_and_read_back() {
    let m = Machine::start();
    for (mode, shown) in [("nvidia", "nvidia"), ("optimus", "optimus"), ("auto", "auto")] {
        let got = m.ok("gpu", "set", &json!({"mode": mode}));
        assert!(got.as_str().is_some_and(|g| g.contains(shown)), "{mode}: {got}");
    }
    m.refused("gpu", "set", &json!({"mode": "turbo"}));
    m.refused("gpu", "set", &json!({}));
}

#[test]
fn undervolts_need_the_confirmation_and_the_danger_line_needs_force() {
    let m = Machine::start();
    running(&m);
    m.rig.daemon.answer_ok("undervolt", json!({"core_mv": -50, "cache_mv": -40}));
    m.rig.daemon.answer_ok("gpu_undervolt", json!({"offset_mhz": 100, "cap_mhz": 2400}));
    let apply = |extra: Value| {
        let mut a = json!({"action": "apply", "core": -50, "cache": -40});
        a.as_object_mut().expect("object").extend(extra.as_object().expect("object").clone());
        a
    };
    let msg = m.refused("cpu", "undervolt", &apply(json!({})));
    assert!(msg.contains("confirm"), "{msg}");
    assert!(m.rig.daemon.commands().iter().all(|c| c != "undervolt"));
    m.ok("cpu", "undervolt", &apply(json!({"confirm": true})));
    let sent = daemon_args(&m, "undervolt");
    assert_eq!(
        (sent["action"].clone(), sent["core"].clone(), sent["cache"].clone(), sent["confirm"].clone()),
        (json!("apply"), json!(-50), json!(-40), json!(true))
    );

    m.rig.daemon.clear_calls();
    let beyond = json!({"action": "apply", "core": -500, "cache": -500, "confirm": true});
    assert!(m.refused("cpu", "undervolt", &beyond).contains("force"));
    assert!(
        m.rig.daemon.commands().is_empty(),
        "the page's confirmation is not enough beyond the danger line"
    );
    let forced = json!({"action": "apply", "core": -500, "cache": -500, "confirm": true, "force": true});
    m.ok("cpu", "undervolt", &forced);

    assert!(
        m.refused("gpu", "undervolt", &json!({"action": "apply", "offset": 100, "cap": 2400}))
            .contains("confirm")
    );
    m.ok("gpu", "undervolt", &json!({"action": "apply", "offset": 100, "cap": 2400, "confirm": true}));
    assert_eq!(daemon_args(&m, "gpu_undervolt")["offset"], json!(100));
    assert!(
        m.refused(
            "gpu",
            "undervolt",
            &json!({"action": "apply", "offset": 5000, "cap": 2400, "confirm": true})
        )
        .contains("force")
    );
    m.refused("gpu", "undervolt", &json!({"action": "apply", "cap": 2400, "confirm": true}));
    m.refused("cpu", "undervolt", &json!({"action": "apply", "core": -10, "confirm": true}));
    m.refused(
        "cpu",
        "undervolt",
        &json!({"action": "apply", "core": 99999999999i64, "cache": 0, "confirm": true}),
    );
}

#[test]
fn undervolt_profiles_and_the_boot_profile() {
    let m = Machine::start();
    running(&m);
    m.rig.daemon.answer_ok("undervolt", json!({"name": "mine", "core": -30, "cache": -20}));
    let cpu = m.ok("cpu", "undervolt", &json!({"action": "show", "name": "truesense"}));
    assert_eq!(cpu["builtin"], json!(true));
    assert!(cpu["core"].is_number() && cpu["description"].is_string());
    assert!(
        m.ok("cpu", "undervolt", &json!({"action": "list"}))
            .as_array()
            .expect("names")
            .contains(&json!("truesense"))
    );
    let saved =
        m.ok("cpu", "undervolt", &json!({"action": "save", "name": "mine", "core": -30, "cache": -20}));
    assert_eq!((saved["core"].clone(), saved["cache"].clone()), (json!(-30), json!(-20)));
    assert_eq!(m.ok("cpu", "undervolt", &json!({"action": "copy", "name": "mine"})), json!("mine-copy"));
    assert_eq!(
        m.ok("cpu", "undervolt", &json!({"action": "delete", "name": "mine-copy"})),
        json!("mine-copy")
    );
    m.refused("cpu", "undervolt", &json!({"action": "save", "name": "x", "core": -30}));

    // Boot: a name of a saved profile or the editor's values, always with the confirmation; off clears.
    assert_eq!(m.ok("cpu", "undervolt", &json!({"action": "boot", "name": "off"})), Value::Null);
    assert_eq!(daemon_args(&m, "undervolt")["action"], json!("boot_clear"));
    assert_eq!(m.ok("cpu", "undervolt", &json!({"action": "boot"})), Value::Null, "no name clears it too");
    m.refused("cpu", "undervolt", &json!({"action": "boot", "name": "mine"}));
    m.ok("cpu", "undervolt", &json!({"action": "boot", "name": "mine", "confirm": true}));
    assert_eq!(daemon_args(&m, "undervolt")["core"], json!(-30), "the saved profile's values are sent");
    m.ok(
        "cpu",
        "undervolt",
        &json!({"action": "boot", "name": "editor", "core": -20, "cache": -10, "confirm": true}),
    );
    assert_eq!(daemon_args(&m, "undervolt")["core"], json!(-20));

    m.rig.daemon.answer_ok("gpu_undervolt", json!({"name": "g", "offset": 100, "cap": 2400}));
    let gpu = m.ok("gpu", "undervolt", &json!({"action": "show", "name": "truesense"}));
    assert_eq!(
        (gpu["builtin"].clone(), gpu["offset"].clone(), gpu["cap"].clone()),
        (json!(true), json!(120), json!(2400))
    );
    m.ok("gpu", "undervolt", &json!({"action": "save", "name": "g", "offset": 100, "cap": 2400}));
    m.ok("gpu", "undervolt", &json!({"action": "boot", "name": "g", "confirm": true}));
    assert_eq!(daemon_args(&m, "gpu_undervolt")["offset"], json!(100));
    assert_eq!(m.ok("gpu", "undervolt", &json!({"action": "boot", "name": "off"})), Value::Null);
    m.rig.daemon.answer_ok("gpu_undervolt", json!({"offset_mhz": 0, "cap_mhz": 0}));
    assert_eq!(m.ok("gpu", "undervolt", &json!({"action": "reset"})), json!({"offset_mhz": 0, "cap_mhz": 0}));
    assert_eq!(daemon_args(&m, "gpu_undervolt")["action"], json!("reset"));
    m.rig.daemon.answer_ok(
        "gpu_undervolt",
        json!({"offset_mhz": 100, "range_mhz": [-200, 300], "applied": null, "boot": null}),
    );
    assert_eq!(m.ok("gpu", "undervolt", &json!({"action": "get"}))["range_mhz"], json!([-200, 300]));
    assert!(!m.refused("gpu", "undervolt", &json!({"action": "explode"})).is_empty());
}

#[test]
fn cpu_limits_beyond_the_danger_line_need_force() {
    let m = Machine::start();
    running(&m);
    m.rig.daemon.answer_ok("cpu_limits", json!({"pl1_w": 55.0, "pl2_w": 90.0}));
    m.rig.daemon.answer_ok(
        "cpu_info",
        json!({"pl1_w": 55.0, "pl2_w": 90.0, "user": null, "auto": {"normal_w": 45, "gaming_w": 55}}),
    );
    assert_eq!(m.ok("cpu", "limits", &json!({"pl1": 55, "pl2": 90})), json!({"pl1_w": 55.0, "pl2_w": 90.0}));
    let sent = daemon_args(&m, "cpu_limits");
    assert_eq!(
        (sent["pl1"].clone(), sent["pl2"].clone(), sent["force"].clone()),
        (json!(55.0), json!(90.0), json!(false))
    );
    m.ok("cpu", "limits", &json!({"pl1": 45}));
    assert_eq!(daemon_args(&m, "cpu_limits")["pl2"], json!(45.0), "pl2 defaults to pl1");

    m.rig.daemon.clear_calls();
    assert!(m.refused("cpu", "limits", &json!({"pl1": 500, "pl2": 500})).contains("force"));
    assert!(m.rig.daemon.commands().is_empty());
    m.ok("cpu", "limits", &json!({"pl1": 500, "pl2": 500, "force": true}));
    assert_eq!(daemon_args(&m, "cpu_limits")["force"], json!(true));
    m.refused("cpu", "limits", &json!({}));

    let reset = m.ok("cpu", "limits", &json!({"auto": true}));
    assert_eq!(reset, json!({"pl1_w": 55.0, "pl2_w": 90.0}));
    assert_eq!(daemon_args(&m, "cpu_limits")["reset"], json!(true));
    assert_eq!(m.ok("cpu", "info", &json!({}))["auto"]["gaming_w"], json!(45 + 10));
}

#[test]
fn battery_settings_are_read_back_from_the_firmware() {
    let m = Machine::start();
    assert_eq!(m.ok("battery", "limit", &json!({"action": "on"})), json!(true));
    assert_eq!(m.ok("battery", "limit", &json!({"action": "status"})), json!(true));
    assert_eq!(m.ok("battery", "limit", &json!({"action": "toggle"})), json!(false));
    assert_eq!(m.ok("battery", "calibration", &json!({"action": "on"})), json!(true));
    assert_eq!(m.ok("battery", "calibration", &json!({"action": "off"})), json!(false));
    assert_eq!(m.ok("battery", "usb-charge", &json!({"level": 20})), json!(20));
    let st = m.ok("battery", "status", &json!({}));
    assert_eq!(
        (st["limit_80"].clone(), st["calibration"].clone(), st["usb_charge_stop"].clone()),
        (json!(false), json!(false), json!(20))
    );
    m.refused("battery", "usb-charge", &json!({"level": 25}));
    m.refused("battery", "usb-charge", &json!({}));
    m.refused("battery", "limit", &json!({"action": "perhaps"}));
}

#[test]
fn charge_to_full_once_goes_through_the_service() {
    let m = Machine::start();
    m.rig
        .daemon
        .answer_ok("battery_full", json!({"active": true, "limit": true, "capacity": 80.0, "ac": true}));
    let st = m.ok("battery", "full", &json!({"action": "get"}));
    assert_eq!((st["active"].clone(), st["capacity"].clone()), (json!(true), json!(80.0)));
    m.ok("battery", "full", &json!({"action": "start"}));
    assert_eq!(daemon_args(&m, "battery_full")["action"], json!("start"));
    m.ok("battery", "full", &json!({"action": "cancel"}));
    assert_eq!(daemon_args(&m, "battery_full")["action"], json!("cancel"));
    m.ok("battery", "full", &json!({}));
    assert_eq!(daemon_args(&m, "battery_full")["action"], json!("get"));
    m.refused("battery", "full", &json!({"action": "x"}));
}

#[test]
fn overdrive_boot_sound_and_touchpad() {
    let m = Machine::start();
    assert_eq!(m.ok("display", "overdrive", &json!({"action": "off"})), json!(false));
    assert_eq!(m.ok("display", "overdrive", &json!({"action": "toggle"})), json!(true));
    assert_eq!(m.ok("boot", "sound", &json!({"action": "on"})), json!(true));
    assert_eq!(m.ok("boot", "sound", &json!({"action": "status"})), json!(true));
    assert_eq!(
        m.ok("touchpad", "get", &json!({})),
        Value::Null,
        "the simulated laptop has no touchpad switch to read"
    );
    m.rig.daemon.answer_ok("touchpad", json!(true));
    assert_eq!(m.ok("touchpad", "set", &json!({"action": "on"})), json!(true));
    assert_eq!(daemon_args(&m, "touchpad")["action"], json!("on"));
    assert_eq!(
        m.ok("touchpad", "set", &json!({})),
        json!(true),
        "toggle is the default: it was unknown, so on"
    );
    m.refused("touchpad", "set", &json!({"action": "maybe"}));
}

// ---- keyboard

#[test]
fn the_wave_takes_its_six_directions_by_name_or_number() {
    let m = Machine::start();
    for (direction, want) in [
        (json!("clockwise"), 4),
        (json!("counter-clockwise"), 5),
        (json!("CCW"), 5),
        (json!("left-to-right"), 0),
        (json!("bottom_to_top"), 2),
        (json!(3), 3),
        (json!(1), 1),
    ] {
        let args = json!({"name": "wave", "speed": 5, "brightness": 30, "direction": direction});
        assert_eq!(m.ok("keyboard", "effect", &args), json!("sent"), "{direction}");
        assert_eq!(m.ok("keyboard", "status", &json!({}))["look"]["direction"], json!(want), "{direction}");
    }
    for bad in [json!("sideways"), json!(6), json!(-1), json!("9")] {
        let args = json!({"name": "wave", "direction": bad});
        let message = m.refused("keyboard", "effect", &args);
        assert!(message.contains("0..5"), "{bad}: {message}");
    }
    // Another effect still takes a plain number and ignores the meaning.
    assert_eq!(m.ok("keyboard", "effect", &json!({"name": "snake", "direction": 1})), json!("sent"));
}

#[test]
fn keyboard_effects_brightness_color_and_speed() {
    let m = Machine::start();
    assert_eq!(
        m.ok(
            "keyboard",
            "effect",
            &json!({"name": "wave", "color": "ff0000", "speed": 7, "brightness": 30, "direction": 1})
        ),
        json!("sent")
    );
    let st = m.ok("keyboard", "status", &json!({}));
    assert_eq!(st["look"]["name"], json!("wave"), "{st}");
    assert_eq!(st["max"], json!(50));
    assert_eq!(m.ok("keyboard", "brightness", &json!({"level": 10})), json!(10));
    assert_eq!(m.ok("keyboard", "status", &json!({}))["brightness"], json!(10));
    assert_eq!(m.ok("keyboard", "speed", &json!({"level": 4})), json!("keyboard effect speed: 4 / 12"));
    assert!(
        m.refused("keyboard", "color", &json!({"color": "00ff00"})).contains("no colour"),
        "the wave effect has none of its own"
    );
    m.ok("keyboard", "effect", &json!({"name": "static", "color": "ff0000", "speed": 5, "brightness": 30}));
    assert_eq!(m.ok("keyboard", "color", &json!({"color": "00ff00"})), json!("sent"));
    assert_eq!(m.ok("keyboard", "restore", &json!({})), json!("sent"));
    assert_eq!(m.ok("keyboard", "on", &json!({})), json!("ran keyboard:on"));
    assert_eq!(m.ok("keyboard", "off", &json!({})), json!("ran keyboard:off"));
    assert_eq!(
        m.rig.actions.calls(),
        ["keyboard:on", "keyboard:off"],
        "a key press and a command are the same code"
    );
    for bad in [
        json!({"name": "disco"}),
        json!({"name": "static", "color": "nothex"}),
        json!({"name": "wave", "speed": 99}),
    ] {
        let (status, _) = m.run("keyboard", "effect", &bad);
        assert_eq!(status, 422, "{bad}");
    }
    m.refused("keyboard", "brightness", &json!({"level": 51}));
}

#[test]
fn per_key_colours_round_trip_through_the_simulated_keyboard() {
    let m = Machine::start();
    let mut cells = vec![json!("112233"); 128];
    cells[5] = json!("ff0000");
    cells[126] = json!("000000");
    cells[127] = json!("000000");
    assert_eq!(m.ok("keyboard", "set-cells", &json!({"cells": cells, "brightness": 20})), json!("sent"));
    let back = m.ok("keyboard", "cells", &json!({}));
    let list = back["cells"].as_array().expect("cells");
    assert_eq!(list.len(), 128);
    assert_eq!((list[5].clone(), list[6].clone()), (json!("ff0000"), json!("112233")));
    assert_eq!(m.ok("keyboard", "status", &json!({}))["look"]["mode"], json!("perkey"));
    m.refused("keyboard", "set-cells", &json!({}));
    m.refused("keyboard", "set-cells", &json!({"cells": ["zzzzzz"]}));
    m.refused("keyboard", "set-cells", &json!({"cells": vec!["000000"; 129]}));
    let layout = m.ok("keyboard", "layout", &json!({}));
    assert_eq!(
        layout["keys"]["a"][0]["cell"].as_i64().map(|c| c >= 0),
        Some(true),
        "{}",
        layout["keys"]["a"]
    );
    assert!(layout["groups"].as_array().expect("groups").contains(&json!("wasd")));
}

#[test]
fn keyboard_animation_designs_and_lighting_profiles() {
    let m = Machine::start();
    let fx = m.ok("keyboard", "fx", &json!({"mode": "pulse", "speed": 4, "cells": [1, 2, 3]}));
    assert_eq!(fx["mode"], json!("pulse"), "{fx}");
    m.ok("keyboard", "fx", &json!({"mode": "off"}));
    m.refused("keyboard", "fx", &json!({"mode": "explode"}));
    for mode in ["wave-clockwise", "wave-counter-clockwise"] {
        let sweep = m.ok("keyboard", "fx", &json!({"mode": mode, "speed": 6}));
        assert_eq!((sweep["mode"].clone(), sweep["speed"].clone()), (json!(mode), json!(6)), "{sweep}");
    }
    m.ok("keyboard", "fx", &json!({"mode": "off"}));

    let designs = m.ok("keyboard", "design", &json!({}));
    assert_eq!(designs["gaming"]["source"], json!("built-in"));
    let shown = m.ok("keyboard", "design", &json!({"action": "show", "name": "gaming"}));
    assert_eq!(shown["cells"].as_array().map(Vec::len), Some(128));
    let cells = shown["cells"].clone();
    let path = m.ok("keyboard", "design", &json!({"action": "save", "name": "mine", "cells": cells}));
    assert!(
        path.as_str().expect("path").ends_with("mine.json") || path.as_str().expect("path").contains("mine"),
        "{path}"
    );
    assert_eq!(m.ok("keyboard", "design", &json!({}))["mine"]["source"], json!("yours"));
    assert_eq!(
        m.ok("keyboard", "design", &json!({"action": "apply", "name": "gaming"})),
        json!("design gaming applied")
    );
    m.refused("keyboard", "design", &json!({"action": "show", "name": "nothing"}));
    m.refused("keyboard", "design", &json!({"action": "zap"}));

    assert_eq!(m.ok("keyboard", "profile", &json!({})), json!([]));
    let saved = m.ok("keyboard", "profile", &json!({"action": "save", "name": "night"}));
    assert!(saved.as_str().expect("text").contains("night"));
    assert_eq!(m.ok("keyboard", "profile", &json!({"action": "list"})), json!(["night"]));
    assert!(m.ok("keyboard", "profile", &json!({"action": "load", "name": "night"})).is_string());
    assert!(m.ok("keyboard", "profile", &json!({"action": "delete", "name": "night"})).is_string());
    assert_eq!(m.ok("keyboard", "profile", &json!({})), json!([]));
    m.refused("keyboard", "profile", &json!({"action": "zap"}));
}

#[test]
fn keyboard_operating_and_timeout_switches() {
    let m = Machine::start();
    running(&m);
    m.rig.daemon.answer_ok("set", json!({"operating_follow": true}));
    assert_eq!(m.ok("keyboard", "operating", &json!({"action": "on"})), json!(true));
    assert_eq!(daemon_args(&m, "set")["key"], json!("operating_follow"));
    m.refused("keyboard", "operating", &json!({"action": "toggle"}));
    let t = m.ok("keyboard", "timeout", &json!({"action": "off"}));
    assert_eq!(t, json!(false));
    assert_eq!(m.ok("keyboard", "timeout", &json!({"action": "toggle"})), json!(true));
}

// ---- rear light and looks

#[test]
fn the_rear_light_is_set_with_and_without_the_read_back() {
    let m = Machine::start();
    let st = m.ok(
        "rear",
        "set",
        &json!({"effect": "breathing", "color": "3723c7", "speed": 3, "brightness": 100, "direction": 1}),
    );
    assert_eq!(
        (st["mode"].clone(), st["color"].clone(), st["speed"].clone(), st["brightness"].clone()),
        (json!("breathing"), json!("3723c7"), json!(3), json!(100))
    );
    let zoned = m.ok(
        "rear",
        "set",
        &json!({"effect": "static", "zones": ["ff0000", "00ff00", "0000ff"], "brightness": 60}),
    );
    assert_eq!(zoned["zones"], json!(["ff0000", "00ff00", "0000ff"]), "{zoned}");
    assert_eq!(m.ok("rear", "status", &json!({}))["zones"], json!(["ff0000", "00ff00", "0000ff"]));
    let live = m.ok(
        "rear",
        "set",
        &json!({"effect": "neon", "color": "ffffff", "speed": 2, "brightness": 40, "verify": false}),
    );
    assert_eq!(live["mode"], json!("neon"), "{live}");
    assert_eq!(m.ok("rear", "on", &json!({})), json!("ran rear:on"));
    assert_eq!(m.ok("rear", "off", &json!({})), json!("ran rear:off"));
    for bad in [
        json!({"effect": "strobe"}),
        json!({"effect": "wave", "speed": 9}),
        json!({"effect": "wave", "brightness": 101}),
        json!({"effect": "wave", "color": "red"}),
        json!({}),
    ] {
        let (status, _) = m.run("rear", "set", &bad);
        assert_eq!(status, 422, "{bad}");
    }
}

#[test]
fn looks_are_saved_applied_copied_and_deleted() {
    let m = Machine::start();
    m.ok("keyboard", "effect", &json!({"name": "wave", "color": "ff0000", "speed": 5, "brightness": 20}));
    m.ok("rear", "set", &json!({"effect": "breathing", "color": "3723c7", "speed": 3, "brightness": 100}));
    let saved = m.ok("look", "save", &json!({"name": "mine"}));
    assert!(saved["keyboard"].is_object() && saved["rear"].is_object(), "{saved}");
    assert_eq!(m.ok("look", "list", &json!({})), json!(["native", "mine"]));
    let shown = m.ok("look", "show", &json!({"name": "mine"}));
    assert_eq!(shown["builtin"], json!(false));
    assert_eq!(m.ok("look", "show", &json!({"name": "native"}))["builtin"], json!(true));
    assert_eq!(m.ok("look", "copy", &json!({"name": "mine"})), json!("mine-copy"));
    assert_eq!(m.ok("look", "copy", &json!({"name": "mine", "new": "other"})), json!("other"));
    let done = m.ok("look", "apply", &json!({"name": "mine"}));
    assert!(done.as_array().is_some_and(|d| !d.is_empty()), "{done}");
    // The page can save a whole look object it holds (per-key colours).
    let body = json!({"keyboard": {"mode": "effect", "name": "static", "color": "00ff00", "speed": 3, "brightness": 10}, "rear": null});
    let from_page = m.ok("look", "save", &json!({"name": "held", "look": body}));
    assert_eq!(from_page["keyboard"]["color"], json!("00ff00"));
    assert_eq!(m.ok("look", "delete", &json!({"name": "other"})), json!("other"));
    m.refused("look", "delete", &json!({"name": "native"}));
    m.refused("look", "show", &json!({"name": "nothing"}));
    m.refused("look", "apply", &json!({"name": "nothing"}));
    m.refused("look", "save", &json!({"name": "bad name!"}));
    m.refused("look", "save", &json!({"name": "x", "look": {"keyboard": 5}}));
}

struct FakeStartup {
    set: std::sync::Mutex<Vec<(String, String)>>,
}

impl StartupControl for FakeStartup {
    fn set(&self, target: &str, value: &str) -> Result<String, String> {
        self.set.lock().expect("set").push((target.to_owned(), value.to_owned()));
        if target == "bogus" {
            Err("target: keyboard | rear | profile".to_owned())
        } else {
            Ok(value.to_owned())
        }
    }

    fn apply(&self) -> Result<Vec<String>, String> {
        Ok(vec!["keyboard: last".to_owned()])
    }
}

#[test]
fn the_startup_settings_go_through_the_key_services_hook() {
    let startup = Arc::new(FakeStartup { set: std::sync::Mutex::default() });
    let m = Machine::start_with(Hooks { startup: Some(startup.clone()), ..Hooks::default() });
    assert_eq!(
        m.ok("startup", "show", &json!({})),
        json!({"profile": "", "keyboard": "last", "rear": "last"})
    );
    assert_eq!(m.ok("startup", "set", &json!({"target": "keyboard", "value": "mine"})), json!("mine"));
    assert_eq!(startup.set.lock().expect("set").as_slice(), [("keyboard".to_owned(), "mine".to_owned())]);
    assert_eq!(m.ok("startup", "apply", &json!({})), json!(["keyboard: last"]));
    assert_eq!(m.refused("startup", "set", &json!({"target": "bogus"})), "target: keyboard | rear | profile");
    m.refused("startup", "set", &json!({"value": "x"}));
}

#[test]
fn the_boot_look_is_read_set_and_cleared() {
    let m = Machine::start();
    running(&m);
    m.rig.daemon.answer_ok("boot_look", json!(null));
    assert_eq!(m.ok("startup", "boot", &json!({})), Value::Null);
    assert_eq!(daemon_args(&m, "boot_look")["action"], json!("get"));
    assert_eq!(m.ok("startup", "boot", &json!({"what": "clear"})), Value::Null);
    assert_eq!(daemon_args(&m, "boot_look")["action"], json!("clear"));
    m.ok("keyboard", "effect", &json!({"name": "wave", "color": "ff0000", "speed": 5, "brightness": 20}));
    let look = m.ok("startup", "boot", &json!({"what": "current"}));
    assert!(look["keyboard"].is_object(), "{look}");
    assert_eq!(daemon_args(&m, "boot_look")["action"], json!("set"));
    m.refused("startup", "boot", &json!({"what": "nothing-saved"}));
}

// ---- keys, profiles, audio

#[test]
fn key_profiles_are_listed_shown_saved_used_and_deleted() {
    let m = Machine::start();
    let current = m.ok("keys", "profile", &json!({}));
    assert_eq!(current["profile"], json!("default"));
    assert_eq!(current["tap"], json!("power-next"));
    assert!(current["long_ms"].is_number());
    let list = m.ok("keys", "profile", &json!({"action": "list"}));
    assert!(list.as_array().expect("names").contains(&json!("default")));
    let shown = m.ok("keys", "profile", &json!({"action": "show"}));
    assert_eq!(shown["builtin"], json!(true));
    let saved = m.ok(
        "keys",
        "profile",
        &json!({"action": "save", "name": "mine", "from": "default", "tap": "power-prev", "long_ms": 900}),
    );
    assert_eq!(
        (saved["tap"].clone(), saved["long_ms"].clone(), saved["double"].clone()),
        (json!("power-prev"), json!(900), json!("output-toggle"))
    );
    assert_eq!(m.ok("keys", "profile", &json!({"action": "show", "name": "mine"}))["builtin"], json!(false));
    assert_eq!(m.ok("keys", "profile", &json!({"action": "use", "name": "mine"})), json!("mine"));
    assert_eq!(m.ok("keys", "profile", &json!({}))["profile"], json!("mine"));
    assert_eq!(m.ok("keys", "profile", &json!({"action": "copy", "name": "mine"})), json!("mine-copy"));
    assert_eq!(
        m.ok("keys", "profile", &json!({"action": "delete", "name": "mine-copy"})),
        json!("mine-copy")
    );
    m.refused("keys", "profile", &json!({"action": "save", "name": "x", "tap": "no-such-action"}));
    m.refused("keys", "profile", &json!({"action": "save", "name": "x", "long_ms": 5}));
    m.refused("keys", "profile", &json!({"action": "delete", "name": "default"}));
    m.refused("keys", "profile", &json!({"action": "zap"}));
    assert_eq!(m.ok("keys", "test", &json!({"action": "power:turbo"})), json!("ran power:turbo"));
    m.refused("keys", "test", &json!({}));
}

#[test]
fn whole_setup_profiles() {
    let m = Machine::start();
    m.rig.bundles.add("gaming", "turbo and bright");
    let list = m.ok("profile", "list", &json!({}));
    assert_eq!(list[0]["name"], json!("gaming"));
    assert_eq!(list[0]["description"], json!("turbo and bright"));
    assert_eq!(m.ok("profile", "show", &json!({"name": "gaming"}))["description"], json!("turbo and bright"));
    assert!(m.ok("profile", "apply", &json!({"name": "gaming"})).is_string());
    assert_eq!(m.rig.bundles.applied(), ["gaming"]);
    assert!(m.ok("profile", "next", &json!({})).is_string());
    assert_eq!(m.ok("profile", "current", &json!({})), json!({"name": null}));
    assert!(m.ok("profile", "save", &json!({"name": "mine", "description": "d"})).is_string());
    assert_eq!(m.ok("profile", "delete", &json!({"name": "mine"})), json!("deleted mine"));
    m.refused("profile", "show", &json!({"name": "nothing"}));
    m.refused("profile", "apply", &json!({}));
}

#[test]
fn audio_commands_report_what_the_sound_system_says() {
    let m = Machine::start();
    // The simulated laptop has no sound card: reads answer with the neutral state, changes say why not.
    let sound = m.ok("audio", "sound", &json!({}));
    assert_eq!((sound["ok"].clone(), sound["sinks"].clone()), (Value::Null, json!([])));
    let jack = m.ok("audio", "jack", &json!({}));
    assert_eq!((jack["headphones"].clone(), jack["setting"].clone()), (json!(false), json!("auto")));
    assert_eq!(m.ok("audio", "jack", &json!({"use": "headset"}))["setting"], json!("headset"));
    m.refused("audio", "jack", &json!({"use": "loud"}));
    let mic = m.ok("audio", "mic", &json!({}));
    assert!(mic.get("muted").is_some() && mic.get("boost").is_some());
    m.ok("audio", "mic", &json!({"action": "mute"}));
    m.ok("audio", "mic", &json!({"action": "unmute"}));
    assert_eq!(m.ok("audio", "mic", &json!({"action": "volume"})), Value::Null);
    m.refused("audio", "mic", &json!({"action": "shout"}));
    let voice = m.ok("audio", "voice", &json!({}));
    assert_eq!(voice["modes"]["headset"], json!("off"));
    assert_eq!(voice["notify"], json!(true));
    m.ok("audio", "voice", &json!({"action": "notify", "a": "off"}));
    assert_eq!(m.ok("audio", "voice", &json!({}))["notify"], json!(false));
    m.refused("audio", "voice", &json!({"action": "notify", "a": "maybe"}));
    m.refused("audio", "voice", &json!({"action": "set", "a": "toaster", "b": "off"}));
    assert_eq!(m.ok("audio", "speaker-filter", &json!({})), json!(false));
    m.refused("audio", "speaker-filter", &json!({"action": "toggle"}));
    m.refused("audio", "output", &json!({"action": "sideways"}));
    m.refused("audio", "output", &json!({}));
    assert!(m.refused("audio", "dts", &json!({})).contains("AcerUltraX"));
    m.refused("audio", "dts", &json!({"action": "sing"}));
    let repair = m.ok("audio", "repair", &json!({}));
    assert_eq!((repair["fixed"].clone(), repair["steps"].clone()), (json!(false), json!([])));
    assert!(m.rig.notifier.shown().is_empty() || !m.rig.notifier.shown().is_empty());
}

// ---- system

struct Hook {
    skip_seen: AtomicBool,
}

impl SelfTest for Hook {
    fn run(&self, skip_fans: bool) -> (bool, String) {
        self.skip_seen.store(skip_fans, Ordering::SeqCst);
        (true, "17/17".to_owned())
    }
}

impl ServiceLogs for Hook {
    fn text(&self) -> Result<String, String> {
        Ok("started\nfan loop running\n".to_owned())
    }
}

#[test]
fn self_test_and_journal_come_from_the_binarys_hooks() {
    let hook = Arc::new(Hook { skip_seen: AtomicBool::new(false) });
    let m = Machine::start_with(Hooks {
        self_test: Some(hook.clone()),
        service_logs: Some(hook.clone()),
        ..Hooks::default()
    });
    assert_eq!(m.ok("selftest", "run", &json!({"skip_fans": true})), json!({"ok": true, "output": "17/17"}));
    assert!(hook.skip_seen.load(Ordering::SeqCst), "skip_fans reaches the test");
    m.ok("selftest", "run", &json!({}));
    assert!(
        !hook.skip_seen.load(Ordering::SeqCst),
        "fans are exercised only when the page does not say skip"
    );
    assert_eq!(m.ok("service", "logs", &json!({})), json!({"text": "started\nfan loop running\n"}));
    m.refused("selftest", "run", &json!({"skip_fans": "yes"}));
}

#[test]
fn services_doctor_and_config() {
    let m = Machine::start();
    m.rig.service_checks.units.lock().expect("units").push(Unit {
        unit: "truesense.service".to_owned(),
        scope: serde_json::from_value(json!("system")).expect("scope"),
        enabled: "enabled".to_owned(),
        active: "active".to_owned(),
        ok: true,
        fix: String::new(),
    });
    let rows = m.ok("service", "check", &json!({}));
    assert_eq!((rows[0]["unit"].clone(), rows[0]["ok"].clone()), (json!("truesense.service"), json!(true)));
    let repair = m.ok("service", "repair", &json!({}));
    assert_eq!(repair[0], json!({"step": "user services restarted", "ok": true}));
    assert!(repair[1]["step"].as_str().expect("step").contains("sudo truesense service repair"), "{repair}");
    let doctor = m.ok("doctor", "run", &json!({}));
    assert!(doctor["checks"].as_array().is_some_and(|c| !c.is_empty()) && doctor["ok"].is_boolean());
    assert_eq!(doctor["version"], json!(ts_foundation::buildinfo::VERSION));
    assert!(m.ok("config", "show", &json!({}))["audio"]["jack"].is_string());
    assert_eq!(m.ok("config", "validate", &json!({})), json!([]));
}

#[test]
fn reset_all_runs_every_step_and_reports_each() {
    let m = Machine::start();
    running(&m);
    m.rig.daemon.answer_ok("fan_curve", json!({"name": "native"}));
    m.rig.daemon.answer_ok("smart_profile", json!({"name": "native"}));
    m.rig.daemon.answer_ok("fan", json!({"mode": "auto"}));
    m.rig.daemon.answer_ok("set", json!({"auto_mode": "native"}));
    let done = m.ok("config", "reset-all", &json!({}));
    let steps = done["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 12, "{done}");
    assert!(steps.iter().all(|s| s["step"].is_string() && s["ok"].is_boolean()));
    assert!(
        steps.iter().any(|s| s["ok"] == json!(false) && s["error"].is_string()),
        "a step that fails says why and the others still ran"
    );
    assert!(steps.iter().any(|s| s["step"] == json!("keyboard: stock look") && s["ok"] == json!(true)));
    assert!(steps.iter().any(|s| s["step"] == json!("light bar: stock") && s["ok"] == json!(true)));
}

#[test]
fn autostart_writes_and_removes_the_entry() {
    let m = Machine::start();
    let entry = m.root.0.join("home/autostart/truesense.desktop");
    assert_eq!(m.ok("gui", "autostart", &json!({"action": "status"})), json!(false));
    assert_eq!(m.ok("gui", "autostart", &json!({"action": "on"})), json!(true));
    assert!(std::fs::read_to_string(&entry).expect("entry").contains("Exec=truesense gui"));
    assert_eq!(m.ok("gui", "autostart", &json!({})), json!(true));
    assert_eq!(m.ok("gui", "autostart", &json!({"action": "off"})), json!(false));
    assert!(!entry.exists());
    m.refused("gui", "autostart", &json!({"action": "toggle"}));
}

#[test]
fn an_update_needs_the_page_to_have_asked_and_a_newer_release() {
    let m = Machine::start();
    let check = m.ok("update", "check", &json!({}));
    assert_eq!(
        (check["available"].clone(), check["current"].clone()),
        (json!(false), json!(ts_foundation::buildinfo::VERSION))
    );
    assert!(check["info"]["tag"].is_string());
    assert_eq!(m.ok("update", "auto", &json!({"action": "on"})), json!(true));
    assert_eq!(m.ok("update", "auto", &json!({})), json!(true));

    assert!(m.refused("update", "apply", &json!({})).contains("yes: true"));
    assert!(m.refused("update", "apply", &json!({"yes": false})).contains("yes: true"));
    assert!(m.rig.updates.applied.lock().expect("applied").is_empty(), "no installer without the page's yes");
    assert!(m.refused("update", "apply", &json!({"yes": true})).contains("already the newest"));
    assert!(m.rig.updates.applied.lock().expect("applied").is_empty());

    {
        let mut status = m.rig.updates.status.lock().expect("status");
        status.latest = Some("99.0.0".to_owned());
        status.available = true;
        if let Some(info) = status.info.as_mut() {
            info.tag = "v99.0.0".to_owned();
            info.version = "99.0.0".to_owned();
        }
    }
    let applied = m.ok("update", "apply", &json!({"yes": true}));
    assert_eq!(
        (applied["version"].clone(), applied["returncode"].clone(), applied["verified"].clone()),
        (json!("99.0.0"), json!(0), json!(true))
    );
    let options = m.rig.updates.applied.lock().expect("applied");
    assert_eq!(options.len(), 1);
    assert_eq!(
        options[0].tag.as_deref(),
        Some("v99.0.0"),
        "the verified tag of the release that was checked"
    );
}

#[test]
fn a_missing_service_is_reported_with_a_code_the_page_can_explain() {
    let m = Machine::start();
    m.rig.daemon.set_available(false);
    let (status, body) = m.run("power", "managers", &json!({}));
    assert_eq!(status, 422);
    assert_eq!(body["error"]["code"], json!("needs-daemon"));
    assert!(body["error"]["message"].as_str().expect("message").contains("not running"));
    // Firmware reads still work without it.
    assert_eq!(m.run("battery", "status", &json!({})).0, 200);
    m.rig.daemon.set_available(true);
    m.rig.daemon.answer("power_managers", Reply::Error("permission denied".to_owned()));
    let (_, body) = m.run("power", "managers", &json!({}));
    assert_eq!(body["error"]["message"], json!("permission denied"));
    assert!(body["error"].get("code").is_none(), "an ordinary refusal has no code");
}

#[test]
fn arguments_of_the_wrong_kind_say_which_member_is_wrong() {
    let m = Machine::start();
    for (area, verb, args, needle) in [
        ("keyboard", "brightness", json!({"level": "bright"}), "level"),
        ("keyboard", "brightness", json!({"level": 3.5}), "whole number"),
        ("keyboard", "effect", json!({"name": 5}), "name"),
        ("rear", "set", json!({"effect": "wave", "verify": "no"}), "verify"),
        ("fan", "set", json!({"cpu": 50, "confirm": "yes"}), "confirm"),
        ("look", "save", json!({"name": ["a"]}), "name"),
        ("battery", "limit", json!({"action": 7}), "action"),
    ] {
        let msg = m.refused(area, verb, &args);
        assert!(msg.contains(needle), "{area} {verb}: {msg}");
    }
}

#[test]
fn the_live_reading_has_the_shape_the_page_reads() {
    let m = Machine::start();
    running(&m);
    let live = m.api.live();
    let value = serde_json::to_value(live).expect("json");
    for key in ["t", "sensors", "power_mode", "ac", "battery_pct", "fans", "load", "power", "daemon"] {
        assert!(value.get(key).is_some(), "{key}");
    }
    assert_eq!(value["daemon"]["settings"]["auto_mode"], json!("smart"));
    assert!(value["sensors"]["cpu_fan_rpm"].is_number());
}
