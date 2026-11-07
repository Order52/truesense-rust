//! `game_rules`, `power_managers`, `cpu_limits` and `cpu_info`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};
use ts_ipc::client::ClientError;
use ts_ipc::wire::Args;
use ts_platform::hw::PowerMode;

use super::rig::{Rig, Setup};
use crate::api::{ApiError, GameRuleAction, LockBit, Refusal};

fn daemon_error(e: &ApiError) -> String {
    match e {
        ApiError::Daemon(ClientError::Daemon(text)) => text.clone(),
        other => panic!("not a daemon refusal: {other:?}"),
    }
}

fn read(rig: &Rig, relative: &str) -> String {
    std::fs::read_to_string(rig.root.join(relative)).unwrap().trim().to_owned()
}

const PL1: &str = "sys/class/powercap/intel-rapl:0/constraint_0_power_limit_uw";
const PL2: &str = "sys/class/powercap/intel-rapl:0/constraint_1_power_limit_uw";

// ---- game_rules ------------------------------------------------------------------------------

#[test]
fn game_rules_list_add_remove_clear_with_python_s_reply() {
    let rig = Rig::new();
    let empty = rig.api.game_rules(&GameRuleAction::List).unwrap();
    assert!(empty.rules.is_empty());
    assert_eq!(empty.running, None);
    // The raw reply of Python `game_rules`: `{"rules": {...}, "running": mode-or-None}`.
    let raw = rig.client.call("game_rules", Args::new().with("action", "list")).unwrap();
    assert_eq!(raw, json!({"rules": {}, "running": null}));

    let add = GameRuleAction::Add { program: "EldenRing.exe".to_owned(), mode: PowerMode::Turbo };
    let rules = rig.api.game_rules(&add).unwrap();
    // `clean_name` lower-cases the program.
    assert_eq!(rules.rules.get("eldenring.exe").map(String::as_str), Some("turbo"));
    let add = GameRuleAction::Add { program: "hl2_linux".to_owned(), mode: PowerMode::Performance };
    assert_eq!(rig.api.game_rules(&add).unwrap().rules.len(), 2);
    // The rules are kept by the service: they are in its settings.
    assert_eq!(
        rig.api.daemon_state().unwrap().settings["game_rules"],
        json!({"eldenring.exe": "turbo", "hl2_linux": "performance"})
    );
    let rules = rig.api.game_rules(&GameRuleAction::Remove { program: "HL2_Linux".to_owned() }).unwrap();
    assert_eq!(rules.rules.keys().collect::<Vec<_>>(), ["eldenring.exe"]);
    let rules = rig.api.game_rules(&GameRuleAction::Clear).unwrap();
    assert!(rules.rules.is_empty());
}

#[test]
fn a_running_program_names_the_mode_it_asks_for() {
    let rig = Rig::new();
    let add = GameRuleAction::Add { program: "hl2_linux".to_owned(), mode: PowerMode::Performance };
    rig.api.game_rules(&add).unwrap();
    assert_eq!(rig.api.game_rules(&GameRuleAction::List).unwrap().running, None);
    rig.procs.run("hl2_linux");
    let rules = rig.api.game_rules(&GameRuleAction::List).unwrap();
    assert_eq!(rules.running.as_deref(), Some("performance"));
    assert_eq!(rules.to_string().lines().last(), Some("  running now: performance mode"));
}

#[test]
fn game_rules_refuse_in_python_s_words() {
    let rig = Rig::new();
    let call = |args: Args| rig.client.call("game_rules", args).map_err(|e| e.to_string());
    // `raise hw.HwError("mode: " + ", ".join(gamerules.MODES))`.
    assert_eq!(
        call(Args::new().with("action", "add").with("name", "x.exe").with("mode", "warp")).unwrap_err(),
        "mode: eco, quiet, balanced, performance, turbo"
    );
    assert_eq!(
        call(Args::new().with("action", "add").with("name", "x.exe")).unwrap_err(),
        "mode: eco, quiet, balanced, performance, turbo"
    );
    assert_eq!(
        call(Args::new().with("action", "add").with("name", "a/b").with("mode", "turbo")).unwrap_err(),
        "program name: letters, digits, . _ + - and spaces (max 80)"
    );
    assert_eq!(
        call(Args::new().with("action", "remove").with("name", "ghost")).unwrap_err(),
        "no rule for 'ghost'"
    );
    assert_eq!(
        call(Args::new().with("action", "frob")).unwrap_err(),
        "game_rules: list | add | remove | clear"
    );
    let err = rig.api.game_rules(&GameRuleAction::Remove { program: "ghost".to_owned() }).unwrap_err();
    assert_eq!(daemon_error(&err), "no rule for 'ghost'");
}

