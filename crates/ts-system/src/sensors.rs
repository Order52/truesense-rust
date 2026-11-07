//! sensors: the machine's own readings that the daemon decides from and the TUI, the window and
//! the status bars show: CPU load (from `/proc/stat`), battery charge and state (from
//! `/sys/class/power_supply`), and the GPU's temperature, load and power draw (streamed from
//! `nvidia-smi`, and only when something smart needs it).
//!
//! # The GPU stream must not keep the card awake
//!
//! On the PH16-71 the discrete GPU is runtime-suspended whenever nothing uses it, and every
//! question to the card resumes it (see [`crate::nvidia`]). A permanent `nvidia-smi -l 2` asks
//! every two seconds, so the card never reaches its 10 to 15 second suspend delay: the battery
//! life of a laptop with a smart feature on would be that of one with the card always on, and
//! every start of the program initialises NVML, which raises system management interrupts. So
//! the stream follows a small policy ([`GpuGate`]): never start for a card that is asleep or
//! absent, end when nothing wants it, and *park* (end, so that the card can suspend) after a
//! minute of idle readings, then start again only when someone else wakes the card.
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

use crate::nvidia::{self, GpuPower};

/// Weight of a new CPU load sample (the daemon smooths the jittery `/proc/stat` deltas without
/// a delay long enough to hide a real rise).
pub const CPU_LOAD_ALPHA: f64 = 0.25;

/// Weight of a new GPU load or power sample.
pub const GPU_ALPHA: f64 = 0.3;

/// Pause before the `nvidia-smi` stream is restarted after it ended or failed to start. Repeated
/// failures double it up to [`GPU_RETRY_MAX`].
pub const GPU_RESTART_DELAY: Duration = Duration::from_secs(5);

/// The longest pause between two start attempts of a stream that keeps failing (no `nvidia-smi`
/// installed, a driver that is not loaded): a dead path settles at one attempt per five minutes.
pub const GPU_RETRY_MAX: Duration = Duration::from_secs(300);

/// How often the stream thread looks at the card's power state while it has nothing to read.
/// Reading `power/runtime_status` does not wake the card.
pub const GPU_POLL: Duration = Duration::from_secs(2);

/// A GPU reading older than this is not current: the stream emits a line every two seconds, so
/// a gap of this length means it ended or is stuck, and the temperature must not be trusted.
pub const GPU_FRESH_S: f64 = 6.0;

/// Idle readings (load and power below the two limits that follow) for this many seconds end the
/// stream so that the card can suspend.
pub const GPU_IDLE_PARK_S: f64 = 60.0;

/// The highest load, in percent, that still counts as idle for parking.
pub const GPU_IDLE_UTIL_PCT: f64 = 5.0;

/// The highest power draw, in watts, that still counts as idle for parking (an awake RTX 4060
/// at P8 draws about 5 to 15 W; a game draws far more).
pub const GPU_IDLE_POWER_W: f64 = 25.0;

/// After a park the card should be seen asleep within this time; if it never is, something else
/// holds it awake (a compositor on the card, a CUDA process, a card without runtime power
/// management), so reading it costs nothing more and the stream starts again.
pub const GPU_PARK_FALLBACK_S: f64 = 180.0;

/// After that fallback restart the stream does not park again for this long, so that a card held
/// awake by someone else is not started and stopped (each start initialises NVML) in a loop.
pub const GPU_NO_PARK_S: f64 = 600.0;

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

/// Where the GPU numbers of the moment come from. The load and power of a card that is not
/// being read are 0, and that 0 means different things: this says which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuFeed {
    /// There is no NVIDIA display card (an AMD or Intel-only machine, or the card is off the bus).
    Absent,
    /// The card is runtime-suspended. Nothing was asked of it, and the 0 load and 0 W are true:
    /// an asleep card does no work.
    Suspended,
    /// The stream delivers current readings.
    Live,
    /// The card is awake but nothing is reading it (no smart feature needs it, the stream is
    /// parked or starting). The 0 load and 0 W are "unknown", not "idle".
    Unsampled,
}

