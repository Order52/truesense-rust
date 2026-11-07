//! Executing actions against the traits of [`super::facade`].
//!
//! The decision logic of each action (which mode comes next, what the light bar comes back
//! as, which effect follows) lives here and is pure given the facade answers; the facades do
//! the touching. The messages are the Python edition's, because they are what the on-screen
//! display shows.
#![forbid(unsafe_code)]

use std::sync::Arc;

use serde_json::{Value, json};

use super::facade::{
    ActionSettings, ActionState, AudioFacade, DaemonFacade, FanCommand, FavouritesFacade, HostError,
    LightingFacade, LocalTouchpad, PowerFacade, ProfilesFacade, RearState, RearWrite, SettingsSource,
    Spawner, truthy,
};
use super::parse::{Action, KB_EFFECTS, REAR_EFFECTS, Switch, TouchpadAction, Validator};
use super::{ActionError, ActionRunner};
use crate::favourites::{Favourite, Kind, next_position};

/// What a rear light comes back as when nothing was remembered.
const REAR_DEFAULT_COLOR: &str = "00aec7";
/// The colour the keyboard effect keys use (the operating colour of the Predator design).
const KB_EFFECT_COLOR: &str = "00aec7";
/// Speed of an effect started by the key.
const KB_EFFECT_SPEED: i64 = 5;
/// A keyboard effect started by the key is never dimmer than this when the backlight was off,
/// and the neon effect is invisible below it.
const KB_EFFECT_MIN_BRIGHTNESS: i64 = 30;
/// What `power-next` walks through when none of the configured modes is supported.
const DEFAULT_CYCLE: [&str; 3] = ["eco", "balanced", "turbo"];

/// Everything the actions reach. All fields are required except `local_touchpad` (absent
/// unless the process is root) and `spawner` (absent refuses `exec:` actions).
#[derive(Clone)]
pub struct Env {
    /// Configuration values, read on every action.
    pub settings: Arc<dyn SettingsSource>,
    /// Firmware features.
    pub power: Arc<dyn PowerFacade>,
    /// The control socket.
    pub daemon: Arc<dyn DaemonFacade>,
    /// Keyboard, light bar, looks.
    pub lighting: Arc<dyn LightingFacade>,
    /// Sound.
    pub audio: Arc<dyn AudioFacade>,
    /// The per-user memory.
    pub state: Arc<dyn ActionState>,
    /// Whole-setup profiles.
    pub profiles: Arc<dyn ProfilesFacade>,
    /// Direct touchpad switch for a root process without a daemon.
    pub local_touchpad: Option<Arc<dyn LocalTouchpad>>,
    /// Starts `exec:` commands.
    pub spawner: Option<Arc<dyn Spawner>>,
}

/// Executes actions against an [`Env`].
pub struct Runner {
    env: Env,
    validator: Validator,
    favourites: Option<Arc<dyn FavouritesFacade>>,
}

/// Where `favourite-next` remembers the position it reached (an index into the list), in the
/// user state like the other `*-next` actions.
const FAVOURITE_KEY: &str = "favourite";

fn host(op: &'static str) -> impl Fn(HostError) -> ActionError {
    move |source| ActionError::Host { op, source }
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

fn mic_text(muted: bool) -> &'static str {
    if muted { "Microphones: MUTED" } else { "Microphones: live" }
}

/// The label of a noise-reduction mode; a mode we have no label for is shown as it is.
fn voice_label(mode: &str) -> &str {
    match mode {
        "off" => "All Off",
        "personal" => "Personal Call",
        "conference" => "Conference Call",
        other => other,
    }
}

impl Runner {
    /// A runner over `env`. `look:NAME` actions are checked against `env.lighting`'s looks.
    pub fn new(env: Env) -> Runner {
        let lighting = Arc::clone(&env.lighting);
        let validator = Validator::with_looks(Arc::new(move || lighting.look_names()));
        Runner { env, validator, favourites: None }
    }

