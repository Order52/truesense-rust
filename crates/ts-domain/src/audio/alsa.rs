//! The ALSA side: the 3.5 mm jack detection and the headset microphone boost, through `amixer`.
//! Only `cget` and the one `cset` of the boost are ever issued; the codec itself is never written.

use std::collections::BTreeMap;

use serde::Serialize;

use super::parse::{boost_value, card_index, control_value};
use super::voice::VoiceDevice;
use super::{Audio, AudioError};

const BOOST_CONTROL: &str = "Headset Mic Boost Volume";

/// What the codec sees on the 3.5 mm jack. The JSON names are the Python edition's; `raw` holds
/// the controls' own text (`null` when a control could not be read).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Jack {
    /// Headphones are plugged in.
    pub headphones: bool,
    /// A headset microphone was detected.
    pub headset_mic: bool,
    /// The two controls' text.
    pub raw: BTreeMap<String, Option<String>>,
}

impl Audio {
    /// The ALSA card index of the laptop's sound card. With no sound subsystem at all (a server, a
    /// container) it is `"1"` and the callers fail with a clear message from `amixer`.
    fn card(&self) -> String {
        std::fs::read_to_string(&self.paths.proc_cards)
            .map_or_else(|_| "1".to_owned(), |text| card_index(&text))
    }

    fn amixer(&self, args: &[&str]) -> String {
        let card = format!("-c{}", self.card());
        let mut all = vec![card.as_str()];
        all.extend_from_slice(args);
        self.out("amixer", &all)
    }

    /// One ALSA control's value; `None` when `amixer` printed nothing.
    fn control(&self, iface: &str, name: &str) -> Option<String> {
        control_value(&self.amixer(&["cget", &format!("iface={iface},name={name}")]))
    }

    /// What the codec sees on the 3.5 mm jack: headphones plugged, and whether a headset
    /// microphone was detected.
    pub fn jack_status(&self) -> Jack {
        let headphones = self.control("CARD", "Headphone Jack");
        let mic = self.control("CARD", "Headset Mic Jack");
        Jack {
            headphones: headphones.as_deref() == Some("on"),
            headset_mic: mic.as_deref() == Some("on"),
            raw: BTreeMap::from([
                ("Headphone Jack".to_owned(), headphones),
                ("Headset Mic Jack".to_owned(), mic),
            ]),
        }
    }

    /// Which microphone the 3.5 mm jack is treated as: the setting `[audio] jack` = `auto` (what the
    /// codec detects), `headphones` (never use the headset mic) or `headset` (always use it). The
    /// jack is read now when `status` is `None`.
    pub fn jack_use(&self, status: Option<&Jack>) -> VoiceDevice {
        match self.cfg().audio.jack.as_str() {
            "headset" => VoiceDevice::Headset,
            "headphones" => VoiceDevice::Array,
            _ => {
                let headset = status.map_or_else(|| self.jack_status().headset_mic, |s| s.headset_mic);
                if headset { VoiceDevice::Headset } else { VoiceDevice::Array }
            }
        }
    }

    /// Whether a boolean ALSA control of the card is on.
    pub fn amixer_on(&self, name: &str) -> bool {
        self.control("CARD", name).as_deref() == Some("on")
    }

    /// The headset microphone boost (0 to 3, +10 dB per step); `None` when the card has no such
    /// control.
    pub fn headset_boost(&self) -> Option<u8> {
        boost_value(&self.amixer(&["cget", &format!("iface=MIXER,name={BOOST_CONTROL}")]))
            .and_then(|v| u8::try_from(v).ok())
    }

