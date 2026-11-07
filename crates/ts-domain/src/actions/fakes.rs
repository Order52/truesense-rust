//! A fake machine for the tests of this module and of the profile bundles: one object
//! implementing every facade trait, recording each call in order so a test can assert both
//! the effect and the sequence.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::err_expect,
    clippy::field_reassign_with_default
)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;

use serde_json::{Map, Value};

use super::facade::{
    ActionSettings, ActionState, AudioFacade, DaemonFacade, FanCommand, FanSnapshot, FavouritesFacade,
    HostError, HostResult, LightingFacade, LocalTouchpad, PowerFacade, ProfilesFacade, RearState, RearWrite,
    SettingsSource, Spawner,
};
use super::parse::TouchpadAction;
use super::runner::Env;
use crate::favourites::Favourite;

/// The state of the fake machine; every field is behind a lock so the traits can take `&self`.
pub(crate) struct World {
    pub log: Mutex<Vec<String>>,
    pub mode: Mutex<String>,
    pub supported: Mutex<Vec<String>>,
    pub fan: Mutex<FanSnapshot>,
    pub battery: Mutex<bool>,
    pub usb: Mutex<i64>,
    pub overdrive: Mutex<Option<bool>>,
    pub boot_sound: Mutex<bool>,
    pub kb_timeout: Mutex<bool>,
    pub daemon_up: Mutex<bool>,
    pub daemon: Mutex<BTreeMap<String, Value>>,
    pub curve: Mutex<String>,
    pub touchpad_on: Mutex<bool>,
    pub kb_brightness: Mutex<i64>,
    pub kb_profiles: Mutex<Vec<String>>,
    pub looks: Mutex<Vec<String>>,
    pub rear: Mutex<RearState>,
    pub output: Mutex<String>,
    pub mic_muted: Mutex<bool>,
    pub voice: Mutex<(String, String)>,
    pub state: Mutex<Map<String, Value>>,
    pub hold: Mutex<f64>,
    pub cycle: Mutex<Vec<String>>,
    pub fail: Mutex<Vec<String>>,
    pub profile_names: Mutex<Vec<String>>,
    pub favs: Mutex<Vec<Favourite>>,
}

impl World {
    pub(crate) fn new() -> Arc<World> {
        let s = |x: &str| x.to_owned();
        Arc::new(World {
            log: Mutex::new(Vec::new()),
            mode: Mutex::new(s("balanced")),
            supported: Mutex::new(["quiet", "balanced", "performance", "turbo", "eco"].map(s).to_vec()),
            fan: Mutex::new(FanSnapshot { mode: s("auto"), cpu: 40, gpu: 35 }),
            battery: Mutex::new(false),
            usb: Mutex::new(0),
            overdrive: Mutex::new(Some(false)),
            boot_sound: Mutex::new(true),
            kb_timeout: Mutex::new(false),
            daemon_up: Mutex::new(true),
            daemon: Mutex::new(BTreeMap::new()),
            curve: Mutex::new(s("truesense")),
            touchpad_on: Mutex::new(true),
            kb_brightness: Mutex::new(50),
            kb_profiles: Mutex::new(Vec::new()),
            looks: Mutex::new(vec![s("native")]),
            rear: Mutex::new(RearState {
                mode: s("static"),
                color: s("112233"),
                speed: 3,
                brightness: 80,
                direction: 1,
            }),
            output: Mutex::new(s("speakers")),
            mic_muted: Mutex::new(false),
            voice: Mutex::new((s("headset"), s("off"))),
            state: Mutex::new(Map::new()),
            hold: Mutex::new(0.0),
            cycle: Mutex::new(["eco", "quiet", "balanced", "performance", "turbo"].map(s).to_vec()),
            fail: Mutex::new(Vec::new()),
            profile_names: Mutex::new(vec![s("a"), s("b"), s("c")]),
            favs: Mutex::new(Vec::new()),
        })
    }

