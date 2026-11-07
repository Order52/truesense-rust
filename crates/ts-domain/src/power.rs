//! power: the CPU package power limits (Intel RAPL PL1 and PL2 through the powercap sysfs), the
//! NVIDIA GPU power limit (through `nvidia-smi`) and the risk rating of a PL1/PL2 pair.
//!
//! What protects the laptop here:
//!
//! * a pair is refused unless both limits are 25 to 120 W and PL2 is not below PL1 (the stock
//!   limit of the i7-13700HX is 80 W; nothing outside this range has ever been tested);
//! * the previous limits are read first, and a write is read back: if the hardware did not keep
//!   the values, or a write failed half way (which would leave PL1 and PL2 inconsistent), the
//!   previous limits are written back;
//! * nothing here runs by itself: the daemon decides when to call, and the risk bands
//!   ([`risk`]) are only advice that the decision layer turns into "needs force".
//!
//! Every effect is injected: the sysfs tree ([`Sysfs`]), the MSR reader ([`MsrReader`]) and the
//! program runner (`hw::CommandRunner`, for `nvidia-smi`), so tests never touch the machine.
#![forbid(unsafe_code)]

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use ts_foundation::fsx;
use ts_foundation::sync::locked;
use ts_platform::hw::CommandRunner;

/// The smallest PL1 or PL2 ever written, in watts.
pub(crate) const MIN_W: u32 = 25;
/// The largest PL1 or PL2 ever written, in watts. The 13700HX laptop's stock limit is 80 W.
pub(crate) const MAX_W: u32 = 120;

/// PL1 (sustained) range that is "safe": inside it the cooling keeps up (stock is 80 W).
pub(crate) const SAFE_PL1_W: (u32, u32) = (35, 80);
/// PL2 (burst) range that is "safe".
pub(crate) const SAFE_PL2_W: (u32, u32) = (35, 100);
/// Past this PL1 the cooling cannot keep up and the CPU throttles hard or overheats.
pub(crate) const DANGER_PL1_W: u32 = 100;
/// Past this PL2 the cooling cannot keep up.
pub(crate) const DANGER_PL2_W: u32 = 115;

/// The powercap zone of the CPU package. `constraint_0` is PL1 (long term), `constraint_1` is
/// PL2 (short term).
const RAPL_DIR: &str = "/sys/class/powercap/intel-rapl:0";
const PL1_FILE: &str = "constraint_0_power_limit_uw";
const PL2_FILE: &str = "constraint_1_power_limit_uw";
/// `MSR_PKG_POWER_LIMIT`: its bit 63 locks PL1/PL2 until the next reboot.
const MSR_PKG_POWER_LIMIT: u32 = 0x610;

/// Where the PCI devices are. Reading `vendor`, `class` and `power/runtime_status` of a device
/// does not wake it, which is how [`PowerLimits::info`] tells whether the NVIDIA card is asleep.
const PCI_DIR: &str = "/sys/bus/pci/devices";
/// The PCI vendor id of NVIDIA.
const NVIDIA_VENDOR: &str = "0x10de";

/// How long `nvidia-smi` may take when only reading: a wedged driver must not wedge the daemon.
const NVIDIA_READ_TIMEOUT: Duration = Duration::from_secs(5);
/// How long `nvidia-smi -pl` may take.
const NVIDIA_WRITE_TIMEOUT: Duration = Duration::from_secs(15);
/// The GPU power limit that `nvidia-smi` is asked for must be in this range; the driver then
/// enforces its own, narrower, minimum and maximum.
const NVIDIA_LIMIT_W: (f64, f64) = (1.0, 1000.0);
/// How far the read-back GPU limit may differ from the request (nvidia-smi prints two decimals).
const NVIDIA_TOLERANCE_W: f64 = 0.5;

/// How risky a setting is. The wording is the Python edition's: the CLI, the window and the
/// decision layer compare against these strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// Inside the range that is usually fine.
    Safe,
    /// Past the safety line: may be unstable or hot; needs confirmation.
    Caution,
    /// Past the danger line: refused unless the caller forces it.
    Danger,
}

impl Level {
    /// The lower-case name used in JSON and in the CLI.
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Safe => "safe",
            Level::Caution => "caution",
            Level::Danger => "danger",
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A rating and the words that explain it to the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Rating {
    /// How risky it is.
    pub level: Level,
    /// Why, in a sentence the user can read.
    pub message: String,
}

impl Rating {
    pub(crate) fn new(level: Level, message: impl Into<String>) -> Self {
        Self { level, message: message.into() }
    }
}

/// Rates a PL1/PL2 pair (in watts). The daemon refuses a [`Level::Danger`] pair unless the caller
/// says force. A value that is not a number is rated danger (the Python edition would have called
/// it safe, because every comparison with NaN is false), though [`PowerLimits::set_limits`]
/// refuses it long before.
pub fn risk(pl1: f64, pl2: f64) -> Rating {
    if !pl1.is_finite() || !pl2.is_finite() {
        return Rating::new(Level::Danger, "The limits are not numbers.");
    }
    if pl1 > f64::from(DANGER_PL1_W) || pl2 > f64::from(DANGER_PL2_W) {
        return Rating::new(
            Level::Danger,
            format!(
                "Beyond the danger line ({DANGER_PL1_W} W sustained / {DANGER_PL2_W} W burst): the cooling cannot keep up and the CPU will throttle hard or overheat."
            ),
        );
    }
    if pl1 > f64::from(SAFE_PL1_W.1) || pl2 > f64::from(SAFE_PL2_W.1) {
        return Rating::new(
            Level::Caution,
            format!(
                "Above stock ({} W sustained / {} W burst): hotter and louder, and it may throttle.",
                SAFE_PL1_W.1, SAFE_PL2_W.1
            ),
        );
    }
    if pl1 < f64::from(SAFE_PL1_W.0) || pl2 < f64::from(SAFE_PL2_W.0) {
        return Rating::new(Level::Caution, "Very low limits make the laptop feel slow.");
    }
    Rating::new(Level::Safe, "Inside the safe range.")
}

/// Access to kernel attribute files (sysfs), as absolute paths such as `/sys/class/...`. It is a
/// trait so that tests can play a firmware that clamps or ignores a write, and so that the
/// production code is pointed at a temporary tree. [`powermgr`](crate::powermgr) uses it too.
pub trait Sysfs: Send + Sync {
    /// The text of an attribute.
    fn read(&self, path: &Path) -> io::Result<String>;
    /// Overwrites an **existing** attribute. It never creates a file: a missing attribute means
    /// the kernel does not offer the feature, and that must stay an error.
    fn write(&self, path: &Path, value: &str) -> io::Result<()>;
    /// The names (not paths) in a directory, sorted; an error when it cannot be listed.
    fn list(&self, dir: &Path) -> io::Result<Vec<String>>;
}

/// The real file system below a root (`/` in production, a temporary tree in tests).
#[derive(Debug, Clone)]
pub struct HostSysfs {
    root: PathBuf,
}

/// Attribute files are a few bytes; this bound keeps a hostile tree from making the root daemon
/// allocate without limit.
const MAX_ATTRIBUTE: u64 = 64 * 1024;

impl HostSysfs {
    /// The tree below `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The real machine.
    pub fn system() -> Self {
        Self::new("/")
    }

