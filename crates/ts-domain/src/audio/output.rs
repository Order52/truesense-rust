//! Output switching between the speakers and the headphones.
//!
//! The laptop plays on the speakers OR the headphone jack, never both: they share one playback PCM,
//! so ALSA offers them as two exclusive card profiles, and flipping the profile rebinds the stream.

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use serde::Serialize;
use ts_foundation::fsx::FileLock;

use super::parse::{active_profile, card_block, card_name, short_column, sink_description};
use super::{Audio, AudioError, CARD_MATCH, FRONT_SINK};

/// How long to wait for the new sink after a profile flip: 100 polls of 30 ms (measured: the sink
/// appears after about 0.04 s) instead of a fixed sleep.
const SINK_POLLS: u32 = 100;
const SINK_POLL: Duration = Duration::from_millis(30);

/// One of the two built-in outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    /// The laptop speakers.
    Speakers,
    /// The 3.5 mm headphone jack.
    Headphones,
}

impl Output {
    /// The name used in JSON and on the command line.
    pub fn as_str(self) -> &'static str {
        match self {
            Output::Speakers => "speakers",
            Output::Headphones => "headphones",
        }
    }

    /// The card profile that selects it.
    pub fn profile(self) -> &'static str {
        match self {
            Output::Speakers => "HiFi (Mic1, Mic2, Speaker)",
            Output::Headphones => "HiFi (Headphones, Mic1, Mic2)",
        }
    }

    /// The human name of the output, as the OSD shows it.
    pub fn label(self) -> &'static str {
        match self {
            Output::Speakers => "Laptop speakers",
            Output::Headphones => "Headphones (3.5 mm jack)",
        }
    }

    /// The OSD icon.
    pub fn icon(self) -> &'static str {
        match self {
            Output::Speakers => "speaker",
            Output::Headphones => "audio",
        }
    }

    /// The part of the sink name that tells the output.
    fn sink_part(self) -> &'static str {
        match self {
            Output::Speakers => "Speaker",
            Output::Headphones => "Headphones",
        }
    }
}

impl fmt::Display for Output {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Output {
    type Err = AudioError;

    fn from_str(s: &str) -> Result<Self, AudioError> {
        match s {
            "speakers" => Ok(Output::Speakers),
            "headphones" => Ok(Output::Headphones),
            _ => Err(AudioError::Invalid("output: speakers | headphones".into())),
        }
    }
}

/// Which card profile is active and whether something is plugged into the jack. The JSON names are
/// the Python edition's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutputStatus {
    /// `speakers` or `headphones`, or the card's own profile name when it is neither (for example
    /// `off`).
    pub active: String,
    /// Something is plugged into the 3.5 mm jack.
    pub jack_plugged: bool,
}

impl Audio {
    /// The PipeWire name of the sound card.
    pub(super) fn card_name(&self) -> Option<String> {
        card_name(&self.out("pactl", &["list", "short", "cards"]), CARD_MATCH)
    }

    /// Which output profile is active.
    pub fn output_status(&self) -> Result<OutputStatus, AudioError> {
        let card = self.card_name().ok_or(AudioError::NoCard)?;
        let block =
            card_block(&self.out("pactl", &["list", "cards"]), &card, false).ok_or(AudioError::NoCard)?;
        let profile = active_profile(&block);
        let active = [Output::Speakers, Output::Headphones]
            .into_iter()
            .find(|o| o.profile() == profile)
            .map_or(profile, |o| o.as_str().to_owned());
        Ok(OutputStatus { active, jack_plugged: self.jack_status().headphones })
    }