    pub(crate) fn env(self: &Arc<World>) -> Env {
        Env {
            settings: self.clone(),
            power: self.clone(),
            daemon: self.clone(),
            lighting: self.clone(),
            audio: self.clone(),
            state: self.clone(),
            profiles: self.clone(),
            local_touchpad: None,
            spawner: Some(self.clone()),
        }
    }

    pub(crate) fn calls(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    pub(crate) fn clear(&self) {
        self.log.lock().unwrap().clear();
    }

    /// Makes the named operation fail (by the name it logs under).
    pub(crate) fn fail(&self, op: &str) {
        self.fail.lock().unwrap().push(op.to_owned());
    }

    pub(crate) fn state_get(&self, key: &str) -> Option<Value> {
        self.state.lock().unwrap().get(key).cloned()
    }

    pub(crate) fn state_put(&self, key: &str, v: Value) {
        self.state.lock().unwrap().insert(key.to_owned(), v);
    }

    fn rec(&self, entry: String) -> HostResult<()> {
        let op = entry.split([' ', ':']).next().unwrap_or("").to_owned();
        self.log.lock().unwrap().push(entry);
        if self.fail.lock().unwrap().contains(&op) {
            Err(HostError::new(format!("{op} failed")))
        } else {
            Ok(())
        }
    }

    fn daemon_check(&self, entry: String) -> HostResult<()> {
        self.rec(entry)?;
        if *self.daemon_up.lock().unwrap() { Ok(()) } else { Err(HostError::new("no daemon")) }
    }
}

impl SettingsSource for World {
    fn settings(&self) -> HostResult<ActionSettings> {
        if self.fail.lock().unwrap().iter().any(|f| f == "settings") {
            return Err(HostError::new("config broken"));
        }
        Ok(ActionSettings {
            hold_minutes: *self.hold.lock().unwrap(),
            power_cycle: self.cycle.lock().unwrap().clone(),
        })
    }
}

impl PowerFacade for World {
    fn mode(&self) -> HostResult<String> {
        self.rec("mode?".into())?;
        Ok(self.mode.lock().unwrap().clone())
    }
    fn supported_modes(&self) -> HostResult<Vec<String>> {
        Ok(self.supported.lock().unwrap().clone())
    }
    fn set_mode(&self, mode: &str, hold_minutes: f64) -> HostResult<String> {
        self.rec(format!("power:{mode}:{hold_minutes}"))?;
        *self.mode.lock().unwrap() = mode.to_owned();
        Ok(mode.to_owned())
    }
    fn fan_state(&self) -> HostResult<FanSnapshot> {
        self.rec("fan_state?".into())?;
        Ok(self.fan.lock().unwrap().clone())
    }
    fn set_fan(&self, command: FanCommand) -> HostResult<()> {
        self.rec(format!("fan:{command:?}"))?;
        let mut f = self.fan.lock().unwrap();
        match command {
            FanCommand::Smart => f.mode = "auto".into(),
            FanCommand::Auto => f.mode = "auto".into(),
            FanCommand::Max => f.mode = "max".into(),
            FanCommand::Manual { cpu, gpu } => {
                f.mode = "manual".into();
                f.cpu = cpu;
                f.gpu = gpu;
            }
        }
        Ok(())
    }
    fn battery_limit(&self) -> HostResult<bool> {
        self.rec("battery?".into())?;
        Ok(*self.battery.lock().unwrap())
    }
    fn set_battery_limit(&self, on: bool) -> HostResult<bool> {
        self.rec(format!("battery:{on}"))?;
        *self.battery.lock().unwrap() = on;
        Ok(on)
    }
    fn usb_charge(&self) -> HostResult<i64> {
        Ok(*self.usb.lock().unwrap())
    }
    fn set_usb_charge(&self, level: i64) -> HostResult<i64> {
        self.rec(format!("usb:{level}"))?;
        *self.usb.lock().unwrap() = level;
        Ok(level)
    }
    fn overdrive(&self) -> HostResult<Option<bool>> {
        self.rec("overdrive?".into())?;
        Ok(*self.overdrive.lock().unwrap())
    }
    fn set_overdrive(&self, on: bool) -> HostResult<bool> {
        self.rec(format!("overdrive:{on}"))?;
        *self.overdrive.lock().unwrap() = Some(on);
        Ok(on)
    }
    fn boot_sound(&self) -> HostResult<bool> {
        Ok(*self.boot_sound.lock().unwrap())
    }
    fn set_boot_sound(&self, on: bool) -> HostResult<bool> {
        self.rec(format!("bootsound:{on}"))?;
        *self.boot_sound.lock().unwrap() = on;
        Ok(on)
    }
    fn keyboard_timeout(&self) -> HostResult<bool> {
        Ok(*self.kb_timeout.lock().unwrap())
    }
    fn set_keyboard_timeout(&self, on: bool) -> HostResult<bool> {
        self.rec(format!("kbtimeout:{on}"))?;
        *self.kb_timeout.lock().unwrap() = on;
        Ok(on)
    }
}

impl DaemonFacade for World {
    fn setting(&self, key: &str) -> HostResult<Option<Value>> {
        if !*self.daemon_up.lock().unwrap() {
            return Err(HostError::new("no daemon"));
        }
        Ok(self.daemon.lock().unwrap().get(key).cloned())
    }
    fn set_setting(&self, key: &str, value: Value) -> HostResult<()> {
        self.daemon_check(format!("daemon_set:{key}={value}"))?;
        self.daemon.lock().unwrap().insert(key.to_owned(), value);
        Ok(())
    }
    fn fan_curve_name(&self) -> HostResult<String> {
        if !*self.daemon_up.lock().unwrap() {
            return Err(HostError::new("no daemon"));
        }
        Ok(self.curve.lock().unwrap().clone())
    }
    fn use_fan_curve(&self, name: &str) -> HostResult<()> {
        self.daemon_check(format!("curve:{name}"))?;
        *self.curve.lock().unwrap() = name.to_owned();
        Ok(())
    }
    fn touchpad(&self, action: TouchpadAction) -> HostResult<bool> {
        self.daemon_check(format!("touchpad_daemon:{}", action.name()))?;
        let mut on = self.touchpad_on.lock().unwrap();
        *on = match action {
            TouchpadAction::On => true,
            TouchpadAction::Off => false,
            TouchpadAction::Toggle => !*on,
        };
        Ok(*on)
    }
}

impl LocalTouchpad for World {
    fn enabled(&self) -> bool {
        *self.touchpad_on.lock().unwrap()
    }
    fn set_enabled(&self, on: bool) -> HostResult<bool> {
        self.rec(format!("touchpad_local:{on}"))?;
        *self.touchpad_on.lock().unwrap() = on;
        Ok(on)
    }
}

impl LightingFacade for World {
    fn keyboard_on(&self) -> HostResult<()> {
        self.rec("kb_on".into())
    }
    fn keyboard_off(&self) -> HostResult<()> {
        self.rec("kb_off".into())
    }
    fn keyboard_restore_default(&self) -> HostResult<()> {
        self.rec("kb_default".into())
    }
    fn keyboard_brightness(&self) -> HostResult<i64> {
        Ok(*self.kb_brightness.lock().unwrap())
    }
    fn keyboard_effect(&self, effect: &str, color: &str, speed: i64, brightness: i64) -> HostResult<()> {
        self.rec(format!("kb_effect:{effect}:{color}:{speed}:{brightness}"))
    }
    fn keyboard_profile_exists(&self, name: &str) -> bool {
        self.kb_profiles.lock().unwrap().iter().any(|p| p == name)
    }
    fn keyboard_profile_apply(&self, name: &str) -> HostResult<()> {
        self.rec(format!("kb_profile:{name}"))?;
        if self.keyboard_profile_exists(name) {
            Ok(())
        } else {
            Err(HostError::new(format!("no profile '{name}'")))
        }
    }
    fn look_names(&self) -> Vec<String> {
        self.looks.lock().unwrap().clone()
    }
    fn look_apply(&self, name: &str) -> HostResult<()> {
        self.rec(format!("look:{name}"))
    }
    fn rear(&self) -> HostResult<RearState> {
        Ok(self.rear.lock().unwrap().clone())
    }
    fn set_rear(&self, w: &RearWrite) -> HostResult<()> {
        let zones = w.zones.as_ref().map_or(String::new(), |z| format!(":{}", z.join(",")));
        self.rec(format!(
            "rear_set:{}:{}:{}:{}:{}{zones}",
            w.mode, w.color, w.speed, w.brightness, w.direction
        ))?;
        *self.rear.lock().unwrap() = RearState {
            mode: w.mode.clone(),
            color: w.color.clone(),
            speed: w.speed,
            brightness: w.brightness,
            direction: w.direction,
        };
        Ok(())
    }
}

impl AudioFacade for World {
    fn output_toggle(&self) -> HostResult<()> {
        self.rec("output_toggle".into())
    }
    fn output_cycle(&self) -> HostResult<()> {
        self.rec("output_cycle".into())
    }
    fn set_output(&self, output: &str) -> HostResult<()> {
        self.rec(format!("output:{output}"))?;
        *self.output.lock().unwrap() = output.to_owned();
        Ok(())
    }
    fn active_output(&self) -> HostResult<String> {
        if self.fail.lock().unwrap().iter().any(|f| f == "audio_read") {
            return Err(HostError::new("no sound"));
        }
        Ok(self.output.lock().unwrap().clone())
    }
    fn mic_toggle(&self) -> HostResult<bool> {
        self.rec("mic_toggle".into())?;
        let mut m = self.mic_muted.lock().unwrap();
        *m = !*m;
        Ok(*m)
    }
    fn set_mic_mute(&self, muted: bool) -> HostResult<()> {
        self.rec(format!("mic:{muted}"))?;
        *self.mic_muted.lock().unwrap() = muted;
        Ok(())
    }
    fn voice_next(&self) -> HostResult<(String, String)> {
        self.rec("voice_next".into())?;
        let mut v = self.voice.lock().unwrap();
        v.1 = match v.1.as_str() {
            "off" => "personal".into(),
            "personal" => "conference".into(),
            _ => "off".into(),
        };
        Ok(v.clone())
    }
    fn voice_modes(&self) -> HostResult<(String, String)> {
        Ok((self.voice.lock().unwrap().1.clone(), "off".into()))
    }
    fn voice_set(&self, device: &str, mode: &str) -> HostResult<()> {
        self.rec(format!("voice:{device}:{mode}"))
    }
    fn dts_mode(&self, name: &str) -> HostResult<()> {
        self.rec(format!("dts:{name}"))
    }
}

impl ProfilesFacade for World {
    fn apply(&self, name: &str) -> HostResult<String> {
        self.rec(format!("profile_apply:{name}"))?;
        Ok(format!("Profile '{name}' applied (0 settings)"))
    }
    fn next_name(&self) -> HostResult<String> {
        let names = self.profile_names.lock().unwrap().clone();
        let last = self.state.lock().unwrap().get("profile").and_then(Value::as_str).map(str::to_owned);
        let at = last.and_then(|l| names.iter().position(|n| *n == l));
        names
            .get(at.map_or(0, |i| (i + 1) % names.len()))
            .cloned()
            .ok_or_else(|| HostError::new("no profiles saved"))
    }
}

impl FavouritesFacade for World {
    fn favourites(&self) -> HostResult<Vec<Favourite>> {
        self.rec("favourites?".into())?;
        Ok(self.favs.lock().unwrap().clone())
    }
    fn use_smart_profile(&self, name: &str) -> HostResult<()> {
        self.rec(format!("smart:{name}"))
    }
}

impl Spawner for World {
    fn spawn(&self, command: &str) -> HostResult<()> {
        self.rec(format!("exec:{command}"))
    }
}

impl ActionState for World {
    fn get(&self, key: &str) -> Option<Value> {
        self.state.lock().unwrap().get(key).cloned()
    }
    fn set(&self, key: &str, value: Value) -> HostResult<()> {
        self.state.lock().unwrap().insert(key.to_owned(), value);
        Ok(())
    }
}