    /// Gives the runner the user's favourites, which `favourite:N` and `favourite-next` need.
    /// Without them those actions answer that favourites are not available (a process that has
    /// no favourites file, such as a bare test environment).
    #[must_use]
    pub fn with_favourites(mut self, favourites: Arc<dyn FavouritesFacade>) -> Runner {
        self.favourites = Some(favourites);
        self
    }

    /// Checks an action the way [`Runner::run`] will, saved looks included. Configuration code
    /// uses it so that a bad action is refused when it is set.
    pub fn validate(&self, spec: &str) -> Result<(), ActionError> {
        self.validator.validate(spec)
    }

    /// The validator of this runner (it knows the saved looks), for code that stores actions.
    pub fn validator(&self) -> &Validator {
        &self.validator
    }

    /// Validates and executes one action; returns the message to show (empty for none). It is
    /// validated first so a spec that went stale (a look deleted since) fails with a clear
    /// message instead of half running.
    pub fn run(&self, spec: &str) -> Result<String, ActionError> {
        let action = self.validator.parse(spec)?;
        self.run_action(&action)
    }

    /// Executes an already parsed action.
    pub fn run_action(&self, action: &Action) -> Result<String, ActionError> {
        match action {
            Action::None => Ok(String::new()),
            Action::PowerNext => self.power_step(None, 1),
            Action::PowerPrev => self.power_step(None, -1),
            Action::PowerCycle(list) => self.power_step(Some(list), 1),
            Action::Power(mode) => self.set_power(mode),
            Action::FanSmart => self.fan(FanCommand::Smart, "Fans: smart curve"),
            Action::FanAuto => self.fan(FanCommand::Auto, "Fans: firmware auto"),
            Action::FanMax => self.fan(FanCommand::Max, "Fans: MAX"),
            Action::FanManual(n) => {
                self.fan(FanCommand::Manual { cpu: *n, gpu: *n }, &format!("Fans: {n} %"))
            }
            Action::FanMaxToggle => {
                let state = self.env.power.fan_state().map_err(host("fan"))?;
                if state.mode == "max" {
                    self.fan(FanCommand::Smart, "Fans: smart curve")
                } else {
                    self.fan(FanCommand::Max, "Fans: MAX")
                }
            }
            Action::SmartFanToggle => self.smart_fan_toggle(),
            Action::Look(name) => {
                self.env.lighting.look_apply(name).map_err(host("look"))?;
                Ok(format!("Look: {name}"))
            }
            Action::Auto(mode) => self.set_auto(mode.name()),
            Action::AutoNext => {
                let current = self.env.daemon.setting("auto_mode").map_err(host("mode control"))?;
                let name = current.as_ref().and_then(Value::as_str).unwrap_or("off");
                self.set_auto(super::parse::AutoMode::next_after(name).name())
            }
            Action::Profile(name) => self.env.profiles.apply(name).map_err(host("profile")),
            Action::ProfileNext => {
                let next = self.env.profiles.next_name().map_err(host("profile"))?;
                self.env.profiles.apply(&next).map_err(host("profile"))
            }
            Action::Favourite(n) => self.favourite(*n),
            Action::FavouriteNext => self.favourite_next(),
            Action::OutputToggle => {
                self.env.audio.output_toggle().map_err(host("audio output"))?;
                Ok(String::new())
            }
            Action::OutputCycle => {
                self.env.audio.output_cycle().map_err(host("audio output"))?;
                Ok(String::new())
            }
            Action::Output(o) => {
                self.env.audio.set_output(o.name()).map_err(host("audio output"))?;
                Ok(format!("Audio output: {}", o.label()))
            }
            Action::MicToggle => {
                let muted = self.env.audio.mic_toggle().map_err(host("microphone"))?;
                Ok(mic_text(muted).to_owned())
            }
            Action::Mic(mute) => {
                self.env.audio.set_mic_mute(*mute).map_err(host("microphone"))?;
                Ok(mic_text(*mute).to_owned())
            }
            Action::VoiceNext => {
                let (device, mode) = self.env.audio.voice_next().map_err(host("voice"))?;
                Ok(format!("Purified Voice ({device}): {}", voice_label(&mode)))
            }
            Action::TouchpadToggle => self.touchpad(TouchpadAction::Toggle),
            Action::Touchpad(a) => self.touchpad(*a),
            Action::Keyboard(sw) => self.keyboard(*sw),
            Action::KeyboardEffectNext => self.keyboard_effect_next(),
            Action::KeyboardProfile(name) => {
                self.env.lighting.keyboard_profile_apply(name).map_err(host("keyboard profile"))?;
                self.remember("keyboard_profile", json!(name));
                self.remember("keyboard_on", json!(true));
                Ok(format!("Keyboard profile: {name}"))
            }
            Action::Rear(sw) => self.rear(*sw),
            Action::RearEffectNext => self.rear_effect_next(),
            Action::BatteryLimitToggle => {
                let cur = self.env.power.battery_limit().map_err(host("battery limit"))?;
                let now = self.env.power.set_battery_limit(!cur).map_err(host("battery limit"))?;
                Ok(format!("Battery 80 % limit: {}", on_off(now)))
            }
            Action::OverdriveToggle => {
                let cur = self.env.power.overdrive().map_err(host("overdrive"))?;
                // Unknown counts as off, so the press turns it on.
                let now = self.env.power.set_overdrive(!cur.unwrap_or(false)).map_err(host("overdrive"))?;
                Ok(format!("LCD overdrive: {}", on_off(now)))
            }
            Action::Notify(text) => Ok(text.clone()),
            Action::Exec(command) => match &self.env.spawner {
                Some(s) => {
                    s.spawn(command).map_err(host("exec"))?;
                    Ok(String::new())
                }
                None => Err(ActionError::Unavailable("exec: actions are not available here".into())),
            },
        }
    }

