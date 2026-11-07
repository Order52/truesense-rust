//! The plain Linux readings next to the firmware: CPU package temperature (coretemp hwmon),
//! GPU temperature (`nvidia-smi`) and whether the laptop is on mains power.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread;
use std::time::{Duration, Instant};

/// How long `nvidia-smi` may take: a wedged NVIDIA driver must not wedge the daemon's tick.
const GPU_QUERY_TIMEOUT: Duration = Duration::from_secs(3);

/// What a finished program left behind: its exit status and both output streams.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommandOutput {
    /// Exit status; `None` when a signal ended the program.
    pub code: Option<i32>,
    /// Everything written to standard output.
    pub stdout: Vec<u8>,
    /// Everything written to standard error.
    pub stderr: Vec<u8>,
}

impl CommandOutput {
    /// True when the program exited with status 0.
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
}

/// Runs an external program. [`output`](CommandRunner::output) returns only what it printed on
/// standard output, whatever its exit status (like the Python edition, which read the output of
/// a failed `nvidia-smi` and found no number in it); [`run`](CommandRunner::run) also reports the
/// exit status and standard error, for callers that must tell failure from empty output.
pub trait CommandRunner: Send + Sync {
    /// Runs `program` with `args`, giving up after `timeout`.
    fn output(&self, program: &str, args: &[&str], timeout: Duration) -> io::Result<Vec<u8>>;

    /// Like [`output`](CommandRunner::output) but with the exit status and standard error. The
    /// default implementation only knows the output (status unknown, no error text); real
    /// runners override it.
    fn run(&self, program: &str, args: &[&str], timeout: Duration) -> io::Result<CommandOutput> {
        self.output(program, args, timeout).map(|stdout| CommandOutput {
            code: None,
            stdout,
            stderr: Vec::new(),
        })
    }
}

/// Runs real processes, killing one that exceeds its timeout.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessRunner;

/// The most standard error text kept from a program: a runaway one must not fill memory.
const MAX_STDERR: u64 = 64 * 1024;

impl CommandRunner for ProcessRunner {
    fn output(&self, program: &str, args: &[&str], timeout: Duration) -> io::Result<Vec<u8>> {
        self.run(program, args, timeout).map(|o| o.stdout)
    }

    fn run(&self, program: &str, args: &[&str], timeout: Duration) -> io::Result<CommandOutput> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut stdout = child.stdout.take().ok_or_else(|| io::Error::other("no stdout pipe"))?;
        let stderr = child.stderr.take().ok_or_else(|| io::Error::other("no stderr pipe"))?;
        // Read on threads so a program that prints more than a pipe holds cannot block its own exit.
        // The standard-output reader also tells this thread when the program closed its output
        // (normally: when it exited), so waiting for a helper is one blocking wait and not a poll
        // every few milliseconds (a `pactl` call used to cost about ten wake-ups of the caller).
        let (done, finished) = mpsc::channel::<io::Result<Vec<u8>>>();
        let out_reader = thread::spawn(move || {
            let mut out = Vec::new();
            let result = stdout.read_to_end(&mut out).map(|_| out);
            let _ = done.send(result);
        });
        let err_reader = thread::spawn(move || {
            let mut out = Vec::new();
            stderr.take(MAX_STDERR).read_to_end(&mut out).map(|_| out)
        });
        let deadline = Instant::now() + timeout;
        let stdout = match finished.recv_timeout(timeout) {
            Ok(result) => result,
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = out_reader.join();
                let _ = err_reader.join();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("{program} did not finish in time"),
                ));
            }
        };
        let _ = out_reader.join();
        // The output is closed; the program is normally gone already. One that closed its output and
        // keeps running still meets the deadline.
        let mut backoff = ts_foundation::backoff::Backoff::new();
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                let _ = err_reader.join();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("{program} did not finish in time"),
                ));
            }
            backoff.wait();
        };
        let stdout = stdout?;
        let stderr = err_reader.join().map_err(|_| io::Error::other("reader thread panicked"))??;
        Ok(CommandOutput { code: status.code(), stdout, stderr })
    }
}

/// The Linux side of the sensors. Paths are taken below `root` (the real `/` in production, a
/// temporary tree in tests) and the program runner is injected.
#[derive(Clone)]
pub struct LinuxSensors {
    root: PathBuf,
    runner: Arc<dyn CommandRunner>,
    /// The coretemp inputs found by the last scan, shared by the clones.
    cpu: Arc<Mutex<CpuInputs>>,
}

/// How often the hwmon tree is scanned again while the CPU sensor keeps answering, in reads: a
/// module reload renumbers `hwmon*`, which also makes a read fail and so a scan sooner.
const RESCAN_EVERY: u32 = 256;

/// What a scan of the hwmon tree found.
#[derive(Default)]
struct CpuInputs {
    /// The `Package id` input of each coretemp device; when there is one, only it is read.
    packages: Vec<PathBuf>,
    /// Every `temp*_input` of the coretemp devices that have no package input.
    cores: Vec<PathBuf>,
    /// Reads since the last scan.
    reads: u32,
    scanned: bool,
}

