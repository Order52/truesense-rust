use serde_json::json;
use ts_domain::audio::{Output, VoiceDevice, VoiceMode};

use super::Fx;
use crate::api::{ApiError, dts_is_on};

#[test]
fn the_output_switch_shows_its_message_first_and_then_fails_without_a_card() {
    let fx = Fx::offline();
    let e = fx.api.set_audio_output(Output::Headphones).unwrap_err();
    assert!(matches!(e, ApiError::Domain { area: "audio", .. }), "{e}");
    let shown = fx.rig.notifier.shown();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].0, format!("Audio output: {}", Output::Headphones.label()));
    assert!(fx.api.audio_output().is_err());
    assert!(fx.api.cycle_audio_output().is_err());
}

#[test]
fn without_a_sound_system_the_readings_are_empty_not_panics() {
    let fx = Fx::offline();
    let mic = fx.api.mic_status();
    assert!(!mic.state.muted);
    assert!(mic.state.sources.is_empty());
    assert_eq!(mic.boost, None);
    assert_eq!(fx.api.mic_volume(), None);
    let json = serde_json::to_value(&mic).unwrap();
    assert_eq!(json["boost"], json!(null));
    assert_eq!(json["sources"], json!({}));
    let jack = fx.api.jack();
    assert_eq!(jack.setting, "auto");
    assert_eq!(jack.microphone_in_use, "array");
    let json = serde_json::to_value(&jack).unwrap();
    assert_eq!(json["headphones"], json!(false));
    assert_eq!(json["microphone_in_use"], json!("array"));
    assert!(json["raw"].is_object());
    let voice = fx.api.voice_status();
    assert!(voice.notify);
    assert_eq!(serde_json::to_value(&voice).unwrap()["modes"], json!({"headset": "off", "array": "off"}));
}

#[test]
fn the_microphone_volume_is_checked_before_anything_runs() {
    let fx = Fx::offline();
    assert_eq!(fx.api.set_mic_volume(151).unwrap_err().to_string(), "mic volume is 0..150 %");
    assert_eq!(fx.api.set_headset_boost(4).unwrap_err().to_string(), "headset mic boost is 0..3");
    assert_eq!(fx.api.set_mic_volume(100).unwrap(), None);
}

#[test]
fn the_jack_setting_is_written_to_the_users_configuration() {
    let fx = Fx::offline();
    fx.api.set_jack("headset").unwrap();
    assert_eq!(fx.api.jack().setting, "headset");
    assert_eq!(fx.api.jack().microphone_in_use, "headset");
    assert!(fx.user_config().contains("jack = \"headset\""));
    assert_eq!(fx.api.set_jack("speakers").unwrap_err().to_string(), "jack: auto | headphones | headset");
    assert_eq!(fx.api.jack().setting, "headset");
    fx.api.set_voice_notify(false).unwrap();
    assert!(!fx.api.voice_status().notify);
    assert!(fx.user_config().contains("jack = \"headset\""), "the other settings are kept");
}

#[test]
fn voice_modes_are_set_per_input_and_need_the_plugin() {
    let fx = Fx::offline();
    let e = fx.api.set_voice(VoiceDevice::Headset, VoiceMode::Personal).unwrap_err();
    assert!(
        e.to_string().to_lowercase().contains("rnnoise") || e.to_string().to_lowercase().contains("plugin"),
        "{e}"
    );
    assert!(fx.api.next_voice().is_err() || fx.api.voice_modes().headset.is_none());
    assert!(fx.api.set_speaker_filter(true).is_err());
    assert!(!fx.api.speaker_filter());
    assert!(!fx.api.set_speaker_filter(false).unwrap());
}

#[test]
fn dts_needs_acerultrax() {
    let fx = Fx::offline();
    assert!(fx.api.dts_status().is_err());
    assert!(fx.api.dts_info().is_err());
    assert!(fx.api.set_dts_mode("movie").is_err());
    assert!(fx.api.dts_profiles().is_err());
    assert!(fx.api.dts_everywhere(Some("movie")).is_err());
    assert!(fx.api.set_dts_power(true).is_err());
    assert!(
        fx.api.set_dts_mode("").unwrap_err().to_string().starts_with("usage: truesense audio dts mode music")
    );
}

#[test]
fn the_dts_status_text_is_read_whatever_the_column_spacing() {
    for (text, on) in [
        ("DTS:   on\nmode: Music", true),
        ("DTS: on", true),
        ("Power  DTS:\ton  ", true),
        ("DTS: off", false),
        ("DTS:on", false),
        ("DTS: online", false),
        ("xDTS: on", false),
        ("NODTS: on", false),
        ("", false),
        ("dts: on", false),
        ("DTS:", false),
    ] {
        assert_eq!(dts_is_on(text), on, "{text:?}");
    }
}

#[test]
fn the_sound_card_verdict_and_the_repair_say_when_there_is_no_card() {
    let fx = Fx::offline();
    let st = fx.api.sound_state().unwrap();
    assert_eq!(st.ok(), None);
    let json = serde_json::to_value(&st).unwrap();
    assert_eq!(json["ok"], json!(null));
    assert!(json["problem"].is_string());
}

#[test]
fn the_microphone_watcher_ends_when_asked_to() {
    let fx = Fx::offline();
    let stop = std::sync::atomic::AtomicBool::new(true);
    fx.api.watch_mic(&stop);
}
