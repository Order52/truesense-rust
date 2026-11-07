//! The GPU helper protocol: the small C program `truesense-nvml` talks to NVIDIA's management
//! library (NVML) and this module talks to the program.
//!
//! Why a helper and not NVML directly: the daemon is a static program that cannot load the
//! driver's library, and it must not try, because a call that hangs inside the NVIDIA driver
//! would block the root service. The helper does exactly one NVML call per run for the GPU at
//! index 0, prints one line and exits 0; the daemon runs it with a timeout through an injected
//! `CommandRunner`, so a driver that hangs becomes an error instead of a hung service.
//!
//! The protocol (see `tools/nvml/truesense-nvml.c`; it must match byte for byte):
//!
//! | arguments | answer |
//! |---|---|
//! | `offset` | `ok OFFSET_MHZ` |
//! | `range` | `ok MIN_MHZ MAX_MHZ` |
//! | `set-offset MHZ` | `ok` |
//! | `lock MIN_MHZ MAX_MHZ` | `ok` |
//! | `reset-clocks` | `ok` |
//!
//! Errors are `err code N` (an NVML return code), `err library` (no NVIDIA driver library),
//! `err init`, `err nodevice` and `err usage`. The answer comes from another program, so every
//! shape is checked and none can panic.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ts_platform::hw::CommandRunner;

/// The helper program's file name.
pub const HELPER_NAME: &str = "truesense-nvml";
/// Where the installer puts the helper.
pub const HELPER_PATH: &str = "/usr/lib/truesense/truesense-nvml";
/// How long one helper run may take. NVML answers in milliseconds; a driver that does not answer
/// within this time is reported as an error.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// Longest piece of a helper answer quoted in an error message.
const QUOTE_LIMIT: usize = 120;

/// Why talking to the GPU failed. The wording is the Python edition's where it had one; the user
/// sees these.
#[derive(Debug)]
pub enum NvmlError {
    /// NVML returned this code (`nvmlReturn_t`, never 0).
    Code(i32),
    /// There is no NVIDIA driver library on this machine.
    LibraryNotFound,
    /// NVML could not be initialised.
    Init,
    /// There is no GPU at index 0.
    NoDevice,
    /// The helper program is not installed.
    HelperMissing,
    /// The driver did not answer in time.
    Timeout,
    /// The helper could not be run, or crashed.
    HelperFailed(io::Error),
    /// The helper printed nothing.
    NoAnswer,
    /// The helper's answer had an unexpected shape.
    BadReply(String),
}

impl NvmlError {
    /// Whether the driver said the GPU or driver does not support the call (code 3). Lifting a
    /// clock cap that was never possible is then not a failure.
    pub fn is_not_supported(&self) -> bool {
        matches!(self, Self::Code(3))
    }
}

impl fmt::Display for NvmlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Code(2) => f.write_str("invalid argument"),
            Self::Code(3) => f.write_str("not supported by this GPU or driver"),
            Self::Code(4) => f.write_str("needs root (the TrueSense service)"),
            Self::Code(9) => f.write_str("driver not loaded"),
            Self::Code(15) => f.write_str("GPU is lost"),
            Self::Code(999) => f.write_str("unknown error"),
            Self::Code(n) => write!(f, "NVML error {n}"),
            Self::LibraryNotFound => f.write_str("NVIDIA driver library (libnvidia-ml) not found"),
            Self::Init => f.write_str("could not start NVML (is the NVIDIA driver loaded?)"),
            Self::NoDevice => f.write_str("no NVIDIA GPU found"),
            Self::HelperMissing => write!(
                f,
                "the GPU helper ({HELPER_NAME}) is not installed: run the TrueSense installer again (GPU undervolt needs it)"
            ),
            Self::Timeout => write!(f, "the NVIDIA driver did not answer within {}s", CALL_TIMEOUT.as_secs()),
            Self::HelperFailed(e) => write!(f, "the GPU helper failed: {e}"),
            Self::NoAnswer => f.write_str("the GPU helper gave no answer"),
            Self::BadReply(line) => write!(f, "the GPU helper gave an unreadable answer: {line:?}"),
        }
    }
}

impl std::error::Error for NvmlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::HelperFailed(e) => Some(e),
            _ => None,
        }
    }
}

/// The small slice of NVML this module needs, for the GPU at index 0. It is a trait so that
/// everything above it is tested against a fake; the real implementation is [`HelperNvml`].
pub trait Nvml: Send + Sync {
    /// The graphics-clock curve offset in MHz.
    fn offset(&self) -> Result<i32, NvmlError>;
    /// The offsets the driver allows (min, max MHz).
    fn offset_range(&self) -> Result<(i32, i32), NvmlError>;
    /// Shifts the clock/voltage curve by `mhz`.
    fn set_offset(&self, mhz: i32) -> Result<(), NvmlError>;
    /// Limits the graphics clock to `min_mhz..=max_mhz`.
    fn lock_clocks(&self, min_mhz: u32, max_mhz: u32) -> Result<(), NvmlError>;
    /// Lifts the clock limit.
    fn reset_clocks(&self) -> Result<(), NvmlError>;
}

/// Where the helper is, evaluated on every use because the installer can add it while the
/// daemon runs.
pub type Locator = Arc<dyn Fn() -> Result<PathBuf, NvmlError> + Send + Sync>;