    /// Switches between the speakers and the headphones and returns the output it ends on. It
    /// waits until the new sink exists instead of a fixed sleep, verifies the result, and makes
    /// that sink the default output: flipping only the card's profile left the sound where the
    /// default was (a Purified Voice filter, a USB or Bluetooth device), so "Headphones" was
    /// chosen and nothing changed. AcerUltraX, when it runs, follows the default and routes to it.
    pub fn set_output(&self, which: Output) -> Result<Output, AudioError> {
        let card = self.card_name().ok_or(AudioError::NoCard)?;
        let _quiet = self.quiet();
        self.run("pactl", &["set-card-profile", &card, which.profile()]);
        let want = format!("{}__sink", which.sink_part());
        for _ in 0..SINK_POLLS {
            if self.out("pactl", &["list", "short", "sinks"]).contains(&want) {
                break;
            }
            self.clock.sleep(SINK_POLL);
        }
        let got = self.output_status()?.active;
        if got != which.as_str() {
            return Err(AudioError::ProfileNotSwitched { got, wanted: which.as_str() });
        }
        if let Some(sink) = self.sinks().into_iter().find(|n| n.contains(CARD_MATCH) && n.ends_with(&want)) {
            self.run("pactl", &["set-default-sink", &sink]);
        }
        Ok(which)
    }

    pub(super) fn sinks(&self) -> Vec<String> {
        short_column(&self.out("pactl", &["list", "short", "sinks"]), 1)
    }

    pub(super) fn default_sink(&self) -> String {
        self.out("pactl", &["get-default-sink"]).trim().to_owned()
    }

    /// Flips between the speakers and the headphones and returns the new output.
    pub fn output_toggle(&self) -> Result<Output, AudioError> {
        let current = self.output_status()?.active;
        let next = if current == Output::Speakers.as_str() { Output::Headphones } else { Output::Speakers };
        let noctalia = self.noctalia_style();
        if !noctalia {
            self.say(&format!("Audio output: {}", next.label()), next.icon()); // instant, before the ~1 s switch
        }
        self.set_output(next)?;
        if noctalia {
            self.say(next.label(), next.icon());
        }
        Ok(next)
    }

    /// The F5-style round robin: laptop speakers, then the headphones (only when the jack is
    /// plugged in), then the other outputs; it flips the card profile when needed and returns the
    /// label of the new output. The AcerUltraX virtual sink (if present) stays the default so the DTS
    /// processing keeps working; it follows the hardware output.
    ///
    /// Two quick presses must not interleave, so the whole cycle runs under a file lock.
    pub fn output_cycle(&self) -> Result<String, AudioError> {
        let _lock = FileLock::exclusive(&self.paths.output_lock)?;
        let _quiet = self.quiet();
        self.cycle()
    }

    fn cycle(&self) -> Result<String, AudioError> {
        let sinks = self.sinks();
        let dts = sinks.iter().any(|n| n == FRONT_SINK);
        // Where sound can really end up: the laptop's own two, then USB and Bluetooth outputs.
        // Not HDMI, not a game controller's speaker, and never a filter in front of a device (the
        // Purified Voice speaker filter, the AcerUltraX sink): those are not a place to switch to.
        let others: Vec<String> =
            sinks.iter().filter(|n| is_real_output(n) && !n.contains(CARD_MATCH)).cloned().collect();
        let mut stops = vec!["int:speakers".to_owned()];
        if self.jack_status().headphones {
            stops.push("int:headphones".into());
        }
        stops.extend(others.iter().cloned());

        let current = self.current_stop(&others);
        let next = match current.and_then(|c| stops.iter().position(|s| *s == c)) {
            Some(i) => &stops[(i + 1) % stops.len()],
            None => &stops[0],
        };
        let internal = next.strip_prefix("int:").and_then(|n| n.parse::<Output>().ok());
        let (label, icon) = match internal {
            Some(o) => (o.label().to_owned(), o.icon()),
            None => (sink_description(&self.out("pactl", &["list", "sinks"]), next), "audio"),
        };
        let noctalia = self.noctalia_style();
        if !noctalia {
            self.say(&format!("Audio output: {label}"), icon);
        }
        // The real device becomes the default. AcerUltraX takes that as the person's pick, routes
        // its output there and puts its own sink back as the default (setting its sink ourselves
        // left it on the device picked before).
        let target = match internal {
            Some(o) => {
                self.set_output(o)?;
                self.sinks().into_iter().find(|n| n.contains(CARD_MATCH) && n.contains(o.sink_part()))
            }
            None => Some(next.clone()),
        };
        let target = target.ok_or(AudioError::NoSink)?;
        self.run("pactl", &["set-default-sink", &target]);
        // The playing apps (never AcerUltraX's own streams or a filter's output). Without
        // AcerUltraX they are moved over. With it they are unpinned instead: an app moved by hand
        // once stays pinned to that device (PipeWire remembers it), and AcerUltraX takes a pinned
        // app as the person's pick and went back to it every second, whatever F5 chose.
        for (input, node) in app_streams(&self.out("pactl", &["list", "sink-inputs"])) {
            if dts {
                self.run("pw-metadata", &["-d", &node, "target.object"]);
                self.run("pw-metadata", &["-d", &node, "target.node"]);
            } else {
                self.run("pactl", &["move-sink-input", &input, &target]);
            }
        }
        if noctalia {
            self.say(&label, icon);
        }
        Ok(label)
    }

