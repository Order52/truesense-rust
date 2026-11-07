//! Tests of the action runner against the fake machine.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::err_expect,
    clippy::field_reassign_with_default
)]

use std::sync::Arc;

use serde_json::json;

use super::fakes::World;
use super::{ActionError, ActionRunner, FanCommand, Runner};

fn setup() -> (Arc<World>, Runner) {
    let w = World::new();
    let r = Runner::new(w.env());
    (w, r)
}

fn run(r: &Runner, spec: &str) -> String {
    r.run(spec).unwrap_or_else(|e| panic!("{spec}: {e}"))
}

fn fails(r: &Runner, spec: &str) -> String {
    r.run(spec).err().map(|e| e.to_string()).unwrap_or_else(|| panic!("{spec} should fail"))
}

#[test]
fn invalid_and_stale_actions_are_refused_before_anything_runs() {
    let (w, r) = setup();
    assert_eq!(
        fails(&r, "power:ludicrous"),
        "power mode must be one of quiet, balanced, performance, turbo, eco"
    );
    assert_eq!(fails(&r, "look:gone"), "no saved look 'gone' (truesense look list)");
    assert!(w.calls().is_empty(), "nothing touched the machine");
    assert!(r.validate("look:native").is_ok());
    assert!(matches!(r.run("nope"), Err(ActionError::Invalid(_))));
}

#[test]
fn power_steps_walk_the_configured_cycle_limited_to_supported_modes() {
    let (w, r) = setup();
    *w.mode.lock().unwrap() = "eco".into();
    *w.hold.lock().unwrap() = 30.0;
    // test_core::test_power_cycle_and_battery: eco -> quiet -> eco -> turbo
    assert_eq!(run(&r, "power-next"), "Power mode: quiet");
    assert_eq!(run(&r, "power-prev"), "Power mode: eco");
    assert_eq!(run(&r, "power:turbo"), "Power mode: turbo");
    assert_eq!(
        w.calls().into_iter().filter(|c| c.starts_with("power:")).collect::<Vec<_>>(),
        ["power:quiet:30", "power:eco:30", "power:turbo:30"]
    );
    // Wraps at both ends.
    *w.mode.lock().unwrap() = "turbo".into();
    assert_eq!(run(&r, "power-next"), "Power mode: eco");
    assert_eq!(run(&r, "power-prev"), "Power mode: turbo");
    // Unsupported modes are skipped.
    *w.supported.lock().unwrap() = vec!["quiet".into(), "balanced".into()];
    *w.mode.lock().unwrap() = "quiet".into();
    assert_eq!(run(&r, "power-next"), "Power mode: balanced");
    assert_eq!(run(&r, "power-next"), "Power mode: quiet");
}

#[test]
fn power_step_from_an_unlisted_mode_starts_at_the_beginning_or_one_before_the_end() {
    let (w, r) = setup();
    *w.mode.lock().unwrap() = "balanced".into();
    assert_eq!(run(&r, "power-cycle:performance,turbo"), "Power mode: performance");
    *w.mode.lock().unwrap() = "balanced".into();
    // prev from "not in the list" is index -2 mod 3, as in Python.
    assert_eq!(run(&r, "power-cycle:eco,quiet,turbo"), "Power mode: eco");
    *w.mode.lock().unwrap() = "balanced".into();
    *w.cycle.lock().unwrap() = vec!["eco".into(), "quiet".into(), "turbo".into()];
    assert_eq!(run(&r, "power-prev"), "Power mode: quiet");
    // Cycle of one mode.
    *w.mode.lock().unwrap() = "turbo".into();
    assert_eq!(run(&r, "power-cycle:turbo"), "Power mode: turbo");
}

#[test]
fn an_empty_supported_cycle_falls_back_to_eco_balanced_turbo() {
    let (w, r) = setup();
    *w.cycle.lock().unwrap() = vec!["warp".into()];
    *w.mode.lock().unwrap() = "eco".into();
    assert_eq!(run(&r, "power-next"), "Power mode: balanced");
    assert_eq!(run(&r, "power-next"), "Power mode: turbo");
    assert_eq!(run(&r, "power-next"), "Power mode: eco");
}