impl GpuFeed {
    /// The word of the live state (`gpu_state`).
    pub fn name(self) -> &'static str {
        match self {
            GpuFeed::Absent => "absent",
            GpuFeed::Suspended => "suspended",
            GpuFeed::Live => "live",
            GpuFeed::Unsampled => "unsampled",
        }
    }
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

impl GpuState {
    /// No reading. The averages start unseeded, so the first line of a session counts in full:
    /// a card that has just been woken by a game must not read 8 seconds of "idle" while an
    /// average climbs from 0.
    fn empty() -> Self {
        Self { temp: None, util: Ema::new(GPU_ALPHA), power: Ema::new(GPU_ALPHA), seen: None }
    }
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
            gpu: Mutex::new(GpuState::empty()),
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
        let Some(sample) = parse_gpu_line(line) else { return false };
        self.feed_sample(sample);
        true
    }

    fn feed_sample(&self, (temp, util, power): (f64, f64, f64)) {
        let mut g = self.gpu.lock().unwrap_or_else(PoisonError::into_inner);
        g.temp = Some(temp);
        g.util.update(util);
        g.power.update(power);
        g.seen = Some((self.clock)());
    }

    /// Forgets the GPU readings: the stream ended or the card went to sleep, so the numbers are
    /// not current any more and a later session must start its averages afresh.
    pub fn reset_gpu(&self) {
        *self.gpu.lock().unwrap_or_else(PoisonError::into_inner) = GpuState::empty();
    }

    /// The runtime power state of the NVIDIA card, read without waking it.
    pub fn gpu_power(&self) -> GpuPower {
        nvidia::gpu_power(&self.root)
    }

    /// Where the GPU numbers come from now (see [`GpuFeed`]). One scan of the PCI devices
    /// below `/sys`: files that do not wake the card.
    pub fn gpu_feed(&self) -> GpuFeed {
        match self.gpu_power() {
            GpuPower::Absent => GpuFeed::Absent,
            GpuPower::Suspended => GpuFeed::Suspended,
            GpuPower::Active | GpuPower::Unknown => {
                let now = (self.clock)();
                let seen = self.gpu.lock().unwrap_or_else(PoisonError::into_inner).seen;
                if seen.is_some_and(|seen| now - seen <= GPU_FRESH_S) {
                    GpuFeed::Live
                } else {
                    GpuFeed::Unsampled
                }
            }
        }
    }

    /// Keeps the `nvidia-smi` stream running and feeds its lines in, until `env.stop` says so or
    /// nothing wants the GPU any more. Blocks: run it on its own thread.
    ///
    /// The stream is only started for a card that is awake, ends when `env.wanted` turns false,
    /// parks itself after a minute of idle readings so the card can suspend, and starts again
    /// when the card is woken by someone else (see [`GpuGate`]). A stream that ends or cannot
    /// start is restarted after [`GPU_RESTART_DELAY`], doubling up to [`GPU_RETRY_MAX`] while it
    /// keeps failing, so a machine without `nvidia-smi` hardly ever tries.
    pub fn run_gpu_stream(&self, source: &dyn LineSource, env: &GpuStreamEnv<'_>) {
        const ARGS: [&str; 4] = [
            "--query-gpu=temperature.gpu,utilization.gpu,power.draw",
            "--format=csv,noheader,nounits",
            "-l",
            "2",
        ];
        let mut gate = GpuGate::default();
        let going = || !(env.stop)() && (env.wanted)();
        while going() {
            let power = self.gpu_power();
            if power == GpuPower::Suspended {
                self.reset_gpu(); // asleep: whatever was read before is history
            }
            if gate.next((self.clock)(), power) == GateStep::Wait {
                (env.sleeper)(GPU_POLL);
                continue;
            }
            self.reset_gpu();
            let mut got_line = false;
            let mut end = SessionEnd::Stream;
            // A failure to start is the same as the stream ending without a line.
            let _ = source.stream("nvidia-smi", &ARGS, &mut |line| {
                if let Some(sample) = parse_gpu_line(line) {
                    got_line = true;
                    self.feed_sample(sample);
                    match gate.observe((self.clock)(), sample, self.gpu_power()) {
                        Verdict::Continue => {}
                        Verdict::Park => {
                            end = SessionEnd::Park;
                            return false;
                        }
                        Verdict::Asleep => {
                            end = SessionEnd::Asleep;
                            return false;
                        }
                    }
                }
                going()
            });
            self.reset_gpu();
            if !going() {
                break;
            }
            gate.ended((self.clock)(), end, got_line);
        }
    }
}

