//! Sound: the output (speakers or headphones), the microphone, the headset jack, Purified Voice
//! and the DTS sound.
//!
//! Nothing here ever writes the HDA codec (`docs/SAFETY.md`): the audio layer refuses to run a
//! command that mentions it, whatever is asked for here.
#![forbid(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde::Serialize;
use ts_domain::audio::DtsDevice;
use ts_domain::audio::events::Subscription;
use ts_domain::audio::{
    DtsScope, Jack, MicPolicy, MicState, MicWatcher, Output, OutputStatus, RepairOptions, RepairResult,
    SoundState, VoiceDevice, VoiceMode, VoiceModes,
};
use ts_foundation::config::{Value, section_table};

use super::{Api, ApiError, ApiResult};

/// The microphone state plus the headset microphone boost (`null` without the control).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MicReport {
    /// The default microphone and every input.
    #[serde(flatten)]
    pub state: MicState,
    /// The headset microphone boost, 0..=3 (+10 dB per step).
    pub boost: Option<u8>,
}

/// What the codec sees on the 3.5 mm jack, how the jack is configured to be used and which
/// microphone that makes the one in use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JackReport {
    /// What the codec sees.
    #[serde(flatten)]
    pub jack: Jack,
    /// `auto`, `headphones` or `headset`.
    pub setting: String,
    /// `headset` or `array`.
    pub microphone_in_use: String,
}

/// Purified Voice: the mode of each input, whether a headset microphone is detected, the default
/// microphone and whether mode changes are announced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VoiceStatus {
    /// The mode of each input.
    pub modes: VoiceModes,
    /// A headset microphone is detected.
    pub jack: bool,
    /// The default microphone's name.
    pub default_source: String,
    /// Whether mode changes are announced on screen.
    pub notify: bool,
}

/// The DTS sound at a glance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DtsInfo {
    /// DTS is on.
    pub on: bool,
    /// The profile of the output playing.
    pub mode: Option<String>,
    /// Every output and its profile.
    pub devices: Vec<DtsDevice>,
}

/// Whether AcerUltraX's status text says DTS is on, whatever the column spacing (`DTS:   on`).
pub fn dts_is_on(status: &str) -> bool {
    status.lines().any(|line| {
        line.match_indices("DTS:").any(|(at, _)| {
            let before_ok = line[..at].chars().next_back().is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
            let rest = &line[at + 4..];
            let trimmed = rest.trim_start();
            let spaced = trimmed.len() < rest.len();
            before_ok
                && spaced
                && trimmed.strip_prefix("on").is_some_and(|after| {
                    after.chars().next().is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
                })
        })
    })
}

impl Api {
    /// Which output profile is active (speakers or headphones) and whether something is plugged
    /// into the jack.
    pub fn audio_output(&self) -> ApiResult<OutputStatus> {
        Ok(self.s.audio.output_status()?)
    }

    /// Switches between the speakers and the headphones. The on-screen message comes first: the
    /// switch takes about a second.
    pub fn set_audio_output(&self, which: Output) -> ApiResult<Output> {
        self.s.notifier.show(&format!("Audio output: {}", which.label()), which.icon());
        Ok(self.s.audio.set_output(which)?)
    }

    /// Flips between the speakers and the headphones and answers the output now active.
    pub fn toggle_audio_output(&self) -> ApiResult<OutputStatus> {
        self.s.audio.output_toggle()?;
        self.audio_output()
    }

    /// The F5 round robin over every output; answers the new one's label.
    pub fn cycle_audio_output(&self) -> ApiResult<String> {
        Ok(self.s.audio.output_cycle()?)
    }

    /// The default microphone and every input.
    pub fn mic(&self) -> MicState {
        self.s.audio.mic_state()
    }

    /// Mutes or unmutes every input, or only the default one.
    pub fn set_mic_mute(&self, muted: bool, default_only: bool) {
        self.s.audio.mic_set_mute(muted, !default_only);
    }

