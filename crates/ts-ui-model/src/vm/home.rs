//! What the Home page decides, as pure functions over the live frame and a few command answers.
//!
//! The page (`HomePage.qml`) owns the widgets and the commands; this module owns every *rule*: which
//! tiles are enabled, which mode control and fan mode are in force, whether a hold is running, what
//! the four live tiles show, what the slider reducers do and when the health banner appears. It
//! returns data and ids, never sentences: the words are translated on the QML side.
//!
//! Verbs (all arguments are JSON objects):
//!
//! * `view` `{frame, items?}`: everything the page paints once a second, in one answer
//!   (`control`, `fans`, `tiles`, `banner`);
//! * `power_tiles` `{supported}`: the five power-mode tiles and which of them this laptop has;
//! * `gpu_tiles` `{current}`: the three GPU modes;
//! * `duties` `{linked, single, cpu, gpu}`: the duties a manual fan write carries (never below 20);
//! * `link` `{linked, single, cpu, gpu}`: the slider values after the "both fans together" switch;
//! * `stop_verdict` `{cpu_rpm, gpu_rpm}`: whether the fans really stopped after a stop.
//!
//! Safety: nothing here can produce a duty of 0. A stop exists only as the separate, confirmed
//! `fan off` command of the page; the manual sliders are clamped to `20..=100`.
#![forbid(unsafe_code)]

use serde_json::{Value, json};

use crate::json::{at, bool_at, f64_at, list_at, str_at};

/// The power modes in the order the tiles show them.
pub const POWER_MODES: [&str; 5] = ["eco", "quiet", "balanced", "performance", "turbo"];
/// The GPU modes in the order the tiles show them.
pub const GPU_MODES: [&str; 3] = ["optimus", "nvidia", "auto"];
/// The fan modes in the order the tiles show them.
pub const FAN_MODES: [&str; 4] = ["smart", "auto", "max", "manual"];
/// The mode controls (`power auto`): the person picks, the daemon follows heat and load, or the firmware's own.
pub const MODE_CONTROLS: [&str; 3] = ["off", "native", "smart"];

/// The lowest duty a running fan is ever asked for (the firmware's own floor for a spinning fan).
pub const MIN_DUTY: i64 = 20;
/// The highest duty.
pub const MAX_DUTY: i64 = 100;
/// CPU temperature from which its live tile is drawn as hot.
pub const CPU_HOT_C: f64 = 85.0;
/// GPU temperature from which its live tile is drawn as hot.
pub const GPU_HOT_C: f64 = 75.0;
/// A fan above this speed is turning: a "stopped" fan that still reads more is held up by the firmware.
pub const FAN_TURNING_RPM: f64 = 500.0;
/// Battery percentage at or below which the battery tile warns (only while discharging).
pub const BATTERY_LOW_PCT: f64 = 20.0;
/// The capabilities (ids of `capabilities get`) whose absence means the firmware cannot be driven at all.
const ESSENTIAL: [&str; 2] = ["power_modes", "fan_control"];

/// Calls one verb of the Home view-model.
///
/// # Errors
/// The text for an unknown verb.
pub fn call(verb: &str, args: &Value) -> Result<Value, String> {
    match verb {
        "view" => Ok(view(args)),
        "power_tiles" => Ok(power_tiles(args)),
        "gpu_tiles" => Ok(gpu_tiles(args)),
        "duties" => Ok(duties(args)),
        "link" => Ok(link(args)),
        "stop_verdict" => Ok(stop_verdict(args)),
        _ => Err(format!("unknown home verb: {verb}")),
    }
}

fn tile(value: &str, enabled: bool) -> Value {
    json!({ "value": value, "enabled": enabled })
}

/// Whether the root service answered in this frame (`daemon` is an object, not `null` or an error string).
fn daemon_up(frame: &Value) -> bool {
    at(frame, "daemon").is_some_and(Value::is_object)
}

