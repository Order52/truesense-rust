//! The machine as a whole: the health check, the model probe, the services and the configuration.
//!
//! The health check (`ts_system::doctor`) cannot see the daemon, the sound system or the keyboard,
//! so this module adapts them to the check's small traits ([`DoctorAdapters`]); the same adapters
//! are what the self-test and any other client of the check use.
#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use ts_domain::audio::{Audio, SoundState as AudioSound};
use ts_domain::powermgr::PowerManagers;
use ts_foundation::config::Table;
use ts_ipc::wire::Args;
use ts_platform::hw::Hw;
use ts_system::doctor::{
    self, AudioChecks, CallError, DaemonState as DoctorState, DomainProbes, Firmware, Jack, SoundState,
};
use ts_system::models;
use ts_system::services::Unit;

use super::{Api, ApiError, ApiResult, Daemon, ServiceChecks};

pub use ts_system::services::Step;

/// What the health check is shown of this process: the firmware, the daemon, the sound system and
/// the keyboard and power probes. `Services::doctor_env` turns them into the check's environment.
#[derive(Clone)]
pub struct DoctorAdapters {
    /// The firmware features.
    pub firmware: Arc<dyn Firmware>,
    /// The control socket.
    pub daemon: Arc<dyn doctor::Daemon>,
    /// The sound system.
    pub audio: Arc<dyn AudioChecks>,
    /// The keyboard's Fn-key node and the CPU frequency ceilings.
    pub probes: Arc<dyn DomainProbes>,
}

/// The control socket as the health check asks it.
struct DaemonCheck(Arc<dyn Daemon>);

impl doctor::Daemon for DaemonCheck {
    fn state(&self) -> Result<DoctorState, CallError> {
        let value = self.0.call("state", Args::new()).map_err(|e| CallError::new(e.to_string()))?;
        serde_json::from_value(value).map_err(|e| CallError::new(format!("state: unexpected reply: {e}")))
    }

    fn read(&self, command: &str) -> Result<(), CallError> {
        self.0
            .call(command, Args::new().with("action", "get"))
            .map(drop)
            .map_err(|e| CallError::new(e.to_string()))
    }
}

/// The sound system as the health check asks it.
struct AudioCheck(Arc<Audio>);

impl AudioChecks for AudioCheck {
    fn has_rnnoise(&self) -> bool {
        self.0.has_rnnoise()
    }

    fn has_dts(&self) -> bool {
        self.0.has_dts()
    }

    fn jack_status(&self) -> Result<Jack, CallError> {
        let j = self.0.jack_status();
        Ok(Jack { headphones: j.headphones, headset_mic: j.headset_mic })
    }

    fn sound_state(&self) -> Result<SoundState, CallError> {
        Ok(match self.0.sound_state().map_err(|e| CallError::new(e.to_string()))? {
            AudioSound::NoCard { .. } => SoundState::Unknown,
            AudioSound::Ok { .. } => SoundState::Healthy,
            AudioSound::Broken { problem, .. } => SoundState::Broken(problem),
        })
    }

    fn plain_headsets(&self) -> Result<Vec<String>, CallError> {
        self.0.plain_headsets().map_err(|e| CallError::new(e.to_string()))
    }
}

/// The keyboard and power probes of the health check.
struct Probes {
    fn_key_node: super::NodeFinder,
    power: Arc<PowerManagers>,
}

impl DomainProbes for Probes {
    fn fn_key_node(&self) -> Option<PathBuf> {
        (self.fn_key_node)()
    }

    fn low_frequency_ceilings(&self) -> usize {
        self.power.ceiling_low().len()
    }
}

/// The health check's environment on the real machine: the real file system, programs and
/// configuration, connected to this process's adapters. This is what `Services::doctor_env` is in
/// production.
pub fn real_doctor_env() -> super::DoctorEnvBuilder {
    Arc::new(|a: DoctorAdapters| doctor::Env::system(a.firmware, a.daemon, a.audio, a.probes))
}

impl Api {
    /// The adapters the health check is built from.
    pub fn doctor_adapters(&self) -> DoctorAdapters {
        let firmware: Arc<dyn Firmware> = Arc::new(self.s.hw.clone());
        DoctorAdapters {
            firmware,
            daemon: Arc::new(DaemonCheck(Arc::clone(&self.s.daemon))),
            audio: Arc::new(AudioCheck(Arc::clone(&self.s.audio))),
            probes: Arc::new(Probes {
                fn_key_node: Arc::clone(&self.s.fn_key_node),
                power: Arc::clone(&self.s.power_managers),
            }),
        }
    }

    /// Runs the whole health check (read-only).
    pub fn run_doctor(&self) -> doctor::Report {
        let env = (self.s.doctor_env)(self.doctor_adapters());
        doctor::run(&env, &doctor::checks())
    }

    /// The titles of the health checks in the order the report shows them.
    pub fn doctor_checks() -> Vec<&'static str> {
        doctor::checks().iter().map(|c| c.title).collect()
    }

    /// The read-only report of this laptop for model support: it never writes a setting.
    pub fn probe(&self) -> models::Report {
        let hw: &Hw = &self.s.hw;
        models::probe(Some(hw.device().as_ref()), &self.s.probe)
    }

    /// The kernel module's version.
    pub fn module_version(&self) -> ApiResult<String> {
        Ok(self.s.hw.version()?)
    }

    /// The service manager, or the reason there is none.
    pub(crate) fn service_checks(&self) -> ApiResult<&Arc<dyn ServiceChecks>> {
        self.s
            .service_checks
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("the service manager is not available here"))
    }

    /// The state of every unit, the kernel module and the control socket (read-only).
    pub fn check_services(&self) -> ApiResult<Vec<Unit>> {
        Ok(self.service_checks()?.check())
    }

    /// The part of `service repair` that needs root: the daemon, its health timer and the kernel
    /// module. It touches no setting.
    pub fn repair_system_services(&self) -> ApiResult<Vec<Step>> {
        if self.s.euid != 0 {
            return Err(ApiError::invalid("the system part needs root: sudo truesense service repair"));
        }
        Ok(self.service_checks()?.repair_system())
    }

    /// The configuration files that are read, system first.
    pub fn config_paths(&self) -> Vec<PathBuf> {
        [self.s.config.system.clone(), self.s.config.user.clone()].into_iter().flatten().collect()
    }

    /// The whole configuration (the files over the defaults) as the nested object the file has.
    pub fn config_values(&self) -> ApiResult<Value> {
        Ok(serde_json::to_value(self.load_config()?)?)
    }

    /// The whole configuration as TOML text.
    pub fn config_toml(&self) -> ApiResult<String> {
        let table = Table::try_from(self.load_config()?)
            .map_err(|e| ApiError::invalid(format!("cannot write the configuration: {e}")))?;
        Ok(table.to_string())
    }

    /// The problems of the configuration (empty: valid), including a file that does not parse,
    /// which keeps the program on the defaults.
    pub fn validate_config(&self) -> Vec<String> {
        match self.load_config() {
            Ok(cfg) => cfg.validate(),
            Err(e) => vec![e.to_string()],
        }
    }
}
