//! `undervolt` and `gpu_undervolt`: apply, boot, reset and the guards, for the CPU and the GPU.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::Ordering;

use serde_json::{Value, json};
use ts_ipc::client::ClientError;
use ts_ipc::wire::Args;

use super::rig::{Rig, locked};
use crate::api::{ApiError, Refusal, UndervoltPlane, UndervoltRequest};

const CONFIRMED: UndervoltRequest = UndervoltRequest { confirm: true, force: false };
const FORCED: UndervoltRequest = UndervoltRequest { confirm: true, force: true };

fn daemon_error(e: &ApiError) -> String {
    match e {
        ApiError::Daemon(ClientError::Daemon(text)) => text.clone(),
        other => panic!("not a daemon refusal: {other:?}"),
    }
}

fn raw(rig: &Rig, cmd: &str, args: Args) -> Result<Value, String> {
    rig.client.call(cmd, args).map_err(|e| e.to_string())
}

// ---- CPU -------------------------------------------------------------------------------------

#[test]
fn the_cpu_undervolt_starts_at_stock_with_python_s_get_shape() {
    let rig = Rig::new();
    let get = raw(&rig, "undervolt", Args::new().with("action", "get")).unwrap();
    // `{**cur, "applied", "boot", "pending"}` with `cur = {"core_mv", "cache_mv"}`.
    assert_eq!(get, json!({"core_mv": 0, "cache_mv": 0, "applied": null, "boot": null, "pending": false}));
    let state = rig.api.cpu_undervolt_state().unwrap();
    assert_eq!((state.core_mv, state.cache_mv), (Some(0), Some(0)));
    assert!(state.applied.is_none() && state.boot.is_none() && !state.pending);
    assert_eq!(state.to_string(), "core 0 mV, cache 0 mV");
}

#[test]
fn an_applied_undervolt_is_written_recorded_and_answered_as_python_does() {
    let rig = Rig::new();
    let applied = rig.api.apply_cpu_undervolt_values("mine", -50, -40, CONFIRMED).unwrap();
    assert_eq!((applied.core_mv, applied.cache_mv), (-50, -40));
    assert_eq!(*locked(&rig.uv.offsets), (-50, -40));
    let state = rig.api.cpu_undervolt_state().unwrap();
    assert_eq!((state.core_mv, state.cache_mv), (Some(-50), Some(-40)));
    let record = state.applied.clone().unwrap();
    assert_eq!((record.name.as_deref(), record.core, record.cache), (Some("mine"), -50, -40));
    // Python: `self.st.set("uv_applied", {"name", "core", "cache"})`.
    assert_eq!(
        rig.api.daemon_state().unwrap().settings["uv_applied"],
        json!({"name": "mine", "core": -50, "cache": -40})
    );
    assert_eq!(state.to_string(), "core -50 mV, cache -40 mV   (applied: mine)");
}

#[test]
fn a_profile_by_name_and_a_single_plane_use_the_same_command() {
    let rig = Rig::new();
    rig.api.save_cpu_undervolt("light", Some(-30), Some(-20)).unwrap();
    let applied = rig.api.apply_cpu_undervolt("light", CONFIRMED).unwrap();
    assert_eq!((applied.core_mv, applied.cache_mv), (-30, -20));
    // One plane: the other keeps what the service reports now (Python `uv core`).
    let applied = rig.api.set_cpu_undervolt(UndervoltPlane::Cache, -35, CONFIRMED).unwrap();
    assert_eq!((applied.core_mv, applied.cache_mv), (-30, -35));
    let record = rig.api.cpu_undervolt_state().unwrap().applied.unwrap();
    assert_eq!(record.name.as_deref(), Some("manual"));
    assert_eq!(rig.api.cpu_undervolts().unwrap().names.last().map(String::as_str), Some("light"));
}

