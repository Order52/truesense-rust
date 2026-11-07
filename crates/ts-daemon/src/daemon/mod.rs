//! daemon: the root service's brain. It decides nothing itself, it wires.
//!
//! Every decision comes from `ts-decision` (the activity classifier, the smart fan, the
//! automatic power mode, the thermal governor, the full-charge rule, the crash guard, the game
//! rules); every effect goes through a port of [`deps`]; this module only reads the sensors on
//! a tick, hands the readings to the engines, carries out what they say and publishes the
//! result. The pieces:
//!
//! * [`deps`]: the traits and the [`Deps`] struct the daemon is built from, and [`adapt`], their
//!   implementations for the concrete domain and system types;
//! * `daemon.rs`: the [`Daemon`] itself, its state, and the control loop with the
//!   watchdog pings, `READY` and `STOPPING`;
//! * `step.rs`: one guarded tick: a panicking step is caught, never leaves the state lock held
//!   and, when the machine is hot, puts the fans on 100 %;
//! * `boot.rs`: restoring the saved settings at start and handing the machine back on exit;
//! * `bindings.rs`: the `mode_bindings` command and the per-power-mode look and fan curve that
//!   are applied when the mode changes;
//! * `handlers.rs`: the control socket's command table, the same names, arguments and result
//!   shapes as the Python daemon so the Python and Go clients keep working;
//! * `plug.rs`: the charger signal with a three second hold-off, so a bouncing contact moves the
//!   power mode and the fans once, not every time it flips;
//! * `publish.rs`: the live `key=value` state for status bars and `health.sh`;
//! * `alerts.rs`: desktop alerts (a hot CPU or GPU, a low battery, ...) evaluated on the tick and
//!   shown to the desktop user by a helper thread, so no notifier can slow or break a tick.
//!
//! Safety, in the words of the Python daemon: the fans go back to firmware Auto and the CPU
//! clock cap is lifted on ANY exit (a stop, an error, a panic, SIGTERM or SIGINT); if a step
//! fails while the machine is hot the fans go to 100 %; a missing or failing device never
//! kills the daemon, it keeps waiting and retrying.
//!
//! The caller owns the process: it builds the concrete ports and puts them in [`Deps`], keeps a
//! clone of the [`StopToken`] for its signal thread, and calls [`run`].
#![forbid(unsafe_code)]

pub mod adapt;
mod alerts;
mod bindings;
mod boot;
#[allow(clippy::module_inception)]
mod daemon;
pub mod deps;
mod handlers;
mod instance;
mod plug;
mod publish;
mod step;

#[cfg(test)]
mod alerts_tests;
#[cfg(test)]
mod bindings_tests;
#[cfg(test)]
mod handler_tests;
#[cfg(test)]
mod hardening_tests;
#[cfg(test)]
mod loop_tests;
#[cfg(test)]
mod resilience_tests;
#[cfg(test)]
mod steady_tests;
#[cfg(test)]
mod testkit;
#[cfg(test)]
mod tests;

use std::fmt;
use std::path::PathBuf;

use ts_ipc::server::ServerError;

pub use self::alerts::{AlertSettings, ConfigFileAlerts};
pub use self::daemon::Daemon;
pub use self::deps::{
    BootLookPort, Clock, ClockCap, CpuUndervoltPort, Deps, GpuUndervoltPort, Level, LivePaths, Logger,
    Notice, OperatingLight, PortError, PortResult, PowerLimitsPort, PowerManagersPort, ProcessList,
    RealSleeper, Sensors, Sleeper, StdoutLogger, StopToken, SystemClock, TouchpadPort,
};
pub use self::handlers::COMMANDS;

/// Why [`run`] ended with an error. By then the fans are back on the firmware and the clock cap
/// is lifted.
#[derive(Debug)]
pub enum DaemonError {
    /// The control socket could not be started.
    Socket(ServerError),
    /// The control loop panicked outside the guarded tick (a bug); the text is the panic message.
    Panicked(String),
    /// Another daemon already drives this machine (it holds the lock file or answers on the
    /// socket). Nothing was touched: two daemons writing the fans against each other, and the
    /// second deleting the first one's socket, is worse than one refusing to start.
    AlreadyRunning(String),
}

impl fmt::Display for DaemonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Socket(e) => write!(f, "control socket: {e}"),
            Self::Panicked(why) => write!(f, "the control loop failed: {why}"),
            Self::AlreadyRunning(how) => {
                write!(f, "another TrueSense daemon is already running ({how}); not starting a second one")
            }
        }
    }
}

impl std::error::Error for DaemonError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Socket(e) => Some(e),
            Self::Panicked(_) | Self::AlreadyRunning(_) => None,
        }
    }
}

/// Builds the daemon from its dependencies and runs it until `deps.stop` is requested: the
/// control socket at `socket` (the desktop user and root may use it), the boot sequence, then
/// the control loop. The fans go back to the firmware and the clock cap is lifted on every way
/// out. Blocks.
pub fn run(deps: Deps, socket: impl Into<PathBuf>) -> Result<(), DaemonError> {
    Daemon::new(deps).run(socket)
}
