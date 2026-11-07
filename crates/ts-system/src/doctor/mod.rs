//! doctor: `truesense doctor` checks the whole stack and says how to fix what is wrong. It is
//! read-only. A check returns a status, a detail line and a fix; the exit code is 1 if anything
//! FAILs (WARN does not fail).
//!
//! Every probe is injected through [`Env`]: files are read below a root path, programs run
//! through a [`Runner`], the firmware, the daemon, the sound system and the other domain modules
//! through small traits, so the whole health check runs in a test against a temporary tree and
//! fakes. The titles and wording are the Python edition's (the Go edition kept them too), because
//! users and the documentation refer to them.
//!
//! The checks only read and mostly wait for other programs, so they run side by side on a small
//! worker pool and are reported in their usual order.
#![deny(unsafe_code)]

mod checks;
mod sys;

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use ts_foundation::buildinfo;
use ts_foundation::config::{Config, ConfigError};
use ts_foundation::sync::locked;
use ts_platform::hw::{Capabilities, FanState, Hw, HwError};

pub use checks::checks;
pub use sys::SystemHost;

use crate::models;
use crate::services::{Runner, SystemRunner};

/// How long the doctor lets one program run (the Python edition's 6 s): a wedged tool must not
/// hold up the whole report.
pub(crate) const PROGRAM_TIMEOUT: Duration = Duration::from_secs(6);

/// How many checks run side by side unless told otherwise.
pub(crate) const DEFAULT_WORKERS: usize = 8;

/// The grade of one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Status {
    /// As it should be.
    #[serde(rename = "OK")]
    Ok,
    /// Works, but something deserves attention. Does not fail the doctor.
    #[serde(rename = "WARN")]
    Warn,
    /// Broken; the doctor exits with 1.
    #[serde(rename = "FAIL")]
    Fail,
    /// Information only; never a problem.
    #[serde(rename = "INFO")]
    Info,
}

impl Status {
    /// The tag the report prints: `OK`, `WARN`, `FAIL` or `INFO`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Warn => "WARN",
            Self::Fail => "FAIL",
            Self::Info => "INFO",
        }
    }

    /// The ANSI colour of the tag on a terminal.
    fn colour(self) -> &'static str {
        match self {
            Self::Ok => "32",
            Self::Warn => "33",
            Self::Fail => "31",
            Self::Info => "36",
        }
    }
}

/// What one check found: its grade, one line of detail and how to fix it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The grade.
    pub status: Status,
    /// One line for the report.
    pub detail: String,
    /// The command or action that fixes it; shown for WARN and FAIL only.
    pub fix: String,
}

impl Outcome {
    fn new(status: Status, detail: impl Into<String>, fix: impl Into<String>) -> Self {
        Self { status, detail: detail.into(), fix: fix.into() }
    }

    /// As it should be.
    pub fn ok(detail: impl Into<String>) -> Self {
        Self::new(Status::Ok, detail, "")
    }

    /// Information only.
    pub fn info(detail: impl Into<String>, fix: impl Into<String>) -> Self {
        Self::new(Status::Info, detail, fix)
    }

    /// Attention needed, but nothing is broken.
    pub fn warn(detail: impl Into<String>, fix: impl Into<String>) -> Self {
        Self::new(Status::Warn, detail, fix)
    }

    /// Broken.
    pub fn fail(detail: impl Into<String>, fix: impl Into<String>) -> Self {
        Self::new(Status::Fail, detail, fix)
    }

    /// OK when `good`, WARN with the same detail and the fix otherwise.
    pub fn graded(good: bool, detail: impl Into<String>, fix: impl Into<String>) -> Self {
        if good { Self::ok(detail) } else { Self::warn(detail, fix) }
    }
}

/// One named probe. The title is what the report shows.
pub struct Check {
    /// The report's name for it.
    pub title: &'static str,
    run: Box<dyn Fn(&Env) -> Outcome + Send + Sync>,
}