    fn resolve(&self, absolute: &Path) -> PathBuf {
        self.root.join(absolute.strip_prefix("/").unwrap_or(absolute))
    }
}

impl Sysfs for HostSysfs {
    fn read(&self, path: &Path) -> io::Result<String> {
        let bytes = fsx::read_limited(&self.resolve(path), MAX_ATTRIBUTE).map_err(io::Error::other)?;
        String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    fn write(&self, path: &Path, value: &str) -> io::Result<()> {
        overwrite_existing(&self.resolve(path), value)
    }

    fn list(&self, dir: &Path) -> io::Result<Vec<String>> {
        let mut names: Vec<String> = fs::read_dir(self.resolve(dir))?
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        names.sort();
        Ok(names)
    }
}

/// Overwrites a file that must already exist. It truncates (a sysfs attribute ignores that, a
/// regular file in a test must not keep the tail of a longer old value) but never creates.
pub(crate) fn overwrite_existing(path: &Path, value: &str) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).truncate(true).open(path)?;
    file.write_all(value.as_bytes())
}

/// Reads one model-specific register of CPU 0. It is read-only on purpose: this module has no
/// business writing MSRs (the undervolt module owns the one write path).
pub trait MsrReader: Send + Sync {
    /// The register's value.
    fn read_msr(&self, register: u32) -> io::Result<u64>;
}

/// Why a power operation failed.
#[derive(Debug)]
pub enum PowerError {
    /// A value was refused before anything was written.
    Invalid(String),
    /// The RAPL limits could not be read, so there is nothing to go back to and nothing was written.
    Unreadable {
        /// The attribute that failed.
        path: PathBuf,
        /// Why.
        source: Option<io::Error>,
    },
    /// A RAPL write was refused (not root, or the kernel has no such file).
    WriteFailed {
        /// The system error.
        source: io::Error,
        /// Whether the previous limits are back in place.
        restored: bool,
    },
    /// The hardware did not keep the limits that were written.
    Readback {
        /// PL1 and PL2 found, in watts.
        got_w: (f64, f64),
        /// PL1 and PL2 asked for, in watts.
        wanted_w: (f64, f64),
        /// Whether the previous limits are back in place.
        restored: bool,
    },
    /// `nvidia-smi` could not be run, or did not take the GPU limit.
    Nvidia(String),
}

impl fmt::Display for PowerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) | Self::Nvidia(message) => f.write_str(message),
            Self::Unreadable { path, source: Some(e) } => {
                write!(f, "cannot write RAPL (needs root, or the daemon): read {}: {e}", path.display())
            }
            Self::Unreadable { path, source: None } => {
                write!(f, "cannot write RAPL (needs root, or the daemon): {} is not a number", path.display())
            }
            Self::WriteFailed { source, restored } => write!(
                f,
                "cannot write RAPL (needs root, or the daemon): {source}{}",
                restored_note(*restored)
            ),
            Self::Readback { got_w, wanted_w, restored } => write!(
                f,
                "readback ({}, {}) != requested ({}, {}){}",
                got_w.0,
                got_w.1,
                wanted_w.0,
                wanted_w.1,
                restored_note(*restored)
            ),
        }
    }
}

fn restored_note(restored: bool) -> &'static str {
    if restored { "; restored previous limits" } else { "; COULD NOT restore the previous limits" }
}

impl std::error::Error for PowerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unreadable { source: Some(e), .. } | Self::WriteFailed { source: e, .. } => Some(e),
            _ => None,
        }
    }
}

/// The GPU power limits in watts as `nvidia-smi` prints them. They stay text because a driver
/// may print `[N/A]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NvidiaLimits {
    /// The limit in force.
    pub current: String,
    /// The factory default.
    pub default: String,
    /// The lowest the driver accepts.
    pub min: String,
    /// The highest the driver accepts.
    pub max: String,
}

/// Whether the package power limits are locked until reboot. Serialised as the Python edition
/// did: a boolean, or a sentence saying why the register cannot be read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum MsrLock {
    /// Bit 63 of `MSR_PKG_POWER_LIMIT`.
    Locked(bool),
    /// The register could not be read.
    Unreadable(String),
}