    /// The stop sound goes to now: the default when it is a real device; through AcerUltraX or a
    /// filter, the device AcerUltraX's output is linked to; the laptop's card as its active output.
    /// `None` when it cannot be told (the cycle then starts from the speakers).
    fn current_stop(&self, others: &[String]) -> Option<String> {
        let mut device = self.default_sink();
        if !is_real_output(&device) {
            device = linked_device(&self.out("pw-link", &["-l"]), "acerultrax_output:output_FL")
                .unwrap_or_default();
        }
        if device.contains(CARD_MATCH) {
            let active = self.output_status().ok()?.active;
            return [Output::Speakers, Output::Headphones]
                .iter()
                .any(|o| o.as_str() == active)
                .then(|| format!("int:{active}"));
        }
        others.contains(&device).then_some(device)
    }
}

/// A sink that is a device sound ends up on (ALSA or Bluetooth), other than HDMI and a game
/// controller's speaker. Filters and the AcerUltraX sink are not.
pub(crate) fn is_real_output(name: &str) -> bool {
    let lower = name.to_lowercase();
    (lower.starts_with("alsa_output.") || lower.starts_with("bluez_output."))
        && !["hdmi", "dualsense", "dualshock", "controller", "gamepad", "xbox"]
            .iter()
            .any(|w| lower.contains(w))
}

/// The playing apps in `pactl list sink-inputs`: (sink input number, node id). A stream with no
/// application name, a filter's output and AcerUltraX's own streams are left out.
pub(crate) fn app_streams(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for block in text.split("Sink Input #").skip(1) {
        let input: String = block.chars().take_while(char::is_ascii_digit).collect();
        let prop = |key: &str| {
            block.lines().find_map(|l| {
                let (k, v) = l.trim().split_once(" = ")?;
                (k == key).then(|| v.trim_matches('"').to_owned())
            })
        };
        let (Some(app), Some(node)) = (prop("application.name"), prop("object.id")) else { continue };
        let name = prop("node.name").unwrap_or_default();
        let ours = name.starts_with("output.filter-chain")
            || format!("{app} {name}").to_lowercase().contains("acerultrax");
        if !input.is_empty() && !ours {
            out.push((input, node));
        }
    }
    out
}

