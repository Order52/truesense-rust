//! The thermal governor: when the CPU gets hot it lowers the maximum clock (the cpufreq cap) a
//! step at a time, and gives the clock back when it cools, so the fans and the firmware do not
//! have to fight a sustained load.
//!
//! The decision ([`Governor`]) is a pure state machine over a tick counter and the CPU
//! temperature; writing the cap to the cpufreq files ([`CapWriter`]) is separate and takes the
//! directory it writes to, so tests use a temporary tree.
//!
//! It only runs while a smart feature is on (`active`): otherwise the firmware is left alone,
//! like PredatorSense on Windows, and any cap this program set is lifted. A cap left behind by
//! a stopped program would hold the CPU below its base clock for good, which is why the
//! daemon uncaps on every exit as well ([`CapWriter::uncap`]).
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use ts_foundation::config::CpuConfig;
use ts_foundation::fsx::{self, FsError};

/// The governor's thresholds: degrees Celsius and kHz.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Params {
    /// Whether the governor runs at all (`[cpu] gov_enabled`).
    pub enabled: bool,
    /// Lower the cap at or above this temperature (`gov_hi`).
    pub hi: i64,
    /// Raise the cap at or below this temperature (`gov_lo`).
    pub lo: i64,
    /// The cap that means "no cap" (`cap_max_khz`).
    pub max_khz: i64,
    /// Never lower the cap below this (`cap_min_khz`).
    pub min_khz: i64,
    /// One step down (`cap_down_khz`).
    pub down_khz: i64,
    /// One step up (`cap_up_khz`).
    pub up_khz: i64,
}

impl From<&CpuConfig> for Params {
    fn from(c: &CpuConfig) -> Self {
        Params {
            enabled: c.gov_enabled,
            hi: c.gov_hi,
            lo: c.gov_lo,
            max_khz: c.cap_max_khz,
            min_khz: c.cap_min_khz,
            down_khz: c.cap_down_khz,
            up_khz: c.cap_up_khz,
        }
    }
}

/// After a change, the next step down waits this many ticks.
const DOWN_EVERY_TICKS: u64 = 2;
/// After a change, the next step up waits this many ticks (cooling is slower to trust).
const UP_EVERY_TICKS: u64 = 3;
/// While capped, the cap is written again every this many ticks, because something else (a
/// power manager, a resume from suspend) may have raised the limit behind our back.
const REASSERT_EVERY_TICKS: u64 = 30;

/// The current cap and when it last changed (in ticks of the daemon's loop).
#[derive(Debug, Clone)]
pub struct Governor {
    params: Params,
    cap: i64,
    changed_tick: u64,
}

impl Governor {
    /// A governor with no cap applied.
    pub fn new(params: Params) -> Governor {
        Governor { cap: params.max_khz, params, changed_tick: 0 }
    }

    /// The cap now, in kHz (equal to the maximum when there is none).
    pub fn cap(&self) -> i64 {
        self.cap
    }

    /// The thresholds in use.
    pub fn params(&self) -> &Params {
        &self.params
    }

    /// Whether a cap lower than the maximum is in force.
    pub fn capped(&self) -> bool {
        self.cap < self.params.max_khz
    }

    /// Decides the cap for this tick and returns whether the cpufreq files must be (re)written
    /// with [`Governor::cap`].
    ///
    /// `active` says whether TrueSense manages the CPU at all (a smart feature is on); when it
    /// does not, or the governor is disabled, the full clock is handed back once.
    pub fn step(&mut self, tick: u64, tcpu: f64, active: bool) -> bool {
        let p = self.params;
        if !p.enabled || !active {
            if self.cap < p.max_khz {
                self.cap = p.max_khz;
                return true;
            }
            return false;
        }
        let since = tick.saturating_sub(self.changed_tick);
        let mut next = self.cap;
        if tcpu >= p.hi as f64 && since >= DOWN_EVERY_TICKS {
            next = p.min_khz.max(self.cap - p.down_khz);
        } else if tcpu <= p.lo as f64 && self.cap < p.max_khz && since >= UP_EVERY_TICKS {
            next = p.max_khz.min(self.cap + p.up_khz);
        }
        if next != self.cap {
            self.cap = next;
            self.changed_tick = tick;
            return true;
        }
        self.capped() && tick % REASSERT_EVERY_TICKS == 0
    }