impl Check {
    /// A check that runs `run`. A closure (not a plain function) so that tests can build checks
    /// that count, wait or panic.
    pub fn new(title: &'static str, run: impl Fn(&Env) -> Outcome + Send + Sync + 'static) -> Self {
        Self { title, run: Box::new(run) }
    }
}

impl fmt::Debug for Check {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Check").field("title", &self.title).finish_non_exhaustive()
    }
}

/// Why a question to the daemon, the sound system or the keyboard got no answer. It carries the
/// text to show; `unknown_command` tells "the running service is older than this app" (the daemon
/// does not know the command) from every other failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallError {
    message: String,
    unknown_command: bool,
}

impl CallError {
    /// A failure with this text. A message that says `unknown command` (what the daemon answers
    /// to a command it has never heard of) is recognised as such.
    pub fn new(message: impl Into<String>) -> Self {
        let message = message.into();
        let unknown_command = message.contains("unknown command");
        Self { message, unknown_command }
    }

    /// Whether the daemon did not know the command: it is older than this program.
    pub fn is_unknown_command(&self) -> bool {
        self.unknown_command
    }
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CallError {}

/// The part of [`Hw`] the checks read. `Hw` implements it; a test can answer anything.
pub trait Firmware: Send + Sync {
    /// Which firmware interfaces the kernel module found.
    fn capabilities(&self) -> Result<Capabilities, HwError>;
    /// What the firmware says about fan control.
    fn fan_state(&self) -> Result<FanState, HwError>;
}

impl Firmware for Hw {
    fn capabilities(&self) -> Result<Capabilities, HwError> {
        Hw::capabilities(self)
    }

    fn fan_state(&self) -> Result<FanState, HwError> {
        Hw::fan_state(self)
    }
}

/// The switches the daemon runs by.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DaemonSettings {
    /// The smart fan curve is on.
    pub smart_fan: bool,
    /// The automatic power profile is on.
    pub auto_profile: bool,
}

/// What the daemon last measured.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DaemonLive {
    /// Whether the daemon can reach the kernel device; `None` = not reported = fine.
    pub hw_ok: Option<bool>,
    /// What the machine is doing (`idle`, `gaming`, ...).
    pub activity: Option<String>,
}

/// The answer to the daemon's `state` command, as far as the checks use it. The JSON names are
/// the daemon's, so the adapter can deserialize the reply straight into it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DaemonState {
    /// What the daemon last measured.
    pub live: DaemonLive,
    /// The switches it runs by.
    pub settings: DaemonSettings,
}

/// The control socket as the checks use it. The assembly layer adapts the control-socket client
/// (`ts-ipc`) to it.
pub trait Daemon: Send + Sync {
    /// The daemon's `state`.
    fn state(&self) -> Result<DaemonState, CallError>;

    /// Asks the daemon for `command` with `{"action": "get"}` and discards the answer: how the
    /// version probe finds commands a stale service does not know. It can only read, by type.
    fn read(&self, command: &str) -> Result<(), CallError>;
}

/// What the codec sees on the 3.5 mm jack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Jack {
    /// Headphones are plugged in.
    pub headphones: bool,
    /// The plug carries a headset microphone.
    pub headset_mic: bool,
}

/// The sound card's health as the Audio check judges it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SoundState {
    /// There is no card to judge (or PipeWire could not say).
    Unknown,
    /// The card has its speaker and headphone outputs.
    Healthy,
    /// The card has no sound output; the text says what is wrong.
    Broken(String),
}

/// The sound system as the Audio check sees it. The audio module of the domain layer is not
/// visible from here; the assembly layer adapts its controller to this trait.
pub trait AudioChecks: Send + Sync {
    /// The RNNoise plugin (Purified Voice) is installed.
    fn has_rnnoise(&self) -> bool;
    /// AcerUltraX (DTS) is installed.
    fn has_dts(&self) -> bool;
    /// What the codec sees on the jack.
    fn jack_status(&self) -> Result<Jack, CallError>;
    /// Judges the card; only asked while PipeWire runs.
    fn sound_state(&self) -> Result<SoundState, CallError>;
    /// The labels of headset-like outputs that AcerUltraX leaves on its `Plain` (no processing)
    /// profile.
    fn plain_headsets(&self) -> Result<Vec<String>, CallError>;
}

