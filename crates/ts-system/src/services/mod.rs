//! services: `truesense service check` (read-only) and `truesense service repair` (idempotent:
//! running it twice changes nothing the second time).
//!
//! The system part (the daemon and its health timer, the kernel module) needs root; the user
//! part (key service, audio service, start-up, update timer) runs as you. Every step says what it
//! did, and the repair never touches the fans or any setting: it only gets the machinery running
//! again. That is true by construction, not by care: the only programs this module starts are
//! `systemctl`, `modprobe` and `dkms`, every one through the injected [`Runner`], so the logic is
//! tested against a fake systemd and a fake file tree.
#![forbid(unsafe_code)]

use std::fmt;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use ts_platform::firmware::DEVICE_PATH;

/// The units that run as root; `sudo truesense service repair` manages them.
pub const SYSTEM_UNITS: [&str; 2] = ["truesense.service", "truesense-health.timer"];

/// The units that run in the user's session; `truesense service repair` manages them without sudo.
pub const USER_UNITS: [&str; 4] = [
    "truesense-keys.service",
    "truesense-audio.service",
    "truesense-startup.service",
    "truesense-update.timer",
];

/// The daemon's control socket.
pub const SOCKET_PATH: &str = "/run/truesense/truesense.sock";

/// The exit code that stands for "could not run it at all" (not installed, timed out), with the
/// reason in standard error: the shell's own convention.
pub const NOT_RUNNABLE: i32 = 127;

/// How long a repair lets one program run: `systemctl restart` and `dkms autoinstall` can take
/// a while.
pub const REPAIR_TIMEOUT: Duration = Duration::from_secs(40);

/// How often a restarted daemon is asked whether it answers yet.
const POLL: Duration = Duration::from_millis(500);

/// How much of a program's output is kept; the rest is read and thrown away so the program never
/// blocks on a full pipe.
const OUTPUT_LIMIT: u64 = 4 << 20;

/// What a finished program left behind.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Output {
    /// Standard output.
    pub stdout: String,
    /// Standard error; for [`NOT_RUNNABLE`] the reason it did not run.
    pub stderr: String,
    /// The exit status, [`NOT_RUNNABLE`] when the program did not run, -1 when a signal ended it.
    pub code: i32,
}

impl Output {
    /// A successful run that printed `stdout`.
    pub fn success(stdout: impl Into<String>) -> Self {
        Self { stdout: stdout.into(), stderr: String::new(), code: 0 }
    }

    /// A run that ended with `code` and said `stderr`.
    pub fn failure(code: i32, stderr: impl Into<String>) -> Self {
        Self { stdout: String::new(), stderr: stderr.into(), code }
    }

    /// A program that could not be started or was killed for taking too long.
    pub fn not_runnable(reason: impl Into<String>) -> Self {
        Self::failure(NOT_RUNNABLE, reason)
    }
}

/// Runs one external program to completion. It never fails: a program that cannot start is an
/// [`Output`] with code [`NOT_RUNNABLE`], because every caller treats both the same way. The
/// doctor and the service repair share it; tests replace it with a fake.
pub trait Runner: Send + Sync {
    /// Runs `program` with `args` and returns what it left behind.
    fn run(&self, program: &str, args: &[&str]) -> Output;
}

/// The production [`Runner`]: real processes, killed when they exceed their timeout.
///
/// [`ts_platform::hw::ProcessRunner`] is not enough here: it returns standard output only, and the
/// repair needs the exit status and the error text to say why a step failed.
#[derive(Debug, Clone, Copy)]
pub struct SystemRunner {
    timeout: Duration,
}

impl SystemRunner {
    /// A runner that gives each program `timeout` (the doctor uses 6 s, the repair
    /// [`REPAIR_TIMEOUT`]).
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }
}

/// Reads a pipe to its end on a thread, keeping at most [`OUTPUT_LIMIT`] bytes.
fn drain(mut pipe: impl Read + Send + 'static) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut kept = Vec::new();
        let _ = pipe.by_ref().take(OUTPUT_LIMIT).read_to_end(&mut kept);
        let _ = io::copy(&mut pipe, &mut io::sink());
        kept
    })
}

