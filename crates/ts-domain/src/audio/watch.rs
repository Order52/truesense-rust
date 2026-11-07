//! The logic of the audio watcher: a loop the login audio service runs. The service itself (the
//! process, its signals, its systemd unit) belongs to the system layer; this module is one pass of
//! the loop ([`MicWatcher::step`]) and the loop around it ([`MicWatcher::run`]).
//!
//! The watcher re-picks the default microphone whenever something relevant CHANGES: the headset is
//! plugged or unplugged, a filter appears or vanishes, or a mode changes. It is edge-triggered, so
//! it never fights a manual choice made in between (for example Shift+F5). It also gives every new
//! output (a USB headset, a controller) DTS once, and starts the login sound repair.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::Audio;
use super::sound::DEFAULT_REPAIR_DELAYS;
use super::voice::{VoiceDevice, VoiceMode};

/// The pause between two passes of the loop.
const TICK: Duration = Duration::from_secs(1);
/// PipeWire needs a moment to finish creating or removing nodes before the policy looks.
const SETTLE: Duration = Duration::from_millis(800);
/// AcerUltraX needs a moment to register a new output before it is covered.
const DTS_DELAY: Duration = Duration::from_millis(1500);

/// Everything the microphone policy depends on. The watcher applies the policy only when this
/// changes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Signature {
    jack_use: VoiceDevice,
    /// The Purified Voice nodes that exist, sorted.
    nodes: Vec<String>,
    /// Each input's configured mode.
    modes: [Option<VoiceMode>; 2],
}

impl Audio {
    fn signature(&self) -> Signature {
        let names = self.source_names();
        let mut nodes: Vec<String> =
            VoiceDevice::ALL.iter().map(|d| d.node()).filter(|n| names.contains(n)).collect();
        nodes.sort();
        let modes = self.voice_modes();
        Signature { jack_use: self.jack_use(None), nodes, modes: [modes.headset, modes.array] }
    }
}

/// Gives DTS to an output that appears after the first look (a USB headset plugged in, a
/// controller connected). The first look is only a baseline.
#[derive(Debug, Default)]
pub struct DtsWatcher {
    /// `None` until the first look.
    sinks: Option<BTreeSet<String>>,
}

impl DtsWatcher {
    /// A watcher that has not looked yet.
    pub fn new() -> Self {
        Self::default()
    }

    fn cover(audio: &Audio) -> bool {
        match audio.dts_cover(false) {
            Ok(lines) => {
                for line in lines {
                    audio.log(&format!("DTS: {line}"));
                }
                true
            }
            Err(e) => {
                audio.log(&format!("DTS cover failed: {e}"));
                false
            }
        }
    }

    /// Looks at the outputs once and covers the new ones. A cover that fails is retried on the next
    /// look (every output then looks new again).
    pub fn step(&mut self, audio: &Audio) {
        let now: BTreeSet<String> = audio.sinks().into_iter().collect();
        if let Some(before) = &self.sinks {
            if now.difference(before).next().is_some() && audio.has_dts() {
                audio.clock.sleep(DTS_DELAY); // let AcerUltraX register the new output first
                if !Self::cover(audio) {
                    self.sinks = Some(BTreeSet::new());
                    return;
                }
            }
        }
        self.sinks = Some(now);
    }
}

/// The microphone and DTS watcher: its state between two passes.
pub struct MicWatcher {
    audio: Arc<Audio>,
    last: Option<Signature>,
    tick: u8,
    dts: DtsWatcher,
}

impl MicWatcher {
    /// A watcher in its initial state: the first pass applies the policy.
    pub fn new(audio: Arc<Audio>) -> Self {
        Self { audio, last: None, tick: 0, dts: DtsWatcher::new() }
    }

    /// One pass of the loop; the caller waits a second between passes. Nothing in a pass can fail:
    /// a tool that does not answer reads as an empty answer, and the loop must outlive every tool
    /// hiccup.
    pub fn step(&mut self) {
        let audio = &self.audio;
        let signature = audio.signature();
        if self.last.as_ref() != Some(&signature) {
            audio.clock.sleep(SETTLE); // let PipeWire finish creating/removing nodes
            let jack_changed = self.last.as_ref().is_none_or(|l| l.jack_use != signature.jack_use);
            let notify = audio.cfg().audio.notify && jack_changed;
            let p = audio.apply_mic_policy(notify);
            audio.log(&format!(
                "microphone policy: device={} mode={} default_source={}",
                p.device,
                p.mode.map_or("?", VoiceMode::as_str),
                p.default_source
            ));
            self.last = Some(signature);
        }
        // The DTS check runs every second pass.
        self.tick = (self.tick + 1) % 2;
        if self.tick == 0 {
            self.dts.step(audio);
        }
    }