/// What the checks need from the domain modules that are not visible from this layer.
pub trait DomainProbes: Send + Sync {
    /// The keyboard's Fn-key interface (a hidraw node), if it is found.
    fn fn_key_node(&self) -> Option<PathBuf>;
    /// How many CPU cores have a frequency ceiling below their maximum (left over from turbo
    /// being off, or a thermal cap).
    fn low_frequency_ceilings(&self) -> usize;
}

/// The operating system as the checks use it, apart from files and programs. [`SystemHost`] is the
/// real one.
pub trait Host: Send + Sync {
    /// An environment variable.
    fn env_var(&self, name: &str) -> Option<String>;
    /// The effective user id of this process.
    fn euid(&self) -> u32;
    /// The user id of a login name.
    fn lookup_uid(&self, user: &str) -> Option<u32>;
    /// Whether an executable of this name is on `PATH`.
    fn program_on_path(&self, name: &str) -> bool;
    /// Whether the process may read (and, with `write`, also write) `path`; the same question as
    /// `access(2)`, which also honours the ACLs udev puts on device nodes.
    fn can_access(&self, path: &Path, write: bool) -> bool;
    /// The current time.
    fn now(&self) -> SystemTime;
}

/// Everything the checks look at. [`Env::system`] is the real machine; tests fill the fields.
pub struct Env {
    /// The prefix of every absolute path the checks read (`/` on a real system).
    pub root: PathBuf,
    /// Runs the programs the checks ask (`systemctl`, `modinfo`, `mokutil`, `dkms status`).
    pub runner: Arc<dyn Runner>,
    /// The operating system.
    pub host: Arc<dyn Host>,
    /// The firmware.
    pub firmware: Arc<dyn Firmware>,
    /// The daemon.
    pub daemon: Arc<dyn Daemon>,
    /// The sound system.
    pub audio: Arc<dyn AudioChecks>,
    /// The domain modules this layer cannot see.
    pub probes: Arc<dyn DomainProbes>,
    /// Loads the configuration the way the program does.
    pub config: Arc<dyn Fn() -> Result<Config, ConfigError> + Send + Sync>,
    /// The running kernel release (`uname -r`).
    pub kernel: String,
    /// This build's version.
    pub version: String,
    /// How many checks run side by side; 0 means `DEFAULT_WORKERS`.
    pub workers: usize,
}

impl Env {
    /// The real machine: the real file system, real programs (each given `PROGRAM_TIMEOUT`), the
    /// running kernel, and the configuration from its standard places. The firmware, the daemon, the
    /// sound system and the domain probes are the caller's to connect.
    pub fn system(
        firmware: Arc<dyn Firmware>,
        daemon: Arc<dyn Daemon>,
        audio: Arc<dyn AudioChecks>,
        probes: Arc<dyn DomainProbes>,
    ) -> Self {
        Self {
            root: PathBuf::from("/"),
            runner: Arc::new(SystemRunner::new(PROGRAM_TIMEOUT)),
            host: Arc::new(SystemHost::new()),
            firmware,
            daemon,
            audio,
            probes,
            config: Arc::new(Config::load_standard),
            kernel: models::kernel_release(),
            version: buildinfo::VERSION.to_owned(),
            workers: DEFAULT_WORKERS,
        }
    }

    /// An absolute path below the root.
    pub(crate) fn path(&self, absolute: &str) -> PathBuf {
        self.root.join(absolute.trim_start_matches('/'))
    }
}

/// One line of the report. The JSON names are the Python edition's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    /// The check's title.
    pub check: String,
    /// Its grade.
    pub status: Status,
    /// One line of detail.
    pub detail: String,
    /// How to fix it.
    pub fix: String,
}