impl Runner for SystemRunner {
    fn run(&self, program: &str, args: &[&str]) -> Output {
        let mut child = match Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(e) => return Output::not_runnable(format!("{program}: {e}")),
        };
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Output::not_runnable(format!("{program}: no output pipes"));
        };
        let (stdout, stderr) = (drain(stdout), drain(stderr));
        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                Ok(None) | Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        let Some(status) = status else {
            // The readers are not waited for: a grandchild that inherited the pipes and outlived
            // the kill would keep them open, and a timeout must end the call.
            return Output::not_runnable(format!("{program}: did not finish in time"));
        };
        let text = |h: thread::JoinHandle<Vec<u8>>| {
            String::from_utf8_lossy(&h.join().unwrap_or_default()).into_owned()
        };
        Output { stdout: text(stdout), stderr: text(stderr), code: status.code().unwrap_or(-1) }
    }
}

/// Asks the daemon whether it is alive. The assembly layer adapts the control-socket client: it
/// returns `true` only when the daemon answered `pong`, and `false` for every kind of failure.
pub trait Pinger: Send + Sync {
    /// Whether the daemon answered.
    fn ping(&self) -> bool;
}

/// Whether a path exists (a `stat` that succeeds); replaced in tests.
pub type Exists = Arc<dyn Fn(&Path) -> bool + Send + Sync>;

/// Waits between two polls of a restarted daemon; replaced in tests.
pub type Sleeper = Arc<dyn Fn(Duration) + Send + Sync>;

/// Where a unit lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// The system manager (root).
    System,
    /// The user's session manager.
    User,
}

impl Scope {
    fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
        }
    }
}

/// One row of the check. The JSON names and order are the Python edition's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unit {
    /// The unit name, or a description for the module and socket rows.
    pub unit: String,
    /// Where it lives.
    pub scope: Scope,
    /// What `systemctl is-enabled` said, `unknown` when nothing, `-` for the module and socket.
    pub enabled: String,
    /// What `systemctl is-active` said, or `present`/`missing`/`answers`/`silent` for the module and socket.
    pub active: String,
    /// Whether this row is as it should be.
    pub ok: bool,
    /// The command that fixes it; empty when `ok`.
    pub fix: String,
}

/// One thing the repair did and whether it worked. It serialises as the Python edition's
/// `[what, ok]` pair, which clients already parse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "(String, bool)", into = "(String, bool)")]
pub struct Step {
    /// What was done, with the reason when it failed.
    pub what: String,
    /// Whether it worked.
    pub ok: bool,
}

impl Step {
    fn new(what: impl Into<String>, ok: bool) -> Self {
        Self { what: what.into(), ok }
    }
}

impl From<(String, bool)> for Step {
    fn from((what, ok): (String, bool)) -> Self {
        Self { what, ok }
    }
}

impl From<Step> for (String, bool) {
    fn from(step: Step) -> Self {
        (step.what, step.ok)
    }
}

/// What a healthy unit looks like: enabled (or static, linked, an alias), and active. A timer is
/// active when it is waiting for its next run.
fn wanted(unit: &Unit) -> bool {
    matches!(unit.enabled.as_str(), "enabled" | "static" | "linked" | "enabled-runtime" | "alias")
        && unit.active == "active"
}

fn or_unknown(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() { "unknown".to_owned() } else { text.to_owned() }
}

/// Why a program failed, for a step: standard error if there is any, else standard output, cut
/// to `limit` characters.
fn reason(out: &Output, limit: usize) -> String {
    let text = if out.stderr.is_empty() { &out.stdout } else { &out.stderr };
    text.trim().chars().take(limit).collect()
}

/// Checks and repairs the services. Build one with [`Manager::new`] and, in tests, replace what
/// it talks to with the `with_*` methods.
pub struct Manager {
    runner: Arc<dyn Runner>,
    exists: Exists,
    pinger: Arc<dyn Pinger>,
    sleep: Sleeper,
    kernel: String,
    wait: Duration,
    socket_path: PathBuf,
    device_path: PathBuf,
}

impl fmt::Debug for Manager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Manager")
            .field("kernel", &self.kernel)
            .field("wait", &self.wait)
            .finish_non_exhaustive()
    }
}