/// What the stream thread needs to know from the daemon.
pub struct GpuStreamEnv<'a> {
    /// The daemon is shutting down.
    pub stop: &'a dyn Fn() -> bool,
    /// A smart feature needs the GPU readings. When it turns false the stream ends within one
    /// line (two seconds) and the card is left alone.
    pub wanted: &'a dyn Fn() -> bool,
    /// Waits between two looks at the card's power state.
    pub sleeper: &'a Sleeper,
}

/// What the stream thread does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GateStep {
    /// Start `nvidia-smi`.
    Start,
    /// Do not touch the card yet.
    Wait,
}

/// What a line of a running session says about the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Continue,
    /// Idle for long enough: end the stream so that the card can suspend.
    Park,
    /// The card reads as asleep: end the session.
    Asleep,
}

/// How a session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionEnd {
    /// `nvidia-smi` ended or failed by itself.
    Stream,
    /// The gate parked it.
    Park,
    /// The gate saw the card asleep.
    Asleep,
}

/// Where the stream waits for the card to sleep after a park.
#[derive(Debug, Clone, Copy)]
struct Parked {
    since: f64,
    /// The card has been seen suspended since the park: it is released.
    slept: bool,
}

/// The decision whether the GPU stream may run now: pure, driven by the card's power state and
/// the readings, so every transition is tested with a fake clock.
///
/// ```text
///   Absent ---------------------------------------> never start
///   Suspended ------------------------------------> never start (and note: released)
///   Active/Unknown, nothing parked ---------------> start
///   running, idle >= 60 s, card Active -----------> park (end the session)
///   parked, card seen asleep, then Active --------> start (someone woke it)
///   parked, never seen asleep for 180 s ----------> start, and do not park for 10 min
/// ```
///
/// A card whose runtime status is unreadable ([`GpuPower::Unknown`]) has no runtime power
/// management, so it is never asleep and never parked: the stream just runs.
#[derive(Debug, Default)]
struct GpuGate {
    parked: Option<Parked>,
    idle_since: Option<f64>,
    hold_until: f64,
    no_park_until: f64,
    failures: u32,
}

impl GpuGate {
    /// Whether to start a session at `now` with the card in state `power`.
    fn next(&mut self, now: f64, power: GpuPower) -> GateStep {
        if now < self.hold_until {
            return GateStep::Wait;
        }
        match power {
            GpuPower::Absent => GateStep::Wait,
            GpuPower::Suspended => {
                if let Some(parked) = &mut self.parked {
                    parked.slept = true;
                }
                GateStep::Wait
            }
            GpuPower::Active | GpuPower::Unknown => match self.parked {
                None => self.start(),
                Some(p) if p.slept => self.start(),
                Some(p) if now - p.since >= GPU_PARK_FALLBACK_S => {
                    self.no_park_until = now + GPU_NO_PARK_S;
                    self.start()
                }
                Some(_) => GateStep::Wait,
            },
        }
    }

    fn start(&mut self) -> GateStep {
        self.parked = None;
        self.idle_since = None;
        GateStep::Start
    }

    /// Looks at one reading of a running session.
    fn observe(&mut self, now: f64, (_, util, power_w): (f64, f64, f64), power: GpuPower) -> Verdict {
        if power == GpuPower::Suspended {
            return Verdict::Asleep;
        }
        let idle = util <= GPU_IDLE_UTIL_PCT && power_w <= GPU_IDLE_POWER_W;
        if !idle {
            self.idle_since = None;
            return Verdict::Continue;
        }
        let since = *self.idle_since.get_or_insert(now);
        if power == GpuPower::Active && now >= self.no_park_until && now - since >= GPU_IDLE_PARK_S {
            Verdict::Park
        } else {
            Verdict::Continue
        }
    }

