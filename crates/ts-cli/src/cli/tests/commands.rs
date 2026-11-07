//! Commands that need the daemon, a scripted double or a second look at the exact words: each is
//! run over the simulated services and the text (or JSON) is the one the Python edition printed.

use serde_json::{Value, json};
use ts_api::api::testing::{Reply, daemon_state};
use ts_system::services::{Scope, Step, Unit};

use super::Fx;

fn json_of(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"))
}

/// The curve of the `truesense` built-in as the daemon would send it.
fn curve_value() -> Value {
    json!({
        "hysteresis": 4.0, "down_rate": 1.5, "predict": 5.0, "couple": 15, "battery_shift": 3.0,
        "cpu": [[52, 0], [56, 30], [62, 38], [68, 48], [74, 58], [80, 70], [85, 85], [89, 100]],
        "gpu": [[48, 0], [54, 30], [60, 38], [65, 48], [70, 60], [74, 78], [78, 100]]
    })
}

/// The Smart auto profile `truesense` as the daemon would send it.
fn smart_value() -> Value {
    json!({
        "ac_idle": "balanced", "ac_browsing": "balanced", "ac_busy": "performance", "ac_turbo": true,
        "turbo_after_s": 30, "turbo_max_cpu": 85, "turbo_max_gpu": 75, "backoff_hold_s": 90, "idle_after_s": 60,
        "battery_idle": "eco", "battery_browsing": "eco", "battery_busy": "balanced", "battery_hot": "quiet",
        "battery_low_pct": 20, "up_dwell_s": 5, "down_dwell_s": 60, "busy_hold_s": 60, "min_change_s": 45,
        "turbo_margin_c": 6, "turbo_retry_s": 180
    })
}

// ---- power -------------------------------------------------------------------------------------------------------------------------

#[test]
fn power_status_adds_the_automatic_profile_when_the_service_answers() {
    let fx = Fx::new();
    fx.daemon().answer_ok("state", daemon_state(json!({"auto_profile": true}), json!({}), 125));
    let ran = fx.run("power status");
    assert_eq!(ran.out, "power mode: eco   (automatic profile on, manual choice held for 2 more min)\n");
    let json = json_of(&fx.run("--json power status").out);
    assert_eq!(json["auto_profile"], json!(true));
    assert_eq!(json["hold_left_s"], json!(125));
    let keys: Vec<&String> = json.as_object().unwrap().keys().collect();
    assert!(keys.contains(&&"mode".to_owned()) && keys.contains(&&"supported".to_owned()));
    // No daemon: only the mode.
    fx.daemon().set_available(false);
    assert_eq!(fx.run("power status").out, "power mode: eco\n");
}

#[test]
fn power_set_asks_the_service_with_a_thirty_minute_hold() {
    let fx = Fx::new();
    fx.daemon().answer_ok("power", json!("turbo"));
    let ran = fx.run("power set turbo");
    assert_eq!(
        ran.out,
        "power mode: turbo   (held 30 min against the automatic profile; `truesense power release` to end)\n"
    );
    let args = fx.daemon().last_args("power").unwrap();
    assert_eq!(args.get("mode"), Some(&json!("turbo")));
    assert_eq!(args.get("hold_minutes"), Some(&json!(30.0)));
    assert_eq!(fx.run("--json power set turbo").out, "\"turbo\"\n");
}

#[test]
fn power_auto_reads_and_changes_the_mode_control() {
    let fx = Fx::new();
    fx.daemon()
        .answer_ok("state", daemon_state(json!({"auto_mode": "native", "auto_profile": true}), json!({}), 0));
    fx.daemon().answer_ok("set", json!({}));
    assert_eq!(
        fx.run("power auto").out,
        "automatic power mode: native (power source only, like PredatorSense: AC balanced, battery eco)\n"
    );
    assert!(fx.daemon().last_args("set").is_none(), "status must not write");
    assert_eq!(fx.run("power auto toggle").out, "automatic power mode: off (you choose)\n");
    let args = fx.daemon().last_args("set").unwrap();
    assert_eq!((args.get("key"), args.get("value")), (Some(&json!("auto_mode")), Some(&json!("off"))));
    assert_eq!(fx.run("power auto on").out, "automatic power mode: smart (heat, load and battery aware)\n");
    assert_eq!(fx.run("--json power auto smart").out, "\"smart\"\n");
    // An old service without auto_mode: auto_profile decides.
    fx.daemon().answer_ok("state", daemon_state(json!({"auto_profile": false}), json!({}), 0));
    assert_eq!(fx.run("power auto").out, "automatic power mode: off (you choose)\n");
    fx.daemon().set_available(false);
    let ran = fx.run("power auto");
    assert_eq!(
        (ran.code, ran.err.as_str()),
        (1, "error: the TrueSense daemon is not running (sudo systemctl start truesense)\n")
    );
}

#[test]
fn smart_profiles_are_listed_with_the_one_in_use_starred() {
    let fx = Fx::new();
    fx.daemon()
        .answer_ok("smart_profile", json!({"name": "native", "profile": smart_value(), "names": ["native"]}));
    let ran = fx.run("power smart list");
    assert!(
        ran.out.starts_with("* native  - PredatorSense default: Balanced on power, Eco on battery"),
        "{}",
        ran.out
    );
    assert!(ran.out.contains("\n  truesense  - Our profile for this laptop"), "{}", ran.out);
    let current = fx.run("power smart");
    assert!(
        current.out.starts_with("Smart auto profile in use: native\n  ac_idle = balanced\n"),
        "{}",
        current.out
    );
    assert!(current.out.contains("  ac_turbo = True\n"));
    let json = json_of(&fx.run("--json power smart current").out);
    assert_eq!(json["name"], json!("native"));
    assert_eq!(json["profile"]["ac_busy"], json!("performance"));
}

