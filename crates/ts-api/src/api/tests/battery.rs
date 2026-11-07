use serde_json::json;
use ts_platform::hw::UsbLevel;

use super::Fx;
use crate::api::testing::Reply;
use crate::api::{ChargeFullAction, Switch};

#[test]
fn every_switch_is_written_and_answers_what_the_firmware_reads_back() {
    let fx = Fx::offline();
    assert!(fx.api.set_battery_limit(true).unwrap());
    assert_eq!(fx.rig.sim.snapshot().battery[0], 1);
    assert!(!fx.api.set_battery_limit(false).unwrap());
    assert!(fx.api.set_calibration(true).unwrap());
    assert!(fx.api.calibration().unwrap());
    assert!(!fx.api.set_overdrive(false).unwrap());
    assert_eq!(fx.api.overdrive().unwrap(), Some(false));
    assert!(fx.api.set_boot_sound(true).unwrap());
    assert_eq!(fx.rig.sim.snapshot().boot, 1);
    assert!(fx.api.boot_sound().unwrap());
}

#[test]
fn the_battery_status_has_the_names_the_old_json_had() {
    let fx = Fx::offline();
    fx.api.set_battery_limit(true).unwrap();
    let st = fx.api.battery_status().unwrap();
    assert_eq!(
        serde_json::to_value(st).unwrap(),
        json!({"limit_80": true, "calibration": false, "usb_charge_stop": 30})
    );
}

#[test]
fn the_usb_level_is_zero_ten_twenty_or_thirty() {
    let fx = Fx::offline();
    for level in [0, 10, 20, 30] {
        assert_eq!(fx.api.set_usb_charge(level).unwrap(), level);
        assert_eq!(fx.api.usb_charge().unwrap(), level);
    }
    let before = fx.rig.sim.snapshot().usb;
    for bad in [-10, 5, 15, 40, 100] {
        let e = fx.api.set_usb_charge(bad).unwrap_err();
        assert_eq!(e.to_string(), "usb-charge level: 0 (off), 10, 20 or 30", "{bad}");
    }
    assert_eq!(fx.rig.sim.snapshot().usb, before);
    assert_eq!(UsbLevel::from_percent(20).unwrap().percent(), 20);
}

#[test]
fn the_backlight_timeout_is_the_controllers_flag() {
    let fx = Fx::offline();
    let before = fx.api.keyboard_timeout().unwrap();
    assert!(before.is_some());
    assert!(!fx.api.set_keyboard_timeout(false).unwrap());
    assert_eq!(fx.api.keyboard_timeout().unwrap(), Some(false));
    assert!(!fx.rig.hid.timeout());
    assert!(fx.api.set_keyboard_timeout(true).unwrap());
    assert!(fx.rig.hid.timeout());
}

#[test]
fn switches_follow_the_on_off_toggle_status_words() {
    assert_eq!("on".parse::<Switch>().unwrap().target(false), Some(true));
    assert_eq!("off".parse::<Switch>().unwrap().target(true), Some(false));
    assert_eq!("toggle".parse::<Switch>().unwrap().target(true), Some(false));
    assert_eq!("toggle".parse::<Switch>().unwrap().target(false), Some(true));
    assert_eq!("status".parse::<Switch>().unwrap().target(true), None);
    assert_eq!("".parse::<Switch>().unwrap().target(true), None);
    assert!("maybe".parse::<Switch>().is_err());
    assert_eq!(crate::api::on_off(true), "on");
    assert_eq!(crate::api::on_off_opt(None), "unknown");
}

#[test]
fn charge_to_full_once_asks_the_service_and_words_the_answer() {
    let fx = Fx::new();
    fx.rig
        .daemon
        .answer_ok("battery_full", json!({"active": true, "limit": false, "capacity": 71.0, "ac": true}));
    let st = fx.api.charge_full(ChargeFullAction::Status).unwrap();
    assert_eq!(fx.rig.daemon.last_args("battery_full").unwrap().get("action"), Some(&json!("get")));
    assert_eq!(
        st.message(ChargeFullAction::Status),
        "80 % limit off, battery 71 %, charging to full once: yes"
    );
    fx.api.charge_full(ChargeFullAction::Start).unwrap();
    assert_eq!(fx.rig.daemon.last_args("battery_full").unwrap().get("action"), Some(&json!("start")));
    assert!(
        st.message(ChargeFullAction::Start)
            .starts_with("charging to 100 % once; the 80 % limit comes back when you unplug")
    );
    fx.api.charge_full(ChargeFullAction::Cancel).unwrap();
    assert_eq!(st.message(ChargeFullAction::Cancel), "80 % limit back on");
    fx.rig.daemon.answer("battery_full", Reply::Error("plug the charger in first".into()));
    assert_eq!(
        fx.api.charge_full(ChargeFullAction::Start).unwrap_err().to_string(),
        "plug the charger in first"
    );
    assert!("sideways".parse::<ChargeFullAction>().is_err());
    assert_eq!("start".parse::<ChargeFullAction>().unwrap(), ChargeFullAction::Start);
}

#[test]
fn the_touchpad_goes_through_the_service_and_root_may_do_it_directly() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok("touchpad", json!(false));
    assert!(!fx.api.set_touchpad(false).unwrap());
    assert_eq!(fx.rig.daemon.last_args("touchpad").unwrap().get("action"), Some(&json!("off")));
    fx.rig.daemon.set_available(false);
    // A user without the service cannot write the kernel's switch.
    assert!(fx.api.set_touchpad(true).is_err());
    assert_eq!(fx.api.touchpad(), None, "no touchpad in the simulated machine");
}

#[test]
fn root_without_a_service_writes_the_switch_itself_and_fails_without_a_touchpad() {
    let fx = Fx::build(|mut s| {
        s.euid = 0;
        s
    });
    fx.rig.daemon.answer("touchpad", Reply::Error("no".into()));
    let e = fx.api.set_touchpad(true).unwrap_err();
    assert!(!e.to_string().is_empty());
    assert_eq!(fx.rig.daemon.commands(), ["touchpad"]);
}
