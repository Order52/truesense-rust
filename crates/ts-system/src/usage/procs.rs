//! The busiest processes, found with one scan of `/proc` per sample.
//!
//! The cost model matters because the monitor page asks every second or two. For each process
//! the scan reads exactly one small file, `/proc/<pid>/stat`, which holds the name, state, CPU
//! ticks and resident pages; the owner comes from the `stat` of the directory (the kernel makes
//! `/proc/<pid>` belong to the process's user), so `status` is never opened. Only after ranking,
//! for the few processes that will be shown, is `cmdline` read. The user-name table
//! (`/etc/passwd`) is read once per scan, not once per process.
//!
//! [`ProcSampler`] also refuses to scan twice inside `min_interval`: two page tabs, or the CPU
//! and memory pages asking in the same second, share one scan.
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

use serde::Serialize;

use super::cpu::parse_stat;
use super::files::{below, read_prefix, read_text};

/// The most processes in each ranking; a request for more is cut to this.
pub const MAX_TOP: usize = 50;

/// The most `/proc` entries looked at in one scan (the kernel's default limit on pids is 4 million;
/// a laptop has hundreds, so this only matters for a hostile tree).
const MAX_SCAN: usize = 200_000;

/// A `stat` file is a few hundred bytes; anything bigger is not one.
const MAX_STAT: u64 = 16 * 1024;

/// How much of `cmdline` is kept: enough to tell two `python` processes apart.
const CMDLINE_KEEP: u64 = 512;

/// Pid files older than this are not reused for a CPU delta: a pause this long (suspend) makes an
/// average over the gap meaningless.
const STALE_SECS: f64 = 30.0;

/// One process as the page shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProcessInfo {
    /// Process id.
    pub pid: u32,
    /// The kernel's name of the process (`comm`, at most 15 characters).
    pub name: String,
    /// The command line, shortened; empty for kernel threads.
    pub cmdline: String,
    /// The owner's login name, or the number when `/etc/passwd` does not know it.
    pub user: String,
    /// The scheduler state letter: `R` running, `S` sleeping, `D` disk wait, `Z` zombie, `T` stopped.
    pub state: String,
    /// CPU use since the previous scan in percent of one core (100 is one full core, so it can
    /// exceed 100 on several cores); 0 on the first scan.
    pub cpu_pct: f64,
    /// Resident memory in bytes.
    pub mem_bytes: u64,
    /// Threads in the process.
    pub threads: u32,
}

/// What one scan found.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct ProcSnapshot {
    /// When the scan was made (the caller's clock, seconds).
    pub taken_at: f64,
    /// Processes that were read successfully.
    pub scanned: usize,
    /// False for the first scan, when no CPU percentage can exist yet.
    pub cpu_valid: bool,
    /// The busiest by CPU, highest first.
    pub by_cpu: Vec<ProcessInfo>,
    /// The largest by resident memory, highest first.
    pub by_mem: Vec<ProcessInfo>,
}

/// The fields of `/proc/<pid>/stat` the monitor uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PidStat {
    /// `comm`, which may contain spaces and parentheses.
    pub name: String,
    /// Field 3, the state letter.
    pub state: String,
    /// Fields 14 and 15 added: user and system ticks.
    pub cpu_ticks: u64,
    /// Field 20, threads.
    pub threads: u32,
    /// Field 22, the start time in ticks after boot: with the pid it identifies a process even when the pid is reused.
    pub start_time: u64,
    /// Field 24, resident pages.
    pub rss_pages: u64,
}

/// Parses one `stat` line. The name is delimited by the first `(` and the *last* `)` because a
/// process may call itself `a) b (c`; everything after it is whitespace-separated numbers.
pub fn parse_pid_stat(text: &str) -> Option<PidStat> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    if close < open {
        return None;
    }
    let name = text[open + 1..close].to_owned();
    let rest: Vec<&str> = text[close + 1..].split_whitespace().collect();
    // `rest[0]` is field 3 (state): field N is rest[N - 3].
    if rest.len() < 22 {
        return None;
    }
    let number = |field: usize| rest[field - 3].parse::<u64>().ok();
    Some(PidStat {
        name,
        state: rest[0].chars().take(1).collect(),
        cpu_ticks: number(14)?.checked_add(number(15)?)?,
        threads: u32::try_from(number(20)?).ok()?,
        start_time: number(22)?,
        rss_pages: number(24)?,
    })
}

