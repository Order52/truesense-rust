//! What the Audio page decides: how the answers of the `audio *` commands become control states
//! (the port of the refresh of the web page `audio.js`), which output a cycle button picks, and
//! how the results of the two long operations (repair, "DTS on every output") are summarised.
//!
//! As in every view-model, the output is data (codes, numbers, short lists): the sentences are
//! chosen and translated by the QML page. A section whose command failed is `null`, and the page
//! leaves its controls alone rather than showing a made-up value.
//!
//! Verbs: `state` (the answers of the status commands), `next_output`, `repair_summary`,
//! `dts_all_summary`, `options`.
#![forbid(unsafe_code)]

use serde_json::{Map, Value, json};

use crate::json::{at, bool_at, f64_at, list_at, str_at};

/// The DTS sound modes the firmware's audio profile knows, in the order the page lists them.
pub const DTS_MODES: &[&str] = &["Music", "Movie", "Voice", "Game1", "Game2", "Game3", "Plain"];
/// The Purified Voice noise-reduction modes.
pub const VOICE_MODES: &[&str] = &["off", "conference", "personal"];
/// What the 3.5 mm jack setting can be.
pub const JACK_SETTINGS: &[&str] = &["auto", "headphones", "headset"];
/// Largest microphone volume the page offers, in percent (above 100 amplifies).
pub const MIC_VOLUME_MAX: f64 = 150.0;
/// Largest headset microphone boost step.
pub const BOOST_MAX: f64 = 3.0;

/// The next output for a "switch output" button: the other one. `headphones` is chosen only when
/// the jack reports something plugged in; with nothing plugged in the answer stays `speakers`,
/// because the command would refuse headphones and a button must not offer a refusal.
pub fn next_output(active: Option<&str>, jack_plugged: Option<bool>) -> &'static str {
    match active {
        Some("speakers") if jack_plugged != Some(false) => "headphones",
        _ => "speakers",
    }
}

fn sound_section(v: Option<&Value>) -> Value {
    let Some(v) = v.filter(|v| v.is_object()) else { return Value::Null };
    // `ok` is true (an output exists), false (a card without an output) or null (no card at all).
    let (state, problem) = match v.get("ok") {
        Some(Value::Bool(true)) => ("ok", Value::Null),
        Some(Value::Bool(false)) => ("problem", json!(str_at(v, "problem"))),
        Some(Value::Null) => ("no-card", Value::Null),
        _ => ("unknown", Value::Null),
    };
    json!({ "state": state, "problem": problem })
}

fn dts_section(v: Option<&Value>) -> Value {
    let Some(v) = v.filter(|v| v.is_object()) else { return Value::Null };
    let devices: Vec<Value> = list_at(v, "devices")
        .unwrap_or_default()
        .iter()
        .filter(|d| d.is_object())
        .map(|d| {
            json!({
                "label": str_at(d, "label").unwrap_or(""),
                "kind": str_at(d, "kind").unwrap_or(""),
                "profile": str_at(d, "profile").unwrap_or(""),
                "current": bool_at(d, "current").unwrap_or(false),
            })
        })
        .collect();
    // The mode shown is the one of the output in use, or of the first output when none is flagged;
    // a profile that is not a DTS mode (a plain stereo one) selects nothing.
    let chosen = devices
        .iter()
        .find(|d| d["current"] == true)
        .or_else(|| devices.first())
        .and_then(|d| d["profile"].as_str())
        .filter(|p| DTS_MODES.contains(p))
        .map(str::to_owned);
    let text = devices
        .iter()
        .map(|d| format!("{}: {}", d["label"].as_str().unwrap_or(""), d["profile"].as_str().unwrap_or("")))
        .collect::<Vec<_>>()
        .join(" \u{b7} ");
    json!({
        "on": bool_at(v, "on").unwrap_or(false),
        "mode": chosen,
        "devices": devices,
        "devices_text": text,
    })
}

