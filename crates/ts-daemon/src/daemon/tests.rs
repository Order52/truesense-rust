//! The daemon driven tick by tick on a fake laptop (the Python `TestDaemonLogic` and the Go
//! daemon tests): the fans, the automatic power mode, the governor, the power limits, the boot
//! guards, the watchdogs and the panic safety.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::Ordering;

use serde_json::{Value, json};
use ts_domain::looks::Look;
use ts_domain::power::RawLimits;
use ts_domain::powermgr::UnitStatus;
use ts_ipc::wire::Args;
use ts_platform::firmware::{Device, SimFan};

use super::testkit::{Rig, Setup};

fn args() -> Args {
    Args::new()
}

fn live(rig: &Rig) -> Value {
    rig.ok("state", args())["live"].clone()
}

#[test]
fn idle_heavy_hot_and_back() {
    let rig = Rig::new();
    rig.drive(25, 45, 45, 2.0);
    assert_eq!(rig.snap().fan, SimFan::Auto, "idle: the firmware has the fans");
    rig.drive(8, 70, 60, 90.0);
    assert_eq!(rig.snap().fan, SimFan::Manual, "heavy: manual duties");
    rig.drive(14, 93, 70, 99.0);
    assert_eq!(
        rig.snap().duty,
        [100, 100],
        "hot: emergency 100/100 (the temperature is smoothed, so it takes a few ticks)"
    );
    rig.drive(45, 50, 48, 2.0);
    assert_eq!(rig.snap().fan, SimFan::Auto, "calm again: back to the firmware");
}

#[test]
fn smart_fan_off_is_hands_off_and_settings_are_checked() {
    let rig = Rig::new();
    rig.ok("set", args().with("key", "smart_fan").with("value", false));
    rig.drive(5, 95, 95, 99.0);
    assert_eq!(rig.snap().fan, SimFan::Auto, "smart fan off: the firmware keeps the fans even when hot");
    rig.ok("set", args().with("key", "smart_fan").with("value", true));
    for bad in [
        args().with("key", "bogus").with("value", true),
        args().with("key", "smart_fan").with("value", "yes"),
        args().with("key", "smart_fan"),
        args().with("key", "auto_mode").with("value", "turbo"),
        args().with("key", "auto_mode"),
        args().with("value", true),
    ] {
        assert!(rig.call("set", bad.clone()).is_err(), "{bad:?} must be refused");
    }
    assert_eq!(
        rig.call("set", args().with("key", "bogus").with("value", true)).unwrap_err(),
        "unknown setting 'bogus'"
    );
    assert_eq!(
        rig.call("set", args().with("key", "smart_fan").with("value", 1)).unwrap_err(),
        "value must be true or false"
    );
    assert_eq!(
        rig.call("set", args().with("key", "auto_mode").with("value", "turbo")).unwrap_err(),
        "auto_mode: off | native | smart"
    );
}

#[test]
fn set_returns_the_settings_and_keeps_auto_profile_and_auto_mode_in_step() {
    let rig = Rig::new();
    let s = rig.ok("set", args().with("key", "auto_profile").with("value", false));
    assert_eq!((s["auto_profile"].clone(), s["auto_mode"].clone()), (json!(false), json!("off")));
    let s = rig.ok("set", args().with("key", "auto_profile").with("value", true));
    assert_eq!(s["auto_mode"], json!("smart"));
    let s = rig.ok("set", args().with("key", "auto_mode").with("value", "native"));
    assert_eq!((s["auto_mode"].clone(), s["auto_profile"].clone()), (json!("native"), json!(true)));
    let s = rig.ok("set", args().with("key", "auto_mode").with("value", "off"));
    assert_eq!(s["auto_profile"], json!(false));
    let s = rig.ok("set", args().with("key", "operating_follow").with("value", true));
    assert_eq!(s["operating_follow"], json!(true));
    // The setting is on disk: a new store over the same file sees it.
    let again = ts_foundation::state::Store::open(rig.state.path(), serde_json::Map::new());
    assert!(again.get_bool("operating_follow", false));
}

#[test]
fn native_auto_follows_the_power_source_only() {
    let rig = Rig::new();
    rig.auto_mode("native");
    rig.sim.run("set_mode 6").unwrap();
    rig.drive(8, 40, 40, 95.0);
    assert_eq!(rig.snap().mode, 1, "on AC: balanced even under load (no heat or load logic)");
    rig.sensors.set_ac(false);
    rig.drive(8, 40, 40, 95.0);
    assert_eq!(rig.snap().mode, 6, "on battery: eco");
}

#[test]
fn smart_auto_escalates_and_backs_off_for_heat() {
    let rig = Rig::new();
    rig.auto_mode("smart");
    rig.drive(70, 40, 40, 5.0);
    assert_eq!(rig.snap().mode, 1, "idle on AC: balanced");
    rig.drive(10, 70, 60, 90.0);
    assert_eq!(rig.snap().mode, 4, "heavy load: performance first");
    rig.drive(70, 70, 60, 90.0);
    assert_eq!(rig.snap().mode, 5, "sustained, cool enough and not changed for a while: turbo");
    rig.drive(10, 93, 70, 95.0);
    assert_eq!(rig.snap().mode, 4, "hot: back off from turbo at once");
    rig.drive(40, 70, 60, 90.0);
    assert_eq!(rig.snap().mode, 4, "and stay there during the back-off hold");
    rig.drive(100, 40, 40, 3.0);
    assert!(matches!(rig.snap().mode, 1 | 4));
}

#[test]
fn smart_auto_on_battery_saves_power() {
    let rig = Rig::new();
    rig.auto_mode("smart");
    rig.sensors.set_ac(false);
    rig.drive(40, 40, 40, 4.0);
    assert_eq!(rig.snap().mode, 6, "idle on battery: eco");
    rig.drive(40, 70, 60, 90.0);
    assert_eq!(rig.snap().mode, 1, "busy on battery: balanced, not turbo");
}

#[test]
fn a_nearly_empty_battery_goes_to_eco_at_once() {
    let rig = Rig::new();
    rig.auto_mode("smart");
    rig.sensors.set_ac(false);
    rig.sensors.set_battery(10.0);
    rig.drive(3, 40, 40, 90.0);
    assert_eq!(rig.snap().mode, 6);
}

#[test]
fn a_pick_ends_by_itself_when_the_situation_changes() {
    let rig = Rig::new();
    rig.auto_mode("smart");
    rig.drive(70, 40, 40, 5.0);
    assert_eq!(rig.snap().mode, 1);
    rig.ok("power", args().with("mode", "eco").with("hold_minutes", 30.0));
    assert_eq!(rig.snap().mode, 6);
    rig.drive(30, 40, 40, 5.0);
    assert_eq!(rig.snap().mode, 6, "same situation: your pick stays");
    rig.drive(8, 70, 60, 90.0);
    assert_eq!(rig.snap().mode, 4, "a game starts: smart auto takes over without anyone pressing resume");
    assert_eq!(rig.ok("state", args())["hold_left_s"], json!(0));
    assert_eq!(rig.state.get_f64("profile_hold_until", 1.0), 0.0, "the dropped hold is stored as dropped");
    rig.ok("power", args().with("mode", "eco").with("hold_minutes", 30.0));
    rig.sensors.set_ac(false);
    rig.drive(8, 70, 60, 90.0);
    assert_ne!(rig.snap().mode, 6, "unplugging also ends the hold");
}

#[test]
fn a_hold_is_remembered_across_a_restart_and_reported() {
    let rig = Rig::new();
    rig.drive(3, 40, 40, 5.0);
    let held = rig.ok("hold", args().with("minutes", 10.0));
    assert_eq!(held, json!(10.0));
    let left = rig.ok("state", args())["hold_left_s"].as_i64().unwrap();
    assert!((599..=600).contains(&left), "{left}");
    assert!(rig.state.get_f64("profile_hold_until", 0.0) > rig.clock.wall_now());
    assert_eq!(rig.ok("hold", args()), json!(0), "no minutes clears it");
    assert_eq!(rig.ok("state", args())["hold_left_s"], json!(0));
}

