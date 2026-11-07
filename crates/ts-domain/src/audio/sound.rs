//! "There is no sound": detect and repair.
//!
//! Seen on the PH16-71 (Nov 2026): at boot the sound DSP (SOF) rejected a stream while WirePlumber
//! probed the card (`ipc tx error ... -22` in the kernel log), so WirePlumber built the card with
//! only `off` and `pro-audio` profiles: no speaker/headphone sink at all, only the virtual DTS sink
//! in front of nothing. The hardware and the UCM profiles were fine (direct ALSA playback worked);
//! restarting WirePlumber so that it probes the card again brought the HiFi profiles and the sink
//! back. Nothing here touches the codec.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Serialize, Serializer};
use serde_json::json;

use super::output::Output;
use super::parse::{active_profile, card_block, profiles_offered};
use super::{Audio, AudioError, CARD_MATCH};

/// The moments after login at which [`Audio::autorepair`] looks: the card is probed a few seconds
/// after boot, and WirePlumber may be slow on a busy session.
pub const DEFAULT_REPAIR_DELAYS: [Duration; 2] = [Duration::from_secs(20), Duration::from_secs(90)];

/// The verdict on the sound card. Which fields exist (and appear in the JSON) depends on the
/// verdict, as in the Python edition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SoundState {
    /// There is no such card to judge (`ok` is `null`).
    NoCard {
        /// Why.
        problem: String,
    },
    /// The card has an output sink.
    Ok {
        /// The card's PipeWire name.
        card: String,
        /// Its active profile.
        profile: String,
        /// The hardware sinks.
        sinks: Vec<String>,
    },
    /// The card has no output sink.
    Broken {
        /// The card's PipeWire name.
        card: String,
        /// Its active profile.
        profile: String,
        /// The profiles the card offers.
        offered: Vec<String>,
        /// Whether the speaker or headphone profile is among them.
        hifi_offered: bool,
        /// What is wrong, in words.
        problem: String,
    },
}

impl SoundState {
    /// `Some(true)` with an output, `Some(false)` without, `None` when there is no card to judge.
    pub fn ok(&self) -> Option<bool> {
        match self {
            Self::NoCard { .. } => None,
            Self::Ok { .. } => Some(true),
            Self::Broken { .. } => Some(false),
        }
    }

    /// The card is known and has an output.
    pub fn is_ok(&self) -> bool {
        self.ok() == Some(true)
    }

    /// The card is known and has no output.
    pub fn is_broken(&self) -> bool {
        self.ok() == Some(false)
    }

    /// What is wrong, when something is.
    pub fn problem(&self) -> Option<&str> {
        match self {
            Self::NoCard { problem } | Self::Broken { problem, .. } => Some(problem),
            Self::Ok { .. } => None,
        }
    }
}

impl Serialize for SoundState {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::NoCard { problem } => json!({"ok": null, "problem": problem}),
            Self::Ok { card, profile, sinks } => {
                json!({"ok": true, "card": card, "profile": profile, "sinks": sinks})
            }
            Self::Broken { card, profile, offered, hifi_offered, problem } => json!({
                "ok": false, "card": card, "profile": profile, "offered": offered,
                "hifi_offered": hifi_offered, "problem": problem,
            }),
        }
        .serialize(serializer)
    }
}

/// What [`Audio::repair`] did. The JSON names are the Python edition's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepairResult {
    /// The output exists now.
    pub fixed: bool,
    /// Something was switched or restarted.
    pub changed: bool,
    /// What was done, in words.
    pub steps: Vec<String>,
    /// The card afterwards.
    pub state: SoundState,
}

/// How long [`Audio::repair`] waits for the sink to come back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepairOptions {
    /// The longest wait after each step.
    pub timeout: Duration,
    /// The pause between two looks.
    pub poll: Duration,
}

impl Default for RepairOptions {
    /// 10 s and 250 ms, the Python edition's.
    fn default() -> Self {
        Self { timeout: Duration::from_secs(10), poll: Duration::from_millis(250) }
    }
}