#[test]
fn using_a_smart_profile_sends_the_data_of_your_own_only() {
    let fx = Fx::new();
    fx.daemon().answer_ok("smart_profile", json!({"name": "mine", "profile": smart_value()}));
    let ran = fx.run("power smart use native");
    assert_eq!(ran.out, "Smart auto profile in use: mine\n", "the answer of the service is shown");
    let args = fx.daemon().last_args("smart_profile").unwrap();
    assert_eq!(args.get("data"), Some(&json!(null)), "a built-in is sent by name");
    let ran = fx.run("power smart use");
    assert_eq!(ran.err, "error: which profile? (truesense power smart list)\n");
    fx.run("power smart save mine --set ac_busy=turbo");
    fx.run("power smart use mine");
    let args = fx.daemon().last_args("smart_profile").unwrap();
    assert_eq!(args.get("data").and_then(|d| d.get("ac_busy")), Some(&json!("turbo")));
}

#[test]
fn hold_and_release_talk_to_the_service() {
    let fx = Fx::new();
    fx.daemon().answer_ok("hold", json!(null));
    assert_eq!(fx.run("power hold 15").out, "holding the current power mode for 15 min\n");
    assert_eq!(fx.daemon().last_args("hold").unwrap().get("minutes"), Some(&json!(15.0)));
    assert_eq!(fx.run("power hold 0.5").out, "holding the current power mode for 0.5 min\n");
    assert_eq!(fx.run("power hold").out, "holding the current power mode for 30 min\n");
    assert_eq!(fx.run("--json power hold 2").out, "\"holding the current power mode for 2 min\"\n");
    assert_eq!(fx.run("power release").out, "hold released: the automatic profile is in charge again\n");
    assert_eq!(fx.daemon().last_args("hold").unwrap().get("minutes"), Some(&json!(null)));
}

#[test]
fn other_power_managers_are_listed_stopped_and_restored() {
    let fx = Fx::new();
    fx.daemon().answer_ok(
        "power_managers",
        json!({"on": true, "found": [{"unit": "tlp.service", "active": true, "enabled": false}], "recorded": true}),
    );
    assert_eq!(
        fx.run("power managers").out,
        "TrueSense manages power: ON\n  tlp.service              running, disabled\n"
    );
    assert_eq!(fx.daemon().last_args("power_managers").unwrap().get("action"), Some(&json!("status")));
    fx.daemon().answer_ok("power_managers", json!({"on": false, "found": [], "recorded": false}));
    assert_eq!(
        fx.run("power managers list").out,
        "TrueSense manages power: off\n  no other power manager is running or enabled\n"
    );
    fx.daemon().answer_ok(
        "power_managers",
        json!({"found": ["tlp"], "stopped": ["tlp"], "failed": [], "cpu_not_set": [], "cpu": {"no_turbo": "1", "governor": "powersave", "epp": "balance_power"}, "ceiling_fixed": false, "on": true}),
    );
    assert_eq!(
        fx.run("power managers stop").out,
        "stopped: tlp\nCPU now: governor powersave, energy preference balance_power, turbo OFF\n(undo: truesense power managers restore)\n"
    );
    assert_eq!(fx.daemon().last_args("power_managers").unwrap().get("action"), Some(&json!("takeover")));
    fx.daemon().answer_ok("power_managers", json!({"restored": [], "cpu": {}, "on": false}));
    assert_eq!(
        fx.run("power managers restore").out,
        "restored: nothing (TrueSense had not stopped anything)\n"
    );
    assert_eq!(fx.daemon().last_args("power_managers").unwrap().get("action"), Some(&json!("restore")));
}

#[test]
fn game_rules_are_listed_added_and_removed() {
    let fx = Fx::new();
    fx.daemon().answer_ok(
        "game_rules",
        json!({"rules": {"eldenring.exe": "turbo", "blender": "performance"}, "running": "turbo"}),
    );
    let ran = fx.run("power game");
    assert_eq!(
        ran.out,
        "  blender                      performance\n  eldenring.exe                turbo\n  running now: turbo mode\n"
    );
    fx.run("power game add blender performance");
    let args = fx.daemon().last_args("game_rules").unwrap();
    assert_eq!(
        (args.get("action"), args.get("name"), args.get("mode")),
        (Some(&json!("add")), Some(&json!("blender")), Some(&json!("performance")))
    );
    fx.run("power game remove blender");
    assert_eq!(fx.daemon().last_args("game_rules").unwrap().get("action"), Some(&json!("remove")));
    fx.run("power game clear");
    assert_eq!(fx.daemon().last_args("game_rules").unwrap().get("action"), Some(&json!("clear")));
    fx.daemon().answer_ok("game_rules", json!({"rules": {}, "running": null}));
    assert_eq!(fx.run("power game list").out, "  (no game rules)\n");
}

#[test]
fn next_and_prev_run_the_key_actions() {
    let fx = Fx::offline();
    assert_eq!(fx.run("power next").out, "ran power-next\n");
    assert_eq!(fx.run("power prev").out, "ran power-prev\n");
    assert_eq!(fx.rig.actions.calls(), ["power-next", "power-prev"]);
    fx.rig.actions.answer("power-next", Err("no profile"));
    let ran = fx.run("power next");
    assert_eq!((ran.code, ran.err.as_str()), (1, "error: no profile\n"));
    assert_eq!(fx.run("--json power prev").out, "\"ran power-prev\"\n");
}

// ---- fan ---------------------------------------------------------------------------------------------------------------------------

#[test]
fn the_smart_curve_switch_follows_the_service() {
    let fx = Fx::new();
    fx.daemon().answer_ok("state", daemon_state(json!({"smart_fan": false}), json!({}), 0));
    fx.daemon().answer_ok("set", json!({"smart_fan": true}));
    assert_eq!(fx.run("fan smart").out, "smart fan curve: off\n");
    assert_eq!(fx.run("fan smart on").out, "smart fan curve: on\n");
    let args = fx.daemon().last_args("set").unwrap();
    assert_eq!((args.get("key"), args.get("value")), (Some(&json!("smart_fan")), Some(&json!(true))));
    assert_eq!(fx.run("fan smart toggle").out, "smart fan curve: on\n");
    assert_eq!(fx.run("--json fan smart on").out, "true\n");
    let st = fx.run("fan status");
    assert_eq!(st.out, "fans: auto   CPU 0 rpm   GPU 0 rpm   smart curve: off\n");
}