    /// The situation changed (plug or unplug, resume): the next change is allowed only after
    /// the usual interval, because the sensors and the firmware are settling.
    pub fn resync(&mut self, tick: u64) {
        self.changed_tick = tick;
    }

    /// Forgets the cap (the daemon is stopping; [`CapWriter::uncap`] hands the clock back).
    pub fn release(&mut self) {
        self.cap = self.params.max_khz;
    }
}

/// Where the kernel keeps the per-CPU frequency policies.
pub const CPU_DIR: &str = "/sys/devices/system/cpu";

/// Sets the maximum clock of every CPU through cpufreq.
#[derive(Debug, Clone)]
pub struct CapWriter {
    cpu_dir: PathBuf,
}

impl CapWriter {
    /// A writer over the real `/sys/devices/system/cpu`.
    pub fn system() -> CapWriter {
        CapWriter::new(CPU_DIR)
    }

    /// A writer over another tree (tests).
    pub fn new(cpu_dir: impl Into<PathBuf>) -> CapWriter {
        CapWriter { cpu_dir: cpu_dir.into() }
    }

    /// The `cpufreq` directories of every `cpuN`, in name order. `cpufreq` and `cpuidle` next
    /// to them do not match: the name must be `cpu` and a digit.
    fn policies(&self) -> Vec<PathBuf> {
        let Ok(rd) = std::fs::read_dir(&self.cpu_dir) else { return Vec::new() };
        let mut dirs: Vec<PathBuf> = rd
            .filter_map(Result::ok)
            .filter(|e| {
                let name = e.file_name();
                name.to_str()
                    .and_then(|n| n.strip_prefix("cpu"))
                    .and_then(|r| r.chars().next())
                    .is_some_and(|c| c.is_ascii_digit())
            })
            .map(|e| e.path().join("cpufreq"))
            .filter(|p| p.join("scaling_max_freq").exists())
            .collect();
        dirs.sort();
        dirs
    }

    /// Writes `khz` to every CPU whose limit differs. A CPU that cannot be written (it went
    /// offline) does not stop the others; the failures are returned for the log.
    pub fn apply(&self, khz: i64) -> Vec<FsError> {
        let want = khz.to_string();
        let mut failed = Vec::new();
        for dir in self.policies() {
            let file = dir.join("scaling_max_freq");
            if fsx::read_trim_opt(&file).as_deref() == Some(want.as_str()) {
                continue;
            }
            if let Err(e) = fsx::write_existing(&file, want.as_bytes()) {
                failed.push(e);
            }
        }
        failed
    }

    /// Gives every CPU its own hardware maximum back (`cpuinfo_max_freq`): what the daemon does
    /// on every exit, so a stopped program can never leave the machine capped.
    pub fn uncap(&self) -> Vec<FsError> {
        let mut failed = Vec::new();
        for dir in self.policies() {
            if let Some(max) = fsx::read_trim_opt(&dir.join("cpuinfo_max_freq")).filter(|m| !m.is_empty()) {
                if let Err(e) = fsx::write_existing(&dir.join("scaling_max_freq"), max.as_bytes()) {
                    failed.push(e);
                }
            }
        }
        failed
    }

