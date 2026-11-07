//! Purified Voice, per input device, and the default-microphone policy that follows the jack.
//!
//! Configuring one input restarts PipeWire once (at configuration time only); plugging and
//! unplugging a headset never does, because the filter chains of both inputs always exist while
//! they are configured and only the *default source* moves between them.

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use serde::{Serialize, Serializer};
use serde_json::json;
use ts_foundation::fsx;

use super::parse::{first_fields, short_column, voice_threshold};
use super::{Audio, AudioError, CARD_MATCH};

/// How long to wait for a filter node to appear or vanish after a PipeWire reload.
const NODE_WAIT: Duration = Duration::from_secs(8);
const NODE_POLL: Duration = Duration::from_millis(200);

/// An input a filter chain can be put in front of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum VoiceDevice {
    /// The headset microphone on the 3.5 mm jack.
    Headset,
    /// The laptop's microphone array.
    Array,
}

impl VoiceDevice {
    /// Both inputs.
    pub const ALL: [VoiceDevice; 2] = [VoiceDevice::Headset, VoiceDevice::Array];

    /// The name used in JSON, in file names and on the command line.
    pub fn as_str(self) -> &'static str {
        match self {
            VoiceDevice::Headset => "headset",
            VoiceDevice::Array => "array",
        }
    }

    /// The PipeWire source suffix of the ALSA input.
    pub(super) fn source_suffix(self) -> &'static str {
        match self {
            VoiceDevice::Headset => "Mic2__source",
            VoiceDevice::Array => "Mic1__source",
        }
    }

    fn description(self) -> &'static str {
        match self {
            VoiceDevice::Headset => "Purified Voice (headset mic)",
            VoiceDevice::Array => "Purified Voice (mic array)",
        }
    }

    /// The raw ALSA source the filter reads.
    pub(super) fn source(self) -> String {
        format!("alsa_input.{CARD_MATCH}.HiFi__{}", self.source_suffix())
    }

    /// The virtual source the filter creates.
    pub(super) fn node(self) -> String {
        format!("truesense_pv_{}", self.as_str())
    }
}

impl fmt::Display for VoiceDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for VoiceDevice {
    type Err = AudioError;

    fn from_str(s: &str) -> Result<Self, AudioError> {
        match s {
            "headset" => Ok(VoiceDevice::Headset),
            "array" => Ok(VoiceDevice::Array),
            _ => Err(AudioError::Invalid("device: headset | array".into())),
        }
    }
}

/// How strictly the RNNoise voice-activity gate of Purified Voice works.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceMode {
    /// No filter.
    Off,
    /// Gentle.
    Conference,
    /// Strict; can clip very soft words.
    Personal,
}

impl VoiceMode {
    /// The name used in JSON and on the command line.
    pub fn as_str(self) -> &'static str {
        match self {
            VoiceMode::Off => "off",
            VoiceMode::Conference => "conference",
            VoiceMode::Personal => "personal",
        }
    }

    /// The name Acer's Purified Voice modes have in the OSD.
    pub fn label(self) -> &'static str {
        match self {
            VoiceMode::Off => "All Off",
            VoiceMode::Personal => "Personal Call",
            VoiceMode::Conference => "Conference Call",
        }
    }

    /// The voice-activity gate: threshold in percent, grace in ms, retroactive grace in ms.
    /// `None` for [`VoiceMode::Off`].
    pub fn gate(self) -> Option<(u32, u32, u32)> {
        match self {
            VoiceMode::Off => None,
            VoiceMode::Conference => Some((50, 20, 0)),
            VoiceMode::Personal => Some((85, 150, 100)),
        }
    }

    /// The mode `voice next` cycles to: off, conference, personal, off.
    pub fn next(self) -> VoiceMode {
        match self {
            VoiceMode::Off => VoiceMode::Conference,
            VoiceMode::Conference => VoiceMode::Personal,
            VoiceMode::Personal => VoiceMode::Off,
        }
    }
}

impl fmt::Display for VoiceMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for VoiceMode {
    type Err = AudioError;

    fn from_str(s: &str) -> Result<Self, AudioError> {
        match s {
            "off" => Ok(VoiceMode::Off),
            "conference" => Ok(VoiceMode::Conference),
            "personal" => Ok(VoiceMode::Personal),
            _ => Err(AudioError::Invalid("mode: off | conference | personal".into())),
        }
    }
}