#[test]
fn stopping_a_fan_needs_the_confirmation_and_goes_through_the_service() {
    let fx = Fx::new();
    fx.daemon().answer_ok("fan", json!("manual"));
    let ran = fx.run("fan set 0");
    assert_eq!((ran.code, ran.err.as_str()), (1, "error: stopping a fan needs explicit confirmation\n"));
    assert!(fx.daemon().calls().iter().all(|(c, _)| c != "fan"), "nothing was sent");
    let ran = fx.run("fan set 0 --confirm");
    assert_eq!(ran.code, 0, "{}", ran.err);
    assert!(
        ran.out.starts_with(
            "fans: manual 0 % / 0 %   (a stopped fan: all fans return to Auto at CPU 70 C / GPU 65 C;"
        ),
        "{}",
        ran.out
    );
    let args = fx.daemon().last_args("fan").unwrap();
    assert_eq!(
        (args.get("mode"), args.get("cpu"), args.get("confirm")),
        (Some(&json!("manual")), Some(&json!(0)), Some(&json!(true)))
    );
    let ran = fx.run("fan off");
    assert_eq!(ran.err, "error: stopping a fan needs explicit confirmation\n");
    let ran = fx.run("fan off --confirm");
    assert_eq!(
        ran.out,
        "fans: OFF   (back on Auto automatically at CPU 70 C / GPU 65 C; `truesense fan auto` now; the firmware may keep the fans at their minimum speed: check `truesense fan status`)\n"
    );
    assert_eq!(fx.daemon().last_args("fan").unwrap().get("mode"), Some(&json!("off")));
    // A duty beyond the range is refused before anything is sent.
    fx.daemon().clear_calls();
    assert_eq!(fx.run("fan set 101").code, 1);
    assert_eq!(fx.run("fan set 19 50").code, 1);
    assert!(fx.daemon().calls().is_empty());
}

#[test]
fn the_fan_goes_through_the_service_in_one_request() {
    let fx = Fx::new();
    fx.daemon().answer_ok("fan", json!("max"));
    assert_eq!(fx.run("fan max").out, "fans: MAX  (`truesense fan smart on` hands them back)\n");
    assert_eq!(fx.daemon().commands(), ["fan"]);
    assert_eq!(fx.run("fan set 60 70").out, "fans: manual 60 % / 70 %\n");
    let args = fx.daemon().last_args("fan").unwrap();
    assert_eq!((args.get("cpu"), args.get("gpu")), (Some(&json!(60)), Some(&json!(70))));
    // A service older than the program: the legacy two-step path with its pause.
    fx.daemon().answer("fan", Reply::UnknownCommand);
    fx.daemon().answer_ok("set", json!({}));
    assert_eq!(fx.run("fan auto").code, 0);
    assert!(fx.rig.sleeps.lock().unwrap().iter().any(|d| d.as_millis() == 1200));
}

#[test]
fn fan_curves_current_use_and_listing_with_the_service() {
    let fx = Fx::new();
    fx.daemon()
        .answer_ok("fan_curve", json!({"name": "truesense", "curve": curve_value(), "names": ["truesense"]}));
    let ran = fx.run("fan curve");
    assert_eq!(
        ran.out,
        "fan curve in use: truesense\n  cpu [[52, 0], [56, 30], [62, 38], [68, 48], [74, 58], [80, 70], [85, 85], [89, 100]]\n  gpu [[48, 0], [54, 30], [60, 38], [65, 48], [70, 60], [74, 78], [78, 100]]\n"
    );
    let list = fx.run("fan curve list");
    assert!(list.out.contains("\n* truesense  - Our curve for this laptop"), "{}", list.out);
    assert!(list.out.starts_with("  native  - PredatorSense default"), "{}", list.out);
    fx.daemon().answer_ok("fan_curve", json!({"name": "quiet", "curve": curve_value()}));
    assert_eq!(fx.run("fan curve use quiet").out, "fan curve in use: quiet\n");
    assert_eq!(fx.daemon().last_args("fan_curve").unwrap().get("data"), Some(&json!(null)));
    assert_eq!(fx.run("fan curve use").err, "error: which curve? (truesense fan curve list)\n");
    fx.run("fan curve copy truesense mine");
    fx.run("fan curve use mine");
    let data = fx.daemon().last_args("fan_curve").unwrap();
    assert_eq!(data.get("data").and_then(|d| d.get("hysteresis")).and_then(Value::as_f64), Some(4.0));
}

#[test]
fn a_curve_is_saved_from_another_curve_and_overrides() {
    let fx = Fx::offline();
    let ran = fx.run("fan curve save mine --from quiet --predict 2 --couple 20");
    assert_eq!(ran.out, "saved fan curve 'mine' (use it: truesense fan curve use mine)\n", "{}", ran.err);
    let shown = fx.run("fan curve show mine");
    assert!(
        shown.out.ends_with("hysteresis=4.0  down_rate=1.0  predict=2.0  couple=20  battery_shift=3.0\n"),
        "{}",
        shown.out
    );
    assert_eq!(
        fx.run("fan curve save x").err,
        "error: give --cpu and --gpu points (or --from an existing curve)\n"
    );
    assert_eq!(fx.run("fan curve save").err, "error: save under which name?\n");
    assert_eq!(fx.run("fan curve copy").err, "error: copy which curve? (truesense fan curve list)\n");
    let ran = fx.run("fan curve copy mine");
    assert_eq!(
        ran.out,
        "copied 'mine' to 'mine-copy' (edit it: truesense fan curve save mine-copy --cpu ...)\n"
    );
}

// ---- battery -----------------------------------------------------------------------------------------------------------------------

