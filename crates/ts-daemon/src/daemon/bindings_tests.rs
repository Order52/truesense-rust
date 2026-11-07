//! Mode bindings against the fake laptop: what the daemon applies when the power mode changes,
//! whoever changed it, and what it must leave alone.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::Ordering;

use serde_json::{Value, json};
use ts_decision::curves::Curve;
use ts_ipc::wire::Args;
use ts_platform::firmware::SimFan;
use ts_platform::hw::{Coded, PowerMode};

use super::testkit::{Rig, Setup};

fn look(name: &str) -> Value {
    json!({"name": name, "data": {"keyboard": {"mode": "effect", "name": "wave", "color": "ff0000",
        "speed": 3, "brightness": 30}, "rear": {"mode": "static", "color": "00ff00", "speed": 3, "brightness": 50}}})
}

fn curve(name: &str, builtin: &str) -> Value {
    json!({"name": name, "data": Curve::builtin(builtin).unwrap().to_value()})
}

fn bind(rig: &Rig, mode: &str, parts: &[(&str, Value)]) -> Result<Value, String> {
    let mut args = Args::new().with("action", "set").with("mode", mode);
    for (key, value) in parts {
        args = args.with(key, value.clone());
    }
    rig.call("mode_bindings", args)
}

fn bindings(rig: &Rig) -> Value {
    rig.ok("mode_bindings", Args::new())["bindings"].clone()
}

/// A quiet laptop on AC whose automatic mode is off, so only the test changes the mode, with the
/// first ticks done (the first mode seen is not a change).
fn calm() -> Rig {
    let rig = Rig::new();
    rig.auto_mode("off");
    rig.drive(4, 40, 40, 2.0);
    rig
}

/// The user's pick through the daemon.
fn pick(rig: &Rig, mode: &str) {
    rig.ok("power", Args::new().with("mode", mode));
}

fn applied_looks(rig: &Rig) -> usize {
    rig.looks.applied.lock().unwrap().len()
}

fn active_curve(rig: &Rig) -> String {
    rig.state.get_str("fan_curve", "")
}

#[test]
fn without_bindings_a_mode_change_touches_nothing_of_the_lights_or_the_curve() {
    let rig = calm();
    let before = active_curve(&rig);
    for mode in ["turbo", "eco", "quiet", "balanced"] {
        pick(&rig, mode);
        rig.drive(4, 40, 40, 2.0);
    }
    assert_eq!(applied_looks(&rig), 0);
    assert_eq!(active_curve(&rig), before);
    assert!(rig.state.get("mode_bindings").is_none(), "nothing was stored");
    assert_eq!(bindings(&rig), json!({}));
}

#[test]
fn the_users_pick_applies_the_look_and_the_curve_once() {
    let rig = calm();
    bind(&rig, "turbo", &[("look", look("red")), ("fan_curve", curve("loud", "performance"))]).unwrap();
    assert_eq!(applied_looks(&rig), 0, "setting a binding applies nothing");
    pick(&rig, "turbo");
    rig.drive(10, 40, 40, 2.0);
    assert_eq!(applied_looks(&rig), 1, "once, not once per tick");
    let look = rig.looks.applied.lock().unwrap()[0].clone();
    assert_eq!(look.keyboard.as_ref().unwrap().name, "wave");
    assert_eq!(look.rear.as_ref().unwrap().color, "00ff00");
    assert_eq!(active_curve(&rig), "loud");
    assert_eq!(rig.state.get("fan_curve_data").unwrap(), Curve::builtin("performance").unwrap().to_value());
    assert!(rig.state.get_bool("smart_fan", false), "Smart stays on");
    assert!(rig.log.has("mode binding turbo: look red: applied"), "{}", rig.log.all());
    assert!(rig.log.has("mode binding turbo: fan curve loud: applied"));
}

#[test]
fn a_mode_without_a_binding_leaves_the_last_look_and_curve_in_place() {
    let rig = calm();
    bind(&rig, "turbo", &[("look", look("red")), ("fan_curve", curve("loud", "performance"))]).unwrap();
    pick(&rig, "turbo");
    rig.drive(4, 40, 40, 2.0);
    pick(&rig, "quiet");
    rig.drive(4, 40, 40, 2.0);
    assert_eq!(applied_looks(&rig), 1);
    assert_eq!(active_curve(&rig), "loud", "the user's choice is never undone by a mode without a binding");
}