/// What the daemon reports about the power limits. Field names are the Python dict keys; an
/// unreadable value is `null`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Info {
    /// PL1 (sustained) in watts.
    pub pl1_w: Option<f64>,
    /// PL2 (burst) in watts.
    pub pl2_w: Option<f64>,
    /// The `enabled` attribute of the RAPL zone.
    pub rapl_enabled: Option<String>,
    /// Whether the limits are locked.
    pub msr_lock_bit: MsrLock,
    /// The GPU limits, when `nvidia-smi` answers.
    pub nvidia_power_limit_w: Option<NvidiaLimits>,
}

/// PL1 and PL2 exactly as RAPL stores them, in microwatts. The daemon keeps these to remember
/// what the firmware chose and to put it back later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawLimits {
    /// PL1 in microwatts.
    pub pl1_uw: u64,
    /// PL2 in microwatts.
    pub pl2_uw: u64,
}

/// The verified result of [`PowerLimits::set_limits`]. Field names are the Python dict keys.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Applied {
    /// PL1 in watts, as read back.
    pub pl1_w: f64,
    /// PL2 in watts, as read back.
    pub pl2_w: f64,
}

/// The power-limit hardware behind its injected dependencies.
#[derive(Clone)]
pub struct PowerLimits {
    sys: Arc<dyn Sysfs>,
    msr: Option<Arc<dyn MsrReader>>,
    runner: Arc<dyn CommandRunner>,
    /// The GPU limits as last read from an awake card, shown while it sleeps.
    last_nvidia: Arc<Mutex<Option<NvidiaLimits>>>,
}

impl PowerLimits {
    /// Power limits over `sys`; `msr` (when the msr module may be read) gives the lock bit;
    /// `runner` runs `nvidia-smi`.
    pub fn new(sys: Arc<dyn Sysfs>, msr: Option<Arc<dyn MsrReader>>, runner: Arc<dyn CommandRunner>) -> Self {
        Self { sys, msr, runner, last_nvidia: Arc::new(Mutex::new(None)) }
    }

    fn attribute(name: &str) -> PathBuf {
        Path::new(RAPL_DIR).join(name)
    }

    fn read_uw(&self, name: &str) -> Result<u64, PowerError> {
        let path = Self::attribute(name);
        let text = self
            .sys
            .read(&path)
            .map_err(|e| PowerError::Unreadable { path: path.clone(), source: Some(e) })?;
        text.trim().parse().map_err(|_| PowerError::Unreadable { path, source: None })
    }

    /// The current limits. Every part is best effort: a missing RAPL zone, an unreadable MSR or
    /// no `nvidia-smi` leaves that part `null` rather than failing. A runtime-suspended NVIDIA
    /// card is never woken for this (see `nvidia_limits_without_waking`).
    pub fn info(&self) -> Info {
        let watts = |name: &str| self.read_uw(name).ok().map(|uw| uw as f64 / 1e6);
        let msr_lock_bit = self.msr.as_ref().and_then(|m| m.read_msr(MSR_PKG_POWER_LIMIT).ok()).map_or_else(
            || MsrLock::Unreadable("unreadable (needs root + msr module)".into()),
            |v| MsrLock::Locked(v >> 63 != 0),
        );
        Info {
            pl1_w: watts(PL1_FILE),
            pl2_w: watts(PL2_FILE),
            rapl_enabled: self.sys.read(&Self::attribute("enabled")).ok().map(|t| t.trim().to_owned()),
            msr_lock_bit,
            nvidia_power_limit_w: self.nvidia_limits_without_waking(),
        }
    }

    /// The first NVIDIA display controller's runtime power state: asleep (suspended or about to
    /// be), judged from files that do not wake it. A machine without such a card, or whose card
    /// has no runtime power management, is "not asleep". (The same rule as
    /// `ts_system::nvidia::gpu_power`, which this crate sits below and so cannot call.)
    fn gpu_asleep(&self) -> bool {
        let dir = Path::new(PCI_DIR);
        let Ok(names) = self.sys.list(dir) else { return false };
        for name in names {
            let device = dir.join(name);
            let read = |file: &str| self.sys.read(&device.join(file)).ok().map(|t| t.trim().to_owned());
            if read("vendor").as_deref() != Some(NVIDIA_VENDOR) {
                continue;
            }
            // The card's audio and USB functions share the vendor id.
            if !read("class").is_some_and(|c| c.starts_with("0x03")) {
                continue;
            }
            return matches!(read("power/runtime_status").as_deref(), Some("suspended" | "suspending"));
        }
        false
    }

    /// The GPU power limits for [`info`](Self::info), which clients poll: every `nvidia-smi`
    /// start wakes a runtime-suspended card and initialises NVML (which stalls every CPU core on
    /// this laptop), so a sleeping card is not asked. Its limits do not change while it sleeps,
    /// so the last answer from an awake card is shown instead (`None` before there was one).
    fn nvidia_limits_without_waking(&self) -> Option<NvidiaLimits> {
        let mut last = locked(&self.last_nvidia);
        if self.gpu_asleep() {
            return last.clone();
        }
        let fresh = self.nvidia_limits();
        if fresh.is_some() {
            last.clone_from(&fresh);
        }
        fresh
    }

    fn nvidia_limits(&self) -> Option<NvidiaLimits> {
        let out = self
            .runner
            .output(
                "nvidia-smi",
                &[
                    "--query-gpu=power.limit,power.default_limit,power.min_limit,power.max_limit",
                    "--format=csv,noheader,nounits",
                ],
                NVIDIA_READ_TIMEOUT,
            )
            .ok()?;
        let text = String::from_utf8_lossy(&out);
        // The first GPU: a hybrid laptop has exactly one NVIDIA GPU.
        let line = text.trim().lines().next()?;
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        let [current, default, min, max, ..] = fields.as_slice() else { return None };
        Some(NvidiaLimits {
            current: (*current).to_owned(),
            default: (*default).to_owned(),
            min: (*min).to_owned(),
            max: (*max).to_owned(),
        })
    }

