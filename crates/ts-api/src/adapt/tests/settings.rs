//! `set`, `power`, `hold`: the switches of the service and the power mode with its hold.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;
use ts_ipc::client::ClientError;
use ts_ipc::wire::Args;
use ts_platform::hw::PowerMode;

use super::rig::Rig;
use crate::api::{ApiError, AutoAction, AutoMode};

fn daemon_error(e: &ApiError) -> &str {
    match e {
        ApiError::Daemon(ClientError::Daemon(text)) => text,
        other => panic!("not a daemon refusal: {other:?}"),
    }
}

#[test]
fn set_flips_each_switch_and_answers_the_whole_settings_object() {
    let rig = Rig::new();
    for key in ["smart_fan", "operating_follow"] {
        let settings = rig.api.daemon_set(key, true).unwrap();
        // Python `set_` returns `self.st.snapshot()`: every setting, not just the one changed.
        assert_eq!(settings[key], json!(true), "{key}");
        assert!(settings.contains_key("auto_mode") && settings.contains_key("fan_curve"));
        assert_eq!(rig.api.daemon_state().unwrap().setting_bool(key), Some(true));
        assert_eq!(rig.api.daemon_set(key, false).unwrap()[key], json!(false));
    }
    assert!(rig.api.set_smart_fan(true).unwrap());
    assert!(rig.api.smart_fan().unwrap());
    assert!(rig.api.set_operating_follow(true).unwrap());
    assert!(rig.api.operating_follow().unwrap());
}

#[test]
fn set_auto_profile_and_auto_mode_stay_in_step_like_python() {
    let rig = Rig::new();
    // `auto_profile` true is the smart mode, false is off.
    let s = rig.api.daemon_set("auto_profile", true).unwrap();
    assert_eq!((s["auto_profile"].clone(), s["auto_mode"].clone()), (json!(true), json!("smart")));
    let s = rig.api.daemon_set("auto_profile", false).unwrap();
    assert_eq!((s["auto_profile"].clone(), s["auto_mode"].clone()), (json!(false), json!("off")));
    // `auto_mode` sets `auto_profile` to "not off".
    for (mode, profile) in [("native", true), ("smart", true), ("off", false)] {
        let s = rig.api.daemon_set("auto_mode", mode).unwrap();
        assert_eq!(
            (s["auto_mode"].clone(), s["auto_profile"].clone()),
            (json!(mode), json!(profile)),
            "{mode}"
        );
    }
}

#[test]
fn the_automatic_mode_commands_round_trip() {
    let rig = Rig::new();
    assert_eq!(rig.api.auto_mode().unwrap(), AutoMode::Off);
    rig.api.set_auto_mode(AutoMode::Native).unwrap();
    assert_eq!(rig.api.auto_mode().unwrap(), AutoMode::Native);
    assert_eq!(rig.api.apply_auto_mode(AutoAction::Toggle).unwrap(), AutoMode::Off);
    assert_eq!(rig.api.apply_auto_mode(AutoAction::On).unwrap(), AutoMode::Smart);
    assert_eq!(rig.api.apply_auto_mode(AutoAction::Status).unwrap(), AutoMode::Smart);
    assert_eq!(rig.api.apply_auto_mode(AutoAction::Set(AutoMode::Native)).unwrap(), AutoMode::Native);
    // The loop publishes the kind it follows.
    rig.ticks(2);
    let st = rig.api.daemon_state().unwrap();
    assert_eq!(st.live_str("auto_mode"), Some("native"));
    assert_eq!(st.live_bool("auto"), Some(true));
}

#[test]
fn set_refuses_what_python_refuses_in_its_words() {
    let rig = Rig::new();
    let err = rig.api.daemon_set("colour", true).unwrap_err();
    assert_eq!(daemon_error(&err), "unknown setting 'colour'");
    let err = rig.api.daemon_set("smart_fan", "yes").unwrap_err();
    assert_eq!(daemon_error(&err), "value must be true or false");
    let err = rig.api.daemon_set("smart_fan", 1).unwrap_err();
    assert_eq!(daemon_error(&err), "value must be true or false");
    let err = rig.api.daemon_set("auto_mode", "sometimes").unwrap_err();
    assert_eq!(daemon_error(&err), "auto_mode: off | native | smart");
    let err = rig.api.daemon_set("auto_mode", true).unwrap_err();
    assert_eq!(daemon_error(&err), "auto_mode: off | native | smart");
    // A request with no `key` at all is an error, not a crash.
    assert!(rig.client.call("set", Args::new().with("value", true)).is_err());
}

#[test]
fn settings_survive_in_the_state_file_the_python_daemon_wrote_to() {
    let rig = Rig::new();
    rig.api.daemon_set("smart_fan", true).unwrap();
    let text = std::fs::read_to_string(rig.dir.0.join("state.json")).unwrap();
    let on_disk: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(on_disk["smart_fan"], json!(true));
}