    /// Records how a session ended.
    fn ended(&mut self, now: f64, end: SessionEnd, got_line: bool) {
        self.idle_since = None;
        match end {
            SessionEnd::Park => {
                self.failures = 0;
                self.parked = Some(Parked { since: now, slept: false });
            }
            SessionEnd::Asleep => self.failures = 0,
            SessionEnd::Stream if got_line => {
                self.failures = 0;
                self.hold_until = now + GPU_RESTART_DELAY.as_secs_f64();
            }
            SessionEnd::Stream => {
                self.failures = self.failures.saturating_add(1);
                let doubled =
                    GPU_RESTART_DELAY.as_secs_f64() * f64::from(1u32 << (self.failures - 1).min(10));
                self.hold_until = now + doubled.min(GPU_RETRY_MAX.as_secs_f64());
            }
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
    use std::sync::atomic::{AtomicU32, Ordering};

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
    fn gpu_readings_are_empty_and_the_first_line_of_a_session_counts_in_full() {
        let t = TempDir::new();
        let s = sampler(&t);
        assert_eq!(s.gpu(), GpuReading { temp: None, util: 0.0, power: 0.0, seen: None });
        assert!(!s.feed_gpu_line("61, 40"));
        assert_eq!(s.gpu().seen, None, "a rejected line changes nothing");
        assert!(s.feed_gpu_line("61, 40, 50"));
        assert_eq!(
            s.gpu(),
            GpuReading { temp: Some(61.0), util: 40.0, power: 50.0, seen: Some(1000.0) },
            "a card just woken by a game must not read idle while an average climbs from 0"
        );
        assert!(s.feed_gpu_line("65, 80, 50"));
        let g = s.gpu();
        assert_eq!(g.temp, Some(65.0), "the temperature is raw");
        assert!((g.util - (40.0 + 0.3 * (80.0 - 40.0))).abs() < 1e-9);
        s.reset_gpu();
        assert_eq!(s.gpu(), GpuReading { temp: None, util: 0.0, power: 0.0, seen: None });
        assert!(s.feed_gpu_line("50, 10, 20"));
        assert_eq!(s.gpu().util, 10.0, "after a reset the averages start afresh");
    }

    // ---- the policy: when the card may be read --------------------------------------------------

    fn idle_sample() -> (f64, f64, f64) {
        (45.0, 1.0, 12.0)
    }

    #[test]
    fn the_gate_never_starts_for_an_absent_or_sleeping_card() {
        let mut gate = GpuGate::default();
        assert_eq!(gate.next(0.0, GpuPower::Absent), GateStep::Wait);
        assert_eq!(gate.next(0.0, GpuPower::Suspended), GateStep::Wait);
        assert_eq!(gate.next(1.0, GpuPower::Active), GateStep::Start);
        let mut gate = GpuGate::default();
        assert_eq!(
            gate.next(0.0, GpuPower::Unknown),
            GateStep::Start,
            "no runtime power management: just read it"
        );
    }

    #[test]
    fn the_gate_parks_after_a_minute_of_idle_and_a_busy_reading_restarts_the_minute() {
        let mut gate = GpuGate::default();
        assert_eq!(gate.next(0.0, GpuPower::Active), GateStep::Start);
        let mut t = 0.0;
        while t < GPU_IDLE_PARK_S - 2.0 {
            t += 2.0;
            assert_eq!(gate.observe(t, idle_sample(), GpuPower::Active), Verdict::Continue, "t={t}");
        }
        // A busy reading (a game's load, or a draw above the idle limit) restarts the count.
        assert_eq!(gate.observe(t + 2.0, (70.0, 60.0, 40.0), GpuPower::Active), Verdict::Continue);
        assert_eq!(
            gate.observe(t + 4.0, (45.0, 1.0, 40.0), GpuPower::Active),
            Verdict::Continue,
            "40 W is not idle"
        );
        let from = t + 6.0;
        assert_eq!(gate.observe(from, idle_sample(), GpuPower::Active), Verdict::Continue);
        assert_eq!(
            gate.observe(from + GPU_IDLE_PARK_S - 0.1, idle_sample(), GpuPower::Active),
            Verdict::Continue
        );
        assert_eq!(gate.observe(from + GPU_IDLE_PARK_S, idle_sample(), GpuPower::Active), Verdict::Park);
    }

    #[test]
    fn a_card_without_runtime_power_management_is_never_parked() {
        let mut gate = GpuGate::default();
        gate.next(0.0, GpuPower::Unknown);
        for i in 0..200 {
            let v = gate.observe(f64::from(i) * 2.0, idle_sample(), GpuPower::Unknown);
            assert_eq!(v, Verdict::Continue, "it would be started again at once: pointless");
        }
    }

    #[test]
    fn a_session_whose_card_reads_asleep_ends() {
        let mut gate = GpuGate::default();
        gate.next(0.0, GpuPower::Active);
        assert_eq!(gate.observe(2.0, (60.0, 90.0, 80.0), GpuPower::Suspended), Verdict::Asleep);
    }

    #[test]
    fn after_a_park_the_stream_waits_for_the_card_to_sleep_and_for_someone_to_wake_it() {
        let mut gate = GpuGate::default();
        gate.next(0.0, GpuPower::Active);
        gate.ended(100.0, SessionEnd::Park, true);
        // The card is still awake for its suspend delay: that is our own last query, not a user.
        assert_eq!(gate.next(102.0, GpuPower::Active), GateStep::Wait);
        assert_eq!(gate.next(110.0, GpuPower::Active), GateStep::Wait);
        assert_eq!(gate.next(112.0, GpuPower::Suspended), GateStep::Wait, "released");
        assert_eq!(gate.next(500.0, GpuPower::Suspended), GateStep::Wait, "asleep as long as nobody uses it");
        assert_eq!(gate.next(501.0, GpuPower::Active), GateStep::Start, "somebody woke it: read it now");
        // The park is over: a new session parks again after its own idle minute.
        assert_eq!(gate.next(503.0, GpuPower::Active), GateStep::Start);
    }

    #[test]
    fn a_card_that_never_sleeps_after_a_park_is_read_again_and_not_parked_for_ten_minutes() {
        let mut gate = GpuGate::default();
        gate.next(0.0, GpuPower::Active);
        gate.ended(100.0, SessionEnd::Park, true);
        assert_eq!(gate.next(100.0 + GPU_PARK_FALLBACK_S - 1.0, GpuPower::Active), GateStep::Wait);
        let restart = 100.0 + GPU_PARK_FALLBACK_S;
        assert_eq!(gate.next(restart, GpuPower::Active), GateStep::Start, "something else holds it awake");
        // Idle for far more than the park minute: held awake by another, so keep the stream.
        for i in 0..40 {
            let t = restart + f64::from(i) * 2.0;
            assert_eq!(gate.observe(t, idle_sample(), GpuPower::Active), Verdict::Continue, "t={t}");
        }
        assert_eq!(
            gate.observe(restart + GPU_NO_PARK_S - 2.0, idle_sample(), GpuPower::Active),
            Verdict::Continue,
            "still inside the pause"
        );
        assert_eq!(
            gate.observe(restart + GPU_NO_PARK_S, idle_sample(), GpuPower::Active),
            Verdict::Park,
            "after the pause a long idle stretch parks it (the other holder has probably gone)"
        );
    }

    #[test]
    fn a_stream_that_keeps_failing_backs_off_to_five_minutes() {
        let mut gate = GpuGate::default();
        let mut now = 0.0;
        let mut waits = Vec::new();
        for _ in 0..9 {
            assert_eq!(gate.next(now, GpuPower::Active), GateStep::Start);
            gate.ended(now, SessionEnd::Stream, false);
            waits.push(gate.hold_until - now);
            assert_eq!(gate.next(now + 0.5, GpuPower::Active), GateStep::Wait, "held back");
            now = gate.hold_until;
        }
        assert_eq!(waits, [5.0, 10.0, 20.0, 40.0, 80.0, 160.0, 300.0, 300.0, 300.0]);
        // One session that delivered a line puts the pause back to the short one.
        assert_eq!(gate.next(now, GpuPower::Active), GateStep::Start);
        gate.ended(now, SessionEnd::Stream, true);
        assert_eq!(gate.hold_until - now, 5.0);
    }

    // ---- the thread that runs the policy -------------------------------------------------------

    /// A temporary machine: a PCI tree with one NVIDIA card, and a clock that moves when the
    /// stream sleeps or reads a line.
    struct World {
        tree: TempDir,
        now: Arc<Mutex<f64>>,
    }

    impl World {
        fn new(status: Option<&str>) -> Self {
            let w = Self { tree: TempDir::new(), now: Arc::new(Mutex::new(1000.0)) };
            w.tree.put("sys/bus/pci/devices/0000:01:00.0/vendor", "0x10de\n");
            w.tree.put("sys/bus/pci/devices/0000:01:00.0/class", "0x030200\n");
            if let Some(s) = status {
                w.status(s);
            }
            w
        }

        fn status(&self, s: &str) {
            self.tree.put("sys/bus/pci/devices/0000:01:00.0/power/runtime_status", &format!("{s}\n"));
        }

        fn sampler(&self) -> Sampler {
            let now = self.now.clone();
            Sampler::new(&self.tree.0, Arc::new(move || *now.lock().unwrap()))
        }

        fn advance(&self, s: f64) {
            *self.now.lock().unwrap() += s;
        }

        fn time(&self) -> f64 {
            *self.now.lock().unwrap()
        }
    }

    /// Plays scripted sessions; every line takes two seconds of fake time and a start is
    /// recorded with the time it happened.
    struct Script<'a> {
        world: &'a World,
        sessions: Mutex<Vec<io::Result<Vec<&'static str>>>>,
        starts: Mutex<Vec<f64>>,
    }

    impl<'a> Script<'a> {
        fn new(world: &'a World, sessions: Vec<io::Result<Vec<&'static str>>>) -> Self {
            Self { world, sessions: Mutex::new(sessions), starts: Mutex::new(Vec::new()) }
        }
    }

    impl LineSource for Script<'_> {
        fn stream(
            &self,
            program: &str,
            args: &[&str],
            on_line: &mut dyn FnMut(&str) -> bool,
        ) -> io::Result<()> {
            assert_eq!(program, "nvidia-smi");
            assert_eq!(
                args,
                [
                    "--query-gpu=temperature.gpu,utilization.gpu,power.draw",
                    "--format=csv,noheader,nounits",
                    "-l",
                    "2"
                ]
            );
            self.starts.lock().unwrap().push(self.world.time());
            let next = {
                let mut s = self.sessions.lock().unwrap();
                if s.is_empty() { Ok(vec![]) } else { s.remove(0) }
            };
            for line in next? {
                self.world.advance(2.0);
                if !on_line(line) {
                    break;
                }
            }
            Ok(())
        }
    }

