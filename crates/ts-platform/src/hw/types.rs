//! The vocabulary of the firmware features: modes, levels, duties and colours as types, so that
//! a value the firmware would refuse cannot be built in the first place.

use std::fmt;
use std::str::FromStr;

use serde::{Serialize, Serializer};

use super::HwError;

/// A firmware setting that is a small set of numbered choices.
pub trait Coded: Copy + PartialEq + fmt::Debug + 'static {
    /// Every choice, in the order users see them.
    const ALL: &'static [Self];
    /// The message when a name does not match.
    const INVALID_NAME: &'static str;
    /// The number the firmware uses.
    fn code(self) -> i64;
    /// The name the user interface and the config files use.
    fn name(self) -> &'static str;

    /// The choice with this firmware number, if there is one.
    fn from_code(code: i64) -> Option<Self> {
        Self::ALL.iter().copied().find(|v| v.code() == code)
    }

    /// The choice with this name, if there is one.
    fn from_name(name: &str) -> Result<Self, HwError> {
        Self::ALL
            .iter()
            .copied()
            .find(|v| v.name() == name)
            .ok_or_else(|| HwError::Invalid(Self::INVALID_NAME.to_string()))
    }
}

/// What the firmware reports for a [`Coded`] setting: a choice we know, or a number we do not
/// (a newer BIOS, or the `-1` the firmware answers right after a write).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reading<T> {
    /// A choice we know.
    Known(T),
    /// A number we have no name for.
    Unknown(i64),
}

impl<T: Coded> Reading<T> {
    /// Interprets a firmware number.
    pub fn from_code(code: i64) -> Self {
        T::from_code(code).map_or(Self::Unknown(code), Self::Known)
    }

    /// The choice, if it is one we know.
    pub fn known(self) -> Option<T> {
        match self {
            Self::Known(v) => Some(v),
            Self::Unknown(_) => None,
        }
    }
}

/// `quiet`, or `unknown(7)`: the form the Python edition showed and stored.
impl<T: Coded> fmt::Display for Reading<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Known(v) => f.write_str(v.name()),
            Self::Unknown(n) => write!(f, "unknown({n})"),
        }
    }
}

impl<T: Coded> Serialize for Reading<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

macro_rules! coded_enum {
    (
        $(#[$meta:meta])*
        $name:ident, invalid: $invalid:expr, {
            $($(#[$vmeta:meta])* $variant:ident = $code:expr, $text:expr;)+
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($(#[$vmeta])* $variant,)+
        }

        impl Coded for $name {
            const ALL: &'static [Self] = &[$(Self::$variant),+];
            const INVALID_NAME: &'static str = $invalid;

            fn code(self) -> i64 {
                match self { $(Self::$variant => $code,)+ }
            }

            fn name(self) -> &'static str {
                match self { $(Self::$variant => $text,)+ }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.name())
            }
        }

        impl FromStr for $name {
            type Err = HwError;

            fn from_str(text: &str) -> Result<Self, HwError> {
                Self::from_name(text)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.name())
            }
        }
    };
}

coded_enum! {
    /// The firmware's power mode (misc setting `0x0B`). The numbers are the firmware's: they
    /// skip 2 and 3.
    PowerMode, invalid: "power mode must be one of: quiet, balanced, performance, turbo, eco", {
        /// Quietest fans, lowest power.
        Quiet = 0, "quiet";
        /// The everyday mode.
        Balanced = 1, "balanced";
        /// Higher power limits.
        Performance = 4, "performance";
        /// Highest limits and loudest fans.
        Turbo = 5, "turbo";
        /// Battery saving.
        Eco = 6, "eco";
    }
}

coded_enum! {
    /// The GPU (MUX) mode (misc setting 2). A change only applies after a reboot.
    GpuMode, invalid: "gpu mode: optimus | nvidia | auto (applies after reboot)", {
        /// The integrated GPU drives the display; the NVIDIA GPU renders on demand.
        Optimus = 1, "optimus";
        /// The NVIDIA GPU drives the display.
        Nvidia = 2, "nvidia";
        /// The firmware decides.
        Auto = 3, "auto";
    }
}

coded_enum! {
    /// An effect of the rear light bar.
    RearMode, invalid: "rear mode: static, breathing, neon, wave, twinkling", {
        /// One colour per part.
        Static = 0, "static";
        /// Fading in and out.
        Breathing = 1, "breathing";
        /// Neon.
        Neon = 2, "neon";
        /// A wave.
        Wave = 3, "wave";
        /// Twinkling.
        Twinkling = 7, "twinkling";
    }
}

coded_enum! {
    /// The battery level down to which the USB ports charge a phone while the laptop is off.
    UsbLevel, invalid: "usb-charge level: 0 (off), 10, 20 or 30", {
        /// No charging while the laptop is off.
        Off = 0, "0";
        /// Stop at 10 %.
        Percent10 = 10, "10";
        /// Stop at 20 %.
        Percent20 = 20, "20";
        /// Stop at 30 %.
        Percent30 = 30, "30";
    }
}

impl UsbLevel {
    /// The level for a percentage the user chose; anything but 0, 10, 20 or 30 is refused before
    /// it reaches the firmware.
    pub fn from_percent(percent: i64) -> Result<Self, HwError> {
        Self::from_code(percent).ok_or_else(|| HwError::Invalid(Self::INVALID_NAME.to_string()))
    }

    /// The percentage (0 = off).
    pub fn percent(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::Percent10 => 10,
            Self::Percent20 => 20,
            Self::Percent30 => 30,
        }
    }
}

/// How the firmware says the fans are controlled (`get_fan`). The numbers are what the module
/// reports as `behavior`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanMode {
    /// The firmware's own curve.
    Auto,
    /// Both fans at full speed.
    Max,
    /// The duty registers are what was written.
    Manual,
    /// A behaviour word we do not know.
    Unknown,
}