    fn favourites(&self) -> Result<(&dyn FavouritesFacade, Vec<Favourite>), ActionError> {
        let facade = self
            .favourites
            .as_deref()
            .ok_or_else(|| ActionError::Unavailable("favourites are not available here".into()))?;
        let list = facade.favourites().map_err(host("favourites"))?;
        Ok((facade, list))
    }

    /// Applies the favourite at 1-based position `n`. The message says what was applied, so the
    /// on-screen display tells which star the key reached.
    fn favourite(&self, n: usize) -> Result<String, ActionError> {
        let (facade, list) = self.favourites()?;
        match n.checked_sub(1).and_then(|i| list.get(i)) {
            Some(fav) => self.apply_favourite(facade, fav, n, list.len()),
            None => Err(ActionError::Invalid(missing_favourite(n, list.len()))),
        }
    }

    /// Applies the favourite after the one reached last. The position is remembered before the
    /// apply, so a favourite whose item was deleted fails once and the next press moves on
    /// instead of sticking on it.
    fn favourite_next(&self) -> Result<String, ActionError> {
        let (facade, list) = self.favourites()?;
        let previous = self.env.state.get(FAVOURITE_KEY).as_ref().and_then(as_int);
        let Some(at) = next_position(previous, list.len()) else {
            return Err(ActionError::Invalid(missing_favourite(1, 0)));
        };
        self.remember(FAVOURITE_KEY, json!(at));
        match list.get(at) {
            Some(fav) => self.apply_favourite(facade, fav, at + 1, list.len()),
            None => Err(ActionError::Invalid(missing_favourite(at + 1, list.len()))),
        }
    }

