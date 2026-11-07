//! The machine side of the PREDATOR-key actions and the whole-setup profiles.
//!
//! The action runner (`ts_domain::actions`) reaches the laptop through five small traits. One
//! value implements them all, over an [`Api`]: a key press, a command and a profile then go
//! through the very same routing rules (the daemon when it runs, the firmware otherwise, a fan is
//! never stopped), which is the point of having one application layer.
//!
//! That `Api` is a second, private one. The runner is part of the `Services` of the public `Api`
//! (`Services::actions`), so it cannot be built over that one; the private one gets the same
//! shared hardware handles and a runner that refuses everything, which is enough because none of
//! the methods used here runs an action (the actions call these methods, never the other way).
#![forbid(unsafe_code)]

use std::fmt::Display;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;

use serde_json::Value;
use ts_domain::actions::{
    ActionError, ActionRunner, AudioFacade, DaemonFacade, FanCommand, FanSnapshot, HostError, HostResult,
    LightingFacade, LocalTouchpad, PowerFacade, RearState, RearWrite, Spawner, TouchpadAction,
};
use ts_domain::audio::{DtsScope, Output, VoiceDevice, VoiceMode};
use ts_platform::hw::{Coded, PowerMode, RearMode, RearSettings, Rgb};
use ts_system::touchpad::Touchpad;

use crate::api::Api;

/// A failure as the text a key press shows.
fn host(e: impl Display) -> HostError {
    HostError::new(e.to_string())
}

/// What the private `Api` is given as its action runner: nothing can run an action through it.
pub(crate) struct NoActions;

impl ActionRunner for NoActions {
    fn run(&self, _spec: &str) -> Result<String, ActionError> {
        Err(ActionError::Unavailable("actions are not available here".to_owned()))
    }
}

/// The five machine-side traits of the actions, over an [`Api`].
pub(crate) struct ApiFacade(pub(crate) Arc<Api>);

impl ApiFacade {
    fn s(&self) -> &crate::api::Services {
        self.0.services()
    }
}

impl PowerFacade for ApiFacade {
    fn mode(&self) -> HostResult<String> {
        self.s().hw.get_mode().map(|m| m.to_string()).map_err(host)
    }

    fn supported_modes(&self) -> HostResult<Vec<String>> {
        let modes = self.s().hw.supported_modes().map_err(host)?;
        Ok(modes.iter().map(|m| m.name().to_owned()).collect())
    }

    fn set_mode(&self, mode: &str, hold_minutes: f64) -> HostResult<String> {
        let wanted = PowerMode::from_name(mode).map_err(host)?;
        let hold = (hold_minutes > 0.0).then_some(hold_minutes);
        self.0.set_power(wanted, hold).map(|m| m.name().to_owned()).map_err(host)
    }

    fn fan_state(&self) -> HostResult<FanSnapshot> {
        let state = self.s().hw.fan_state().map_err(host)?;
        let percent = |v: i64| u8::try_from(v.clamp(0, 100)).unwrap_or(0);
        Ok(FanSnapshot {
            mode: state.mode.to_string(),
            cpu: percent(state.duty_register_pct.cpu),
            gpu: percent(state.duty_register_pct.gpu),
        })
    }

    fn set_fan(&self, command: FanCommand) -> HostResult<()> {
        // A `FanCommand` cannot ask for a stop (the duties are 20..=100) and `confirm` stays
        // false, so this path can never write a 0.
        match command {
            FanCommand::Smart => self.0.fan_smart(),
            FanCommand::Auto => self.0.fan_auto(),
            FanCommand::Max => self.0.fan_max(),
            FanCommand::Manual { cpu, gpu } => self.0.fan_set(i64::from(cpu), Some(i64::from(gpu)), false),
        }
        .map(drop)
        .map_err(host)
    }

    fn battery_limit(&self) -> HostResult<bool> {
        self.0.battery_limit().map_err(host)
    }

    fn set_battery_limit(&self, on: bool) -> HostResult<bool> {
        self.0.set_battery_limit(on).map_err(host)
    }