#[test]
fn an_undervolt_needs_the_confirmation_in_both_halves() {
    let rig = Rig::new();
    let err = rig.api.apply_cpu_undervolt_values("x", -50, -40, UndervoltRequest::default()).unwrap_err();
    assert_eq!(err.refusal(), Some(&Refusal::UndervoltNeedsConfirmation { gpu: false }));
    let args =
        Args::new().with("action", "apply").with("core", -50).with("cache", -40).with("confirm", false);
    // The daemon's own sentence is the same as the application layer's.
    assert_eq!(raw(&rig, "undervolt", args).unwrap_err(), err.to_string());
    let args = Args::new().with("action", "boot").with("core", -50).with("cache", -40);
    assert_eq!(raw(&rig, "undervolt", args).unwrap_err(), err.to_string());
    assert_eq!(*locked(&rig.uv.offsets), (0, 0));
}

#[test]
fn a_danger_value_needs_force_in_both_halves_with_the_same_sentence() {
    let rig = Rig::new();
    let err = rig.api.apply_cpu_undervolt_values("x", -100, -60, CONFIRMED).unwrap_err();
    let Some(Refusal::Danger { why }) = err.refusal() else { panic!("{err:?}") };
    let args =
        Args::new().with("action", "apply").with("core", -100).with("cache", -60).with("confirm", true);
    let from_daemon = raw(&rig, "undervolt", args).unwrap_err();
    // `why + " Add the explicit force flag if you really mean it (CLI: --force)."`.
    assert_eq!(from_daemon, err.to_string());
    assert!(
        from_daemon.ends_with("Add the explicit force flag if you really mean it (CLI: --force)."),
        "{from_daemon}"
    );
    assert_eq!(*locked(&rig.uv.offsets), (0, 0));
    assert!(why.len() > 20);
    let applied = rig.api.apply_cpu_undervolt_values("x", -100, -60, FORCED).unwrap();
    assert_eq!(applied.core_mv, -100);
}

#[test]
fn values_the_validator_refuses_never_reach_the_registers() {
    let rig = Rig::new();
    for (core, cache) in [(10, 0), (-1000, 0)] {
        let args =
            Args::new().with("action", "apply").with("core", core).with("cache", cache).with("confirm", true);
        assert!(raw(&rig, "undervolt", args.with("force", true)).is_err(), "{core}/{cache}");
    }
    let args = Args::new().with("action", "apply").with("confirm", true);
    assert!(raw(&rig, "undervolt", args).is_err(), "no numbers at all");
    let args =
        Args::new().with("action", "apply").with("core", "deep").with("cache", 0).with("confirm", true);
    assert!(raw(&rig, "undervolt", args).is_err());
    assert_eq!(*locked(&rig.uv.offsets), (0, 0));
}

#[test]
fn a_failed_apply_puts_the_stock_voltage_back_and_records_nothing() {
    let rig = Rig::new();
    rig.api.apply_cpu_undervolt_values("first", -20, -10, CONFIRMED).unwrap();
    rig.uv.locked.store(true, Ordering::SeqCst);
    let err = rig.api.apply_cpu_undervolt_values("second", -50, -40, CONFIRMED).unwrap_err();
    assert!(daemon_error(&err).contains("BIOS lock"), "{err}");
    // Python: on a write failure `cpu.uv_reset()`, and `uv_applied` keeps the earlier value.
    assert_eq!(*locked(&rig.uv.offsets), (0, 0));
    let applied = rig.api.cpu_undervolt_state().unwrap().applied.unwrap();
    assert_eq!(applied.name.as_deref(), Some("first"));
}

#[test]
fn boot_records_the_choice_and_clearing_forgets_it() {
    let rig = Rig::new();
    let record = rig.api.set_cpu_undervolt_boot_values("light", -30, -20, CONFIRMED).unwrap();
    assert_eq!((record.name.as_deref(), record.core, record.cache), (Some("light"), -30, -20));
    // `boot` writes nothing to the registers.
    assert_eq!(*locked(&rig.uv.offsets), (0, 0));
    let raw_record = rig.api.daemon_state().unwrap().settings["uv_boot"].clone();
    assert_eq!(raw_record, json!({"name": "light", "core": -30, "cache": -20}));
    assert_eq!(rig.api.cpu_undervolt_state().unwrap().boot.unwrap().name.as_deref(), Some("light"));
    assert_eq!(rig.api.set_cpu_undervolt_boot(None, UndervoltRequest::default()).unwrap(), None);
    assert_eq!(rig.api.cpu_undervolt_state().unwrap().boot, None);
    assert_eq!(rig.api.daemon_state().unwrap().settings["uv_boot"], Value::Null);
    assert_eq!(raw(&rig, "undervolt", Args::new().with("action", "boot_clear")).unwrap(), Value::Null);
    rig.api.save_cpu_undervolt("light", Some(-30), Some(-20)).unwrap();
    let boot = rig.api.set_cpu_undervolt_boot(Some("light"), CONFIRMED).unwrap().unwrap();
    assert_eq!(boot.core, -30);
}