    /// The directory this writer works on.
    pub fn cpu_dir(&self) -> &Path {
        &self.cpu_dir
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::smartprofile::testdir::TempDir;

    /// The shipped `[cpu]` defaults: cap down at 88 C, up at 83 C, 5.0 GHz down to 2.8 GHz.
    fn params() -> Params {
        Params {
            enabled: true,
            hi: 88,
            lo: 83,
            max_khz: 5_000_000,
            min_khz: 2_800_000,
            down_khz: 200_000,
            up_khz: 100_000,
        }
    }

    #[test]
    fn params_come_from_the_cpu_config() {
        let c = CpuConfig::default();
        let p = Params::from(&c);
        assert_eq!((p.enabled, p.hi, p.lo, p.max_khz), (c.gov_enabled, c.gov_hi, c.gov_lo, c.cap_max_khz));
        assert_eq!((p.min_khz, p.down_khz, p.up_khz), (c.cap_min_khz, c.cap_down_khz, c.cap_up_khz));
    }

    #[test]
    fn hot_lowers_the_cap_at_most_every_two_ticks_and_cool_raises_it_every_three() {
        let mut g = Governor::new(params());
        assert!(g.step(10, 90.0, true));
        assert_eq!(g.cap(), 4_800_000);
        assert!(!g.step(11, 95.0, true), "one tick later it must wait (no flapping)");
        assert!(g.step(12, 95.0, true));
        assert_eq!(g.cap(), 4_600_000);
        assert!(!g.step(14, 80.0, true), "two ticks after the last change is too soon to go back up");
        assert!(g.step(15, 80.0, true));
        assert_eq!(g.cap(), 4_700_000);
        assert!(!g.step(20, 85.0, true), "between lo and hi nothing changes");
        assert_eq!(g.cap(), 4_700_000);
    }

    #[test]
    fn thresholds_are_inclusive() {
        let mut g = Governor::new(params());
        assert!(!g.step(10, 87.9, true), "just under hi");
        assert!(g.step(10, 88.0, true), "at hi: down");
        assert!(g.step(13, 83.0, true), "at lo: up");
        assert_eq!(g.cap(), 4_900_000);
        assert!(!g.step(20, 83.1, true), "just over lo");
    }

    #[test]
    fn never_below_the_minimum_or_above_the_maximum() {
        let mut g = Governor::new(params());
        for tick in (0..200).step_by(2) {
            g.step(tick, 99.0, true);
        }
        assert_eq!(g.cap(), params().min_khz, "the floor");
        for tick in (300..700).step_by(3) {
            g.step(tick, 50.0, true);
        }
        assert_eq!(g.cap(), params().max_khz, "back to the maximum");
        assert!(!g.capped());
        assert!(!g.step(1000, 50.0, true), "nothing to raise once there");
    }

    #[test]
    fn a_step_never_overshoots_the_floor_or_ceiling() {
        let p = Params { min_khz: 4_900_000, ..params() };
        let mut g = Governor::new(p);
        g.step(10, 99.0, true);
        assert_eq!(g.cap(), 4_900_000, "a 200 MHz step from 5.0 GHz stops at the 4.9 GHz floor");
        let p = Params { up_khz: 400_000, ..params() };
        let mut g = Governor::new(p);
        g.step(10, 99.0, true);
        g.step(13, 50.0, true);
        assert_eq!(g.cap(), 5_000_000, "a 400 MHz step up stops at the maximum");
    }

    #[test]
    fn not_managing_hands_the_full_clock_back_once() {
        let mut g = Governor::new(params());
        g.step(10, 99.0, true);
        assert!(g.capped());
        assert!(g.step(11, 99.0, false), "when nothing smart is on the cap is lifted at once");
        assert_eq!(g.cap(), params().max_khz);
        assert!(!g.step(12, 99.0, false), "and nothing is written again");
        let mut off = Governor::new(Params { enabled: false, ..params() });
        assert!(!off.step(1, 99.0, true), "a disabled governor never caps");
        assert!(!off.capped());
    }

    #[test]
    fn a_disabled_governor_lifts_a_cap_it_holds() {
        let mut g = Governor::new(params());
        g.step(10, 99.0, true);
        g.params.enabled = false;
        assert!(g.step(11, 99.0, true));
        assert!(!g.capped());
    }

    #[test]
    fn plug_or_resume_delays_the_next_change() {
        let mut g = Governor::new(params());
        g.resync(100);
        assert!(!g.step(101, 99.0, true), "too soon after a resync");
        assert!(g.step(102, 99.0, true), "two ticks later it may act");
        let mut g = Governor::new(params());
        g.step(10, 99.0, true);
        g.step(13, 50.0, true);
        g.resync(20);
        assert!(!g.step(22, 50.0, true), "going up waits three ticks after a resync");
        assert!(g.step(23, 50.0, true));
    }

    #[test]
    fn a_tick_counter_that_restarts_cannot_underflow() {
        let mut g = Governor::new(params());
        g.resync(1_000_000);
        assert!(!g.step(5, 99.0, true), "tick before the last change counts as no time passed");
    }

    #[test]
    fn while_capped_the_limit_is_reasserted_every_thirty_ticks() {
        let mut g = Governor::new(params());
        g.step(10, 90.0, true);
        assert!(g.step(30, 85.0, true), "tick 30 must re-assert a cap that something else may have raised");
        assert!(!g.step(31, 85.0, true), "other ticks must not");
        assert!(g.step(60, 85.0, true));
        let mut free = Governor::new(params());
        assert!(!free.step(30, 85.0, true), "no cap, nothing to re-assert");
    }

    #[test]
    fn release_forgets_the_cap() {
        let mut g = Governor::new(params());
        g.step(10, 99.0, true);
        g.release();
        assert_eq!(g.cap(), params().max_khz);
    }

    // ---- the writer -----------------------------------------------------------------------

    fn cpus(n: usize) -> TempDir {
        let root = TempDir::new("thermal");
        for i in 0..n {
            let d = root.path().join(format!("cpu{i}/cpufreq"));
            std::fs::create_dir_all(&d).expect("mkdir");
            std::fs::write(d.join("scaling_max_freq"), "5000000\n").expect("write");
            std::fs::write(d.join("cpuinfo_max_freq"), "5400000\n").expect("write");
        }
        // entries that are not CPUs
        std::fs::create_dir_all(root.path().join("cpufreq/policy0")).expect("mkdir");
        std::fs::create_dir_all(root.path().join("cpuidle")).expect("mkdir");
        std::fs::write(root.path().join("cpuidle/scaling_max_freq"), "1").expect("write");
        root
    }

    fn read(root: &TempDir, i: usize) -> String {
        std::fs::read_to_string(root.path().join(format!("cpu{i}/cpufreq/scaling_max_freq")))
            .expect("read")
            .trim()
            .to_owned()
    }

    #[test]
    fn the_writer_applies_to_every_cpu_and_uncap_restores_the_hardware_maximum() {
        let root = cpus(3);
        let w = CapWriter::new(root.path());
        assert!(w.apply(4_000_000).is_empty());
        for i in 0..3 {
            assert_eq!(read(&root, i), "4000000", "cpu{i}");
        }
        assert!(w.uncap().is_empty());
        for i in 0..3 {
            assert_eq!(read(&root, i), "5400000", "uncap restores the CPU's own maximum (cpu{i})");
        }
        assert_eq!(
            std::fs::read_to_string(root.path().join("cpuidle/scaling_max_freq")).expect("read"),
            "1",
            "only cpuN directories are touched"
        );
    }

    #[test]
    fn the_writer_skips_cpus_already_at_the_value() {
        use std::os::unix::fs::PermissionsExt;
        let root = cpus(2);
        let w = CapWriter::new(root.path());
        w.apply(4_000_000);
        // A read-only file that already holds the value is not written, so it is not a failure
        // (and not a pointless write to the kernel every tick).
        let file = root.path().join("cpu1/cpufreq/scaling_max_freq");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).expect("chmod");
        assert!(w.apply(4_000_000).is_empty());
    }

