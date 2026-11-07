//! `audio`: output, microphone, headset jack, Purified Voice, speaker filter and DTS.
#![forbid(unsafe_code)]

use std::sync::atomic::Ordering;

use ts_domain::audio::{Output, SoundState, VoiceDevice, VoiceMode};

use super::{Cx, Fail, Outcome, fail};
use crate::cli::argparse::Matches;

const CHOOSE: &str = "choose: output | mic | jack | voice | speaker-filter | dts   (truesense audio -h)";

pub(super) fn audio(cx: &mut Cx<'_, '_>, verb: Option<&str>, m: &Matches) -> Outcome {
    match verb {
        None => Err(fail(CHOOSE)),
        Some("output") => output(cx, m.str("action").unwrap_or("status")),
        Some("mic") => mic(cx, m),
        Some("jack") => jack(cx, m.str("use")),
        Some("voice") => return voice(cx, m),
        Some("repair") => return repair(cx),
        Some("speaker-filter") => speaker_filter(cx, m.str("action").unwrap_or("status")),
        Some("dts") => dts(cx, m),
        Some(other) => Err(fail(format!("unknown audio command {other}"))),
    }
    .map(|()| 0)
}

fn output(cx: &mut Cx<'_, '_>, action: &str) -> Result<(), Fail> {
    let api = cx.api;
    match action {
        "status" => {
            let st = api.audio_output()?;
            cx.emit(&st, None);
        }
        "toggle" => {
            let st = api.toggle_audio_output()?;
            cx.emit(&st, None);
        }
        "cycle" => {
            let said = api.cycle_audio_output()?;
            cx.emit(&said, None);
        }
        which => {
            let output: Output = which.parse().map_err(fail)?;
            let set = api.set_audio_output(output)?;
            cx.emit(set.as_str(), Some(&format!("audio output: {which}")));
        }
    }
    Ok(())
}

/// A number that is out of the range a setting takes: the setting's own words.
fn in_range(value: i64, max: u32, words: &str) -> Result<u32, Fail> {
    u32::try_from(value).ok().filter(|v| *v <= max).ok_or_else(|| fail(words))
}

fn mic(cx: &mut Cx<'_, '_>, m: &Matches) -> Result<(), Fail> {
    let api = cx.api;
    let default_only = m.flag("default_only");
    match m.str("action").unwrap_or("status") {
        "status" => {
            let st = api.mic();
            cx.emit(&st, None);
        }
        act @ ("mute" | "unmute") => {
            api.set_mic_mute(act == "mute", default_only);
            cx.emit_text(&format!("{act}d"));
        }
        "toggle" => {
            let muted = api.toggle_mic_mute(default_only);
            cx.emit(&muted, Some(if muted { "microphones: MUTED" } else { "microphones: live" }));
        }
        "volume" => {
            let now = match m.int("value") {
                None => api.mic_volume(),
                Some(v) => api.set_mic_volume(in_range(v, 150, "mic volume is 0..150 %")?)?,
            };
            let shown = now.map_or_else(|| "None".to_owned(), |v| v.to_string());
            cx.emit(&now, Some(&format!("microphone volume: {shown} %")));
        }
        _ => {
            let now = match m.int("value") {
                None => api.headset_boost(),
                Some(v) => {
                    let level = in_range(v, 3, "headset mic boost is 0..3")?;
                    Some(api.set_headset_boost(u8::try_from(level).unwrap_or(3))?)
                }
            };
            let shown = now.map_or_else(|| "None".to_owned(), |v| v.to_string());
            cx.emit(&now, Some(&format!("headset mic boost: {shown} (0..3, +10 dB per step)")));
        }
    }
    Ok(())
}

fn jack(cx: &mut Cx<'_, '_>, use_: Option<&str>) -> Result<(), Fail> {
    match use_ {
        Some(use_) => {
            cx.api.set_jack(use_)?;
            let note = if use_ == "headset" {
                "  (headset microphone is used even if the codec does not detect it)"
            } else {
                ""
            };
            cx.emit(use_, Some(&format!("3.5 mm jack: {use_}{note}")));
        }
        None => {
            let report = cx.api.jack();
            cx.emit(&report, None);
        }
    }
    Ok(())
}