/// The name of a mode read back from a file; `?` for a file this program did not write.
fn mode_name(mode: Option<VoiceMode>) -> &'static str {
    mode.map_or("?", VoiceMode::as_str)
}

/// Each input's mode as read back from its configuration file; `None` is a file this program did
/// not write (shown as `?`). Serialised as `{"headset": "off", "array": "conference"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceModes {
    /// The headset input.
    pub headset: Option<VoiceMode>,
    /// The microphone array.
    pub array: Option<VoiceMode>,
}

impl VoiceModes {
    /// The mode of one input.
    pub fn get(&self, device: VoiceDevice) -> Option<VoiceMode> {
        match device {
            VoiceDevice::Headset => self.headset,
            VoiceDevice::Array => self.array,
        }
    }
}

impl Serialize for VoiceModes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        json!({"headset": mode_name(self.headset), "array": mode_name(self.array)}).serialize(serializer)
    }
}

/// What [`Audio::apply_mic_policy`] chose. The JSON names are the Python edition's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicPolicy {
    /// The input in use.
    pub device: VoiceDevice,
    /// Its mode; `None` when its file was not written by this program (shown as `?`).
    pub mode: Option<VoiceMode>,
    /// The source that is now the default.
    pub default_source: String,
}

impl Serialize for MicPolicy {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        json!({"device": self.device.as_str(), "mode": mode_name(self.mode), "default_source": self.default_source})
            .serialize(serializer)
    }
}

/// The PipeWire config of one input's filter chain. It is byte-for-byte what the older editions
/// wrote: a differing file would make the next `voice set` see a change and restart PipeWire once
/// for nothing. `None` for [`VoiceMode::Off`], which has no file.
pub(crate) fn voice_conf(device: VoiceDevice, mode: VoiceMode, plugin: &str) -> Option<String> {
    let (threshold, grace, retro) = mode.gate()?;
    Some(
        r#"context.modules = [
  { name = libpipewire-module-filter-chain
    args = {
      node.description = "@DESC@"
      media.name = "@DESC@"
      filter.graph = {
        nodes = [ { type = ladspa name = rnnoise plugin = "@PLUGIN@" label = noise_suppressor_stereo
                    control = { "VAD Threshold (%)" = @THR@ "VAD Grace Period (ms)" = @GRACE@ "Retroactive VAD Grace (ms)" = @RETRO@ } } ]
      }
      capture.props = {
                node.passive = true
                target.object = "@SOURCE@"
      }
      playback.props = {
                media.class = Audio/Source
                node.name = "@NODE@"
                node.description = "@DESC@"
                audio.position = [ FL FR ]
      }
    }
  }
]
"#
        .replace("@DESC@", device.description())
        .replace("@SOURCE@", &device.source())
        .replace("@NODE@", &device.node())
        .replace("@PLUGIN@", plugin)
        .replace("@THR@", &threshold.to_string())
        .replace("@GRACE@", &grace.to_string())
        .replace("@RETRO@", &retro.to_string()),
    )
}

/// The speaker-side filter's config (off by default: it also gates music).
fn speaker_conf(mode: VoiceMode, plugin: &str) -> Option<String> {
    let (threshold, grace, retro) = mode.gate()?;
    Some(
        r#"context.modules = [
  { name = libpipewire-module-filter-chain
    args = {
      node.description = "Purified Voice Speaker"
      filter.graph = { nodes = [ { type = ladspa name = rnnoise plugin = "@PLUGIN@" label = noise_suppressor_stereo
                    control = { "VAD Threshold (%)" = @THR@ "VAD Grace Period (ms)" = @GRACE@ "Retroactive VAD Grace (ms)" = @RETRO@ } } ] }
      capture.props = { media.class = Audio/Sink node.name = "truesense_pv_speaker" node.description = "Purified Voice Speaker" audio.position = [ FL FR ] }
      playback.props = { node.passive = true }
    }
  }
]
"#
        .replace("@PLUGIN@", plugin)
        .replace("@THR@", &threshold.to_string())
        .replace("@GRACE@", &grace.to_string())
        .replace("@RETRO@", &retro.to_string()),
    )
}

impl Audio {
    fn conf_path(&self, device: VoiceDevice) -> PathBuf {
        self.paths.conf_dir.join(format!("truesense-pv-{}.conf", device.as_str()))
    }