fn view(args: &Value) -> Value {
    let frame = args.get("frame").unwrap_or(&Value::Null);
    json!({
        "control": control(frame),
        "fans": fans(frame),
        "tiles": tiles(frame),
        "banner": banner(frame, args.get("items")),
    })
}

/// The five power-mode tiles. `supported` is the list `power status` gave; a tile the laptop lacks
/// stays visible and greyed, and an empty or missing list (not read yet) leaves every tile usable
/// because the command itself is the authority.
fn power_tiles(args: &Value) -> Value {
    let known: Vec<&str> = list_at(args, "supported")
        .map(|l| l.iter().filter_map(Value::as_str).filter(|m| POWER_MODES.contains(m)).collect())
        .unwrap_or_default();
    Value::Array(POWER_MODES.iter().map(|m| tile(m, known.is_empty() || known.contains(m))).collect())
}

/// The three GPU-mode tiles; `current` is the mode read from `status get`, `null` when this laptop
/// has no GPU switch (then every tile is off).
fn gpu_tiles(args: &Value) -> Value {
    let has_switch = str_at(args, "current").is_some_and(|m| GPU_MODES.contains(&m));
    let unknown = at(args, "current").is_none();
    // Not read yet is not the same as "no switch": an unknown mode keeps the tiles usable.
    Value::Array(GPU_MODES.iter().map(|m| tile(m, has_switch || unknown)).collect())
}

/// The mode control, the Smart profile and the hold of the automatic power mode.
fn control(frame: &Value) -> Value {
    let up = daemon_up(frame);
    let mode = if up { control_mode(frame) } else { None };
    let seconds = f64_at(frame, "daemon.hold_left_s").unwrap_or(0.0).max(0.0);
    let hold_active = up && mode.is_some_and(|m| m != "off") && seconds > 0.0;
    let now = str_at(frame, "daemon.live.mode_reason").filter(|s| !s.is_empty());
    json!({
        "daemon_up": up,
        "mode_control": mode,
        "smart_profile": str_at(frame, "daemon.live.smart_profile"),
        "now": if mode == Some("smart") { now } else { None },
        "hold": { "active": hold_active, "seconds": if hold_active { seconds } else { 0.0 }, "control": mode },
        "tiles": MODE_CONTROLS.iter().map(|m| tile(m, up)).collect::<Vec<_>>(),
    })
}

/// `auto_mode` of the daemon's settings, or what the older `auto_profile` flag meant.
fn control_mode(frame: &Value) -> Option<&'static str> {
    match str_at(frame, "daemon.settings.auto_mode") {
        Some(m) => MODE_CONTROLS.iter().find(|c| **c == m).copied().or(Some("off")),
        None => Some(if bool_at(frame, "daemon.settings.auto_profile") == Some(true) { "smart" } else { "off" }),
    }
}

/// The fan mode in force, the duty registers (clamped to what a slider can show: they are where a
/// switch to Manual starts from) and which fan tiles can be used.
fn fans(frame: &Value) -> Value {
    let up = daemon_up(frame);
    let smart = up && bool_at(frame, "daemon.settings.smart_fan") == Some(true);
    let reported = str_at(frame, "fans.mode");
    let mode = if smart {
        "smart"
    } else {
        reported.filter(|m| matches!(*m, "auto" | "max" | "manual")).unwrap_or("auto")
    };
    let cpu = f64_at(frame, "fans.duty_register_pct.cpu");
    let gpu = f64_at(frame, "fans.duty_register_pct.gpu");
    // Both registers at 0 in manual mode is the confirmed stop (`fan off`).
    let stopped = mode == "manual" && cpu == Some(0.0) && gpu == Some(0.0);
    let duty = match (cpu, gpu) {
        (Some(c), Some(g)) => json!({ "cpu": clamp_duty(c), "gpu": clamp_duty(g) }),
        _ => Value::Null,
    };
    json!({
        "mode": mode,
        "duty": duty,
        "stopped": stopped,
        "fan_curve": str_at(frame, "daemon.live.fan_curve"),
        "tiles": FAN_MODES.iter().map(|m| tile(m, *m != "smart" || up)).collect::<Vec<_>>(),
    })
}

