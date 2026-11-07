use serde_json::json;
use ts_system::doctor::SoundState;

use super::{Fx, daemon_state};
use crate::api::Api;
use crate::api::testing::Reply;

#[test]
fn the_health_check_runs_over_the_adapters_and_changes_nothing() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok(
        "state",
        daemon_state(json!({"smart_fan": true, "auto_profile": true}), json!({"hw_ok": true}), 0),
    );
    let before = fx.rig.sim.snapshot();
    let report = fx.api.run_doctor();
    assert_eq!(report.checks.len(), Api::doctor_checks().len());
    assert_eq!(report.checks[0].check, Api::doctor_checks()[0]);
    assert_eq!(report.version, ts_foundation::buildinfo::VERSION);
    assert_eq!(report.kernel, "test");
    assert_eq!(report.exit_code(), i32::from(report.failures > 0));
    assert_eq!(fx.rig.sim.snapshot(), before, "the health check is read-only");
    for (cmd, args) in fx.rig.daemon.calls() {
        assert!(cmd == "state" || args.get("action") == Some(&json!("get")), "{cmd}: only reads");
    }
}

#[test]
fn the_daemon_adapter_decodes_state_and_probes_commands_with_get() {
    let fx = Fx::new();
    fx.rig
        .daemon
        .answer_ok("state", daemon_state(json!({"smart_fan": true}), json!({"activity": "idle"}), 5));
    let adapters = fx.api.doctor_adapters();
    let state = adapters.daemon.state().unwrap();
    assert!(state.settings.smart_fan);
    assert_eq!(state.live.activity.as_deref(), Some("idle"));
    adapters.daemon.read("fan_curve").unwrap_err();
    fx.rig.daemon.answer_ok("fan_curve", json!({}));
    adapters.daemon.read("fan_curve").unwrap();
    assert_eq!(fx.rig.daemon.last_args("fan_curve").unwrap().get("action"), Some(&json!("get")));
    let old = adapters.daemon.read("smart_profile").unwrap_err();
    assert!(old.is_unknown_command(), "an old service is recognised");
    fx.rig.daemon.answer_ok("state", json!("junk"));
    assert!(adapters.daemon.state().is_err());
    fx.rig.daemon.answer("state", Reply::Error("boom".into()));
    assert_eq!(adapters.daemon.state().unwrap_err().to_string(), "boom");
}

#[test]
fn the_audio_adapter_maps_the_card_verdict() {
    let fx = Fx::offline();
    let audio = fx.api.doctor_adapters().audio;
    assert!(!audio.has_rnnoise() && !audio.has_dts());
    assert_eq!(audio.sound_state().unwrap(), SoundState::Unknown);
    let jack = audio.jack_status().unwrap();
    assert!(!jack.headphones && !jack.headset_mic);
    assert!(audio.plain_headsets().is_err(), "AcerUltraX is not installed");
}

#[test]
fn the_probe_adapter_counts_ceilings_and_finds_the_node() {
    let fx = Fx::build(|mut s| {
        s.fn_key_node = std::sync::Arc::new(|| Some(std::path::PathBuf::from("/dev/hidraw3")));
        s
    });
    let probes = fx.api.doctor_adapters().probes;
    assert_eq!(probes.fn_key_node(), Some(std::path::PathBuf::from("/dev/hidraw3")));
    assert_eq!(probes.low_frequency_ceilings(), 0);
    let cpu = fx.dir.0.join("sys/devices/system/cpu/cpu0/cpufreq");
    std::fs::create_dir_all(&cpu).unwrap();
    std::fs::write(cpu.join("scaling_max_freq"), "800000\n").unwrap();
    std::fs::write(cpu.join("cpuinfo_max_freq"), "4900000\n").unwrap();
    assert_eq!(probes.low_frequency_ceilings(), 1);
}

#[test]
fn the_probe_writes_nothing_and_says_so() {
    let fx = Fx::offline();
    let before = fx.rig.sim.snapshot();
    let report = fx.api.probe();
    assert_eq!(report.writes, 0);
    assert_eq!(fx.rig.sim.snapshot(), before);
    assert_eq!(report.truesense, ts_foundation::buildinfo::VERSION);
    assert_eq!(fx.api.module_version().unwrap(), fx.api.services().hw.version().unwrap());
}

