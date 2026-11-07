use serde_json::json;
use ts_platform::hw::GpuMode;

use super::Fx;
use crate::api::testing::Reply;
use crate::api::{ApiError, Refusal, UndervoltPlane, UndervoltRequest};

const CONFIRM: UndervoltRequest = UndervoltRequest { confirm: true, force: false };
const FORCE: UndervoltRequest = UndervoltRequest { confirm: true, force: true };

fn refusal(e: ApiError) -> Refusal {
    e.refusal().cloned().unwrap_or_else(|| panic!("expected a refusal, got: {e}"))
}

#[test]
fn the_hardware_limits_come_from_the_tree_below_the_root() {
    let fx = Fx::offline();
    let info = fx.api.cpu_info();
    assert_eq!(info.pl1_w, None, "the simulated machine has no RAPL zone");
    assert_eq!(serde_json::to_value(&info).unwrap()["pl1_w"], json!(null));
}

#[test]
fn the_services_limits_decode_with_what_you_set_and_the_automatic_ones() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok(
        "cpu_info",
        json!({"pl1_w": 55.0, "pl2_w": 90.0, "rapl_enabled": "1", "msr_lock_bit": false,
               "nvidia_power_limit_w": {"current": "80", "default": "80", "min": "5", "max": "100"},
               "user": {"pl1": 45.0, "pl2": 70.0}, "auto": {"normal_w": 55, "gaming_w": 90}}),
    );
    let state = fx.api.cpu_limits().unwrap();
    assert_eq!(state.to_string(), "PL1 55 W, PL2 90 W   (yours: 45/70 W)");
    assert_eq!(state.info.nvidia_power_limit_w.unwrap().max, "100");
    fx.rig.daemon.answer_ok(
        "cpu_info",
        json!({"pl1_w": 55.0, "pl2_w": 90.0, "rapl_enabled": null, "msr_lock_bit": "msr not readable", "nvidia_power_limit_w": null,
               "user": null, "auto": {"normal_w": 55, "gaming_w": 90}}),
    );
    let state = fx.api.cpu_limits().unwrap();
    assert_eq!(state.to_string(), "PL1 55 W, PL2 90 W   (automatic: 55 W, 90 W when gaming)");
    let json = serde_json::to_value(&state).unwrap();
    assert_eq!(json["msr_lock_bit"], json!("msr not readable"));
    assert_eq!(json["auto"]["gaming_w"], json!(90));
}

#[test]
fn power_limits_beyond_the_danger_line_need_force() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok("cpu_limits", json!({"pl1_w": 60.0, "pl2_w": 80.0}));
    let ok = fx.api.set_cpu_limits(60.0, 80.0, false).unwrap();
    assert_eq!(
        ok.to_string(),
        "power limits set: PL1 60 W, PL2 80 W   (the service keeps them; `truesense cpu limits --auto` goes back)"
    );
    let args = fx.rig.daemon.last_args("cpu_limits").unwrap();
    assert_eq!(
        (args.get("pl1"), args.get("pl2"), args.get("force")),
        (Some(&json!(60.0)), Some(&json!(80.0)), Some(&json!(false)))
    );
    fx.rig.daemon.clear_calls();
    let r = refusal(fx.api.set_cpu_limits(110.0, 120.0, false).unwrap_err());
    assert!(matches!(&r, Refusal::Danger { why } if why.starts_with("Beyond the danger line")), "{r:?}");
    assert!(r.to_string().ends_with("Add the explicit force flag if you really mean it (CLI: --force)."));
    assert!(fx.rig.daemon.calls().is_empty());
    fx.rig.daemon.answer_ok("cpu_limits", json!({"pl1_w": 110.0, "pl2_w": 120.0}));
    fx.api.set_cpu_limits(110.0, 120.0, true).unwrap();
    assert_eq!(fx.rig.daemon.last_args("cpu_limits").unwrap().get("force"), Some(&json!(true)));
}

#[test]
fn going_back_to_automatic_limits_asks_the_service_what_it_holds_now() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok("cpu_limits", json!(null));
    fx.rig.daemon.answer_ok(
        "cpu_info",
        json!({"pl1_w": 55.0, "pl2_w": 90.0, "user": null, "auto": {"normal_w": 55, "gaming_w": 90}}),
    );
    let state = fx.api.reset_cpu_limits().unwrap();
    assert_eq!(fx.rig.daemon.commands(), ["cpu_limits", "cpu_info"]);
    assert_eq!(fx.rig.daemon.calls()[0].1.get("reset"), Some(&json!(true)));
    assert_eq!(state.user, None);
}

