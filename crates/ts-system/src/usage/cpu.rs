//! CPU load per core and in total, frequencies, load average and package power.
//!
//! Load comes from the cumulative tick counters of `/proc/stat`: a percentage needs two samples,
//! so [`CpuSampler`] keeps the previous one. Every awkward case has a defined answer:
//!
//! * two samples in the same jiffy (zero delta), or counters that moved backwards: the previous
//!   percentage is held, nothing is divided by zero and nothing goes negative;
//! * a core that goes offline (hot-unplug, `echo 0 > online`) disappears from `/proc/stat`: it is
//!   dropped from the answer and its history forgotten, so it starts clean when it returns;
//! * a garbled line is skipped, the other lines still count.
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::files::{below, dir_names, indexed, read_line, read_text, read_u64};

/// The most cores looked at. A hostile `/proc/stat` cannot make the sampler allocate without bound.
pub const MAX_CORES: usize = 1024;

/// A CPU frequency above this is a broken file, not a processor (kHz: 100 GHz).
const MAX_PLAUSIBLE_KHZ: u64 = 100_000_000;

/// Cumulative scheduler ticks of one CPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuTicks {
    /// `user + nice + system + idle + iowait + irq + softirq + steal`. `guest` and `guest_nice`
    /// are already counted inside `user` and `nice`, so they are not added again.
    pub total: u64,
    /// `idle + iowait`: time the CPU had nothing to run.
    pub idle: u64,
}

/// The counters of one reading of `/proc/stat`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatSnapshot {
    /// The `cpu` line: all cores together.
    pub total: Option<CpuTicks>,
    /// The `cpuN` lines as (N, ticks), in file order, only for cores that are online.
    pub cores: Vec<(usize, CpuTicks)>,
}

/// Reads the numbers after the label: at least `idle` (field four) must be there; a field that
/// is not a number makes the whole line unusable.
fn parse_ticks<'a>(fields: impl Iterator<Item = &'a str>) -> Option<CpuTicks> {
    let mut values = [0u64; 8];
    let mut count = 0;
    for field in fields.take(8) {
        values[count] = field.parse().ok()?;
        count += 1;
    }
    if count < 4 {
        return None;
    }
    let total = values.iter().try_fold(0u64, |sum, v| sum.checked_add(*v))?;
    let idle = values[3].checked_add(values[4])?;
    Some(CpuTicks { total, idle })
}

/// Parses `/proc/stat`. Lines that are not `cpu` or `cpuN`, and lines that are garbled, are
/// skipped. Never fails and never reads more than [`MAX_CORES`] core lines.
pub fn parse_stat(text: &str) -> StatSnapshot {
    let mut snapshot = StatSnapshot::default();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(label) = fields.next() else { continue };
        if !label.starts_with("cpu") {
            continue;
        }
        let Some(ticks) = parse_ticks(fields) else { continue };
        if label == "cpu" {
            snapshot.total = Some(ticks);
        } else if let Some(id) = indexed(label, "cpu")
            && snapshot.cores.len() < MAX_CORES
        {
            snapshot.cores.push((id, ticks));
        }
    }
    snapshot
}

/// The share of the time between two readings that the CPU was not idle, in percent. `None` when
/// no time passed or the counters went backwards (a reset), because there is nothing to divide by.
pub fn busy_pct(prev: CpuTicks, now: CpuTicks) -> Option<f64> {
    let total = now.total.checked_sub(prev.total).filter(|t| *t > 0)?;
    let idle = now.idle.saturating_sub(prev.idle);
    let busy = total.saturating_sub(idle);
    Some((busy as f64 * 100.0 / total as f64).clamp(0.0, 100.0))
}

/// The load of one core.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct CoreLoad {
    /// The kernel's number of the CPU (`cpu3` is 3); offline numbers are absent, so the list can have gaps.
    pub id: usize,
    /// Busy percentage since the previous sample; `null` before there is a second sample.
    pub load_pct: Option<f64>,
}

/// One sample of the CPU load.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct CpuLoad {
    /// All cores together; `null` before there is a second sample or when `/proc/stat` is unreadable.
    pub total_pct: Option<f64>,
    /// Each online core.
    pub cores: Vec<CoreLoad>,
}

/// Turns successive `/proc/stat` readings into percentages. Not shared between threads by
/// itself: the application layer keeps one behind a mutex.
#[derive(Debug)]
pub struct CpuSampler {
    root: PathBuf,
    prev: Option<StatSnapshot>,
    last_total: Option<f64>,
    last_core: HashMap<usize, f64>,
}