    /// A sleeper that moves the world's clock and counts the calls; `each` runs after the clock
    /// moved, with the number of the call.
    fn sleeper(world: &World, each: impl Fn(usize) + Send + Sync + 'static) -> (Sleeper, Arc<AtomicU32>) {
        let now = world.now.clone();
        let calls = Arc::new(AtomicU32::new(0));
        let n = calls.clone();
        let sleeper: Sleeper = Arc::new(move |d: Duration| {
            *now.lock().unwrap() += d.as_secs_f64();
            let k = n.fetch_add(1, Ordering::Relaxed) as usize + 1;
            each(k);
        });
        (sleeper, calls)
    }

    #[test]
    fn a_sleeping_card_is_never_asked_and_is_polled_without_waking() {
        let w = World::new(Some("suspended"));
        let s = w.sampler();
        let script = Script::new(&w, vec![Ok(vec!["50, 1, 10"])]);
        let (sleep, calls) = sleeper(&w, |_| {});
        let stop = || calls.load(Ordering::Relaxed) >= 30;
        s.run_gpu_stream(&script, &GpuStreamEnv { stop: &stop, wanted: &|| true, sleeper: &sleep });
        assert!(script.starts.lock().unwrap().is_empty(), "no nvidia-smi for a card that is asleep");
        assert_eq!(s.gpu_feed(), GpuFeed::Suspended);
        assert_eq!(s.gpu().temp, None);
    }