#[test]
fn at_most_sixty_four_rules() {
    let rig = Rig::new();
    for i in 0..64 {
        let add = GameRuleAction::Add { program: format!("game{i}"), mode: PowerMode::Turbo };
        rig.api.game_rules(&add).unwrap();
    }
    let add = GameRuleAction::Add { program: "one-too-many".to_owned(), mode: PowerMode::Turbo };
    assert_eq!(daemon_error(&rig.api.game_rules(&add).unwrap_err()), "at most 64 rules");
}

// ---- power_managers --------------------------------------------------------------------------

#[test]
fn power_managers_status_lists_what_runs_or_is_enabled() {
    let rig = Rig::with(Setup { managers: vec!["tlp", "power-profiles-daemon"], ..Setup::default() });
    let status = rig.api.power_managers().unwrap();
    assert!(!status.on);
    assert!(!status.recorded);
    let mut units: Vec<(&str, bool, bool)> =
        status.found.iter().map(|m| (m.unit.as_str(), m.active, m.enabled)).collect();
    units.sort_unstable();
    assert_eq!(units, [("power-profiles-daemon", true, true), ("tlp", true, true)]);
    // Python: `{"on": ..., "found": [{"unit", "active", "enabled"}], "recorded": ...}`.
    let raw = rig.client.call("power_managers", Args::new().with("action", "status")).unwrap();
    assert_eq!(
        raw["found"][0].as_object().unwrap().keys().cloned().collect::<Vec<_>>(),
        ["active", "enabled", "unit"]
    );
    assert_eq!(
        rig.api.power_managers().unwrap().to_string().lines().next(),
        Some("TrueSense manages power: off")
    );
}

#[test]
fn takeover_and_restore_report_what_python_reports() {
    let rig = Rig::with(Setup { managers: vec!["tlp"], ..Setup::default() });
    let stopped = rig.api.stop_power_managers().unwrap();
    assert_eq!(stopped.found, ["tlp"]);
    assert_eq!(stopped.stopped, ["tlp"]);
    assert!(stopped.failed.is_empty());
    assert!(stopped.on);
    // Python then puts the CPU on its defaults: powersave, balance_performance, turbo on.
    assert_eq!(stopped.cpu.governor.as_deref(), Some("powersave"));
    assert_eq!(stopped.cpu.epp.as_deref(), Some("balance_performance"));
    assert_eq!(stopped.cpu.no_turbo.as_deref(), Some("0"));
    assert!(stopped.ceiling_fixed, "{stopped:?}");
    assert_eq!(
        read(&rig, "sys/devices/system/cpu/cpu1/cpufreq/scaling_max_freq"),
        "5100000",
        "the low ceiling was lifted"
    );
    assert!(stopped.cpu_not_set.is_empty(), "{:?}", stopped.cpu_not_set);
    let status = rig.api.power_managers().unwrap();
    assert!(status.on && status.recorded && status.found.is_empty(), "{status:?}");
    assert_eq!(rig.api.daemon_state().unwrap().settings["pm_takeover"], json!(true));

    let restored = rig.api.restore_power_managers().unwrap();
    assert_eq!(restored.restored, ["tlp"]);
    assert!(!restored.on);
    assert_eq!(restored.note, None);
    // The CPU is put back the way it was found.
    assert_eq!(read(&rig, "sys/devices/system/cpu/cpu0/cpufreq/scaling_governor"), "performance");
    assert_eq!(read(&rig, "sys/devices/system/cpu/intel_pstate/no_turbo"), "1");
    let status = rig.api.power_managers().unwrap();
    assert!(!status.on && !status.recorded);
    assert_eq!(status.found.len(), 1, "tlp runs again");
    let calls = rig.systemctl.calls.lock().unwrap().join("|");
    assert!(calls.contains("disable --now tlp.service") && calls.contains("start tlp.service"), "{calls}");
}