impl Audio {
    /// Whether the card has an output sink: [`SoundState::Ok`] when it does, [`SoundState::Broken`]
    /// when it does not, [`SoundState::NoCard`] when PipeWire does not know the card.
    pub fn sound_state(&self) -> Result<SoundState, AudioError> {
        let Some(card) = self.card_name() else {
            return Ok(SoundState::NoCard { problem: "no sound card known to PipeWire".into() });
        };
        let block = card_block(&self.out("pactl", &["list", "cards"]), &card, true)
            .ok_or_else(|| AudioError::CardNotListed(card.clone()))?;
        let profile = active_profile(&block);
        let offered = profiles_offered(&block);
        let prefix = format!("alsa_output.{CARD_MATCH}");
        let sinks: Vec<String> = self.sinks().into_iter().filter(|s| s.starts_with(&prefix)).collect();
        if !sinks.is_empty() {
            return Ok(SoundState::Ok { card, profile, sinks });
        }
        let hifi_offered =
            offered.iter().any(|p| p == Output::Speakers.profile() || p == Output::Headphones.profile());
        let mut problem = if profile == "off" {
            "the sound card is switched off".to_owned()
        } else {
            format!("the sound card has no output (profile: {profile})")
        };
        if !hifi_offered {
            problem.push_str("; its speaker and headphone profiles are missing");
        }
        Ok(SoundState::Broken { card, profile, offered, hifi_offered, problem })
    }

    /// Waits up to `options.timeout` for a hardware sink to exist.
    fn wait_for_sink(&self, options: RepairOptions) -> bool {
        let end = self.clock.now() + options.timeout;
        while self.clock.now() < end {
            if self.sound_state().is_ok_and(|s| s.is_ok()) {
                return true;
            }
            self.clock.sleep(options.poll);
        }
        false
    }

    /// Brings the sound output back: if the card still offers its HiFi profiles, pick one;
    /// otherwise restart WirePlumber so that it probes the card again (it then restores the saved
    /// profile). It verifies that a hardware sink exists afterwards, never forces a profile the card
    /// does not offer, and never writes codec settings.
    pub fn repair(&self, options: RepairOptions) -> Result<RepairResult, AudioError> {
        let state = self.sound_state()?;
        let mut steps = Vec::new();
        let SoundState::Broken { card, hifi_offered, .. } = &state else {
            return Ok(RepairResult { fixed: state.is_ok(), changed: false, steps, state });
        };
        let select_speakers = |card: &str, steps: &mut Vec<String>| {
            self.run("pactl", &["set-card-profile", card, Output::Speakers.profile()]);
            steps.push("selected the speaker profile".to_owned());
        };
        let current = |this: &Self| {
            this.sound_state().unwrap_or_else(|e| SoundState::NoCard { problem: e.to_string() })
        };
        if *hifi_offered {
            select_speakers(card, &mut steps);
            if self.wait_for_sink(options) {
                return Ok(RepairResult { fixed: true, changed: true, steps, state: current(self) });
            }
        }
        self.run("systemctl", &["--user", "restart", "wireplumber.service"]);
        steps.push("restarted WirePlumber so it probes the sound card again".to_owned());
        if !self.wait_for_sink(options) {
            if let SoundState::Broken { card, hifi_offered: true, .. } = current(self) {
                select_speakers(&card, &mut steps);
                self.wait_for_sink(options);
            }
        }
        let state = current(self);
        Ok(RepairResult { fixed: state.is_ok(), changed: true, steps, state })
    }

