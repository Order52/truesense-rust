//! The self-test's view of the daemon, the keyboard and the sound system.
//!
//! `ts_system::selftest` sets, reads back and restores every firmware feature, but it cannot see
//! the layers above it, so it takes three small traits. These adapters answer them from an
//! [`Api`]: the self-test then pauses the daemon's automation, reads the keyboard and flips the
//! microphone through exactly the operations a command would use.
#![forbid(unsafe_code)]

use ts_ipc::wire::Args;
use ts_system::selftest::{Audio, Automation, CallError, CpuInfo, Daemon, Keyboard};

use crate::api::Api;

/// The control socket as the self-test asks it.
pub struct SelftestDaemon<'a>(pub &'a Api);

impl Daemon for SelftestDaemon<'_> {
    fn automation(&self) -> Result<Automation, CallError> {
        let state = self.0.daemon_state().map_err(call_error)?;
        Ok(Automation {
            smart_fan: state.setting_bool("smart_fan").unwrap_or(false),
            auto_profile: state.setting_bool("auto_profile").unwrap_or(false),
        })
    }

    fn set_automation(&self, automation: Automation) -> Result<(), CallError> {
        self.0.daemon_set("smart_fan", automation.smart_fan).map_err(call_error)?;
        self.0.daemon_set("auto_profile", automation.auto_profile).map_err(call_error)?;
        Ok(())
    }

    fn cpu_info(&self) -> Result<CpuInfo, CallError> {
        let value = self.0.ask_value("cpu_info", Args::new()).map_err(call_error)?;
        // The daemon rounds nothing: watts may come as `80.0`.
        let watts = |key: &str| value.get(key).and_then(serde_json::Value::as_f64).map(|w| w.round() as i64);
        match (watts("pl1_w"), watts("pl2_w")) {
            (Some(pl1_w), Some(pl2_w)) => {
                Ok(CpuInfo { pl1_w, pl2_w, user: value.get("user").is_some_and(|u| !u.is_null()) })
            }
            _ => Err(CallError::new("cpu_info: the service did not report PL1 and PL2")),
        }
    }
}

/// The keyboard controller as the self-test asks it.
pub struct SelftestKeyboard<'a>(pub &'a Api);

impl Keyboard for SelftestKeyboard<'_> {
    fn brightness(&self) -> Result<i64, CallError> {
        self.0.keyboard_brightness().map(i64::from).map_err(call_error)
    }
}

/// The sound system as the self-test asks it.
pub struct SelftestAudio<'a>(pub &'a Api);

impl Audio for SelftestAudio<'_> {
    fn output_active(&self) -> Result<String, CallError> {
        self.0.audio_output().map(|o| o.active).map_err(call_error)
    }

    fn headset_boost(&self) -> Result<Option<u8>, CallError> {
        Ok(self.0.headset_boost())
    }

    fn set_headset_boost(&self, level: u8) -> Result<u8, CallError> {
        self.0.set_headset_boost(level).map_err(call_error)
    }

    fn mic_muted(&self) -> Result<bool, CallError> {
        Ok(self.0.mic().muted)
    }

    fn set_mic_mute(&self, muted: bool) -> Result<(), CallError> {
        // Every input, so a muted test leaves no microphone live.
        self.0.set_mic_mute(muted, false);
        Ok(())
    }
}

/// A failure as the text the self-test shows (and recognises as "unknown command").
fn call_error(e: impl std::fmt::Display) -> CallError {
    CallError::new(e.to_string())
}