#[test]
fn restoring_when_nothing_was_stopped_says_so_like_python() {
    let rig = Rig::new();
    let restored = rig.api.restore_power_managers().unwrap();
    assert!(restored.restored.is_empty());
    assert_eq!(restored.note.as_deref(), Some("nothing was changed by TrueSense"));
    assert!(!restored.on);
    // Python's reply in that case has no `cpu_not_set`; ours must not invent one either.
    let raw = rig.client.call("power_managers", Args::new().with("action", "restore")).unwrap();
    assert!(raw.get("cpu_not_set").is_none(), "{raw}");
    assert_eq!(raw["note"], json!("nothing was changed by TrueSense"));
    assert_eq!(restored.to_string(), "restored: nothing (TrueSense had not stopped anything)");
}

#[test]
fn the_takeover_reply_has_python_s_keys() {
    let rig = Rig::new();
    let raw = rig.client.call("power_managers", Args::new().with("action", "takeover")).unwrap();
    let mut keys: Vec<&str> = raw.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    // `{"found", "stopped", "failed", "cpu_not_set", "cpu", "ceiling_fixed"}` plus `on`.
    assert_eq!(keys, ["ceiling_fixed", "cpu", "cpu_not_set", "failed", "found", "on", "stopped"]);
    let mut cpu: Vec<&str> = raw["cpu"].as_object().unwrap().keys().map(String::as_str).collect();
    cpu.sort_unstable();
    assert_eq!(cpu, ["epp", "governor", "no_turbo"]);
    let err = rig.client.call("power_managers", Args::new().with("action", "frob")).unwrap_err();
    assert!(
        matches!(&err, ClientError::Daemon(t) if t == "power_managers: status | takeover | restore"),
        "{err:?}"
    );
}

// ---- cpu_info and cpu_limits -----------------------------------------------------------------

#[test]
fn cpu_info_has_python_s_keys_and_the_api_reads_it_two_ways() {
    let rig = Rig::new();
    let raw = rig.client.call("cpu_info", Args::new()).unwrap();
    let mut keys: Vec<&str> = raw.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    // `cpu.info()` plus `user` and `auto`.
    assert_eq!(
        keys,
        ["auto", "msr_lock_bit", "nvidia_power_limit_w", "pl1_w", "pl2_w", "rapl_enabled", "user"]
    );
    assert_eq!((raw["pl1_w"].clone(), raw["pl2_w"].clone()), (json!(80.0), json!(100.0)));
    assert_eq!(raw["rapl_enabled"], json!("1"));
    assert_eq!(raw["user"], Value::Null);
    assert!(raw["auto"]["normal_w"].is_i64() && raw["auto"]["gaming_w"].is_i64(), "{raw}");
    assert!(raw["msr_lock_bit"].is_boolean() || raw["msr_lock_bit"].is_string(), "{raw}");

    let state = rig.api.cpu_limits().unwrap();
    assert_eq!((state.info.pl1_w, state.info.pl2_w), (Some(80.0), Some(100.0)));
    assert_eq!(state.user, None);
    assert!(matches!(state.info.msr_lock_bit, LockBit::Unreadable(_) | LockBit::Locked(_)));
    // The window's direct reading of the same files agrees with the service's.
    let direct = rig.api.cpu_info();
    assert_eq!((direct.pl1_w, direct.pl2_w), (state.info.pl1_w, state.info.pl2_w));
    assert_eq!(direct.rapl_enabled, state.info.rapl_enabled);
    assert!(state.to_string().starts_with("PL1 80 W, PL2 100 W   (automatic: "), "{state}");
}

