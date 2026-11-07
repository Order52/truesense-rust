//! sensors: the machine's own readings that the daemon decides from and the TUI, the window and
//! the status bars show: CPU load (from `/proc/stat`), battery charge and state (from
//! `/sys/class/power_supply`), and the GPU's temperature, load and power draw (streamed from
//! `nvidia-smi`, and only when something smart needs it).
//!
//! The temperatures of the CPU package and of the GPU, and whether the laptop is on mains
//! power, are read by `ts_platform::hw::LinuxSensors`; this module does not repeat them.
//!
//! Every file path and process is injected: a root directory for `/proc` and `/sys`, a line
//! source for the `nvidia-smi` stream, a clock and a sleeper. Nothing here fails loudly: a
//! sensor that is gone gives the last reading or a neutral default, because a broken sensor
//! must never stop the control loop that reads it.
#![forbid(unsafe_code)]

use std::fs;
use std::io::{self, BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use ts_platform::hw::Sleeper;

/// Weight of a new CPU load sample (the daemon smooths the jittery `/proc/stat` deltas without
/// a delay long enough to hide a real rise).
pub const CPU_LOAD_ALPHA: f64 = 0.25;

/// Weight of a new GPU load or power sample.
pub const GPU_ALPHA: f64 = 0.3;

/// Pause before the `nvidia-smi` stream is restarted after it ended or failed to start.
pub const GPU_RESTART_DELAY: Duration = Duration::from_secs(5);

/// The seconds since the Unix epoch, as a float (the `time.time()` of the Python edition).
pub type Clock = Arc<dyn Fn() -> f64 + Send + Sync>;

/// One step of an exponential moving average: `prev + alpha * (value - prev)`, and just `value`
/// when there is no previous reading.
pub fn ema(prev: Option<f64>, value: f64, alpha: f64) -> f64 {
    prev.map_or(value, |p| p + alpha * (value - p))
}

/// A running exponential moving average.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ema {
    alpha: f64,
    value: Option<f64>,
}

impl Ema {
    /// An average whose first sample is taken as it is.
    pub fn new(alpha: f64) -> Self {
        Self { alpha, value: None }
    }

    /// An average that starts at `initial`, so the first sample only moves it by `alpha` of the
    /// difference. The daemon's load readings start at 0 this way, which makes the first
    /// seconds after start-up read low rather than high.
    pub fn starting_at(alpha: f64, initial: f64) -> Self {
        Self { alpha, value: Some(initial) }
    }

    /// Folds in a sample and returns the new average.
    pub fn update(&mut self, sample: f64) -> f64 {
        let v = ema(self.value, sample, self.alpha);
        self.value = Some(v);
        v
    }

    /// The average, or `None` before any sample (and without a starting value).
    pub fn value(&self) -> Option<f64> {
        self.value
    }
}

/// The state of the first battery that can be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Battery {
    /// Charge in percent (the `capacity` file).
    pub percent: i64,
    /// The kernel's state word: `Charging`, `Discharging`, `Full`, `Not charging`, `Unknown`.
    pub status: String,
}

/// The latest GPU reading. Load and power are smoothed; the temperature is the raw reading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpuReading {
    /// Temperature in degrees C, `None` until the first line arrived.
    pub temp: Option<f64>,
    /// Load in percent, smoothed; 0 until the first line arrived.
    pub util: f64,
    /// Power draw in watts, smoothed; 0 until the first line arrived.
    pub power: f64,
    /// When the last line arrived (seconds since the epoch), `None` before the first. A reading
    /// older than a few seconds means the stream is stuck and the temperature must not be trusted.
    pub seen: Option<f64>,
}

/// Parses one line of `nvidia-smi --query-gpu=temperature.gpu,utilization.gpu,power.draw
/// --format=csv,noheader,nounits`: `61, 40, 55.5`. `None` for anything else, including a line
/// with a field the driver could not give (`[N/A]`) and a number that is not finite (a NaN
/// would poison the running averages for good).
pub fn parse_gpu_line(line: &str) -> Option<(f64, f64, f64)> {
    let mut fields = line.split(',').map(str::trim);
    let (t, u, w) = (fields.next()?, fields.next()?, fields.next()?);
    if fields.next().is_some() {
        return None;
    }
    let number = |s: &str| s.parse::<f64>().ok().filter(|v| v.is_finite());
    Some((number(t)?, number(u)?, number(w)?))
}