#[test]
fn charge_to_full_once() {
    let fx = Fx::new();
    fx.daemon()
        .answer_ok("battery_full", json!({"active": false, "limit": true, "capacity": 64, "ac": true}));
    assert_eq!(fx.run("battery full").out, "80 % limit on, battery 64 %, charging to full once: no\n");
    assert_eq!(fx.daemon().last_args("battery_full").unwrap().get("action"), Some(&json!("get")));
    fx.daemon().answer_ok("battery_full", json!({"active": true}));
    assert_eq!(
        fx.run("battery full start").out,
        "charging to 100 % once; the 80 % limit comes back when you unplug, when it has been full for 5 minutes, or after 24 hours\n"
    );
    assert_eq!(fx.daemon().last_args("battery_full").unwrap().get("action"), Some(&json!("start")));
    assert_eq!(fx.run("battery full cancel").out, "80 % limit back on\n");
    let json = json_of(&fx.run("--json battery full start").out);
    assert_eq!(json["active"], json!(true));
}

// ---- cpu and gpu ---------------------------------------------------------------------------------------------------------------------

#[test]
fn cpu_limits_are_shown_set_and_reset() {
    let fx = Fx::new();
    let info = json!({"pl1_w": 55.0, "pl2_w": 90.0, "rapl_enabled": "1", "msr_lock_bit": false, "nvidia_power_limit_w": null,
                      "user": null, "auto": {"normal_w": 55, "gaming_w": 90}});
    fx.daemon().answer_ok("cpu_info", info);
    assert_eq!(fx.run("cpu limits").out, "PL1 55.0 W, PL2 90.0 W   (automatic: 55 W, 90 W when gaming)\n");
    fx.daemon().answer_ok("cpu_limits", json!({"pl1_w": 60.0, "pl2_w": 80.0}));
    let ran = fx.run("cpu limits 60 80");
    assert_eq!(
        ran.out,
        "power limits set: PL1 60 W, PL2 80 W   (the service keeps them; `truesense cpu limits --auto` goes back)\n",
        "{}",
        ran.err
    );
    let args = fx.daemon().last_args("cpu_limits").unwrap();
    assert_eq!(
        (args.get("pl1"), args.get("pl2"), args.get("force")),
        (Some(&json!(60.0)), Some(&json!(80.0)), Some(&json!(false)))
    );
    fx.run("cpu limits 60");
    assert_eq!(fx.daemon().last_args("cpu_limits").unwrap().get("pl2"), Some(&json!(60.0)));
    let ran = fx.run("cpu limits --auto");
    assert!(ran.out.starts_with("automatic power limits: PL1 55.0 W, PL2 90.0 W"), "{}", ran.out);
    // Beyond the danger line, force is needed.
    let ran = fx.run("cpu limits 120 140");
    assert_eq!(ran.code, 1);
    assert!(ran.err.contains("Add the explicit force flag"), "{}", ran.err);
    assert_eq!(fx.run("cpu limits 120 140 --force").code, 0);
    assert_eq!(fx.daemon().last_args("cpu_limits").unwrap().get("force"), Some(&json!(true)));
}

#[test]
fn a_cpu_undervolt_is_confirmed_and_shown() {
    let fx = Fx::new();
    fx.daemon().answer_ok(
        "undervolt",
        json!({"core_mv": -50, "cache_mv": -40, "applied": {"name": "truesense", "core": -50, "cache": -40},
               "boot": {"name": "mild", "core": -30, "cache": -30}, "pending": false}),
    );
    assert_eq!(
        fx.run("cpu undervolt get").out,
        "core -50 mV, cache -40 mV   (applied: truesense)   (at boot: mild)\n"
    );
    fx.daemon().answer_ok("undervolt", json!({"core_mv": -50, "cache_mv": -40}));
    let ran = fx.run("cpu undervolt use truesense");
    assert_eq!(ran.code, 1);
    assert_eq!(
        ran.err,
        "error: an undervolt needs explicit confirmation (it can freeze or reboot the laptop if too aggressive)\n"
    );
    let ran = fx.run("cpu undervolt use truesense --confirm");
    assert!(
        ran.out.starts_with("applied 'truesense': core -50 mV, cache -40 mV   (a reboot returns to stock;"),
        "{}{}",
        ran.out,
        ran.err
    );
    assert_eq!(fx.daemon().last_args("undervolt").unwrap().get("name"), Some(&json!("truesense")));
    let ran = fx.run("cpu undervolt reset");
    assert_eq!(ran.out, "undervolt reset: core -50 mV, cache -40 mV\n");
    let ran = fx.run("cpu undervolt boot off");
    assert_eq!(ran.out, "no undervolt is applied at boot\n");
    assert_eq!(fx.daemon().last_args("undervolt").unwrap().get("action"), Some(&json!("boot_clear")));
    assert_eq!(fx.run("--json cpu undervolt boot").out, "null\n");
    fx.daemon().answer_ok("undervolt", json!({"name": "mild", "core": -30, "cache": -30}));
    let ran = fx.run("cpu undervolt boot mild --confirm");
    assert_eq!(
        ran.out,
        "'mild' will be applied at boot (guarded: skipped and switched off if the previous boot did not stay up 10 minutes)\n",
        "{}",
        ran.err
    );
}

#[test]
fn one_plane_of_the_cpu_undervolt_keeps_the_other() {
    let fx = Fx::new();
    fx.daemon().answer_ok(
        "undervolt",
        json!({"core_mv": -20, "cache_mv": -10, "applied": null, "boot": null, "pending": false}),
    );
    // The value is the third optional word (the Python edition's own usage line, `set -10`, puts it
    // in `name`; the rest of the command line is as it was).
    let ran = fx.run("cpu undervolt set x y -35 --confirm");
    assert_eq!(ran.code, 0, "{}", ran.err);
    let args = fx.daemon().last_args("undervolt").unwrap();
    assert_eq!(
        (args.get("core"), args.get("cache"), args.get("name")),
        (Some(&json!(-35)), Some(&json!(-10)), Some(&json!("manual")))
    );
    fx.run("cpu undervolt set x y -5 --plane 2 --confirm");
    let args = fx.daemon().last_args("undervolt").unwrap();
    assert_eq!((args.get("core"), args.get("cache")), (Some(&json!(-20)), Some(&json!(-5))));
    assert_eq!(
        fx.run("cpu undervolt set").err,
        "error: usage: truesense cpu undervolt set -10 --confirm   (or use a profile)\n"
    );
    assert_eq!(fx.run("cpu undervolt set x y -5 --plane 1").code, 2);
    // The documented form, `set -10`, is read as the profile name and so asks for the value.
    assert_eq!(
        fx.run("cpu undervolt set -10 --confirm").err,
        "error: usage: truesense cpu undervolt set -10 --confirm   (or use a profile)\n"
    );
}

