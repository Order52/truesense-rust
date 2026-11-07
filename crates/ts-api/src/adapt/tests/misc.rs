//! `battery_full`, `touchpad` and `boot_look`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};
use ts_ipc::client::ClientError;
use ts_ipc::wire::Args;

use super::rig::Rig;
use crate::api::{ApiError, ChargeFullAction, EffectRequest};

fn daemon_error(e: &ApiError) -> String {
    match e {
        ApiError::Daemon(ClientError::Daemon(text)) => text.clone(),
        other => panic!("not a daemon refusal: {other:?}"),
    }
}

// ---- battery_full ----------------------------------------------------------------------------

#[test]
fn battery_full_get_has_python_s_four_keys() {
    let rig = Rig::new();
    let raw = rig.client.call("battery_full", Args::new().with("action", "get")).unwrap();
    // `{"active", "limit", "capacity", "ac"}`.
    assert_eq!(raw, json!({"active": false, "limit": false, "capacity": 80.0, "ac": true}));
    let status = rig.api.charge_full(ChargeFullAction::Status).unwrap();
    assert_eq!(
        (status.active, status.limit, status.capacity, status.ac),
        (false, Some(false), Some(80.0), Some(true))
    );
    assert_eq!(status.to_string(), "80 % limit off, battery 80 %, charging to full once: no");
}

#[test]
fn charge_to_full_once_starts_and_cancels_through_the_service() {
    let rig = Rig::new();
    rig.api.set_battery_limit(true).unwrap();
    let started = rig.api.charge_full(ChargeFullAction::Start).unwrap();
    assert!(started.active);
    // Python: the 80 % limit is lifted and the record is `{"since": now, "full_at": None}`.
    assert_eq!(rig.sim.snapshot().battery[0], 0, "the limit is off");
    let record = rig.api.daemon_state().unwrap().settings["charge_full"].clone();
    assert!(record["since"].as_f64().unwrap() > 1_800_000_000.0, "{record}");
    assert_eq!(record["full_at"], Value::Null);
    let status = rig.api.charge_full(ChargeFullAction::Status).unwrap();
    assert!(status.active, "{status:?}");
    assert!(!rig.api.battery_status().unwrap().limit_80);
    let cancelled = rig.api.charge_full(ChargeFullAction::Cancel).unwrap();
    assert!(!cancelled.active);
    assert_eq!(rig.sim.snapshot().battery[0], 1, "the limit is back");
    assert_eq!(rig.api.daemon_state().unwrap().settings["charge_full"], Value::Null);
    assert!(rig.api.battery_status().unwrap().limit_80);
}

#[test]
fn the_raw_replies_of_start_and_cancel_are_python_s() {
    let rig = Rig::new();
    rig.api.set_battery_limit(true).unwrap();
    let start = rig.client.call("battery_full", Args::new().with("action", "start")).unwrap();
    assert_eq!(start, json!({"active": true}));
    let cancel = rig.client.call("battery_full", Args::new().with("action", "cancel")).unwrap();
    assert_eq!(cancel, json!({"active": false}));
}

#[test]
fn starting_needs_the_charger_and_the_limit_on_in_python_s_words() {
    let rig = Rig::new();
    // The limit is off already on a fresh simulated laptop.
    let err = rig.api.charge_full(ChargeFullAction::Start).unwrap_err();
    assert_eq!(daemon_error(&err), "the 80 % limit is off already: the battery charges to 100 %");
    rig.api.set_battery_limit(true).unwrap();
    rig.sensors.set_ac(false);
    rig.ticks(3);
    let err = rig.api.charge_full(ChargeFullAction::Start).unwrap_err();
    assert_eq!(daemon_error(&err), "plug the charger in first");
    assert_eq!(rig.sim.snapshot().battery[0], 1, "nothing was written");
    let raw = rig.client.call("battery_full", Args::new().with("action", "frob")).unwrap_err();
    assert!(matches!(&raw, ClientError::Daemon(t) if t == "battery_full: get | start | cancel"), "{raw:?}");
}

#[test]
fn the_full_charge_ends_by_itself_when_the_charger_is_pulled() {
    let rig = Rig::new();
    rig.api.set_battery_limit(true).unwrap();
    rig.api.charge_full(ChargeFullAction::Start).unwrap();
    rig.sensors.set_ac(false);
    rig.wait_for(|| rig.sim.snapshot().battery[0] == 1, "the limit to come back on unplugging");
    assert!(!rig.api.charge_full(ChargeFullAction::Status).unwrap().active);
}

// ---- touchpad --------------------------------------------------------------------------------

#[test]
fn the_touchpad_switch_is_the_same_file_for_the_service_and_the_client() {
    let rig = Rig::new();
    // The client's own reading, from the kernel's file.
    assert_eq!(rig.api.touchpad(), Some(true));
    assert_eq!(
        rig.client.call("touchpad", Args::new()).unwrap(),
        json!(true),
        "status is the default action"
    );
    assert!(!rig.api.set_touchpad(false).unwrap());
    assert_eq!(rig.api.touchpad(), Some(false));
    assert_eq!(
        std::fs::read_to_string(rig.root.join("sys/class/input/event13/device/inhibited")).unwrap().trim(),
        "1"
    );
    assert_eq!(rig.client.call("touchpad", Args::new().with("action", "status")).unwrap(), json!(false));
    assert!(rig.api.set_touchpad(true).unwrap());
    assert_eq!(rig.client.call("touchpad", Args::new().with("action", "toggle")).unwrap(), json!(false));
    assert_eq!(rig.client.call("touchpad", Args::new().with("action", "toggle")).unwrap(), json!(true));
    assert_eq!(rig.client.call("touchpad", Args::new().with("action", "off")).unwrap(), json!(false));
}