    /// PL1 and PL2 in microwatts, or an error when RAPL cannot be read.
    pub fn read_raw(&self) -> Result<RawLimits, PowerError> {
        Ok(RawLimits { pl1_uw: self.read_uw(PL1_FILE)?, pl2_uw: self.read_uw(PL2_FILE)? })
    }

    /// Writes both limits (microwatts) with no range check and no read-back. It exists to put back
    /// values the firmware itself chose; anything a user asked for goes through
    /// [`set_limits`](Self::set_limits).
    pub fn write_raw(&self, limits: RawLimits) -> Result<(), PowerError> {
        for (name, value) in [(PL1_FILE, limits.pl1_uw), (PL2_FILE, limits.pl2_uw)] {
            self.sys
                .write(&Self::attribute(name), &value.to_string())
                .map_err(|source| PowerError::WriteFailed { source, restored: true })?;
        }
        Ok(())
    }

    /// Sets PL1 (sustained) and PL2 (burst) in watts. Both must be [`MIN_W`]..=[`MAX_W`] with PL2 at
    /// least PL1. The values are read back; if the hardware did not keep them, or a write failed
    /// half way, the previous limits are put back and an error says so.
    ///
    /// Safety: this is the only path from a user's number to the RAPL files.
    pub fn set_limits(&self, pl1: f64, pl2: f64) -> Result<Applied, PowerError> {
        let (min, max) = (f64::from(MIN_W), f64::from(MAX_W));
        // Comparisons with NaN are false, so NaN falls through to the refusal.
        if !(min <= pl1 && pl1 <= max && min <= pl2 && pl2 <= max && pl2 >= pl1) {
            return Err(PowerError::Invalid(format!("PL1/PL2 must be {MIN_W}..{MAX_W} W with PL2 >= PL1")));
        }
        // Without the current limits there would be nothing to go back to, so do not start.
        let old = self.read_raw()?;
        let wanted = RawLimits { pl1_uw: watts_to_uw(pl1), pl2_uw: watts_to_uw(pl2) };
        if let Err(e) = self.write_raw(wanted) {
            // A half-written pair would leave PL1 and PL2 inconsistent.
            let restored = self.restore(old);
            return Err(match e {
                PowerError::WriteFailed { source, .. } => PowerError::WriteFailed { source, restored },
                other => other,
            });
        }
        let got = match self.read_raw() {
            Ok(got) => got,
            Err(e) => {
                self.restore(old);
                return Err(e);
            }
        };
        if got != wanted {
            let restored = self.restore(old);
            return Err(PowerError::Readback {
                got_w: (got.pl1_uw as f64 / 1e6, got.pl2_uw as f64 / 1e6),
                wanted_w: (pl1, pl2),
                restored,
            });
        }
        Ok(Applied { pl1_w: got.pl1_uw as f64 / 1e6, pl2_w: got.pl2_uw as f64 / 1e6 })
    }

    /// Puts `old` back and says whether the hardware now really holds it (a write that is accepted
    /// is not proof: a firmware may clamp it).
    fn restore(&self, old: RawLimits) -> bool {
        self.write_raw(old).is_ok() && self.read_raw().is_ok_and(|now| now == old)
    }

    /// Sets the GPU power limit in watts through `nvidia-smi -pl` (the driver enforces its own
    /// minimum and maximum) and returns what `nvidia-smi` printed.
    ///
    /// The runner only gives back what the program printed, not its exit status, so success is
    /// decided by reading the limit back (which is also what "writes are read back" asks for).
    pub fn set_nvidia_limit(&self, watts: f64) -> Result<String, PowerError> {
        if !(NVIDIA_LIMIT_W.0 <= watts && watts <= NVIDIA_LIMIT_W.1) {
            return Err(PowerError::Invalid("GPU power limit must be 1..1000 W".into()));
        }
        let whole = watts as i64; // like Python's int(): the driver takes whole watts here
        let printed = self
            .runner
            .output("nvidia-smi", &["-pl", &whole.to_string()], NVIDIA_WRITE_TIMEOUT)
            .map_err(|e| PowerError::Nvidia(format!("run nvidia-smi: {e}")))?;
        let printed = String::from_utf8_lossy(&printed).trim().to_owned();
        let limit = self.nvidia_limits().and_then(|l| l.current.parse::<f64>().ok());
        match limit {
            Some(now) if (now - whole as f64).abs() <= NVIDIA_TOLERANCE_W => {
                Ok(if printed.is_empty() { format!("GPU power limit set to {whole} W") } else { printed })
            }
            _ => Err(PowerError::Nvidia(if printed.is_empty() {
                format!("the GPU did not take a power limit of {whole} W")
            } else {
                printed
            })),
        }
    }
}

/// Watts to the microwatts RAPL stores. Rounded, not truncated like Python's `int()`: 70.3 W is
/// 70300000, not 70299999 from a float artefact, so the read-back compares equal.
fn watts_to_uw(watts: f64) -> u64 {
    (watts * 1e6).round() as u64
}

/// Fakes shared by the tests of this module and of [`powermgr`](crate::powermgr).
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
pub(crate) mod fake {
    use std::collections::BTreeMap;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::Duration;

    use ts_platform::hw::CommandRunner;

    use super::Sysfs;

    /// What a fake attribute does with a write.
    pub(crate) enum OnWrite {
        /// Keeps the value, as a normal attribute does.
        Keep,
        /// Accepts the write and changes nothing (a firmware that ignores it).
        Ignore,
        /// Keeps this text instead (a firmware that clamps).
        Clamp(String),
        /// Refuses the write.
        Fail,
    }

    type WriteHook = Box<dyn Fn(&Path, &str) -> OnWrite + Send>;