    fn usb_charge(&self) -> HostResult<i64> {
        self.0.usb_charge().map_err(host)
    }

    fn set_usb_charge(&self, level: i64) -> HostResult<i64> {
        self.0.set_usb_charge(level).map_err(host)
    }

    fn overdrive(&self) -> HostResult<Option<bool>> {
        self.0.overdrive().map_err(host)
    }

    fn set_overdrive(&self, on: bool) -> HostResult<bool> {
        self.0.set_overdrive(on).map_err(host)
    }

    fn boot_sound(&self) -> HostResult<bool> {
        self.0.boot_sound().map_err(host)
    }

    fn set_boot_sound(&self, on: bool) -> HostResult<bool> {
        self.0.set_boot_sound(on).map_err(host)
    }

    fn keyboard_timeout(&self) -> HostResult<bool> {
        // A firmware that cannot say is "off" for a profile: it has nothing to restore.
        Ok(self.0.keyboard_timeout().map_err(host)?.unwrap_or(false))
    }

    fn set_keyboard_timeout(&self, on: bool) -> HostResult<bool> {
        self.0.set_keyboard_timeout(on).map_err(host)
    }
}

impl DaemonFacade for ApiFacade {
    fn setting(&self, key: &str) -> HostResult<Option<Value>> {
        Ok(self.0.daemon_state().map_err(host)?.settings.get(key).cloned())
    }

    fn set_setting(&self, key: &str, value: Value) -> HostResult<()> {
        self.0.daemon_set(key, value).map(drop).map_err(host)
    }

    fn fan_curve_name(&self) -> HostResult<String> {
        self.0.fan_curve_in_use().map(|c| c.name).map_err(host)
    }

    fn use_fan_curve(&self, name: &str) -> HostResult<()> {
        self.0.use_fan_curve(name).map(drop).map_err(host)
    }

    fn touchpad(&self, action: TouchpadAction) -> HostResult<bool> {
        self.0.ask("touchpad", ts_ipc::wire::Args::new().with("action", action.name())).map_err(host)
    }
}

/// The touchpad switch for a root process with no daemon.
pub(crate) struct DirectTouchpad(pub(crate) Touchpad);

impl LocalTouchpad for DirectTouchpad {
    fn enabled(&self) -> bool {
        self.0.enabled().unwrap_or(false)
    }

    fn set_enabled(&self, on: bool) -> HostResult<bool> {
        self.0.set_enabled(on).map_err(host)
    }
}

impl LightingFacade for ApiFacade {
    fn keyboard_on(&self) -> HostResult<()> {
        self.s().keyboard.on().map_err(host)
    }

    fn keyboard_off(&self) -> HostResult<()> {
        self.s().keyboard.off().map_err(host)
    }

    fn keyboard_restore_default(&self) -> HostResult<()> {
        self.s().keyboard.restore_default().map_err(host)
    }

    fn keyboard_brightness(&self) -> HostResult<i64> {
        self.s().keyboard.get_brightness().map(i64::from).map_err(host)
    }

    fn keyboard_effect(&self, effect: &str, color: &str, speed: i64, brightness: i64) -> HostResult<()> {
        // Direction 0 is what the Python edition's `set_effect` defaulted to.
        self.s().keyboard.set_effect(effect, color, speed, brightness, 0).map_err(host)
    }

    fn keyboard_profile_exists(&self, name: &str) -> bool {
        self.s().lighting.exists(name)
    }

    fn keyboard_profile_apply(&self, name: &str) -> HostResult<()> {
        self.s().lighting.apply(name, None).map_err(host)
    }

    fn look_names(&self) -> Vec<String> {
        self.s().looks.names()
    }

    fn look_apply(&self, name: &str) -> HostResult<()> {
        let look = self.s().looks.load(name).map_err(host)?;
        self.s().looks.apply(&look).map(drop).map_err(host)
    }