#[test]
fn without_the_service_a_limit_says_so_instead_of_refusing_first() {
    let fx = Fx::offline();
    assert!(fx.api.set_cpu_limits(110.0, 120.0, false).unwrap_err().daemon_missing());
    assert!(fx.api.cpu_limits().unwrap_err().daemon_missing());
}

#[test]
fn an_undervolt_needs_the_confirmation_and_beyond_the_danger_line_force() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok("undervolt", json!({"core_mv": -50, "cache_mv": -40}));
    let r = refusal(fx.api.apply_cpu_undervolt("truesense", UndervoltRequest::default()).unwrap_err());
    assert_eq!(r, Refusal::UndervoltNeedsConfirmation { gpu: false });
    assert_eq!(
        r.to_string(),
        "an undervolt needs explicit confirmation (it can freeze or reboot the laptop if too aggressive)"
    );
    assert!(fx.rig.daemon.calls().is_empty(), "a refused undervolt must not reach the service");
    let off = fx.api.apply_cpu_undervolt("truesense", CONFIRM).unwrap();
    assert_eq!(off.to_string(), "core -50 mV, cache -40 mV");
    let args = fx.rig.daemon.last_args("undervolt").unwrap();
    assert_eq!(args.get("action"), Some(&json!("apply")));
    assert_eq!((args.get("core"), args.get("cache")), (Some(&json!(-50)), Some(&json!(-40))));
    assert_eq!(
        (args.get("confirm"), args.get("force"), args.get("name")),
        (Some(&json!(true)), Some(&json!(false)), Some(&json!("truesense")))
    );
    fx.rig.daemon.clear_calls();
    assert!(matches!(
        refusal(fx.api.apply_cpu_undervolt("aggressive", CONFIRM).unwrap_err()),
        Refusal::Danger { .. }
    ));
    assert!(matches!(
        refusal(fx.api.apply_cpu_undervolt_values("x", -120, 0, CONFIRM).unwrap_err()),
        Refusal::Danger { .. }
    ));
    assert!(fx.rig.daemon.calls().is_empty());
    fx.api.apply_cpu_undervolt("aggressive", FORCE).unwrap();
    assert_eq!(fx.rig.daemon.last_args("undervolt").unwrap().get("force"), Some(&json!(true)));
}

#[test]
fn a_reset_and_clearing_the_boot_profile_need_no_confirmation() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok("undervolt", json!({"core_mv": 0, "cache_mv": 0}));
    assert_eq!(fx.api.reset_cpu_undervolt().unwrap().core_mv, 0);
    assert_eq!(fx.rig.daemon.last_args("undervolt").unwrap().get("action"), Some(&json!("reset")));
    assert_eq!(fx.api.set_cpu_undervolt_boot(None, UndervoltRequest::default()).unwrap(), None);
    assert_eq!(fx.rig.daemon.last_args("undervolt").unwrap().get("action"), Some(&json!("boot_clear")));
    fx.api.set_cpu_undervolt_boot(Some("off"), UndervoltRequest::default()).unwrap();
    fx.api.set_cpu_undervolt_boot(Some(""), UndervoltRequest::default()).unwrap();
}

#[test]
fn the_boot_profile_is_confirmed_like_an_undervolt() {
    let fx = Fx::new();
    assert_eq!(
        refusal(fx.api.set_cpu_undervolt_boot(Some("mild"), UndervoltRequest::default()).unwrap_err()),
        Refusal::UndervoltNeedsConfirmation { gpu: false }
    );
    fx.rig.daemon.answer_ok("undervolt", json!({"name": "mild", "core": -30, "cache": -30}));
    let boot = fx.api.set_cpu_undervolt_boot(Some("mild"), CONFIRM).unwrap().unwrap();
    assert_eq!((boot.core, boot.cache), (-30, -30));
    assert_eq!(fx.rig.daemon.last_args("undervolt").unwrap().get("action"), Some(&json!("boot")));
    assert!(fx.api.set_cpu_undervolt_boot(Some("nosuch"), CONFIRM).is_err());
}

