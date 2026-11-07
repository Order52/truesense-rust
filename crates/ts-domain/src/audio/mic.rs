//! The microphone: state, mute and volume, through `pactl` and `wpctl`.

use std::collections::BTreeMap;

use serde::Serialize;

use super::parse::{is_muted, short_sources, volume_percent};
use super::{Audio, AudioError};

/// The largest microphone volume, in percent (PipeWire allows boosting up to 150 %).
const MAX_VOLUME: u32 = 150;

/// The microphone's state. The JSON names are the Python edition's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MicState {
    /// The default source's name.
    pub default: String,
    /// Whether the default source is muted.
    pub muted: bool,
    /// Every real input (by its short name) and whether it is muted.
    pub sources: BTreeMap<String, bool>,
    /// The default source's volume in percent; `null` when it cannot be read.
    pub volume: Option<u32>,
}

impl Audio {
    /// `(id, name)` of every real input; monitors of outputs are not microphones.
    pub(super) fn sources(&self) -> Vec<(String, String)> {
        short_sources(&self.out("pactl", &["list", "short", "sources"]))
    }

    pub(super) fn default_source(&self) -> String {
        self.out("pactl", &["get-default-source"]).trim().to_owned()
    }

    fn source_muted(&self, name: &str) -> bool {
        is_muted(&self.out("pactl", &["get-source-mute", name]))
    }

    /// The default microphone and every input.
    pub fn mic_state(&self) -> MicState {
        let default = self.default_source();
        let muted = self.source_muted(&default);
        let sources = self
            .sources()
            .into_iter()
            .map(|(_, name)| {
                let short = name.rsplit("HiFi__").next().unwrap_or(&name).to_owned();
                (short, self.source_muted(&name))
            })
            .collect();
        let volume = volume_percent(&self.out("pactl", &["get-source-volume", &default]));
        MicState { default, muted, sources, volume }
    }

    /// Mutes or unmutes the default microphone, or (with `all_inputs`) EVERY input: the internal
    /// array, the headset, USB and Bluetooth ones. Returns `muted`.
    pub fn mic_set_mute(&self, muted: bool, all_inputs: bool) -> bool {
        let targets: Vec<String> = if all_inputs {
            self.sources().into_iter().map(|(_, name)| name).collect()
        } else {
            vec![self.default_source()]
        };
        let flag = if muted { "1" } else { "0" };
        for name in targets {
            self.run("pactl", &["set-source-mute", &name, flag]);
        }
        muted
    }

    /// Flips the mute state of the default microphone (and of every input with `all_inputs`) and
    /// returns the new state.
    pub fn mic_toggle(&self, all_inputs: bool) -> bool {
        self.mic_set_mute(!self.mic_state().muted, all_inputs)
    }

    /// The default microphone's volume in percent; `None` when it cannot be read.
    pub fn mic_volume(&self) -> Option<u32> {
        self.mic_state().volume
    }

    /// Sets the default microphone's volume (0 to 150 %) and returns what it reads back.
    pub fn set_mic_volume(&self, percent: u32) -> Result<Option<u32>, AudioError> {
        if percent > MAX_VOLUME {
            return Err(AudioError::Invalid("mic volume is 0..150 %".into()));
        }
        self.run("wpctl", &["set-volume", "@DEFAULT_AUDIO_SOURCE@", &format!("{percent}%")]);
        Ok(self.mic_volume())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use crate::audio::testing::*;

    #[test]
    fn the_state_names_every_real_input() {
        let rig = Rig::new(false);
        rig.sys.state().extra_sources = vec!["bluez_input.AA.1".into()];
        rig.sys.state().muted.insert(SRC_HEADSET.into(), true);
        let st = rig.audio.mic_state();
        assert_eq!(st.default, SRC_ARRAY);
        assert!(!st.muted);
        assert_eq!(st.volume, Some(100));
        assert!(!st.sources["Mic1__source"]);
        assert!(st.sources["Mic2__source"]);
        assert!(!st.sources["bluez_input.AA.1"]);
        assert_eq!(st.sources.len(), 3, "the monitor of the speaker is not a microphone: {:?}", st.sources);
        let json = serde_json::to_value(&st).unwrap();
        assert_eq!(json["volume"], 100);
        assert_eq!(json["default"], SRC_ARRAY);
    }

    #[test]
    fn mute_goes_to_every_input_or_only_the_default() {
        let rig = Rig::new(false);
        rig.sys.state().extra_sources = vec!["bluez_input.AA.1".into()];
        assert!(rig.audio.mic_set_mute(true, true));
        for name in [SRC_ARRAY, SRC_HEADSET, "bluez_input.AA.1"] {
            assert_eq!(rig.sys.state().muted.get(name), Some(&true), "{name}");
        }
        assert!(!rig.audio.mic_set_mute(false, false));
        assert!(!rig.sys.state().muted[SRC_ARRAY], "only the default was unmuted");
        assert!(rig.sys.state().muted[SRC_HEADSET]);
    }

    #[test]
    fn toggle_flips_the_default_and_reports_the_new_state() {
        let rig = Rig::new(false);
        assert!(rig.audio.mic_toggle(true));
        assert!(rig.audio.mic_state().muted);
        assert!(!rig.audio.mic_toggle(true));
        assert!(!rig.audio.mic_state().muted);
        let rig = Rig::new(false);
        assert!(rig.audio.mic_toggle(false));
        assert_eq!(rig.sys.state().muted.get(SRC_ARRAY), Some(&true));
        assert_eq!(
            rig.sys.state().muted.get(SRC_HEADSET),
            None,
            "the default-only toggle leaves other inputs alone"
        );
    }

    #[test]
    fn volume_is_range_checked_and_read_back() {
        let rig = Rig::new(false);
        assert_eq!(rig.audio.mic_volume(), Some(100));
        assert_eq!(rig.audio.set_mic_volume(65).unwrap(), Some(65));
        assert!(rig.sys.did(&["wpctl", "set-volume", "@DEFAULT_AUDIO_SOURCE@", "65%"]));
        assert_eq!(rig.audio.set_mic_volume(150).unwrap(), Some(150));
        assert_eq!(rig.audio.set_mic_volume(0).unwrap(), Some(0));
        let before = rig.sys.sent().len();
        assert_eq!(rig.audio.set_mic_volume(151).unwrap_err().to_string(), "mic volume is 0..150 %");
        assert!(rig.audio.set_mic_volume(u32::MAX).is_err());
        assert_eq!(rig.sys.sent().len(), before, "a refused volume reaches no tool");
        rig.sys.state().volume = "Volume: unavailable".into();
        assert_eq!(rig.audio.mic_volume(), None);
    }
}