fn jack_section(v: Option<&Value>) -> Value {
    let Some(v) = v.filter(|v| v.is_object()) else { return Value::Null };
    let setting = str_at(v, "setting").filter(|s| JACK_SETTINGS.contains(s)).unwrap_or("auto");
    let detected = if bool_at(v, "headset_mic") == Some(true) {
        "headset"
    } else if bool_at(v, "headphones") == Some(true) {
        "headphones"
    } else {
        "none"
    };
    json!({ "setting": setting, "detected": detected })
}

fn mic_section(v: Option<&Value>) -> Value {
    let Some(v) = v.filter(|v| v.is_object()) else { return Value::Null };
    json!({
        "muted": bool_at(v, "muted").unwrap_or(false),
        "volume": f64_at(v, "volume").map(|x| x.round().clamp(0.0, MIC_VOLUME_MAX)),
        "boost": f64_at(v, "boost").map(|x| x.round().clamp(0.0, BOOST_MAX)),
    })
}

fn voice_section(v: Option<&Value>) -> Value {
    let Some(v) = v.filter(|v| v.is_object()) else { return Value::Null };
    let mode = |path: &str| str_at(v, path).filter(|m| VOICE_MODES.contains(m)).unwrap_or("off");
    json!({ "headset": mode("modes.headset"), "array": mode("modes.array"), "notify": bool_at(v, "notify") })
}

fn output_section(v: Option<&Value>) -> Value {
    let Some(v) = v.filter(|v| v.is_object()) else { return Value::Null };
    json!({
        "active": str_at(v, "active").filter(|a| matches!(*a, "speakers" | "headphones")),
        "jack_plugged": bool_at(v, "jack_plugged"),
    })
}

/// The page model for the answers of the status commands. `args` carries `sound` (`audio sound`),
/// `output`, `filter` (a boolean), `dts`, `jack`, `mic` and `voice`; any of them may be missing or
/// `null` (a command that failed or was not asked this time), which gives a `null` section.
pub fn state(args: &Value) -> Value {
    let mut out = Map::new();
    out.insert("sound".into(), sound_section(at(args, "sound")));
    out.insert("output".into(), output_section(at(args, "output")));
    out.insert("filter".into(), at(args, "filter").and_then(Value::as_bool).map_or(Value::Null, Value::Bool));
    out.insert("dts".into(), dts_section(at(args, "dts")));
    out.insert("jack".into(), jack_section(at(args, "jack")));
    out.insert("mic".into(), mic_section(at(args, "mic")));
    out.insert("voice".into(), voice_section(at(args, "voice")));
    Value::Object(out)
}

/// What `audio repair` did: `repaired` (something was changed and the output is there), `nothing`
/// (it was there already) or `failed`, with the steps joined for the message.
pub fn repair_summary(result: &Value) -> Value {
    let fixed = bool_at(result, "fixed").unwrap_or(false);
    let changed = bool_at(result, "changed").unwrap_or(false);
    let steps: Vec<&str> = list_at(result, "steps").unwrap_or_default().iter().filter_map(Value::as_str).collect();
    let kind = match (fixed, changed) {
        (true, true) => "repaired",
        (true, false) => "nothing",
        (false, _) => "failed",
    };
    json!({ "kind": kind, "steps": steps.join("; ") })
}

/// What "DTS on every output" did: the outputs it set up, joined, and how many.
pub fn dts_all_summary(result: &Value) -> Value {
    let done: Vec<&str> = list_at(result, "done").unwrap_or_default().iter().filter_map(Value::as_str).collect();
    json!({ "count": done.len(), "text": done.join("; ") })
}

/// The fixed choices of the page, so QML and tests share one list.
pub fn options() -> Value {
    json!({ "dts_modes": DTS_MODES, "voice_modes": VOICE_MODES, "jack_settings": JACK_SETTINGS })
}