    /// One favourite through the same calls the library pages and the matching key actions use.
    /// A failure is the machine side's own text (for a deleted item, the library's "no ..."
    /// message), so a dangling favourite fails like any other bad name.
    fn apply_favourite(
        &self,
        facade: &dyn FavouritesFacade,
        fav: &Favourite,
        position: usize,
        total: usize,
    ) -> Result<String, ActionError> {
        let name = fav.name.as_str();
        match fav.kind {
            Kind::Look => self.env.lighting.look_apply(name).map_err(host("look"))?,
            Kind::FanCurve => self.env.daemon.use_fan_curve(name).map_err(host("fan curve"))?,
            Kind::SmartProfile => facade.use_smart_profile(name).map_err(host("smart profile"))?,
            Kind::Profile => {
                self.env.profiles.apply(name).map_err(host("profile"))?;
            }
        }
        Ok(format!("Favourite {position}/{total}: {} {name}", fav.kind.label()))
    }

    fn settings(&self) -> Result<ActionSettings, ActionError> {
        self.env.settings.settings().map_err(|e| ActionError::Host {
            op: "configuration",
            source: HostError::new(format!("read configuration: {e}")),
        })
    }

    /// Moves to the next or previous mode of a list: the configured cycle limited to the modes
    /// this laptop supports, or the list the action names. A mode that is not in the list
    /// starts the walk at its beginning (next) or one before its end (previous), as the Python
    /// edition did.
    fn power_step(&self, list: Option<&[String]>, step: i64) -> Result<String, ActionError> {
        let settings = self.settings()?;
        let order: Vec<String> = match list {
            Some(l) => l.to_vec(),
            None => {
                let supported = self.env.power.supported_modes().map_err(host("power"))?;
                let mut order: Vec<String> =
                    settings.power_cycle.iter().filter(|m| supported.contains(m)).cloned().collect();
                if order.is_empty() {
                    order = DEFAULT_CYCLE.iter().map(|m| (*m).to_owned()).collect();
                }
                order
            }
        };
        let current = self.env.power.mode().map_err(host("power"))?;
        let at = order.iter().position(|m| *m == current).and_then(|i| i64::try_from(i).ok()).unwrap_or(-1);
        let len = i64::try_from(order.len()).unwrap_or(1).max(1);
        let next = usize::try_from((at + step).rem_euclid(len)).unwrap_or(0);
        match order.get(next) {
            Some(mode) => self.set_power_held(mode, settings.hold_minutes),
            None => Err(ActionError::Invalid("power-cycle needs a comma list of power modes".into())),
        }
    }

    fn set_power(&self, mode: &str) -> Result<String, ActionError> {
        let settings = self.settings()?;
        self.set_power_held(mode, settings.hold_minutes)
    }

    fn set_power_held(&self, mode: &str, hold_minutes: f64) -> Result<String, ActionError> {
        let got = self.env.power.set_mode(mode, hold_minutes).map_err(host("power"))?;
        Ok(format!("Power mode: {got}"))
    }

    fn fan(&self, command: FanCommand, message: &str) -> Result<String, ActionError> {
        self.env.power.set_fan(command).map_err(host("fan"))?;
        Ok(message.to_owned())
    }

    fn smart_fan_toggle(&self) -> Result<String, ActionError> {
        // With no daemon the state is unknown and counts as off; the set below then reports
        // the missing daemon.
        let on = match self.env.daemon.setting("smart_fan") {
            Ok(Some(v)) => !truthy(&v),
            _ => true,
        };
        if self.env.daemon.set_setting("smart_fan", Value::Bool(on)).is_err() {
            return Err(ActionError::Unavailable("the TrueSense daemon is not running".into()));
        }
        Ok(if on { "Fans: smart curve" } else { "Fans: smart curve OFF" }.to_owned())
    }

    fn set_auto(&self, value: &str) -> Result<String, ActionError> {
        self.env.daemon.set_setting("auto_mode", json!(value)).map_err(host("mode control"))?;
        Ok(format!("Mode control: {value}"))
    }