#[test]
fn power_cycle_action_ignores_the_configured_cycle() {
    let (w, r) = setup();
    *w.cycle.lock().unwrap() = vec!["eco".into()];
    *w.mode.lock().unwrap() = "performance".into();
    assert_eq!(run(&r, "power-cycle:performance,turbo"), "Power mode: turbo");
    assert_eq!(run(&r, "power-cycle:performance,turbo"), "Power mode: performance");
}

#[test]
fn the_configuration_is_read_on_every_run() {
    let (w, r) = setup();
    *w.hold.lock().unwrap() = 10.0;
    run(&r, "power:eco");
    *w.hold.lock().unwrap() = 99.0;
    run(&r, "power:eco");
    let calls: Vec<String> = w.calls().into_iter().filter(|c| c.starts_with("power:")).collect();
    assert_eq!(calls, ["power:eco:10", "power:eco:99"]);
    // A broken configuration is a clear error for a power action, and no problem for the others.
    w.fail("settings");
    assert_eq!(fails(&r, "power:eco"), "read configuration: config broken");
    assert_eq!(run(&r, "notify:still works"), "still works");
}

#[test]
fn fan_actions_and_the_key_can_never_stop_a_fan() {
    let (w, r) = setup();
    assert_eq!(run(&r, "fan:smart"), "Fans: smart curve");
    assert_eq!(run(&r, "fan:auto"), "Fans: firmware auto");
    assert_eq!(run(&r, "fan:max"), "Fans: MAX");
    assert_eq!(run(&r, "fan:60"), "Fans: 60 %");
    assert_eq!(w.calls(), ["fan:Smart", "fan:Auto", "fan:Max", "fan:Manual { cpu: 60, gpu: 60 }"]);
    w.clear();
    for bad in ["fan:0", "fan:19", "fan:00", "fan:-1"] {
        assert_eq!(fails(&r, bad), "fan: smart | auto | max | 20..100");
    }
    assert!(w.calls().is_empty(), "no fan command was sent");
    assert_eq!(FanCommand::Smart, FanCommand::Smart);
}

#[test]
fn fan_max_toggle_goes_to_smart_when_already_max() {
    let (w, r) = setup();
    assert_eq!(run(&r, "fan-max-toggle"), "Fans: MAX");
    assert_eq!(w.fan.lock().unwrap().mode, "max");
    assert_eq!(run(&r, "fan-max-toggle"), "Fans: smart curve");
    assert_eq!(run(&r, "fan-max-toggle"), "Fans: MAX");
}

#[test]
fn smart_fan_toggle_needs_the_daemon() {
    let (w, r) = setup();
    assert_eq!(run(&r, "smart-fan-toggle"), "Fans: smart curve");
    assert_eq!(run(&r, "smart-fan-toggle"), "Fans: smart curve OFF");
    assert_eq!(run(&r, "smart-fan-toggle"), "Fans: smart curve");
    *w.daemon_up.lock().unwrap() = false;
    assert_eq!(fails(&r, "smart-fan-toggle"), "the TrueSense daemon is not running");
}

#[test]
fn mode_control_actions() {
    let (w, r) = setup();
    assert_eq!(run(&r, "auto:smart"), "Mode control: smart");
    assert_eq!(w.daemon.lock().unwrap().get("auto_mode"), Some(&json!("smart")));
    // auto-next cycles off -> native -> smart -> off.
    w.daemon.lock().unwrap().insert("auto_mode".into(), json!("off"));
    assert_eq!(run(&r, "auto-next"), "Mode control: native");
    assert_eq!(run(&r, "auto-next"), "Mode control: smart");
    assert_eq!(run(&r, "auto-next"), "Mode control: off");
    // An unknown stored value goes to smart; a missing one counts as off.
    w.daemon.lock().unwrap().insert("auto_mode".into(), json!("weird"));
    assert_eq!(run(&r, "auto-next"), "Mode control: smart");
    w.daemon.lock().unwrap().remove("auto_mode");
    assert_eq!(run(&r, "auto-next"), "Mode control: native");
    *w.daemon_up.lock().unwrap() = false;
    assert_eq!(fails(&r, "auto-next"), "no daemon");
    assert_eq!(fails(&r, "auto:off"), "no daemon");
}