    /// The watch loop: starts the login sound repair, covers the DTS outputs once, and then watches
    /// until `stop` is set. It returns only after everything it started has ended; the repair's
    /// pauses end early too when the audio's clock is tied to the same flag.
    pub fn run(&mut self, stop: &AtomicBool) {
        let audio = Arc::clone(&self.audio);
        std::thread::scope(|scope| {
            scope.spawn(|| audio.autorepair(&DEFAULT_REPAIR_DELAYS, stop));
            if audio.has_dts() {
                DtsWatcher::cover(&audio); // logged inside; the watch goes on without it
            }
            while !stop.load(Ordering::Relaxed) {
                self.step();
                audio.clock.sleep(TICK);
            }
        });
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::audio::testing::*;

    fn watcher(rig: Rig) -> (MicWatcher, Rig) {
        (MicWatcher::new(rig.audio.clone()), rig)
    }

    #[test]
    fn the_first_pass_applies_the_policy_and_later_passes_leave_it_alone() {
        let (mut w, rig) = watcher(Rig::new(false));
        w.step();
        assert_eq!(rig.shown(), ["mic: Microphone: Mic array - All Off"]);
        assert!(
            rig.log_lines()[0]
                .starts_with("microphone policy: device=array mode=off default_source=alsa_input")
        );
        assert!(rig.clock.sleeps.lock().unwrap().contains(&SETTLE), "PipeWire gets its moment to settle");
        for _ in 0..5 {
            w.step();
        }
        assert_eq!(rig.shown().len(), 1, "an unchanged signature applies nothing");
        assert_eq!(rig.log_lines().iter().filter(|l| l.starts_with("microphone policy")).count(), 1);
    }

    #[test]
    fn a_manual_choice_between_changes_is_not_fought() {
        let (mut w, rig) = watcher(Rig::new(false));
        w.step();
        // Shift+F5: the user picks another source by hand.
        rig.sys.state().default_source = "bluez_input.AA.1".into();
        for _ in 0..4 {
            w.step();
        }
        assert_eq!(
            rig.sys.state().default_source,
            "bluez_input.AA.1",
            "edge-triggered: nothing changed, so nothing is re-applied"
        );
    }

    #[test]
    fn plugging_the_headset_applies_the_policy_with_a_notification() {
        let (mut w, rig) = watcher(Rig::new(false));
        w.step();
        rig.sys.state().headset_mic_jack = "on".into();
        w.step();
        assert_eq!(
            rig.shown(),
            ["mic: Microphone: Mic array - All Off", "mic: Microphone: Headset - All Off"]
        );
        assert_eq!(rig.sys.state().default_source, SRC_HEADSET);
    }

    #[test]
    fn a_new_filter_or_mode_applies_the_policy_silently() {
        let (mut w, rig) = watcher(Rig::new(false));
        w.step();
        rig.audio.voice_set(VoiceDevice::Array, VoiceMode::Conference).unwrap();
        w.step();
        assert_eq!(rig.sys.state().default_source, "truesense_pv_array");
        assert_eq!(rig.shown().len(), 1, "same jack: no notification");
        rig.audio.voice_set(VoiceDevice::Array, VoiceMode::Off).unwrap();
        w.step();
        assert_eq!(rig.sys.state().default_source, SRC_ARRAY);
    }

    #[test]
    fn notifications_can_be_switched_off() {
        let (mut w, rig) = watcher(Rig::new(false));
        rig.config().audio.notify = false;
        w.step();
        assert!(rig.shown().is_empty());
        assert_eq!(rig.log_lines().len(), 1);
    }

    #[test]
    fn a_pass_survives_tools_that_do_not_answer() {
        let (mut w, rig) = watcher(Rig::new(false));
        // No card, no sinks, no sources: every tool answers with nothing.
        rig.sys.state().card_present = false;
        rig.sys.state().no_jack_controls = true;
        for _ in 0..3 {
            w.step();
        }
        assert_eq!(rig.log_lines().len(), 1, "the policy was applied once and not again");
    }

    #[test]
    fn a_headset_plugged_in_later_gets_dts_by_itself() {
        let rig = Rig::new(false);
        let mut dts = DtsWatcher::new();
        dts.step(&rig.audio);
        assert_eq!(rig.sys.count(&["/x/acerultrax-cli", "mode"]), 0, "the first look is only a baseline");
        rig.sys.state().extra_sinks = vec![USB_HEADSET.into()];
        dts.step(&rig.audio);
        assert!(rig.sys.count(&["/x/acerultrax-cli", "mode"]) > 0, "the new output was covered");
        assert!(
            rig.clock.sleeps.lock().unwrap().contains(&DTS_DELAY),
            "AcerUltraX gets its moment to register the output"
        );
        assert!(rig.log_lines().iter().any(|l| l.starts_with("DTS: ")), "{:?}", rig.log_lines());
        rig.sys.clear();
        dts.step(&rig.audio);
        assert_eq!(rig.sys.count(&["/x/acerultrax-cli"]), 0, "nothing new, nothing done");
    }

    #[test]
    fn a_failed_cover_is_retried_on_the_next_look() {
        let rig = Rig::new(false);
        let mut dts = DtsWatcher::new();
        dts.step(&rig.audio);
        rig.sys.state().extra_sinks = vec![USB_HEADSET.into()];
        rig.sys.state().dts_fail.insert("devices".into());
        dts.step(&rig.audio);
        assert!(rig.log_lines().iter().any(|l| l.starts_with("DTS cover failed")));
        rig.sys.state().dts_fail.clear();
        dts.step(&rig.audio);
        assert!(rig.sys.count(&["/x/acerultrax-cli", "mode"]) > 0, "every output looked new again");
    }

    #[test]
    fn without_acerultrax_a_new_output_is_not_covered() {
        let rig = Rig::new(false);
        let mut dts = DtsWatcher::new();
        dts.step(&rig.audio);
        rig.dts_installed.store(false, Ordering::SeqCst);
        rig.sys.state().extra_sinks = vec![USB_HEADSET.into()];
        dts.step(&rig.audio);
        assert!(rig.sys.sent().iter().all(|c| !c.contains("acerultrax-cli")));
    }

    #[test]
    fn the_dts_check_runs_every_second_pass() {
        let (mut w, rig) = watcher(Rig::new(false));
        rig.sys.clear();
        w.step();
        let after_one = rig.sys.count(&["pactl", "list", "short", "sinks"]);
        w.step();
        let after_two = rig.sys.count(&["pactl", "list", "short", "sinks"]);
        assert!(after_two > after_one, "the second pass looked at the sinks for DTS");
        let before = after_two;
        w.step();
        assert_eq!(rig.sys.count(&["pactl", "list", "short", "sinks"]), before, "the third pass did not");
    }

    #[test]
    fn run_covers_dts_first_repairs_sound_and_stops_when_asked() {
        let rig = Rig::new(true); // no sound output: the login repair has work to do
        let stop = Arc::new(AtomicBool::new(false));
        let (flag, sys) = (stop.clone(), rig.sys.clone());
        // What the daemon's stop flag does: end the loop once the repair has happened.
        rig.clock.on_sleep(move |_| {
            if sys.state().restarted > 0 {
                flag.store(true, Ordering::SeqCst);
            }
        });
        let mut w = MicWatcher::new(rig.audio.clone());
        w.run(&stop);
        let logs = rig.log_lines();
        assert!(logs.iter().any(|l| l.starts_with("DTS: ")), "DTS is covered at the start: {logs:?}");
        assert!(
            logs.iter().any(|l| l.starts_with("sound output was missing") && l.ends_with("FIXED")),
            "the login repair ran: {logs:?}"
        );
        assert!(stop.load(Ordering::SeqCst));
    }

    #[test]
    fn run_returns_at_once_when_already_stopped() {
        let rig = Rig::new(false);
        let stop = AtomicBool::new(true);
        MicWatcher::new(rig.audio.clone()).run(&stop);
        assert!(rig.shown().is_empty(), "no pass was made");
    }
}