    /// A sysfs tree in memory.
    #[derive(Default)]
    pub(crate) struct MemSysfs {
        files: Mutex<BTreeMap<PathBuf, String>>,
        hook: Mutex<Option<WriteHook>>,
        pub(crate) writes: Mutex<Vec<(PathBuf, String)>>,
    }

    impl MemSysfs {
        pub(crate) fn with(files: &[(&str, &str)]) -> Self {
            let fs = Self::default();
            for (path, text) in files {
                fs.put(path, text);
            }
            fs
        }

        pub(crate) fn put(&self, path: &str, text: &str) {
            self.files.lock().unwrap().insert(PathBuf::from(path), text.to_owned());
        }

        pub(crate) fn get(&self, path: &str) -> Option<String> {
            self.files.lock().unwrap().get(Path::new(path)).cloned()
        }

        pub(crate) fn on_write(&self, hook: impl Fn(&Path, &str) -> OnWrite + Send + 'static) {
            *self.hook.lock().unwrap() = Some(Box::new(hook));
        }
    }

    impl Sysfs for MemSysfs {
        fn read(&self, path: &Path) -> io::Result<String> {
            self.files
                .lock()
                .unwrap()
                .get(path)
                .cloned()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }

        fn write(&self, path: &Path, value: &str) -> io::Result<()> {
            let mut files = self.files.lock().unwrap();
            if !files.contains_key(path) {
                return Err(io::Error::from(io::ErrorKind::NotFound));
            }
            let behaviour = self.hook.lock().unwrap().as_ref().map_or(OnWrite::Keep, |h| h(path, value));
            self.writes.lock().unwrap().push((path.to_path_buf(), value.to_owned()));
            match behaviour {
                OnWrite::Keep => {
                    files.insert(path.to_path_buf(), value.to_owned());
                }
                OnWrite::Ignore => {}
                OnWrite::Clamp(text) => {
                    files.insert(path.to_path_buf(), text);
                }
                OnWrite::Fail => return Err(io::Error::from(io::ErrorKind::PermissionDenied)),
            }
            Ok(())
        }

        fn list(&self, dir: &Path) -> io::Result<Vec<String>> {
            let files = self.files.lock().unwrap();
            let mut names: Vec<String> = files
                .keys()
                .filter_map(|p| p.strip_prefix(dir).ok())
                .filter_map(|rest| rest.components().next())
                .filter_map(|c| c.as_os_str().to_str().map(str::to_owned))
                .collect();
            names.sort();
            names.dedup();
            if names.is_empty() { Err(io::Error::from(io::ErrorKind::NotFound)) } else { Ok(names) }
        }
    }

    type Handler = Box<dyn Fn(&str, &[String]) -> io::Result<Vec<u8>> + Send + Sync>;

    /// A program runner that answers from a closure and records every call.
    pub(crate) struct FakeRunner {
        handler: Handler,
        pub(crate) calls: Mutex<Vec<Vec<String>>>,
    }

    impl FakeRunner {
        pub(crate) fn new(
            handler: impl Fn(&str, &[String]) -> io::Result<Vec<u8>> + Send + Sync + 'static,
        ) -> Self {
            Self { handler: Box::new(handler), calls: Mutex::new(Vec::new()) }
        }

        /// Every call as one line, program first.
        pub(crate) fn lines(&self) -> Vec<String> {
            self.calls.lock().unwrap().iter().map(|c| c.join(" ")).collect()
        }
    }

    impl CommandRunner for FakeRunner {
        fn output(&self, program: &str, args: &[&str], _timeout: Duration) -> io::Result<Vec<u8>> {
            let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
            let mut call = vec![program.to_owned()];
            call.extend(args.iter().cloned());
            self.calls.lock().unwrap().push(call);
            (self.handler)(program, &args)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::io;
    use std::sync::Arc;

    use super::fake::{FakeRunner, MemSysfs, OnWrite};
    use super::*;

    const PL1: &str = "/sys/class/powercap/intel-rapl:0/constraint_0_power_limit_uw";
    const PL2: &str = "/sys/class/powercap/intel-rapl:0/constraint_1_power_limit_uw";
    const ENABLED: &str = "/sys/class/powercap/intel-rapl:0/enabled";

    fn tree(uw: &str) -> Arc<MemSysfs> {
        Arc::new(MemSysfs::with(&[(PL1, uw), (PL2, uw), (ENABLED, "1\n")]))
    }

    fn no_smi() -> Arc<FakeRunner> {
        Arc::new(FakeRunner::new(|_, _| Err(io::Error::from(io::ErrorKind::NotFound))))
    }

    fn smi(out: &'static str) -> Arc<FakeRunner> {
        Arc::new(FakeRunner::new(move |_, _| Ok(out.as_bytes().to_vec())))
    }

    struct Msr(io::Result<u64>);
    impl MsrReader for Msr {
        fn read_msr(&self, register: u32) -> io::Result<u64> {
            assert_eq!(register, 0x610, "only MSR_PKG_POWER_LIMIT is read");
            match &self.0 {
                Ok(v) => Ok(*v),
                Err(e) => Err(io::Error::new(e.kind(), "msr")),
            }
        }
    }

    // Python: test_core cpu.pl_risk(65, 80) safe, (90, 100) caution, (110, 115) danger.
    #[test]
    fn risk_bands() {
        let table: [(f64, f64, Level); 14] = [
            (65.0, 80.0, Level::Safe),
            (90.0, 100.0, Level::Caution),
            (110.0, 115.0, Level::Danger),
            (80.0, 100.0, Level::Safe),
            (80.5, 100.0, Level::Caution),
            (80.0, 100.5, Level::Caution),
            (100.0, 115.0, Level::Caution),
            (100.5, 115.0, Level::Danger),
            (100.0, 115.5, Level::Danger),
            (35.0, 35.0, Level::Safe),
            (34.0, 80.0, Level::Caution),
            (60.0, 34.0, Level::Caution),
            (f64::NAN, 80.0, Level::Danger),
            (80.0, f64::INFINITY, Level::Danger),
        ];
        for (pl1, pl2, want) in table {
            assert_eq!(risk(pl1, pl2).level, want, "risk({pl1}, {pl2})");
        }
    }

    #[test]
    fn risk_messages_are_the_python_ones() {
        assert_eq!(risk(65.0, 80.0).message, "Inside the safe range.");
        assert_eq!(
            risk(90.0, 100.0).message,
            "Above stock (80 W sustained / 100 W burst): hotter and louder, and it may throttle."
        );
        assert_eq!(risk(20.0, 80.0).message, "Very low limits make the laptop feel slow.");
        assert_eq!(
            risk(110.0, 115.0).message,
            "Beyond the danger line (100 W sustained / 115 W burst): the cooling cannot keep up and the CPU will throttle hard or overheat."
        );
        assert_eq!(Level::Danger.to_string(), "danger");
        assert_eq!(serde_json::to_string(&Level::Caution).unwrap(), "\"caution\"");
    }

    #[test]
    fn info_reads_everything() {
        let p = PowerLimits::new(
            tree("80000000"),
            Some(Arc::new(Msr(Ok(1 << 63)))),
            smi("95.00, 80.00, 35.00, 115.00\n"),
        );
        let info = p.info();
        assert_eq!((info.pl1_w, info.pl2_w), (Some(80.0), Some(80.0)));
        assert_eq!(info.rapl_enabled.as_deref(), Some("1"));
        assert_eq!(info.msr_lock_bit, MsrLock::Locked(true));
        assert_eq!(
            info.nvidia_power_limit_w,
            Some(NvidiaLimits {
                current: "95.00".into(),
                default: "80.00".into(),
                min: "35.00".into(),
                max: "115.00".into()
            })
        );
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["pl1_w"], 80.0);
        assert_eq!(json["msr_lock_bit"], true);
        assert_eq!(json["nvidia_power_limit_w"]["min"], "35.00");
    }