    /// Run from the login audio service: a few checks after login (the delays are measured from the
    /// start), and a repair when the output is missing. Like the Python edition it acts at most
    /// once: the later checks only run when an earlier one could not be made.
    pub fn autorepair(&self, delays: &[Duration], stop: &AtomicBool) {
        let mut previous = Duration::ZERO;
        for delay in delays {
            self.clock.sleep(delay.saturating_sub(previous));
            previous = *delay;
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let state = match self.sound_state() {
                Ok(state) => state,
                Err(e) => {
                    self.log(&format!("sound check failed: {e}"));
                    continue;
                }
            };
            if !state.is_broken() {
                return;
            }
            match self.repair(RepairOptions::default()) {
                Ok(r) => {
                    let verdict = if r.fixed { "FIXED" } else { "still missing" };
                    self.log(&format!("sound output was missing: {}: {verdict}", r.steps.join("; ")));
                    if r.fixed {
                        self.say("Sound output repaired", "audio");
                    }
                    return;
                }
                Err(e) => self.log(&format!("sound check failed: {e}")),
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::audio::testing::*;

    fn quick() -> RepairOptions {
        RepairOptions { timeout: Duration::from_secs(1), poll: Duration::from_millis(10) }
    }

    // Python: test_a_card_that_is_off_with_no_output_sink_is_detected_and_explained.
    #[test]
    fn a_card_that_is_off_with_no_output_sink_is_detected_and_explained() {
        let rig = Rig::new(true);
        let state = rig.audio.sound_state().unwrap();
        let SoundState::Broken { profile, hifi_offered, problem, offered, .. } = &state else {
            panic!("{state:?}")
        };
        assert_eq!(profile, "off");
        assert!(!hifi_offered);
        assert_eq!(offered, &["off", "pro-audio"]);
        assert!(problem.contains("switched off") && problem.contains("profiles are missing"), "{problem}");
        assert_eq!(problem, "the sound card is switched off; its speaker and headphone profiles are missing");
        assert_eq!(state.ok(), Some(false));
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["ok"], false);
        assert_eq!(json["hifi_offered"], false);
        assert_eq!(json["offered"], serde_json::json!(["off", "pro-audio"]));
    }

    #[test]
    fn a_card_on_an_odd_profile_is_described_with_it() {
        let rig = Rig::new(true);
        rig.sys.state().offered = vec!["off".into(), HIFI_SPEAKERS.into()];
        rig.sys.state().profile = "pro-audio".into();
        let state = rig.audio.sound_state().unwrap();
        assert_eq!(state.problem(), Some("the sound card has no output (profile: pro-audio)"));
    }

    #[test]
    fn a_healthy_card_and_a_missing_card_are_not_touched() {
        let rig = Rig::new(false);
        let state = rig.audio.sound_state().unwrap();
        assert!(state.is_ok());
        assert_eq!(state.problem(), None);
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"ok": true, "card": CARD, "profile": HIFI_SPEAKERS, "sinks": [HW_SPEAKER]})
        );
        let r = rig.audio.repair(RepairOptions::default()).unwrap();
        assert_eq!((r.fixed, r.changed, r.steps.len()), (true, false, 0));
        assert_eq!(
            rig.sys.count(&["systemctl"]) + rig.sys.count(&["pactl", "set-card-profile"]),
            0,
            "nothing is restarted or switched"
        );