#[test]
fn a_gpu_undervolt_is_confirmed_and_shown() {
    let fx = Fx::new();
    fx.daemon().answer_ok(
        "gpu_undervolt",
        json!({"offset_mhz": 120, "range_mhz": [-300, 250], "applied": {"name": "mild", "offset": 120, "cap": 0}, "boot": null, "pending": false}),
    );
    assert_eq!(
        fx.run("gpu undervolt get").out,
        "offset +120 MHz (driver range -300..250)   (applied: mild, cap none MHz)\n"
    );
    fx.daemon().answer_ok("gpu_undervolt", json!({"offset_mhz": 60, "cap_mhz": 2500}));
    let ran = fx.run("gpu undervolt use mild");
    assert_eq!(
        ran.err,
        "error: a GPU undervolt needs explicit confirmation (it can crash games or the driver if too aggressive)\n"
    );
    let ran = fx.run("gpu undervolt use mild --confirm");
    assert_eq!(
        ran.out,
        "applied 'mild': offset +60 MHz, cap 2500 MHz   (a reboot returns to stock; `truesense gpu undervolt reset` does it now)\n"
    );
    assert_eq!(fx.run("gpu undervolt reset").out, "GPU undervolt reset: offset 0, no clock cap\n");
    assert_eq!(fx.run("gpu undervolt boot").out, "no GPU undervolt is applied at boot\n");
    fx.daemon().answer_ok("gpu_undervolt", json!({"name": "mild", "offset": 60, "cap": 2500}));
    assert_eq!(
        fx.run("gpu undervolt boot mild --confirm").out,
        "'mild' will be applied at boot (guarded, like the CPU undervolt)\n"
    );
    fx.daemon().answer_ok(
        "gpu_undervolt",
        json!({"error": "driver too old", "applied": null, "boot": null, "pending": false}),
    );
    assert_eq!(fx.run("gpu undervolt get").out, "not readable: driver too old\n");
}

// ---- startup, looks ----------------------------------------------------------------------------------------------------------------

#[test]
fn the_boot_look_is_shown_set_and_cleared() {
    let fx = Fx::new();
    fx.daemon().answer_ok("boot_look", json!(null));
    assert_eq!(fx.run("startup boot").out, "no look is applied at boot\n");
    assert_eq!(fx.run("--json startup boot").out, "null\n");
    fx.daemon().answer_ok(
        "boot_look",
        json!({"keyboard": {"mode": "effect", "name": "wave", "brightness": 30, "speed": 5}, "rear": null}),
    );
    let ran = fx.run("startup boot");
    assert_eq!(
        ran.out, "at boot and after suspend: keyboard wave brightness 30 speed 5; rear unchanged\n",
        "{}",
        ran.err
    );
    assert_eq!(fx.run("startup boot clear").out, "nothing is applied at boot any more\n");
    assert_eq!(fx.daemon().last_args("boot_look").unwrap().get("action"), Some(&json!("clear")));
    assert_eq!(fx.run("--json startup boot clear").out, "null\n");
    assert_eq!(fx.run("rear set breathing").code, 0);
    let ran = fx.run("startup boot current");
    assert!(ran.out.starts_with("at boot and after suspend: "), "{}{}", ran.out, ran.err);
    assert_eq!(fx.daemon().last_args("boot_look").unwrap().get("action"), Some(&json!("set")));
    let ran = fx.run("startup boot nothing");
    assert_eq!((ran.code, ran.err.as_str()), (1, "error: no look 'nothing' (truesense look list)\n"));
}

#[test]
fn login_settings_are_validated_before_they_are_written() {
    let fx = Fx::offline();
    fx.rig.bundles.add("gaming", "fast");
    assert_eq!(fx.run("startup set profile gaming").out, "at login, profile: gaming\n");
    assert!(fx.user_config().contains("profile = \"gaming\""), "{}", fx.user_config());
    assert_eq!(fx.run("startup set profile none").out, "at login, profile: none\n");
    let ran = fx.run("startup set profile nope");
    assert_eq!(ran.err, "error: no profile 'nope': gaming\n");
    let ran = fx.run("startup set keyboard profile:nope");
    assert_eq!(ran.err, "error: no lighting profile 'nope': \n");
    let ran = fx.run("startup set rear look:nope");
    assert_eq!(ran.err, "error: no look 'nope': native\n");
    let ran = fx.run("startup set keyboard weird");
    assert_eq!(ran.code, 1, "{}", ran.out);
    assert!(ran.err.contains("[startup]"), "{}", ran.err);
    assert_eq!(fx.run("--json startup set rear last").out, "\"last\"\n");
    assert_eq!(
        fx.run("startup show").out,
        "{\n  \"profile\": \"\",\n  \"keyboard\": \"last\",\n  \"rear\": \"last\"\n}\n"
    );
}

#[test]
fn startup_apply_is_the_launchers() {
    let fx = Fx::offline();
    let ran = fx.run("startup apply");
    assert_eq!(ran.out, "profile: applied\n");
    assert_eq!(fx.launcher.calls(), ["startup apply"]);
    assert_eq!(fx.run("--json startup apply").out, "[\n  \"profile: applied\"\n]\n");
}

// ---- update ------------------------------------------------------------------------------------------------------------------------

#[test]
fn update_check_says_what_is_available() {
    let fx = Fx::offline();
    let ran = fx.run("update check");
    assert_eq!(
        ran.out,
        format!("TrueSense {0} is up to date (newest {0})\n", ts_foundation::buildinfo::VERSION)
    );
    fx.rig.updates.status.lock().unwrap().available = true;
    fx.rig.updates.status.lock().unwrap().latest = Some("9.9.9".to_owned());
    let ran = fx.run("update check --force");
    assert_eq!(
        ran.out,
        format!(
            "TrueSense {}: update 9.9.9 is available   (truesense update apply)\n",
            ts_foundation::buildinfo::VERSION
        )
    );
    let json = json_of(&fx.run("--json update check").out);
    assert_eq!(json["available"], json!(true));
    assert_eq!(json["latest"], json!("9.9.9"));
    assert_eq!(fx.run("update").out, fx.run("update check").out);
}