#[test]
fn choosing_smart_or_native_applies_immediately_even_during_a_hold() {
    let rig = Rig::new();
    rig.auto_mode("off");
    rig.drive(5, 40, 40, 5.0);
    rig.ok("power", args().with("mode", "turbo").with("hold_minutes", 30.0));
    assert_eq!(rig.snap().mode, 5);
    rig.auto_mode("native");
    rig.drive(3, 40, 40, 5.0);
    assert_eq!(rig.snap().mode, 1, "native auto (AC: balanced) must not wait for the old hold to run out");
    assert_eq!(rig.ok("state", args())["hold_left_s"], json!(0));
}

#[test]
fn power_validates_the_mode_and_reports_the_new_one() {
    let rig = Rig::new();
    rig.auto_mode("off");
    assert_eq!(rig.ok("power", args().with("mode", "performance")), json!("performance"));
    assert_eq!(rig.snap().mode, 4);
    let e = rig.call("power", args().with("mode", "warp")).unwrap_err();
    assert!(e.contains("power mode must be one of"), "{e}");
    assert!(rig.call("power", args()).is_err());
    assert!(rig.call("power", args().with("mode", "eco").with("hold_minutes", "x")).is_err());
}

#[test]
fn fan_requests_need_confirmation_and_a_cool_machine_to_stop_a_fan() {
    let rig = Rig::new();
    rig.drive(5, 40, 40, 2.0);
    let cases = [
        ("unconfirmed off", args().with("mode", "off")),
        ("unconfirmed manual", args().with("mode", "manual").with("cpu", 0).with("gpu", 50)),
        ("bad duty", args().with("mode", "manual").with("cpu", 10)),
        ("duty too high", args().with("mode", "manual").with("cpu", 30).with("gpu", 101)),
        ("no duty", args().with("mode", "manual")),
        ("unknown mode", args().with("mode", "turbo")),
        ("no mode", args()),
    ];
    for (name, request) in cases {
        let before = rig.snap();
        assert!(rig.call("fan", request).is_err(), "{name} must be refused");
        assert_eq!(
            (rig.snap().duty, rig.snap().fan),
            (before.duty, before.fan),
            "{name}: a refused request must not reach the firmware"
        );
    }
    assert!(rig.state.get_bool("smart_fan", false), "a refused request leaves the smart fan on");
    assert_eq!(
        rig.call("fan", args().with("mode", "off")).unwrap_err(),
        "stopping a fan needs explicit confirmation"
    );
    assert_eq!(
        rig.call("fan", args().with("mode", "manual").with("cpu", 10)).unwrap_err(),
        "manual fan duty is 20..100 % per fan, or 0 to stop that fan"
    );
    assert_eq!(rig.call("fan", args().with("mode", "turbo")).unwrap_err(), "fan mode 'turbo'");
    rig.drive(10, 66, 50, 20.0);
    let e = rig.call("fan", args().with("mode", "off").with("confirm", true)).unwrap_err();
    assert!(e.contains("too warm to stop"), "{e}");
}

#[test]
fn a_stopped_fan_comes_back_when_the_laptop_gets_warm() {
    let rig = Rig::new();
    rig.drive(30, 45, 45, 2.0);
    assert_eq!(rig.ok("fan", args().with("mode", "off").with("confirm", true)), json!("off"));
    assert_eq!((rig.snap().duty, rig.snap().fan), ([0, 0], SimFan::Manual));
    assert_eq!(live(&rig)["fan_off"], json!(false), "the live state is published by the next tick");
    rig.drive(3, 50, 45, 2.0);
    assert_eq!(rig.snap().duty, [0, 0], "stays off while it is cool");
    assert_eq!(live(&rig)["fan_off"], json!(true));
    assert_eq!(rig.daemon.interval().as_secs(), 1, "a stopped fan is watched every second");
    rig.drive(12, 75, 50, 20.0);
    assert_eq!(rig.snap().fan, SimFan::Auto, "the watchdog gives the fans back to the firmware");
    assert_eq!(live(&rig)["fan_off"], json!(false));
    assert!(rig.log.has("fans back on Auto: the laptop got warm"));
}

#[test]
fn a_stopped_fan_stays_watched_until_the_firmware_really_has_it_back() {
    let rig = Rig::new();
    rig.drive(30, 45, 45, 2.0);
    rig.ok("fan", args().with("mode", "off").with("confirm", true));
    rig.device.dead.store(true, Ordering::SeqCst);
    rig.sensors.set_pkg(Some(75.0)); // warm, and the device is gone: the hand-back fails
    rig.drive(3, 45, 45, 2.0);
    assert_eq!(rig.snap().duty, [0, 0], "the firmware could not be reached");
    assert!(rig.log.has("could not hand the fans back to the firmware"));
    rig.device.dead.store(false, Ordering::SeqCst);
    rig.drive(2, 45, 45, 2.0);
    assert_eq!(
        rig.snap().fan,
        SimFan::Auto,
        "the watchdog never gave up: the first tick that can reach the firmware hands the fans back"
    );
}

#[test]
fn one_fan_can_be_stopped_with_the_same_guards() {
    let rig = Rig::new();
    rig.drive(30, 45, 45, 2.0);
    for request in [
        args().with("mode", "manual").with("cpu", 0).with("gpu", 60),
        args().with("mode", "manual").with("cpu", 60).with("gpu", 0),
    ] {
        assert!(rig.call("fan", request).is_err(), "needs confirmation");
    }
    for bad in [(10, 60), (0, 150)] {
        let request =
            args().with("mode", "manual").with("cpu", bad.0).with("gpu", bad.1).with("confirm", true);
        assert!(rig.call("fan", request).is_err());
    }
    let partial = args().with("mode", "manual").with("cpu", 0).with("gpu", 60).with("confirm", true);
    assert_eq!(rig.ok("fan", partial), json!("partial-off"));
    assert_eq!((rig.snap().fan, rig.snap().duty), (SimFan::Manual, [0, 60]));
    rig.drive(3, 50, 45, 2.0);
    assert_eq!(rig.snap().duty, [0, 60], "stays while cool");
    rig.drive(12, 75, 50, 20.0);
    assert_eq!(rig.snap().fan, SimFan::Auto, "the watchdog gives ALL fans back when it gets warm");
    rig.drive(30, 45, 45, 2.0);
    rig.ok("fan", args().with("mode", "manual").with("cpu", 70).with("gpu", 0).with("confirm", true));
    assert_eq!(rig.snap().duty, [70, 0]);
    rig.ok("fan", args().with("mode", "manual").with("cpu", 50).with("gpu", 50));
    assert_eq!(live(&rig)["fan_off"], json!(false), "the next tick sees the stopped state over");
    rig.drive(1, 45, 45, 2.0);
    assert_eq!(live(&rig)["fan_off"], json!(false), "a normal manual speed ends the stopped state");
    rig.drive(10, 66, 50, 20.0);
    let request = args().with("mode", "manual").with("cpu", 0).with("gpu", 60).with("confirm", true);
    assert!(rig.call("fan", request).is_err(), "too warm to start");
}

#[test]
fn a_refused_fan_stop_hands_the_fans_back_and_arms_no_watchdog() {
    // The firmware says yes but the duty register does not move to a stop: the hardware layer
    // gives the fans back, and the daemon must not believe a stop is in force.
    let rig = Rig::new();
    rig.drive(30, 45, 45, 2.0);
    rig.device.stuck_stop.store(true, Ordering::SeqCst);
    let e = rig.call("fan", args().with("mode", "off").with("confirm", true)).unwrap_err();
    assert!(e.contains("the firmware did not accept the stop"), "{e}");
    assert_eq!(rig.snap().fan, SimFan::Auto, "the fans were returned to the firmware");
    rig.device.stuck_stop.store(false, Ordering::SeqCst);
    rig.drive(1, 45, 45, 2.0);
    assert_eq!(live(&rig)["fan_off"], json!(false), "no stop was armed");
}