#[allow(clippy::cast_possible_truncation)] // clamped to 20..=100 first
fn clamp_duty(v: f64) -> i64 {
    v.round().clamp(MIN_DUTY as f64, MAX_DUTY as f64) as i64
}

/// The four live tiles: raw numbers (the page formats them) and the flags that colour them.
fn tiles(frame: &Value) -> Value {
    let cpu_temp = f64_at(frame, "sensors.cpu_temp");
    let gpu_temp = f64_at(frame, "sensors.gpu_temp");
    let cpu_rpm = f64_at(frame, "sensors.cpu_fan_rpm");
    let gpu_rpm = f64_at(frame, "sensors.gpu_fan_rpm");
    let fan_rpm = match (cpu_rpm, gpu_rpm) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
    let pct = f64_at(frame, "battery_pct");
    let ac = bool_at(frame, "ac");
    json!({
        "cpu": { "temp": cpu_temp, "load": f64_at(frame, "load.cpu_pct"), "hot": cpu_temp.is_some_and(|t| t >= CPU_HOT_C) },
        "gpu": { "temp": gpu_temp, "load": f64_at(frame, "load.gpu_pct"), "hot": gpu_temp.is_some_and(|t| t >= GPU_HOT_C) },
        "fan": { "rpm": fan_rpm, "cpu_rpm": cpu_rpm, "gpu_rpm": gpu_rpm },
        "battery": {
            "pct": pct,
            "ac": ac,
            "low": ac == Some(false) && pct.is_some_and(|p| p <= BATTERY_LOW_PCT),
        },
    })
}

/// The health banner: the problems that make the page unreliable, none when all is well.
///
/// Nothing is reported before the first frame (the page is still loading, a banner would flash).
/// `items` is the `items` array of `capabilities get`: an essential firmware function that is
/// unsupported carries the machine's own sentence (for example that the kernel module is not
/// loaded), which is shown as it is.
fn banner(frame: &Value, items: Option<&Value>) -> Value {
    let mut problems = Vec::new();
    if frame.is_object() {
        if !daemon_up(frame) {
            problems.push(json!({ "kind": "daemon" }));
        }
        let mut seen: Vec<&str> = Vec::new();
        for item in items.and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default() {
            let (Some(id), Some(false), Some(why)) =
                (str_at(item, "id"), bool_at(item, "supported"), str_at(item, "why"))
            else {
                continue;
            };
            if ESSENTIAL.contains(&id) && !seen.contains(&why) {
                seen.push(why);
                problems.push(json!({ "kind": "firmware", "why": why }));
            }
        }
    }
    json!({ "show": !problems.is_empty(), "problems": problems })
}

fn int_arg(args: &Value, key: &str) -> Option<f64> {
    f64_at(args, key)
}

fn duty_arg(args: &Value, key: &str, default: i64) -> i64 {
    int_arg(args, key).map_or(default, clamp_duty)
}

/// The duties a manual write carries: both sliders in unlinked mode, the single one twice when linked.
fn duties(args: &Value) -> Value {
    let linked = bool_at(args, "linked").unwrap_or(true);
    let single = duty_arg(args, "single", MIN_DUTY);
    let (cpu, gpu) = if linked { (single, single) } else { (duty_arg(args, "cpu", MIN_DUTY), duty_arg(args, "gpu", MIN_DUTY)) };
    json!({ "cpu": cpu, "gpu": gpu })
}

/// The slider values after "control both fans together" was switched to `linked`: linking takes
/// the higher of the two (never quieter than what the louder fan had), unlinking copies the single one.
fn link(args: &Value) -> Value {
    let linked = bool_at(args, "linked").unwrap_or(true);
    let single = duty_arg(args, "single", MIN_DUTY);
    let cpu = duty_arg(args, "cpu", MIN_DUTY);
    let gpu = duty_arg(args, "gpu", MIN_DUTY);
    if linked {
        json!({ "single": cpu.max(gpu), "cpu": cpu, "gpu": gpu })
    } else {
        json!({ "single": single, "cpu": single, "gpu": single })
    }
}