#[test]
fn profile_look_notify_none_exec() {
    let (w, r) = setup();
    assert_eq!(run(&r, "profile:gaming"), "Profile 'gaming' applied (0 settings)");
    assert_eq!(run(&r, "profile-next"), "Profile 'a' applied (0 settings)");
    w.state_put("profile", json!("a"));
    assert_eq!(run(&r, "profile-next"), "Profile 'b' applied (0 settings)");
    assert_eq!(run(&r, "look:native"), "Look: native");
    assert_eq!(run(&r, "notify:hello"), "hello");
    assert_eq!(run(&r, "none"), "");
    assert_eq!(run(&r, "exec:truesense gui"), "");
    assert_eq!(w.calls().last().map(String::as_str), Some("exec:truesense gui"));
}

#[test]
fn exec_without_a_spawner_is_refused() {
    let w = World::new();
    let mut env = w.env();
    env.spawner = None;
    let r = Runner::new(env);
    assert_eq!(fails(&r, "exec:true"), "exec: actions are not available here");
    assert!(matches!(r.run("exec:true"), Err(ActionError::Unavailable(_))));
}

#[test]
fn audio_actions() {
    let (w, r) = setup();
    assert_eq!(run(&r, "output-toggle"), "");
    assert_eq!(run(&r, "output-cycle"), "");
    assert_eq!(run(&r, "output:headphones"), "Audio output: Headphones (3.5 mm jack)");
    assert_eq!(run(&r, "output:speakers"), "Audio output: Laptop speakers");
    assert_eq!(run(&r, "mic-toggle"), "Microphones: MUTED");
    assert_eq!(run(&r, "mic-toggle"), "Microphones: live");
    assert_eq!(run(&r, "mic:mute"), "Microphones: MUTED");
    assert_eq!(run(&r, "mic:unmute"), "Microphones: live");
    assert_eq!(run(&r, "voice-next"), "Purified Voice (headset): Personal Call");
    assert_eq!(run(&r, "voice-next"), "Purified Voice (headset): Conference Call");
    assert_eq!(run(&r, "voice-next"), "Purified Voice (headset): All Off");
    assert_eq!(w.calls().len(), 11);
}

#[test]
fn touchpad_goes_through_the_daemon_and_root_falls_back_to_the_switch() {
    let (w, r) = setup();
    assert_eq!(run(&r, "touchpad-toggle"), "Touchpad: off");
    assert_eq!(run(&r, "touchpad:on"), "Touchpad: on");
    assert_eq!(run(&r, "touchpad:off"), "Touchpad: off");
    // No daemon and not root: the daemon's error.
    *w.daemon_up.lock().unwrap() = false;
    assert_eq!(fails(&r, "touchpad:on"), "no daemon");
    // Root without a daemon switches it itself.
    let mut env = w.env();
    env.local_touchpad = Some(w.clone());
    let root = Runner::new(env);
    assert_eq!(run(&root, "touchpad:on"), "Touchpad: on");
    assert_eq!(run(&root, "touchpad-toggle"), "Touchpad: off");
    assert_eq!(run(&root, "touchpad-toggle"), "Touchpad: on");
    assert_eq!(run(&root, "touchpad:off"), "Touchpad: off");
    let local: Vec<String> = w.calls().into_iter().filter(|c| c.starts_with("touchpad_local")).collect();
    assert_eq!(
        local,
        ["touchpad_local:true", "touchpad_local:false", "touchpad_local:true", "touchpad_local:false"]
    );
}

#[test]
fn keyboard_actions_follow_the_remembered_state() {
    let (w, r) = setup();
    // Default state is on, so toggle switches off.
    assert_eq!(run(&r, "keyboard:toggle"), "Keyboard light: off");
    assert_eq!(w.state_get("keyboard_on"), Some(json!(false)));
    // On again with nothing remembered: the default look.
    assert_eq!(run(&r, "keyboard:on"), "Keyboard light: on");
    // With a remembered look, restore it.
    w.state_put("kb_restore", json!({"mode": "effect"}));
    run(&r, "keyboard:off");
    run(&r, "keyboard:on");
    // With a saved profile of the remembered name and no restore point, apply the profile.
    w.state_put("kb_restore", json!(null));
    w.state_put("keyboard_profile", json!("mine"));
    w.kb_profiles.lock().unwrap().push("mine".into());
    run(&r, "keyboard:off");
    run(&r, "keyboard:toggle");
    assert_eq!(w.calls(), ["kb_off", "kb_default", "kb_off", "kb_on", "kb_off", "kb_profile:mine"]);
}

