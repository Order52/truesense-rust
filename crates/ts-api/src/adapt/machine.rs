//! The machine a client runs on, as a value.
//!
//! Everything the assembly reads from outside the process (environment variables, the root of
//! `/sys` and `/proc`, the program runner, the firmware device, the clock, the service manager's
//! programs, the health check's environment) is one field of [`Machine`]. The real machine is
//! [`Machine::real`]; a test builds the same assembly over a temporary tree, a scripted runner and
//! the firmware simulator, so the assembly itself is exercised without touching the laptop.
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ts_domain::audio::events::{EventSource, PactlSubscribe};
use ts_domain::audio::{DtsLocator, system_dts_locator};
use ts_domain::keyboard::HidOpener;
use ts_foundation::fsx;
use ts_platform::firmware::Device;
use ts_platform::hw::{CommandRunner, ProcessRunner, Sleeper};
use ts_system::osd::effective_uid;
use ts_system::sensors::Clock;
use ts_system::services::{Runner as UnitRunner, SystemRunner};

use crate::api::{DoctorEnvBuilder, EnvLookup, real_doctor_env};

/// How long a repair step may take: `systemctl restart` and `dkms autoinstall` are slow.
const REPAIR_TIMEOUT: Duration = ts_system::services::REPAIR_TIMEOUT;

/// Everything a client reads from the world, in one place.
#[derive(Clone)]
pub struct Machine {
    /// The process environment (`HOME`, `TRUESENSE_SIM`, `TRUESENSE_SOCKET`, ...). An empty value
    /// counts as unset, as everywhere else in the program.
    pub env: EnvLookup,
    /// The prefix of `/sys`, `/proc` and `/dev` for the readings made here: `/` on a real system.
    pub root: PathBuf,
    /// The effective user id (0 is root).
    pub euid: u32,
    /// The firmware, when the caller already has one (a test shares the simulator with the
    /// daemon); `None` opens the kernel device, or the simulator under `TRUESENSE_SIM`.
    pub device: Option<Arc<dyn Device>>,
    /// The keyboard controller, when the caller already has one (a test gives each assembly its
    /// own simulated controller); `None` opens the real hidraw node, or the process-wide
    /// simulated controller under `TRUESENSE_SIM`.
    pub hid: Option<Arc<dyn HidOpener>>,
    /// Runs the programs the sound system, the power managers and the sensors ask for.
    pub runner: Arc<dyn CommandRunner>,
    /// Runs `systemctl`, `modprobe` and `dkms` for the service check and repair.
    pub units: Arc<dyn UnitRunner>,
    /// Builds the health check's view of the machine.
    pub doctor_env: DoctorEnvBuilder,
    /// Finds AcerUltraX's command.
    pub dts_locator: DtsLocator,
    /// Pauses (the firmware layer's retries, the legacy fan path).
    pub sleep: Sleeper,
    /// Wall-clock seconds since the epoch.
    pub clock: Clock,
    /// Where the sound system's change notices come from (`pactl subscribe`); `None` makes the
    /// audio watcher ask once a second, as it did before, which is what a test wants (no process
    /// is ever started by a test).
    pub audio_events: Option<Arc<dyn EventSource>>,
}

impl Machine {
    /// The real machine: the process environment, `/`, real programs, the real kernel device.
    pub fn real() -> Self {
        Self {
            env: Arc::new(fsx::env_nonempty),
            root: PathBuf::from("/"),
            // An unreadable `/proc` must never make this process look like root.
            euid: effective_uid(Path::new("/proc")).unwrap_or(u32::MAX),
            device: None,
            hid: None,
            runner: Arc::new(ProcessRunner),
            units: Arc::new(SystemRunner::new(REPAIR_TIMEOUT)),
            doctor_env: real_doctor_env(),
            dts_locator: system_dts_locator(),
            sleep: Arc::new(std::thread::sleep),
            clock: Arc::new(wall_clock),
            audio_events: Some(Arc::new(PactlSubscribe)),
        }
    }

    /// An absolute path (`/proc/stat`) below this machine's root.
    pub fn path(&self, absolute: &str) -> PathBuf {
        self.root.join(absolute.trim_start_matches('/'))
    }

    /// The home directory. Without `HOME` it is a directory that does not exist, so a write fails
    /// loudly instead of landing in the working directory.
    pub fn home(&self) -> PathBuf {
        (self.env)("HOME").map_or_else(|| PathBuf::from("/nonexistent"), PathBuf::from)
    }

    /// Whether `TRUESENSE_SIM` asks for the simulated hardware.
    pub fn simulated(&self) -> bool {
        (self.env)("TRUESENSE_SIM").is_some()
    }
}

/// Seconds since the Unix epoch.
fn wall_clock() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}