impl LinuxSensors {
    /// Sensors below `root`, running programs with `runner`.
    pub fn new(root: impl Into<PathBuf>, runner: Arc<dyn CommandRunner>) -> Self {
        Self { root: root.into(), runner, cpu: Arc::new(Mutex::new(CpuInputs::default())) }
    }

    /// The real machine.
    pub fn system() -> Self {
        Self::new("/", Arc::new(ProcessRunner))
    }

    fn path(&self, absolute: &str) -> PathBuf {
        self.root.join(absolute.trim_start_matches('/'))
    }

    /// The CPU package temperature in degrees C: the `Package id` input of the `coretemp` hwmon
    /// (what the Go edition always read), or, on a CPU whose coretemp has no package input, the
    /// hottest `temp*_input`. `None` when there is no coretemp (not an Intel CPU, or the module is
    /// missing).
    ///
    /// Why only the package input: every core's input is a model-specific register read on that
    /// core, an inter-processor interrupt that wakes it from idle. On the 24-thread PH16-71 the 17
    /// inputs cost 16 wake-ups and about 1.4 ms of kernel time per read, once a second, which was
    /// most of the daemon's CPU use; the package input is one read. (On a single-core burst the
    /// package value can lag the hottest core by a few degrees for a second or two; the firmware's
    /// own CPU temperature, which the daemon also takes the maximum with, covers that.)
    pub fn cpu_temp(&self) -> Option<f64> {
        let mut cache = self.cpu.lock().unwrap_or_else(PoisonError::into_inner);
        if !cache.scanned || cache.reads >= RESCAN_EVERY {
            self.scan(&mut cache);
        }
        cache.reads = cache.reads.saturating_add(1);
        let reading = read_hottest(if cache.packages.is_empty() { &cache.cores } else { &cache.packages });
        if reading.is_some() || cache.reads == 1 {
            return reading;
        }
        // The sensor stopped answering (a module reload renumbers the devices): look again once.
        self.scan(&mut cache);
        cache.reads = 1;
        read_hottest(if cache.packages.is_empty() { &cache.cores } else { &cache.packages })
    }

    fn scan(&self, cache: &mut CpuInputs) {
        cache.packages.clear();
        cache.cores.clear();
        cache.reads = 0;
        cache.scanned = true;
        for hwmon in list_dir(&self.path("/sys/class/hwmon"), |n| n.starts_with("hwmon")) {
            let is_coretemp = fs::read_to_string(hwmon.join("name")).is_ok_and(|n| n.trim() == "coretemp");
            if !is_coretemp {
                continue;
            }
            for input in list_dir(&hwmon, |n| n.starts_with("temp") && n.ends_with("_input")) {
                let label = input.with_file_name(
                    input.file_name().and_then(|n| n.to_str()).unwrap_or("").replace("_input", "_label"),
                );
                let is_package =
                    fs::read_to_string(label).is_ok_and(|l| l.trim_start().starts_with("Package id"));
                if is_package {
                    cache.packages.push(input);
                } else {
                    cache.cores.push(input);
                }
            }
        }
    }

    /// The NVIDIA GPU temperature from `nvidia-smi`, or `None` when it cannot be read.
    pub fn gpu_temp(&self) -> Option<f64> {
        let out = self
            .runner
            .output(
                "nvidia-smi",
                &["--query-gpu=temperature.gpu", "--format=csv,noheader,nounits"],
                GPU_QUERY_TIMEOUT,
            )
            .ok()?;
        let value: f64 = String::from_utf8_lossy(&out).split_whitespace().next()?.parse().ok()?;
        value.is_finite().then_some(value)
    }

    /// Whether the laptop is on mains power: the first supply of the usual names that reports
    /// `online`; `true` when none does (a desktop-like machine, or unreadable).
    pub fn on_ac(&self) -> bool {
        for name in ["ACAD", "AC", "AC0", "ADP0", "ADP1"] {
            if let Ok(text) = fs::read_to_string(self.path(&format!("/sys/class/power_supply/{name}/online")))
            {
                return text.trim() == "1";
            }
        }
        true
    }
}

/// The entries of `dir` whose name passes `keep`, sorted; empty when the directory cannot be read.
/// The hottest of the readable inputs, in degrees. A file that cannot be read or is not a number is
/// skipped, not an error: a sensor that is gone must not hide the others.
fn read_hottest(inputs: &[PathBuf]) -> Option<f64> {
    let mut best: Option<f64> = None;
    for input in inputs {
        let Some(millidegrees) = fs::read_to_string(input).ok().and_then(|t| t.trim().parse::<i64>().ok())
        else {
            continue;
        };
        let degrees = millidegrees as f64 / 1000.0;
        best = Some(best.map_or(degrees, |b| b.max(degrees)));
    }
    best
}

fn list_dir(dir: &Path, keep: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_str().is_some_and(&keep))
        .map(|e| e.path())
        .collect();
    paths.sort();
    paths
}