#[test]
fn keyboard_effect_next_steps_in_a_fixed_order_and_wraps() {
    let (w, r) = setup();
    let names = super::KB_EFFECTS;
    for (i, n) in names.iter().enumerate() {
        assert_eq!(run(&r, "keyboard-effect-next"), format!("Keyboard effect: {n}"), "step {i}");
    }
    assert_eq!(run(&r, "keyboard-effect-next"), "Keyboard effect: static", "wraps");
    assert_eq!(w.state_get("kb_effect"), Some(json!(0)));
    assert_eq!(w.state_get("keyboard_on"), Some(json!(true)));
    assert!(w.calls()[0].starts_with("kb_effect:static:00aec7:5:50"));
}

#[test]
fn keyboard_effect_never_starts_dim_and_neon_is_never_dim() {
    let (w, r) = setup();
    *w.kb_brightness.lock().unwrap() = 0;
    run(&r, "keyboard-effect-next");
    assert_eq!(w.calls()[0], "kb_effect:static:00aec7:5:30");
    *w.kb_brightness.lock().unwrap() = 10;
    w.state_put("kb_effect", json!(4));
    w.clear();
    assert_eq!(run(&r, "keyboard-effect-next"), "Keyboard effect: neon");
    assert_eq!(w.calls()[0], "kb_effect:neon:00aec7:5:30");
    // A stored index that is not a number starts over.
    w.state_put("kb_effect", json!("junk"));
    assert_eq!(run(&r, "keyboard-effect-next"), "Keyboard effect: static");
    // A float from a JSON round trip still counts.
    w.state_put("kb_effect", json!(1.0));
    assert_eq!(run(&r, "keyboard-effect-next"), "Keyboard effect: wave");
}

#[test]
fn keyboard_profile_action_remembers_it() {
    let (w, r) = setup();
    w.kb_profiles.lock().unwrap().push("mine".into());
    assert_eq!(run(&r, "keyboard-profile:mine"), "Keyboard profile: mine");
    assert_eq!(w.state_get("keyboard_profile"), Some(json!("mine")));
    assert_eq!(w.state_get("keyboard_on"), Some(json!(true)));
    assert_eq!(fails(&r, "keyboard-profile:nope"), "no profile 'nope'");
}

#[test]
fn rear_light_off_remembers_and_on_restores() {
    let (w, r) = setup();
    assert_eq!(run(&r, "rear:toggle"), "Rear light: off");
    assert_eq!(w.calls(), ["rear_set:static:112233:3:0:1"]);
    assert_eq!(
        w.state_get("rear_last"),
        Some(json!({"mode": "static", "color": "112233", "speed": 3, "brightness": 80, "direction": 1}))
    );
    assert_eq!(run(&r, "rear:toggle"), "Rear light: on");
    assert_eq!(w.calls()[1], "rear_set:static:112233:3:80:1");
    // Already on and asked for on: written again (idempotent), nothing remembered over it.
    assert_eq!(run(&r, "rear:on"), "Rear light: on");
}

#[test]
fn rear_light_keeps_zone_colours_for_the_static_effect_only() {
    let (w, r) = setup();
    w.state_put("rear_zones", json!(["ff0000", "00ff00", "0000ff"]));
    run(&r, "rear:off");
    run(&r, "rear:on");
    assert_eq!(
        w.calls(),
        [
            "rear_set:static:112233:3:0:1:ff0000,00ff00,0000ff",
            "rear_set:static:112233:3:80:1:ff0000,00ff00,0000ff"
        ]
    );
    // Not three zones, or not text: ignored.
    for bad in [json!(["ff0000"]), json!(["a", "b", "c", "d"]), json!(["a", 1, "c"]), json!("x")] {
        w.state_put("rear_zones", bad);
        w.clear();
        run(&r, "rear:off");
        assert_eq!(w.calls()[0].split(':').count(), 6, "{:?}", w.calls());
        run(&r, "rear:on");
    }
    // A non-static effect has no parts.
    w.state_put("rear_zones", json!(["ff0000", "00ff00", "0000ff"]));
    w.rear.lock().unwrap().mode = "wave".into();
    w.rear.lock().unwrap().brightness = 70;
    w.clear();
    run(&r, "rear:off");
    assert_eq!(w.calls(), ["rear_set:wave:112233:3:0:1"]);
}