#[test]
fn update_apply_asks_first_and_installs_the_tag() {
    let fx = Fx::offline();
    // Nothing newer: nothing to do and nothing asked.
    let ran = fx.run("update apply");
    assert_eq!(
        ran.out,
        format!("TrueSense {} is already the newest version\n", ts_foundation::buildinfo::VERSION)
    );
    {
        let mut status = fx.rig.updates.status.lock().unwrap();
        status.available = true;
        status.latest = Some("9.9.9".to_owned());
        if let Some(info) = status.info.as_mut() {
            info.tag = "v9.9.9".to_owned();
            info.version = "9.9.9".to_owned();
        }
    }
    let declined = fx.run_with("update apply", "n\n");
    assert_eq!((declined.code, declined.err.as_str()), (1, "error: cancelled\n"));
    assert!(declined.out.contains("Install TrueSense 9.9.9 (you have "), "{}", declined.out);
    assert!(
        declined.out.contains(
            "downloads the release from github.com/example/truesense and runs its installer as root"
        )
    );
    assert!(declined.out.ends_with("Continue? [y/N] "), "{:?}", declined.out);
    assert!(fx.rig.updates.applied.lock().unwrap().is_empty());
    let silent = fx.run_with("update apply", "");
    assert_eq!(silent.err, "error: cancelled\n", "end of input is a no");
    let yes = fx.run_with("update apply", "yes\n");
    assert_eq!(yes.code, 0, "{}", yes.err);
    assert!(yes.out.contains("updated to 9.9.9 (download sha256 0123456789abcdef\u{2026}, checksum matches the release); restart the window"), "{}", yes.out);
    assert_eq!(fx.rig.updates.applied.lock().unwrap()[0].tag.as_deref(), Some("v9.9.9"));
    // --yes skips the question; --version names a release.
    let ran = fx.run("update apply --yes --version v1.2.3");
    assert!(ran.out.starts_with("updated to 1.2.3 "), "{}", ran.out);
    assert_eq!(fx.rig.updates.applied.lock().unwrap().last().unwrap().tag.as_deref(), Some("v1.2.3"));
}

#[test]
fn a_failed_installer_is_an_error() {
    let fx = Fx::offline();
    *fx.rig.updates.returncode.lock().unwrap() = 3;
    let ran = fx.run("update apply --yes --version 1.0.0");
    assert_eq!(ran.code, 1);
    assert_eq!(ran.out, "the installer exited with code 3 (see /var/log/truesense-install.log)\n");
    assert_eq!(ran.err, "error: installer exit code 3\n");
}

#[test]
fn update_auto_run_notifies_once_per_version() {
    let fx = Fx::offline();
    assert_eq!(fx.run("update auto on").out, "check for updates once a day: on\n");
    {
        let mut status = fx.rig.updates.status.lock().unwrap();
        status.available = true;
        status.latest = Some("9.9.9".to_owned());
    }
    let ran = fx.run("update auto-run");
    assert_eq!((ran.code, ran.out.as_str()), (0, ""));
    assert_eq!(fx.rig.notifier.shown().len(), 1);
    assert!(fx.rig.notifier.shown()[0].0.starts_with("TrueSense 9.9.9 is available"));
    fx.run("update auto-run");
    assert_eq!(fx.rig.notifier.shown().len(), 1, "once per version");
    fx.run("update auto off");
    fx.rig.updates.status.lock().unwrap().latest = Some("10.0.0".to_owned());
    fx.run("update auto-run");
    assert_eq!(fx.rig.notifier.shown().len(), 1, "off means nothing");
}

// ---- service, config, doctor ---------------------------------------------------------------------------------------------------

fn unit(name: &str, ok: bool) -> Unit {
    Unit {
        unit: name.to_owned(),
        scope: Scope::System,
        enabled: if ok { "enabled" } else { "unknown" }.to_owned(),
        active: if ok { "active" } else { "unknown" }.to_owned(),
        ok,
        fix: if ok { String::new() } else { "sudo truesense service repair".to_owned() },
    }
}

#[test]
fn service_check_lists_the_units_and_fails_when_one_is_down() {
    let fx = Fx::offline();
    *fx.rig.service_checks.units.lock().unwrap() = vec![unit("truesense.service", true)];
    let ran = fx.run("service check");
    assert_eq!(ran.code, 0);
    assert_eq!(ran.out, "  [ok ] truesense.service                  system enabled   active\n\nall good\n");
    *fx.rig.service_checks.units.lock().unwrap() =
        vec![unit("truesense.service", true), unit("truesense-keys.service", false)];
    let ran = fx.run("service check");
    assert_eq!(ran.code, 1);
    assert!(
        ran.out.ends_with(
            "   -> sudo truesense service repair\n\nsomething is not running: truesense service repair\n"
        ),
        "{}",
        ran.out
    );
    let json = json_of(&fx.run("--json service check").out);
    assert_eq!(json[1]["ok"], json!(false));
    assert_eq!(json[0]["unit"], json!("truesense.service"));
}

#[test]
fn service_commands_start_programs_through_the_launcher() {
    let fx = Fx::offline();
    for word in ["status", "restart", "logs"] {
        let ran = fx.run(&format!("service {word}"));
        assert_eq!((ran.code, ran.out.as_str(), ran.err.as_str()), (0, "", ""));
    }
    assert_eq!(fx.run("service").code, 0);
    assert_eq!(fx.launcher.calls(), ["service status", "service restart", "service logs", "service status"]);
}