/// Reads stdout of a long-running program line by line.
pub trait LineSource: Send + Sync {
    /// Starts `program` and calls `on_line` for each line it prints, until it ends or `on_line`
    /// returns `false` (the program is then stopped). An `Err` means it could not be started
    /// or read.
    fn stream(&self, program: &str, args: &[&str], on_line: &mut dyn FnMut(&str) -> bool) -> io::Result<()>;
}

/// The longest line read from a helper; `nvidia-smi` lines are a few dozen bytes, and the rest
/// of anything longer is junk to be ignored, not memory to be filled.
const MAX_LINE: u64 = 4096;

/// Runs real processes.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessLineSource;

impl LineSource for ProcessLineSource {
    fn stream(&self, program: &str, args: &[&str], on_line: &mut dyn FnMut(&str) -> bool) -> io::Result<()> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdout = child.stdout.take().ok_or_else(|| io::Error::other("no stdout pipe"))?;
        let mut reader = BufReader::new(stdout);
        let mut buf = Vec::new();
        let result = loop {
            buf.clear();
            match (&mut reader).take(MAX_LINE).read_until(b'\n', &mut buf) {
                Ok(0) => break Ok(()),
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buf);
                    if !on_line(line.trim_end_matches(['\n', '\r'])) {
                        break Ok(());
                    }
                }
                Err(e) => break Err(e),
            }
        };
        // Whatever ended the loop, the program must not be left running behind us.
        let _ = child.kill();
        let _ = child.wait();
        result
    }
}

/// CPU counters of the previous sample.
#[derive(Debug, Clone, Copy)]
struct CpuCounters {
    total: u64,
    idle: u64,
}

#[derive(Debug)]
struct CpuState {
    previous: Option<CpuCounters>,
    load: Ema,
}

#[derive(Debug)]
struct GpuState {
    temp: Option<f64>,
    util: Ema,
    power: Ema,
    seen: Option<f64>,
}

/// Turns raw counters and streams into readings. Shared between threads: the daemon's tick
/// reads the CPU load and battery while a stream thread feeds in GPU lines.
pub struct Sampler {
    root: PathBuf,
    clock: Clock,
    cpu: Mutex<CpuState>,
    gpu: Mutex<GpuState>,
}

impl Sampler {
    /// A sampler reading below `root` (`/` in production, a temporary tree in tests) and
    /// stamping GPU lines with `clock`.
    pub fn new(root: impl Into<PathBuf>, clock: Clock) -> Self {
        Self {
            root: root.into(),
            clock,
            cpu: Mutex::new(CpuState { previous: None, load: Ema::starting_at(CPU_LOAD_ALPHA, 0.0) }),
            gpu: Mutex::new(GpuState {
                temp: None,
                util: Ema::starting_at(GPU_ALPHA, 0.0),
                power: Ema::starting_at(GPU_ALPHA, 0.0),
                seen: None,
            }),
        }
    }

    /// The real machine, with the wall clock.
    pub fn system() -> Self {
        Self::new("/", Arc::new(wall_clock))
    }

    fn path(&self, absolute: &str) -> PathBuf {
        self.root.join(absolute.trim_start_matches('/'))
    }

    /// Samples `/proc/stat` and returns the smoothed CPU busy percentage (0 until a second
    /// sample exists, and the previous value whenever the file cannot be understood).
    ///
    /// Busy is the share of the time since the last call not spent idle or waiting for I/O.
    pub fn cpu_load(&self) -> f64 {
        let mut state = self.cpu.lock().unwrap_or_else(PoisonError::into_inner);
        let current = fs::read(self.path("/proc/stat"))
            .ok()
            .and_then(|b| parse_cpu_counters(&String::from_utf8_lossy(&b)));
        if let Some(now) = current {
            if let Some(prev) = state.previous
                && now.total > prev.total
            {
                let idle_delta = now.idle as f64 - prev.idle as f64;
                let busy = 100.0 * (1.0 - idle_delta / (now.total - prev.total) as f64);
                // Counters that went backwards on one side would give a share outside 0..100.
                state.load.update(busy.clamp(0.0, 100.0));
            }
            state.previous = Some(now);
        }
        state.load.value().unwrap_or(0.0)
    }

    /// The charge of the first readable battery in percent; 100 when there is none (a desktop,
    /// or no battery driver), so "battery low" rules never fire without a battery.
    pub fn battery_pct(&self) -> f64 {
        for dir in self.battery_dirs() {
            if let Some(v) = fs::read_to_string(dir.join("capacity"))
                .ok()
                .and_then(|t| t.trim().parse::<f64>().ok())
                .filter(|v| v.is_finite())
            {
                return v;
            }
        }
        100.0
    }