impl Manager {
    /// A manager for the real machine: real programs (each given [`REPAIR_TIMEOUT`]), real
    /// files, and a restarted daemon given 15 s to answer. `pinger` is the daemon connection and
    /// `kernel_release` the kernel the DKMS rebuild is for (`uname -r`).
    pub fn new(pinger: Arc<dyn Pinger>, kernel_release: impl Into<String>) -> Self {
        Self {
            runner: Arc::new(SystemRunner::new(REPAIR_TIMEOUT)),
            exists: Arc::new(|p| p.exists()),
            pinger,
            sleep: Arc::new(thread::sleep),
            kernel: kernel_release.into(),
            wait: Duration::from_secs(15),
            socket_path: PathBuf::from(SOCKET_PATH),
            device_path: PathBuf::from(DEVICE_PATH),
        }
    }

    /// Replaces the program runner.
    #[must_use]
    pub fn with_runner(mut self, runner: Arc<dyn Runner>) -> Self {
        self.runner = runner;
        self
    }

    /// Replaces the file-existence test.
    #[must_use]
    pub fn with_exists(mut self, exists: Exists) -> Self {
        self.exists = exists;
        self
    }

    /// Replaces the pause between two polls of a restarted daemon.
    #[must_use]
    pub fn with_sleeper(mut self, sleep: Sleeper) -> Self {
        self.sleep = sleep;
        self
    }

    /// Changes how long a restarted daemon may take to answer.
    #[must_use]
    pub fn with_wait(mut self, wait: Duration) -> Self {
        self.wait = wait;
        self
    }

    /// Changes where the control socket and the kernel module's device node are looked for.
    #[must_use]
    pub fn with_paths(mut self, socket: impl Into<PathBuf>, device: impl Into<PathBuf>) -> Self {
        self.socket_path = socket.into();
        self.device_path = device.into();
        self
    }

    fn systemctl(&self, user: bool, args: &[&str]) -> Output {
        if user {
            let mut all = vec!["--user"];
            all.extend_from_slice(args);
            self.runner.run("systemctl", &all)
        } else {
            self.runner.run("systemctl", args)
        }
    }

    fn device_present(&self) -> bool {
        (self.exists)(&self.device_path)
    }

    fn socket_present(&self) -> bool {
        (self.exists)(&self.socket_path)
    }

    fn answers(&self) -> bool {
        self.pinger.ping()
    }

    /// Asks systemd about one unit; `ok` and `fix` are left unset.
    pub fn unit_state(&self, unit: &str, user: bool) -> Unit {
        Unit {
            unit: unit.to_owned(),
            scope: if user { Scope::User } else { Scope::System },
            enabled: or_unknown(&self.systemctl(user, &["is-enabled", unit]).stdout),
            active: or_unknown(&self.systemctl(user, &["is-active", unit]).stdout),
            ok: false,
            fix: String::new(),
        }
    }

    /// Read-only: one row per unit, then the kernel module and the control socket. Nothing is
    /// started, enabled or restarted.
    pub fn check(&self) -> Vec<Unit> {
        let mut rows = Vec::new();
        for (user, units) in [(false, &SYSTEM_UNITS[..]), (true, &USER_UNITS[..])] {
            for name in units {
                let mut row = self.unit_state(name, user);
                row.ok = wanted(&row);
                if !row.ok {
                    row.fix = if user { "truesense service repair" } else { "sudo truesense service repair" }
                        .to_owned();
                }
                rows.push(row);
            }
        }
        let module = self.device_present();
        rows.push(Unit {
            unit: "kernel module (/dev/truesense)".to_owned(),
            scope: Scope::System,
            enabled: "-".to_owned(),
            active: if module { "present" } else { "missing" }.to_owned(),
            ok: module,
            fix: if module { "" } else { "sudo truesense service repair" }.to_owned(),
        });
        let socket = self.socket_present();
        let answered = socket && self.answers();
        rows.push(Unit {
            unit: "control socket".to_owned(),
            scope: Scope::System,
            enabled: "-".to_owned(),
            active: if answered {
                "answers"
            } else if socket {
                "silent"
            } else {
                "missing"
            }
            .to_owned(),
            ok: answered,
            fix: if answered { "" } else { "sudo truesense service repair" }.to_owned(),
        });
        rows
    }

