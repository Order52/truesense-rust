//! The DTS sound (AcerUltraX). TrueSense only drives `acerultrax-cli`; it does not bundle Acer's
//! files.
//!
//! The runner gives back what the program printed, not its exit status (see the module docs of
//! [`audio`](super)), so a `acerultrax-cli` that exits with an error shows up here as an empty or
//! unexpected answer instead of the Python edition's error with the last line of its message; only
//! a program that cannot be started or times out is an [`AudioError::Dts`].

use ts_foundation::config::Table;
use ts_foundation::fsx;
use ts_foundation::userstate::keys::DTS_SEEN;

use super::parse::{DtsDevice, is_headset_label, parse_dts_devices};
use super::{Audio, AudioError, DTS_TIMEOUT, forbidden};

/// The largest AcerUltraX config file that is read (it holds a few EQ curves).
const MAX_DTS_CONFIG: u64 = 1 << 20;

/// Which outputs a DTS mode choice goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DtsScope {
    /// What `[audio] dts_all` says (on unless changed).
    Configured,
    /// Every output.
    Everywhere,
    /// Only the one playing now.
    CurrentOnly,
}

/// A profile name must be one word for `acerultrax-cli`: not empty, not an option (`--device`), no
/// control characters. The Python edition passed anything on.
fn valid_profile_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('-') && !name.chars().any(char::is_control)
}

impl Audio {
    /// Runs `acerultrax-cli` with `args` and returns its trimmed output.
    pub fn dts(&self, args: &[&str]) -> Result<String, AudioError> {
        let binary = (self.dts_locator)().ok_or(AudioError::DtsNotInstalled)?;
        let program = binary.to_string_lossy();
        if let Some(why) = forbidden(&program, args) {
            return Err(AudioError::Dts(format!("refused: {why}")));
        }
        let out = self
            .runner
            .output(&program, args, DTS_TIMEOUT)
            .map_err(|e| AudioError::Dts(format!("acerultrax-cli: {e}")))?;
        Ok(String::from_utf8_lossy(&out).trim().to_owned())
    }

    /// AcerUltraX's own status text.
    pub fn dts_status(&self) -> Result<String, AudioError> {
        self.dts(&["status"])
    }

    /// Switches DTS on or off.
    pub fn dts_power(&self, on: bool) -> Result<String, AudioError> {
        self.dts(&["power", if on { "on" } else { "off" }])
    }

    /// Every output AcerUltraX knows, from `acerultrax-cli devices`.
    pub fn dts_devices(&self) -> Result<Vec<DtsDevice>, AudioError> {
        Ok(parse_dts_devices(&self.dts(&["devices"])?))
    }

    /// Whether AcerUltraX holds a headphone EQ for this output (its config file; a missing file or
    /// key simply means no).
    fn dts_has_eq(&self, reference: &str) -> bool {
        let Ok(bytes) = fsx::read_limited(&self.paths.dts_config, MAX_DTS_CONFIG) else { return false };
        let Ok(text) = String::from_utf8(bytes) else { return false };
        text.parse::<Table>()
            .ok()
            .and_then(|t| t.get("headphones").and_then(|h| h.as_table()).map(|h| h.contains_key(reference)))
            .unwrap_or(false)
    }

    /// Picks a DTS profile. With `[audio] dts_all` (default on) it goes to EVERY output (speakers,
    /// jack, USB and Bluetooth headsets, controllers), not just the one playing now, so switching the
    /// output never lands on an unprocessed one.
    pub fn dts_mode(&self, name: &str, scope: DtsScope) -> Result<String, AudioError> {
        if !valid_profile_name(name) {
            return Err(AudioError::Invalid("DTS profile names are one word, not an option".into()));
        }
        let out = self.dts(&["mode", name])?;
        let everywhere = match scope {
            DtsScope::Everywhere => true,
            DtsScope::CurrentOnly => false,
            DtsScope::Configured => self.cfg().audio.dts_all,
        };
        if everywhere {
            for device in self.dts_devices()? {
                if !device.current && !device.profile.eq_ignore_ascii_case(name) {
                    self.dts(&["mode", name, "--device", &device.reference])?;
                }
            }
        }
        Ok(out)
    }