    fn speaker_conf_path(&self) -> PathBuf {
        self.paths.conf_dir.join("truesense-pv-speaker.conf")
    }

    fn plugin(&self, speaker: bool) -> Result<String, AudioError> {
        self.paths
            .plugin
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .ok_or(AudioError::PluginMissing { speaker })
    }

    /// Restarts PipeWire so that it reads its drop-in configuration again.
    fn reload(&self) {
        self.run("systemctl", &["--user", "restart", "pipewire.service"]);
    }

    pub(crate) fn source_names(&self) -> Vec<String> {
        short_column(&self.out("pactl", &["list", "short", "sources"]), 1)
    }

    fn node_exists(&self, name: &str) -> bool {
        self.source_names().iter().any(|n| n == name)
    }

    /// After a PipeWire reload the filter node takes a moment to appear (or vanish). `true` when it
    /// did within the time allowed.
    fn wait_node(&self, device: VoiceDevice, present: bool) -> bool {
        let end = self.clock.now() + NODE_WAIT;
        while self.clock.now() < end {
            if self.node_exists(&device.node()) == present {
                return true;
            }
            self.clock.sleep(NODE_POLL);
        }
        false
    }

    /// Configures one input's filter to off, conference or personal, and returns the mode. It
    /// restarts PipeWire only when the configuration actually changes (once, at configuration time).
    pub fn voice_set(&self, device: VoiceDevice, mode: VoiceMode) -> Result<VoiceMode, AudioError> {
        let path = self.conf_path(device);
        if mode == VoiceMode::Off {
            if path.exists() {
                std::fs::remove_file(&path).map_err(|source| fsx::FsError::Io {
                    op: "remove",
                    path: path.clone(),
                    source,
                })?;
                self.reload();
                self.wait_node(device, false);
            }
            return Ok(VoiceMode::Off);
        }
        let plugin = self.plugin(false)?;
        let wanted = voice_conf(device, mode, &plugin).unwrap_or_default();
        if std::fs::read_to_string(&path).ok().as_deref() != Some(wanted.as_str()) {
            fsx::write_atomic_mkdir(&path, wanted.as_bytes(), 0o644)?;
            self.reload();
            self.wait_node(device, true);
        }
        Ok(mode)
    }

    /// Each input's mode, read back from its configuration file.
    pub fn voice_modes(&self) -> VoiceModes {
        let read = |device: VoiceDevice| -> Option<VoiceMode> {
            let Ok(text) = std::fs::read_to_string(self.conf_path(device)) else {
                return Some(VoiceMode::Off);
            };
            let threshold = voice_threshold(&text);
            [VoiceMode::Conference, VoiceMode::Personal]
                .into_iter()
                .find(|m| m.gate().map(|g| g.0) == threshold)
        };
        VoiceModes { headset: read(VoiceDevice::Headset), array: read(VoiceDevice::Array) }
    }

    /// With `on`, restores the configured modes (`[audio] headset_mode` / `array_mode`); with
    /// `off`, leaves both inputs unfiltered. Both configured modes are checked before anything is
    /// changed.
    pub fn voice_enable(&self, on: bool) -> Result<VoiceModes, AudioError> {
        let audio = self.cfg().audio;
        let (headset, array) = if on {
            (audio.headset_mode.parse::<VoiceMode>()?, audio.array_mode.parse::<VoiceMode>()?)
        } else {
            (VoiceMode::Off, VoiceMode::Off)
        };
        self.voice_set(VoiceDevice::Headset, headset)?;
        self.voice_set(VoiceDevice::Array, array)?;
        Ok(self.voice_modes())
    }

    /// Cycles one input's mode (the one in use when `device` is `None`): off, conference, personal,
    /// off. Returns the input and its new mode.
    pub fn voice_next(&self, device: Option<VoiceDevice>) -> Result<(VoiceDevice, VoiceMode), AudioError> {
        let device = device.unwrap_or_else(|| {
            if self.jack_status().headset_mic { VoiceDevice::Headset } else { VoiceDevice::Array }
        });
        // A file this program did not write ("?") starts the cycle again at conference.
        let next = self.voice_modes().get(device).map_or(VoiceMode::Conference, VoiceMode::next);
        self.voice_set(device, next)?;
        Ok((device, next))
    }