#[test]
fn setting_one_plane_keeps_the_other_at_what_the_service_reports() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok(
        "undervolt",
        json!({"core_mv": -20, "cache_mv": -10, "applied": null, "boot": null, "pending": false}),
    );
    fx.api.set_cpu_undervolt(UndervoltPlane::Cache, -35, CONFIRM).unwrap();
    let args = fx.rig.daemon.last_args("undervolt").unwrap();
    assert_eq!(
        (args.get("core"), args.get("cache"), args.get("name")),
        (Some(&json!(-20)), Some(&json!(-35)), Some(&json!("manual")))
    );
    fx.api.set_cpu_undervolt(UndervoltPlane::Core, -5, CONFIRM).unwrap();
    let args = fx.rig.daemon.last_args("undervolt").unwrap();
    assert_eq!((args.get("core"), args.get("cache")), (Some(&json!(-5)), Some(&json!(-10))));
    assert_eq!(UndervoltPlane::from_number(0).unwrap(), UndervoltPlane::Core);
    assert_eq!(UndervoltPlane::from_number(2).unwrap(), UndervoltPlane::Cache);
    assert!(UndervoltPlane::from_number(1).is_err());
}

#[test]
fn the_undervolt_state_has_a_line_for_every_case() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok(
        "undervolt",
        json!({"core_mv": -50, "cache_mv": -40, "applied": {"name": "truesense", "core": -50, "cache": -40},
               "boot": {"name": "mild", "core": -30, "cache": -30}, "pending": true}),
    );
    let st = fx.api.cpu_undervolt_state().unwrap();
    assert_eq!(st.to_string(), "core -50 mV, cache -40 mV   (applied: truesense)   (at boot: mild)");
    assert!(st.pending);
    fx.rig.daemon.answer_ok(
        "undervolt",
        json!({"error": "msr module not loaded", "applied": null, "boot": null, "pending": false}),
    );
    let st = fx.api.cpu_undervolt_state().unwrap();
    assert_eq!(st.to_string(), "not readable: msr module not loaded");
    let json = serde_json::to_value(&st).unwrap();
    assert!(json.get("core_mv").is_none());
}

#[test]
fn undervolt_profiles_list_save_copy_and_delete_with_the_old_rules() {
    let fx = Fx::new();
    let list = fx.api.cpu_undervolts().unwrap();
    assert_eq!(&list.names[..2], ["native", "truesense"]);
    assert!(list.descriptions["native"].contains("no undervolt"));
    assert!(fx.api.save_cpu_undervolt("mine", Some(-45), Some(-35)).is_ok());
    assert_eq!(fx.api.cpu_undervolt_profile("mine").unwrap().core, -45);
    assert!(fx.api.cpu_undervolts().unwrap().names.contains(&"mine".to_owned()));
    for bad in [(Some(-45), None), (None, Some(-35)), (Some(5), Some(-35)), (Some(-45), Some(-121))] {
        assert!(fx.api.save_cpu_undervolt("bad", bad.0, bad.1).is_err(), "{bad:?}");
    }
    assert_eq!(
        fx.api.save_cpu_undervolt("", Some(-1), Some(-1)).unwrap_err().to_string(),
        "usage: truesense cpu undervolt save NAME --core -50 --cache -40"
    );
    assert!(
        fx.api.save_cpu_undervolt("native", Some(-1), Some(-1)).is_err(),
        "built-in profiles are read-only"
    );
    assert_eq!(fx.api.copy_cpu_undervolt("mine", None).unwrap(), "mine-copy");
    fx.api.delete_cpu_undervolt("mine-copy").unwrap();
    assert!(fx.api.delete_cpu_undervolt("truesense").is_err());
    assert!(crate::api::Api::cpu_undervolt_is_builtin("mild"));
}

#[test]
fn the_gpu_mode_is_written_and_read_back() {
    let fx = Fx::offline();
    assert_eq!(fx.api.gpu_mode().unwrap().to_string(), "nvidia");
    assert_eq!(fx.api.set_gpu_mode(GpuMode::Optimus).unwrap(), GpuMode::Optimus);
    assert_eq!(fx.rig.sim.snapshot().gpu, 1);
    assert_eq!(crate::api::Api::parse_gpu_mode("auto").unwrap(), GpuMode::Auto);
    assert_eq!(
        crate::api::Api::parse_gpu_mode("both").unwrap_err().to_string(),
        "gpu mode: optimus | nvidia | auto (applies after reboot)"
    );
}

