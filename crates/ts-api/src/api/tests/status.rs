use serde_json::json;

use super::{Fx, daemon_state};
use crate::api::describe_sensors;

#[test]
fn the_status_reads_every_part_in_the_old_order() {
    let fx = Fx::offline();
    let st = fx.api.status();
    let keys: Vec<&str> = st.entries().iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        [
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
            "voice"
        ]
    );
    assert_eq!(st.get("power_mode"), Some(&json!("eco")));
    assert_eq!(st.get("sensors").unwrap()["cpu_temp"], json!(50));
    assert_eq!(st.get("fans").unwrap()["mode"], json!("auto"));
    assert_eq!(st.get("usb_charge_stop"), Some(&json!(30)));
    assert_eq!(st.get("gpu_mode"), Some(&json!("nvidia")));
    assert_eq!(st.get("touchpad"), Some(&json!(null)));
}

#[test]
fn a_part_that_fails_is_an_error_entry_and_the_rest_are_still_there() {
    let fx = Fx::offline();
    let st = fx.api.status();
    let daemon = st.get("daemon").unwrap().as_str().unwrap();
    assert!(daemon.starts_with("ERROR: the TrueSense daemon is not running"), "{daemon}");
    assert!(st.get("audio_output").unwrap().as_str().unwrap().starts_with("ERROR: "));
    assert_eq!(st.get("power_mode"), Some(&json!("eco")));
    assert!(st.get("rear").unwrap().is_object());
}

#[test]
fn the_status_prints_as_one_object_in_reading_order() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok("state", daemon_state(json!({"smart_fan": true}), json!({"tcpu": 61}), 0));
    let st = fx.api.status();
    let text = serde_json::to_string(&st).unwrap();
    let positions: Vec<usize> = ["power_mode", "sensors", "fans", "daemon", "rear", "voice"]
        .iter()
        .map(|k| text.find(&format!("\"{k}\":")).unwrap())
        .collect();
    assert!(positions.windows(2).all(|w| w[0] < w[1]), "{text}");
    assert_eq!(st.get("daemon").unwrap()["live"]["tcpu"], json!(61));
    assert!(st.to_string().contains("\n  \"power_mode\": \"eco\""));
}

#[test]
fn the_full_status_adds_what_the_window_shows() {
    let fx = Fx::offline();
    let st = fx.api.status_full();
    assert_eq!(st.get("ac"), Some(&json!(true)), "no power supply files means mains power");
    assert_eq!(st.get("battery_pct"), Some(&json!(null)));
    assert_eq!(st.get("kb_brightness"), Some(&json!(25)));
    assert_eq!(st.get("kb_look"), Some(&json!({})));
}

#[test]
fn sensors_read_in_one_call_and_have_a_line() {
    let fx = Fx::offline();
    fx.rig.sim.set_sensors([61, 45, 55], [2400, 2100]);
    let s = fx.api.sensors().unwrap();
    assert_eq!(describe_sensors(&s), "CPU 61 C   GPU 55 C   board 45 C   fans 2400 / 2100 rpm");
    assert_eq!(serde_json::to_value(s).unwrap()["cpu_fan_rpm"], json!(2400));
}

#[test]
fn the_live_reading_uses_the_service_when_it_runs() {
    let fx = Fx::new();
    fx.rig.sim.set_sensors([1, 2, 3], [4, 5]);
    fx.rig.daemon.answer_ok(
        "state",
        daemon_state(
            json!({"smart_fan": true}),
            json!({"tcpu": 70, "tgpu": 66, "chassis": 40, "rpm1": 3000, "rpm2": 2800, "power_mode": "turbo", "ac": false, "cpu": 55.0, "gpu": 90.0, "gpuw": 80.0}),
            0,
        ),
    );
    let live = fx.api.live();
    let s = live.sensors.unwrap();
    assert_eq!((s.cpu_temp, s.gpu_temp, s.sys_temp, s.cpu_fan_rpm, s.gpu_fan_rpm), (70, 66, 40, 3000, 2800));
    assert_eq!(live.power_mode.as_deref(), Some("turbo"));
    assert_eq!(live.ac, Some(false));
    assert_eq!(live.load.cpu_pct, Some(55.0));
    assert_eq!(live.power.gpu_w, Some(80.0));
    assert_eq!(live.t, 1_700_000_000);
    assert!(live.daemon.is_some() && live.fans.is_some());
    let json = serde_json::to_value(&live).unwrap();
    assert_eq!(json["load"]["gpu_pct"], json!(90.0));
    assert_eq!(json["sensors"]["gpu_temp"], json!(66));
}