    /// Gives every output DTS. AcerUltraX leaves anything it does not know (a USB headset, a
    /// controller, a dock) on the Plain profile, i.e. no processing at all, so a headset worked on
    /// the jack and did nothing over USB. Each output is handled ONCE (remembered in the user state),
    /// so a later "Plain" chosen on purpose stays. New headsets get the profile of the laptop
    /// speakers plus a headphone EQ: AcerUltraX's own pick by name when it knows the model, else
    /// Acer's general one. `force` redoes every output. Returns what changed, for example
    /// `["Headset PC38X: Music + headphone EQ"]`.
    pub fn dts_cover(&self, force: bool) -> Result<Vec<String>, AudioError> {
        let devices = self.dts_devices()?;
        let mut seen: std::collections::BTreeSet<String> = self
            .state
            .as_ref()
            .and_then(|s| s.get_as::<Vec<String>>(DTS_SEEN))
            .unwrap_or_default()
            .into_iter()
            .collect();
        let speakers = devices.iter().find(|d| d.kind == "speakers").map_or("Music", |d| d.profile.as_str());
        let plain_or_off = |profile: &str| profile == "Plain" || profile == "Off";
        // The profile the speakers have, unless they have none on purpose: speakers on Plain/Off
        // mean the user wants no DTS.
        let want: Option<String> = devices
            .iter()
            .find(|d| d.kind == "speakers" && !plain_or_off(&d.profile))
            .map(|d| d.profile.clone())
            .or_else(|| force.then(|| "Music".to_owned()));
        let mut done = Vec::new();
        for device in &devices {
            let first = !seen.contains(&device.reference);
            if !(first || force)
                || device.kind == "speakers"
                || device.kind == "headphones"
                || (device.kind == "bluetooth-speaker" && !force)
            {
                continue; // AcerUltraX keeps Plain for Bluetooth speakers on purpose
            }
            seen.insert(device.reference.clone());
            let Some(want) = &want else { continue };
            if !plain_or_off(&device.profile) && !force {
                continue; // a profile that was chosen stays
            }
            let mut bits = Vec::new();
            if device.profile != *want {
                self.dts(&["mode", want, "--device", &device.reference])?;
                bits.push(want.clone());
            }
            if device.kind != "bluetooth-speaker" && is_headset_label(&device.label) {
                // "auto" is a no-op when AcerUltraX does not recognise the name; then Acer's general EQ.
                for pick in ["auto", "default"] {
                    if !self.dts_has_eq(&device.reference) {
                        self.dts(&["headphone", pick, "--device", &device.reference])?;
                    }
                }
                bits.push("headphone EQ".to_owned());
            }
            if !bits.is_empty() {
                done.push(format!("{}: {}", device.label, bits.join(" + ")));
            }
        }
        if let Some(state) = &self.state {
            // A convenience file: failing to save it is not an error.
            let _ = state.set_as(DTS_SEEN, &seen.into_iter().collect::<Vec<_>>());
        }
        if done.is_empty() && force {
            return Ok(vec![format!("every output is already on {speakers}")]);
        }
        Ok(done)
    }