/// [`Nvml`] by running the helper once per call.
pub struct HelperNvml {
    runner: Arc<dyn CommandRunner>,
    locate: Locator,
}

impl HelperNvml {
    /// A helper client that finds the program with `locate` and runs it with `runner`.
    pub fn new(runner: Arc<dyn CommandRunner>, locate: Locator) -> Self {
        Self { runner, locate }
    }

    /// The production client: the helper is looked for in [`helper_candidates`].
    pub fn system(runner: Arc<dyn CommandRunner>) -> Self {
        let locate: Locator = Arc::new(|| {
            let env = std::env::var("TRUESENSE_NVML").ok().filter(|v| !v.is_empty());
            let exe = std::env::current_exe().ok().map(|e| e.canonicalize().unwrap_or(e));
            find_helper(&helper_candidates(env.as_deref(), exe.as_deref()), &is_executable_file)
        });
        Self::new(runner, locate)
    }

    fn call(&self, args: &[&str]) -> Result<Vec<i64>, NvmlError> {
        let path = (self.locate)()?;
        let program = path.to_str().ok_or(NvmlError::HelperMissing)?;
        match self.runner.output(program, args, CALL_TIMEOUT) {
            Ok(out) => parse_reply(&String::from_utf8_lossy(&out)),
            Err(e) if e.kind() == io::ErrorKind::TimedOut => Err(NvmlError::Timeout),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(NvmlError::HelperMissing),
            Err(e) => Err(NvmlError::HelperFailed(e)),
        }
    }
}

fn exactly<const N: usize>(numbers: Vec<i64>) -> Result<[i32; N], NvmlError> {
    let narrowed: Option<Vec<i32>> = numbers.iter().map(|n| i32::try_from(*n).ok()).collect();
    narrowed
        .and_then(|v| <[i32; N]>::try_from(v).ok())
        .ok_or_else(|| NvmlError::BadReply(format!("unexpected numbers: {numbers:?}")))
}

impl Nvml for HelperNvml {
    fn offset(&self) -> Result<i32, NvmlError> {
        exactly::<1>(self.call(&["offset"])?).map(|[v]| v)
    }

    fn offset_range(&self) -> Result<(i32, i32), NvmlError> {
        exactly::<2>(self.call(&["range"])?).map(|[lo, hi]| (lo, hi))
    }

    fn set_offset(&self, mhz: i32) -> Result<(), NvmlError> {
        self.call(&["set-offset", &mhz.to_string()]).map(drop)
    }

    fn lock_clocks(&self, min_mhz: u32, max_mhz: u32) -> Result<(), NvmlError> {
        self.call(&["lock", &min_mhz.to_string(), &max_mhz.to_string()]).map(drop)
    }

    fn reset_clocks(&self) -> Result<(), NvmlError> {
        self.call(&["reset-clocks"]).map(drop)
    }
}

/// The places the helper is looked for, best first: `TRUESENSE_NVML` when set, `../lib/truesense`
/// next to the running program, then the installed locations.
pub fn helper_candidates(env_override: Option<&str>, exe: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(p) = env_override.filter(|p| !p.is_empty()) {
        out.push(PathBuf::from(p));
    }
    if let Some(dir) = exe.and_then(Path::parent) {
        out.push(dir.join("..").join("lib").join("truesense").join(HELPER_NAME));
    }
    out.push(PathBuf::from("/usr/local/lib/truesense").join(HELPER_NAME));
    out.push(PathBuf::from(HELPER_PATH));
    out
}

/// The first candidate that `usable` accepts, or [`NvmlError::HelperMissing`].
pub fn find_helper(candidates: &[PathBuf], usable: &dyn Fn(&Path) -> bool) -> Result<PathBuf, NvmlError> {
    candidates.iter().find(|c| usable(c)).cloned().ok_or(NvmlError::HelperMissing)
}

/// Whether `path` is a regular file with an execute bit.
pub fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

fn quote(line: &str) -> String {
    line.chars().take(QUOTE_LIMIT).collect()
}

/// Reads the helper's answer: `ok [numbers]` or `err <what>`. Anything else, a number that is not
/// a number, a missing answer, an unknown error word: a typed error, never a panic.
pub fn parse_reply(out: &str) -> Result<Vec<i64>, NvmlError> {
    let line = out.trim();
    let mut words = line.split_whitespace();
    match words.next() {
        None => Err(NvmlError::NoAnswer),
        Some("ok") => words.map(|w| w.parse::<i64>().map_err(|_| NvmlError::BadReply(quote(line)))).collect(),
        Some("err") => Err(helper_error(words.collect::<Vec<_>>().as_slice(), line)),
        Some(_) => Err(NvmlError::BadReply(quote(line))),
    }
}

fn helper_error(words: &[&str], line: &str) -> NvmlError {
    match words {
        ["code", n] => n.parse::<i32>().map_or_else(|_| NvmlError::BadReply(quote(line)), NvmlError::Code),
        ["library"] => NvmlError::LibraryNotFound,
        ["init"] => NvmlError::Init,
        ["nodevice"] => NvmlError::NoDevice,
        ["usage"] => NvmlError::Code(2),
        _ => NvmlError::BadReply(quote(line)),
    }
}