    #[test]
    fn info_degrades_without_failing() {
        let empty = Arc::new(MemSysfs::default());
        let info = PowerLimits::new(empty, None, no_smi()).info();
        assert_eq!(
            (info.pl1_w, info.pl2_w, &info.rapl_enabled, &info.nvidia_power_limit_w),
            (None, None, &None, &None)
        );
        assert_eq!(info.msr_lock_bit, MsrLock::Unreadable("unreadable (needs root + msr module)".into()));
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["msr_lock_bit"], "unreadable (needs root + msr module)");
        assert!(json["pl1_w"].is_null());

        let unlocked = PowerLimits::new(tree("1"), Some(Arc::new(Msr(Ok(1)))), no_smi()).info();
        assert_eq!(unlocked.msr_lock_bit, MsrLock::Locked(false));
        let failing = PowerLimits::new(
            tree("1"),
            Some(Arc::new(Msr(Err(io::ErrorKind::PermissionDenied.into())))),
            no_smi(),
        );
        assert!(matches!(failing.info().msr_lock_bit, MsrLock::Unreadable(_)));
    }

    #[test]
    fn info_survives_hostile_values() {
        let hostile = ["", "abc", "-5", "1e6", "99999999999999999999999", "8 0"];
        for text in hostile {
            let info = PowerLimits::new(tree(text), None, no_smi()).info();
            assert_eq!((info.pl1_w, info.pl2_w), (None, None), "{text:?}");
        }
        for out in ["", "garbage", "1,2,3", "\u{0}\u{1},,,", "[N/A], [N/A], [N/A], [N/A]"] {
            let limits = PowerLimits::new(tree("1"), None, smi(out)).info().nvidia_power_limit_w;
            if out.starts_with('[') {
                assert_eq!(limits.unwrap().current, "[N/A]");
            } else {
                assert!(limits.is_none() || out.starts_with('\u{0}'), "{out:?}");
            }
        }
    }

    const SMI_LINE: &str = "80.00, 80.00, 35.00, 80.00\n";

    fn card(fs: &MemSysfs, status: &str) {
        fs.put("/sys/bus/pci/devices/0000:00:02.0/vendor", "0x8086\n");
        fs.put("/sys/bus/pci/devices/0000:00:02.0/class", "0x030000\n");
        fs.put("/sys/bus/pci/devices/0000:01:00.0/vendor", "0x10de\n");
        fs.put("/sys/bus/pci/devices/0000:01:00.0/class", "0x030200\n");
        fs.put("/sys/bus/pci/devices/0000:01:00.0/power/runtime_status", &format!("{status}\n"));
        // the card's audio function shares the vendor id and may be in another state
        fs.put("/sys/bus/pci/devices/0000:01:00.1/vendor", "0x10de\n");
        fs.put("/sys/bus/pci/devices/0000:01:00.1/class", "0x040300\n");
        fs.put("/sys/bus/pci/devices/0000:01:00.1/power/runtime_status", "active\n");
    }

    #[test]
    fn a_sleeping_gpu_is_not_woken_to_read_its_power_limit() {
        let fs = tree("80000000");
        card(&fs, "suspended");
        let runner = smi(SMI_LINE);
        let p = PowerLimits::new(fs.clone(), None, runner.clone());
        assert_eq!(p.info().nvidia_power_limit_w, None, "asleep and never read: nothing to show");
        assert!(runner.lines().is_empty(), "nvidia-smi was not started: {:?}", runner.lines());
        // Awake: asked, and the answer is remembered.
        fs.put("/sys/bus/pci/devices/0000:01:00.0/power/runtime_status", "active\n");
        let awake = p.info().nvidia_power_limit_w.unwrap();
        assert_eq!((awake.current.as_str(), awake.max.as_str()), ("80.00", "80.00"));
        assert_eq!(runner.lines().len(), 1);
        // Asleep again: the remembered limits are shown, nothing is asked.
        fs.put("/sys/bus/pci/devices/0000:01:00.0/power/runtime_status", "suspending\n");
        assert_eq!(p.info().nvidia_power_limit_w.as_ref(), Some(&awake));
        assert_eq!(runner.lines().len(), 1, "no second question");
        // Clones share what was learned (the daemon hands out clones).
        assert_eq!(p.clone().info().nvidia_power_limit_w.as_ref(), Some(&awake));
    }

