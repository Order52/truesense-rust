//! Applying the login settings: the decisions, apart from the machine.
//!
//! [`Login`] walks the `[startup]` choices in the order the earlier editions did (profile first,
//! then a look, then the keyboard, then the rear light) and reaches the lights through
//! [`Lights`], so every branch is tested with a fake that can fail on demand. [`apply`] builds the
//! real [`Lights`] from the services of an [`Api`].
#![forbid(unsafe_code)]

use std::time::Duration;

use serde_json::Value;
use ts_api::api::{Api, ApiResult, Services};
use ts_domain::looks::{Look, Rear};
use ts_foundation::config::StartupConfig;
use ts_platform::hw::{Coded, RearMode, RearSettings, Rgb};

/// Everything applying the login settings touches. Errors are the machine's own words: they go
/// into the `FAILED (...)` line of the step.
pub trait Lights {
    /// Applies a whole-setup profile; answers its summary line.
    fn apply_profile(&self, name: &str) -> Result<String, String>;
    /// Reads a saved or built-in look.
    fn load_look(&self, name: &str) -> Result<Look, String>;
    /// Puts a look on the lights; answers one line per part that was applied.
    fn apply_look(&self, look: &Look) -> Result<Vec<String>, String>;
    /// Puts back the keyboard look that was set last; `None` when nothing is remembered.
    fn keyboard_last(&self) -> Result<Option<String>, String>;
    /// The stock keyboard look.
    fn keyboard_default(&self) -> Result<(), String>;
    /// Keyboard backlight off.
    fn keyboard_off(&self) -> Result<(), String>;
    /// Applies a saved per-key lighting profile, keeping the brightness the keyboard has.
    fn lighting_profile(&self, name: &str) -> Result<(), String>;
    /// Writes the rear light bar and reads it back.
    fn rear_set(&self, settings: &RearSettings) -> Result<(), String>;
    /// What the rear light was last set to (`rear_state` of the user state), as stored.
    fn rear_state(&self) -> Option<Value>;
    /// Waits between two attempts of a step.
    fn pause(&self, how_long: Duration);
}

/// How hard a login step tries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ApplyOptions {
    /// How many more times a failed keyboard or rear step is tried, [`ApplyOptions::delay`] apart:
    /// the keyboard may not have enumerated yet when the service starts at login. Zero tries
    /// once, as 0.3.x did. A profile is never retried (it is a whole setup, not a device wait).
    pub retries: u32,
    /// The wait between two attempts.
    pub delay: Duration,
}

/// The login settings being applied: one [`Login::run`] gives the report lines.
pub struct Login<'a> {
    lights: &'a dyn Lights,
    options: ApplyOptions,
    done: Vec<String>,
}

impl<'a> Login<'a> {
    /// A run over these lights.
    pub fn new(lights: &'a dyn Lights, options: ApplyOptions) -> Self {
        Self { lights, options, done: Vec::new() }
    }

    /// One step: `label: result`, `label: nothing saved` when the step answers an empty result,
    /// `label: FAILED (why)` when every attempt failed. Never stops the others.
    fn step(&mut self, label: &str, retry: bool, mut f: impl FnMut() -> Result<String, String>) {
        let attempts = 1 + if retry { self.options.retries } else { 0 };
        let mut outcome = Err(String::new());
        for attempt in 0..attempts {
            if attempt > 0 {
                self.lights.pause(self.options.delay);
            }
            outcome = f();
            if outcome.is_ok() {
                break;
            }
        }
        self.done.push(match outcome {
            Ok(r) if r.is_empty() => format!("{label}: nothing saved"),
            Ok(r) => format!("{label}: {r}"),
            Err(e) => format!("{label}: FAILED ({e})"),
        });
    }

    /// Applies `settings` and answers the lines, one per step.
    pub fn run(mut self, settings: &StartupConfig) -> Vec<String> {
        if !settings.profile.is_empty() {
            let lights = self.lights;
            self.step("profile", false, || lights.apply_profile(&settings.profile));
        }
        let mut keyboard = settings.keyboard.as_str();
        let mut rear = settings.rear.as_str();
        let keyboard_look = keyboard.strip_prefix("look:");
        let rear_look = rear.strip_prefix("look:");
        // A saved look sets keyboard and rear together. Like 0.3.x, one look is loaded (the
        // keyboard's when both are looks) and both parts are taken from it.
        if let Some(name) = keyboard_look.or(rear_look) {
            self.apply_look_parts(name, keyboard_look.is_some(), rear_look.is_some());
            if keyboard_look.is_some() {
                keyboard = "none";
            }
            if rear_look.is_some() {
                rear = "none";
            }
        }
        self.keyboard(keyboard);
        self.rear(rear);
        self.done
    }

    fn apply_look_parts(&mut self, name: &str, for_keyboard: bool, for_rear: bool) {
        let lights = self.lights;
        let look = match lights.load_look(name) {
            Ok(look) => look,
            Err(why) => {
                // 0.3.x let this escape and the whole login service failed; a missing look is
                // one failed step like any other.
                for label in [("keyboard", for_keyboard), ("rear", for_rear)] {
                    if label.1 {
                        self.done.push(format!("{}: FAILED ({why})", label.0));
                    }
                }
                return;
            }
        };
        let part = |keyboard: bool| Look {
            keyboard: if keyboard { look.keyboard.clone() } else { None },
            rear: if keyboard { None } else { look.rear.clone() },
        };
        if for_keyboard {
            let part = part(true);
            self.step("keyboard", true, || look_part(lights, &part));
        }
        if for_rear {
            let part = part(false);
            self.step("rear", true, || look_part(lights, &part));
        }
    }