#[test]
fn explicit_fan_modes_switch_the_smart_curve_off_and_smart_switches_it_on() {
    let rig = Rig::new();
    rig.drive(5, 40, 40, 2.0);
    assert_eq!(rig.ok("fan", args().with("mode", "max")), json!("max"));
    assert_eq!(rig.snap().fan, SimFan::Max);
    assert!(!rig.state.get_bool("smart_fan", true), "an explicit fan mode switches the smart curve off");
    assert_eq!(rig.ok("fan", args().with("mode", "manual").with("cpu", 45)), json!("manual"));
    assert_eq!(
        (rig.snap().fan, rig.snap().duty),
        (SimFan::Manual, [45, 45]),
        "one duty applies to both fans"
    );
    assert_eq!(
        rig.ok("fan", args().with("mode", "manual").with("cpu", 30.0).with("gpu", 60.0)),
        json!("manual")
    );
    assert_eq!(rig.snap().duty, [30, 60]);
    rig.drive(3, 40, 40, 2.0);
    assert_eq!(rig.snap().duty, [30, 60], "with the smart fan off the loop leaves the fans alone");
    assert_eq!(rig.ok("fan", args().with("mode", "auto")), json!("auto"));
    assert_eq!(rig.snap().fan, SimFan::Auto);
    assert_eq!(rig.ok("fan", args().with("mode", "smart")), json!("smart"));
    assert!(rig.state.get_bool("smart_fan", false));
}

#[test]
fn the_fan_curve_is_followed_and_custom_curves_are_checked() {
    let rig = Rig::new();
    rig.drive(25, 45, 45, 2.0);
    assert_eq!(rig.snap().fan, SimFan::Auto, "native curve idle: auto");
    rig.ok("fan_curve", args().with("action", "use").with("name", "max-cooling"));
    rig.drive(8, 65, 50, 40.0);
    assert_eq!(rig.snap().fan, SimFan::Manual);
    assert!(rig.snap().duty[0] >= 90, "{:?}", rig.snap());
    let weak = json!({"cpu": [[50, 0], [70, 50]], "gpu": [[50, 0], [78, 100]]});
    let request = args().with("action", "use").with("name", "weak").with("data", weak);
    assert!(rig.call("fan_curve", request).is_err(), "a weak curve must be refused");
    assert_eq!(
        rig.ok("fan_curve", args().with("action", "get"))["name"],
        json!("max-cooling"),
        "the failed switch changed nothing"
    );
    let got = rig.ok("fan_curve", args());
    assert!(got["names"].as_array().unwrap().contains(&json!("quiet")), "{got}");
    assert_eq!(got["curve"]["cpu"][0][0].as_f64(), Some(40.0));
    // A custom curve sent as data is kept by the daemon.
    let custom = json!({"cpu": [[40, 30], [90, 100]], "gpu": [[40, 30], [80, 100]]});
    let used = rig.ok("fan_curve", args().with("action", "use").with("data", custom.clone()));
    assert_eq!(used["name"], json!("custom"));
    assert!(rig.state.get("fan_curve_data").is_some_and(|v| !v.is_null()));
    rig.ok("fan_curve", args().with("action", "use").with("name", "quiet"));
    assert!(
        rig.state.get("fan_curve_data").is_some_and(|v| v.is_null()),
        "a built-in is stored by name only"
    );
    rig.ok("fan_curve", args().with("action", "use").with("name", "native"));
    assert!(
        !rig.state.get_bool("smart_fan", true) && rig.snap().fan == SimFan::Auto,
        "the native curve is no curve"
    );
    assert!(rig.call("fan_curve", args().with("action", "zap")).is_err());
    assert!(rig.call("fan_curve", args().with("action", "use").with("name", "no-such-curve")).is_err());
}

#[test]
fn smart_profile_switching() {
    let rig = Rig::new();
    rig.auto_mode("smart");
    rig.ok("smart_profile", args().with("action", "use").with("name", "battery-first"));
    rig.drive(70, 40, 40, 5.0);
    rig.drive(60, 70, 60, 90.0);
    assert_eq!(rig.snap().mode, 4, "battery-first never uses Turbo, even when sustained");
    rig.ok("smart_profile", args().with("action", "use").with("name", "performance-first"));
    rig.drive(25, 70, 60, 90.0);
    assert_eq!(rig.snap().mode, 5, "performance-first: Turbo after 10 s");
    let request = args().with("action", "use").with("name", "bad").with("data", json!({"ac_busy": "warp"}));
    assert!(rig.call("smart_profile", request).is_err(), "an invalid profile must be refused");
    let got = rig.ok("smart_profile", args());
    assert_eq!(got["name"], json!("performance-first"));
    assert_eq!(got["profile"]["ac_busy"], json!("performance"));
    assert!(got["names"].as_array().unwrap().contains(&json!("steady")));
    assert!(!live(&rig)["mode_reason"].as_str().unwrap().is_empty());
    // A profile sent as data is kept by the daemon.
    let data = json!({"ac_busy": "performance", "ac_turbo": false});
    let used = rig.ok("smart_profile", args().with("action", "use").with("data", data));
    assert_eq!(used["name"], json!("custom"));
    assert_eq!(rig.state.get("smart_profile_data").unwrap()["ac_turbo"], json!(false));
    assert!(rig.call("smart_profile", args().with("action", "zap")).is_err());
}

#[test]
fn undervolt_needs_confirmation_and_boot_has_a_crash_guard() {
    let rig = Rig::new();
    let apply =
        |core: i64, cache: i64| args().with("action", "apply").with("core", core).with("cache", cache);
    assert!(rig.call("undervolt", apply(-50, -40)).is_err(), "no confirmation: refused");
    assert!(
        rig.call("undervolt", apply(-300, -40).with("confirm", true)).is_err(),
        "beyond the limit: refused"
    );
    assert_eq!(*rig.uv.offsets.lock().unwrap(), (0, 0));
    assert!(rig.uv.applied.lock().unwrap().is_empty(), "nothing must have been applied");
    let done = rig.ok("undervolt", apply(-50, -40).with("confirm", true).with("name", "truesense"));
    assert_eq!(done, json!({"core_mv": -50, "cache_mv": -40}));
    assert_eq!(*rig.uv.offsets.lock().unwrap(), (-50, -40));
    let got = rig.ok("undervolt", args());
    assert_eq!(got["applied"], json!({"name": "truesense", "core": -50, "cache": -40}));
    assert_eq!(got["pending"], json!(false));
    rig.ok("undervolt", args().with("action", "reset"));
    assert_eq!(*rig.uv.offsets.lock().unwrap(), (0, 0));
    assert_eq!(rig.ok("undervolt", args())["applied"], Value::Null);
    let boot = args()
        .with("action", "boot")
        .with("core", -30)
        .with("cache", -30)
        .with("confirm", true)
        .with("name", "mild");
    rig.ok("undervolt", boot);
    assert_eq!(rig.ok("undervolt", args())["boot"]["name"], json!("mild"));
    rig.daemon.boot();
    assert_eq!(rig.uv.offsets.lock().unwrap().0, -30, "applied at boot");
    assert!(rig.path("var/uv-pending").exists(), "with the guard flag set");
    assert_eq!(rig.ok("undervolt", args())["pending"], json!(true));
    // Ten minutes later the flag goes.
    rig.drive(602, 40, 40, 2.0);
    assert!(!rig.path("var/uv-pending").exists(), "the boot undervolt survived ten minutes");
    rig.ok("undervolt", args().with("action", "boot_clear"));
    assert_eq!(rig.ok("undervolt", args())["boot"], Value::Null);
}

#[test]
fn a_boot_undervolt_whose_previous_boot_never_proved_stable_is_dropped() {
    let rig = Rig::new();
    rig.state.set("uv_boot", json!({"name": "mild", "core": -30, "cache": -30})).unwrap();
    std::fs::create_dir_all(rig.path("var")).unwrap();
    std::fs::write(rig.path("var/uv-pending"), "1").unwrap(); // the flag of the boot that crashed
    rig.daemon.boot();
    assert_eq!(
        *rig.uv.offsets.lock().unwrap(),
        (0, 0),
        "the previous boot never proved stable: do not apply again"
    );
    assert_eq!(rig.state.get("uv_boot"), Some(Value::Null), "and the setting is forgotten");
    assert!(!rig.path("var/uv-pending").exists());
    assert!(rig.log.has("undervolt at boot disabled"));
}