    #[test]
    fn a_machine_without_runtime_status_or_without_a_gpu_is_asked_as_before() {
        let fs = tree("80000000");
        let runner = smi(SMI_LINE);
        let p = PowerLimits::new(fs.clone(), None, runner.clone());
        assert!(p.info().nvidia_power_limit_w.is_some(), "no PCI tree at all: ask");
        card(&fs, "unsupported");
        assert!(p.info().nvidia_power_limit_w.is_some(), "unknown power state: ask");
        // Only the audio function sleeps: that is not the card.
        fs.put("/sys/bus/pci/devices/0000:01:00.0/power/runtime_status", "active\n");
        fs.put("/sys/bus/pci/devices/0000:01:00.1/power/runtime_status", "suspended\n");
        assert!(p.info().nvidia_power_limit_w.is_some());
        assert_eq!(runner.lines().len(), 3);
    }

    #[test]
    fn setting_the_gpu_limit_still_asks_the_card() {
        let fs = tree("80000000");
        card(&fs, "suspended");
        let runner = smi("65.00, 80.00, 35.00, 80.00\n");
        let p = PowerLimits::new(fs, None, runner.clone());
        // An explicit request wakes the card on purpose: the read-back is part of the write.
        assert!(p.set_nvidia_limit(65.0).is_ok());
        assert!(runner.lines().iter().any(|l| l == "nvidia-smi -pl 65"));
    }

    #[test]
    fn set_limits_writes_and_verifies() {
        let fs = tree("95000000");
        let p = PowerLimits::new(fs.clone(), None, no_smi());
        assert_eq!(p.set_limits(70.0, 90.0).unwrap(), Applied { pl1_w: 70.0, pl2_w: 90.0 });
        assert_eq!(fs.get(PL1).as_deref(), Some("70000000"));
        assert_eq!(fs.get(PL2).as_deref(), Some("90000000"));
        // A fractional wattage survives float artefacts.
        assert_eq!(p.set_limits(70.3, 70.3).unwrap().pl1_w, 70.3);
        // The edges of the range are allowed.
        p.set_limits(25.0, 120.0).unwrap();
        p.set_limits(120.0, 120.0).unwrap();
        assert_eq!(p.read_raw().unwrap(), RawLimits { pl1_uw: 120_000_000, pl2_uw: 120_000_000 });
    }

    #[test]
    fn set_limits_range_table_writes_nothing_when_refused() {
        let fs = tree("80000000");
        let p = PowerLimits::new(fs.clone(), None, no_smi());
        let refused = [
            (24.9, 80.0),
            (80.0, 79.9),
            (130.0, 130.0),
            (60.0, 121.0),
            (0.0, 0.0),
            (-5.0, 80.0),
            (f64::NAN, 80.0),
            (80.0, f64::NAN),
            (f64::INFINITY, f64::INFINITY),
            (f64::NEG_INFINITY, 80.0),
        ];
        for (pl1, pl2) in refused {
            let err = p.set_limits(pl1, pl2).unwrap_err();
            assert!(matches!(err, PowerError::Invalid(_)), "({pl1}, {pl2}): {err}");
            assert_eq!(err.to_string(), "PL1/PL2 must be 25..120 W with PL2 >= PL1");
        }
        assert!(fs.writes.lock().unwrap().is_empty(), "a refused request still wrote");
    }

    #[test]
    fn set_limits_restores_when_the_hardware_clamps() {
        let fs = tree("80000000");
        fs.on_write(|path, _| {
            if path.ends_with("constraint_1_power_limit_uw") {
                OnWrite::Clamp("65000000".into())
            } else {
                OnWrite::Keep
            }
        });
        let p = PowerLimits::new(fs.clone(), None, no_smi());
        let err = p.set_limits(70.0, 90.0).unwrap_err();
        // The firmware clamps every PL2 write, the restore included: the error must not claim
        // the old limits are back.
        assert!(matches!(err, PowerError::Readback { restored: false, .. }), "{err}");
        assert!(err.to_string().contains("COULD NOT restore"));
        assert_eq!(fs.get(PL1).as_deref(), Some("80000000"));
    }

    #[test]
    fn set_limits_restores_when_only_the_new_value_is_refused() {
        let fs = tree("80000000");
        fs.on_write(
            |_, value| if value == "90000000" { OnWrite::Clamp("65000000".into()) } else { OnWrite::Keep },
        );
        let err = PowerLimits::new(fs.clone(), None, no_smi()).set_limits(70.0, 90.0).unwrap_err();
        assert!(matches!(err, PowerError::Readback { restored: true, .. }), "{err}");
        assert!(err.to_string().ends_with("; restored previous limits"));
        assert_eq!((fs.get(PL1).as_deref(), fs.get(PL2).as_deref()), (Some("80000000"), Some("80000000")));
    }