impl CpuSampler {
    /// A sampler reading `/proc/stat` below `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), prev: None, last_total: None, last_core: HashMap::new() }
    }

    /// Whether a first reading exists, so the next [`sample`](Self::sample) can give percentages.
    pub fn has_baseline(&self) -> bool {
        self.prev.is_some()
    }

    /// Reads `/proc/stat` and answers the load since the previous call.
    pub fn sample(&mut self) -> CpuLoad {
        let Some(text) = read_text(&below(&self.root, "/proc/stat")) else {
            return CpuLoad::default();
        };
        let now = parse_stat(&text);
        if now.total.is_none() && now.cores.is_empty() {
            // Nothing usable: keep the baseline we have, a later read may be fine.
            return CpuLoad::default();
        }
        let prev = self.prev.take();
        let total_pct = match (prev.as_ref().and_then(|p| p.total), now.total) {
            (Some(p), Some(n)) => busy_pct(p, n).or(self.last_total),
            _ => self.last_total,
        };
        self.last_total = total_pct;
        let mut cores = Vec::with_capacity(now.cores.len());
        let mut remembered = HashMap::with_capacity(now.cores.len());
        for (id, ticks) in &now.cores {
            let before = prev.as_ref().and_then(|p| p.cores.iter().find(|(i, _)| i == id)).map(|(_, t)| *t);
            let load_pct =
                before.and_then(|b| busy_pct(b, *ticks).or_else(|| self.last_core.get(id).copied()));
            if let Some(v) = load_pct {
                remembered.insert(*id, v);
            }
            cores.push(CoreLoad { id: *id, load_pct });
        }
        // Only cores still online keep a history: one that left starts clean when it returns.
        self.last_core = remembered;
        self.prev = Some(now);
        CpuLoad { total_pct, cores }
    }
}

/// The clock of one core.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CoreFreq {
    /// The kernel's number of the CPU.
    pub id: usize,
    /// Current frequency in MHz; `null` when the cpufreq driver does not say.
    pub cur_mhz: Option<u32>,
    /// The highest frequency the core can reach, in MHz.
    pub max_mhz: Option<u32>,
}

fn mhz(path: &Path) -> Option<u32> {
    let khz = read_u64(path).filter(|k| *k <= MAX_PLAUSIBLE_KHZ)?;
    u32::try_from(khz / 1000).ok()
}

/// The frequency of every core that has a cpufreq directory, in core order. An offline core has
/// none and is simply absent. `scaling_cur_freq` is world-readable, unlike `cpuinfo_cur_freq`.
pub fn read_freqs(root: &Path) -> Vec<CoreFreq> {
    let base = below(root, "/sys/devices/system/cpu");
    let mut out = Vec::new();
    for name in dir_names(&base) {
        let Some(id) = indexed(&name, "cpu") else { continue };
        if out.len() >= MAX_CORES {
            break;
        }
        let dir = base.join(&name).join("cpufreq");
        let cur_mhz = mhz(&dir.join("scaling_cur_freq"));
        let max_mhz = mhz(&dir.join("cpuinfo_max_freq"));
        if cur_mhz.is_some() || max_mhz.is_some() {
            out.push(CoreFreq { id, cur_mhz, max_mhz });
        }
    }
    out
}

/// The scaling governor of the first core (`powersave`, `performance`), when there is one.
pub fn read_governor(root: &Path) -> Option<String> {
    read_line(&below(root, "/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")).filter(|g| !g.is_empty())
}

/// The run-queue averages of `/proc/loadavg`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct LoadAverage {
    /// Average number of runnable tasks over the last minute.
    pub one: f64,
    /// Over five minutes.
    pub five: f64,
    /// Over fifteen minutes.
    pub fifteen: f64,
    /// Tasks runnable right now.
    pub running: Option<u32>,
    /// Tasks and threads that exist.
    pub tasks: Option<u32>,
}

/// Parses `0.52 0.58 0.59 1/1234 5678`. `None` unless the three averages are finite, non-negative numbers.
pub fn parse_loadavg(text: &str) -> Option<LoadAverage> {
    let mut fields = text.split_whitespace();
    let mut average = || fields.next()?.parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0);
    let (one, five, fifteen) = (average()?, average()?, average()?);
    let counts = fields.next().and_then(|f| f.split_once('/'));
    let running = counts.and_then(|(r, _)| r.parse().ok());
    let tasks = counts.and_then(|(_, t)| t.parse().ok());
    Some(LoadAverage { one, five, fifteen, running, tasks })
}