#[test]
fn a_failed_boot_undervolt_removes_its_flag_and_is_not_fatal() {
    let rig = Rig::new();
    rig.uv.fail_apply.store(true, Ordering::SeqCst);
    rig.state.set("uv_boot", json!({"name": "mild", "core": -30, "cache": -30})).unwrap();
    rig.daemon.boot();
    assert!(!rig.path("var/uv-pending").exists(), "nothing was applied, so nothing needs guarding");
    assert!(rig.log.has("undervolt at boot failed"));
    assert!(rig.state.get("uv_boot").is_some_and(|v| !v.is_null()), "the setting stays for the next boot");
}

#[test]
fn an_invalid_saved_boot_undervolt_is_not_applied() {
    let rig = Rig::new();
    rig.state.set("uv_boot", json!({"name": "x", "core": -900, "cache": -30})).unwrap();
    rig.daemon.boot();
    assert!(rig.uv.applied.lock().unwrap().is_empty());
    assert!(!rig.path("var/uv-pending").exists());
}

#[test]
fn a_dangerous_undervolt_needs_force_and_a_failed_one_is_rolled_back() {
    let rig = Rig::new();
    let request = args().with("action", "apply").with("core", -100).with("cache", -60).with("confirm", true);
    let e = rig.call("undervolt", request.clone()).unwrap_err();
    assert!(e.ends_with("Add the explicit force flag if you really mean it (CLI: --force)."), "{e}");
    assert!(e.starts_with("Beyond the danger line"), "{e}");
    rig.ok("undervolt", request.clone().with("force", true));
    assert_eq!(rig.uv.offsets.lock().unwrap().0, -100);
    *rig.uv.offsets.lock().unwrap() = (-10, -10);
    rig.uv.fail_apply.store(true, Ordering::SeqCst);
    let e = rig.call(
        "undervolt",
        args().with("action", "apply").with("core", -50).with("cache", -40).with("confirm", true),
    );
    assert_eq!(e.unwrap_err(), "the CPU kept its stock voltage");
    assert_eq!(*rig.uv.offsets.lock().unwrap(), (0, 0), "a failed apply resets to stock");
    assert!(rig.call("undervolt", args().with("action", "zap")).is_err());
}

#[test]
fn gpu_undervolt_needs_confirmation_and_boot_has_a_crash_guard() {
    let rig = Rig::new();
    let apply =
        |offset: i64, cap: i64| args().with("action", "apply").with("offset", offset).with("cap", cap);
    assert!(rig.call("gpu_undervolt", apply(100, 2400)).is_err(), "no confirmation: refused");
    assert!(rig.call("gpu_undervolt", apply(900, 2400).with("confirm", true)).is_err(), "beyond the range");
    assert!(rig.call("gpu_undervolt", apply(100, 50).with("confirm", true)).is_err(), "cap too low");
    assert_eq!(*rig.gpu_uv.state.lock().unwrap(), (0, 0));
    rig.ok("gpu_undervolt", apply(120, 2400).with("confirm", true).with("name", "truesense"));
    assert_eq!(*rig.gpu_uv.state.lock().unwrap(), (120, 2400));
    let got = rig.ok("gpu_undervolt", args());
    assert_eq!(got["applied"]["name"], json!("truesense"));
    assert_eq!(got["offset_mhz"], json!(120));
    assert_eq!(got["range_mhz"], json!([-300, 250]));
    rig.ok("gpu_undervolt", args().with("action", "reset"));
    assert_eq!(*rig.gpu_uv.state.lock().unwrap(), (0, 0));
    let boot = args()
        .with("action", "boot")
        .with("offset", 60)
        .with("cap", 2500)
        .with("confirm", true)
        .with("name", "mild");
    let saved = rig.ok("gpu_undervolt", boot);
    assert_eq!(
        (saved["offset"].clone(), saved["cap"].clone(), saved["name"].clone()),
        (json!(60), json!(2500), json!("mild"))
    );
    rig.daemon.boot();
    assert_eq!(rig.gpu_uv.state.lock().unwrap().0, 60);
    assert!(rig.path("var/gpu-uv-pending").exists());
    // A danger-zone offset needs force.
    let e = rig.call("gpu_undervolt", apply(220, 2400).with("confirm", true)).unwrap_err();
    assert!(e.contains("force"), "{e}");
    rig.ok("gpu_undervolt", apply(220, 2400).with("confirm", true).with("force", true));
    // An absent cap means none; a null one is refused as in Python.
    rig.ok("gpu_undervolt", args().with("action", "apply").with("offset", 10).with("confirm", true));
    assert_eq!(*rig.gpu_uv.state.lock().unwrap(), (10, 0));
    let null_cap =
        args().with("action", "apply").with("offset", 10).with("cap", Value::Null).with("confirm", true);
    assert!(rig.call("gpu_undervolt", null_cap).is_err());
    rig.ok("gpu_undervolt", args().with("action", "boot_clear"));
    assert_eq!(rig.ok("gpu_undervolt", args())["boot"], Value::Null);
}

#[test]
fn a_gpu_boot_undervolt_whose_previous_boot_never_proved_stable_is_dropped() {
    let rig = Rig::new();
    rig.state.set("gpu_uv_boot", json!({"name": "mild", "offset": 60, "cap": 2500})).unwrap();
    std::fs::create_dir_all(rig.path("var")).unwrap();
    std::fs::write(rig.path("var/gpu-uv-pending"), "1").unwrap();
    rig.daemon.boot();
    assert_eq!(*rig.gpu_uv.state.lock().unwrap(), (0, 0));
    assert_eq!(rig.state.get("gpu_uv_boot"), Some(Value::Null));
    rig.state.set("gpu_uv_boot", json!({"offset": 60, "cap": 2500})).unwrap();
    rig.daemon.boot();
    assert_eq!(rig.gpu_uv.state.lock().unwrap().0, 60, "the next boot after the dropped one applies again");
    rig.drive(602, 40, 40, 2.0);
    assert!(!rig.path("var/gpu-uv-pending").exists());
}

#[test]
fn boot_forgets_applied_offsets() {
    let rig = Rig::new();
    rig.state.set("uv_applied", json!({"core": -50})).unwrap();
    rig.state.set("gpu_uv_applied", json!({"offset": 50})).unwrap();
    rig.daemon.boot();
    assert_eq!(rig.state.get("uv_applied"), Some(Value::Null));
    assert_eq!(rig.state.get("gpu_uv_applied"), Some(Value::Null));
}

#[test]
fn charge_to_full_once_restores_the_limit() {
    let rig = Rig::new();
    rig.drive(6, 40, 40, 2.0);
    rig.sim.run("set_battery 1 1").unwrap();
    assert_eq!(rig.ok("battery_full", args())["limit"], json!(true));
    assert_eq!(rig.ok("battery_full", args().with("action", "start")), json!({"active": true}));
    assert_eq!(rig.snap().battery[0], 0, "the 80 % limit is lifted for one charge");
    let got = rig.ok("battery_full", args());
    assert_eq!(
        (got["active"].clone(), got["ac"].clone(), got["limit"].clone()),
        (json!(true), json!(true), json!(false))
    );
    let e = rig.call("battery_full", args().with("action", "start")).unwrap_err();
    assert_eq!(e, "the 80 % limit is off already: the battery charges to 100 %");
    rig.sensors.set_ac(false);
    rig.drive(6, 40, 40, 2.0);
    assert_eq!(rig.snap().battery[0], 1, "unplugged: the limit is back");
    assert_eq!(rig.state.get("charge_full"), Some(Value::Null), "and the record cleared");
    assert_eq!(rig.ok("battery_full", args())["active"], json!(false));
    assert!(rig.log.has("80 % charge limit is back (unplugged)"));
    assert_eq!(
        rig.call("battery_full", args().with("action", "start")).unwrap_err(),
        "plug the charger in first"
    );
    rig.sensors.set_ac(true);
    rig.drive(2, 40, 40, 2.0);
    rig.ok("battery_full", args().with("action", "start"));
    assert_eq!(rig.ok("battery_full", args().with("action", "cancel")), json!({"active": false}));
    assert_eq!(rig.snap().battery[0], 1);
    assert!(rig.call("battery_full", args().with("action", "zap")).is_err());
}

