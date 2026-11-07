//! The one error type of the application layer.
//!
//! Every client (command line, terminal UI, window, key service) shows `Display` to the person
//! using it, and the lower layers already word their errors for that, so the text of a lower error
//! is kept unchanged. What this type adds is the *kind* of failure, so a client can decide what to
//! do without reading prose: fall back, offer the confirmation dialog, tell the user to start the
//! service.
#![forbid(unsafe_code)]

use std::error::Error;
use std::fmt;

use ts_ipc::client::ClientError;
use ts_platform::hw::HwError;

/// Why an operation was refused before anything was sent. Each is a safety rule of
/// `docs/SAFETY.md`; the client that sees one asks the user (confirmation, force) or explains,
/// it never retries silently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Stopping a fan (duty 0) was asked without the explicit confirmation.
    StopNeedsConfirmation,
    /// Stopping a fan needs the service: its watchdog is what brings the fans back when it gets warm.
    StopNeedsService,
    /// Turning both fans off needs the service, for the same reason.
    OffNeedsService,
    /// The smart curve is the service's: nothing runs it without the service.
    SmartNeedsService,
    /// A manual fan duty outside 0 or 20..=100, or none at all.
    FanDuty,
    /// A CPU or GPU undervolt was asked without the explicit confirmation.
    UndervoltNeedsConfirmation {
        /// Whether it is the GPU undervolt (the wording differs).
        gpu: bool,
    },
    /// A value beyond the danger line was asked without `force`. The text is the rating's own.
    Danger {
        /// The rating's explanation, ending in a full stop.
        why: String,
    },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StopNeedsConfirmation => f.write_str("stopping a fan needs explicit confirmation"),
            Self::StopNeedsService => f.write_str(
                "stopping a fan needs the TrueSense service (it brings the fans back when it gets warm)",
            ),
            Self::OffNeedsService => f.write_str(
                "turning the fans off needs the TrueSense service (it brings them back when it gets warm)",
            ),
            Self::SmartNeedsService => f.write_str("the smart curve needs the TrueSense daemon"),
            Self::FanDuty => f.write_str("manual fan duty is 20..100 % per fan, or 0 to stop that fan"),
            Self::UndervoltNeedsConfirmation { gpu: false } => f.write_str(
                "an undervolt needs explicit confirmation (it can freeze or reboot the laptop if too aggressive)",
            ),
            Self::UndervoltNeedsConfirmation { gpu: true } => f.write_str(
                "a GPU undervolt needs explicit confirmation (it can crash games or the driver if too aggressive)",
            ),
            Self::Danger { why } => {
                write!(f, "{why} Add the explicit force flag if you really mean it (CLI: --force).")
            }
        }
    }
}

/// A failure of an application operation.
#[derive(Debug)]
pub enum ApiError {
    /// A firmware feature failed (the hardware layer's own words).
    Hw(HwError),
    /// The daemon is missing, refused, or answered something unexpected.
    Daemon(ClientError),
    /// A safety rule refused the request before anything was sent.
    Refused(Refusal),
    /// The request itself is wrong (a missing name, an unknown word); the text says what is allowed.
    Invalid(String),
    /// This process cannot do it here (no image decoder, no service manager in this build).
    Unavailable(String),
    /// Another layer failed; `area` says which, the text is its own.
    Domain {
        /// The layer or feature (`fan curve`, `keyboard`, `audio`, ...).
        area: &'static str,
        /// What failed.
        source: Box<dyn Error + Send + Sync>,
    },
}

impl ApiError {
    /// A wrong request.
    pub fn invalid(text: impl Into<String>) -> Self {
        Self::Invalid(text.into())
    }

    /// Something this build cannot do.
    pub fn unavailable(text: impl Into<String>) -> Self {
        Self::Unavailable(text.into())
    }

    /// Wraps another layer's error, keeping its text.
    pub fn domain(area: &'static str, source: impl Error + Send + Sync + 'static) -> Self {
        Self::Domain { area, source: Box::new(source) }
    }

    /// Whether the daemon is simply not there (a client may say how to start it).
    pub fn daemon_missing(&self) -> bool {
        matches!(self, Self::Daemon(e) if e.is_not_running())
    }

    /// The refusal behind this error, if it is one.
    pub fn refusal(&self) -> Option<&Refusal> {
        match self {
            Self::Refused(r) => Some(r),
            _ => None,
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hw(e) => e.fmt(f),
            Self::Daemon(e) => e.fmt(f),
            Self::Refused(r) => r.fmt(f),
            Self::Invalid(t) | Self::Unavailable(t) => f.write_str(t),
            Self::Domain { source, .. } => source.fmt(f),
        }
    }
}

impl Error for ApiError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Hw(e) => Some(e),
            Self::Daemon(e) => Some(e),
            Self::Domain { source, .. } => Some(source.as_ref()),
            Self::Refused(_) | Self::Invalid(_) | Self::Unavailable(_) => None,
        }
    }
}

impl From<HwError> for ApiError {
    fn from(e: HwError) -> Self {
        Self::Hw(e)
    }
}

impl From<ClientError> for ApiError {
    fn from(e: ClientError) -> Self {
        Self::Daemon(e)
    }
}

impl From<Refusal> for ApiError {
    fn from(r: Refusal) -> Self {
        Self::Refused(r)
    }
}

macro_rules! domain_from {
    ($($ty:ty => $area:expr),+ $(,)?) => {$(
        impl From<$ty> for ApiError {
            fn from(e: $ty) -> Self {
                Self::domain($area, e)
            }
        }
    )+};
}

domain_from! {
    ts_decision::curves::CurveError => "fan curve",
    ts_decision::smartprofile::ProfileError => "smart profile",
    ts_decision::gamerules::RulesError => "game rules",
    ts_domain::undervolt::ProfileError => "undervolt profile",
    ts_domain::undervolt::UndervoltError => "cpu undervolt",
    ts_domain::gpuundervolt::GpuError => "gpu undervolt",
    ts_domain::power::PowerError => "power limits",
    ts_domain::keyboard::KeyboardError => "keyboard",
    ts_domain::keymap::KeymapError => "keyboard",
    ts_domain::keyprofiles::ProfileError => "lighting profile",
    ts_domain::designs::DesignError => "design",
    ts_domain::looks::LooksError => "look",
    ts_domain::perkeyfx::FxError => "animation",
    ts_domain::audio::AudioError => "audio",
    ts_domain::keybinds::KeybindsError => "key profile",
    ts_domain::profiles::BundleError => "profile",
    ts_domain::actions::ActionError => "action",
    ts_domain::actions::HostError => "profile",
    ts_domain::powermgr::PowerMgrError => "power managers",
    ts_foundation::config::ConfigError => "configuration",
    ts_foundation::fsx::FsError => "file",
    ts_system::touchpad::TouchpadError => "touchpad",
    ts_system::update::UpdateError => "update",
    serde_json::Error => "data",
}

/// The result of every operation of the application layer.
pub type ApiResult<T> = Result<T, ApiError>;