#[test]
fn reset_writes_stock_and_clears_the_applied_record() {
    let rig = Rig::new();
    rig.api.apply_cpu_undervolt_values("x", -50, -40, CONFIRMED).unwrap();
    let offsets = rig.api.reset_cpu_undervolt().unwrap();
    assert_eq!((offsets.core_mv, offsets.cache_mv), (0, 0));
    assert_eq!(rig.api.daemon_state().unwrap().settings["uv_applied"], Value::Null);
    assert_eq!(*locked(&rig.uv.offsets), (0, 0));
    assert_eq!(rig.api.cpu_undervolt_state().unwrap().applied, None);
}

#[test]
fn the_crash_guard_flag_shows_as_pending() {
    let rig = Rig::new();
    std::fs::create_dir_all(rig.dir.0.join("var")).unwrap();
    std::fs::write(rig.dir.0.join("var/uv-pending"), "1").unwrap();
    assert!(rig.api.cpu_undervolt_state().unwrap().pending);
    assert_eq!(raw(&rig, "undervolt", Args::new()).unwrap()["pending"], json!(true));
}

#[test]
fn an_unknown_undervolt_action_is_refused_in_python_s_words() {
    let rig = Rig::new();
    assert_eq!(
        raw(&rig, "undervolt", Args::new().with("action", "frob")).unwrap_err(),
        "undervolt: get | apply | reset | boot | boot_clear"
    );
    assert_eq!(
        raw(&rig, "gpu_undervolt", Args::new().with("action", "frob")).unwrap_err(),
        "gpu_undervolt: get | apply | reset | boot | boot_clear"
    );
}

// ---- GPU -------------------------------------------------------------------------------------

#[test]
fn the_gpu_undervolt_starts_at_stock_with_python_s_get_shape() {
    let rig = Rig::new();
    let get = raw(&rig, "gpu_undervolt", Args::new().with("action", "get")).unwrap();
    // `gpuundervolt.read()` plus `applied`, `boot` and `pending`.
    assert_eq!(
        get,
        json!({"offset_mhz": 0, "range_mhz": [-300, 250], "applied": null, "boot": null, "pending": false})
    );
    let state = rig.api.gpu_undervolt_state().unwrap();
    assert_eq!((state.offset_mhz, state.range_mhz), (Some(0), Some([-300, 250])));
    assert_eq!(state.to_string(), "offset +0 MHz (driver range -300..250)");
}

#[test]
fn a_gpu_undervolt_is_applied_with_its_cap_and_recorded() {
    let rig = Rig::new();
    let applied = rig.api.apply_gpu_undervolt_values("mine", 120, 2400, CONFIRMED).unwrap();
    assert_eq!((applied.offset_mhz, applied.cap_mhz), (120, 2400));
    assert_eq!(*locked(&rig.gpu_uv.0), (120, 2400));
    let state = rig.api.gpu_undervolt_state().unwrap();
    assert_eq!(state.offset_mhz, Some(120));
    let record = state.applied.clone().unwrap();
    assert_eq!((record.name.as_deref(), record.offset, record.cap), (Some("mine"), 120, 2400));
    // Python records `{"name": name, **p}` where `p` is the validated profile.
    let stored = rig.api.daemon_state().unwrap().settings["gpu_uv_applied"].clone();
    assert_eq!(stored["name"], json!("mine"));
    assert_eq!((stored["offset"].clone(), stored["cap"].clone()), (json!(120), json!(2400)));
    assert!(state.to_string().contains("(applied: mine, cap 2400 MHz)"), "{state}");
}