/// The whole health check. The JSON names and order are the Python edition's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// This build's version.
    pub version: String,
    /// The running kernel.
    pub kernel: String,
    /// No check failed (warnings are allowed).
    pub ok: bool,
    /// How many checks failed.
    pub failures: usize,
    /// How many checks warned.
    pub warnings: usize,
    /// Every check, in the order of the list that was run.
    pub checks: Vec<CheckResult>,
}

impl Report {
    /// The process exit code: 1 if anything FAILed (WARN does not fail), else 0.
    pub fn exit_code(&self) -> i32 {
        i32::from(self.failures > 0)
    }
}

/// The text of a panic, for the report line of a check that crashed.
fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned())
}

fn run_one(env: &Env, check: &Check) -> CheckResult {
    match catch_unwind(AssertUnwindSafe(|| (check.run)(env))) {
        Ok(o) => {
            CheckResult { check: check.title.to_owned(), status: o.status, detail: o.detail, fix: o.fix }
        }
        // A check that crashes is a warning that says so, never a crashed doctor.
        Err(p) => CheckResult {
            check: check.title.to_owned(),
            status: Status::Warn,
            detail: format!("check crashed: {}", panic_text(p.as_ref())),
            fix: String::new(),
        },
    }
}

/// Runs the checks side by side, at most `env.workers` at a time, and reports them in the order
/// of `checks`. A check that panics is a WARN that says so.
pub fn run(env: &Env, checks: &[Check]) -> Report {
    let workers = if env.workers == 0 { DEFAULT_WORKERS } else { env.workers }.min(checks.len()).max(1);
    let next = AtomicUsize::new(0);
    let slots: Mutex<Vec<Option<CheckResult>>> = Mutex::new(vec![None; checks.len()]);
    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(check) = checks.get(i) else { break };
                    let result = run_one(env, check);
                    locked(&slots)[i] = Some(result);
                }
            });
        }
    });
    let results: Vec<CheckResult> = slots
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner)
        .into_iter()
        .zip(checks)
        .map(|(slot, check)| {
            // Every slot is filled unless a worker thread itself died; say so rather than hide a check.
            slot.unwrap_or_else(|| CheckResult {
                check: check.title.to_owned(),
                status: Status::Warn,
                detail: "check did not finish".to_owned(),
                fix: String::new(),
            })
        })
        .collect();
    let failures = results.iter().filter(|r| r.status == Status::Fail).count();
    let warnings = results.iter().filter(|r| r.status == Status::Warn).count();
    Report {
        version: env.version.clone(),
        kernel: env.kernel.clone(),
        ok: failures == 0,
        failures,
        warnings,
        checks: results,
    }
}

/// The report for a person; `colour` adds ANSI colours to the status tags (for a terminal).
pub fn render_text(report: &Report, colour: bool) -> String {
    let mut out = format!("TrueSense {} doctor ({})\n", report.version, report.kernel);
    for c in &report.checks {
        let tag = format!("{:<4}", c.status.as_str());
        let tag = if colour { format!("\x1b[{}m{tag}\x1b[0m", c.status.colour()) } else { tag };
        out.push_str(&format!("  [{tag}] {:<16} {}\n", c.check, c.detail));
        if !c.fix.is_empty() && matches!(c.status, Status::Warn | Status::Fail) {
            out.push_str(&format!("         fix: {}\n", c.fix));
        }
    }
    let verdict = match (report.failures, report.warnings) {
        (0, 0) => "ALL GOOD",
        (0, _) => "OK with warnings",
        _ => "PROBLEMS FOUND",
    };
    out.push_str(&format!("\n{verdict}: {} failure(s), {} warning(s)\n", report.failures, report.warnings));
    out
}

/// The report as indented JSON.
pub fn render_json(report: &Report) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(report)
}

#[cfg(test)]
mod tests;