/// Login names by uid; the first line for a uid wins, as `getpwuid` would answer.
pub fn parse_passwd(text: &str) -> HashMap<u32, String> {
    let mut users = HashMap::new();
    for line in text.lines() {
        let mut parts = line.split(':');
        let (Some(name), Some(_), Some(uid)) = (parts.next(), parts.next(), parts.next()) else { continue };
        if let (false, Ok(uid)) = (name.is_empty(), uid.parse::<u32>()) {
            users.entry(uid).or_insert_with(|| name.to_owned());
        }
    }
    users
}

/// A process seen in the previous scan, to compute its CPU use.
#[derive(Debug, Clone, Copy)]
struct Seen {
    ticks: u64,
    start_time: u64,
}

/// A scan result before the command lines are read.
struct Candidate {
    pid: u32,
    stat: PidStat,
    uid: u32,
    cpu_pct: f64,
}

/// Samples the process table. Keep one for the life of the page: the CPU percentage of a process
/// is the change since the previous scan.
#[derive(Debug)]
pub struct ProcSampler {
    root: PathBuf,
    page_bytes: u64,
    top: usize,
    min_interval: f64,
    prev: HashMap<u32, Seen>,
    prev_total: Option<u64>,
    last: Option<ProcSnapshot>,
}

impl ProcSampler {
    /// A sampler of `/proc` below `root` keeping `top` processes per ranking (at most
    /// [`MAX_TOP`]) and scanning at most once per `min_interval` seconds. `page_bytes` is the
    /// kernel's page size (4096 on x86-64): `stat` counts resident memory in pages and reading
    /// the real value needs a system call that this crate avoids.
    pub fn new(root: impl Into<PathBuf>, top: usize, min_interval: f64, page_bytes: u64) -> Self {
        Self {
            root: root.into(),
            page_bytes,
            top: top.clamp(1, MAX_TOP),
            min_interval,
            prev: HashMap::new(),
            prev_total: None,
            last: None,
        }
    }

    /// Whether at least one scan has been made.
    pub fn has_baseline(&self) -> bool {
        self.last.is_some()
    }

    /// The latest snapshot, scanning only when the last one is `min_interval` old (or from the
    /// future, which a clock step can cause).
    pub fn sample(&mut self, now: f64) -> &ProcSnapshot {
        let fresh =
            self.last.as_ref().is_some_and(|l| now >= l.taken_at && now - l.taken_at < self.min_interval);
        if !fresh {
            let snapshot = self.scan(now);
            self.last = Some(snapshot);
        }
        self.last.get_or_insert_with(ProcSnapshot::default)
    }

    /// Scans now whatever the interval says: for priming a new sampler, which needs a second scan
    /// a moment after the first to have CPU percentages at all.
    pub fn rescan(&mut self, now: f64) -> &ProcSnapshot {
        let snapshot = self.scan(now);
        self.last.insert(snapshot)
    }

    /// Resizes the rankings (a page asking for a different number); the next scan applies it.
    pub fn set_top(&mut self, top: usize) {
        self.top = top.clamp(1, MAX_TOP);
    }

    /// The ranking size.
    pub fn top(&self) -> usize {
        self.top
    }