    /// Runs one repair command and records it; a failure says why (cut to 100 characters).
    fn step(&self, steps: &mut Vec<Step>, what: &str, program: &str, args: &[&str]) -> bool {
        let out = self.runner.run(program, args);
        let ok = out.code == 0;
        if ok {
            steps.push(Step::new(what, true));
        } else {
            let why = reason(&out, 100);
            steps
                .push(Step::new(format!("{what} ({})", if why.is_empty() { "failed" } else { &why }), false));
        }
        ok
    }

    /// Gets the system part running again. Run it as root. Every step is recorded and nothing
    /// is raised: a step that did not work is a [`Step`] with `ok` false and the reason. A healthy
    /// daemon is left alone.
    pub fn repair_system(&self) -> Vec<Step> {
        let mut steps = Vec::new();
        self.step(&mut steps, "systemd re-reads the unit files", "systemctl", &["daemon-reload"]);
        if !self.device_present() {
            self.runner.run("modprobe", &["truesense_wmi"]);
            if !self.device_present() {
                // The module is built per kernel: after a kernel update it may simply not exist yet.
                let what = format!("rebuilt the kernel module for {} (DKMS)", self.kernel);
                self.step(&mut steps, &what, "dkms", &["autoinstall", "-k", &self.kernel]);
                self.runner.run("modprobe", &["truesense_wmi"]);
            }
            let present = self.device_present();
            steps.push(Step::new(
                if present {
                    "kernel module loaded"
                } else {
                    "kernel module still missing: is this a supported laptop? (truesense doctor)"
                },
                present,
            ));
        }
        for unit in SYSTEM_UNITS {
            let state = self.unit_state(unit, false);
            if !matches!(state.enabled.as_str(), "enabled" | "static" | "linked" | "alias") {
                self.step(&mut steps, &format!("enabled {unit}"), "systemctl", &["enable", unit]);
            }
        }
        self.runner.run("systemctl", &["reset-failed", "truesense.service"]);
        let service = self.unit_state("truesense.service", false);
        let timer = self.unit_state("truesense-health.timer", false);
        if timer.active != "active" {
            self.step(
                &mut steps,
                "started the health timer",
                "systemctl",
                &["start", "truesense-health.timer"],
            );
        }
        if service.active == "active" && self.socket_present() && self.answers() {
            steps.push(Step::new("truesense.service is running and answers: left alone", true));
            return steps;
        }
        self.step(&mut steps, "restarted truesense.service", "systemctl", &["restart", "truesense.service"]);
        let polls = (self.wait.as_millis() / POLL.as_millis()) as u64;
        for _ in 0..polls {
            if self.socket_present() && self.answers() {
                break;
            }
            (self.sleep)(POLL);
        }
        let up = self.socket_present() && self.answers();
        steps.push(Step::new(
            if up {
                "the service answers on its control socket"
            } else {
                "the service does not answer: see  journalctl -u truesense -b"
            },
            up,
        ));
        steps
    }

    /// Enables what is not enabled and starts what is not running, as the user. A running unit is
    /// left alone.
    pub fn repair_user(&self) -> Vec<Step> {
        let reload = self.systemctl(true, &["daemon-reload"]);
        let mut steps = vec![Step::new("user systemd re-reads the unit files", reload.code == 0)];
        for unit in USER_UNITS {
            if wanted(&self.unit_state(unit, true)) {
                continue;
            }
            let out = self.systemctl(true, &["enable", "--now", unit]);
            // `enable --now` can report an error for a unit that nevertheless came up.
            let ok = out.code == 0 || self.unit_state(unit, true).active == "active";
            if ok {
                steps.push(Step::new(format!("enabled and started {unit}"), true));
            } else {
                let why = reason(&out, 90);
                let why = if why.is_empty() { "is it installed?" } else { &why };
                steps.push(Step::new(format!("could not start {unit} ({why})"), false));
            }
        }
        if steps.len() == 1 {
            steps.push(Step::new("every user service is already running", true));
        }
        steps
    }
}

/// The check as text, one line per row.
pub fn render(rows: &[Unit]) -> String {
    rows.iter()
        .map(|r| {
            let mark = if r.ok { "ok " } else { "!! " };
            let mut line =
                format!("  [{mark}] {:<34} {:<6} {:<9} {}", r.unit, r.scope.as_str(), r.enabled, r.active);
            if !r.fix.is_empty() {
                line.push_str(&format!("   -> {}", r.fix));
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests;