fn voice(cx: &mut Cx<'_, '_>, m: &Matches) -> Outcome {
    let api = cx.api;
    match m.str("action").unwrap_or("status") {
        "status" => {
            let st = api.voice_status();
            cx.emit(&st, None);
        }
        "set" => {
            let (Some(a), Some(b)) =
                (m.str("a").filter(|s| !s.is_empty()), m.str("b").filter(|s| !s.is_empty()))
            else {
                return Err(fail("usage: truesense audio voice set headset|array off|conference|personal"));
            };
            let device: VoiceDevice = a.parse().map_err(fail)?;
            let mode: VoiceMode = b.parse().map_err(fail)?;
            let got = api.set_voice(device, mode)?;
            cx.emit(got.as_str(), Some(&format!("Purified Voice ({a}): {b}")));
        }
        word @ ("enable" | "disable") => {
            let modes = api.enable_voice(word == "enable")?;
            cx.emit(&modes, None);
        }
        "next" => {
            let (device, mode) = api.next_voice()?;
            cx.emit_text(&format!("Purified Voice ({device}): {mode}"));
        }
        "apply" => {
            let policy = api.apply_mic_policy();
            cx.emit(&policy, None);
        }
        "watch" => {
            let stop = cx.launcher.interrupt();
            api.watch_mic(&stop);
            if stop.load(Ordering::SeqCst) {
                return Ok(130);
            }
        }
        _ => {
            let on = match m.str("a") {
                Some("on") => true,
                Some("off") => false,
                _ => return Err(fail("usage: truesense audio voice notify on|off")),
            };
            api.set_voice_notify(on)?;
            cx.emit_text(&format!("mic mode notification: {}", if on { "on" } else { "off" }));
        }
    }
    Ok(0)
}

fn repair(cx: &mut Cx<'_, '_>) -> Outcome {
    let api = cx.api;
    let state = api.sound_state()?;
    match &state {
        SoundState::NoCard { problem } => {
            cx.emit(&state, Some(&format!("no sound card found in PipeWire: {problem}")));
        }
        SoundState::Ok { sinks, .. } => {
            cx.emit(
                &state,
                Some(&format!("the sound output is there ({}): nothing to repair", sinks.join(", "))),
            );
        }
        SoundState::Broken { problem, .. } => {
            cx.say(&format!("problem: {problem}\nrepairing (restarting WirePlumber if needed)..."));
            let r = api.repair_sound()?;
            let mut text =
                format!("{}{}", if r.fixed { "FIXED: " } else { "NOT FIXED: " }, r.steps.join("; "));
            if !r.fixed {
                text.push_str("\nCheck the kernel log (journalctl -k -b | grep -i sof) and try a reboot.");
            }
            cx.emit(&r, Some(&text));
            return Ok(i32::from(!r.fixed));
        }
    }
    Ok(0)
}

fn speaker_filter(cx: &mut Cx<'_, '_>, action: &str) -> Result<(), Fail> {
    let api = cx.api;
    if action == "status" {
        let on = api.speaker_filter();
        cx.emit(&on, Some(&format!("speaker filter: {}", ts_api::api::on_off(on))));
    } else {
        api.set_speaker_filter(action == "on")?;
        cx.emit_text(&format!("speaker filter: {action}"));
    }
    Ok(())
}

fn dts(cx: &mut Cx<'_, '_>, m: &Matches) -> Result<(), Fail> {
    let api = cx.api;
    let name = m.str("name").filter(|n| !n.is_empty());
    match m.str("action").unwrap_or("status") {
        "status" => {
            let text = api.dts_status()?;
            cx.say(&text);
        }
        word @ ("on" | "off") => {
            let text = api.set_dts_power(word == "on")?;
            cx.say(&text);
        }
        "mode" => {
            let text = api.set_dts_mode(name.unwrap_or_default())?;
            cx.say(&text);
        }
        "profiles" => {
            let text = api.dts_profiles()?;
            cx.say(&text);
        }
        "devices" => {
            for d in api.dts_devices()? {
                let mark = if d.current { '*' } else { ' ' };
                cx.say(&format!("{mark} {:<44} {:<18} {}", d.label, d.kind, d.profile));
            }
        }
        "all" => {
            let done = api.dts_everywhere(name)?;
            cx.say(&if done.is_empty() { "DTS is on for every output".to_owned() } else { done.join("\n") });
        }
        _ => {
            let text = api.services().audio.dts(&["setup"]).map_err(fail)?;
            cx.say(&text);
        }
    }
    Ok(())
}