/// The `behavior` field of `get_fan` for [`FanMode::Auto`].
pub const FAN_AUTO_BEHAVIOR: i64 = 0x41;
/// The `behavior` field of `get_fan` for [`FanMode::Max`].
pub const FAN_MAX_BEHAVIOR: i64 = 0x82;
/// The `behavior` field of `get_fan` for [`FanMode::Manual`].
pub const FAN_MANUAL_BEHAVIOR: i64 = 0xC3;

impl FanMode {
    /// Interprets the `behavior` field.
    pub fn from_behavior(behavior: i64) -> Self {
        match behavior {
            FAN_AUTO_BEHAVIOR => Self::Auto,
            FAN_MAX_BEHAVIOR => Self::Max,
            FAN_MANUAL_BEHAVIOR => Self::Manual,
            _ => Self::Unknown,
        }
    }

    /// `auto`, `max`, `manual` or `unknown`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Max => "max",
            Self::Manual => "manual",
            Self::Unknown => "unknown",
        }
    }
}

impl fmt::Display for FanMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl Serialize for FanMode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

/// The duty of one fan in percent: 0 (stopped) or 20 to 100. The firmware takes nothing in
/// between, and a duty below 20 that is not 0 would leave a fan spinning too slowly to cool.
///
/// A value of 0 stops the fan. Building one is allowed because the daemon needs it, but the
/// daemon alone decides: it asks for a typed confirmation, checks that the machine is cool and
/// runs a watchdog that gives the fans back to the firmware (see `docs/SAFETY.md`). Nothing in
/// this crate stops a fan on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FanDuty(u8);

impl FanDuty {
    /// A duty from a percentage, or an error for anything but 0 or 20..=100.
    pub fn new(percent: i64) -> Result<Self, HwError> {
        match u8::try_from(percent) {
            Ok(p) if p == 0 || (20..=100).contains(&p) => Ok(Self(p)),
            _ => Err(HwError::Invalid(
                "manual duty must be 20..100 % per fan (or 0 to stop that fan)".to_string(),
            )),
        }
    }

    /// The percentage.
    pub fn percent(self) -> u8 {
        self.0
    }

    /// Whether this duty stops the fan.
    pub fn is_stop(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for FanDuty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A colour as the light bar takes it, written `RRGGBB` in hex.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb(pub [u8; 3]);

impl Rgb {
    /// Parses exactly six hex digits, either case.
    pub fn parse(text: &str) -> Result<Self, HwError> {
        let bytes = text.as_bytes();
        if bytes.len() != 6 || !bytes.iter().all(u8::is_ascii_hexdigit) {
            return Err(HwError::Invalid("color must be RRGGBB hex".to_string()));
        }
        let digit = |b: u8| char::from(b).to_digit(16).and_then(|d| u8::try_from(d).ok()).unwrap_or(0);
        let byte = |i: usize| digit(bytes[i]) * 16 + digit(bytes[i + 1]);
        Ok(Self([byte(0), byte(2), byte(4)]))
    }
}

impl FromStr for Rgb {
    type Err = HwError;

    fn from_str(text: &str) -> Result<Self, HwError> {
        Self::parse(text)
    }
}

/// Lower-case `rrggbb`, the form the firmware commands and the saved files use.
impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02x}{:02x}{:02x}", self.0[0], self.0[1], self.0[2])
    }
}

impl Serialize for Rgb {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}