/// After `fan off` the firmware may keep a minimum speed; the page reads the speeds a few seconds
/// later and tells the person what really happened.
fn stop_verdict(args: &Value) -> Value {
    let cpu = f64_at(args, "cpu_rpm");
    let gpu = f64_at(args, "gpu_rpm");
    let turning = [cpu, gpu].iter().flatten().any(|v| *v > FAN_TURNING_RPM);
    // With no reading at all nothing can be claimed: neither "stopped" nor "still turning".
    json!({ "turning": turning, "known": cpu.is_some() || gpu.is_some() })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn frame_with_daemon() -> Value {
        json!({
            "sensors": {"cpu_temp": 54.4, "gpu_temp": 49, "cpu_fan_rpm": 2150, "gpu_fan_rpm": 2090},
            "power_mode": "balanced", "ac": true, "battery_pct": 87,
            "fans": {"mode": "auto", "duty_register_pct": {"cpu": 40, "gpu": 40}},
            "load": {"cpu_pct": 12, "gpu_pct": 3},
            "daemon": {
                "live": {"smart_profile": "balanced", "fan_curve": "quiet", "mode_reason": "on AC, light load"},
                "settings": {"auto_mode": "smart", "smart_fan": true},
                "hold_left_s": 1500
            }
        })
    }

    fn run(verb: &str, args: Value) -> Value {
        call(verb, &args).unwrap()
    }

    fn enabled(tiles: &Value) -> Vec<(String, bool)> {
        tiles
            .as_array()
            .unwrap()
            .iter()
            .map(|t| (t["value"].as_str().unwrap().to_owned(), t["enabled"].as_bool().unwrap()))
            .collect()
    }

    #[test]
    fn unknown_verb_names_itself() {
        assert!(call("nope", &json!({})).unwrap_err().contains("nope"));
    }

    #[test]
    fn power_tiles_grey_out_what_the_laptop_lacks_and_keep_the_order() {
        let t = enabled(&run("power_tiles", json!({"supported": ["quiet", "balanced", "turbo", "warp"]})));
        assert_eq!(
            t,
            [("eco", false), ("quiet", true), ("balanced", true), ("performance", false), ("turbo", true)]
                .map(|(m, e)| (m.to_owned(), e))
        );
    }

    #[test]
    fn power_tiles_stay_usable_while_the_list_is_unknown_or_useless() {
        for args in [json!({}), json!({"supported": null}), json!({"supported": []}), json!({"supported": ["warp", 3]}), json!({"supported": "x"})] {
            assert!(enabled(&run("power_tiles", args)).iter().all(|(_, e)| *e));
        }
    }

    #[test]
    fn gpu_tiles_follow_the_switch() {
        assert!(enabled(&run("gpu_tiles", json!({"current": "optimus"}))).iter().all(|(_, e)| *e));
        assert!(enabled(&run("gpu_tiles", json!({}))).iter().all(|(_, e)| *e), "not read yet");
        assert!(enabled(&run("gpu_tiles", json!({"current": "weird"}))).iter().all(|(_, e)| !*e));
        assert_eq!(run("gpu_tiles", json!({}))[2]["value"], json!("auto"));
    }

    #[test]
    fn control_reads_the_daemon_and_shows_the_now_line_only_for_smart() {
        let v = run("view", json!({"frame": frame_with_daemon()}));
        let c = &v["control"];
        assert_eq!(c["daemon_up"], json!(true));
        assert_eq!(c["mode_control"], json!("smart"));
        assert_eq!(c["smart_profile"], json!("balanced"));
        assert_eq!(c["now"], json!("on AC, light load"));
        assert_eq!(c["hold"], json!({"active": true, "seconds": 1500.0, "control": "smart"}));
        assert!(enabled(&c["tiles"]).iter().all(|(_, e)| *e));

        let mut native = frame_with_daemon();
        native["daemon"]["settings"]["auto_mode"] = json!("native");
        let c = run("view", json!({"frame": native}))["control"].clone();
        assert_eq!(c["mode_control"], json!("native"));
        assert_eq!(c["now"], Value::Null);
        assert_eq!(c["hold"]["active"], json!(true));
    }

    #[test]
    fn a_hold_needs_an_automatic_mode_and_time_left() {
        let mut f = frame_with_daemon();
        f["daemon"]["settings"]["auto_mode"] = json!("off");
        assert_eq!(run("view", json!({"frame": f}))["control"]["hold"]["active"], json!(false));
        let mut f = frame_with_daemon();
        f["daemon"]["hold_left_s"] = json!(0);
        assert_eq!(run("view", json!({"frame": f}))["control"]["hold"]["active"], json!(false));
        let mut f = frame_with_daemon();
        f["daemon"]["hold_left_s"] = json!(-5);
        let h = run("view", json!({"frame": f}))["control"]["hold"].clone();
        assert_eq!(h["active"], json!(false));
        assert_eq!(h["seconds"], json!(0.0));
    }

    #[test]
    fn the_old_auto_profile_flag_still_means_smart_and_unknown_words_mean_off() {
        let mut f = frame_with_daemon();
        f["daemon"]["settings"] = json!({"auto_profile": true});
        assert_eq!(run("view", json!({"frame": f}))["control"]["mode_control"], json!("smart"));
        let mut f = frame_with_daemon();
        f["daemon"]["settings"] = json!({});
        assert_eq!(run("view", json!({"frame": f}))["control"]["mode_control"], json!("off"));
        let mut f = frame_with_daemon();
        f["daemon"]["settings"]["auto_mode"] = json!("warp");
        assert_eq!(run("view", json!({"frame": f}))["control"]["mode_control"], json!("off"));
    }

    #[test]
    fn without_the_daemon_the_control_is_unknown_and_off_limits() {
        for daemon in [json!(null), json!("ERROR: no socket")] {
            let mut f = frame_with_daemon();
            f["daemon"] = daemon;
            let c = run("view", json!({"frame": f}))["control"].clone();
            assert_eq!(c["daemon_up"], json!(false));
            assert_eq!(c["mode_control"], Value::Null);
            assert_eq!(c["hold"]["active"], json!(false));
            assert!(enabled(&c["tiles"]).iter().all(|(_, e)| !*e));
        }
    }

    #[test]
    fn fan_mode_prefers_the_smart_curve_then_the_firmware_state() {
        let v = run("view", json!({"frame": frame_with_daemon()}))["fans"].clone();
        assert_eq!(v["mode"], json!("smart"));
        assert_eq!(v["fan_curve"], json!("quiet"));
        assert_eq!(v["duty"], json!({"cpu": 40, "gpu": 40}), "where a switch to Manual would start");
        assert_eq!(v["stopped"], json!(false));

        let mut f = frame_with_daemon();
        f["daemon"]["settings"]["smart_fan"] = json!(false);
        f["fans"]["mode"] = json!("max");
        assert_eq!(run("view", json!({"frame": f}))["fans"]["mode"], json!("max"));

        let mut f = frame_with_daemon();
        f["daemon"]["settings"]["smart_fan"] = json!(false);
        f["fans"]["mode"] = json!("unknown");
        assert_eq!(run("view", json!({"frame": f}))["fans"]["mode"], json!("auto"));
    }

    #[test]
    fn manual_duties_are_clamped_and_a_stop_is_recognised() {
        let mut f = frame_with_daemon();
        f["daemon"]["settings"]["smart_fan"] = json!(false);
        f["fans"] = json!({"mode": "manual", "duty_register_pct": {"cpu": 10, "gpu": 140}});
        let v = run("view", json!({"frame": f.clone()}))["fans"].clone();
        assert_eq!(v["mode"], json!("manual"));
        assert_eq!(v["duty"], json!({"cpu": 20, "gpu": 100}));
        assert_eq!(v["stopped"], json!(false));

        f["fans"]["duty_register_pct"] = json!({"cpu": 0, "gpu": 0});
        let v = run("view", json!({"frame": f.clone()}))["fans"].clone();
        assert_eq!(v["stopped"], json!(true));
        assert_eq!(v["duty"], json!({"cpu": 20, "gpu": 20}), "the sliders never show or send 0");

        f["fans"]["duty_register_pct"] = json!({"cpu": 0, "gpu": 30});
        assert_eq!(run("view", json!({"frame": f}))["fans"]["stopped"], json!(false), "one fan stopped is not both");
    }

    #[test]
    fn the_smart_fan_tile_needs_the_daemon_the_others_do_not() {
        let mut f = frame_with_daemon();
        f["daemon"] = Value::Null;
        let t = enabled(&run("view", json!({"frame": f}))["fans"]["tiles"]);
        assert_eq!(
            t,
            [("smart", false), ("auto", true), ("max", true), ("manual", true)].map(|(m, e)| (m.to_owned(), e))
        );
        // The smart flag of a dead daemon is not believed.
        let mut f = frame_with_daemon();
        f["daemon"] = json!("ERROR: gone");
        f["fans"]["mode"] = json!("auto");
        assert_eq!(run("view", json!({"frame": f}))["fans"]["mode"], json!("auto"));
    }

    #[test]
    fn live_tiles_carry_numbers_and_the_hot_and_low_flags() {
        let t = run("view", json!({"frame": frame_with_daemon()}))["tiles"].clone();
        assert_eq!(t["cpu"], json!({"temp": 54.4, "load": 12.0, "hot": false}));
        assert_eq!(t["gpu"]["temp"], json!(49.0));
        assert_eq!(t["fan"], json!({"rpm": 2150.0, "cpu_rpm": 2150.0, "gpu_rpm": 2090.0}));
        assert_eq!(t["battery"], json!({"pct": 87.0, "ac": true, "low": false}));

        let hot = json!({"sensors": {"cpu_temp": 85, "gpu_temp": 74.9, "cpu_fan_rpm": 3000},
                         "ac": false, "battery_pct": 20});
        let t = run("view", json!({"frame": hot}))["tiles"].clone();
        assert_eq!(t["cpu"]["hot"], json!(true));
        assert_eq!(t["gpu"]["hot"], json!(false));
        assert_eq!(t["fan"]["rpm"], json!(3000.0), "one fan reading is enough");
        assert_eq!(t["battery"]["low"], json!(true));
    }

    #[test]
    fn missing_readings_are_null_never_zero() {
        let t = run("view", json!({"frame": {"sensors": {"cpu_temp": null}, "ac": null}}))["tiles"].clone();
        assert_eq!(t["cpu"], json!({"temp": null, "load": null, "hot": false}));
        assert_eq!(t["fan"]["rpm"], Value::Null);
        assert_eq!(t["battery"], json!({"pct": null, "ac": null, "low": false}));
        let t = run("view", json!({"frame": null}))["tiles"].clone();
        assert_eq!(t["cpu"]["temp"], Value::Null);
        let low_on_ac = run("view", json!({"frame": {"ac": true, "battery_pct": 5}}))["tiles"]["battery"]["low"].clone();
        assert_eq!(low_on_ac, json!(false), "plugged in is not a warning");
    }

    #[test]
    fn the_banner_is_silent_until_there_is_a_frame_and_while_all_is_well() {
        assert_eq!(run("view", json!({"frame": null}))["banner"], json!({"show": false, "problems": []}));
        assert_eq!(run("view", json!({}))["banner"]["show"], json!(false));
        let ok = json!({"frame": frame_with_daemon(), "items": [{"id": "power_modes", "supported": true, "why": null}]});
        assert_eq!(run("view", ok)["banner"], json!({"show": false, "problems": []}));
    }

    #[test]
    fn the_banner_names_a_dead_daemon_and_a_missing_module_once() {
        let mut f = frame_with_daemon();
        f["daemon"] = Value::Null;
        let why = "The TrueSense kernel module is not loaded.";
        let items = json!([
            {"id": "power_modes", "supported": false, "why": why},
            {"id": "fan_control", "supported": false, "why": why},
            {"id": "rear_light", "supported": false, "why": "This laptop's firmware does not offer a rear light."},
            {"id": "overdrive", "supported": true, "why": null},
        ]);
        let b = run("view", json!({"frame": f, "items": items}))["banner"].clone();
        assert_eq!(b["show"], json!(true));
        assert_eq!(b["problems"], json!([{"kind": "daemon"}, {"kind": "firmware", "why": why}]));
    }

    #[test]
    fn garbage_in_the_items_is_ignored() {
        let items = json!([1, null, "x", {"id": 3}, {"id": "power_modes"}, {"id": "power_modes", "supported": false}]);
        assert_eq!(
            run("view", json!({"frame": frame_with_daemon(), "items": items}))["banner"],
            json!({"show": false, "problems": []})
        );
        assert_eq!(run("view", json!({"frame": frame_with_daemon(), "items": "no"}))["banner"]["show"], json!(false));
    }

    #[test]
    fn manual_duties_never_go_below_the_floor_and_follow_the_link() {
        assert_eq!(run("duties", json!({"linked": true, "single": 55, "cpu": 30, "gpu": 90})), json!({"cpu": 55, "gpu": 55}));
        assert_eq!(run("duties", json!({"linked": false, "single": 55, "cpu": 30, "gpu": 90})), json!({"cpu": 30, "gpu": 90}));
        assert_eq!(run("duties", json!({"linked": false, "cpu": 0, "gpu": -5})), json!({"cpu": 20, "gpu": 20}));
        assert_eq!(run("duties", json!({"linked": true, "single": 0})), json!({"cpu": 20, "gpu": 20}));
        assert_eq!(run("duties", json!({"linked": true, "single": 250})), json!({"cpu": 100, "gpu": 100}));
        assert_eq!(run("duties", json!({})), json!({"cpu": 20, "gpu": 20}));
        assert_eq!(run("duties", json!({"single": "x"})), json!({"cpu": 20, "gpu": 20}));
    }

    #[test]
    fn linking_takes_the_higher_duty_and_unlinking_copies_the_single_one() {
        assert_eq!(run("link", json!({"linked": true, "single": 40, "cpu": 30, "gpu": 70})), json!({"single": 70, "cpu": 30, "gpu": 70}));
        assert_eq!(run("link", json!({"linked": false, "single": 45, "cpu": 30, "gpu": 70})), json!({"single": 45, "cpu": 45, "gpu": 45}));
    }

    #[test]
    fn the_stop_verdict_does_not_claim_what_it_cannot_know() {
        assert_eq!(run("stop_verdict", json!({"cpu_rpm": 0, "gpu_rpm": 0})), json!({"turning": false, "known": true}));
        assert_eq!(run("stop_verdict", json!({"cpu_rpm": 1800, "gpu_rpm": 0})), json!({"turning": true, "known": true}));
        assert_eq!(run("stop_verdict", json!({"cpu_rpm": 500, "gpu_rpm": null})), json!({"turning": false, "known": true}));
        assert_eq!(run("stop_verdict", json!({})), json!({"turning": false, "known": false}));
    }

    #[test]
    fn hostile_frames_do_not_panic() {
        for frame in [json!(null), json!(3), json!("x"), json!([1, 2]), json!({"daemon": {"settings": 5, "live": []}, "fans": "x", "sensors": []})] {
            let v = run("view", json!({"frame": frame}));
            assert!(v["control"].is_object() && v["fans"].is_object() && v["tiles"].is_object());
        }
    }
}