    #[test]
    fn a_cpu_that_cannot_be_written_does_not_stop_the_others() {
        use std::os::unix::fs::PermissionsExt;
        let root = cpus(3);
        let w = CapWriter::new(root.path());
        let bad = root.path().join("cpu1/cpufreq/scaling_max_freq");
        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o444)).expect("chmod");
        let failed = w.apply(4_000_000);
        let writable = std::fs::OpenOptions::new().write(true).open(&bad).is_ok();
        assert_eq!(
            failed.len(),
            usize::from(!writable),
            "one failure when the file really is read-only (root can write anyway)"
        );
        assert_eq!(read(&root, 0), "4000000");
        assert_eq!(read(&root, 2), "4000000");
    }

    #[test]
    fn a_missing_tree_is_harmless() {
        let w = CapWriter::new("/nonexistent/ts-decision-test");
        assert!(w.apply(1).is_empty() && w.uncap().is_empty());
        assert_eq!(w.cpu_dir(), Path::new("/nonexistent/ts-decision-test"));
        assert_eq!(CapWriter::system().cpu_dir(), Path::new(CPU_DIR));
    }

    #[test]
    fn uncap_ignores_a_cpu_without_a_hardware_maximum() {
        let root = cpus(2);
        std::fs::remove_file(root.path().join("cpu0/cpufreq/cpuinfo_max_freq")).expect("remove");
        std::fs::write(root.path().join("cpu1/cpufreq/cpuinfo_max_freq"), "\n").expect("write");
        let w = CapWriter::new(root.path());
        w.apply(3_000_000);
        assert!(w.uncap().is_empty());
        assert_eq!(read(&root, 0), "3000000");
        assert_eq!(read(&root, 1), "3000000", "an empty maximum is not written");
    }
}