#[test]
fn a_gpu_apply_without_a_cap_means_none() {
    let rig = Rig::new();
    // Python: `cap=0` is the default of the command, and `validate` keeps `p.get("cap", 0)`.
    let args =
        Args::new().with("action", "apply").with("offset", 100).with("confirm", true).with("name", "n");
    let reply = raw(&rig, "gpu_undervolt", args).unwrap();
    assert_eq!(reply, json!({"offset_mhz": 100, "cap_mhz": 0}));
    assert_eq!(rig.api.gpu_undervolt_state().unwrap().applied.unwrap().cap, 0);
}

#[test]
fn a_gpu_undervolt_needs_the_confirmation_in_both_halves() {
    let rig = Rig::new();
    let err = rig.api.apply_gpu_undervolt_values("x", 100, 2200, UndervoltRequest::default()).unwrap_err();
    assert_eq!(err.refusal(), Some(&Refusal::UndervoltNeedsConfirmation { gpu: true }));
    let args = Args::new().with("action", "apply").with("offset", 100).with("cap", 2200);
    assert_eq!(raw(&rig, "gpu_undervolt", args).unwrap_err(), err.to_string());
    assert_eq!(*locked(&rig.gpu_uv.0), (0, 0));
}

#[test]
fn a_gpu_danger_value_needs_force_with_the_same_sentence_in_both_halves() {
    let rig = Rig::new();
    let err = rig.api.apply_gpu_undervolt_values("x", 250, 1200, CONFIRMED).unwrap_err();
    let Some(Refusal::Danger { .. }) = err.refusal() else { panic!("{err:?}") };
    let args =
        Args::new().with("action", "apply").with("offset", 250).with("cap", 1200).with("confirm", true);
    assert_eq!(raw(&rig, "gpu_undervolt", args).unwrap_err(), err.to_string());
    assert_eq!(*locked(&rig.gpu_uv.0), (0, 0));
    let applied = rig.api.apply_gpu_undervolt_values("x", 250, 1200, FORCED).unwrap();
    assert_eq!(applied.offset_mhz, 250);
}

#[test]
fn gpu_values_the_validator_refuses_never_reach_the_driver() {
    let rig = Rig::new();
    for (offset, cap) in [(400, 0), (-400, 0), (100, 100), (100, 9000)] {
        let args = Args::new()
            .with("action", "apply")
            .with("offset", offset)
            .with("cap", cap)
            .with("confirm", true)
            .with("force", true);
        assert!(raw(&rig, "gpu_undervolt", args).is_err(), "{offset}/{cap}");
    }
    let args =
        Args::new().with("action", "apply").with("offset", 100).with("cap", "high").with("confirm", true);
    assert!(raw(&rig, "gpu_undervolt", args).is_err());
    assert_eq!(*locked(&rig.gpu_uv.0), (0, 0));
}

#[test]
fn gpu_boot_and_reset_follow_the_cpu_ones() {
    let rig = Rig::new();
    let record = rig.api.set_gpu_undervolt_boot_values("cool", 100, 2100, CONFIRMED).unwrap();
    assert_eq!((record.name.as_deref(), record.offset, record.cap), (Some("cool"), 100, 2100));
    assert_eq!(*locked(&rig.gpu_uv.0), (0, 0), "boot writes nothing now");
    assert_eq!(rig.api.gpu_undervolt_state().unwrap().boot.unwrap().name.as_deref(), Some("cool"));
    assert_eq!(rig.api.set_gpu_undervolt_boot(None, UndervoltRequest::default()).unwrap(), None);
    assert_eq!(rig.api.gpu_undervolt_state().unwrap().boot, None);
    rig.api.apply_gpu_undervolt_values("x", 80, 2300, CONFIRMED).unwrap();
    let reset = rig.api.reset_gpu_undervolt().unwrap();
    assert_eq!((reset.offset_mhz, reset.cap_mhz), (0, 0));
    assert_eq!(rig.api.gpu_undervolt_state().unwrap().applied, None);
    assert_eq!(rig.api.daemon_state().unwrap().settings["gpu_uv_applied"], Value::Null);
}