    #[test]
    fn no_card_no_stream() {
        let w = World::new(None);
        fs::remove_dir_all(w.tree.0.join("sys/bus/pci/devices")).unwrap();
        let s = w.sampler();
        let script = Script::new(&w, vec![Ok(vec!["50, 1, 10"])]);
        let (sleep, calls) = sleeper(&w, |_| {});
        let stop = || calls.load(Ordering::Relaxed) >= 10;
        s.run_gpu_stream(&script, &GpuStreamEnv { stop: &stop, wanted: &|| true, sleeper: &sleep });
        assert!(script.starts.lock().unwrap().is_empty());
        assert_eq!(s.gpu_feed(), GpuFeed::Absent);
    }

    #[test]
    fn a_stream_waits_for_the_card_to_be_woken_and_then_starts() {
        let w = World::new(Some("suspended"));
        let s = w.sampler();
        let script = Script::new(&w, vec![Ok(vec!["60, 80, 90", "61, 85, 95"])]);
        let woken = Arc::new(Mutex::new(None::<f64>));
        let (wk, world_now) = (woken.clone(), w.now.clone());
        let tree = w.tree.0.clone();
        let (sleep, calls) = sleeper(&w, move |k| {
            if k == 4 {
                // A game starts: somebody else wakes the card.
                fs::write(tree.join("sys/bus/pci/devices/0000:01:00.0/power/runtime_status"), "active\n")
                    .unwrap();
                *wk.lock().unwrap() = Some(*world_now.lock().unwrap());
            }
        });
        let stop = || calls.load(Ordering::Relaxed) >= 8;
        s.run_gpu_stream(&script, &GpuStreamEnv { stop: &stop, wanted: &|| true, sleeper: &sleep });
        let starts = script.starts.lock().unwrap();
        assert!(!starts.is_empty());
        assert!(starts[0] >= woken.lock().unwrap().unwrap(), "started only after the card was awake");
    }

