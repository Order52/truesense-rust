//! What a window or a status line does: many operations in a row against the one service.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::Value;
use ts_platform::firmware::SimFan;

use super::rig::{Rig, locked, put};
use crate::api::{AutoMode, CurveEdit, UndervoltRequest};

const CONFIRMED: UndervoltRequest = UndervoltRequest { confirm: true, force: false };

#[test]
fn reset_all_puts_every_setting_of_the_service_back_and_every_step_works() {
    let rig = Rig::new();
    let api = &rig.api;
    // Move everything the reset is supposed to move.
    api.save_fan_curve("mine", &CurveEdit { from: Some("truesense".to_owned()), ..CurveEdit::default() })
        .unwrap();
    api.use_fan_curve("mine").unwrap();
    api.use_smart_profile("truesense").unwrap();
    api.fan_max().unwrap();
    api.set_auto_mode(AutoMode::Smart).unwrap();
    api.set_cpu_limits(60.0, 70.0, false).unwrap();
    api.apply_cpu_undervolt_values("x", -30, -20, CONFIRMED).unwrap();
    api.set_cpu_undervolt_boot_values("x", -30, -20, CONFIRMED).unwrap();
    api.apply_gpu_undervolt_values("x", 80, 2300, CONFIRMED).unwrap();
    api.set_gpu_undervolt_boot_values("x", 80, 2300, CONFIRMED).unwrap();

    let steps = api.reset_all();
    let failed: Vec<_> = steps.iter().filter(|s| !s.ok).collect();
    assert!(failed.is_empty(), "{failed:?}");
    assert_eq!(steps.len(), 12);

    let st = api.daemon_state().unwrap();
    assert_eq!(st.setting_str("fan_curve"), Some("native"));
    assert_eq!(st.setting_str("smart_profile"), Some("native"));
    assert_eq!(st.setting_str("auto_mode"), Some("native"));
    assert_eq!(st.settings["pl_user"], Value::Null);
    for key in ["uv_applied", "uv_boot", "gpu_uv_applied", "gpu_uv_boot"] {
        assert_eq!(st.settings[key], Value::Null, "{key}");
    }
    assert_eq!(rig.sim.snapshot().fan, SimFan::Auto);
    assert_eq!(*locked(&rig.uv.offsets), (0, 0));
    assert_eq!(*locked(&rig.gpu_uv.0), (0, 0));
    // The light bar is the stock breathing one the window shows after a reset.
    let rear = api.rear().unwrap();
    assert_eq!(
        (rear.mode.to_string(), rear.color.to_string(), rear.brightness),
        ("breathing".to_owned(), "3723c7".to_owned(), 100)
    );
}

#[test]
fn reset_all_without_a_service_reports_the_steps_that_need_it_instead_of_failing_the_rest() {
    let rig = Rig::new();
    rig.shutdown().unwrap();
    let steps = rig.api.reset_all();
    assert_eq!(steps.len(), 12);
    let failed: Vec<&str> = steps.iter().filter(|s| !s.ok).map(|s| s.step.as_str()).collect();
    assert!(failed.contains(&"fan curve: native") && failed.contains(&"mode control: native"), "{failed:?}");
    // What needs no service still happened: the fans went to the firmware, the light bar is stock.
    assert!(steps.iter().any(|s| s.step == "fans: firmware auto" && s.ok), "{steps:?}");
    assert!(steps.iter().any(|s| s.step == "light bar: stock" && s.ok), "{steps:?}");
    assert!(
        steps.iter().filter(|s| !s.ok).all(|s| s.error.as_deref().is_some_and(|e| e.contains("not running"))),
        "{steps:?}"
    );
}

#[test]
fn the_status_line_and_the_window_read_the_service_s_numbers() {
    let rig = Rig::new();
    rig.api.set_auto_mode(AutoMode::Smart).unwrap();
    rig.api.fan_smart().unwrap();
    rig.heat(58, 50, 49);
    let summary = rig.api.summary();
    assert!(summary.daemon_running);
    assert_eq!(summary.smart_fan, Some(true));
    assert_eq!(summary.auto_mode.as_deref(), Some("smart"));
    assert_eq!((summary.cpu_temp, summary.gpu_temp), (Some(58), Some(49)));
    assert_eq!((summary.cpu_fan_rpm, summary.gpu_fan_rpm), (Some(2400), Some(2200)));
    assert_eq!(summary.ac, Some(true));
    let live = rig.api.live();
    assert_eq!(live.daemon.as_ref().unwrap().setting_str("auto_mode"), Some("smart"));
    assert!(live.fans.is_some());
    // The full status has the service's whole answer under `daemon`, untouched.
    let status = rig.api.status();
    let daemon = status.get("daemon").unwrap();
    assert!(daemon["live"]["tcpu"].is_i64() && daemon["settings"].is_object(), "{daemon}");
    assert!(status.get("power_mode").is_some_and(Value::is_string), "{status:?}");
    let json = serde_json::to_value(&status).unwrap();
    assert!(json.get("daemon").is_some() && json.get("fans").is_some() && json.get("sensors").is_some());
}

#[test]
fn the_status_without_a_service_marks_the_daemon_entry_and_keeps_the_rest() {
    let rig = Rig::new();
    rig.shutdown().unwrap();
    let status = rig.api.status();
    let daemon = status.get("daemon").unwrap().to_string();
    assert!(daemon.contains("not running"), "{daemon}");
    assert!(status.get("fans").is_some());
    let summary = rig.api.summary();
    assert!(!summary.daemon_running);
    assert_eq!(summary.smart_fan, None);
    // The fans still read from the firmware and the temperatures from the sensors.
    assert!(summary.cpu_temp.is_some());
}

#[test]
fn the_health_check_finds_the_keyboard_s_fn_key_interface_through_the_services() {
    let rig = Rig::new();
    let probes = rig.api.doctor_adapters().probes;
    assert_eq!(probes.fn_key_node(), None);
    let device = rig.root.join("sys/class/hidraw/hidraw4/device");
    put(&device.join("uevent"), "HID_ID=0003:000004F2:00000117\n");
    std::fs::write(device.join("report_descriptor"), [0x05u8, 0x88]).unwrap();
    // `/dev` is below the machine's root like everything else.
    assert_eq!(probes.fn_key_node(), Some(rig.root.join("dev/hidraw4")));
    assert_eq!(rig.api.services().fn_key_node.as_ref()(), Some(rig.root.join("dev/hidraw4")));
}

#[test]
fn the_model_probe_and_the_module_version_read_the_simulated_firmware() {
    let rig = Rig::new();
    let version = rig.api.module_version().unwrap();
    assert!(!version.is_empty());
    let report = rig.api.probe();
    // A temporary tree has no DMI identity: the probe says what it could not see and writes nothing.
    let json = serde_json::to_value(&report).unwrap();
    assert!(json.is_object(), "{json}");
    assert_eq!(rig.sim.snapshot().mode, 6);
}