#[test]
fn the_first_mode_after_start_is_not_a_change_but_the_next_one_is() {
    let rig = Rig::with(Setup {
        settings: vec![(
            "mode_bindings",
            json!({"eco": {"look": look("calm")}, "turbo": {"look": look("red")}}),
        )],
        ..Setup::default()
    });
    rig.auto_mode("off");
    rig.drive(6, 40, 40, 2.0);
    assert_eq!(rig.snap().mode, 6, "the simulated laptop starts in eco, which is bound");
    assert_eq!(applied_looks(&rig), 0, "a restart does not repaint the lights");
    pick(&rig, "turbo");
    rig.drive(4, 40, 40, 2.0);
    assert_eq!(applied_looks(&rig), 1);
}

#[test]
fn a_change_made_behind_the_daemons_back_counts_too() {
    // The PREDATOR key and the key service set the mode in the firmware directly.
    let rig = calm();
    bind(&rig, "performance", &[("look", look("fast"))]).unwrap();
    rig.daemon.hw.set_mode(PowerMode::from_name("performance").unwrap()).unwrap();
    rig.drive(6, 40, 40, 2.0);
    assert_eq!(applied_looks(&rig), 1);
}

#[test]
fn the_smart_auto_engine_changing_the_mode_applies_the_binding() {
    let rig = Rig::new();
    rig.auto_mode("smart");
    bind(&rig, "performance", &[("look", look("work")), ("fan_curve", curve("steady", "quiet"))]).unwrap();
    rig.drive(70, 40, 40, 5.0);
    assert_eq!(rig.snap().mode, 1, "idle on AC: balanced");
    assert_eq!(applied_looks(&rig), 0);
    rig.drive(10, 70, 60, 90.0);
    assert_eq!(rig.snap().mode, 4, "heavy load: performance");
    assert_eq!(applied_looks(&rig), 1);
    assert_eq!(active_curve(&rig), "steady");
}

#[test]
fn a_game_rule_changing_the_mode_applies_the_binding() {
    let rig = Rig::new();
    rig.auto_mode("native");
    rig.ok(
        "game_rules",
        Args::new().with("action", "add").with("name", "hl2_linux").with("mode", "performance"),
    );
    bind(&rig, "performance", &[("look", look("game"))]).unwrap();
    rig.procs.none();
    rig.drive(6, 40, 40, 5.0);
    rig.procs.run("hl2_linux");
    rig.drive(8, 40, 40, 5.0);
    assert_eq!(rig.snap().mode, 4);
    assert_eq!(applied_looks(&rig), 1);
}

#[test]
fn a_manual_fan_setting_stays_and_only_the_look_is_applied() {
    let rig = calm();
    bind(&rig, "turbo", &[("look", look("red")), ("fan_curve", curve("loud", "performance"))]).unwrap();
    let before = active_curve(&rig);
    rig.ok_fan_manual(55);
    pick(&rig, "turbo");
    rig.drive(6, 40, 40, 2.0);
    assert_eq!(applied_looks(&rig), 1, "the look is not a fan setting");
    assert_eq!(active_curve(&rig), before, "the curve did not become active");
    assert!(!rig.state.get_bool("smart_fan", true), "Smart was not switched on");
    assert_eq!(rig.snap().duty, [55, 55], "the manual duty stays");
    assert!(rig.log.has("fan curve loud: skipped (a manual fan setting is held)"), "{}", rig.log.all());
}

#[test]
fn firmware_auto_fans_are_not_turned_into_smart() {
    let rig = calm();
    bind(&rig, "turbo", &[("fan_curve", curve("loud", "performance"))]).unwrap();
    rig.ok("fan", Args::new().with("mode", "auto"));
    let before = active_curve(&rig);
    pick(&rig, "turbo");
    rig.drive(6, 40, 40, 2.0);
    assert_eq!(active_curve(&rig), before);
    assert_eq!(rig.snap().fan, SimFan::Auto);
    assert!(!rig.state.get_bool("smart_fan", true));
    assert!(rig.log.has("skipped (the fans are not in Smart mode)"));
}

#[test]
fn max_fans_set_by_the_user_are_kept_as_a_manual_setting() {
    let rig = calm();
    bind(&rig, "turbo", &[("fan_curve", curve("loud", "performance"))]).unwrap();
    rig.ok("fan", Args::new().with("mode", "max"));
    let before = active_curve(&rig);
    pick(&rig, "turbo");
    rig.drive(6, 40, 40, 2.0);
    assert_eq!(active_curve(&rig), before);
    assert!(rig.log.has("a manual fan setting is held"));
}