    /// Asks the daemon (the switch needs root); a root process without a daemon switches it
    /// itself.
    fn touchpad(&self, action: TouchpadAction) -> Result<String, ActionError> {
        let on = match self.env.daemon.touchpad(action) {
            Ok(on) => on,
            Err(e) => match &self.env.local_touchpad {
                None => return Err(host("touchpad")(e)),
                Some(local) => {
                    let want = match action {
                        TouchpadAction::On => true,
                        TouchpadAction::Off => false,
                        TouchpadAction::Toggle => !local.enabled(),
                    };
                    local.set_enabled(want).map_err(host("touchpad"))?
                }
            },
        };
        Ok(format!("Touchpad: {}", on_off(on)))
    }

    fn keyboard(&self, sw: Switch) -> Result<String, ActionError> {
        let lighting = &self.env.lighting;
        let next = sw.resolve(self.state_bool("keyboard_on", true));
        if !next {
            lighting.keyboard_off().map_err(host("keyboard"))?;
        } else if self.env.state.get("kb_restore").is_some_and(|v| truthy(&v)) {
            lighting.keyboard_on().map_err(host("keyboard"))?;
        } else {
            let profile = self.state_string("keyboard_profile", "default");
            if lighting.keyboard_profile_exists(&profile) {
                lighting.keyboard_profile_apply(&profile).map_err(host("keyboard"))?;
            } else {
                lighting.keyboard_restore_default().map_err(host("keyboard"))?;
            }
        }
        self.remember("keyboard_on", json!(next));
        Ok(format!("Keyboard light: {}", on_off(next)))
    }

    fn keyboard_effect_next(&self) -> Result<String, ActionError> {
        let index = self.next_index("kb_effect", KB_EFFECTS.len());
        self.remember("kb_effect", json!(index));
        let mut brightness = self.env.lighting.keyboard_brightness().map_err(host("keyboard"))?;
        if brightness == 0 {
            brightness = KB_EFFECT_MIN_BRIGHTNESS;
        }
        let effect = KB_EFFECTS.get(index).copied().unwrap_or("static");
        let level = if effect == "neon" { brightness.max(KB_EFFECT_MIN_BRIGHTNESS) } else { brightness };
        self.env
            .lighting
            .keyboard_effect(effect, KB_EFFECT_COLOR, KB_EFFECT_SPEED, level)
            .map_err(host("keyboard"))?;
        self.remember("keyboard_on", json!(true));
        Ok(format!("Keyboard effect: {effect}"))
    }

    fn rear(&self, sw: Switch) -> Result<String, ActionError> {
        let lighting = &self.env.lighting;
        let cur = lighting.rear().map_err(host("rear light"))?;
        let was_on = cur.brightness > 0;
        let next = sw.resolve(was_on);
        if was_on && !next {
            self.remember(
                "rear_last",
                json!({"mode": cur.mode, "color": cur.color, "speed": cur.speed, "brightness": cur.brightness, "direction": cur.direction}),
            );
        }
        let write = if next {
            let last = self.rear_last();
            let mode = known_rear_mode(&last.mode);
            RearWrite {
                zones: self.rear_zones(&mode),
                mode,
                color: last.color,
                speed: last.speed,
                brightness: if last.brightness == 0 { 100 } else { last.brightness },
                direction: last.direction,
            }
        } else {
            let mode = known_rear_mode(&cur.mode);
            RearWrite {
                zones: self.rear_zones(&mode),
                mode,
                color: cur.color,
                speed: cur.speed,
                brightness: 0,
                direction: cur.direction,
            }
        };
        lighting.set_rear(&write).map_err(host("rear light"))?;
        Ok(format!("Rear light: {}", on_off(next)))
    }