#[test]
fn a_gpu_undervolt_is_confirmed_and_force_guards_the_danger_line() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok("gpu_undervolt", json!({"offset_mhz": 100, "cap_mhz": 2400}));
    let r = refusal(fx.api.apply_gpu_undervolt("truesense", UndervoltRequest::default()).unwrap_err());
    assert_eq!(r, Refusal::UndervoltNeedsConfirmation { gpu: true });
    assert!(r.to_string().starts_with("a GPU undervolt needs explicit confirmation"));
    assert!(fx.rig.daemon.calls().is_empty());
    let applied = fx.api.apply_gpu_undervolt_values("m", 100, 2400, CONFIRM).unwrap();
    assert_eq!(applied.to_string(), "offset +100 MHz, cap 2400 MHz");
    let args = fx.rig.daemon.last_args("gpu_undervolt").unwrap();
    assert_eq!(
        (args.get("offset"), args.get("cap"), args.get("confirm")),
        (Some(&json!(100)), Some(&json!(2400)), Some(&json!(true)))
    );
    fx.rig.daemon.clear_calls();
    assert!(matches!(
        refusal(fx.api.apply_gpu_undervolt_values("m", 220, 0, CONFIRM).unwrap_err()),
        Refusal::Danger { .. }
    ));
    assert!(fx.rig.daemon.calls().is_empty());
    fx.api.apply_gpu_undervolt_values("m", 220, 0, FORCE).unwrap();
}

#[test]
fn the_gpu_state_reset_and_boot_profile() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok(
        "gpu_undervolt",
        json!({"offset_mhz": 120, "range_mhz": [-300, 250], "applied": {"name": "mild", "offset": 120, "cap": 0}, "boot": null, "pending": false}),
    );
    let st = fx.api.gpu_undervolt_state().unwrap();
    assert_eq!(st.to_string(), "offset +120 MHz (driver range -300..250)   (applied: mild, cap none MHz)");
    fx.rig.daemon.answer_ok("gpu_undervolt", json!({"offset_mhz": 0, "cap_mhz": 0}));
    assert_eq!(fx.api.reset_gpu_undervolt().unwrap().to_string(), "offset +0 MHz, cap none MHz");
    assert_eq!(fx.api.set_gpu_undervolt_boot(Some("off"), UndervoltRequest::default()).unwrap(), None);
    assert_eq!(fx.rig.daemon.last_args("gpu_undervolt").unwrap().get("action"), Some(&json!("boot_clear")));
    fx.rig.daemon.answer_ok("gpu_undervolt", json!({"name": "mild", "offset": 100, "cap": 0}));
    assert!(fx.api.set_gpu_undervolt_boot(Some("mild"), UndervoltRequest::default()).is_err());
    assert!(fx.api.set_gpu_undervolt_boot(Some("mild"), CONFIRM).unwrap().is_some());
    fx.rig.daemon.answer_ok(
        "gpu_undervolt",
        json!({"error": "driver too old", "applied": null, "boot": null, "pending": false}),
    );
    assert_eq!(fx.api.gpu_undervolt_state().unwrap().to_string(), "not readable: driver too old");
}

#[test]
fn gpu_profiles_are_saved_and_checked() {
    let fx = Fx::new();
    assert!(fx.api.gpu_undervolts().unwrap().names.contains(&"cool".to_owned()));
    assert_eq!(fx.api.save_gpu_undervolt("mine", Some(100), 2400).unwrap().cap, 2400);
    assert_eq!(fx.api.gpu_undervolt_profile("mine").unwrap().offset, 100);
    assert_eq!(fx.api.copy_gpu_undervolt("mine", Some("yours")).unwrap(), "yours");
    assert!(fx.api.save_gpu_undervolt("bad", None, 0).is_err());
    assert!(fx.api.save_gpu_undervolt("bad", Some(400), 0).is_err());
    assert!(fx.api.save_gpu_undervolt("bad", Some(10), 100).is_err(), "a cap is 0 or 600..3300");
    fx.api.delete_gpu_undervolt("yours").unwrap();
    assert!(crate::api::Api::gpu_undervolt_is_builtin("cool"));
    assert!(fx.api.apply_gpu_undervolt("nosuch", CONFIRM).is_err());
}

#[test]
fn the_service_saying_no_is_not_turned_into_anything_else() {
    let fx = Fx::new();
    fx.rig.daemon.answer("undervolt", Reply::Error("fixed by the firmware".into()));
    let e = fx.api.apply_cpu_undervolt_values("x", -10, -10, CONFIRM).unwrap_err();
    assert_eq!(e.to_string(), "fixed by the firmware");
}