#[test]
fn charge_to_full_ends_when_the_battery_has_been_full_and_after_a_day() {
    let rig = Rig::new();
    rig.drive(6, 40, 40, 2.0);
    rig.sim.run("set_battery 1 1").unwrap();
    rig.ok("battery_full", args().with("action", "start"));
    rig.sensors.set_battery(100.0);
    rig.drive(5, 40, 40, 2.0);
    assert!(
        rig.state.get("charge_full").unwrap()["full_at"].is_number(),
        "the moment it was full is recorded"
    );
    assert_eq!(rig.snap().battery[0], 0, "not yet");
    rig.drive(310, 40, 40, 2.0);
    assert_eq!(rig.snap().battery[0], 1, "full for five minutes: the limit is back");
    assert!(rig.log.has("the battery is full"));
    // A record older than a day ends at the next check.
    rig.sensors.set_battery(80.0);
    rig.ok("battery_full", args().with("action", "start"));
    rig.clock.advance(86_401.0);
    rig.drive(5, 40, 40, 2.0);
    assert_eq!(rig.snap().battery[0], 1);
    assert!(rig.log.has("24 hours passed"));
}

#[test]
fn a_charge_limit_that_cannot_be_restored_is_retried() {
    let rig = Rig::new();
    rig.drive(6, 40, 40, 2.0);
    rig.sim.run("set_battery 1 1").unwrap();
    rig.ok("battery_full", args().with("action", "start"));
    rig.sensors.set_ac(false);
    rig.device.dead.store(true, Ordering::SeqCst);
    rig.drive(6, 40, 40, 2.0);
    assert!(
        rig.state.get("charge_full").is_some_and(|v| !v.is_null()),
        "the record stays until the limit really came back"
    );
    rig.device.dead.store(false, Ordering::SeqCst);
    rig.drive(6, 40, 40, 2.0);
    assert_eq!(rig.snap().battery[0], 1);
    assert_eq!(rig.state.get("charge_full"), Some(Value::Null));
}

#[test]
fn power_limits_are_enforced_and_handed_back() {
    let rig = Rig::new();
    rig.daemon.boot(); // captures what the firmware set, before anything can change it
    let applied = rig.ok("cpu_limits", args().with("pl1", 70).with("pl2", 90));
    assert_eq!((applied["pl1_w"].clone(), applied["pl2_w"].clone()), (json!(70.0), json!(90.0)));
    rig.drive(10, 40, 40, 2.0);
    assert_eq!(*rig.power.raw.lock().unwrap(), RawLimits { pl1_uw: 70_000_000, pl2_uw: 90_000_000 });
    rig.power.raw.lock().unwrap().pl1_uw = 50_000_000; // something else changes them
    rig.drive(4, 40, 40, 2.0);
    assert_eq!(rig.power.raw.lock().unwrap().pl1_uw, 70_000_000, "they are re-asserted");
    assert_eq!(rig.ok("cpu_info", args())["user"], json!({"pl1": 70.0, "pl2": 90.0}));
    assert_eq!(rig.ok("cpu_info", args())["auto"], json!({"normal_w": 80, "gaming_w": 65}));
    let e = rig.call("cpu_limits", args().with("pl1", 105).with("pl2", 118)).unwrap_err();
    assert!(
        e.starts_with("Beyond the danger line") && e.ends_with("force flag if you really mean it."),
        "{e}"
    );
    rig.ok("cpu_limits", args().with("pl1", 105).with("pl2", 118).with("force", true));
    let reset = rig.ok("cpu_limits", args().with("reset", true));
    assert_eq!(reset["automatic"], json!(true));
    rig.drive(4, 40, 40, 2.0);
    assert_eq!(rig.ok("cpu_info", args())["user"], Value::Null);
    {
        let raw = rig.power.raw.lock().unwrap();
        assert_eq!(raw.pl1_uw, raw.pl2_uw, "automatic limits are one value for both");
    }
    rig.ok("set", args().with("key", "smart_fan").with("value", false));
    rig.auto_mode("native");
    rig.drive(10, 40, 40, 2.0);
    assert_eq!(
        *rig.power.raw.lock().unwrap(),
        RawLimits { pl1_uw: 80_000_000, pl2_uw: 100_000_000 },
        "nothing smart on: the firmware's own limits come back"
    );
    assert!(rig.call("cpu_limits", args().with("pl1", 40)).is_err(), "pl2 is needed");
    assert!(rig.call("cpu_limits", args()).is_err());
}

#[test]
fn the_first_install_defaults_leave_the_firmware_alone() {
    // The Python test: defaults only, the firmware set 95 W, a hot busy machine.
    let setup = Setup {
        settings: vec![("smart_fan", json!(false)), ("auto_mode", json!("native")), ("pl_user", Value::Null)],
        ..Setup::default()
    };
    let rig = Rig::with(setup);
    *rig.power.raw.lock().unwrap() = RawLimits { pl1_uw: 95_000_000, pl2_uw: 95_000_000 };
    rig.daemon.boot();
    assert!(!rig.daemon.smart_active());
    rig.drive(40, 88, 70, 95.0);
    assert_eq!(
        rig.snap().fan,
        SimFan::Auto,
        "fans stay on the firmware's own control until you choose smart"
    );
    assert!(rig.cap.caps.lock().unwrap().is_empty(), "no CPU clock cap");
    assert!(rig.power.writes.lock().unwrap().is_empty(), "power limits stay as the firmware set them");
    assert_eq!(rig.snap().mode, 1, "native auto: Balanced on power");
    rig.ok("set", args().with("key", "smart_fan").with("value", true));
    rig.drive(5, 60, 55, 30.0);
    assert!(rig.daemon.smart_active());
    assert_eq!(rig.power.raw.lock().unwrap().pl1_uw, 80_000_000, "smart on: ours");
    rig.ok("set", args().with("key", "smart_fan").with("value", false));
    rig.drive(5, 60, 55, 30.0);
    assert_eq!(
        rig.power.raw.lock().unwrap().pl1_uw,
        95_000_000,
        "switching smart off hands the firmware's limits back"
    );
}

#[test]
fn the_gaming_power_limit_applies_while_gaming() {
    let rig = Rig::new();
    rig.daemon.boot();
    rig.sensors.set_gpu(Some(60.0), 90.0, 60.0, Some(rig.clock.wall_now()));
    rig.drive(10, 70, 60, 30.0);
    assert_eq!(rig.power.raw.lock().unwrap().pl1_uw, 65_000_000, "gaming: pl_gaming_w");
}

#[test]
fn game_rules_raise_the_mode_while_the_game_runs() {
    let rig = Rig::new();
    rig.auto_mode("native");
    let added = rig
        .ok("game_rules", args().with("action", "add").with("name", "HL2_Linux").with("mode", "performance"));
    assert_eq!(added["rules"], json!({"hl2_linux": "performance"}));
    assert_eq!(added["running"], Value::Null);
    rig.procs.none();
    rig.drive(6, 40, 40, 5.0);
    assert_eq!(rig.snap().mode, 1, "no game: native balanced");
    rig.procs.run("hl2_linux");
    rig.drive(8, 40, 40, 5.0);
    assert_eq!(rig.snap().mode, 4, "the game runs: performance");
    assert_eq!(rig.ok("game_rules", args().with("action", "list"))["running"], json!("performance"));
    for bad in [
        args().with("action", "add").with("name", "x").with("mode", "warp"),
        args().with("action", "add").with("name", "../x").with("mode", "turbo"),
        args().with("action", "remove").with("name", "nothing"),
        args().with("action", "zap"),
    ] {
        assert!(rig.call("game_rules", bad.clone()).is_err(), "{bad:?}");
    }
    assert_eq!(
        rig.call("game_rules", args().with("action", "remove").with("name", "nothing")).unwrap_err(),
        "no rule for 'nothing'"
    );
    rig.procs.none();
    rig.drive(40, 40, 40, 5.0);
    assert_eq!(rig.snap().mode, 1, "back to the automatic profile when the game ends");
    rig.ok("game_rules", args().with("action", "remove").with("name", "hl2_linux"));
    rig.ok("game_rules", args().with("action", "add").with("name", "a").with("mode", "turbo"));
    let cleared = rig.ok("game_rules", args().with("action", "clear"));
    assert_eq!(cleared["rules"], json!({}));
    assert_eq!(rig.ok("game_rules", args())["rules"], json!({}));
}