#[test]
fn service_repair_as_a_user_runs_the_system_part_through_sudo() {
    let fx = Fx::offline();
    *fx.launcher.sudo_steps.lock().unwrap() = Some(vec![Step { what: "daemon restarted".into(), ok: true }]);
    let ran = fx.run("service repair");
    assert_eq!(ran.code, 0, "{}", ran.err);
    assert_eq!(ran.out, "  [ok ] user services restarted\n  [ok ] daemon restarted\n");
    let json = json_of(&fx.run("--json service repair").out);
    assert_eq!(
        json,
        json!([{"step": "user services restarted", "ok": true}, {"step": "daemon restarted", "ok": true}])
    );
    *fx.launcher.sudo_steps.lock().unwrap() = None;
    let ran = fx.run("service repair");
    assert_eq!(ran.code, 1);
    assert!(
        ran.out.ends_with("  [!! ] the system part did not run (sudo was refused or failed)\n"),
        "{}",
        ran.out
    );
    *fx.launcher.sudo_steps.lock().unwrap() = Some(vec![Step { what: "daemon restarted".into(), ok: false }]);
    assert_eq!(fx.run("service repair").code, 1);
}

#[test]
fn service_repair_as_root_does_the_system_part_itself() {
    let fx = Fx::build(|mut s| {
        s.euid = 0;
        s
    });
    let ran = fx.run("service repair");
    assert_eq!(ran.code, 0, "{}", ran.err);
    assert_eq!(
        ran.out,
        "  [ok ] your user services were not touched (you ran this as root): run it again without sudo for them\n  [ok ] daemon restarted\n"
    );
    assert!(fx.launcher.calls().is_empty(), "no sudo as root");
    // --system prints one JSON line of [what, ok] pairs, for the user's run to read.
    let ran = fx.run("service repair --system");
    assert_eq!((ran.code, ran.out.as_str()), (0, "[[\"daemon restarted\", true]]\n"));
}

#[test]
fn service_repair_system_needs_root() {
    let fx = Fx::offline();
    let ran = fx.run("service repair --system");
    assert_eq!((ran.code, ran.out.as_str()), (1, ""));
    assert_eq!(ran.err, "error: the system part needs root: sudo truesense service repair\n");
}

#[test]
fn without_a_service_manager_repair_says_so() {
    let fx = Fx::build(ts_api::api::Services::without_desktop);
    let ran = fx.run("service repair");
    assert_eq!(ran.err, "error: the service manager is not available here\n");
    assert_eq!(fx.run("service check").err, "error: the service manager is not available here\n");
}

#[test]
fn config_commands() {
    let fx = Fx::offline();
    let ran = fx.run("config validate");
    assert_eq!((ran.code, ran.out.as_str()), (0, "configuration valid\n"));
    assert_eq!(fx.run("--json config validate").out, "[]\n");
    std::fs::create_dir_all(fx.dir.0.join("home")).unwrap();
    std::fs::write(fx.dir.0.join("home/config.toml"), "[startup]\nkeyboard = \"weird\"\n").unwrap();
    let ran = fx.run("config validate");
    assert_eq!(ran.code, 1);
    assert!(ran.out.contains("[startup]"), "{}", ran.out);
    let paths = json_of(&fx.run("--json config path").out);
    assert_eq!(paths.as_array().map(Vec::len), Some(2));
    let shown = fx.run("config show");
    assert_eq!(shown.code, 0, "{}", shown.err);
    assert!(shown.out.contains("[startup]"), "{}", shown.out);
    let json = json_of(&fx.run("--json config show").out);
    assert_eq!(json["startup"]["keyboard"], json!("weird"));
    assert_eq!(fx.run("config").out, fx.run("config show").out);
}

#[test]
fn doctor_prints_the_report_and_exits_with_its_verdict() {
    let fx = Fx::offline();
    let ran = fx.run("doctor");
    assert!(ran.out.starts_with("TrueSense "), "{}", ran.out);
    assert!(ran.out.contains(" doctor (test)\n"), "{}", ran.out);
    assert!(ran.out.contains("failure(s)"), "{}", ran.out);
    let json = json_of(&fx.run("--json doctor").out);
    assert_eq!(json["kernel"], json!("test"));
    assert!(json["checks"].as_array().is_some_and(|c| c.len() > 5));
    let failures = json["failures"].as_u64().unwrap();
    assert_eq!(fx.run("doctor").code, i32::from(failures > 0));
}

#[test]
fn probe_prints_the_report_for_a_person_or_as_json() {
    let fx = Fx::offline();
    let ran = fx.run("probe");
    assert_eq!(ran.code, 0, "{}", ran.err);
    assert!(ran.out.contains("Verdict"), "{}", ran.out);
    assert!(ran.out.contains("Nothing was written (0 writes)."), "{}", ran.out);
    let json = json_of(&fx.run("--json probe").out);
    assert_eq!(json["writes"], json!(0));
}

#[test]
fn selftest_runs_with_the_simulated_firmware() {
    let fx = Fx::offline();
    let ran = fx.run("selftest --skip-fans");
    assert!(ran.out.contains("PASS"), "{}", ran.out);
    assert!(ran.code == 0 || ran.code == 1);
    // The fans end on firmware Auto whatever happened.
    assert_eq!(fx.run("fan status").out.lines().next().map(|l| l.starts_with("fans: ")), Some(true));
}

// ---- the commands that start something --------------------------------------------------------------------------------------------

#[test]
fn gui_tui_daemon_and_key_service_go_to_the_launcher() {
    let fx = Fx::offline();
    assert_eq!(fx.run("gui").code, 0);
    assert_eq!(fx.run("gui --page keys").code, 0);
    assert_eq!(fx.run("tui --page fans").code, 0);
    assert_eq!(fx.run("tui").code, 0);
    assert_eq!(fx.run("daemon").code, 0);
    assert_eq!(fx.run("keys run").code, 0);
    assert_eq!(fx.launcher.calls(), ["gui -", "gui keys", "tui fans", "tui -", "daemon", "keys run"]);
    let ran = fx.run("gui --page lighting");
    assert_eq!(ran.code, 2);
    assert!(ran.err.ends_with(
        "argument --page: invalid choice: 'lighting' (choose from 'home', 'keys', 'audio', 'settings')\n"
    ));
    let ran = fx.run("tui --page nope");
    assert_eq!(ran.code, 2);
}