    fn rear(&self) -> HostResult<RearState> {
        let rear = self.s().hw.rear_get().map_err(host)?;
        Ok(RearState {
            mode: rear.mode.to_string(),
            color: rear.color.to_string(),
            speed: i64::from(rear.speed),
            brightness: i64::from(rear.brightness),
            // A firmware that did not read the direction back is shown as 1, the Python default.
            direction: i64::from(rear.direction.unwrap_or(1)),
        })
    }

    fn set_rear(&self, write: &RearWrite) -> HostResult<()> {
        let zones = write
            .zones
            .as_ref()
            .map(|z| z.iter().map(|c| Rgb::parse(c)).collect::<Result<Vec<_>, _>>())
            .transpose()
            .map_err(host)?;
        let settings = RearSettings {
            mode: RearMode::from_name(&write.mode).map_err(host)?,
            color: Rgb::parse(&write.color).map_err(host)?,
            speed: write.speed,
            brightness: write.brightness,
            direction: write.direction,
            zones,
            verify: true,
        };
        self.s().hw.rear_set(&settings).map(drop).map_err(host)
    }
}

impl AudioFacade for ApiFacade {
    fn output_toggle(&self) -> HostResult<()> {
        self.s().audio.output_toggle().map(drop).map_err(host)
    }

    fn output_cycle(&self) -> HostResult<()> {
        self.s().audio.output_cycle().map(drop).map_err(host)
    }

    fn set_output(&self, output: &str) -> HostResult<()> {
        let which: Output = output.parse().map_err(host)?;
        self.s().audio.set_output(which).map(drop).map_err(host)
    }

    fn active_output(&self) -> HostResult<String> {
        self.s().audio.output_status().map(|o| o.active).map_err(host)
    }

    fn mic_toggle(&self) -> HostResult<bool> {
        Ok(self.s().audio.mic_toggle(true))
    }

    fn set_mic_mute(&self, muted: bool) -> HostResult<()> {
        self.s().audio.mic_set_mute(muted, true);
        Ok(())
    }

    fn voice_next(&self) -> HostResult<(String, String)> {
        let (device, mode) = self.s().audio.voice_next(None).map_err(host)?;
        self.s().audio.apply_mic_policy(false);
        Ok((device.as_str().to_owned(), mode.as_str().to_owned()))
    }

    fn voice_modes(&self) -> HostResult<(String, String)> {
        let modes = self.s().audio.voice_modes();
        let name = |m: Option<VoiceMode>| m.map_or("?", VoiceMode::as_str).to_owned();
        Ok((name(modes.headset), name(modes.array)))
    }

    fn voice_set(&self, device: &str, mode: &str) -> HostResult<()> {
        let device: VoiceDevice = device.parse().map_err(host)?;
        let mode: VoiceMode = mode.parse().map_err(host)?;
        self.s().audio.voice_set(device, mode).map_err(host)?;
        self.s().audio.apply_mic_policy(false);
        Ok(())
    }

    fn dts_mode(&self, name: &str) -> HostResult<()> {
        self.s().audio.dts_mode(name, DtsScope::Configured).map(drop).map_err(host)
    }
}

/// Starts `exec:` commands: through `/bin/sh -c`, as the calling user, in a process group of its
/// own, with no input and no output. A thread reaps the child so a finished command never stays a
/// zombie in a long-running key service; the command outlives the key press (`exec:truesense gui`).
#[derive(Debug, Clone, Copy, Default)]
pub struct ShellSpawner;

impl Spawner for ShellSpawner {
    fn spawn(&self, command: &str) -> HostResult<()> {
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg(command)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // Its own group: a Ctrl-C meant for the key service must not reach the command, and
            // the command must survive the service.
            .process_group(0)
            .spawn()
            .map_err(|e| HostError::new(format!("cannot run '{command}': {e}")))?;
        thread::Builder::new()
            .name("exec-reaper".to_owned())
            .spawn(move || {
                let _ = child.wait();
            })
            .map(drop)
            .map_err(|e| HostError::new(format!("cannot watch '{command}': {e}")))
    }
}