    /// Flips the mute state (of every input unless `default_only`) and answers the new state.
    pub fn toggle_mic_mute(&self, default_only: bool) -> bool {
        self.s.audio.mic_toggle(!default_only)
    }

    /// The default microphone's volume in percent; `None` when unreadable.
    pub fn mic_volume(&self) -> Option<u32> {
        self.s.audio.mic_volume()
    }

    /// Sets the default microphone's volume (0..=150 %) and answers what it reads back.
    pub fn set_mic_volume(&self, percent: u32) -> ApiResult<Option<u32>> {
        Ok(self.s.audio.set_mic_volume(percent)?)
    }

    /// The headset microphone boost (0..=3, +10 dB per step); `None` when the card has no such control.
    pub fn headset_boost(&self) -> Option<u8> {
        self.s.audio.headset_boost()
    }

    /// Sets the boost and reads it back; a value that did not stick is an error.
    pub fn set_headset_boost(&self, level: u8) -> ApiResult<u8> {
        Ok(self.s.audio.set_headset_boost(level)?)
    }

    /// The microphones and the boost.
    pub fn mic_status(&self) -> MicReport {
        MicReport { state: self.mic(), boost: self.headset_boost() }
    }

    /// The jack controls and the configured use.
    pub fn jack(&self) -> JackReport {
        let jack = self.s.audio.jack_status();
        let use_ = self.s.audio.jack_use(Some(&jack));
        JackReport { jack, setting: self.cfg().audio.jack, microphone_in_use: use_.as_str().to_owned() }
    }

    /// Chooses how the jack is used (`auto`, `headphones` or `headset`: always use the headset
    /// microphone) and points the default microphone accordingly.
    pub fn set_jack(&self, use_: &str) -> ApiResult<()> {
        if !matches!(use_, "auto" | "headphones" | "headset") {
            return Err(ApiError::invalid("jack: auto | headphones | headset"));
        }
        self.s.config.write_user(section_table("audio", [("jack", Value::String(use_.to_owned()))]))?;
        self.s.audio.apply_mic_policy(false);
        Ok(())
    }

    /// Reads Purified Voice.
    pub fn voice_status(&self) -> VoiceStatus {
        VoiceStatus {
            modes: self.s.audio.voice_modes(),
            jack: self.s.audio.jack_status().headset_mic,
            default_source: self.s.audio.mic_state().default,
            notify: self.cfg().audio.notify,
        }
    }

    /// Each input's noise-reduction mode.
    pub fn voice_modes(&self) -> VoiceModes {
        self.s.audio.voice_modes()
    }

    /// Sets one input to off, conference or personal and points the default microphone at the
    /// right input.
    pub fn set_voice(&self, device: VoiceDevice, mode: VoiceMode) -> ApiResult<VoiceMode> {
        let got = self.s.audio.voice_set(device, mode)?;
        self.s.audio.apply_mic_policy(false);
        Ok(got)
    }

    /// Restores the configured modes of both inputs (`on`) or leaves both unfiltered (`off`).
    pub fn enable_voice(&self, on: bool) -> ApiResult<VoiceModes> {
        let modes = self.s.audio.voice_enable(on)?;
        self.s.audio.apply_mic_policy(false);
        Ok(modes)
    }

    /// Cycles the mode of the input in use (off, conference, personal) and answers the input and
    /// its new mode.
    pub fn next_voice(&self) -> ApiResult<(VoiceDevice, VoiceMode)> {
        let answer = self.s.audio.voice_next(None)?;
        self.s.audio.apply_mic_policy(false);
        Ok(answer)
    }

    /// Points the default microphone at the right input, announcing it.
    pub fn apply_mic_policy(&self) -> MicPolicy {
        self.s.audio.apply_mic_policy(true)
    }

    /// Switches the on-screen message of microphone mode changes.
    pub fn set_voice_notify(&self, on: bool) -> ApiResult<()> {
        self.s.config.write_user(section_table("audio", [("notify", Value::Boolean(on))]))
    }