#[test]
fn the_service_check_is_read_only_and_the_system_repair_needs_root() {
    let fx = Fx::new();
    assert!(fx.api.check_services().unwrap().is_empty());
    assert!(
        fx.api.repair_system_services().unwrap_err().to_string().starts_with("the system part needs root")
    );
    let root = Fx::build(|mut s| {
        s.euid = 0;
        s
    });
    assert_eq!(root.api.repair_system_services().unwrap()[0].what, "daemon restarted");
    let none = Fx::build(|s| s.without_desktop());
    assert!(none.api.check_services().is_err());
}

#[test]
fn the_configuration_is_read_fresh_and_a_typo_is_reported_not_fatal() {
    let fx = Fx::new();
    assert_eq!(fx.api.validate_config(), Vec::<String>::new());
    assert_eq!(fx.api.config_paths().len(), 2);
    assert_eq!(fx.api.config_values().unwrap()["keys"]["tap"], json!("power-next"));
    assert!(fx.api.config_toml().unwrap().contains("[keys]"));
    std::fs::create_dir_all(fx.dir.0.join("home")).unwrap();
    std::fs::write(fx.dir.0.join("home/config.toml"), "[cpu]\npl_normal_w = 500\n").unwrap();
    assert_eq!(fx.api.validate_config(), ["[cpu] power limits must be 25..120 W"]);
    std::fs::write(fx.dir.0.join("home/config.toml"), "[cpu\n").unwrap();
    let problems = fx.api.validate_config();
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("config.toml"), "{problems:?}");
    assert!(fx.api.config_values().is_err());
    // Operations that do not use the broken setting keep working on the defaults.
    assert_eq!(fx.api.startup().keyboard, "last");
    assert!(fx.api.load_config().is_err());
}

#[test]
fn the_user_file_is_merged_not_replaced() {
    let fx = Fx::new();
    std::fs::create_dir_all(fx.dir.0.join("home")).unwrap();
    std::fs::write(
        fx.dir.0.join("home/config.toml"),
        "[startup]\nkeyboard = \"off\"\n\n[custom]\nkept = 1\n",
    )
    .unwrap();
    fx.api.set_auto_update_check(true).unwrap();
    let text = fx.user_config();
    assert!(
        text.contains("keyboard = \"off\"")
            && text.contains("kept = 1")
            && text.contains("auto_check = true"),
        "{text}"
    );
    assert_eq!(fx.api.startup().keyboard, "off");
}

#[test]
fn a_look_is_saved_applied_and_set_for_boot() {
    let fx = Fx::new();
    fx.api.fill_keyboard("ff0000", Some(20)).unwrap();
    let look = fx.api.save_look("red").unwrap();
    assert!(look.keyboard.is_some());
    assert!(fx.api.looks().contains(&"red".to_owned()) && fx.api.looks()[0] == "native");
    fx.api.fill_keyboard("0000ff", Some(20)).unwrap();
    assert!(!fx.api.apply_look("red").unwrap().is_empty());
    assert_eq!(fx.api.keyboard_cells()[0], "ff0000");
    assert_eq!(fx.api.copy_look("red", None).unwrap(), "red-copy");
    assert!(Api::look_is_builtin("native") && !Api::look_is_builtin("red"));
    fx.api.delete_look("red-copy").unwrap();
    assert!(fx.api.apply_look("nosuch").is_err());
    assert!(fx.api.save_look("a/b").is_err());
    assert!(fx.api.save_look_value("x", &look).is_ok());

    fx.rig.daemon.answer_ok("boot_look", json!(null));
    assert!(fx.api.set_boot_look("red").is_ok());
    let sent = fx.rig.daemon.last_args("boot_look").unwrap();
    assert_eq!(sent.get("action"), Some(&json!("set")));
    assert!(sent.get("look").unwrap()["keyboard"]["cells"].is_array());
    assert!(fx.api.set_boot_look("current").is_ok());
    assert!(fx.api.boot_look().unwrap().is_none());
    fx.api.clear_boot_look().unwrap();
    assert_eq!(fx.rig.daemon.last_args("boot_look").unwrap().get("action"), Some(&json!("clear")));
    let before = fx.rig.daemon.calls().len();
    assert!(fx.api.set_boot_look("nosuch").is_err());
    assert_eq!(fx.rig.daemon.calls().len(), before, "nothing is sent for a look that does not exist");
    fx.rig.daemon.answer_ok("boot_look", serde_json::to_value(&look).unwrap());
    assert!(fx.api.boot_look().unwrap().is_some());
}