#[test]
fn setting_limits_writes_rapl_keeps_them_and_cpu_info_shows_them() {
    let rig = Rig::new();
    let applied = rig.api.set_cpu_limits(60.0, 70.0, false).unwrap();
    assert_eq!((applied.pl1_w, applied.pl2_w), (60.0, 70.0));
    assert_eq!(read(&rig, PL1), "60000000");
    assert_eq!(read(&rig, PL2), "70000000");
    let state = rig.api.cpu_limits().unwrap();
    let user = state.user.unwrap();
    assert_eq!((user.pl1, user.pl2), (60.0, 70.0));
    assert_eq!(rig.api.daemon_state().unwrap().settings["pl_user"], json!({"pl1": 60.0, "pl2": 70.0}));
    assert!(state.to_string().ends_with("(yours: 60/70 W)"), "{state}");
    // The service keeps the limits: someone else changing them is undone by the loop.
    std::fs::write(rig.root.join(PL1), "30000000").unwrap();
    rig.wait_for(|| read(&rig, PL1) == "60000000", "the loop to put the user's limits back");
}

#[test]
fn reset_goes_back_to_the_automatic_limits() {
    let rig = Rig::new();
    rig.api.set_cpu_limits(60.0, 70.0, false).unwrap();
    let state = rig.api.reset_cpu_limits().unwrap();
    assert_eq!(state.user, None);
    assert_eq!(rig.api.daemon_state().unwrap().settings["pl_user"], Value::Null);
    // Python: `{"automatic": True, **cpu.info()}`.
    let raw = rig.client.call("cpu_limits", Args::new().with("reset", true)).unwrap();
    assert_eq!(raw["automatic"], json!(true));
    assert!(raw.get("pl1_w").is_some() && raw.get("msr_lock_bit").is_some(), "{raw}");
}

#[test]
fn the_raw_reply_of_cpu_limits_is_python_s_applied_pair() {
    let rig = Rig::new();
    let raw = rig.client.call("cpu_limits", Args::new().with("pl1", 55).with("pl2", 65.5)).unwrap();
    assert_eq!(raw, json!({"pl1_w": 55.0, "pl2_w": 65.5}));
}

#[test]
fn limits_beyond_the_danger_line_need_force_in_both_halves() {
    let rig = Rig::new();
    let err = rig.api.set_cpu_limits(105.0, 118.0, false).unwrap_err();
    let Some(Refusal::Danger { why }) = err.refusal() else { panic!("{err:?}") };
    assert!(why.starts_with("Beyond the danger line (100 W sustained / 115 W burst)"), "{why}");
    // The daemon refuses with the same explanation when the client did not.
    let args = Args::new().with("pl1", 105.0).with("pl2", 118.0).with("force", false);
    let raw = rig.client.call("cpu_limits", args).unwrap_err().to_string();
    assert!(raw.starts_with(why.as_str()), "{raw}");
    assert!(raw.ends_with("Add the explicit force flag if you really mean it."), "{raw}");
    assert_eq!(read(&rig, PL1), "80000000", "nothing was written");
    // With `force` the pair is applied.
    let applied = rig.api.set_cpu_limits(105.0, 118.0, true).unwrap();
    assert_eq!(applied.pl1_w, 105.0);
}

#[test]
fn limits_outside_the_range_are_refused_by_the_hardware_layer() {
    let rig = Rig::new();
    // `PL1/PL2 must be {PL_MIN_W}..{PL_MAX_W} W with PL2 >= PL1`, with Python's 25..120.
    for (pl1, pl2) in [(20.0, 30.0), (60.0, 130.0), (70.0, 60.0)] {
        let err = rig.api.set_cpu_limits(pl1, pl2, true).unwrap_err();
        assert_eq!(daemon_error(&err), "PL1/PL2 must be 25..120 W with PL2 >= PL1", "{pl1}/{pl2}");
    }
    assert_eq!(read(&rig, PL1), "80000000");
}

#[test]
fn cpu_limits_without_numbers_asks_for_them() {
    let rig = Rig::new();
    let err = rig.client.call("cpu_limits", Args::new()).unwrap_err();
    assert!(matches!(&err, ClientError::Daemon(t) if t.contains("pl1")), "{err:?}");
    assert!(rig.client.call("cpu_limits", Args::new().with("pl1", "high").with("pl2", 70)).is_err());
}