#[test]
fn the_live_reading_falls_back_to_the_hardware_without_the_service() {
    let fx = Fx::offline();
    fx.rig.sim.set_sensors([61, 45, 55], [2400, 2100]);
    let live = fx.api.live();
    assert_eq!(live.sensors.unwrap().cpu_temp, 61);
    assert_eq!(live.power_mode.as_deref(), Some("eco"));
    assert_eq!(live.ac, Some(true));
    assert_eq!(live.daemon, None);
    assert_eq!(live.load.cpu_pct, None);
    // A daemon that does not know the readings yet (no tcpu) is not trusted either.
    fx.rig.daemon.set_available(true);
    fx.rig.daemon.answer_ok("state", daemon_state(json!({}), json!({"power_mode": "?"}), 0));
    let live = fx.api.live();
    assert_eq!(live.sensors.unwrap().cpu_temp, 61);
    assert_eq!(live.power_mode.as_deref(), Some("eco"), "a question mark is not a mode");
}

#[test]
fn cpu_power_is_the_difference_of_two_energy_readings() {
    use std::sync::atomic::Ordering;
    let fx = Fx::offline();
    let dir = fx.dir.0.join("sys/class/powercap/intel-rapl:0");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("energy_uj"), "1000000\n").unwrap();
    assert_eq!(fx.api.live().power.cpu_w, None, "the first reading has nothing to compare with");
    fx.rig.clock.fetch_add(2, Ordering::SeqCst);
    std::fs::write(dir.join("energy_uj"), "31000000\n").unwrap();
    assert_eq!(fx.api.live().power.cpu_w, Some(15.0));
    // The counter wrapped: no negative power.
    fx.rig.clock.fetch_add(1, Ordering::SeqCst);
    std::fs::write(dir.join("energy_uj"), "5\n").unwrap();
    assert_eq!(fx.api.live().power.cpu_w, None);
    // The clock did not move: no division by zero.
    std::fs::write(dir.join("energy_uj"), "500\n").unwrap();
    assert_eq!(fx.api.live().power.cpu_w, None);
    // Junk in the file is not a reading.
    std::fs::write(dir.join("energy_uj"), "junk").unwrap();
    assert_eq!(fx.api.live().power.cpu_w, None);
}

#[test]
fn the_battery_is_the_first_one_that_can_be_read() {
    let fx = Fx::offline();
    assert_eq!(fx.api.supply(), None);
    let bat = fx.dir.0.join("sys/class/power_supply/BAT0");
    std::fs::create_dir_all(&bat).unwrap();
    std::fs::write(bat.join("capacity"), "83\n").unwrap();
    std::fs::write(bat.join("status"), "Discharging\n").unwrap();
    let supply = fx.api.supply().unwrap();
    assert_eq!((supply.percent, supply.status.as_str()), (83, "Discharging"));
    assert_eq!(fx.api.live().battery_pct, Some(83));
}

#[test]
fn the_summary_is_one_line() {
    let fx = Fx::offline();
    let s = fx.api.summary();
    assert!(!s.daemon_running);
    assert_eq!(s.fan_mode.as_deref(), Some("auto"));
    assert_eq!(s.to_string(), "eco   CPU 50 C   GPU 48 C   fans 0 / 0 rpm   (service not running)");
    fx.rig.daemon.set_available(true);
    fx.rig
        .daemon
        .answer_ok("state", daemon_state(json!({"smart_fan": true, "auto_mode": "smart"}), json!({}), 0));
    let s = fx.api.summary();
    assert!(s.daemon_running);
    assert_eq!(s.smart_fan, Some(true));
    assert_eq!(s.auto_mode.as_deref(), Some("smart"));
    assert!(!s.to_string().contains("not running"));
}

#[test]
fn the_daemon_state_keeps_every_key_the_daemon_sent() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok("state", json!({"live": {"future_key": 1}, "settings": {"smart_fan": false}, "hold_left_s": 12, "extra": true}));
    let st = fx.api.daemon_state().unwrap();
    assert_eq!(st.live["future_key"], json!(1));
    assert_eq!(st.hold_left_s, 12);
    assert_eq!(st.setting_bool("smart_fan"), Some(false));
    assert_eq!(st.setting_bool("nope"), None);
    assert!(fx.api.daemon_available());
    fx.rig.daemon.answer_ok("ping", json!("pong"));
    assert!(fx.api.daemon_responds());
    fx.rig.daemon.answer_ok("ping", json!("pang"));
    assert!(!fx.api.daemon_responds());
    fx.rig.daemon.set_available(false);
    assert!(fx.api.daemon_state().unwrap_err().daemon_missing());
}