        let none = Rig::new(false);
        none.sys.state().card_present = false;
        let state = none.audio.sound_state().unwrap();
        assert_eq!(state.ok(), None);
        assert_eq!(
            serde_json::to_value(&state).unwrap(),
            serde_json::json!({"ok": null, "problem": "no sound card known to PipeWire"})
        );
        assert!(!none.audio.repair(RepairOptions::default()).unwrap().fixed);
    }

    #[test]
    fn a_card_that_is_named_but_not_listed_is_an_error() {
        let rig = Rig::new(false);
        rig.sys.state().card_missing_from_long_list = true;
        let err = rig.audio.sound_state().unwrap_err();
        assert!(matches!(err, AudioError::CardNotListed(_)));
        assert_eq!(err.to_string(), format!("card {CARD} is not in the card list"));
        assert!(rig.audio.repair(quick()).is_err());
        assert!(rig.audio.output_status().is_err());
    }

    #[test]
    fn only_the_hardware_sinks_of_the_card_count_as_output() {
        // The virtual DTS sink and an HDMI sink in front of a dead card are not "sound".
        let rig = Rig::new(true);
        rig.sys.state().extra_sinks = vec!["alsa_output.pci-0000_00_03.0.hdmi-stereo".into()];
        assert!(rig.audio.sound_state().unwrap().is_broken());
    }

    #[test]
    fn repair_restarts_wireplumber_when_the_profiles_are_missing() {
        let rig = Rig::new(true);
        let r = rig.audio.repair(quick()).unwrap();
        assert!(r.fixed && r.changed);
        assert!(r.steps[0].contains("restarted WirePlumber"));
        assert_eq!(r.state.ok(), Some(true));
        assert!(rig.sys.did(&["systemctl", "--user", "restart", "wireplumber.service"]));
        assert_eq!(rig.sys.state().profile, HIFI_SPEAKERS);
    }

    #[test]
    fn repair_just_picks_the_profile_when_the_card_still_offers_it() {
        let rig = Rig::new(true);
        rig.sys.state().offered =
            ["off", HIFI_HEADPHONES, HIFI_SPEAKERS, "pro-audio"].map(str::to_owned).to_vec();
        let r = rig.audio.repair(quick()).unwrap();
        assert!(r.fixed);
        assert_eq!(r.steps, ["selected the speaker profile"]);
        assert!(!rig.sys.did(&["systemctl"]), "no restart needed");
    }

    #[test]
    fn repair_reports_honestly_when_a_restart_does_not_help() {
        let rig = Rig::new(true);
        rig.sys.state().restart_fixes = false;
        let r = rig
            .audio
            .repair(RepairOptions { timeout: Duration::from_millis(300), poll: Duration::from_millis(10) })
            .unwrap();
        assert!(!r.fixed && r.changed && !r.steps.is_empty());
        assert_eq!(r.state.ok(), Some(false));
        assert!(
            !rig.sys.sent().iter().any(|c| c.starts_with("pactl set-card-profile") && c.contains("HiFi")),
            "it never forces a profile the card does not offer"
        );
    }

    #[test]
    fn a_restart_that_brings_back_only_the_profiles_is_followed_by_picking_one() {
        // After the restart WirePlumber offers HiFi again but does not activate it.
        let rig = Rig::new(true);
        {
            let mut s = rig.sys.state();
            s.restart_fixes = false;
            s.restart_offers_only = true;
        }
        let r = rig.audio.repair(quick()).unwrap();
        assert!(r.fixed, "{r:?}");
        assert_eq!(r.steps.len(), 2, "{:?}", r.steps);
        assert!(r.steps[0].contains("restarted WirePlumber"));
        assert_eq!(r.steps[1], "selected the speaker profile");
    }

    #[test]
    fn the_repair_never_touches_the_codec() {
        let rig = Rig::new(true);
        rig.audio.repair(quick()).unwrap();
        assert!(rig.sys.sent().iter().all(|c| !c.to_lowercase().contains("reconfig")
            && !c.starts_with("amixer")
            && !c.starts_with("hda-verb")
            && !c.starts_with("tee")));
    }

    #[test]
    fn the_login_service_repairs_a_missing_output_once_and_leaves_a_healthy_one_alone() {
        let stop = AtomicBool::new(false);
        let rig = Rig::new(true);
        rig.audio.autorepair(&[Duration::from_secs(1), Duration::from_secs(2)], &stop);
        assert_eq!(rig.sys.state().profile, HIFI_SPEAKERS);
        assert_eq!(rig.shown(), ["audio: Sound output repaired"]);
        assert_eq!(rig.sys.count(&["systemctl"]), 1, "repaired once, not again at the second moment");
        assert_eq!(rig.clock.sleeps.lock().unwrap()[0], Duration::from_secs(1));
        assert!(rig.log_lines()[0].starts_with("sound output was missing: restarted WirePlumber"));
        assert!(rig.log_lines()[0].ends_with(": FIXED"));

        let healthy = Rig::new(false);
        healthy.audio.autorepair(&[Duration::from_secs(1), Duration::from_secs(2)], &stop);
        assert!(healthy.shown().is_empty());
        assert!(!healthy.sys.did(&["systemctl"]));
        assert_eq!(healthy.clock.sleeps.lock().unwrap().len(), 1, "a healthy card is checked once");
    }

    #[test]
    fn the_second_check_runs_only_when_the_first_could_not_judge() {
        let stop = AtomicBool::new(false);
        let rig = Rig::new(false);
        rig.audio.autorepair(&[Duration::from_secs(20), Duration::from_secs(90)], &stop);
        assert_eq!(rig.clock.sleeps.lock().unwrap().clone(), [Duration::from_secs(20)]);

        let rig = Rig::new(false);
        rig.sys.state().card_missing_from_long_list = true;
        rig.audio.autorepair(&[Duration::from_secs(20), Duration::from_secs(90)], &stop);
        assert_eq!(
            rig.clock.sleeps.lock().unwrap().clone(),
            [Duration::from_secs(20), Duration::from_secs(70)],
            "delays are measured from the start"
        );
        assert_eq!(rig.log_lines().len(), 2);
        assert!(rig.log_lines()[0].starts_with("sound check failed: card "));
    }

    #[test]
    fn a_stop_request_ends_the_login_service_before_it_acts() {
        let stop = AtomicBool::new(true);
        let rig = Rig::new(true);
        rig.audio.autorepair(&DEFAULT_REPAIR_DELAYS, &stop);
        assert!(rig.sys.sent().iter().all(|c| !c.starts_with("systemctl")));
        assert_eq!(DEFAULT_REPAIR_DELAYS, [Duration::from_secs(20), Duration::from_secs(90)]);
    }

    #[test]
    fn a_repair_that_does_not_work_says_so_and_stays_quiet() {
        let stop = AtomicBool::new(false);
        let rig = Rig::new(true);
        rig.sys.state().restart_fixes = false;
        rig.audio.autorepair(&[Duration::from_secs(1)], &stop);
        assert!(rig.shown().is_empty());
        assert!(rig.log_lines()[0].ends_with(": still missing"), "{:?}", rig.log_lines());
    }
}