#[test]
fn a_binding_that_fails_validation_is_never_applied_and_does_not_block_the_others() {
    // The state file was edited by hand: a curve that would leave the CPU uncooled, a look that is
    // not a look. The good look next to the bad curve still applies.
    let rig = Rig::with(Setup {
        settings: vec![(
            "mode_bindings",
            json!({
                "turbo": {
                    "look": look("red"),
                    "fan_curve": {"name": "bad", "data": {"cpu": [[40, 30], [60, 40]], "gpu": [[40, 30], [60, 40]]}}
                },
                "eco": {"look": {"name": "junk", "data": {"nothing": 1}}}
            }),
        )],
        ..Setup::default()
    });
    rig.auto_mode("off");
    rig.drive(4, 40, 40, 2.0);
    let before = active_curve(&rig);
    pick(&rig, "turbo");
    rig.drive(4, 40, 40, 2.0);
    assert_eq!(applied_looks(&rig), 1, "the valid part applies");
    assert_eq!(active_curve(&rig), before, "the invalid curve does not");
    assert!(rig.log.has("fan curve bad: failed (invalid, not applied"), "{}", rig.log.all());
    pick(&rig, "eco");
    rig.drive(4, 40, 40, 2.0);
    assert_eq!(applied_looks(&rig), 1, "a look that is not a look is not applied");
    assert!(rig.log.has("look junk: failed (invalid, not applied"));
}

#[test]
fn unreadable_stored_bindings_apply_nothing_and_say_so() {
    let rig = Rig::with(Setup { settings: vec![("mode_bindings", json!("garbage"))], ..Setup::default() });
    rig.auto_mode("off");
    rig.drive(4, 40, 40, 2.0);
    pick(&rig, "turbo");
    rig.drive(4, 40, 40, 2.0);
    assert_eq!(applied_looks(&rig), 0);
    assert!(rig.log.has("mode bindings not applied: mode bindings are malformed"), "{}", rig.log.all());
    assert!(rig.call("mode_bindings", Args::new()).is_err(), "get says so too");
}

#[test]
fn a_look_that_fails_to_apply_is_logged_and_not_retried() {
    let rig = calm();
    bind(&rig, "turbo", &[("look", look("red"))]).unwrap();
    rig.looks.fail_first.store(1, Ordering::SeqCst);
    pick(&rig, "turbo");
    rig.drive(30, 40, 40, 2.0);
    assert_eq!(applied_looks(&rig), 0, "the one failure was not followed by a retry (a retry would succeed)");
    assert_eq!(rig.looks.fail_first.load(Ordering::SeqCst), 0, "it was tried exactly once");
    assert!(rig.log.has("look red: failed (keyboard not found)"), "{}", rig.log.all());
    // The next change tries again.
    pick(&rig, "quiet");
    rig.drive(4, 40, 40, 2.0);
    pick(&rig, "turbo");
    rig.drive(4, 40, 40, 2.0);
    assert_eq!(applied_looks(&rig), 1);
}

#[test]
fn a_failed_look_does_not_stop_the_curve() {
    let rig = calm();
    bind(&rig, "turbo", &[("look", look("red")), ("fan_curve", curve("loud", "performance"))]).unwrap();
    rig.looks.fail_first.store(1, Ordering::SeqCst);
    pick(&rig, "turbo");
    rig.drive(4, 40, 40, 2.0);
    assert_eq!(active_curve(&rig), "loud");
}

#[test]
fn a_bound_look_beats_operating_follow_for_that_mode_only() {
    let rig = calm();
    rig.ok("set", Args::new().with("key", "operating_follow").with("value", true));
    rig.drive(6, 40, 40, 2.0);
    let before = rig.keyboard.followed.lock().unwrap().len();
    bind(&rig, "turbo", &[("look", look("red"))]).unwrap();
    pick(&rig, "turbo");
    rig.drive(8, 40, 40, 2.0);
    assert_eq!(applied_looks(&rig), 1);
    assert_eq!(rig.keyboard.followed.lock().unwrap().len(), before, "the colour did not repaint the look");
    pick(&rig, "quiet");
    rig.drive(8, 40, 40, 2.0);
    assert_eq!(rig.keyboard.followed.lock().unwrap().last().map(String::as_str), Some("quiet"));
}