    fn keyboard(&mut self, choice: &str) {
        let lights = self.lights;
        match choice {
            "last" => self.step("keyboard", true, || Ok(lights.keyboard_last()?.unwrap_or_default())),
            "default" => {
                self.step("keyboard", true, || lights.keyboard_default().map(|()| "stock look".to_owned()))
            }
            "off" => self.step("keyboard", true, || lights.keyboard_off().map(|()| "off".to_owned())),
            other => {
                if let Some(name) = other.strip_prefix("profile:") {
                    self.step("keyboard", true, || lights.lighting_profile(name).map(|()| other.to_owned()));
                }
            }
        }
    }

    fn rear(&mut self, choice: &str) {
        let lights = self.lights;
        match choice {
            "last" => self.step("rear", true, || rear_last(lights)),
            "default" => {
                let stock = rear_fixed(RearMode::Breathing, [0x37, 0x23, 0xc7], 100);
                self.step("rear", true, || lights.rear_set(&stock).map(|()| "stock look".to_owned()));
            }
            "off" => {
                let dark = rear_fixed(RearMode::Static, [0x00, 0xae, 0xc7], 0);
                self.step("rear", true, || lights.rear_set(&dark).map(|()| "off".to_owned()));
            }
            _ => {}
        }
    }
}

/// One half of a look; a look without that half leaves the lights alone.
fn look_part(lights: &dyn Lights, part: &Look) -> Result<String, String> {
    if part.is_empty() {
        return Ok("unchanged".to_owned());
    }
    Ok(lights.apply_look(part)?.into_iter().next().unwrap_or_else(|| "unchanged".to_owned()))
}

/// The two fixed rear settings: the stock look (Breathing `#3723c7`, brightness 100, as
/// PredatorSense leaves it on Windows) and dark (a static effect at brightness 0).
fn rear_fixed(mode: RearMode, color: [u8; 3], brightness: i64) -> RearSettings {
    RearSettings { mode, color: Rgb(color), speed: 3, brightness, direction: 1, zones: None, verify: true }
}

/// Puts the rear light back as the hardware layer remembered it. Nothing remembered, or something
/// that is not a setting we wrote, is "nothing saved"; a setting missing a number is a failure
/// (a brightness guessed as 0 would switch the bar off).
fn rear_last(lights: &dyn Lights) -> Result<String, String> {
    let Some(state) = lights.rear_state().and_then(|v| serde_json::from_value::<Rear>(v).ok()) else {
        return Ok(String::new());
    };
    let incomplete = || "the saved rear light setting is incomplete".to_owned();
    let (Some(speed), Some(brightness)) = (state.speed, state.brightness) else {
        return Err(incomplete());
    };
    if state.mode.is_empty() || state.color.is_empty() {
        return Err(incomplete());
    }
    let zones = match &state.zones {
        None => None,
        Some(z) => {
            Some(z.iter().map(|c| Rgb::parse(c)).collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?)
        }
    };
    let settings = RearSettings {
        mode: RearMode::from_name(&state.mode).map_err(|e| e.to_string())?,
        color: Rgb::parse(&state.color).map_err(|e| e.to_string())?,
        speed,
        brightness,
        direction: state.direction.unwrap_or(1),
        zones,
        verify: true,
    };
    lights.rear_set(&settings)?;
    Ok("restored".to_owned())
}

/// The real machine behind [`Lights`]: the services of the application layer.
struct MachineLights<'a>(&'a Services);

impl Lights for MachineLights<'_> {
    fn apply_profile(&self, name: &str) -> Result<String, String> {
        self.0.bundles.apply(name).map_err(|e| e.to_string())
    }

    fn load_look(&self, name: &str) -> Result<Look, String> {
        self.0.looks.load(name).map_err(|e| e.to_string())
    }

    fn apply_look(&self, look: &Look) -> Result<Vec<String>, String> {
        self.0.looks.apply(look).map_err(|e| e.to_string())
    }

    fn keyboard_last(&self) -> Result<Option<String>, String> {
        self.0.keyboard.replay_last().map_err(|e| e.to_string())
    }

    fn keyboard_default(&self) -> Result<(), String> {
        self.0.keyboard.restore_default().map_err(|e| e.to_string())
    }

    fn keyboard_off(&self) -> Result<(), String> {
        self.0.keyboard.off().map_err(|e| e.to_string())
    }

    fn lighting_profile(&self, name: &str) -> Result<(), String> {
        self.0.lighting.apply(name, None).map_err(|e| e.to_string())
    }

    fn rear_set(&self, settings: &RearSettings) -> Result<(), String> {
        self.0.hw.rear_set(settings).map(|_| ()).map_err(|e| e.to_string())
    }

    fn rear_state(&self) -> Option<Value> {
        self.0.user.get("rear_state")
    }

    fn pause(&self, how_long: Duration) {
        (self.0.sleep)(how_long);
    }
}

/// Applies the login settings of the configuration once, as the earlier editions did: the profile,
/// then the lighting. Answers one line per step.
///
/// # Errors
///
/// Only a configuration that cannot be read; a failing step is a `FAILED (...)` line instead.
pub fn apply(api: &Api) -> ApiResult<Vec<String>> {
    apply_with(api, ApplyOptions::default())
}

/// [`apply`] with retries for a keyboard that has not enumerated yet.
///
/// # Errors
///
/// See [`apply`].
pub fn apply_with(api: &Api, options: ApplyOptions) -> ApiResult<Vec<String>> {
    let cfg = api.load_config()?;
    Ok(Login::new(&MachineLights(api.services()), options).run(&cfg.startup))
}