    fn rear_effect_next(&self) -> Result<String, ActionError> {
        let index = self.next_index("rear_effect", REAR_EFFECTS.len());
        self.remember("rear_effect", json!(index));
        let cur = self.env.lighting.rear().map_err(host("rear light"))?;
        let effect = REAR_EFFECTS.get(index).copied().unwrap_or("static");
        let write = RearWrite {
            mode: effect.to_owned(),
            color: cur.color,
            speed: cur.speed,
            brightness: if cur.brightness == 0 { 100 } else { cur.brightness },
            direction: cur.direction,
            zones: None,
        };
        self.env.lighting.set_rear(&write).map_err(host("rear light"))?;
        Ok(format!("Rear effect: {effect}"))
    }

    /// What the light bar was before it was switched off, so switching it on restores it.
    /// Missing or mistyped fields of a hand-edited state fall back to the defaults one by one.
    fn rear_last(&self) -> RearState {
        let mut last = RearState {
            mode: "static".into(),
            color: REAR_DEFAULT_COLOR.into(),
            speed: 3,
            brightness: 100,
            direction: 1,
        };
        let Some(Value::Object(m)) = self.env.state.get("rear_last") else { return last };
        if let Some(v) = m.get("mode").and_then(Value::as_str) {
            last.mode = v.to_owned();
        }
        if let Some(v) = m.get("color").and_then(Value::as_str) {
            last.color = v.to_owned();
        }
        for (key, slot) in [
            ("speed", &mut last.speed),
            ("brightness", &mut last.brightness),
            ("direction", &mut last.direction),
        ] {
            if let Some(v) = m.get(key).and_then(as_int) {
                *slot = v;
            }
        }
        last
    }

    /// The three part colours of the static effect that were set last, kept through on and off
    /// so a multi-coloured bar comes back as it was. Only the static effect has parts.
    fn rear_zones(&self, mode: &str) -> Option<[String; 3]> {
        if mode != "static" {
            return None;
        }
        let Some(Value::Array(list)) = self.env.state.get("rear_zones") else { return None };
        let colours: Vec<String> = list.iter().filter_map(|z| z.as_str().map(str::to_owned)).collect();
        <[String; 3]>::try_from(colours).ok().filter(|_| list.len() == 3)
    }

    /// The index after the one remembered under `key`, wrapping at `len`.
    fn next_index(&self, key: &str, len: usize) -> usize {
        let len = i64::try_from(len).unwrap_or(1).max(1);
        let previous = self.env.state.get(key).as_ref().and_then(as_int).unwrap_or(-1);
        usize::try_from((previous.saturating_add(1)).rem_euclid(len)).unwrap_or(0)
    }

    /// Saves a convenience value; a read-only disk is not an error for an action.
    fn remember(&self, key: &str, value: Value) {
        let _ = self.env.state.set(key, value);
    }

    fn state_bool(&self, key: &str, default: bool) -> bool {
        self.env.state.get(key).map_or(default, |v| truthy(&v))
    }

    fn state_string(&self, key: &str, default: &str) -> String {
        match self.env.state.get(key) {
            Some(Value::String(s)) => s,
            _ => default.to_owned(),
        }
    }
}

impl ActionRunner for Runner {
    fn run(&self, spec: &str) -> Result<String, ActionError> {
        Runner::run(self, spec)
    }
}

/// What a key press says when the favourite it names is not there.
fn missing_favourite(position: usize, total: usize) -> String {
    match total {
        0 => "no favourites yet (star one: truesense favourite add look NAME)".to_owned(),
        1 => format!("no favourite {position}: you have 1"),
        _ => format!("no favourite {position}: you have {total}"),
    }
}

/// A rear mode the firmware knows, else `static` (a number as text, or a stale name).
fn known_rear_mode(mode: &str) -> String {
    if REAR_EFFECTS.contains(&mode) { mode.to_owned() } else { "static".to_owned() }
}

/// A JSON number as a whole number (the user state is JSON, where 3.0 may come back for 3).
fn as_int(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_f64().filter(|f| f.is_finite() && f.abs() < 1e15).map(|f| f as i64))
}