    /// The charge and state of the first battery whose both files can be read, for the status
    /// line of the TUI and the window; `None` without one.
    pub fn battery(&self) -> Option<Battery> {
        self.battery_dirs().into_iter().find_map(|dir| {
            let percent = fs::read_to_string(dir.join("capacity")).ok()?.trim().parse().ok()?;
            let status = fs::read_to_string(dir.join("status")).ok()?.trim().to_owned();
            Some(Battery { percent, status })
        })
    }

    /// The `BAT*` directories, in name order so the answer does not depend on directory order.
    fn battery_dirs(&self) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(self.path("/sys/class/power_supply")) else { return Vec::new() };
        let mut dirs: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_str().is_some_and(|n| n.starts_with("BAT")))
            .map(|e| e.path())
            .collect();
        dirs.sort();
        dirs
    }

    /// The latest GPU reading (the empty reading until the stream produced one).
    pub fn gpu(&self) -> GpuReading {
        let g = self.gpu.lock().unwrap_or_else(PoisonError::into_inner);
        GpuReading {
            temp: g.temp,
            util: g.util.value().unwrap_or(0.0),
            power: g.power.value().unwrap_or(0.0),
            seen: g.seen,
        }
    }

    /// Folds one `nvidia-smi` line into the readings. Returns `false`, changing nothing, for a
    /// line that is not `temp, util, power`.
    pub fn feed_gpu_line(&self, line: &str) -> bool {
        let Some((temp, util, power)) = parse_gpu_line(line) else { return false };
        let mut g = self.gpu.lock().unwrap_or_else(PoisonError::into_inner);
        g.temp = Some(temp);
        g.util.update(util);
        g.power.update(power);
        g.seen = Some((self.clock)());
        true
    }

    /// Keeps the `nvidia-smi` stream running and feeds its lines in, until `stop` says so.
    /// Blocks: run it on its own thread, and only when a smart feature needs the GPU (the
    /// stream keeps the driver awake). A stream that ends or cannot start is restarted after
    /// [`GPU_RESTART_DELAY`], so a machine without `nvidia-smi` just never gets a reading.
    pub fn run_gpu_stream(&self, source: &dyn LineSource, stop: &dyn Fn() -> bool, sleeper: &Sleeper) {
        let args = [
            "--query-gpu=temperature.gpu,utilization.gpu,power.draw",
            "--format=csv,noheader,nounits",
            "-l",
            "2",
        ];
        while !stop() {
            // A failure to start is the same as the stream ending: wait, then try again.
            let _ = source.stream("nvidia-smi", &args, &mut |line| {
                self.feed_gpu_line(line);
                !stop()
            });
            if stop() {
                break;
            }
            sleeper(GPU_RESTART_DELAY);
        }
    }
}

/// The wall clock in seconds since the epoch.
fn wall_clock() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