#[test]
fn an_unknown_touchpad_action_is_refused() {
    let rig = Rig::new();
    // Python raises a bare KeyError here; the words differ, the refusal does not.
    let err = rig.client.call("touchpad", Args::new().with("action", "frob")).unwrap_err();
    assert!(matches!(err, ClientError::Daemon(_)), "{err:?}");
    assert_eq!(rig.api.touchpad(), Some(true));
}

// ---- boot_look -------------------------------------------------------------------------------

#[test]
fn there_is_no_boot_look_at_first() {
    let rig = Rig::new();
    assert_eq!(rig.client.call("boot_look", Args::new()).unwrap(), Value::Null);
    assert!(rig.api.boot_look().unwrap().is_none());
}

#[test]
fn a_look_set_as_the_boot_look_is_kept_by_the_service_and_read_back() {
    let rig = Rig::new();
    // What the keyboard shows, as the client remembers it.
    rig.api
        .set_keyboard_effect(&EffectRequest {
            name: "wave".to_owned(),
            color: "ff8800".to_owned(),
            speed: 4,
            brightness: Some(30),
            direction: 1,
        })
        .unwrap();
    let look = rig.api.set_boot_look("current").unwrap();
    assert!(!look.is_empty());
    let back = rig.api.boot_look().unwrap().unwrap();
    assert_eq!(back, look);
    // The daemon (root) keeps its own self-contained copy, as `looks.write_boot` does.
    let file = std::fs::read_to_string(rig.dir.0.join("boot-look.json")).unwrap();
    let on_disk: Value = serde_json::from_str(&file).unwrap();
    assert_eq!(on_disk, serde_json::to_value(&look).unwrap());
    // Python's `keyboard.set_effect` remembers `{"mode": "effect", "name", "color", "speed",
    // "brightness", "direction"}`, and a look is that dictionary next to the rear light's.
    assert_eq!(
        on_disk["keyboard"],
        json!({"mode": "effect", "name": "wave", "color": "ff8800", "speed": 4, "brightness": 30, "direction": 1}),
        "{on_disk}"
    );
    assert_eq!(on_disk["rear"], Value::Null, "nothing was ever set on the light bar: {on_disk}");
    rig.api.clear_boot_look().unwrap();
    assert!(rig.api.boot_look().unwrap().is_none());
    assert!(!rig.dir.0.join("boot-look.json").exists());
}

#[test]
fn a_boot_look_given_as_python_writes_it_is_accepted_and_returned_as_sent() {
    let rig = Rig::new();
    // `looks.capture()`: `{"keyboard": <keyboard.last()>, "rear": <userstate rear_state>}`.
    let look = json!({
        "keyboard": {"name": "wave", "color": "ff8800", "speed": 4, "brightness": 30, "direction": 1},
        "rear": {"mode": "breathing", "color": "3723c7", "speed": 3, "brightness": 100, "direction": 1, "zones": null},
    });
    let reply =
        rig.client.call("boot_look", Args::new().with("action", "set").with("look", look.clone())).unwrap();
    assert_eq!(reply, look, "Python returns `looks.write_boot(look)`, which is the look");
    let got = rig.client.call("boot_look", Args::new().with("action", "get")).unwrap();
    assert_eq!(got["keyboard"]["name"], json!("wave"));
    assert_eq!(got["rear"]["mode"], json!("breathing"));
    assert!(rig.api.boot_look().unwrap().is_some());
}

#[test]
fn a_boot_look_without_any_lights_is_refused_in_python_s_words() {
    let rig = Rig::new();
    let text = "a look needs keyboard and/or rear settings";
    for look in [json!({}), json!({"keyboard": null, "rear": null}), json!("wave"), json!(null), json!([1])] {
        let err = rig
            .client
            .call("boot_look", Args::new().with("action", "set").with("look", look.clone()))
            .unwrap_err();
        assert!(matches!(&err, ClientError::Daemon(t) if t == text), "{look}: {err:?}");
    }
    assert!(rig.client.call("boot_look", Args::new().with("action", "set")).is_err());
    let err = rig.client.call("boot_look", Args::new().with("action", "frob")).unwrap_err();
    assert!(matches!(&err, ClientError::Daemon(t) if t == "boot_look: get | set | clear"), "{err:?}");
    let err = rig.api.set_boot_look("no-such-look").unwrap_err();
    assert!(err.to_string().contains("no-such-look"), "{err}");
    assert!(rig.api.boot_look().unwrap().is_none());
}

#[test]
fn the_built_in_native_look_can_be_the_boot_look() {
    let rig = Rig::new();
    let look = rig.api.set_boot_look("native").unwrap();
    assert!(look.keyboard.is_some() && look.rear.is_some());
    assert_eq!(rig.api.boot_look().unwrap().unwrap(), look);
}