#[test]
fn power_sets_the_firmware_and_answers_the_mode_name() {
    let rig = Rig::new();
    for (mode, code) in [
        (PowerMode::Turbo, 5),
        (PowerMode::Performance, 4),
        (PowerMode::Balanced, 1),
        (PowerMode::Quiet, 0),
        (PowerMode::Eco, 6),
    ] {
        assert_eq!(rig.api.set_power(mode, None).unwrap(), mode);
        assert_eq!(rig.sim.snapshot().mode, code, "{mode}");
    }
    // The raw reply is the mode's name as text (Python returns `hw.set_mode(mode)`).
    let raw = rig
        .client
        .call("power", Args::new().with("mode", "turbo").with("hold_minutes", json!(null)))
        .unwrap();
    assert_eq!(raw, json!("turbo"));
}

#[test]
fn power_with_a_hold_keeps_the_pick_against_the_automatic_mode() {
    let rig = Rig::new();
    rig.api.set_power(PowerMode::Performance, Some(30.0)).unwrap();
    let st = rig.api.daemon_state().unwrap();
    // 30 minutes, less the few simulated seconds the loop has run since.
    assert!((1500..=1800).contains(&st.hold_left_s), "{}", st.hold_left_s);
    let status = rig.api.power_status().unwrap();
    assert_eq!(status.mode.to_string(), "performance");
    assert_eq!(status.auto_profile, Some(false));
    assert!(status.hold_left_s.unwrap() > 1500);
    // The deadline is stored as wall-clock seconds, so it survives a restart.
    assert!(rig.state.get_f64("profile_hold_until", 0.0) > 1_800_000_000.0);
}

#[test]
fn power_refuses_an_unknown_mode_and_a_missing_one() {
    let rig = Rig::new();
    let err = rig.client.call("power", Args::new().with("mode", "ludicrous")).unwrap_err();
    assert!(matches!(&err, ClientError::Daemon(t) if t.contains("power mode must be one of")), "{err:?}");
    assert!(rig.client.call("power", Args::new()).is_err());
}

#[test]
fn hold_and_release_move_the_deadline() {
    let rig = Rig::new();
    let held = rig.api.hold_power(20.0).unwrap();
    assert_eq!(held.minutes, Some(20.0));
    let left = rig.api.daemon_state().unwrap().hold_left_s;
    assert!((1000..=1200).contains(&left), "{left}");
    rig.api.release_hold().unwrap();
    assert_eq!(rig.api.daemon_state().unwrap().hold_left_s, 0);
    assert_eq!(rig.state.get_f64("profile_hold_until", -1.0), 0.0);
    // A hold of 0 and a hold of null both mean "release".
    rig.api.hold_power(5.0).unwrap();
    assert_eq!(rig.client.call("hold", Args::new().with("minutes", 0)).unwrap(), json!(0));
    assert_eq!(rig.api.daemon_state().unwrap().hold_left_s, 0);
}

#[test]
fn choosing_an_automatic_mode_drops_the_hold_like_python() {
    let rig = Rig::new();
    rig.api.hold_power(20.0).unwrap();
    rig.api.set_auto_mode(AutoMode::Native).unwrap();
    // Python: `if value != "off": self.set_hold(0)`.
    assert_eq!(rig.api.daemon_state().unwrap().hold_left_s, 0);
}

#[test]
fn power_status_reports_the_service_when_it_runs() {
    let rig = Rig::new();
    let status = rig.api.power_status().unwrap();
    assert_eq!(status.auto_profile, Some(false));
    assert_eq!(status.hold_left_s, Some(0));
    assert_eq!(status.mode.to_string(), "eco");
    assert_eq!(status.supported.len(), 5);
}

#[test]
fn the_journal_says_what_python_s_daemon_said() {
    let rig = Rig::new();
    rig.api.daemon_set("smart_fan", true).unwrap();
    rig.api.set_auto_mode(AutoMode::Native).unwrap();
    rig.api.set_power(PowerMode::Turbo, None).unwrap();
    rig.api.fan_max().unwrap();
    rig.api.set_cpu_limits(60.0, 70.0, false).unwrap();
    rig.api.use_fan_curve("truesense").unwrap();
    rig.api.use_smart_profile("truesense").unwrap();
    let journal = rig.journal.text();
    // The lines of `daemon.py`'s `self.log(...)` calls, which people read in `journalctl`.
    for line in [
        "setting smart_fan = true",
        "setting auto_mode = native",
        "setting smart_fan = false (fan max)",
        "setting power limits = 60/70 W",
        "setting fan_curve = truesense",
        "setting smart_profile = truesense",
    ] {
        // Python formats a bool as `True`/`False`; ours is lower case JSON. The words around it
        // are what an operator greps for.
        let python_like = line.replace("true", "True").replace("false", "False");
        assert!(
            journal.contains(line) || journal.contains(&python_like),
            "the journal lacks {line:?}:\n{journal}"
        );
    }
}