#[test]
fn rear_light_handles_unknown_modes_and_hand_edited_memory() {
    let (w, r) = setup();
    // An effect code we do not know is written as static when switching off.
    w.rear.lock().unwrap().mode = "9".into();
    run(&r, "rear:off");
    assert_eq!(w.calls()[0], "rear_set:static:112233:3:0:1");
    // Nothing remembered: the default look, full brightness.
    let (w2, r2) = setup();
    w2.rear.lock().unwrap().brightness = 0;
    run(&r2, "rear:on");
    assert_eq!(w2.calls()[0], "rear_set:static:00aec7:3:100:1");
    // A remembered bar of brightness 0 comes back at 100; unknown mode becomes static; junk fields keep defaults.
    let (w3, r3) = setup();
    w3.rear.lock().unwrap().brightness = 0;
    w3.state_put(
        "rear_last",
        json!({"mode": "bogus", "color": 7, "speed": "fast", "brightness": 0, "direction": 0.0}),
    );
    run(&r3, "rear:on");
    assert_eq!(w3.calls()[0], "rear_set:static:00aec7:3:100:0");
    // An empty remembered object is the default.
    let (w4, r4) = setup();
    w4.rear.lock().unwrap().brightness = 0;
    w4.state_put("rear_last", json!({}));
    run(&r4, "rear:on");
    assert_eq!(w4.calls()[0], "rear_set:static:00aec7:3:100:1");
}

#[test]
fn rear_effect_next_steps_in_a_fixed_order() {
    let (w, r) = setup();
    for n in super::REAR_EFFECTS {
        assert_eq!(run(&r, "rear-effect-next"), format!("Rear effect: {n}"));
    }
    assert_eq!(run(&r, "rear-effect-next"), "Rear effect: static");
    // The colour, speed and direction are kept; a dark bar comes on at 100.
    w.rear.lock().unwrap().brightness = 0;
    w.clear();
    run(&r, "rear-effect-next");
    assert_eq!(w.calls(), ["rear_set:breathing:112233:3:100:1"]);
}

#[test]
fn battery_and_overdrive_toggles() {
    let (w, r) = setup();
    assert_eq!(run(&r, "battery-limit-toggle"), "Battery 80 % limit: on");
    assert_eq!(run(&r, "battery-limit-toggle"), "Battery 80 % limit: off");
    assert_eq!(run(&r, "overdrive-toggle"), "LCD overdrive: on");
    assert_eq!(run(&r, "overdrive-toggle"), "LCD overdrive: off");
    // Unknown counts as off, so the press turns it on.
    *w.overdrive.lock().unwrap() = None;
    assert_eq!(run(&r, "overdrive-toggle"), "LCD overdrive: on");
}

#[test]
fn machine_errors_surface_with_the_machines_words_and_the_step() {
    let (w, r) = setup();
    w.fail("power");
    let e = r.run("power:eco").err().expect("fails");
    assert_eq!(e.to_string(), "power failed");
    assert!(matches!(e, ActionError::Host { op: "power", .. }));
    w.fail("fan");
    assert_eq!(fails(&r, "fan:max"), "fan failed");
    w.fail("rear_set");
    assert_eq!(fails(&r, "rear:off"), "rear_set failed");
    w.fail("battery");
    assert_eq!(fails(&r, "battery-limit-toggle"), "battery failed");
}

#[test]
fn the_runner_is_usable_as_a_trait_object() {
    let (_w, r) = setup();
    let dynamic: Arc<dyn ActionRunner> = Arc::new(r);
    assert_eq!(dynamic.run("notify:x").as_deref(), Ok("x"));
}

#[test]
fn a_failed_state_write_never_fails_an_action() {
    // The memory is a convenience: a read-only disk must not stop a key press. The fake state
    // here always accepts, so use the real store on an impossible path.
    use ts_foundation::userstate;
    let file = std::env::temp_dir().join(format!("ts-domain-actions-{}-notadir", std::process::id()));
    std::fs::write(&file, b"x").unwrap();
    let w = World::new();
    let mut env = w.env();
    // A state file whose parent is a plain file can never be written.
    env.state = Arc::new(userstate::Store::new(file.join("user.json")));
    let r = Runner::new(env);
    let out = r.run("keyboard-effect-next");
    std::fs::remove_file(&file).unwrap();
    assert_eq!(out.as_deref(), Ok("Keyboard effect: static"));
}