#[test]
fn a_failed_look_leaves_the_mode_to_operating_follow() {
    let rig = calm();
    rig.ok("set", Args::new().with("key", "operating_follow").with("value", true));
    rig.drive(6, 40, 40, 2.0);
    bind(&rig, "turbo", &[("look", look("red"))]).unwrap();
    rig.looks.fail_first.store(1, Ordering::SeqCst);
    pick(&rig, "turbo");
    rig.drive(8, 40, 40, 2.0);
    assert_eq!(rig.keyboard.followed.lock().unwrap().last().map(String::as_str), Some("turbo"));
}

#[test]
fn going_back_and_forth_applies_at_every_change() {
    let rig = calm();
    bind(&rig, "turbo", &[("look", look("red"))]).unwrap();
    bind(&rig, "eco", &[("look", look("dim"))]).unwrap();
    for _ in 0..2 {
        pick(&rig, "turbo");
        rig.drive(4, 40, 40, 2.0);
        pick(&rig, "eco");
        rig.drive(4, 40, 40, 2.0);
    }
    assert_eq!(applied_looks(&rig), 4);
    pick(&rig, "eco");
    rig.drive(4, 40, 40, 2.0);
    assert_eq!(applied_looks(&rig), 4, "the same mode again is not a change");
}

#[test]
fn bindings_survive_a_restart_of_the_state_store() {
    let rig = calm();
    bind(&rig, "turbo", &[("look", look("red"))]).unwrap();
    let on_disk = rig.read(&rig.path("state.json"));
    let stored: Value = serde_json::from_str(&on_disk).unwrap();
    assert_eq!(stored["mode_bindings"]["turbo"]["look"]["name"], json!("red"));
}

#[test]
fn set_get_and_clear_through_the_command() {
    let rig = calm();
    let got = rig.ok("mode_bindings", Args::new());
    assert_eq!(got["bindings"], json!({}));
    assert_eq!(got["mode"], json!("eco"));
    let set = bind(&rig, "turbo", &[("look", look("red"))]).unwrap();
    assert_eq!(set["bindings"]["turbo"]["look"]["name"], json!("red"));
    // A second part keeps the first.
    let set = bind(&rig, "turbo", &[("fan_curve", curve("loud", "performance"))]).unwrap();
    assert_eq!(set["bindings"]["turbo"]["look"]["name"], json!("red"));
    assert_eq!(set["bindings"]["turbo"]["fan_curve"]["name"], json!("loud"));
    // The stored curve is the validated, normal form.
    assert_eq!(
        set["bindings"]["turbo"]["fan_curve"]["data"],
        Curve::builtin("performance").unwrap().to_value()
    );
    let cleared = rig
        .ok("mode_bindings", Args::new().with("action", "clear").with("mode", "turbo").with("what", "look"));
    assert_eq!(cleared["bindings"]["turbo"]["look"], Value::Null);
    assert_eq!(cleared["bindings"]["turbo"]["fan_curve"]["name"], json!("loud"));
    let cleared = rig.ok("mode_bindings", Args::new().with("action", "clear").with("mode", "turbo"));
    assert_eq!(cleared["bindings"], json!({}));
    // Clearing what is not there is fine.
    rig.ok("mode_bindings", Args::new().with("action", "clear").with("mode", "eco"));
}

#[test]
fn bad_requests_are_refused_and_change_nothing() {
    let rig = calm();
    bind(&rig, "turbo", &[("look", look("red"))]).unwrap();
    let before = bindings(&rig);
    let native = json!({"name": "n", "data": Curve::builtin("native").unwrap().to_value()});
    let stops = json!({"name": "s", "data": {"cpu": [[40, 0], [60, 0]], "gpu": [[40, 0], [60, 0]]}});
    let cases: Vec<(&str, Vec<(&str, Value)>)> = vec![
        ("ludicrous", vec![("look", look("x"))]),
        ("turbo", vec![]),
        ("turbo", vec![("look", json!("red"))]),
        ("turbo", vec![("look", json!({"name": "../x", "data": look("x")["data"]}))]),
        ("turbo", vec![("look", json!({"name": "x", "data": {}}))]),
        ("turbo", vec![("look", json!({"name": "x", "data": {"keyboard": {}}}))]),
        ("turbo", vec![("fan_curve", native)]),
        ("turbo", vec![("fan_curve", stops)]),
        ("turbo", vec![("fan_curve", json!({"name": "x", "data": null}))]),
        ("turbo", vec![("fan_curve", json!({"name": "x"}))]),
        ("Turbo", vec![("look", look("x"))]),
    ];
    for (mode, parts) in cases {
        assert!(bind(&rig, mode, &parts).is_err(), "{mode} {parts:?}");
    }
    for bad in [
        Args::new().with("action", "zap"),
        Args::new().with("action", "clear").with("mode", "turbo").with("what", "everything"),
        Args::new().with("action", "clear"),
        Args::new().with("action", "refresh").with("kind", "sound").with("name", "x").with("data", json!({})),
        Args::new().with("action", "refresh").with("kind", "look").with("name", "x"),
        Args::new().with("action", "apply_now").with("mode", "warp"),
    ] {
        assert!(rig.call("mode_bindings", bad.clone()).is_err(), "{bad:?}");
    }
    assert_eq!(bindings(&rig), before);
}