#[test]
fn a_turbo_game_rule_backs_off_when_hot_and_the_battery_wins() {
    let rig = Rig::new();
    rig.auto_mode("native");
    rig.ok("game_rules", args().with("action", "add").with("name", "game").with("mode", "turbo"));
    rig.procs.run("game");
    rig.drive(40, 93, 93, 99.0);
    assert_ne!(rig.snap().mode, 5, "turbo still backs off when hot");
    rig.sensors.set_ac(false);
    rig.sensors.set_battery(15.0);
    rig.drive(10, 40, 40, 5.0);
    assert_eq!(rig.snap().mode, 6, "a nearly empty battery wins over a game");
}

#[test]
fn shutdown_returns_the_fans_to_auto_and_lifts_the_clock_cap() {
    let rig = Rig::new();
    rig.ok("fan", args().with("mode", "manual").with("cpu", 70).with("gpu", 70));
    rig.daemon.shutdown();
    assert_eq!(rig.snap().fan, SimFan::Auto, "{:?}", rig.snap());
    assert!(rig.cap.uncapped.load(Ordering::SeqCst) >= 1, "the clock cap must be lifted");
    assert!(rig.stop.is_requested(), "the background work is told to stop");
    rig.daemon.shutdown(); // twice is fine
}

#[test]
fn the_thermal_governor_caps_a_hot_cpu_only_while_managing_it() {
    let rig = Rig::new();
    rig.ok("set", args().with("key", "smart_fan").with("value", false));
    rig.auto_mode("native");
    rig.drive(20, 92, 60, 99.0);
    assert!(rig.cap.caps.lock().unwrap().is_empty(), "nothing smart on: the CPU is left alone");
    rig.ok("set", args().with("key", "smart_fan").with("value", true));
    rig.drive(20, 92, 60, 99.0);
    let caps = rig.cap.caps.lock().unwrap().clone();
    assert!(!caps.is_empty(), "a smart feature is on and the CPU is hot: the clock is capped");
    assert!(caps.last().unwrap() < &5_000_000, "{caps:?}");
    assert!(live(&rig)["cap"].as_i64().unwrap() < 5000);
    rig.ok("set", args().with("key", "smart_fan").with("value", false));
    rig.auto_mode("native");
    rig.drive(3, 92, 60, 99.0);
    assert_eq!(*rig.cap.caps.lock().unwrap().last().unwrap(), 5_000_000, "the full clock comes back");
}

#[test]
fn the_live_state_is_published_for_status_bars_and_health_checks() {
    let rig = Rig::new();
    assert_eq!(rig.ok("state", args())["live"], json!({}), "nothing before the first tick");
    rig.drive(3, 50, 48, 10.0);
    let text = rig.read(&rig.path("run/state"));
    for want in [
        "mode=smart\n",
        "activity=",
        "tcpu=",
        "power_mode=",
        "ac=1\n",
        "auto_mode=smart\n",
        "hw_ok=1\n",
        "fan_off=0\n",
    ] {
        assert!(text.contains(want), "state file lacks {want:?}:\n{text}");
    }
    let keys: Vec<&str> = text.lines().map(|l| l.split('=').next().unwrap()).collect();
    assert_eq!(
        keys,
        [
            "mode",
            "activity",
            "fan",
            "tcpu",
            "tgpu",
            "chassis",
            "cpu",
            "gpu",
            "gpuw",
            "rpm1",
            "rpm2",
            "cap",
            "power_mode",
            "ac",
            "auto",
            "auto_mode",
            "fan_curve",
            "smart_profile",
            "mode_reason",
            "fan_off",
            "hw_ok",
            // appended after the Python daemon's keys
            "gpu_state",
            "tgpu_real"
        ],
        "the order status bars were written against, new keys at the end"
    );
    assert_eq!(rig.read(&rig.path("tmp-state")), text, "the world-readable copy is the same");
    assert!(!rig.path("predator-state").exists(), "the legacy file only with the compatibility switch");
    let live = live(&rig);
    assert_eq!(live["hw_ok"], json!(true));
    assert_eq!(live["tcpu"], json!(50));
    assert_eq!(live["chassis"], json!(55));
    assert_eq!(live["cap"], json!(5000));
    assert_eq!(live["fan"], json!("auto"));
    assert_eq!(live["fan_curve"], json!("native"));
    let settings = rig.ok("state", args())["settings"].clone();
    assert_eq!(settings["smart_fan"], json!(true));
}

#[test]
fn the_legacy_state_file_and_flags_work_only_with_the_compatibility_switch() {
    let mut config = ts_foundation::config::Config { user: "tester".to_owned(), ..Default::default() };
    config.compat.predator_widgets = true;
    let rig = Rig::with(Setup { config, ..Setup::default() });
    rig.drive(2, 50, 48, 10.0);
    assert!(rig.path("predator-state").exists());
    // The legacy flag files of the old widgets: another tool owns the feature.
    std::fs::create_dir_all(rig.home.join(".cache")).unwrap();
    std::fs::write(rig.home.join(".cache/predator-fan-mode"), "").unwrap();
    rig.drive(10, 95, 95, 99.0);
    assert_eq!(rig.snap().fan, SimFan::Auto, "predator-fan-mode: the smart fan stays out of it");
    std::fs::write(rig.home.join(".cache/predator-auto-off"), "").unwrap();
    rig.sim.run("set_mode 6").unwrap();
    rig.drive(10, 40, 40, 5.0);
    assert_eq!(rig.snap().mode, 6, "predator-auto-off: the automatic power mode stays out of it");
}

#[test]
fn a_dead_device_is_waited_for_not_fatal() {
    let rig = Rig::new();
    rig.drive(30, 45, 45, 2.0);
    rig.device.dead.store(true, Ordering::SeqCst);
    rig.drive(10, 45, 45, 2.0); // must not panic
    assert_eq!(
        live(&rig)["hw_ok"],
        json!(true),
        "the live state is the last good tick's: nothing was published without a reading"
    );
    assert!(rig.log.has("device unavailable, waiting"));
    assert_eq!(rig.log.all().matches("device unavailable").count(), 1, "one line a minute, not one per tick");
    rig.device.dead.store(false, Ordering::SeqCst);
    rig.drive(5, 50, 50, 5.0);
    assert_eq!(live(&rig)["hw_ok"], json!(true));
}

#[test]
fn a_dead_device_with_a_package_sensor_keeps_the_loop_running_and_says_so() {
    let rig = Rig::new();
    rig.drive(3, 50, 48, 10.0);
    rig.device.dead.store(true, Ordering::SeqCst);
    rig.sensors.set_pkg(Some(55.0));
    rig.daemon.guarded_step();
    assert_eq!(live(&rig)["hw_ok"], json!(false), "the loop must notice the device is gone");
    assert!(rig.daemon.interval().as_secs() >= 1);
}

#[test]
fn with_no_sensors_at_all_the_firmware_decides() {
    let rig = Rig::new();
    rig.drive(5, 70, 60, 90.0);
    assert_eq!(rig.snap().fan, SimFan::Manual);
    rig.sim.set_sensors([0, 0, 0], [0, 0]);
    rig.daemon.guarded_step();
    assert_eq!(rig.snap().fan, SimFan::Auto, "no sensors: the firmware has the fans");
}

#[test]
fn the_package_and_gpu_sensors_raise_the_temperatures_the_firmware_reports() {
    let rig = Rig::new();
    rig.sensors.set_pkg(Some(88.0));
    rig.sensors.set_gpu(Some(80.0), 0.0, 0.0, Some(rig.clock.wall_now()));
    rig.drive(1, 40, 40, 10.0);
    let l = live(&rig);
    assert_eq!(
        (l["tcpu"].clone(), l["tgpu"].clone()),
        (json!(88), json!(80)),
        "the hottest reading wins on each side"
    );
    // A GPU reading older than six seconds is not trusted.
    rig.sensors.set_gpu(Some(95.0), 0.0, 0.0, Some(rig.clock.wall_now() - 100.0));
    rig.drive(40, 40, 40, 10.0);
    assert!(live(&rig)["tgpu"].as_i64().unwrap() < 60, "{}", live(&rig));
}