    /// Points the default microphone at the right input: headset plugged in gives the headset
    /// filter (or the raw headset mic), else the array filter (or the raw array). Streams that are
    /// already recording are moved along.
    pub fn apply_mic_policy(&self, notify: bool) -> MicPolicy {
        let device = self.jack_use(None);
        let mode = self.voice_modes().get(device);
        let want = if mode != Some(VoiceMode::Off) && self.node_exists(&device.node()) {
            device.node()
        } else {
            device.source()
        };
        if self.default_source() != want {
            self.run("pactl", &["set-default-source", &want]);
            for id in first_fields(&self.out("pactl", &["list", "short", "source-outputs"])) {
                self.run("pactl", &["move-source-output", &id, &want]);
            }
        }
        if notify {
            let place = if device == VoiceDevice::Headset { "Headset" } else { "Mic array" };
            let label = mode.map_or("?", VoiceMode::label);
            self.say(&format!("Microphone: {place} - {label}"), "mic");
        }
        MicPolicy { device, mode, default_source: want }
    }

    /// Switches the speaker-side noise filter (off by default: it also gates music; meant for calls
    /// on the speakers) and returns its new state. `mode` must be conference or personal when
    /// switching it on.
    pub fn speaker_filter(&self, on: bool, mode: VoiceMode) -> Result<bool, AudioError> {
        let path = self.speaker_conf_path();
        if !on {
            if path.exists() {
                std::fs::remove_file(&path).map_err(|source| fsx::FsError::Io {
                    op: "remove",
                    path: path.clone(),
                    source,
                })?;
                self.reload();
            }
            return Ok(false);
        }
        let plugin = self.plugin(true)?;
        let conf = speaker_conf(mode, &plugin)
            .ok_or_else(|| AudioError::Invalid("mode: conference | personal".into()))?;
        fsx::write_atomic_mkdir(&path, conf.as_bytes(), 0o644)?;
        self.reload();
        Ok(true)
    }