#[test]
fn refresh_replaces_stale_copies_by_name() {
    let rig = calm();
    bind(&rig, "turbo", &[("look", look("red"))]).unwrap();
    bind(&rig, "eco", &[("look", look("red"))]).unwrap();
    bind(&rig, "quiet", &[("look", look("other"))]).unwrap();
    let fresh = json!({"rear": {"mode": "static", "color": "0000ff", "speed": 3, "brightness": 20}});
    let r = rig.ok(
        "mode_bindings",
        Args::new()
            .with("action", "refresh")
            .with("kind", "look")
            .with("name", "red")
            .with("data", fresh.clone()),
    );
    assert_eq!(r["updated"], json!(2));
    assert_eq!(r["bindings"]["turbo"]["look"]["data"], fresh);
    assert_eq!(r["bindings"]["eco"]["look"]["data"], fresh);
    assert_eq!(r["bindings"]["quiet"]["look"]["data"], look("other")["data"]);
    let r = rig.ok(
        "mode_bindings",
        Args::new().with("action", "refresh").with("kind", "look").with("name", "absent").with("data", fresh),
    );
    assert_eq!(r["updated"], json!(0));
}

#[test]
fn apply_now_applies_the_current_modes_binding_and_reports_each_part() {
    let rig = calm();
    bind(&rig, "eco", &[("look", look("calm")), ("fan_curve", curve("steady", "quiet"))]).unwrap();
    assert_eq!(applied_looks(&rig), 0);
    let r = rig.ok("mode_bindings", Args::new().with("action", "apply_now"));
    assert_eq!(r["mode"], json!("eco"));
    assert_eq!(r["results"][0]["status"], json!("applied"));
    assert_eq!(r["results"][1]["target"], json!("fan_curve"));
    assert_eq!(r["results"][1]["status"], json!("applied"));
    assert_eq!(applied_looks(&rig), 1);
    assert_eq!(active_curve(&rig), "steady");
    // With a manual fan the curve is reported as skipped, not as an error.
    rig.ok_fan_manual(60);
    let r = rig.ok("mode_bindings", Args::new().with("action", "apply_now"));
    assert_eq!(r["results"][1]["status"], json!("skipped"));
    assert_eq!(rig.snap().duty, [60, 60]);
    // A mode without a binding has nothing to apply.
    let r = rig.ok("mode_bindings", Args::new().with("action", "apply_now").with("mode", "balanced"));
    assert_eq!(r["results"], json!([]));
    // A failure is reported, not raised, and not retried.
    rig.looks.fail_first.store(1, Ordering::SeqCst);
    let r = rig.ok("mode_bindings", Args::new().with("action", "apply_now"));
    assert_eq!(r["results"][0]["status"], json!("failed"));
    assert_eq!(rig.looks.fail_first.load(Ordering::SeqCst), 0);
}

#[test]
fn a_bound_curve_never_stops_a_fan() {
    // The bound curve has low duties; whatever the temperature the Smart follower writes either a
    // duty of at least 20 % or hands the fans to the firmware, and never a manual 0.
    let rig = calm();
    let low = json!({"name": "low", "data": {"cpu": [[40, 20], [70, 60], [95, 100]], "gpu": [[40, 20], [65, 60], [85, 100]]}});
    bind(&rig, "quiet", &[("fan_curve", low)]).unwrap();
    pick(&rig, "quiet");
    for temp in [30, 45, 60, 75, 90] {
        rig.drive(5, temp, temp, 20.0);
        let snap = rig.snap();
        assert!(
            snap.fan == SimFan::Auto || snap.duty.iter().all(|d| (20..=100).contains(d)),
            "{snap:?} at {temp}"
        );
    }
}