/// The busy-relevant counters of the first line of `/proc/stat`: `cpu user nice system idle
/// iowait irq softirq steal ...`. The total is the sum of the first eight numbers; idle is
/// `idle + iowait`. `None` when the line is not that.
fn parse_cpu_counters(stat: &str) -> Option<CpuCounters> {
    let line = stat.lines().next()?;
    let mut fields = line.split_whitespace();
    if fields.next()? != "cpu" {
        return None;
    }
    let mut values = [0u64; 8];
    for slot in &mut values {
        *slot = fields.next()?.parse().ok()?;
    }
    let total = values.iter().try_fold(0u64, |a, v| a.checked_add(*v))?;
    Some(CpuCounters { total, idle: values[3].checked_add(values[4])? })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let p = std::env::temp_dir().join(format!(
                "ts-system-sensors-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn put(&self, rel: &str, text: &str) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn sampler(t: &TempDir) -> Sampler {
        Sampler::new(&t.0, Arc::new(|| 1000.0))
    }

    #[test]
    fn ema_takes_the_first_sample_as_it_is_and_smooths_the_rest() {
        assert_eq!(ema(None, 10.0, 0.5), 10.0);
        assert_eq!(ema(Some(10.0), 20.0, 0.5), 15.0);
        let mut e = Ema::new(0.5);
        assert_eq!(e.value(), None);
        assert_eq!(e.update(10.0), 10.0);
        assert_eq!(e.update(20.0), 15.0);
        assert_eq!(e.value(), Some(15.0));
    }

    #[test]
    fn ema_can_start_from_a_value_like_the_daemons_load_readings() {
        let mut e = Ema::starting_at(0.25, 0.0);
        assert_eq!(e.value(), Some(0.0));
        assert_eq!(e.update(40.0), 10.0, "0 + 0.25 * 40: the first sample is not taken as it is");
    }

    #[test]
    fn cpu_load_needs_two_samples_and_is_smoothed_from_zero() {
        let t = TempDir::new();
        t.put("proc/stat", "cpu  100 0 100 800 0 0 0 0 0 0\ncpu0 1 1 1 1 1 1 1 1\n");
        let s = sampler(&t);
        assert_eq!(s.cpu_load(), 0.0, "nothing to compare with yet");
        // 200 more busy and 200 more idle ticks: 50 % busy; 0.25 of that from the start value 0.
        t.put("proc/stat", "cpu  200 0 200 1000 0 0 0 0 0 0\n");
        assert_eq!(s.cpu_load(), 12.5);
        t.put("proc/stat", "cpu  300 0 300 1200 0 0 0 0 0 0\n");
        assert_eq!(s.cpu_load(), 12.5 + 0.25 * (50.0 - 12.5));
    }

    #[test]
    fn iowait_counts_as_idle_and_steal_as_busy() {
        let t = TempDir::new();
        // user nice system idle iowait irq softirq steal
        t.put("proc/stat", "cpu  0 0 0 0 0 0 0 0\n");
        let s = sampler(&t);
        s.cpu_load();
        t.put("proc/stat", "cpu  0 0 0 50 50 0 0 0\n");
        assert_eq!(s.cpu_load(), 0.0, "all idle or waiting for I/O");
        t.put("proc/stat", "cpu  0 0 0 50 50 0 0 100\n");
        assert_eq!(s.cpu_load(), 0.25 * 100.0, "steal time is busy");
        // Counters past the eighth field (guest, guest_nice) are already inside user and nice.
        t.put("proc/stat", "cpu  0 0 0 50 50 0 0 100 999 999\n");
        assert_eq!(s.cpu_load(), 25.0, "no change in the eight counted numbers: unchanged load");
    }

    #[test]
    fn cpu_load_keeps_the_last_value_when_the_file_is_unusable() {
        let t = TempDir::new();
        let s = sampler(&t);
        assert_eq!(s.cpu_load(), 0.0, "no /proc/stat at all");
        t.put("proc/stat", "cpu  100 0 100 800 0 0 0 0\n");
        s.cpu_load();
        t.put("proc/stat", "cpu  200 0 200 1000 0 0 0 0\n");
        let v = s.cpu_load();
        for junk in [
            "",
            "cpu",
            "cpu  1 2 3",
            "cpu  a b c d e f g h",
            "intr 1 2 3",
            "cpu  -1 0 0 0 0 0 0 0",
            "\u{0}\u{ffff}",
        ] {
            t.put("proc/stat", junk);
            assert_eq!(s.cpu_load(), v, "{junk:?}");
        }
        // A counter that went backwards (a reset) only re-bases the sample.
        t.put("proc/stat", "cpu  1 0 1 8 0 0 0 0\n");
        assert_eq!(s.cpu_load(), v);
        t.put("proc/stat", "cpu  101 0 101 108 0 0 0 0\n");
        assert!(s.cpu_load() > v, "load resumes from the re-based counters");
    }

    #[test]
    fn counters_that_do_not_add_up_stay_inside_zero_to_one_hundred() {
        let t = TempDir::new();
        t.put("proc/stat", "cpu  0 0 0 100 0 0 0 0\n");
        let s = sampler(&t);
        s.cpu_load();
        // Idle went DOWN while the total went up: a share above 100 % without the clamp.
        t.put("proc/stat", "cpu  500 0 0 50 0 0 0 0\n");
        assert_eq!(s.cpu_load(), 25.0, "clamped to 100 then smoothed from 0");
        // Overflowing counters are refused rather than wrapped.
        t.put("proc/stat", &format!("cpu  {} {} 0 0 0 0 0 0\n", u64::MAX, u64::MAX));
        assert_eq!(s.cpu_load(), 25.0);
    }

    #[test]
    fn battery_percent_defaults_to_full_without_a_battery() {
        let t = TempDir::new();
        assert_eq!(sampler(&t).battery_pct(), 100.0);
        assert_eq!(sampler(&t).battery(), None);
        t.put("sys/class/power_supply/AC/online", "1\n");
        assert_eq!(sampler(&t).battery_pct(), 100.0, "a mains supply is not a battery");
    }

    #[test]
    fn battery_reads_the_first_readable_battery_in_name_order() {
        let t = TempDir::new();
        t.put("sys/class/power_supply/BAT1/capacity", "73\n");
        t.put("sys/class/power_supply/BAT1/status", "Discharging\n");
        t.put("sys/class/power_supply/BAT0/capacity", "garbage");
        t.put("sys/class/power_supply/BAT0/status", "Full\n");
        t.put("sys/class/power_supply/AC/capacity", "5");
        let s = sampler(&t);
        assert_eq!(s.battery_pct(), 73.0, "BAT0 is unreadable, BAT1 answers");
        assert_eq!(s.battery(), Some(Battery { percent: 73, status: "Discharging".into() }));
        t.put("sys/class/power_supply/BAT0/capacity", "41\n");
        assert_eq!(s.battery_pct(), 41.0);
        assert_eq!(s.battery().unwrap().status, "Full");
    }

    #[test]
    fn a_battery_without_a_status_file_still_gives_its_percentage_but_not_the_pair() {
        let t = TempDir::new();
        t.put("sys/class/power_supply/BAT0/capacity", "88\n");
        let s = sampler(&t);
        assert_eq!(s.battery_pct(), 88.0);
        assert_eq!(s.battery(), None);
        t.put("sys/class/power_supply/BAT0/capacity", "88.5");
        assert_eq!(s.battery_pct(), 88.5);
        assert_eq!(s.battery(), None, "the TUI's integer percentage must be an integer");
        t.put("sys/class/power_supply/BAT0/capacity", "NaN");
        assert_eq!(s.battery_pct(), 100.0, "a non-finite capacity is no reading");
    }

    #[test]
    fn gpu_lines_are_parsed_strictly() {
        assert_eq!(parse_gpu_line("61, 40, 55.5"), Some((61.0, 40.0, 55.5)));
        assert_eq!(parse_gpu_line("  61 ,40,  55.5\n"), Some((61.0, 40.0, 55.5)));
        for bad in [
            "",
            "1,2",
            "a, b, c",
            "1,2,3,4",
            "61, 10, x",
            "61, 40, [N/A]",
            "61, 40,",
            "nan, 1, 1",
            "1, inf, 1",
            "1, 1, -inf",
            ",,",
        ] {
            assert_eq!(parse_gpu_line(bad), None, "{bad:?}");
        }
        assert_eq!(parse_gpu_line(&"9,".repeat(50)), None);
    }

    #[test]
    fn gpu_readings_are_empty_then_smoothed_from_zero() {
        let t = TempDir::new();
        let s = sampler(&t);
        assert_eq!(s.gpu(), GpuReading { temp: None, util: 0.0, power: 0.0, seen: None });
        assert!(!s.feed_gpu_line("61, 40"));
        assert_eq!(s.gpu().seen, None, "a rejected line changes nothing");
        assert!(s.feed_gpu_line("61, 40, 50"));
        assert_eq!(s.gpu(), GpuReading { temp: Some(61.0), util: 12.0, power: 15.0, seen: Some(1000.0) });
        assert!(s.feed_gpu_line("65, 80, 50"));
        let g = s.gpu();
        assert_eq!(g.temp, Some(65.0), "the temperature is raw");
        assert!((g.util - (12.0 + 0.3 * (80.0 - 12.0))).abs() < 1e-9);
    }

    /// A stream that plays scripted sessions: each call to `stream` consumes the next one.
    struct Script {
        sessions: Mutex<Vec<io::Result<Vec<&'static str>>>>,
        starts: Mutex<Vec<(String, Vec<String>)>>,
    }

    impl LineSource for Script {
        fn stream(
            &self,
            program: &str,
            args: &[&str],
            on_line: &mut dyn FnMut(&str) -> bool,
        ) -> io::Result<()> {
            self.starts
                .lock()
                .unwrap()
                .push((program.to_owned(), args.iter().map(|a| (*a).to_owned()).collect()));
            let next = {
                let mut s = self.sessions.lock().unwrap();
                if s.is_empty() { Ok(vec![]) } else { s.remove(0) }
            };
            for line in next? {
                if !on_line(line) {
                    break;
                }
            }
            Ok(())
        }
    }

    #[test]
    fn the_gpu_stream_is_restarted_after_a_pause_until_stopped() {
        let t = TempDir::new();
        let s = sampler(&t);
        let script = Script {
            sessions: Mutex::new(vec![
                Err(io::Error::from(io::ErrorKind::NotFound)),
                Ok(vec!["60, 10, 20", "junk", "62, 30, 25"]),
                Ok(vec!["70, 50, 80"]),
            ]),
            starts: Mutex::new(Vec::new()),
        };
        let pauses = Arc::new(Mutex::new(Vec::new()));
        let p = pauses.clone();
        let sleeper: Sleeper = Arc::new(move |d| p.lock().unwrap().push(d));
        let calls = AtomicU32::new(0);
        // Stop once the third session has started.
        let stop = || script.starts.lock().unwrap().len() >= 3 && calls.fetch_add(1, Ordering::Relaxed) >= 1;
        s.run_gpu_stream(&script, &stop, &sleeper);
        let starts = script.starts.lock().unwrap();
        assert_eq!(starts.len(), 3);
        assert_eq!(starts[0].0, "nvidia-smi");
        assert_eq!(
            starts[0].1,
            [
                "--query-gpu=temperature.gpu,utilization.gpu,power.draw",
                "--format=csv,noheader,nounits",
                "-l",
                "2"
            ]
        );
        assert_eq!(
            pauses.lock().unwrap().len(),
            2,
            "a pause after the failed start and after the first session"
        );
        assert!(pauses.lock().unwrap().iter().all(|d| *d == GPU_RESTART_DELAY));
        assert_eq!(s.gpu().temp, Some(70.0));
    }

    /// Wraps a source so that a flag turns on while its first line is being handled.
    struct FlipOnFirstLine<'a>(&'a Script, &'a AtomicBool);
    impl LineSource for FlipOnFirstLine<'_> {
        fn stream(&self, p: &str, a: &[&str], on_line: &mut dyn FnMut(&str) -> bool) -> io::Result<()> {
            self.0.stream(p, a, &mut |l| {
                self.1.store(true, Ordering::Relaxed);
                on_line(l)
            })
        }
    }

    #[test]
    fn a_stop_request_ends_the_stream_between_lines_and_never_sleeps() {
        let t = TempDir::new();
        let s = sampler(&t);
        let script = Script {
            sessions: Mutex::new(vec![Ok(vec!["60, 10, 20", "61, 10, 20", "62, 10, 20"])]),
            starts: Mutex::new(Vec::new()),
        };
        let sleeps = Arc::new(AtomicU32::new(0));
        let n = sleeps.clone();
        let sleeper: Sleeper = Arc::new(move |_| {
            n.fetch_add(1, Ordering::Relaxed);
        });
        let stopped = AtomicBool::new(false);
        s.run_gpu_stream(&FlipOnFirstLine(&script, &stopped), &|| stopped.load(Ordering::Relaxed), &sleeper);
        assert_eq!(s.gpu().temp, Some(60.0), "the first line was fed, then the stream was told to end");
        assert_eq!(sleeps.load(Ordering::Relaxed), 0, "no restart pause once a stop was requested");
    }

    #[test]
    fn a_stream_that_is_already_stopped_never_starts() {
        let t = TempDir::new();
        let s = sampler(&t);
        let script = Script { sessions: Mutex::new(vec![]), starts: Mutex::new(Vec::new()) };
        let sleeper: Sleeper = Arc::new(|_| {});
        s.run_gpu_stream(&script, &|| true, &sleeper);
        assert!(script.starts.lock().unwrap().is_empty());
    }

    #[test]
    fn the_real_line_source_reports_a_missing_program_as_an_error() {
        // Only the failure path runs a (non-existent) program: the tests start no real process.
        let err = ProcessLineSource.stream("/nonexistent/nvidia-smi", &[], &mut |_| true).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn the_system_sampler_reads_the_wall_clock() {
        assert!(wall_clock() > 1.7e9);
    }

    #[test]
    fn hostile_stat_text_never_panics() {
        for text in [
            "cpu",
            "cpu ",
            "cpu 18446744073709551615 1 0 0 0 0 0 0",
            "\n\n",
            "cpu  1 1 1 1 1 1 1 1 1 1 1 1 1",
            "cpu\t1\t1\t1\t1\t1\t1\t1\t1",
        ] {
            let _ = parse_cpu_counters(text);
        }
        assert!(parse_cpu_counters("cpu\t1\t1\t1\t1\t1\t1\t1\t1").is_some());
        assert!(
            parse_cpu_counters("cpu 18446744073709551615 1 0 0 0 0 0 0").is_none(),
            "an overflowing total is no reading"
        );
    }
}