    #[test]
    fn set_limits_restores_a_half_written_pair() {
        let fs = tree("80000000");
        let first = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let f = first.clone();
        fs.on_write(move |path, _| {
            let is_pl2 = path.ends_with("constraint_1_power_limit_uw");
            if is_pl2 && f.swap(false, std::sync::atomic::Ordering::SeqCst) {
                OnWrite::Fail
            } else {
                OnWrite::Keep
            }
        });
        let p = PowerLimits::new(fs.clone(), None, no_smi());
        let err = p.set_limits(70.0, 90.0).unwrap_err();
        assert!(matches!(err, PowerError::WriteFailed { restored: true, .. }), "{err}");
        assert!(err.to_string().starts_with("cannot write RAPL (needs root, or the daemon)"));
        assert_eq!((fs.get(PL1).as_deref(), fs.get(PL2).as_deref()), (Some("80000000"), Some("80000000")));
    }

    #[test]
    fn set_limits_reports_when_the_restore_also_fails() {
        let fs = tree("80000000");
        fs.on_write(|_, _| OnWrite::Fail);
        let err = PowerLimits::new(fs, None, no_smi()).set_limits(70.0, 90.0).unwrap_err();
        assert!(matches!(err, PowerError::WriteFailed { restored: false, .. }));
        assert!(err.to_string().contains("COULD NOT restore"));
    }

    #[test]
    fn set_limits_ignored_write_is_a_readback_error() {
        let fs = tree("80000000");
        fs.on_write(|_, _| OnWrite::Ignore);
        let err = PowerLimits::new(fs, None, no_smi()).set_limits(70.0, 90.0).unwrap_err();
        assert!(matches!(
            err,
            PowerError::Readback { got_w: (80.0, 80.0), wanted_w: (70.0, 90.0), restored: true }
        ));
    }

    #[test]
    fn set_limits_refuses_to_start_when_the_current_limits_are_unreadable() {
        for text in ["", "junk"] {
            let fs = tree(text);
            let err = PowerLimits::new(fs.clone(), None, no_smi()).set_limits(70.0, 90.0).unwrap_err();
            assert!(matches!(err, PowerError::Unreadable { .. }), "{err}");
            assert!(fs.writes.lock().unwrap().is_empty());
        }
        let none = Arc::new(MemSysfs::default());
        let err = PowerLimits::new(none, None, no_smi()).set_limits(70.0, 90.0).unwrap_err();
        assert!(err.to_string().contains("needs root, or the daemon"), "{err}");
        assert!(std::error::Error::source(&err).is_some());
    }

    #[test]
    fn raw_round_trip() {
        let fs = tree("80000000");
        let p = PowerLimits::new(fs, None, no_smi());
        let raw = RawLimits { pl1_uw: 95_000_000, pl2_uw: 135_000_000 };
        p.write_raw(raw).unwrap(); // no range check: this puts back what the firmware chose
        assert_eq!(p.read_raw().unwrap(), raw);
    }

    #[test]
    fn nvidia_limit_is_set_and_read_back() {
        let state = Arc::new(std::sync::Mutex::new(95u32));
        let s = state.clone();
        let runner = Arc::new(FakeRunner::new(move |_, args| {
            if args.first().map(String::as_str) == Some("-pl") {
                *s.lock().unwrap() = args[1].parse().unwrap();
                return Ok(
                    b"Power limit for GPU 00000000:01:00.0 was set to 80.00 W from 95.00 W.\n".to_vec()
                );
            }
            let now = *s.lock().unwrap();
            Ok(format!("{now}.00, 80.00, 35.00, 115.00\n").into_bytes())
        }));
        let p = PowerLimits::new(tree("1"), None, runner.clone());
        let out = p.set_nvidia_limit(80.7).unwrap();
        assert!(out.starts_with("Power limit for GPU"), "{out}");
        assert!(runner.lines().contains(&"nvidia-smi -pl 80".to_owned()), "{:?}", runner.lines());
    }

    #[test]
    fn nvidia_limit_failures() {
        let p = PowerLimits::new(tree("1"), None, smi("Setting the power limit is not supported.\n"));
        let err = p.set_nvidia_limit(80.0).unwrap_err();
        assert_eq!(err.to_string(), "Setting the power limit is not supported.");
        let p = PowerLimits::new(tree("1"), None, no_smi());
        assert!(p.set_nvidia_limit(80.0).unwrap_err().to_string().starts_with("run nvidia-smi:"));
        let runner = smi("");
        let p = PowerLimits::new(tree("1"), None, runner.clone());
        for w in [0.0, 0.5, 1001.0, f64::NAN, -3.0] {
            assert!(matches!(p.set_nvidia_limit(w), Err(PowerError::Invalid(_))), "{w}");
        }
        assert!(runner.lines().is_empty(), "a refused value never reaches nvidia-smi");
        // A driver that accepts silently but reports another limit is an error, not a success.
        let p = PowerLimits::new(tree("1"), None, smi("95.00, 80.00, 35.00, 115.00\n"));
        assert!(p.set_nvidia_limit(60.0).is_err());
    }

    #[test]
    fn host_sysfs_reads_writes_and_lists_below_its_root() {
        let dir = std::env::temp_dir().join(format!("ts-domain-power-{}", std::process::id()));
        let attr = dir.join("sys/class/powercap/intel-rapl:0");
        std::fs::create_dir_all(&attr).unwrap();
        std::fs::write(attr.join("constraint_0_power_limit_uw"), "95000000\n").unwrap();
        let fs = HostSysfs::new(&dir);
        let path = Path::new(PL1);
        assert_eq!(fs.read(path).unwrap(), "95000000\n");
        fs.write(path, "7").unwrap(); // a shorter value must not keep the tail of the longer one
        assert_eq!(fs.read(path).unwrap(), "7");
        assert_eq!(
            fs.list(Path::new("/sys/class/powercap/intel-rapl:0")).unwrap(),
            ["constraint_0_power_limit_uw"]
        );
        let missing = fs.write(Path::new("/sys/class/powercap/intel-rapl:0/nope"), "1").unwrap_err();
        assert_eq!(missing.kind(), io::ErrorKind::NotFound, "a write never creates an attribute");
        assert!(fs.list(Path::new("/nowhere")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