    #[test]
    fn an_idle_stream_parks_so_the_card_can_sleep_and_restarts_when_it_is_woken() {
        let w = World::new(Some("active"));
        let s = w.sampler();
        // 40 idle lines (80 s): parks after 60 s. Then a busy session after the wake.
        let idle: Vec<&'static str> = vec!["45, 1, 12"; 40];
        let script = Script::new(&w, vec![Ok(idle), Ok(vec!["70, 90, 80", "71, 92, 85"])]);
        let flips = Arc::new(Mutex::new((0.0f64, 0.0f64)));
        let (f, world_now) = (flips.clone(), w.now.clone());
        let tree = w.tree.0.clone();
        let status = tree.join("sys/bus/pci/devices/0000:01:00.0/power/runtime_status");
        let (sleep, calls) = sleeper(&w, move |k| {
            let now = *world_now.lock().unwrap();
            if k == 5 {
                fs::write(&status, "suspended\n").unwrap(); // our last query is over: it sleeps
                f.lock().unwrap().0 = now;
            }
            if k == 25 {
                fs::write(&status, "active\n").unwrap(); // a game wakes it
                f.lock().unwrap().1 = now;
            }
        });
        let stop = || calls.load(Ordering::Relaxed) >= 40;
        s.run_gpu_stream(&script, &GpuStreamEnv { stop: &stop, wanted: &|| true, sleeper: &sleep });
        let starts = script.starts.lock().unwrap().clone();
        let (slept_at, woke_at) = *flips.lock().unwrap();
        assert!(starts.len() >= 2, "{starts:?}");
        assert!(starts[0] < slept_at, "the first session ran while the card was awake");
        assert!(starts[1] >= woke_at, "restarted only when the card was woken: {starts:?} woke {woke_at}");
        assert!(
            starts[1] - starts[0] > GPU_IDLE_PARK_S,
            "and not before the minute of idle was over: {starts:?}"
        );
    }