/// The load average now, from `/proc/loadavg` below `root`.
pub fn read_loadavg(root: &Path) -> Option<LoadAverage> {
    parse_loadavg(&read_text(&below(root, "/proc/loadavg"))?)
}

/// Converts the cumulative RAPL energy counter into watts between two readings.
///
/// Since 2020 the kernel makes `energy_uj` readable by root only (a side-channel fix), so as a
/// normal user this reports `None`; the meter still works when the file is readable.
#[derive(Debug)]
pub struct RaplMeter {
    root: PathBuf,
    last: Option<(u64, f64)>,
}

const RAPL_DIR: &str = "/sys/class/powercap/intel-rapl:0";

impl RaplMeter {
    /// A meter reading below `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), last: None }
    }

    /// The package power in watts since the previous call; `None` for the first reading, for a
    /// zero or negative time step, and when the counter cannot be read. A counter that wrapped
    /// around is handled with `max_energy_range_uj`.
    pub fn watts(&mut self, now: f64) -> Option<f64> {
        let dir = below(&self.root, RAPL_DIR);
        let microjoules = read_u64(&dir.join("energy_uj"))?;
        let previous = self.last.replace((microjoules, now));
        let (before, then) = previous?;
        let dt = now - then;
        if dt <= 0.0 || !dt.is_finite() {
            return None;
        }
        let delta = if microjoules >= before {
            microjoules - before
        } else {
            let range = read_u64(&dir.join("max_energy_range_uj")).filter(|r| *r >= before)?;
            range - before + microjoules
        };
        Some(delta as f64 / 1e6 / dt)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::super::files::testutil::Tree;
    use super::*;

    /// A real `/proc/stat` of a 4-thread laptop, cut to the CPU lines plus the other lines the file has.
    const STAT_A: &str = "cpu  4705 150 3120 87654 321 0 42 0 0 0\n\
        cpu0 1200 30 800 21900 80 0 10 0 0 0\n\
        cpu1 1100 40 780 21950 81 0 12 0 0 0\n\
        cpu2 1205 40 770 21900 80 0 10 0 0 0\n\
        cpu3 1200 40 770 21904 80 0 10 0 0 0\n\
        intr 8912345 0 0\nctxt 123456\nbtime 1700000000\nprocesses 4321\n";

    fn stat(total: [u64; 3], cores: &[[u64; 3]]) -> String {
        // user, system, idle; the other counters zero
        let mut s = format!("cpu  {} 0 {} {} 0 0 0 0 0 0\n", total[0], total[1], total[2]);
        for (i, c) in cores.iter().enumerate() {
            s += &format!("cpu{i} {} 0 {} {} 0 0 0 0 0 0\n", c[0], c[1], c[2]);
        }
        s
    }

    #[test]
    fn parses_the_cpu_lines_of_a_real_proc_stat() {
        let s = parse_stat(STAT_A);
        let total = s.total.unwrap();
        assert_eq!(total.total, 4705 + 150 + 3120 + 87654 + 321 + 42);
        assert_eq!(total.idle, 87654 + 321, "idle and iowait both count as idle");
        assert_eq!(s.cores.iter().map(|(i, _)| *i).collect::<Vec<_>>(), [0, 1, 2, 3]);
    }

    #[test]
    fn guest_time_is_not_counted_twice() {
        // user 100 includes guest 50: the total must be 100 + 0 + 0 + 100 = 200, not 250.
        let s = parse_stat("cpu 100 0 0 100 0 0 0 0 50 0\n");
        assert_eq!(s.total.unwrap().total, 200);
    }

    #[test]
    fn hostile_stat_files_never_panic() {
        let cases = [
            "",
            "cpu",
            "cpu 1 2 3",
            "cpu a b c d",
            "cpu 18446744073709551615 1 1 1",
            "cpu -1 -1 -1 -1",
            "cpux 1 1 1 1\ncpu99999999999999999999 1 1 1 1",
            "\0\0\0",
            "cpu0 1 1 1 1 1 1 1 1 1 1 1 1 1 1",
            "cpu\t1\t1\t1\t1\n\n\ncpu0",
        ];
        for text in cases {
            let _ = parse_stat(text);
        }
        assert_eq!(parse_stat("cpu 18446744073709551615 1 1 1").total, None, "an overflowing sum is garbage");
        assert_eq!(parse_stat("cpu 1 2 3").total, None, "idle is missing");
        let one_good = parse_stat("cpu a b c d\ncpu0 1 1 1 1\n");
        assert_eq!(one_good.total, None);
        assert_eq!(one_good.cores.len(), 1, "a garbled line does not hide the next one");
        let many: String = (0..5000).map(|i| format!("cpu{i} 1 1 1 1\n")).collect();
        assert_eq!(parse_stat(&many).cores.len(), MAX_CORES);
    }

    #[test]
    fn busy_pct_handles_zero_delta_and_counters_that_went_backwards() {
        let a = CpuTicks { total: 1000, idle: 800 };
        assert_eq!(busy_pct(a, CpuTicks { total: 1100, idle: 850 }), Some(50.0));
        assert_eq!(busy_pct(a, a), None, "no time passed");
        assert_eq!(busy_pct(a, CpuTicks { total: 10, idle: 5 }), None, "counters reset");
        // idle ran backwards alone: busy cannot exceed the total
        assert_eq!(busy_pct(a, CpuTicks { total: 1100, idle: 700 }), Some(100.0));
        // idle grew more than total (inconsistent read): never negative
        assert_eq!(busy_pct(a, CpuTicks { total: 1010, idle: 900 }), Some(0.0));
    }

    #[test]
    fn sampler_needs_a_second_sample_then_gives_percentages() {
        let t = Tree::new();
        let mut s = CpuSampler::new(t.root());
        assert_eq!(s.sample(), CpuLoad::default(), "no file: nothing, no panic");
        assert!(!s.has_baseline());
        t.put("proc/stat", &stat([100, 100, 800], &[[50, 50, 400], [50, 50, 400]]));
        let first = s.sample();
        assert!(s.has_baseline());
        assert_eq!(first.total_pct, None);
        assert!(first.cores.iter().all(|c| c.load_pct.is_none()));
        // total: +100 busy, +100 idle -> 50 %; core0: +90 busy, +10 idle -> 90 %; core1: all idle
        t.put("proc/stat", &stat([150, 150, 900], &[[90, 100, 410], [50, 50, 500]]));
        let second = s.sample();
        assert_eq!(second.total_pct, Some(50.0));
        assert_eq!(second.cores[0].load_pct, Some(90.0));
        assert_eq!(second.cores[1].load_pct, Some(0.0));
    }

    #[test]
    fn zero_delta_holds_the_previous_percentage() {
        let t = Tree::new();
        let mut s = CpuSampler::new(t.root());
        t.put("proc/stat", &stat([0, 0, 0], &[[0, 0, 0]]));
        s.sample();
        t.put("proc/stat", &stat([50, 0, 50], &[[50, 0, 50]]));
        assert_eq!(s.sample().total_pct, Some(50.0));
        // the same counters again (two reads inside one jiffy)
        let again = s.sample();
        assert_eq!(again.total_pct, Some(50.0));
        assert_eq!(again.cores[0].load_pct, Some(50.0));
        // counters reset to small values: still held, then recovers
        t.put("proc/stat", &stat([1, 0, 1], &[[1, 0, 1]]));
        assert_eq!(s.sample().total_pct, Some(50.0));
        t.put("proc/stat", &stat([11, 0, 11], &[[11, 0, 11]]));
        assert_eq!(s.sample().total_pct, Some(50.0));
        t.put("proc/stat", &stat([21, 0, 11], &[[21, 0, 11]]));
        assert_eq!(s.sample().total_pct, Some(100.0));
    }

    #[test]
    fn a_core_that_goes_offline_leaves_the_list_and_returns_clean() {
        let t = Tree::new();
        let mut s = CpuSampler::new(t.root());
        t.put("proc/stat", &stat([0, 0, 0], &[[0, 0, 0], [0, 0, 0]]));
        s.sample();
        t.put("proc/stat", &stat([20, 0, 20], &[[10, 0, 10], [10, 0, 10]]));
        assert_eq!(s.sample().cores.len(), 2);
        // cpu1 is offlined: its line is gone
        t.put("proc/stat", "cpu  30 0 0 30 0 0 0 0 0 0\ncpu0 20 0 0 20 0 0 0 0 0 0\n");
        let out = s.sample();
        assert_eq!(out.cores.iter().map(|c| c.id).collect::<Vec<_>>(), [0]);
        // it comes back: no history, so no percentage until the second sample
        t.put("proc/stat", &stat([40, 0, 40], &[[30, 0, 30], [10, 0, 10]]));
        let back = s.sample();
        assert_eq!(back.cores.len(), 2);
        assert_eq!(back.cores[1].load_pct, None);
        t.put("proc/stat", &stat([60, 0, 60], &[[40, 0, 40], [20, 0, 10]]));
        assert_eq!(s.sample().cores[1].load_pct, Some(100.0));
    }

    #[test]
    fn an_unreadable_stat_keeps_the_baseline() {
        let t = Tree::new();
        let mut s = CpuSampler::new(t.root());
        t.put("proc/stat", &stat([0, 0, 0], &[[0, 0, 0]]));
        s.sample();
        t.put("proc/stat", "garbage\n");
        assert_eq!(s.sample(), CpuLoad::default());
        assert!(s.has_baseline());
        t.put("proc/stat", &stat([10, 0, 10], &[[10, 0, 10]]));
        assert_eq!(s.sample().total_pct, Some(50.0));
    }

    #[test]
    fn frequencies_come_from_cpufreq_and_skip_offline_and_garbage() {
        let t = Tree::new();
        let cpu = "sys/devices/system/cpu";
        t.put(&format!("{cpu}/cpu0/cpufreq/scaling_cur_freq"), "3200000\n");
        t.put(&format!("{cpu}/cpu0/cpufreq/cpuinfo_max_freq"), "5400000\n");
        t.put(&format!("{cpu}/cpu0/cpufreq/scaling_governor"), "powersave\n");
        t.put(&format!("{cpu}/cpu2/cpufreq/scaling_cur_freq"), "800000\n");
        t.put(&format!("{cpu}/cpu10/cpufreq/scaling_cur_freq"), "not a number\n");
        t.put(&format!("{cpu}/cpu11/cpufreq/scaling_cur_freq"), "999999999999\n");
        t.put(&format!("{cpu}/cpu3/online"), "0\n"); // offline: no cpufreq directory
        t.put(&format!("{cpu}/cpufreq/policy0"), "x");
        let freqs = read_freqs(t.root());
        assert_eq!(
            freqs,
            vec![
                CoreFreq { id: 0, cur_mhz: Some(3200), max_mhz: Some(5400) },
                CoreFreq { id: 2, cur_mhz: Some(800), max_mhz: None },
            ]
        );
        assert_eq!(read_governor(t.root()).as_deref(), Some("powersave"));
        assert!(read_freqs(&t.root().join("nowhere")).is_empty());
    }

    #[test]
    fn loadavg_parses_and_rejects_garbage() {
        let l = parse_loadavg("0.52 0.58 0.59 1/1234 5678\n").unwrap();
        assert_eq!((l.one, l.five, l.fifteen, l.running, l.tasks), (0.52, 0.58, 0.59, Some(1), Some(1234)));
        assert!(parse_loadavg("0.1 0.2 0.3").unwrap().tasks.is_none());
        for bad in ["", "a b c", "1 2", "NaN 1 1", "-1 1 1", "inf 1 1"] {
            assert!(parse_loadavg(bad).is_none(), "{bad}");
        }
        let odd = parse_loadavg("0.1 0.2 0.3 x/y z").unwrap();
        assert_eq!((odd.running, odd.tasks), (None, None), "garbled counts do not hide the averages");
    }

    #[test]
    fn rapl_power_needs_two_readings_and_survives_wraparound() {
        let t = Tree::new();
        let dir = "sys/class/powercap/intel-rapl:0";
        let mut m = RaplMeter::new(t.root());
        assert_eq!(m.watts(0.0), None, "no file (root-only on current kernels)");
        t.put(&format!("{dir}/energy_uj"), "1000000\n");
        assert_eq!(m.watts(10.0), None, "first reading");
        t.put(&format!("{dir}/energy_uj"), "31000000\n");
        assert_eq!(m.watts(12.0), Some(15.0), "30 J in 2 s");
        assert_eq!(m.watts(12.0), None, "no time passed");
        // wrap: 31 J -> counter range 100 J, now 5 J: 74 J in 2 s
        t.put(&format!("{dir}/max_energy_range_uj"), "100000000\n");
        t.put(&format!("{dir}/energy_uj"), "5000000\n");
        assert_eq!(m.watts(14.0), Some(37.0));
        // wrap without a known range, or a range smaller than the last value: unknown, not negative
        t.remove(&format!("{dir}/max_energy_range_uj"));
        t.put(&format!("{dir}/energy_uj"), "1000000\n");
        assert_eq!(m.watts(16.0), None);
    }
}
