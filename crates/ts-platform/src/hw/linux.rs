//! The plain Linux readings next to the firmware: CPU package temperature (coretemp hwmon),
//! GPU temperature (`nvidia-smi`) and whether the laptop is on mains power.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// How long `nvidia-smi` may take: a wedged NVIDIA driver must not wedge the daemon's tick.
const GPU_QUERY_TIMEOUT: Duration = Duration::from_secs(3);

/// Runs an external program and returns what it printed on standard output, whatever its exit
/// status (like the Python edition, which read the output of a failed `nvidia-smi` and found
/// no number in it).
pub trait CommandRunner: Send + Sync {
    /// Runs `program` with `args`, giving up after `timeout`.
    fn output(&self, program: &str, args: &[&str], timeout: Duration) -> io::Result<Vec<u8>>;
}

/// Runs real processes, killing one that exceeds its timeout.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessRunner;

impl CommandRunner for ProcessRunner {
    fn output(&self, program: &str, args: &[&str], timeout: Duration) -> io::Result<Vec<u8>> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut stdout = child.stdout.take().ok_or_else(|| io::Error::other("no stdout pipe"))?;
        // Read on a thread so a program that prints more than a pipe holds cannot block its own exit.
        let reader = thread::spawn(move || {
            let mut out = Vec::new();
            stdout.read_to_end(&mut out).map(|_| out)
        });
        let deadline = Instant::now() + timeout;
        loop {
            if child.try_wait()?.is_some() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("{program} did not finish in time"),
                ));
            }
            thread::sleep(Duration::from_millis(10));
        }
        reader.join().map_err(|_| io::Error::other("reader thread panicked"))?
    }
}

/// The Linux side of the sensors. Paths are taken below `root` (the real `/` in production, a
/// temporary tree in tests) and the program runner is injected.
#[derive(Clone)]
pub struct LinuxSensors {
    root: PathBuf,
    runner: Arc<dyn CommandRunner>,
}

impl LinuxSensors {
    /// Sensors below `root`, running programs with `runner`.
    pub fn new(root: impl Into<PathBuf>, runner: Arc<dyn CommandRunner>) -> Self {
        Self { root: root.into(), runner }
    }

    /// The real machine.
    pub fn system() -> Self {
        Self::new("/", Arc::new(ProcessRunner))
    }

    fn path(&self, absolute: &str) -> PathBuf {
        self.root.join(absolute.trim_start_matches('/'))
    }

    /// The CPU package temperature in degrees C: the hottest `temp*_input` of the `coretemp`
    /// hwmon. `None` when there is none (not an Intel CPU, or the module is missing).
    pub fn cpu_temp(&self) -> Option<f64> {
        let mut best: Option<f64> = None;
        for hwmon in list_dir(&self.path("/sys/class/hwmon"), |n| n.starts_with("hwmon")) {
            let is_coretemp = fs::read_to_string(hwmon.join("name")).is_ok_and(|n| n.trim() == "coretemp");
            if !is_coretemp {
                continue;
            }
            for input in list_dir(&hwmon, |n| n.starts_with("temp") && n.ends_with("_input")) {
                // A file that cannot be read or is not a number is skipped, not an error: a
                // sensor that is gone must not hide the others.
                let Some(millidegrees) =
                    fs::read_to_string(input).ok().and_then(|t| t.trim().parse::<i64>().ok())
                else {
                    continue;
                };
                let degrees = millidegrees as f64 / 1000.0;
                best = Some(best.map_or(degrees, |b| b.max(degrees)));
            }
        }
        best
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