    /// Sets the headset microphone boost (0 to 3) and reads it back; a value that did not stick is
    /// an error.
    pub fn set_headset_boost(&self, level: u8) -> Result<u8, AudioError> {
        if level > 3 {
            return Err(AudioError::Invalid("headset mic boost is 0..3".into()));
        }
        self.amixer(&["cset", &format!("iface=MIXER,name={BOOST_CONTROL}"), &format!("{level},{level}")]);
        if self.headset_boost() != Some(level) {
            return Err(AudioError::BoostNotKept);
        }
        Ok(level)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::audio::testing::Rig;

    #[test]
    fn the_jack_is_read_through_amixer_of_the_right_card() {
        let rig = Rig::new(false);
        let j = rig.audio.jack_status();
        assert!(!j.headphones && !j.headset_mic);
        assert_eq!(j.raw["Headphone Jack"].as_deref(), Some("off"));
        assert!(
            rig.sys.did(&["amixer", "-c0", "cget", "iface=CARD,name=Headphone Jack"]),
            "the card index comes from /proc/asound/cards: {:?}",
            rig.sys.sent()
        );
        {
            let mut s = rig.sys.state();
            s.headphone_jack = "on".into();
            s.headset_mic_jack = "on".into();
        }
        let j = rig.audio.jack_status();
        assert!(j.headphones && j.headset_mic);
        let json = serde_json::to_value(&j).unwrap();
        assert_eq!(json["raw"]["Headset Mic Jack"], "on");
        assert!(rig.audio.amixer_on("Headphone Jack") && !rig.audio.amixer_on("No Such Control"));
    }

    #[test]
    fn a_control_amixer_cannot_read_is_null() {
        let rig = Rig::new(false);
        rig.sys.state().no_jack_controls = true;
        let j = rig.audio.jack_status();
        assert!(!j.headphones && !j.headset_mic);
        assert_eq!(serde_json::to_value(&j).unwrap()["raw"]["Headphone Jack"], serde_json::Value::Null);
    }

    #[test]
    fn jack_use_follows_the_setting() {
        let rig = Rig::new(false);
        rig.sys.state().headset_mic_jack = "on".into();
        for (setting, want) in [
            ("auto", VoiceDevice::Headset),
            ("headset", VoiceDevice::Headset),
            ("headphones", VoiceDevice::Array),
            ("junk", VoiceDevice::Headset),
        ] {
            rig.config().audio.jack = setting.into();
            assert_eq!(rig.audio.jack_use(None), want, "{setting}");
        }
        rig.sys.state().headset_mic_jack = "off".into();
        rig.config().audio.jack = "auto".into();
        assert_eq!(rig.audio.jack_use(None), VoiceDevice::Array);
        // A status that was already read is not read again.
        rig.sys.clear();
        let status = Jack { headphones: true, headset_mic: true, raw: BTreeMap::new() };
        assert_eq!(rig.audio.jack_use(Some(&status)), VoiceDevice::Headset);
        assert!(rig.sys.sent().is_empty());
    }

    // Python: test_headset_boost_reads_the_value_line_not_the_channel_count.
    #[test]
    fn the_boost_is_set_and_verified() {
        let rig = Rig::new(false);
        assert_eq!(rig.audio.headset_boost(), Some(1));
        for level in [0u8, 3, 2, 1] {
            assert_eq!(rig.audio.set_headset_boost(level).unwrap(), level);
            assert_eq!(rig.audio.headset_boost(), Some(level));
        }
        assert!(rig.sys.did(&["amixer", "-c0", "cset", "iface=MIXER,name=Headset Mic Boost Volume", "2,2"]));
        let before = rig.sys.sent().len();
        for bad in [4u8, 200, 255] {
            assert_eq!(
                rig.audio.set_headset_boost(bad).unwrap_err().to_string(),
                "headset mic boost is 0..3"
            );
        }
        assert_eq!(rig.sys.sent().len(), before, "a refused level reaches no tool");
    }

    #[test]
    fn a_boost_that_does_not_stick_or_does_not_exist_is_reported() {
        let rig = Rig::new(false);
        rig.sys.state().boost_broken = true;
        assert!(matches!(rig.audio.set_headset_boost(3), Err(AudioError::BoostNotKept)));
        let rig = Rig::new(false);
        rig.sys.state().no_boost_control = true;
        assert_eq!(rig.audio.headset_boost(), None);
        assert!(rig.audio.set_headset_boost(2).is_err());
    }

    #[test]
    fn without_a_sound_subsystem_the_card_is_one() {
        let rig = Rig::new(false);
        std::fs::remove_file(&rig.audio.paths.proc_cards).unwrap();
        assert_eq!(rig.audio.card(), "1");
        rig.audio.jack_status();
        assert!(rig.sys.did(&["amixer", "-c1"]));
    }
}