    /// The headset-like outputs left on the Plain profile (no DTS processing): the doctor's view.
    pub fn plain_headsets(&self) -> Result<Vec<String>, AudioError> {
        Ok(self
            .dts_devices()?
            .into_iter()
            .filter(|d| d.profile == "Plain" && d.kind != "bluetooth-speaker" && is_headset_label(&d.label))
            .map(|d| d.label)
            .collect())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::audio::testing::*;

    fn profile_of(rig: &Rig, reference: &str) -> Option<String> {
        rig.sys.state().dts_profile.get(reference).cloned()
    }

    fn mode_calls(rig: &Rig) -> usize {
        rig.sys.count(&["/x/acerultrax-cli", "mode"])
    }

    #[test]
    fn without_acerultrax_every_call_says_so_and_runs_nothing() {
        let rig = Rig::new(false);
        rig.dts_installed.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(!rig.audio.has_dts());
        for result in [
            rig.audio.dts_status(),
            rig.audio.dts_power(true),
            rig.audio.dts_mode("Music", DtsScope::Configured),
            rig.audio.dts_devices().map(|_| String::new()),
        ] {
            assert!(matches!(result, Err(AudioError::DtsNotInstalled)));
        }
        assert!(rig.audio.dts_cover(false).is_err() && rig.audio.plain_headsets().is_err());
        assert!(rig.sys.sent().is_empty());
        assert!(
            rig.audio.dts_status().unwrap_err().to_string().contains("https://github.com/fawaz7/AcerUltraX")
        );
    }

    #[test]
    fn status_and_power_pass_through() {
        let rig = Rig::new(false);
        assert_eq!(rig.audio.dts_status().unwrap(), "ok");
        assert_eq!(rig.audio.dts_power(true).unwrap(), "ok");
        rig.audio.dts_power(false).unwrap();
        assert!(
            rig.sys.did(&["/x/acerultrax-cli", "status"])
                && rig.sys.did(&["/x/acerultrax-cli", "power", "on"])
                && rig.sys.did(&["/x/acerultrax-cli", "power", "off"])
        );
        rig.sys.state().dts_fail.insert("status".into());
        assert!(matches!(rig.audio.dts_status(), Err(AudioError::Dts(_))));
    }

    #[test]
    fn a_plain_usb_headset_gets_the_speakers_profile_and_a_headphone_eq_once() {
        let rig = Rig::new(false);
        let done = rig.audio.dts_cover(false).unwrap();
        assert_eq!(
            done,
            [
                "DualSense wireless controller (PS5) Direct DualSense Wireless Controller: Music + headphone EQ",
                "Headset PC38X: Music + headphone EQ"
            ]
        );
        assert_eq!(profile_of(&rig, USB_HEADSET).as_deref(), Some("Music"));
        assert!(rig.sys.state().dts_eq.contains(USB_HEADSET));
        assert_eq!(profile_of(&rig, "bluez_output.AA.1"), None, "a Bluetooth speaker keeps Plain");
        rig.sys.clear();
        assert_eq!(rig.audio.dts_cover(false).unwrap(), Vec::<String>::new());
        assert_eq!(mode_calls(&rig), 0, "the second look changes nothing");
        let saved: Vec<String> = serde_json::from_str::<serde_json::Value>(&rig.read("user.json").unwrap())
            .unwrap()["dts_seen"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        assert!(saved.contains(&USB_HEADSET.to_owned()) && saved.contains(&DUALSENSE.to_owned()));
        assert!(saved.windows(2).all(|w| w[0] <= w[1]), "the list is stored sorted");
    }

    #[test]
    fn the_headphone_eq_is_not_asked_for_twice() {
        let rig = Rig::new(false);
        rig.audio.dts_cover(false).unwrap();
        // "auto" did not give an EQ in the fake, "default" did: both were asked, in that order.
        let sent = rig.sys.sent();
        let auto = sent
            .iter()
            .position(|c| c.contains("headphone auto --device") && c.contains("GeneralPlus"))
            .unwrap();
        let default = sent
            .iter()
            .position(|c| c.contains("headphone default --device") && c.contains("GeneralPlus"))
            .unwrap();
        assert!(auto < default);
        // When AcerUltraX already holds an EQ (its config says so), neither is asked.
        let rig = Rig::new(false);
        std::fs::write(
            rig.dir.join("acerultrax.toml"),
            format!("[headphones.\"{USB_HEADSET}\"]\ngain = 1\n[headphones.\"{DUALSENSE}\"]\n"),
        )
        .unwrap();
        rig.audio.dts_cover(false).unwrap();
        assert!(!rig.sys.did(&["/x/acerultrax-cli", "headphone"]), "{:?}", rig.sys.sent());
    }

    #[test]
    fn a_profile_chosen_on_purpose_later_is_left_alone() {
        let rig = Rig::new(false);
        rig.audio.dts_cover(false).unwrap();
        rig.sys.state().dts_profile.insert(USB_HEADSET.into(), "Plain".into());
        rig.sys.clear();
        rig.audio.dts_cover(false).unwrap();
        assert!(
            !rig.sys.did(&["/x/acerultrax-cli", "mode"]) && !rig.sys.did(&["/x/acerultrax-cli", "headphone"])
        );
    }

    #[test]
    fn speakers_on_plain_means_no_dts_so_nothing_is_changed() {
        let rig = Rig::new(false);
        rig.sys.state().dts_devices_text =
            Some(REAL_DEVICES.replacen("speakers           Music", "speakers           Plain", 1));
        assert_eq!(rig.audio.dts_cover(false).unwrap(), Vec::<String>::new());
        assert_eq!(mode_calls(&rig), 0);
    }

    #[test]
    fn force_redoes_every_output() {
        let rig = Rig::new(false);
        rig.audio.dts_cover(false).unwrap();
        rig.sys.state().dts_profile.insert(USB_HEADSET.into(), "Plain".into());
        let done = rig.audio.dts_cover(true).unwrap();
        assert!(!done.is_empty());
        assert_eq!(profile_of(&rig, USB_HEADSET).as_deref(), Some("Music"));
        assert_eq!(
            profile_of(&rig, "bluez_output.AA.1").as_deref(),
            Some("Music"),
            "force includes Bluetooth speakers"
        );
    }

    #[test]
    fn force_with_speakers_on_plain_uses_music_and_says_so_when_nothing_changed() {
        let rig = Rig::new(false);
        rig.sys.state().dts_devices_text =
            Some(REAL_DEVICES.replacen("speakers           Music", "speakers           Plain", 1));
        assert!(rig.audio.dts_cover(true).unwrap().iter().any(|l| l.contains("Music")));
        let rig = Rig::new(false);
        rig.audio.dts_cover(true).unwrap();
        rig.sys.clear();
        assert_eq!(rig.audio.dts_cover(true).unwrap().len(), 2, "the two headsets report their EQ again");
        assert!(
            !rig.sys.did(&["/x/acerultrax-cli", "headphone"]),
            "an EQ AcerUltraX already holds is not asked for again"
        );
        let rig = Rig::new(false);
        rig.sys.state().dts_devices_text =
            Some("  Laptop speakers   speakers  Music\n    @speakers\n".into());
        assert_eq!(rig.audio.dts_cover(true).unwrap(), ["every output is already on Music"]);
    }

    #[test]
    fn a_mode_choice_goes_to_every_output() {
        let rig = Rig::new(false);
        rig.audio.dts_mode("game1", DtsScope::Everywhere).unwrap();
        let sent = rig.sys.sent();
        let modes: Vec<&String> = sent.iter().filter(|c| c.contains(" mode ")).collect();
        assert_eq!(modes[0], "/x/acerultrax-cli mode game1");
        assert_eq!(
            modes.iter().filter(|c| c.contains("--device")).count(),
            4,
            "every other output, not the current one twice"
        );
        assert!(!modes.iter().any(|c| c.contains(USB_HEADSET)), "the current output got the first call");
    }

    #[test]
    fn the_scope_follows_the_setting_unless_told() {
        let rig = Rig::new(false);
        rig.audio.dts_mode("Voice", DtsScope::CurrentOnly).unwrap();
        assert_eq!(mode_calls(&rig), 1);
        rig.sys.clear();
        rig.audio.dts_mode("Voice", DtsScope::Configured).unwrap();
        assert!(mode_calls(&rig) > 1, "dts_all is on by default");
        rig.config().audio.dts_all = false;
        rig.sys.clear();
        rig.audio.dts_mode("Voice", DtsScope::Configured).unwrap();
        assert_eq!(mode_calls(&rig), 1);
        // An output already on the profile is not asked again (case-insensitively).
        rig.sys.clear();
        rig.audio.dts_mode("music", DtsScope::Everywhere).unwrap();
        assert!(
            !rig.sys
                .sent()
                .iter()
                .any(|c| c.contains("--device") && (c.contains("@speakers") || c.contains("@headphones"))),
            "{:?}",
            rig.sys.sent()
        );
    }

    #[test]
    fn hostile_profile_names_never_reach_the_program() {
        let rig = Rig::new(false);
        for bad in ["", "--device", "-x", "a\nb", "a\0b", "hwC0D0", "reconfig"] {
            assert!(rig.audio.dts_mode(bad, DtsScope::Everywhere).is_err(), "{bad:?}");
        }
        assert!(rig.sys.sent().is_empty(), "{:?}", rig.sys.sent());
        assert!(
            rig.audio.dts_mode("Custom Audio", DtsScope::CurrentOnly).is_ok(),
            "profile names may hold a space"
        );
    }

    #[test]
    fn a_failing_command_is_an_error_not_a_silent_success() {
        let rig = Rig::new(false);
        rig.sys.state().dts_fail.insert("devices".into());
        assert!(matches!(rig.audio.dts_cover(false), Err(AudioError::Dts(_))));
        assert!(rig.audio.dts_mode("Music", DtsScope::Everywhere).is_err());
        let rig = Rig::new(false);
        rig.sys.state().dts_fail.insert("headphone".into());
        assert!(rig.audio.dts_cover(false).is_err());
        assert!(rig.read("user.json").is_none(), "nothing is remembered when the cover failed half way");
    }

    #[test]
    fn plain_headsets_is_the_doctors_view() {
        let rig = Rig::new(false);
        assert_eq!(
            rig.audio.plain_headsets().unwrap(),
            ["DualSense wireless controller (PS5) Direct DualSense Wireless Controller", "Headset PC38X"]
        );
        rig.audio.dts_cover(false).unwrap();
        assert!(rig.audio.plain_headsets().unwrap().is_empty());
    }

    #[test]
    fn the_eq_lookup_survives_a_broken_config() {
        let rig = Rig::new(false);
        assert!(!rig.audio.dts_has_eq("x"), "no file");
        for (text, want) in [
            ("[headphones.x]\n", true),
            ("[headphones]\nx = 1\n", true),
            ("[headphones]\ny = 1\n", false),
            ("headphones = 5\n", false),
            ("[[[ not toml", false),
            ("", false),
        ] {
            std::fs::write(rig.dir.join("acerultrax.toml"), text).unwrap();
            assert_eq!(rig.audio.dts_has_eq("x"), want, "{text:?}");
        }
        std::fs::write(rig.dir.join("acerultrax.toml"), [0xff, 0xfe, 0x00]).unwrap();
        assert!(!rig.audio.dts_has_eq("x"));
    }
}