    #[test]
    fn the_stream_ends_when_nothing_wants_the_gpu_and_the_readings_go_with_it() {
        let w = World::new(Some("active"));
        let s = w.sampler();
        let script = Script::new(&w, vec![Ok(vec!["60, 10, 20", "61, 10, 20", "62, 10, 20", "63, 10, 20"])]);
        // Wanted until the second line has been read.
        let wanted = || w.time() < 1004.0;
        let (sleep, calls) = sleeper(&w, |_| {});
        s.run_gpu_stream(&script, &GpuStreamEnv { stop: &|| false, wanted: &wanted, sleeper: &sleep });
        assert_eq!(script.starts.lock().unwrap().len(), 1);
        assert_eq!(calls.load(Ordering::Relaxed), 0, "no pause and no restart once nothing wants it");
        assert_eq!(s.gpu().temp, None, "the stream ended: its numbers are history");
    }

    #[test]
    fn a_stop_request_ends_the_stream_between_lines_and_never_sleeps() {
        let w = World::new(Some("active"));
        let s = w.sampler();
        let script = Script::new(&w, vec![Ok(vec!["60, 10, 20", "61, 10, 20", "62, 10, 20"])]);
        let (sleep, calls) = sleeper(&w, |_| {});
        let stop = || w.time() >= 1002.0; // after the first line
        s.run_gpu_stream(&script, &GpuStreamEnv { stop: &stop, wanted: &|| true, sleeper: &sleep });
        assert_eq!(script.starts.lock().unwrap().len(), 1);
        assert_eq!(calls.load(Ordering::Relaxed), 0, "no restart pause once a stop was requested");
    }

    #[test]
    fn a_stream_that_is_already_stopped_or_unwanted_never_starts() {
        let w = World::new(Some("active"));
        let s = w.sampler();
        let script = Script::new(&w, vec![]);
        let (sleep, _) = sleeper(&w, |_| {});
        s.run_gpu_stream(&script, &GpuStreamEnv { stop: &|| true, wanted: &|| true, sleeper: &sleep });
        s.run_gpu_stream(&script, &GpuStreamEnv { stop: &|| false, wanted: &|| false, sleeper: &sleep });
        assert!(script.starts.lock().unwrap().is_empty());
    }

    #[test]
    fn a_missing_nvidia_smi_is_retried_ever_more_rarely() {
        let w = World::new(Some("active"));
        let s = w.sampler();
        let failures: Vec<io::Result<Vec<&'static str>>> =
            (0..20).map(|_| Err(io::Error::from(io::ErrorKind::NotFound))).collect();
        let script = Script::new(&w, failures);
        let (sleep, _) = sleeper(&w, |_| {});
        let stop = || w.time() >= 1000.0 + 3600.0;
        s.run_gpu_stream(&script, &GpuStreamEnv { stop: &stop, wanted: &|| true, sleeper: &sleep });
        let starts = script.starts.lock().unwrap();
        // 5 + 10 + 20 + 40 + 80 + 160, then one try per five minutes: about 17 in an hour, not 720.
        assert!((10..=20).contains(&starts.len()), "{} tries in an hour", starts.len());
    }

    #[test]
    fn the_feed_says_what_a_zero_load_means() {
        let w = World::new(Some("active"));
        let s = w.sampler();
        assert_eq!(s.gpu_feed(), GpuFeed::Unsampled, "awake, nothing reads it: zero means unknown");
        s.feed_gpu_line("50, 10, 20");
        assert_eq!(s.gpu_feed(), GpuFeed::Live);
        w.advance(GPU_FRESH_S + 1.0);
        assert_eq!(s.gpu_feed(), GpuFeed::Unsampled, "a reading that stopped coming is not live");
        w.status("suspended");
        assert_eq!(s.gpu_feed(), GpuFeed::Suspended, "asleep: zero means zero");
        w.status("unsupported");
        assert_eq!(s.gpu_feed(), GpuFeed::Unsampled, "no runtime power management: awake");
        for (feed, name) in [
            (GpuFeed::Absent, "absent"),
            (GpuFeed::Suspended, "suspended"),
            (GpuFeed::Live, "live"),
            (GpuFeed::Unsampled, "unsampled"),
        ] {
            assert_eq!(feed.name(), name);
        }
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