/// The sink a port's link goes to, from `pw-link -l` (`port` then `  |-> sink:playback_FL`).
pub(crate) fn linked_device(links: &str, port: &str) -> Option<String> {
    let mut lines = links.lines();
    while let Some(line) = lines.next() {
        if line.trim() == port {
            let next = lines.next()?;
            let target = next.trim().strip_prefix("|->")?.trim();
            return target.rsplit_once(':').map(|(sink, _)| sink.to_owned());
        }
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::audio::testing::*;

    #[test]
    fn output_names_and_parsing() {
        assert_eq!("speakers".parse::<Output>().unwrap(), Output::Speakers);
        assert_eq!("headphones".parse::<Output>().unwrap(), Output::Headphones);
        for bad in ["", "Speakers", "speaker", "hwC0D0/reconfig", "/sys/class/sound/hwC0D0/reconfig"] {
            assert_eq!(
                bad.parse::<Output>().unwrap_err().to_string(),
                "output: speakers | headphones",
                "{bad:?}"
            );
        }
        assert_eq!(Output::Speakers.label(), "Laptop speakers");
        assert_eq!(Output::Headphones.label(), "Headphones (3.5 mm jack)");
        assert_eq!((Output::Speakers.icon(), Output::Headphones.icon()), ("speaker", "audio"));
        assert_eq!(Output::Speakers.to_string(), "speakers");
    }

    #[test]
    fn the_output_status_names_the_profile() {
        let rig = Rig::new(false);
        assert_eq!(
            rig.audio.output_status().unwrap(),
            OutputStatus { active: "speakers".into(), jack_plugged: false }
        );
        rig.sys.state().profile = HIFI_HEADPHONES.into();
        rig.sys.state().headphone_jack = "on".into();
        assert_eq!(
            rig.audio.output_status().unwrap(),
            OutputStatus { active: "headphones".into(), jack_plugged: true }
        );
        let broken = Rig::new(true);
        assert_eq!(
            broken.audio.output_status().unwrap().active,
            "off",
            "an unknown profile is reported as it is"
        );
        let json = serde_json::to_value(rig.audio.output_status().unwrap()).unwrap();
        assert_eq!(json, serde_json::json!({"active": "headphones", "jack_plugged": true}));
        rig.sys.state().card_present = false;
        assert!(matches!(rig.audio.output_status(), Err(AudioError::NoCard)));
    }

    #[test]
    fn set_output_flips_the_profile_and_verifies_it() {
        let rig = Rig::new(false);
        assert_eq!(rig.audio.set_output(Output::Headphones).unwrap(), Output::Headphones);
        assert!(rig.sys.did(&["pactl", "set-card-profile", CARD, HIFI_HEADPHONES]));
        assert_eq!(rig.sys.state().profile, HIFI_HEADPHONES);
        // The jack is also the default output, whatever was the default before (here a filter).
        rig.sys.state().default_sink = "truesense_pv_speaker".into();
        rig.audio.set_output(Output::Headphones).unwrap();
        assert_eq!(rig.sys.state().default_sink, HW_HEADPHONES);
        assert_eq!(rig.audio.set_output(Output::Speakers).unwrap(), Output::Speakers);
        assert_eq!(rig.sys.state().default_sink, HW_SPEAKER);
        assert_eq!(rig.osd.quiet_depth(), 0, "the popups are enabled again");
        assert_eq!(rig.osd.max_depth.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn set_output_reports_a_card_that_stays_on_another_profile() {
        let rig = Rig::new(true); // the card only offers off and pro-audio
        let err = rig.audio.set_output(Output::Speakers).unwrap_err();
        assert_eq!(err.to_string(), "profile readback off != speakers");
        assert_eq!(rig.clock.sleeps.lock().unwrap().len(), 100, "it waited its 3 s for the sink, no more");
        assert_eq!(rig.osd.quiet_depth(), 0);
        let none = Rig::new(false);
        none.sys.state().card_present = false;
        assert!(matches!(none.audio.set_output(Output::Speakers), Err(AudioError::NoCard)));
    }

    #[test]
    fn toggle_announces_before_the_switch_unless_the_shell_wants_it_after() {
        let rig = Rig::new(false);
        assert_eq!(rig.audio.output_toggle().unwrap(), Output::Headphones);
        assert_eq!(rig.shown(), ["audio: Audio output: Headphones (3.5 mm jack)"]);
        assert_eq!(rig.audio.output_toggle().unwrap(), Output::Speakers);
        assert_eq!(rig.shown()[1], "speaker: Audio output: Laptop speakers");

        let rig = Rig::new(false);
        rig.config().osd.style = "noctalia".into();
        rig.audio.output_toggle().unwrap();
        assert_eq!(
            rig.shown(),
            ["audio: Headphones (3.5 mm jack)"],
            "Noctalia gets its popup after the switch"
        );

        let rig = Rig::new(false);
        rig.config().osd.style = "noctalia".into();
        rig.env.lock().unwrap().insert("TRUESENSE_OSD".into(), "native".into());
        rig.audio.output_toggle().unwrap();
        assert_eq!(
            rig.shown(),
            ["audio: Audio output: Headphones (3.5 mm jack)"],
            "TRUESENSE_OSD overrides the config"
        );
    }

    #[test]
    fn cycle_goes_speakers_headphones_others_and_back() {
        let rig = Rig::new(false);
        rig.sys.state().extra_sinks = vec![
            "bluez_output.AA.1".into(),
            "alsa_output.pci-0000_00_03.0.hdmi-stereo".into(),
            "truesense_pv_speaker".into(),
            DUALSENSE.into(),
        ];
        rig.sys.state().sink_descs.insert("bluez_output.AA.1".into(), "Some Speaker".into());
        rig.sys.state().headphone_jack = "on".into();
        rig.sys.state().default_sink = HW_SPEAKER.into();
        rig.sys.state().sink_inputs_long = APPS.into();

        // speakers -> headphones: the jack's own sink is the default (AcerUltraX takes it from
        // there); the music app is unpinned, not moved, and the filter's output is left alone.
        assert_eq!(rig.audio.output_cycle().unwrap(), "Headphones (3.5 mm jack)");
        assert_eq!(rig.sys.state().profile, HIFI_HEADPHONES);
        assert_eq!(rig.sys.state().default_sink, HW_HEADPHONES);
        let sent = rig.sys.sent();
        assert!(!sent.iter().any(|c| c.contains("move-sink-input")), "{sent:?}");
        assert!(sent.contains(&"pw-metadata -d 146 target.object".to_owned()), "{sent:?}");
        assert!(
            !sent.iter().any(|c| c.contains("pw-metadata -d 49 ")),
            "the filter's output stays: {sent:?}"
        );
        // headphones -> the Bluetooth speaker (HDMI, the filter and the controller are skipped)
        assert_eq!(rig.audio.output_cycle().unwrap(), "Some Speaker");
        assert_eq!(rig.sys.state().default_sink, "bluez_output.AA.1");
        // bluetooth -> speakers
        assert_eq!(rig.audio.output_cycle().unwrap(), "Laptop speakers");
        assert_eq!(rig.sys.state().profile, HIFI_SPEAKERS);
        assert_eq!(rig.sys.state().default_sink, HW_SPEAKER);
        assert_eq!(rig.osd.quiet_depth(), 0);
        let shown = rig.shown();
        assert!(
            shown.iter().all(|s| !s.contains("hdmi") && !s.contains("Purified") && !s.contains("DualSense")),
            "{shown:?}"
        );
    }

    #[test]
    fn cycle_finds_the_real_device_behind_acerultrax_and_a_filter() {
        // The laptop as it was found: the speaker filter is the default, AcerUltraX feeds the
        // headphones, the jack is in. F5 must go on from the headphones, not from a filter.
        let rig = Rig::new(false);
        rig.sys.state().headphone_jack = "on".into();
        rig.audio.set_output(Output::Headphones).unwrap();
        rig.sys.state().extra_sinks =
            vec!["truesense_pv_speaker".into(), DUALSENSE.into(), "bluez_output.AA.1".into()];
        rig.sys.state().default_sink = "truesense_pv_speaker".into();
        rig.sys.state().links = format!(
            "acerultrax_sink:monitor_FL\n  |-> acerultrax_output:g0_FL\nacerultrax_output:output_FL\n  |-> {HW_HEADPHONES}:playback_FL\n"
        );
        assert_eq!(rig.audio.output_cycle().unwrap(), "Sink bluez_output.AA.1");
        // Through AcerUltraX to a controller (not a stop): start again from the speakers.
        rig.sys.state().default_sink = "acerultrax_sink".into();
        rig.sys.state().links = format!("acerultrax_output:output_FL\n  |-> {DUALSENSE}:playback_FL\n");
        assert_eq!(rig.audio.output_cycle().unwrap(), "Laptop speakers");
        assert_eq!(rig.sys.state().default_sink, HW_SPEAKER);
    }

    /// `pactl list sink-inputs` as on the laptop: a filter's output and a music app.
    const APPS: &str = "Sink Input #49\n\tProperties:\n\t\tmedia.name = \"Purified Voice Speaker output\"\n\t\tnode.name = \"output.filter-chain-1552-31\"\n\t\tobject.id = \"49\"\nSink Input #473\n\tProperties:\n\t\tapplication.name = \"Gapless\"\n\t\tnode.name = \"Gapless\"\n\t\tobject.id = \"146\"\n";

    #[test]
    fn the_playing_apps_are_found_and_filters_left_out() {
        assert_eq!(app_streams(APPS), [("473".to_owned(), "146".to_owned())]);
        assert!(app_streams("").is_empty());
        // Without AcerUltraX the apps are moved to the new device.
        let rig = Rig::new(false);
        rig.sys.state().no_front_sink = true;
        rig.sys.state().default_sink = HW_SPEAKER.into();
        rig.sys.state().headphone_jack = "on".into();
        rig.sys.state().sink_inputs_long = APPS.into();
        rig.audio.output_cycle().unwrap();
        assert!(rig.sys.did(&["pactl", "move-sink-input", "473", HW_HEADPHONES]));
        assert!(!rig.sys.did(&["pactl", "move-sink-input", "49"]));
    }

    #[test]
    fn real_outputs_and_links_are_read_right() {
        assert!(is_real_output(HW_HEADPHONES) && is_real_output("bluez_output.AA.1"));
        assert!(is_real_output("alsa_output.usb-GeneralPlus_USB_Audio_Device-00.analog-stereo"));
        for not in [
            "truesense_pv_speaker",
            "acerultrax_sink",
            DUALSENSE,
            "alsa_output.pci-0000_01_00.1.hdmi-stereo",
            "",
        ] {
            assert!(!is_real_output(not), "{not}");
        }
        let links = "a:x\n  |-> b:y\nacerultrax_output:output_FL\n  |-> sink.name:playback_FL\n";
        assert_eq!(linked_device(links, "acerultrax_output:output_FL").as_deref(), Some("sink.name"));
        assert_eq!(linked_device(links, "missing:port"), None);
        assert_eq!(linked_device("acerultrax_output:output_FL\n", "acerultrax_output:output_FL"), None);
    }

    #[test]
    fn cycle_skips_the_headphones_when_the_jack_is_empty_and_uses_the_hardware_sink_without_dts() {
        let rig = Rig::new(false);
        rig.sys.state().default_sink = HW_SPEAKER.into();
        rig.sys.state().extra_sinks = vec!["bluez_output.AA.1".into()];
        assert_eq!(
            rig.audio.output_cycle().unwrap(),
            "Sink bluez_output.AA.1",
            "the description of the sink is its label"
        );
        assert_eq!(rig.audio.output_cycle().unwrap(), "Laptop speakers");
        // Without the AcerUltraX sink the default becomes the hardware sink of that output.
        let rig = Rig::new(false);
        rig.sys.state().no_front_sink = true;
        rig.sys.state().default_sink = HW_SPEAKER.into();
        rig.sys.state().headphone_jack = "on".into();
        assert_eq!(rig.audio.output_cycle().unwrap(), "Headphones (3.5 mm jack)");
        assert_eq!(rig.sys.state().default_sink, HW_HEADPHONES);
    }

    #[test]
    fn cycle_with_a_broken_card_fails_cleanly() {
        let rig = Rig::new(true);
        assert!(rig.audio.output_cycle().is_err());
        assert_eq!(rig.osd.quiet_depth(), 0, "the popups are enabled again even after a failure");
    }

    #[test]
    fn a_lock_that_cannot_be_taken_stops_the_cycle() {
        let mut rig = Rig::new(false);
        let lock = rig.dir.join("no/such/dir/lock");
        rig.paths_mut().output_lock = lock;
        let err = rig.audio.output_cycle().unwrap_err();
        assert!(matches!(err, AudioError::Fs(_)), "{err}");
        assert!(rig.sys.sent().is_empty());
    }
}
