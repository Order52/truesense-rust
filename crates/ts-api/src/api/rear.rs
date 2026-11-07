//! The rear light bar.
#![forbid(unsafe_code)]

use serde::Serialize;
use ts_foundation::userstate::keys;
use ts_platform::hw::{Coded, Rear, RearMode, RearSettings, Rgb};

use super::{Api, ApiError, ApiResult};

/// A complete light bar setting. The numbers are plain integers and are checked once, by the
/// firmware layer, so the command line and the window get the same messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RearRequest {
    /// The effect.
    pub mode: RearMode,
    /// The effect colour (the static effect colours its three parts from `zones` or from this).
    pub color: Rgb,
    /// Effect speed, 1..=5.
    pub speed: i64,
    /// Brightness, 0..=100.
    pub brightness: i64,
    /// Direction, 0 or 1.
    pub direction: i64,
    /// One colour, or three (left, middle, right); only the static effect can colour the parts
    /// differently.
    pub zones: Option<Vec<Rgb>>,
}

impl RearRequest {
    /// What `rear set EFFECT` uses for what is not given.
    pub fn new(mode: RearMode) -> Self {
        Self { mode, color: Rgb([0xff; 3]), speed: 3, brightness: 100, direction: 1, zones: None }
    }

    /// A request from the words of the command line: the effect name, `RRGGBB`, and optionally a
    /// comma list of one or three zone colours.
    pub fn parse(
        mode: &str,
        color: &str,
        speed: i64,
        brightness: i64,
        direction: i64,
        zones: Option<&str>,
    ) -> ApiResult<Self> {
        let zones =
            zones.map(|list| list.split(',').map(Rgb::parse).collect::<Result<Vec<_>, _>>()).transpose()?;
        Ok(Self {
            mode: RearMode::from_name(mode)?,
            color: Rgb::parse(color)?,
            speed,
            brightness,
            direction,
            zones,
        })
    }

    fn settings(&self, verify: bool) -> RearSettings {
        RearSettings {
            mode: self.mode,
            color: self.color,
            speed: self.speed,
            brightness: self.brightness,
            direction: self.direction,
            zones: self.zones.clone(),
            verify,
        }
    }
}

/// What the light bar can be switched to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RearPowerAction {
    /// On, in the look it had.
    On,
    /// Off, remembering the look.
    Off,
    /// The other way.
    Toggle,
}

impl RearPowerAction {
    fn word(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Off => "off",
            Self::Toggle => "toggle",
        }
    }
}

impl std::str::FromStr for RearPowerAction {
    type Err = ApiError;

    fn from_str(text: &str) -> Result<Self, ApiError> {
        match text {
            "on" => Ok(Self::On),
            "off" => Ok(Self::Off),
            "toggle" => Ok(Self::Toggle),
            other => Err(ApiError::invalid(format!("rear: on | off | toggle (got '{other}')"))),
        }
    }
}

/// The light bar with the three part colours of its static effect (`null` when the bar has one
/// colour), as the window shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RearStatus {
    /// What the firmware reports.
    #[serde(flatten)]
    pub rear: Rear,
    /// The three part colours the window saved, left to right.
    pub zones: Option<Vec<Rgb>>,
}

impl Api {
    /// Reads the light bar.
    pub fn rear(&self) -> ApiResult<Rear> {
        Ok(self.s.hw.rear_get()?)
    }

    /// Writes the light bar and answers what the firmware reports back.
    pub fn set_rear(&self, request: &RearRequest) -> ApiResult<Rear> {
        Ok(self.s.hw.rear_set(&request.settings(true))?)
    }

    /// Switches the light bar on, off or the other way, keeping its look and the three part colours
    /// of the static effect, and answers the message the key would show.
    pub fn rear_power(&self, action: RearPowerAction) -> ApiResult<String> {
        self.run_action(&format!("rear:{}", action.word()))
    }

    /// The part colours remembered with the light bar (exactly three, else none).
    fn remembered_zones(&self) -> Option<Vec<Rgb>> {
        let names: Vec<String> = self.s.user.get_as(keys::REAR_ZONES)?;
        let zones: Result<Vec<Rgb>, _> = names.iter().map(|n| Rgb::parse(n)).collect();
        zones.ok().filter(|z| z.len() == 3)
    }

    /// Reads the light bar and the part colours remembered with it.
    pub fn rear_status(&self) -> ApiResult<RearStatus> {
        let mut rear = self.s.hw.rear_get()?;
        rear.zones = None;
        Ok(RearStatus { rear, zones: self.remembered_zones() })
    }

    /// Writes the light bar without waiting for the read-back (a slider or a colour drag follows
    /// with the next write at once). With `keep_zones` the three part colours of the static effect
    /// that the window saved are written again, so changing the brightness does not flatten them.
    pub fn set_rear_live(&self, request: &RearRequest, keep_zones: bool) -> ApiResult<RearStatus> {
        let mut request = request.clone();
        if keep_zones && request.mode == RearMode::Static && request.zones.is_none() {
            request.zones = self.remembered_zones();
        }
        let mut rear = self.s.hw.rear_set(&request.settings(false))?;
        let zones = request.zones.clone().or_else(|| self.remembered_zones());
        rear.zones = None;
        Ok(RearStatus { rear, zones })
    }

    /// The effect names in display order.
    pub fn rear_effects() -> Vec<&'static str> {
        RearMode::ALL.iter().map(|m| m.name()).collect()
    }
}