    /// Judges whether the sound card has an output.
    pub fn sound_state(&self) -> ApiResult<SoundState> {
        Ok(self.s.audio.sound_state()?)
    }

    /// Brings the sound output back (restarting WirePlumber if needed) and verifies it.
    pub fn repair_sound(&self) -> ApiResult<RepairResult> {
        Ok(self.s.audio.repair(RepairOptions::default())?)
    }

    /// Whether the speaker noise filter is configured.
    pub fn speaker_filter(&self) -> bool {
        self.s.audio.speaker_filter_on()
    }

    /// Switches the noise filter on the output (off by default: it also gates music) and answers
    /// its new state.
    pub fn set_speaker_filter(&self, on: bool) -> ApiResult<bool> {
        Ok(self.s.audio.speaker_filter(on, VoiceMode::Conference)?)
    }

    /// Keeps the default microphone on the right input until `stop` is set: it picks it again
    /// whenever the headset is plugged or a filter changes, and gives new outputs DTS once.
    pub fn watch_mic(&self, stop: &AtomicBool) {
        let mut watcher = MicWatcher::new(Arc::clone(&self.s.audio));
        // One resident `pactl subscribe` replaces the questions the loop used to ask every
        // second; it ends with the subscription when the watch does.
        let subscription = self.s.audio.event_source().map(|source| Arc::new(Subscription::start(source)));
        if let Some(subscription) = &subscription {
            watcher = watcher.with_events(subscription.clone());
        }
        watcher.run(stop);
    }

    /// AcerUltraX's own status text.
    pub fn dts_status(&self) -> ApiResult<String> {
        Ok(self.s.audio.dts_status()?)
    }

    /// Switches DTS on or off and answers AcerUltraX's message.
    pub fn set_dts_power(&self, on: bool) -> ApiResult<String> {
        Ok(self.s.audio.dts_power(on)?)
    }

    /// Picks a DTS profile (`music`, `movie`, `voice`, `game1..3`, `off`, `plain`, `custom audio`)
    /// for every output, or only the one playing when `[audio] dts_all` is off.
    pub fn set_dts_mode(&self, name: &str) -> ApiResult<String> {
        if name.is_empty() {
            return Err(ApiError::invalid(
                "usage: truesense audio dts mode music|movie|voice|game1|game2|game3|off|plain|\"custom audio\"   (every output; [audio] dts_all = false: only the one playing)",
            ));
        }
        Ok(self.s.audio.dts_mode(name, DtsScope::Configured)?)
    }

    /// The sound profiles AcerUltraX knows, in its own words.
    pub fn dts_profiles(&self) -> ApiResult<String> {
        Ok(self.s.audio.dts(&["profiles"])?)
    }

    /// Every output AcerUltraX knows and its profile.
    pub fn dts_devices(&self) -> ApiResult<Vec<DtsDevice>> {
        Ok(self.s.audio.dts_devices()?)
    }

    /// Gives every output DTS now (each handled once) and, with a mode, sets it on every output.
    /// Answers what changed.
    pub fn dts_everywhere(&self, mode: Option<&str>) -> ApiResult<Vec<String>> {
        let mut done = self.s.audio.dts_cover(true)?;
        if let Some(mode) = mode.filter(|m| !m.is_empty()) {
            self.s.audio.dts_mode(mode, DtsScope::Everywhere)?;
            done.push(format!("every output: {mode}"));
        }
        Ok(done)
    }

    /// DTS at a glance: whether it is on, the profile of the output playing, every output.
    pub fn dts_info(&self) -> ApiResult<DtsInfo> {
        let status = self.dts_status()?;
        let devices = self.dts_devices()?;
        let mode = devices.iter().find(|d| d.current).map(|d| d.profile.clone());
        Ok(DtsInfo { on: dts_is_on(&status), mode, devices })
    }
}