/// `state`, `next_output`, `repair_summary`, `dts_all_summary`, `options`.
///
/// # Errors
/// The text for an unknown verb.
pub fn call(verb: &str, args: &Value) -> Result<Value, String> {
    match verb {
        "state" => Ok(state(args)),
        "next_output" => Ok(json!(next_output(str_at(args, "active"), bool_at(args, "jack_plugged")))),
        "repair_summary" => Ok(repair_summary(args)),
        "dts_all_summary" => Ok(dts_all_summary(args)),
        "options" => Ok(options()),
        _ => Err(format!("unknown audio verb: {verb}")),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn full() -> Value {
        json!({
            "sound": {"ok": true, "problem": null, "sinks": []},
            "output": {"active": "speakers", "jack_plugged": false},
            "filter": false,
            "dts": {"on": true, "mode": "Music", "devices": [
                {"label": "Speakers", "kind": "builtin", "profile": "Movie", "current": true},
                {"label": "Headphones", "kind": "jack", "profile": "Music", "current": false}]},
            "jack": {"headphones": false, "headset_mic": true, "raw": "x", "setting": "headset", "microphone_in_use": true},
            "mic": {"default": "mic", "muted": true, "sources": [], "volume": 87.6, "boost": 2},
            "voice": {"modes": {"headset": "personal", "array": "conference"}, "notify": true},
        })
    }

    #[test]
    fn the_sections_follow_the_status_answers() {
        let s = state(&full());
        assert_eq!(s["sound"], json!({"state": "ok", "problem": null}));
        assert_eq!(s["output"], json!({"active": "speakers", "jack_plugged": false}));
        assert_eq!(s["filter"], false);
        assert_eq!(s["dts"]["on"], true);
        assert_eq!(s["dts"]["mode"], "Movie", "the mode of the output in use");
        assert_eq!(s["dts"]["devices_text"], "Speakers: Movie \u{b7} Headphones: Music");
        assert_eq!(s["jack"], json!({"setting": "headset", "detected": "headset"}));
        assert_eq!(s["mic"], json!({"muted": true, "volume": 88.0, "boost": 2.0}));
        assert_eq!(s["voice"], json!({"headset": "personal", "array": "conference", "notify": true}));
    }

    #[test]
    fn sound_states_are_ok_problem_no_card_and_unknown() {
        let s = |v: Value| state(&json!({"sound": v}))["sound"].clone();
        assert_eq!(s(json!({"ok": false, "problem": "no sink"})), json!({"state": "problem", "problem": "no sink"}));
        assert_eq!(s(json!({"ok": null})), json!({"state": "no-card", "problem": null}));
        assert_eq!(s(json!({"ok": "?"}))["state"], "unknown");
        assert_eq!(s(json!({}))["state"], "unknown");
        assert_eq!(s(json!(null)), Value::Null);
    }

    #[test]
    fn failed_commands_leave_their_sections_null() {
        let s = state(&json!({"output": null, "jack": "ERROR: no daemon"}));
        for key in ["sound", "output", "filter", "dts", "jack", "mic", "voice"] {
            assert_eq!(s[key], Value::Null, "{key}");
        }
        assert_eq!(state(&json!(null))["dts"], Value::Null);
    }

    #[test]
    fn dts_mode_comes_from_the_current_device_else_the_first_and_only_if_it_is_a_dts_mode() {
        let mode = |devices: Value| state(&json!({"dts": {"on": false, "devices": devices}}))["dts"]["mode"].clone();
        assert_eq!(mode(json!([{"profile": "Game2", "current": false}, {"profile": "Voice", "current": true}])), "Voice");
        assert_eq!(mode(json!([{"profile": "Game2"}, {"profile": "Voice"}])), "Game2");
        assert_eq!(mode(json!([{"profile": "Stereo", "current": true}])), Value::Null);
        assert_eq!(mode(json!([])), Value::Null);
        assert_eq!(mode(json!("junk")), Value::Null);
        assert_eq!(state(&json!({"dts": {"on": false, "devices": []}}))["dts"]["devices_text"], "");
        // Every listed mode is accepted.
        for m in DTS_MODES {
            assert_eq!(mode(json!([{"profile": m, "current": true}])), *m);
        }
    }

    #[test]
    fn jack_setting_and_detection_have_safe_defaults() {
        let j = |v: Value| state(&json!({"jack": v}))["jack"].clone();
        assert_eq!(j(json!({"setting": "bogus", "headphones": true})), json!({"setting": "auto", "detected": "headphones"}));
        assert_eq!(j(json!({"headphones": true, "headset_mic": true}))["detected"], "headset");
        assert_eq!(j(json!({}))["detected"], "none");
        assert_eq!(j(json!({"setting": "headphones"}))["setting"], "headphones");
    }

    #[test]
    fn microphone_values_are_rounded_and_clamped() {
        let m = |v: Value| state(&json!({"mic": v}))["mic"].clone();
        assert_eq!(m(json!({"volume": 400, "boost": 9}))["volume"], 150.0);
        assert_eq!(m(json!({"volume": 400, "boost": 9}))["boost"], 3.0);
        assert_eq!(m(json!({"volume": -5, "boost": -1}))["volume"], 0.0);
        assert_eq!(m(json!({"volume": null}))["volume"], Value::Null);
        assert_eq!(m(json!({"volume": "loud"}))["volume"], Value::Null);
        assert_eq!(m(json!({}))["muted"], false);
    }

    #[test]
    fn unknown_voice_modes_read_as_off() {
        let v = |x: Value| state(&json!({"voice": x}))["voice"].clone();
        assert_eq!(v(json!({"modes": {"headset": "turbo", "array": null}})), json!({"headset": "off", "array": "off", "notify": null}));
        assert_eq!(v(json!({"modes": {"headset": "off", "array": "personal"}, "notify": false}))["array"], "personal");
        assert_eq!(v(json!({}))["headset"], "off");
        for m in VOICE_MODES {
            assert_eq!(v(json!({"modes": {"headset": m}}))["headset"], *m);
        }
    }

    #[test]
    fn the_output_button_picks_the_other_output_but_never_a_refusal() {
        assert_eq!(next_output(Some("speakers"), Some(true)), "headphones");
        assert_eq!(next_output(Some("speakers"), None), "headphones", "unknown jack: let the command decide");
        assert_eq!(next_output(Some("speakers"), Some(false)), "speakers", "nothing plugged in");
        assert_eq!(next_output(Some("headphones"), Some(true)), "speakers");
        assert_eq!(next_output(Some("headphones"), Some(false)), "speakers");
        assert_eq!(next_output(None, Some(true)), "speakers");
        assert_eq!(next_output(Some("hdmi"), Some(true)), "speakers");
    }

    #[test]
    fn output_names_other_than_the_two_are_ignored() {
        let s = state(&json!({"output": {"active": "hdmi", "jack_plugged": "yes"}}));
        assert_eq!(s["output"], json!({"active": null, "jack_plugged": null}));
    }

    #[test]
    fn repair_results_are_classified() {
        let r = |fixed: bool, changed: bool| repair_summary(&json!({"fixed": fixed, "changed": changed, "steps": ["restart wireplumber", 5, "probe"]}));
        assert_eq!(r(true, true), json!({"kind": "repaired", "steps": "restart wireplumber; probe"}));
        assert_eq!(r(true, false)["kind"], "nothing");
        assert_eq!(r(false, true)["kind"], "failed");
        assert_eq!(r(false, false)["kind"], "failed");
        assert_eq!(repair_summary(&json!(null)), json!({"kind": "failed", "steps": ""}));
    }

    #[test]
    fn dts_all_lists_what_it_did() {
        assert_eq!(dts_all_summary(&json!({"done": ["Speakers", "Headset"]})), json!({"count": 2, "text": "Speakers; Headset"}));
        assert_eq!(dts_all_summary(&json!({"done": []})), json!({"count": 0, "text": ""}));
        assert_eq!(dts_all_summary(&json!("x")), json!({"count": 0, "text": ""}));
    }

    #[test]
    fn dispatch_reaches_every_verb() {
        assert!(call("state", &full()).unwrap().is_object());
        assert_eq!(call("next_output", &json!({"active": "speakers", "jack_plugged": true})), Ok(json!("headphones")));
        assert_eq!(call("repair_summary", &json!({"fixed": true})).unwrap()["kind"], "nothing");
        assert_eq!(call("dts_all_summary", &json!({})).unwrap()["count"], 0);
        assert_eq!(call("options", &json!({})).unwrap()["dts_modes"][0], "Music");
        assert!(call("nope", &json!({})).unwrap_err().contains("nope"));
    }
}