    fn scan(&mut self, now: f64) -> ProcSnapshot {
        let proc_dir = below(&self.root, "/proc");
        let stat_text = read_text(&below(&self.root, "/proc/stat")).unwrap_or_default();
        let cpus = parse_stat(&stat_text);
        let total = cpus.total.map(|t| t.total);
        let ncpu = cpus.cores.len().max(1) as f64;
        let stale = self.last.as_ref().is_none_or(|l| now < l.taken_at || now - l.taken_at > STALE_SECS);
        // Ticks that elapsed on all CPUs together since the previous scan.
        let total_delta = match (self.prev_total, total) {
            (Some(before), Some(after)) if !stale => after.checked_sub(before).filter(|d| *d > 0),
            _ => None,
        };
        let cpu_valid = total_delta.is_some();

        let mut seen_now = HashMap::new();
        let mut candidates = Vec::new();
        for entry in fs::read_dir(&proc_dir).into_iter().flatten().filter_map(Result::ok).take(MAX_SCAN) {
            let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
            let dir = entry.path();
            // The process may exit between the listing and the read: that is not an error.
            let Some(text) = read_prefix(&dir.join("stat"), MAX_STAT) else { continue };
            let Some(stat) = parse_pid_stat(&text) else { continue };
            let uid = fs::metadata(&dir).map_or(u32::MAX, |m| m.uid());
            let cpu_pct = match (total_delta, self.prev.get(&pid)) {
                (Some(delta), Some(before)) if before.start_time == stat.start_time => {
                    let used = stat.cpu_ticks.saturating_sub(before.ticks);
                    (used as f64 / delta as f64 * 100.0 * ncpu).clamp(0.0, 100.0 * ncpu)
                }
                _ => 0.0,
            };
            seen_now.insert(pid, Seen { ticks: stat.cpu_ticks, start_time: stat.start_time });
            candidates.push(Candidate { pid, stat, uid, cpu_pct });
        }
        // Processes that exited are forgotten here, so the table cannot grow without bound.
        self.prev = seen_now;
        self.prev_total = total;
        let scanned = candidates.len();

        let mut by_cpu: Vec<usize> = (0..scanned).collect();
        by_cpu.sort_by(|a, b| {
            let (x, y) = (&candidates[*a], &candidates[*b]);
            y.cpu_pct.total_cmp(&x.cpu_pct).then(x.pid.cmp(&y.pid))
        });
        let mut by_mem: Vec<usize> = (0..scanned).collect();
        by_mem.sort_by(|a, b| {
            let (x, y) = (&candidates[*a], &candidates[*b]);
            y.stat.rss_pages.cmp(&x.stat.rss_pages).then(x.pid.cmp(&y.pid))
        });
        by_cpu.truncate(self.top);
        by_mem.truncate(self.top);

        let users = parse_passwd(&read_text(&below(&self.root, "/etc/passwd")).unwrap_or_default());
        let mut cmdlines: HashMap<usize, String> = HashMap::new();
        let mut build = |index: usize| {
            let c = &candidates[index];
            let cmdline = cmdlines
                .entry(index)
                .or_insert_with(|| {
                    let raw = read_prefix(&proc_dir.join(c.pid.to_string()).join("cmdline"), CMDLINE_KEEP);
                    raw.map(|r| r.replace('\0', " ").trim().to_owned()).unwrap_or_default()
                })
                .clone();
            ProcessInfo {
                pid: c.pid,
                name: c.stat.name.clone(),
                cmdline,
                user: users.get(&c.uid).cloned().unwrap_or_else(|| c.uid.to_string()),
                state: c.stat.state.clone(),
                cpu_pct: c.cpu_pct,
                mem_bytes: c.stat.rss_pages.saturating_mul(self.page_bytes),
                threads: c.stat.threads,
            }
        };
        let by_cpu = by_cpu.into_iter().map(&mut build).collect();
        let by_mem = by_mem.into_iter().map(&mut build).collect();
        ProcSnapshot { taken_at: now, scanned, cpu_valid, by_cpu, by_mem }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::super::files::testutil::Tree;
    use super::*;

    /// A `stat` line the way the kernel prints it (52 fields; the monitor reads six of them).
    fn stat_line(
        pid: u32,
        comm: &str,
        state: char,
        ticks: (u64, u64),
        threads: u32,
        start: u64,
        rss: u64,
    ) -> String {
        format!(
            "{pid} ({comm}) {state} 1 {pid} {pid} 0 -1 4194560 1200 0 3 0 {} {} 0 0 20 0 {threads} 0 {start} \
             123456789 {rss} 18446744073709551615 1 1 0 0 0 0 0 4096 0 0 0 0 17 3 0 0 0 0 0\n",
            ticks.0, ticks.1
        )
    }

    fn proc(t: &Tree, pid: u32, comm: &str, ticks: (u64, u64), start: u64, rss: u64) {
        t.put(&format!("proc/{pid}/stat"), &stat_line(pid, comm, 'S', ticks, 2, start, rss));
    }

    fn cpu_stat(total_busy: u64, total_idle: u64) -> String {
        // two cores, each gets half
        let (b, i) = (total_busy / 2, total_idle / 2);
        format!(
            "cpu  {total_busy} 0 0 {total_idle} 0 0 0 0 0 0\ncpu0 {b} 0 0 {i} 0 0 0 0 0 0\ncpu1 {b} 0 0 {i} 0 0 0 0 0 0\n"
        )
    }

    #[test]
    fn parses_a_real_stat_line_including_awkward_names() {
        let s = parse_pid_stat(&stat_line(42, "firefox", 'S', (700, 300), 56, 9000, 250_000)).unwrap();
        assert_eq!(
            (s.name.as_str(), s.state.as_str(), s.cpu_ticks, s.threads, s.start_time, s.rss_pages),
            ("firefox", "S", 1000, 56, 9000, 250_000)
        );
        let odd = parse_pid_stat(&stat_line(7, "a) b (c d", 'R', (1, 1), 1, 5, 6)).unwrap();
        assert_eq!(odd.name, "a) b (c d");
        assert_eq!(odd.state, "R");
        assert_eq!(parse_pid_stat(&stat_line(8, "", 'Z', (0, 0), 1, 1, 0)).unwrap().name, "");
    }

    #[test]
    fn hostile_stat_lines_never_panic() {
        for text in [
            "",
            "1",
            "1 (x",
            "1 x) (y",
            ") (",
            "1 (x) S",
            "1 (x) S 1 2 3",
            "1 (x) S 1 1 1 0 -1 0 0 0 0 0 a b 0 0 20 0 1 0 1 1 1",
            "1 (x) S 1 1 1 0 -1 0 0 0 0 0 99999999999999999999 1 0 0 20 0 1 0 1 1 1 1",
            "1 (x) S 1 1 1 0 -1 0 0 0 0 0 18446744073709551615 1 0 0 20 0 1 0 1 1 1 1",
            "\0\0\0 (\0) \0",
        ] {
            let _ = parse_pid_stat(text);
        }
        assert_eq!(parse_pid_stat("1 (x) S 1 2 3"), None);
        assert_eq!(
            parse_pid_stat("1 (x) S 1 1 1 0 -1 0 0 0 0 0 18446744073709551615 1 0 0 20 0 1 0 1 1 1 1"),
            None,
            "ticks overflow"
        );
    }

    #[test]
    fn passwd_first_line_wins_and_garbage_is_skipped() {
        let users = parse_passwd(
            "root:x:0:0::/root:/bin/bash\nghost:x:1000:1000::/home/ghost:/bin/zsh\ntwin:x:1000:1000::/:/bin/sh\nbroken line\n:x:5:5\nbad:x:abc:1\n",
        );
        assert_eq!(users.get(&0).map(String::as_str), Some("root"));
        assert_eq!(users.get(&1000).map(String::as_str), Some("ghost"));
        assert_eq!(users.len(), 2);
    }

    #[test]
    fn ranks_by_cpu_and_by_memory_from_one_scan() {
        let t = Tree::new();
        let uid = std::fs::metadata(t.root()).unwrap().uid();
        t.put("etc/passwd", &format!("me:x:{uid}:{uid}::/home/me:/bin/sh\n"));
        t.put("proc/stat", &cpu_stat(1000, 3000));
        proc(&t, 10, "busy", (100, 0), 1, 1000);
        proc(&t, 11, "big", (10, 0), 2, 900_000);
        proc(&t, 12, "idle", (0, 0), 3, 10);
        t.put("proc/10/cmdline", "/usr/bin/busy\0--flag\0");
        t.put("proc/self/stat", "not a pid directory");
        t.put("proc/cpuinfo", "ignored");
        let mut s = ProcSampler::new(t.root(), 2, 1.0, 4096);
        assert!(!s.has_baseline());
        let first = s.sample(100.0).clone();
        assert!(!first.cpu_valid);
        assert_eq!(first.scanned, 3);
        assert!(first.by_cpu.iter().all(|p| p.cpu_pct == 0.0));
        assert_eq!(first.by_mem.iter().map(|p| p.pid).collect::<Vec<_>>(), [11, 10]);
        assert_eq!(first.by_mem[0].mem_bytes, 900_000 * 4096);
        assert_eq!(first.by_mem[0].user, "me", "name from passwd");
        // 4000 ticks on 2 CPUs elapse; busy used 400 -> 400/4000 * 100 * 2 = 20 % of one core
        t.put("proc/stat", &cpu_stat(3000, 5000));
        proc(&t, 10, "busy", (300, 200), 1, 1000);
        proc(&t, 11, "big", (10, 0), 2, 900_000);
        t.remove("proc/12");
        let second = s.sample(102.0).clone();
        assert!(second.cpu_valid);
        assert_eq!(second.scanned, 2, "pid 12 exited");
        assert_eq!(second.by_cpu[0].pid, 10);
        assert!((second.by_cpu[0].cpu_pct - 20.0).abs() < 1e-9, "{}", second.by_cpu[0].cpu_pct);
        assert_eq!(second.by_cpu[0].cmdline, "/usr/bin/busy --flag");
        assert_eq!(second.by_cpu[0].user, "me");
        assert_eq!(second.by_cpu[1].cpu_pct, 0.0);
    }

    #[test]
    fn one_scan_serves_every_caller_inside_the_interval() {
        let t = Tree::new();
        t.put("proc/stat", &cpu_stat(10, 10));
        proc(&t, 1, "init", (1, 1), 1, 5);
        let mut s = ProcSampler::new(t.root(), 5, 1.5, 4096);
        assert_eq!(s.sample(10.0).scanned, 1);
        proc(&t, 2, "new", (1, 1), 2, 5);
        assert_eq!(s.sample(10.4).scanned, 1, "inside the interval: the cached scan");
        assert_eq!(s.sample(11.4).scanned, 1);
        assert_eq!(s.sample(11.6).scanned, 2, "interval over: scans again");
        assert_eq!(s.sample(5.0).scanned, 2, "a clock that went backwards rescans");
        proc(&t, 3, "newer", (1, 1), 3, 5);
        assert_eq!(s.sample(5.1).scanned, 2, "cached");
        assert_eq!(s.rescan(5.2).scanned, 3, "a forced scan ignores the interval");
    }

    #[test]
    fn a_reused_pid_is_a_new_process_and_zero_total_delta_gives_no_percentages() {
        let t = Tree::new();
        t.put("proc/stat", &cpu_stat(100, 100));
        proc(&t, 5, "old", (50, 50), 1, 5);
        let mut s = ProcSampler::new(t.root(), 5, 0.0, 4096);
        s.sample(1.0);
        // same pid, different start time, ticks lower: must not be a negative or huge delta
        t.put("proc/stat", &cpu_stat(200, 200));
        proc(&t, 5, "new", (10, 0), 99, 5);
        assert_eq!(s.sample(2.0).by_cpu[0].cpu_pct, 0.0);
        // the CPU counters did not move: no division by zero
        proc(&t, 5, "new", (90, 0), 99, 5);
        let frozen = s.sample(3.0);
        assert!(!frozen.cpu_valid);
        assert_eq!(frozen.by_cpu[0].cpu_pct, 0.0);
    }

    #[test]
    fn a_long_pause_does_not_average_over_the_gap() {
        let t = Tree::new();
        t.put("proc/stat", &cpu_stat(100, 100));
        proc(&t, 5, "p", (0, 0), 1, 5);
        let mut s = ProcSampler::new(t.root(), 5, 0.0, 4096);
        s.sample(1.0);
        t.put("proc/stat", &cpu_stat(1000, 1000));
        proc(&t, 5, "p", (500, 0), 1, 5);
        assert!(!s.sample(1000.0).cpu_valid, "a suspend in between: start over");
    }

    #[test]
    fn broken_entries_are_skipped_and_limits_hold() {
        let t = Tree::new();
        t.put("proc/stat", &cpu_stat(10, 10));
        for pid in 1..=80 {
            proc(&t, pid, "p", (0, 0), 1, u64::from(pid));
        }
        t.put("proc/90/stat", "garbage");
        t.put_bytes("proc/91/stat", &[0xff, 0xfe, 0x28, 0x29]);
        std::fs::create_dir_all(t.root().join("proc/92")).unwrap(); // a process with no stat (just exited)
        t.put("proc/notapid/stat", "x");
        let mut s = ProcSampler::new(t.root(), 1000, 0.0, 4096);
        assert_eq!(s.top(), MAX_TOP);
        let snap = s.sample(1.0);
        assert_eq!(snap.scanned, 80);
        assert_eq!(snap.by_mem.len(), MAX_TOP);
        assert_eq!(snap.by_mem[0].pid, 80, "largest first");
        let uid = std::fs::metadata(t.root()).unwrap().uid();
        assert_eq!(snap.by_mem[0].user, uid.to_string(), "no passwd entry: the number");
        s.set_top(0);
        assert_eq!(s.top(), 1);
        let missing = ProcSampler::new(t.root().join("nowhere"), 3, 0.0, 4096).sample(1.0).clone();
        assert_eq!(missing.scanned, 0);
    }
}