#[test]
fn a_step_that_panics_while_hot_drives_the_fans_to_full_and_leaves_the_lock_usable() {
    let rig = Rig::new();
    rig.drive(20, 90, 80, 90.0);
    rig.sensors.explode(true);
    rig.daemon.guarded_step();
    assert_eq!(
        rig.snap().duty,
        [100, 100],
        "a failing step while hot must not leave the fans low: {:?}",
        rig.snap()
    );
    assert!(rig.log.has("step failed: sensor exploded"));
    // The panic happened with the state lock held. It must not be stuck (the deadlock the Go
    // port found on the laptop): requests, the next tick and the shutdown all still work.
    assert_eq!(rig.ok("ping", args()), json!("pong"));
    assert_eq!(rig.ok("state", args())["live"]["hw_ok"], json!(true));
    rig.sensors.explode(false);
    rig.drive(2, 90, 80, 90.0);
    rig.sensors.explode(true);
    rig.daemon.guarded_step();
    rig.daemon.shutdown();
    assert_eq!(rig.snap().fan, SimFan::Auto);
}

#[test]
fn a_step_that_panics_while_cool_leaves_the_fans_alone_and_before_any_reading_counts_as_hot() {
    let rig = Rig::new();
    rig.drive(20, 40, 40, 2.0);
    rig.sensors.explode(true);
    rig.daemon.guarded_step();
    assert_eq!(rig.snap().fan, SimFan::Auto, "a cool machine keeps the firmware's control");
    // A machine whose temperature was never read is treated as hot.
    let fresh = Rig::new();
    fresh.sensors.explode(true);
    fresh.daemon.guarded_step();
    assert_eq!(fresh.snap().duty, [100, 100]);
}

#[test]
fn a_panic_in_the_tick_does_not_kill_the_loop_or_the_next_tick() {
    let rig = Rig::new();
    rig.drive(5, 50, 50, 10.0);
    rig.sensors.explode(true);
    rig.drive(3, 50, 50, 10.0);
    rig.sensors.explode(false);
    let before = rig.daemon.ticks();
    rig.drive(2, 50, 50, 10.0);
    assert_eq!(rig.daemon.ticks(), before + 2);
}

#[test]
fn the_boot_look_is_stored_and_applied() {
    let rig = Rig::new();
    assert_eq!(rig.ok("boot_look", args()), Value::Null, "no boot look yet");
    for bad in [json!({}), json!("x"), Value::Null, json!({"keyboard": {}}), json!({"rear": {}})] {
        assert_eq!(
            rig.call("boot_look", args().with("action", "set").with("look", bad.clone())).unwrap_err(),
            "a look needs keyboard and/or rear settings",
            "{bad}"
        );
    }
    assert!(rig.call("boot_look", args().with("action", "set")).is_err());
    let look = json!({"keyboard": {"mode": "effect", "name": "wave", "color": "00ff00", "speed": 4, "brightness": 20, "direction": 0},
                       "rear": {"mode": "breathing", "color": "ff0000", "speed": 2, "brightness": 80, "direction": 1}});
    let stored = rig.ok("boot_look", args().with("action", "set").with("look", look.clone()));
    assert_eq!(stored, look, "the reply echoes the look, as Python did");
    assert_eq!(rig.ok("boot_look", args())["keyboard"]["name"], json!("wave"));
    assert!(rig.looks.boot.lock().unwrap().is_some());
    rig.daemon.boot();
    rig.daemon.join_background();
    assert_eq!(rig.looks.applied.lock().unwrap().len(), 1, "the boot look reached the lights");
    assert!(rig.log.has("boot look (boot): keyboard: wave"));
    rig.ok("boot_look", args().with("action", "clear"));
    assert_eq!(rig.ok("boot_look", args()), Value::Null);
    assert!(rig.call("boot_look", args().with("action", "zap")).is_err());
    let _: Look = serde_json::from_value(look).unwrap();
}

#[test]
fn the_boot_look_is_retried_while_the_keyboard_enumerates_and_gives_up_after_20_tries() {
    let rig = Rig::new();
    *rig.looks.boot.lock().unwrap() = Some(
        serde_json::from_value(
            json!({"rear": {"mode": "static", "color": "ff0000", "speed": 1, "brightness": 50}}),
        )
        .unwrap(),
    );
    rig.looks.fail_first.store(3, Ordering::SeqCst);
    rig.daemon.apply_boot_look("boot");
    rig.daemon.join_background();
    assert_eq!(rig.looks.applied.lock().unwrap().len(), 1, "applied on the fourth try");
    assert_eq!(rig.sleeper.calls.load(Ordering::SeqCst), 3, "one second between tries, never a real wait");
    let rig = Rig::new();
    *rig.looks.boot.lock().unwrap() = Some(
        serde_json::from_value(
            json!({"rear": {"mode": "static", "color": "ff0000", "speed": 1, "brightness": 50}}),
        )
        .unwrap(),
    );
    rig.looks.fail_first.store(1000, Ordering::SeqCst);
    rig.daemon.apply_boot_look("boot");
    rig.daemon.join_background();
    assert!(rig.log.has("boot look (boot) failed: keyboard not found"));
    assert_eq!(rig.sleeper.calls.load(Ordering::SeqCst), 19);
}

#[test]
fn the_boot_look_is_put_back_after_a_resume_from_suspend() {
    let rig = Rig::new();
    *rig.looks.boot.lock().unwrap() = Some(
        serde_json::from_value(
            json!({"rear": {"mode": "static", "color": "ff0000", "speed": 1, "brightness": 50}}),
        )
        .unwrap(),
    );
    rig.drive(3, 40, 40, 2.0);
    rig.daemon.join_background();
    assert!(rig.looks.applied.lock().unwrap().is_empty(), "no resume yet");
    rig.clock.suspend(3600.0);
    rig.drive(1, 40, 40, 2.0);
    rig.daemon.join_background();
    assert_eq!(rig.looks.applied.lock().unwrap().len(), 1);
    assert!(rig.log.has("boot look (resume from suspend)"));
}

#[test]
fn power_managers_and_touchpad() {
    let rig = Rig::new();
    let status = rig.ok("power_managers", args());
    assert_eq!(
        (status["on"].clone(), status["found"].clone(), status["recorded"].clone()),
        (json!(false), json!([]), json!(false))
    );
    *rig.pm.found.lock().unwrap() = vec![UnitStatus { unit: "tlp".to_owned(), active: true, enabled: true }];
    assert_eq!(rig.ok("power_managers", args())["found"][0]["unit"], json!("tlp"));
    let taken = rig.ok("power_managers", args().with("action", "takeover"));
    assert_eq!((taken["on"].clone(), taken["stopped"].clone()), (json!(true), json!(["tlp"])));
    assert!(rig.state.get_bool("pm_takeover", false), "takeover is remembered");
    assert_eq!(rig.ok("power_managers", args())["recorded"], json!(true));
    let restored = rig.ok("power_managers", args().with("action", "restore"));
    assert_eq!(restored["on"], json!(false));
    assert!(!rig.state.get_bool("pm_takeover", true), "restore clears it");
    assert!(rig.call("power_managers", args().with("action", "zap")).is_err());

    assert_eq!(rig.ok("touchpad", args()), json!(true));
    assert_eq!(rig.ok("touchpad", args().with("action", "toggle")), json!(false));
    assert_eq!(rig.ok("touchpad", args().with("action", "status")), json!(false));
    assert_eq!(rig.ok("touchpad", args().with("action", "on")), json!(true));
    assert_eq!(rig.ok("touchpad", args().with("action", "off")), json!(false));
    assert!(rig.call("touchpad", args().with("action", "sideways")).is_err());
    *rig.touchpad.on.lock().unwrap() = None;
    assert_eq!(rig.ok("touchpad", args()), Value::Null, "a touchpad that cannot be read is null");
}