#[test]
fn the_exit_code_of_a_launched_program_is_the_commands() {
    use std::sync::atomic::Ordering;
    let fx = Fx::offline();
    fx.launcher.interrupt.store(true, Ordering::SeqCst);
    let ran = fx.run("audio voice watch");
    assert_eq!(ran.code, 130, "an interrupted watch ends with 130");
}

// ---- the rest of the areas against their doubles ---------------------------------------------------------------------------------

#[test]
fn profiles_are_listed_shown_applied_saved_and_deleted() {
    let fx = Fx::offline();
    assert_eq!(fx.run("profile list").out, "(no profiles)\n");
    assert_eq!(fx.run("--json profile list").out, "[]\n");
    fx.rig.bundles.add("gaming", "Fast and loud");
    let ran = fx.run("profile list");
    assert_eq!(ran.out, "gaming           [  yours] Fast and loud\n");
    let json = json_of(&fx.run("--json profile list").out);
    assert_eq!(json, json!([{"name": "gaming", "source": "yours", "description": "Fast and loud"}]));
    assert_eq!(fx.run("profile apply gaming").out, "applied gaming\n");
    assert_eq!(fx.run("profile next").out, "applied gaming\n");
    assert_eq!(fx.rig.bundles.applied(), ["gaming", "gaming"]);
    let ran = fx.run("profile show gaming");
    assert!(ran.out.contains("description = \"Fast and loud\""), "{}", ran.out);
    let ran = fx.run("profile save mine --description hi");
    assert_eq!(ran.out, "saved the current state as profile 'mine'\n");
    assert_eq!(fx.run("--json profile save two").out, "\"two.toml\"\n");
    assert_eq!(fx.run("profile delete mine").out, "deleted mine\n");
    let ran = fx.run("profile delete mine");
    assert_eq!((ran.code, ran.err.as_str()), (1, "error: no profile 'mine'\n"));
    assert_eq!(fx.run("profile").out, fx.run("profile list").out);
}

#[test]
fn keys_test_runs_the_action_and_says_ok_when_it_has_nothing_to_say() {
    let fx = Fx::offline();
    assert_eq!(fx.run("keys test fan:max").out, "ran fan:max\n");
    fx.rig.actions.answer("none", Ok(""));
    assert_eq!(fx.run("keys test none").out, "ok\n");
    fx.rig.actions.answer("bad", Err("unknown action 'bad'"));
    let ran = fx.run("keys test bad");
    assert_eq!((ran.code, ran.err.as_str()), (1, "error: unknown action 'bad'\n"));
}

#[test]
fn rear_and_keyboard_power_run_the_key_actions() {
    let fx = Fx::offline();
    for (line, action) in [
        ("rear on", "rear:on"),
        ("rear off", "rear:off"),
        ("rear toggle", "rear:toggle"),
        ("keyboard on", "keyboard:on"),
        ("keyboard off", "keyboard:off"),
        ("keyboard toggle", "keyboard:toggle"),
    ] {
        assert_eq!(fx.run(line).out, format!("ran {action}\n"));
    }
}

#[test]
fn audio_commands_without_a_sound_card_say_so() {
    let fx = Fx::offline();
    let ran = fx.run("audio output");
    assert_eq!((ran.code, ran.err.as_str()), (1, "error: sound card not found\n"));
    assert_eq!(
        fx.run("audio").err,
        "error: choose: output | mic | jack | voice | speaker-filter | dts   (truesense audio -h)\n"
    );
    let ran = fx.run("audio dts");
    assert_eq!(ran.code, 1);
    assert!(ran.err.starts_with("error: "), "{}", ran.err);
    assert_eq!(fx.run("audio mic mute").out, "muted\n");
    assert_eq!(fx.run("audio mic unmute").out, "unmuted\n");
    assert_eq!(fx.run("--json audio mic mute").out, "\"muted\"\n");
    assert_eq!(
        fx.run("audio voice set headset").err,
        "error: usage: truesense audio voice set headset|array off|conference|personal\n"
    );
    assert_eq!(fx.run("audio voice notify").err, "error: usage: truesense audio voice notify on|off\n");
    assert_eq!(fx.run("audio voice notify off").out, "mic mode notification: off\n");
    assert_eq!(
        fx.run("audio jack headset").out,
        "3.5 mm jack: headset  (headset microphone is used even if the codec does not detect it)\n"
    );
    assert_eq!(fx.run("audio jack auto").out, "3.5 mm jack: auto\n");
    let ran = fx.run("audio repair");
    assert!(ran.out.starts_with("no sound card found in PipeWire: "), "{}", ran.out);
    assert_eq!(fx.run("audio speaker-filter").out, "speaker filter: off\n");
    assert_eq!(fx.run("audio mic volume").out, "microphone volume: None %\n");
    assert_eq!(fx.run("audio mic volume 200").err, "error: mic volume is 0..150 %\n");
    assert_eq!(fx.run("audio mic boost 9").err, "error: headset mic boost is 0..3\n");
    assert_eq!(fx.run("audio mic boost -1").err, "error: headset mic boost is 0..3\n");
}

#[test]
fn the_rest_of_the_keyboard_that_needs_a_picture_decoder_says_so() {
    let fx = Fx::offline();
    let ran = fx.run("keyboard image /nonexistent/x.png");
    assert_eq!(ran.err, "error: painting a picture needs an image decoder, which this build does not have\n");
}

#[test]
fn the_machine_readings_that_come_from_the_tree() {
    let fx = Fx::offline();
    let ran = fx.run("--json cpu info");
    assert_eq!(ran.code, 0);
    assert_eq!(json_of(&ran.out)["rapl_enabled"], json!(null));
    assert_eq!(fx.run("touchpad").out, "touchpad: unknown\n");
    assert_eq!(fx.run("--json touchpad status").out, "null\n");
}

#[test]
fn version_prints_the_build() {
    let fx = Fx::offline();
    let want = ts_foundation::buildinfo::VERSION;
    assert_eq!(fx.run("version").out, format!("truesense {want}\n"));
    assert_eq!(fx.run("--json version").out, format!("{{\n  \"truesense\": \"{want}\"\n}}\n"));
}