    /// Whether the speaker noise filter is configured.
    pub fn speaker_filter_on(&self) -> bool {
        self.speaker_conf_path().exists()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::audio::testing::*;

    const PLUGIN: &str = "/usr/lib/ladspa/librnnoise_ladspa.so";

    #[test]
    fn names_and_parsing() {
        for (text, device) in [("headset", VoiceDevice::Headset), ("array", VoiceDevice::Array)] {
            assert_eq!(text.parse::<VoiceDevice>().unwrap(), device);
            assert_eq!(device.to_string(), text);
        }
        for (text, mode) in [
            ("off", VoiceMode::Off),
            ("conference", VoiceMode::Conference),
            ("personal", VoiceMode::Personal),
        ] {
            assert_eq!(text.parse::<VoiceMode>().unwrap(), mode);
            assert_eq!(mode.to_string(), text);
        }
        for bad in ["", "Headset", "hwC0D0/reconfig", "speaker"] {
            assert_eq!(
                bad.parse::<VoiceDevice>().unwrap_err().to_string(),
                "device: headset | array",
                "{bad:?}"
            );
        }
        for bad in ["", "strict", "Off", "../x"] {
            assert_eq!(
                bad.parse::<VoiceMode>().unwrap_err().to_string(),
                "mode: off | conference | personal",
                "{bad:?}"
            );
        }
        assert_eq!(
            [VoiceMode::Off, VoiceMode::Conference, VoiceMode::Personal].map(VoiceMode::label),
            ["All Off", "Conference Call", "Personal Call"]
        );
        assert_eq!(VoiceMode::Conference.gate(), Some((50, 20, 0)));
        assert_eq!(VoiceMode::Personal.gate(), Some((85, 150, 100)));
        assert_eq!(VoiceMode::Off.gate(), None);
        assert_eq!(VoiceMode::Personal.next(), VoiceMode::Off);
        assert_eq!(
            VoiceMode::Personal.next().next().next(),
            VoiceMode::Personal,
            "three steps are a full cycle"
        );
    }

    #[test]
    fn the_config_is_byte_for_byte_what_the_older_editions_wrote() {
        let expected = "context.modules = [\n  { name = libpipewire-module-filter-chain\n    args = {\n      node.description = \"Purified Voice (headset mic)\"\n      media.name = \"Purified Voice (headset mic)\"\n      filter.graph = {\n        nodes = [ { type = ladspa name = rnnoise plugin = \"/usr/lib/ladspa/librnnoise_ladspa.so\" label = noise_suppressor_stereo\n                    control = { \"VAD Threshold (%)\" = 50 \"VAD Grace Period (ms)\" = 20 \"Retroactive VAD Grace (ms)\" = 0 } } ]\n      }\n      capture.props = {\n                node.passive = true\n                target.object = \"alsa_input.pci-0000_00_1f.3-platform-skl_hda_dsp_generic.HiFi__Mic2__source\"\n      }\n      playback.props = {\n                media.class = Audio/Source\n                node.name = \"truesense_pv_headset\"\n                node.description = \"Purified Voice (headset mic)\"\n                audio.position = [ FL FR ]\n      }\n    }\n  }\n]\n";
        assert_eq!(voice_conf(VoiceDevice::Headset, VoiceMode::Conference, PLUGIN).unwrap(), expected);
        let array = voice_conf(VoiceDevice::Array, VoiceMode::Personal, PLUGIN).unwrap();
        assert!(array.contains(
            "\"VAD Threshold (%)\" = 85 \"VAD Grace Period (ms)\" = 150 \"Retroactive VAD Grace (ms)\" = 100"
        ));
        assert!(array.contains(
            "target.object = \"alsa_input.pci-0000_00_1f.3-platform-skl_hda_dsp_generic.HiFi__Mic1__source\""
        ));
        assert!(
            array.contains("node.name = \"truesense_pv_array\"")
                && array.contains("Purified Voice (mic array)")
        );
        assert!(voice_conf(VoiceDevice::Array, VoiceMode::Off, PLUGIN).is_none());
        let speaker = speaker_conf(VoiceMode::Conference, PLUGIN).unwrap();
        assert!(speaker.starts_with("context.modules = [\n  { name = libpipewire-module-filter-chain\n    args = {\n      node.description = \"Purified Voice Speaker\"\n"));
        assert!(speaker.contains("media.class = Audio/Sink node.name = \"truesense_pv_speaker\""));
        assert!(speaker.ends_with("playback.props = { node.passive = true }\n    }\n  }\n]\n"));
        assert!(!speaker.contains('@') && !array.contains('@'), "every placeholder was filled");
    }

    #[test]
    fn voice_set_writes_the_config_reloads_once_and_waits_for_the_node() {
        let rig = Rig::new(false);
        assert_eq!(
            rig.audio.voice_set(VoiceDevice::Headset, VoiceMode::Conference).unwrap(),
            VoiceMode::Conference
        );
        let text = rig.read("pw/truesense-pv-headset.conf").unwrap();
        assert_eq!(text, voice_conf(VoiceDevice::Headset, VoiceMode::Conference, PLUGIN).unwrap());
        assert_eq!(rig.sys.count(&["systemctl", "--user", "restart", "pipewire.service"]), 1);
        assert_eq!(
            rig.audio.voice_modes(),
            VoiceModes { headset: Some(VoiceMode::Conference), array: Some(VoiceMode::Off) }
        );

        // The same mode again restarts nothing.
        rig.sys.clear();
        rig.audio.voice_set(VoiceDevice::Headset, VoiceMode::Conference).unwrap();
        assert_eq!(rig.sys.count(&["systemctl"]), 0, "an unchanged config must not restart PipeWire");

        // Another mode rewrites and restarts once.
        rig.audio.voice_set(VoiceDevice::Headset, VoiceMode::Personal).unwrap();
        assert_eq!(rig.sys.count(&["systemctl"]), 1);
        assert_eq!(rig.audio.voice_modes().headset, Some(VoiceMode::Personal));

        // Off removes the file and reloads, but only when there was one.
        rig.sys.clear();
        assert_eq!(rig.audio.voice_set(VoiceDevice::Headset, VoiceMode::Off).unwrap(), VoiceMode::Off);
        assert!(rig.read("pw/truesense-pv-headset.conf").is_none());
        assert_eq!(rig.sys.count(&["systemctl"]), 1);
        rig.audio.voice_set(VoiceDevice::Headset, VoiceMode::Off).unwrap();
        assert_eq!(rig.sys.count(&["systemctl"]), 1, "nothing to remove, nothing to restart");
    }

    #[test]
    fn a_node_that_never_appears_is_waited_for_eight_seconds_and_no_longer() {
        let rig = Rig::new(false);
        rig.sys.state().conf_dir = None; // the fake PipeWire never creates the node
        rig.audio.voice_set(VoiceDevice::Array, VoiceMode::Conference).unwrap();
        let waited: Duration = rig.clock.sleeps.lock().unwrap().iter().sum();
        assert!(waited >= Duration::from_secs(8) && waited < Duration::from_secs(9), "{waited:?}");
    }

    #[test]
    fn voice_set_needs_the_plugin_and_never_writes_without_it() {
        let mut rig = Rig::new(false);
        rig.paths_mut().plugin = None;
        assert!(!rig.audio.has_rnnoise());
        let err = rig.audio.voice_set(VoiceDevice::Headset, VoiceMode::Conference).unwrap_err();
        assert!(err.to_string().starts_with("RNNoise LADSPA plugin missing (Arch:"), "{err}");
        assert!(rig.read("pw/truesense-pv-headset.conf").is_none());
        assert_eq!(rig.sys.count(&["systemctl"]), 0);
        // Off does not need the plugin: it only removes a file.
        assert_eq!(rig.audio.voice_set(VoiceDevice::Headset, VoiceMode::Off).unwrap(), VoiceMode::Off);
        assert!(rig.audio.speaker_filter(true, VoiceMode::Conference).is_err());
    }

    #[test]
    fn modes_read_back_from_files_including_ones_this_program_did_not_write() {
        let rig = Rig::new(false);
        std::fs::create_dir_all(rig.dir.join("pw")).unwrap();
        std::fs::write(rig.dir.join("pw/truesense-pv-headset.conf"), "context.modules = []\n").unwrap();
        std::fs::write(rig.dir.join("pw/truesense-pv-array.conf"), "\"VAD Threshold (%)\" = 42\n").unwrap();
        let modes = rig.audio.voice_modes();
        assert_eq!(modes, VoiceModes { headset: None, array: None });
        assert_eq!(serde_json::to_value(modes).unwrap(), serde_json::json!({"headset": "?", "array": "?"}));
        // "?" starts the cycle at conference.
        assert_eq!(
            rig.audio.voice_next(Some(VoiceDevice::Headset)).unwrap(),
            (VoiceDevice::Headset, VoiceMode::Conference)
        );
    }

    #[test]
    fn voice_next_cycles_the_input_in_use() {
        let rig = Rig::new(false);
        assert_eq!(rig.audio.voice_next(None).unwrap(), (VoiceDevice::Array, VoiceMode::Conference));
        assert_eq!(rig.audio.voice_next(None).unwrap(), (VoiceDevice::Array, VoiceMode::Personal));
        assert_eq!(rig.audio.voice_next(None).unwrap(), (VoiceDevice::Array, VoiceMode::Off));
        rig.sys.state().headset_mic_jack = "on".into();
        assert_eq!(rig.audio.voice_next(None).unwrap(), (VoiceDevice::Headset, VoiceMode::Conference));
        assert_eq!(rig.audio.voice_modes().array, Some(VoiceMode::Off), "the other input is untouched");
    }

    #[test]
    fn voice_enable_follows_the_configured_modes() {
        let rig = Rig::new(false);
        rig.config().audio.headset_mode = "personal".into();
        rig.config().audio.array_mode = "off".into();
        let modes = rig.audio.voice_enable(true).unwrap();
        assert_eq!(modes, VoiceModes { headset: Some(VoiceMode::Personal), array: Some(VoiceMode::Off) });
        let modes = rig.audio.voice_enable(false).unwrap();
        assert_eq!(modes, VoiceModes { headset: Some(VoiceMode::Off), array: Some(VoiceMode::Off) });
        assert_eq!(
            serde_json::to_value(modes).unwrap(),
            serde_json::json!({"headset": "off", "array": "off"})
        );
        // A bad configured mode is refused before anything changes.
        rig.config().audio.array_mode = "strict".into();
        rig.sys.clear();
        assert!(rig.audio.voice_enable(true).is_err());
        assert_eq!(rig.audio.voice_modes().headset, Some(VoiceMode::Off));
        assert_eq!(rig.sys.count(&["systemctl"]), 0);
    }

    #[test]
    fn the_mic_policy_follows_the_jack_and_the_filters() {
        let rig = Rig::new(false);
        // Array, no filter: the raw array source.
        let p = rig.audio.apply_mic_policy(true);
        assert_eq!(
            (p.device, p.mode, p.default_source.as_str()),
            (VoiceDevice::Array, Some(VoiceMode::Off), SRC_ARRAY)
        );
        assert_eq!(rig.shown(), ["mic: Microphone: Mic array - All Off"]);
        assert!(!rig.sys.did(&["pactl", "set-default-source"]), "the default was already right");

        // Headset plugged in, filter configured: the filter node, and recording streams move along.
        rig.sys.state().headset_mic_jack = "on".into();
        rig.sys.state().source_outputs = vec!["7\t1\tPipeWire".into()];
        rig.audio.voice_set(VoiceDevice::Headset, VoiceMode::Personal).unwrap();
        let p = rig.audio.apply_mic_policy(false);
        assert_eq!(
            (p.device, p.mode, p.default_source.as_str()),
            (VoiceDevice::Headset, Some(VoiceMode::Personal), "truesense_pv_headset")
        );
        assert_eq!(rig.sys.state().default_source, "truesense_pv_headset");
        assert!(rig.sys.did(&["pactl", "move-source-output", "7", "truesense_pv_headset"]));
        assert_eq!(rig.shown().len(), 1, "no notification when not asked");
        assert_eq!(
            serde_json::to_value(&p).unwrap(),
            serde_json::json!({"device": "headset", "mode": "personal", "default_source": "truesense_pv_headset"})
        );

        // Unplugged again: back to the array.
        rig.sys.state().headset_mic_jack = "off".into();
        assert_eq!(rig.audio.apply_mic_policy(false).default_source, SRC_ARRAY);

        // [audio] jack = headphones never uses the headset mic, whatever the codec says.
        rig.sys.state().headset_mic_jack = "on".into();
        rig.config().audio.jack = "headphones".into();
        assert_eq!(rig.audio.apply_mic_policy(false).device, VoiceDevice::Array);
    }

    #[test]
    fn a_configured_filter_without_its_node_falls_back_to_the_raw_source() {
        let rig = Rig::new(false);
        rig.audio.voice_set(VoiceDevice::Array, VoiceMode::Conference).unwrap();
        rig.sys.state().conf_dir = None; // PipeWire has not created the node (yet)
        let p = rig.audio.apply_mic_policy(false);
        assert_eq!(p.default_source, SRC_ARRAY);
        assert_eq!(p.mode, Some(VoiceMode::Conference));
    }

    #[test]
    fn a_foreign_config_is_treated_as_a_filter_and_its_label_is_a_question_mark() {
        let rig = Rig::new(false);
        std::fs::create_dir_all(rig.dir.join("pw")).unwrap();
        std::fs::write(rig.dir.join("pw/truesense-pv-array.conf"), "# not ours\n").unwrap();
        // Python raises a KeyError on the label here; the notification says "?".
        let p = rig.audio.apply_mic_policy(true);
        assert_eq!(p.mode, None);
        assert_eq!(rig.shown(), ["mic: Microphone: Mic array - ?"]);
        assert_eq!(serde_json::to_value(&p).unwrap()["mode"], "?");
    }

    #[test]
    fn the_speaker_filter_switches_a_file() {
        let rig = Rig::new(false);
        assert!(!rig.audio.speaker_filter_on());
        assert!(rig.audio.speaker_filter(true, VoiceMode::Personal).unwrap());
        assert!(rig.audio.speaker_filter_on());
        assert!(rig.read("pw/truesense-pv-speaker.conf").unwrap().contains("\"VAD Threshold (%)\" = 85"));
        assert_eq!(rig.sys.count(&["systemctl", "--user", "restart", "pipewire.service"]), 1);
        assert!(!rig.audio.speaker_filter(false, VoiceMode::Conference).unwrap());
        assert!(!rig.audio.speaker_filter_on());
        assert_eq!(rig.sys.count(&["systemctl"]), 2);
        rig.audio.speaker_filter(false, VoiceMode::Conference).unwrap();
        assert_eq!(rig.sys.count(&["systemctl"]), 2, "nothing to remove, nothing to restart");
        assert_eq!(
            rig.audio.speaker_filter(true, VoiceMode::Off).unwrap_err().to_string(),
            "mode: conference | personal"
        );
        assert!(!rig.audio.speaker_filter_on(), "an invalid mode writes nothing");
    }
}