#[test]
fn a_power_manager_that_came_back_is_stopped_again_while_the_takeover_is_on() {
    let rig = Rig::new();
    rig.ok("power_managers", args().with("action", "takeover"));
    rig.drive(2, 40, 40, 2.0);
    assert_eq!(rig.pm.takeovers.load(Ordering::SeqCst), 1);
    *rig.pm.found.lock().unwrap() = vec![UnitStatus { unit: "tlp".to_owned(), active: true, enabled: false }];
    rig.drive(160, 40, 40, 2.0);
    assert_eq!(rig.pm.takeovers.load(Ordering::SeqCst), 2, "stopped again, once");
    assert!(rig.log.has("power managers stopped again: tlp"));
    assert!(rig.osd.shown.lock().unwrap().iter().any(|t| t == "Stopped another power manager: tlp"));
}

#[test]
fn the_operating_mode_light_follows_the_power_mode_when_asked() {
    let rig = Rig::new();
    rig.auto_mode("native");
    rig.drive(6, 40, 40, 2.0);
    assert!(rig.keyboard.followed.lock().unwrap().is_empty(), "off by default");
    rig.ok("set", args().with("key", "operating_follow").with("value", true));
    rig.drive(6, 40, 40, 2.0);
    assert_eq!(*rig.keyboard.followed.lock().unwrap(), ["balanced"], "once per change, not once per tick");
    rig.ok("power", args().with("mode", "turbo"));
    rig.auto_mode("off");
    rig.drive(6, 40, 40, 2.0);
    assert_eq!(*rig.keyboard.followed.lock().unwrap(), ["balanced", "turbo"]);
}

#[test]
fn a_power_mode_change_is_announced_at_most_every_thirty_seconds() {
    let rig = Rig::new();
    rig.auto_mode("native");
    rig.drive(3, 40, 40, 2.0);
    let shown = rig.osd.shown.lock().unwrap().clone();
    assert_eq!(shown, ["Power mode: balanced (native auto: on power)"]);
    rig.sensors.set_ac(false);
    rig.drive(3, 40, 40, 2.0);
    assert_eq!(rig.osd.shown.lock().unwrap().len(), 2, "an urgent change (the plug) is announced at once");
}

#[test]
fn nothing_smart_runs_by_default_and_the_pace_adapts() {
    let rig = Rig::new();
    rig.auto_mode("native");
    rig.ok("set", args().with("key", "smart_fan").with("value", false));
    rig.drive(10, 45, 45, 2.0);
    assert_eq!(rig.daemon.interval().as_secs(), 2, "hands off: every two seconds");
    assert_eq!(
        rig.sensors.gpu_streams.load(Ordering::SeqCst),
        0,
        "the GPU sampler is only started when a smart feature needs it"
    );
    rig.auto_mode("smart");
    rig.drive(3, 45, 45, 2.0);
    assert_eq!(
        rig.daemon.interval().as_secs(),
        1,
        "smart: every second (with the smart fan off the calm count never grows)"
    );
    assert!(rig.sensors.gpu_streams.load(Ordering::SeqCst) > 0);
    rig.ok_set_smart_fan(true);
    rig.drive(3, 45, 45, 2.0);
    assert_eq!(rig.daemon.interval().as_secs(), 1, "smart and not calm for long yet: a second");
    rig.drive(40, 45, 45, 2.0);
    assert_eq!(rig.daemon.interval().as_secs(), 2, "calm for a long time: back to two seconds");
}

#[test]
fn a_replug_makes_the_fans_and_the_mode_be_written_again() {
    let rig = Rig::new();
    rig.drive(8, 70, 60, 90.0);
    assert_eq!(rig.snap().fan, SimFan::Manual);
    // Something resets the fans behind the daemon's back when the charger is unplugged.
    rig.sim.run("set_fan_auto").unwrap();
    rig.sensors.set_ac(false);
    rig.drive(2, 70, 60, 90.0);
    assert_eq!(rig.snap().fan, SimFan::Manual, "re-asserted after the plug change");
}

#[test]
fn the_fans_are_re_asserted_when_something_resets_them() {
    let rig = Rig::new();
    rig.drive(8, 70, 60, 90.0);
    rig.sim.run("set_fan_auto").unwrap();
    rig.drive(16, 70, 60, 90.0);
    assert_eq!(rig.snap().fan, SimFan::Manual, "verified every 15 ticks, then written again");
}

#[test]
fn a_saved_curve_and_profile_that_no_longer_validate_fall_back_to_the_built_in() {
    let setup = Setup {
        settings: vec![
            ("fan_curve_data", json!({"cpu": [[1, 2]]})),
            ("smart_profile_data", json!({"ac_busy": "warp"})),
        ],
        ..Setup::default()
    };
    let rig = Rig::with(setup);
    assert!(rig.log.has("fan curve invalid, using the built-in"));
    assert!(rig.log.has("smart profile invalid, using the built-in"));
    rig.drive(8, 70, 60, 90.0);
    assert_eq!(rig.snap().fan, SimFan::Manual, "the built-in curve runs");
}

#[test]
fn a_real_first_start_changes_nothing_but_the_power_mode() {
    // The settings a first start gets from the configuration: smart fan off, native auto, the
    // built-in curve and Smart profile named `native`.
    let config = ts_foundation::config::Config { user: "tester".to_owned(), ..Default::default() };
    let settings: Vec<(&'static str, Value)> = [
        ("smart_fan", json!(false)),
        ("auto_profile", json!(true)),
        ("auto_mode", json!("native")),
        ("fan_curve", json!("native")),
        ("smart_profile", json!("native")),
        ("pl_user", Value::Null),
    ]
    .into_iter()
    .collect();
    let rig = Rig::with(Setup { config, settings, ..Setup::default() });
    *rig.power.raw.lock().unwrap() = RawLimits { pl1_uw: 95_000_000, pl2_uw: 95_000_000 };
    rig.daemon.boot();
    rig.drive(40, 88, 70, 95.0);
    assert_eq!(rig.snap().fan, SimFan::Auto);
    assert!(rig.cap.caps.lock().unwrap().is_empty());
    assert!(rig.power.writes.lock().unwrap().is_empty());
    assert_eq!(rig.snap().mode, 1);
    assert_eq!(live(&rig)["auto_mode"], json!("native"));
    assert_eq!(live(&rig)["smart_profile"], json!("native"));
}

#[test]
fn boot_puts_the_users_saved_power_limits_back_before_the_first_tick() {
    let rig = Rig::new();
    rig.state.set("pl_user", json!({"pl1": 60.0, "pl2": 75.0})).unwrap();
    rig.daemon.boot();
    assert_eq!(*rig.power.raw.lock().unwrap(), RawLimits { pl1_uw: 60_000_000, pl2_uw: 75_000_000 });
    // And what the firmware had set is remembered to hand back.
    rig.ok("cpu_limits", args().with("reset", true));
    rig.ok("set", args().with("key", "smart_fan").with("value", false));
    rig.auto_mode("native");
    rig.drive(10, 40, 40, 2.0);
    assert_eq!(*rig.power.raw.lock().unwrap(), RawLimits { pl1_uw: 80_000_000, pl2_uw: 100_000_000 });
}

#[test]
fn boot_creates_the_run_directory_and_a_dead_device_does_not_stop_it() {
    let rig = Rig::new();
    std::fs::remove_dir_all(rig.path("run")).unwrap();
    rig.device.dead.store(true, Ordering::SeqCst);
    rig.daemon.boot();
    assert!(rig.path("run").is_dir());
    rig.daemon.shutdown(); // the fans cannot be reached: logged, nothing else
    assert!(rig.log.has("could not hand the fans back to the firmware"));
    assert!(rig.cap.uncapped.load(Ordering::SeqCst) >= 1, "the clock cap is lifted regardless");
}

#[test]
fn deps_helpers_have_sensible_defaults() {
    use super::deps::{Clock, LivePaths, PortError, SystemClock};
    let clock = SystemClock::new();
    let (a, b) = (clock.mono(), clock.mono());
    assert!(b >= a && clock.wall() > 1_600_000_000.0);
    let paths = LivePaths::in_run_dir("/run/truesense");
    assert_eq!(paths.run_state, std::path::PathBuf::from("/run/truesense/state"));
    assert_eq!(paths.tmp_state, std::path::PathBuf::from("/tmp/truesense-state"));
    assert_eq!(PortError::new("x").to_string(), "x");
}
