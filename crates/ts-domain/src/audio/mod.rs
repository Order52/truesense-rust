//! audio: the laptop's sound through PipeWire/WirePlumber (`pactl`, `wpctl`, `systemctl --user`),
//! ALSA's mixer (`amixer`, read-mostly) and AcerUltraX (`acerultrax-cli`): output switching between
//! the speakers and the headphones, "there is no sound" detection and repair, the microphone, the
//! headset jack, Purified Voice (RNNoise noise reduction on PipeWire, one filter chain per input)
//! and the DTS sound, plus the logic of the audio watcher.
//!
//! Acer's Purified Voice runs a proprietary Realtek model that cannot run on Linux. This builds an
//! open equivalent: one PipeWire filter chain per input (headset mic, mic array) with RNNoise
//! voice-activity gating. Modes: off, conference (gentle), personal (strict; can clip very soft
//! words).
//!
//! # Hard rule
//!
//! Nothing here ever writes the HDA codec (`/sys/class/sound/hwC*D*/reconfig`, `hda-verb`): on the
//! PH16-71 that deadlocks the kernel's audio until a hard reboot. [`Audio`] refuses to run any
//! command that mentions them, whatever the caller asks for; the tests prove it.
//!
//! # Design
//!
//! Every external program goes through the injected `hw::CommandRunner` (with a 30 s timeout per
//! call, so a hung tool cannot hang the service), every file through an injected path, every
//! pause through an injected [`Clock`], every notification through [`Osd`]; the tools' output is
//! read by the pure functions of [`parse`]. The runner returns what a tool printed, not its exit
//! status, so a failing `pactl` looks like an empty answer, which is how every caller here treats it;
//! only `acerultrax-cli` errors are about its status, see [`Audio::dts`].
#![forbid(unsafe_code)]

mod alsa;
mod dts;
mod mic;
mod output;
pub mod parse;
mod sound;
mod voice;
mod watch;

#[cfg(test)]
mod testing;

use std::fmt;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ts_foundation::config::Config;
use ts_foundation::fsx::{self, FsError};
use ts_foundation::userstate;
use ts_platform::hw::CommandRunner;

pub use alsa::Jack;
pub use dts::DtsScope;
pub use mic::MicState;
pub use output::{Output, OutputStatus};
pub use parse::{DTS_PROFILES, DtsDevice};
pub use sound::{DEFAULT_REPAIR_DELAYS, RepairOptions, RepairResult, SoundState};
pub use voice::{MicPolicy, VoiceDevice, VoiceMode, VoiceModes};
pub use watch::{DtsWatcher, MicWatcher};

/// The card of this laptop's sound hardware (Intel SOF HDA DSP) as PipeWire names its devices.
pub(crate) const CARD_MATCH: &str = "pci-0000_00_1f.3-platform-skl_hda_dsp_generic";
/// AcerUltraX's virtual sink. It stays the default so that DTS keeps working; it follows the
/// hardware output.
pub(crate) const FRONT_SINK: &str = "acerultrax_sink";
/// The most one `pactl`/`wpctl`/`amixer`/`systemctl` call may take.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// The most `acerultrax-cli` may take.
const DTS_TIMEOUT: Duration = Duration::from_secs(30);

/// Where time comes from, and how a pause is spent. Tests pass a clock that advances only when
/// something sleeps, so waits and timeouts are deterministic and instant.
pub trait Clock: Send + Sync {
    /// A monotonic reading.
    fn now(&self) -> Duration;
    /// Pauses for `d`; may return early when the service is asked to stop.
    fn sleep(&self, d: Duration);
}

/// The real clock. It can be tied to a stop flag so that a pause of 90 s (the login repair) never
/// delays the service's shutdown.
#[derive(Debug, Clone)]
pub struct SystemClock {
    start: Instant,
    stop: Option<Arc<AtomicBool>>,
}

impl SystemClock {
    /// A clock whose pauses always run to the end.
    pub fn new() -> Self {
        Self { start: Instant::now(), stop: None }
    }

    /// A clock whose pauses end early once `stop` is set.
    #[must_use]
    pub fn stoppable(mut self, stop: Arc<AtomicBool>) -> Self {
        self.stop = Some(stop);
        self
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.start.elapsed()
    }

    fn sleep(&self, d: Duration) {
        let slice = Duration::from_millis(50);
        let end = Instant::now() + d;
        loop {
            if self.stop.as_ref().is_some_and(|s| s.load(Ordering::Relaxed)) {
                return;
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            std::thread::sleep(left.min(slice));
        }
    }
}

/// While this value lives, the desktop shell's own popups are silenced (a profile flip makes
/// PipeWire recreate devices and the shell would announce every one).
pub struct Quiet(Option<Box<dyn FnOnce() + Send>>);

impl Quiet {
    /// Runs `end` when dropped.
    pub fn new(end: impl FnOnce() + Send + 'static) -> Self {
        Self(Some(Box::new(end)))
    }

    /// Does nothing when dropped (no notifications to silence).
    pub fn none() -> Self {
        Self(None)
    }
}

impl Drop for Quiet {
    fn drop(&mut self) {
        if let Some(end) = self.0.take() {
            end();
        }
    }
}

/// On-screen notifications; the OSD of the system layer implements it.
pub trait Osd: Send + Sync {
    /// Shows `text` with an icon name.
    fn show(&self, text: &str, icon: &str);
    /// Silences the shell's own popups until the returned guard is dropped.
    fn quiet(&self) -> Quiet;
}

/// The configuration, read on every use like the Python edition's `config.load()`: a change
/// applies without restarting the service.
pub type ConfigSource = Arc<dyn Fn() -> Config + Send + Sync>;
type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;
type Logger = Arc<dyn Fn(&str) + Send + Sync>;
/// Where `acerultrax-cli` is, evaluated on every use because AcerUltraX can be installed while the
/// audio service runs.
pub type DtsLocator = Arc<dyn Fn() -> Option<PathBuf> + Send + Sync>;

/// The files the audio code reads and writes, so that tests can point them at a temporary tree.
#[derive(Debug, Clone)]
pub struct Paths {
    /// `/proc/asound/cards`.
    pub proc_cards: PathBuf,
    /// The directory PipeWire reads its drop-in configuration from.
    pub conf_dir: PathBuf,
    /// The RNNoise LADSPA plugin; `None` when it is not installed.
    pub plugin: Option<PathBuf>,
    /// AcerUltraX's own config (its list of headphone EQs).
    pub dts_config: PathBuf,
    /// The lock that stops two output switches (two quick key presses) from interleaving.
    pub output_lock: PathBuf,
}

const PLUGIN_CANDIDATES: [&str; 3] = [
    "/usr/lib/ladspa/librnnoise_ladspa.so",
    "/usr/lib64/ladspa/librnnoise_ladspa.so",
    "/usr/lib/x86_64-linux-gnu/ladspa/librnnoise_ladspa.so",
];

impl Paths {
    /// The paths of a user's session. Without `HOME` the user directories point below
    /// `/nonexistent`, so a write fails loudly instead of landing in the working directory. The
    /// lock goes to `XDG_RUNTIME_DIR` when there is one (the Python edition used a predictable
    /// name in the world-writable `/tmp`, which another user could occupy).
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>, uid: u32, exists: &dyn Fn(&Path) -> bool) -> Paths {
        let home = env("HOME").map_or_else(|| PathBuf::from("/nonexistent"), PathBuf::from);
        let output_lock = env("XDG_RUNTIME_DIR")
            .map(|d| PathBuf::from(d).join("truesense-output.lock"))
            .unwrap_or_else(|| PathBuf::from(format!("/tmp/truesense-output-{uid}.lock")));
        Paths {
            proc_cards: PathBuf::from("/proc/asound/cards"),
            conf_dir: home.join(".config/pipewire/pipewire.conf.d"),
            plugin: PLUGIN_CANDIDATES.iter().map(PathBuf::from).find(|p| exists(p)),
            dts_config: home.join(".config/acerultrax/config.toml"),
            output_lock,
        }
    }

    /// The running process's session.
    pub fn system() -> Paths {
        Self::from_env(&fsx::env_nonempty, current_uid(), &|p| p.exists())
    }
}

/// The effective user id, read from the ownership of `/proc/self` (no `unsafe` needed).
fn current_uid() -> u32 {
    std::fs::metadata("/proc/self").map_or(0, |m| m.uid())
}

const DTS_CLI: &str = "acerultrax-cli";

/// Finds AcerUltraX's command: on `PATH` first, then where its installer puts it (the user's
/// `~/.local/bin`, the invoking user's when run with sudo, `/usr/local/bin`, `/usr/bin`).
pub fn locate_dts(
    path_env: Option<&str>,
    homes: &[PathBuf],
    usable: &dyn Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let on_path = path_env
        .into_iter()
        .flat_map(|p| p.split(':'))
        .filter(|d| !d.is_empty())
        .map(|d| Path::new(d).join(DTS_CLI));
    let in_homes = homes.iter().map(|h| h.join(".local/bin").join(DTS_CLI));
    let fixed = ["/usr/local/bin", "/usr/bin"].into_iter().map(|d| Path::new(d).join(DTS_CLI));
    on_path.chain(in_homes).chain(fixed).find(|p| usable(p))
}

/// The home directory of `user` from the text of `/etc/passwd`.
pub fn home_of(passwd: &str, user: &str) -> Option<PathBuf> {
    if user.is_empty() {
        return None;
    }
    passwd.lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        (fields.len() >= 6 && fields[0] == user && !fields[5].is_empty()).then(|| PathBuf::from(fields[5]))
    })
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The production [`DtsLocator`]. When run with sudo (installer, doctor) it also looks in the
/// invoking user's home, not only root's.
pub fn system_dts_locator() -> DtsLocator {
    Arc::new(|| {
        let mut homes: Vec<PathBuf> = fsx::env_nonempty("HOME").map(PathBuf::from).into_iter().collect();
        if let (Some(user), Ok(passwd)) =
            (fsx::env_nonempty("SUDO_USER"), std::fs::read_to_string("/etc/passwd"))
        {
            homes.extend(home_of(&passwd, &user));
        }
        locate_dts(fsx::env_nonempty("PATH").as_deref(), &homes, &is_executable)
    })
}

/// Why an audio operation failed. The wording is the Python edition's where it had one.
#[derive(Debug)]
pub enum AudioError {
    /// A value was refused before anything was run; the text says what is allowed.
    Invalid(String),
    /// PipeWire does not know the laptop's sound card.
    NoCard,
    /// The card is known by name but missing from the card list.
    CardNotListed(String),
    /// After switching the card profile the card is on another one.
    ProfileNotSwitched {
        /// The profile it is on.
        got: String,
        /// The output asked for.
        wanted: &'static str,
    },
    /// The microphone boost did not read back as set.
    BoostNotKept,
    /// There is no sink to switch to.
    NoSink,
    /// The RNNoise plugin is not installed.
    PluginMissing {
        /// Whether the speaker filter (not Purified Voice) asked.
        speaker: bool,
    },
    /// AcerUltraX is not installed.
    DtsNotInstalled,
    /// `acerultrax-cli` could not be run.
    Dts(String),
    /// A file could not be read, written or locked.
    Fs(FsError),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) | Self::Dts(message) => f.write_str(message),
            Self::NoCard => f.write_str("sound card not found"),
            Self::CardNotListed(card) => write!(f, "card {card} is not in the card list"),
            Self::ProfileNotSwitched { got, wanted } => write!(f, "profile readback {got} != {wanted}"),
            Self::BoostNotKept => f.write_str("boost readback mismatch"),
            Self::NoSink => f.write_str("no sink to switch to"),
            Self::PluginMissing { speaker: false } => {
                f.write_str("RNNoise LADSPA plugin missing (Arch: sudo pacman -S noise-suppression-for-voice)")
            }
            Self::PluginMissing { speaker: true } => f.write_str("RNNoise LADSPA plugin missing"),
            Self::DtsNotInstalled => f.write_str(
                "AcerUltraX is not installed (https://github.com/fawaz7/AcerUltraX); TrueSense only drives it, it does not bundle Acer's files",
            ),
            Self::Fs(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for AudioError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Fs(e) => Some(e),
            _ => None,
        }
    }
}

impl From<FsError> for AudioError {
    fn from(e: FsError) -> Self {
        Self::Fs(e)
    }
}

/// The sound system, as one value that carries its dependencies. It is `Sync`: the watcher uses one
/// from two threads.
pub struct Audio {
    runner: Arc<dyn CommandRunner>,
    config: ConfigSource,
    paths: Paths,
    osd: Option<Arc<dyn Osd>>,
    state: Option<Arc<userstate::Store>>,
    clock: Arc<dyn Clock>,
    env: EnvLookup,
    log: Logger,
    dts_locator: DtsLocator,
}

impl Audio {
    /// Audio over `runner` with the given configuration and files. No notifications, no memory of
    /// which DTS outputs were seen, the real clock, the real environment, the journal (standard
    /// output) as log and the real AcerUltraX lookup; change any of them with the `with_*` methods.
    pub fn new(runner: Arc<dyn CommandRunner>, config: ConfigSource, paths: Paths) -> Self {
        Self {
            runner,
            config,
            paths,
            osd: None,
            state: None,
            clock: Arc::new(SystemClock::new()),
            env: Arc::new(fsx::env_nonempty),
            log: Arc::new(|line| println!("{line}")),
            dts_locator: system_dts_locator(),
        }
    }

    /// Shows notifications through `osd`.
    #[must_use]
    pub fn with_osd(mut self, osd: Arc<dyn Osd>) -> Self {
        self.osd = Some(osd);
        self
    }

    /// Remembers which DTS outputs were handled in the user state `store`.
    #[must_use]
    pub fn with_state(mut self, store: Arc<userstate::Store>) -> Self {
        self.state = Some(store);
        self
    }

    /// Replaces the clock (tests, and the stoppable clock of the daemon).
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Replaces the environment lookup (only `TRUESENSE_OSD` is read from it).
    #[must_use]
    pub fn with_env(mut self, env: EnvLookup) -> Self {
        self.env = env;
        self
    }

    /// Replaces where log lines go.
    #[must_use]
    pub fn with_logger(mut self, log: Logger) -> Self {
        self.log = log;
        self
    }

    /// Replaces the AcerUltraX lookup.
    #[must_use]
    pub fn with_dts_locator(mut self, locator: DtsLocator) -> Self {
        self.dts_locator = locator;
        self
    }

    /// Whether the RNNoise LADSPA plugin (Purified Voice) is installed.
    pub fn has_rnnoise(&self) -> bool {
        self.paths.plugin.is_some()
    }

    /// Whether AcerUltraX is installed.
    pub fn has_dts(&self) -> bool {
        (self.dts_locator)().is_some()
    }

    fn cfg(&self) -> Config {
        (self.config)()
    }

    fn log(&self, line: &str) {
        (self.log)(line);
    }

    fn say(&self, text: &str, icon: &str) {
        if let Some(osd) = &self.osd {
            osd.show(text, icon);
        }
    }

    fn quiet(&self) -> Quiet {
        self.osd.as_ref().map_or_else(Quiet::none, |osd| osd.quiet())
    }

    /// Whether notifications go to the Noctalia shell, which wants its popup AFTER the (slow)
    /// switch instead of before it. `TRUESENSE_OSD` overrides the configured style.
    fn noctalia_style(&self) -> bool {
        let style =
            (self.env)("TRUESENSE_OSD").filter(|s| !s.is_empty()).unwrap_or_else(|| self.cfg().osd.style);
        style == "noctalia"
    }

    /// Runs a program under the call timeout, after refusing anything that would touch the codec.
    fn call(&self, program: &str, args: &[&str]) -> io::Result<Vec<u8>> {
        if let Some(why) = forbidden(program, args) {
            self.log(&format!("refused to run a codec command: {program}: {why}"));
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("refused: {why}")));
        }
        self.runner.output(program, args, CALL_TIMEOUT)
    }

    /// What a program printed; empty when it could not run (not installed, timed out).
    fn out(&self, program: &str, args: &[&str]) -> String {
        self.call(program, args).map(|o| String::from_utf8_lossy(&o).into_owned()).unwrap_or_default()
    }

    /// Runs a program whose output is not needed. A failure is not an error here: every caller
    /// verifies the effect by reading it back.
    fn run(&self, program: &str, args: &[&str]) {
        let _ = self.call(program, args);
    }
}

/// Whether the text names a codec device node, `hwC<digits>D<digits>` in any case.
fn mentions_codec_node(lower: &str) -> bool {
    lower.match_indices("hwc").any(|(at, _)| {
        let rest = &lower[at + 3..];
        let cards = rest.bytes().take_while(u8::is_ascii_digit).count();
        let after = &rest[cards..];
        cards > 0 && after.starts_with('d') && after[1..].bytes().next().is_some_and(|b| b.is_ascii_digit())
    })
}

/// Why a command must never run: it would touch the HDA codec (its `reconfig` file, a `hwC*D*`
/// node, `hda-verb`). On the PH16-71 that hangs the audio until a hard reboot.
fn forbidden(program: &str, args: &[&str]) -> Option<&'static str> {
    let base = program.rsplit('/').next().unwrap_or(program);
    if matches!(base, "hda-verb" | "hda-jack-retask") {
        return Some("it writes codec verbs");
    }
    std::iter::once(program)
        .chain(args.iter().copied())
        .any(|a| {
            let lower = a.to_lowercase();
            lower.contains("reconfig") || mentions_codec_node(&lower)
        })
        .then_some("the HDA codec is never written (reconfig / hwC*D* hang the audio until reboot)")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn the_codec_guard_refuses_whatever_is_asked() {
        let refused: [(&str, &[&str]); 12] = [
            ("hda-verb", &["/dev/snd/hwC0D0", "0x1", "SET_POWER_STATE", "0x0"]),
            ("/usr/bin/hda-verb", &[]),
            ("hda-jack-retask", &[]),
            ("tee", &["/sys/class/sound/hwC0D0/reconfig"]),
            ("sh", &["-c", "echo 1 > /sys/class/sound/hwC0D0/reconfig"]),
            ("cat", &["/sys/class/sound/hwC1D0/init_pin_configs"]),
            ("/usr/local/bin/hda-reconfig", &[]),
            ("echo", &["RECONFIG"]),
            ("echo", &["HWC12D3"]),
            ("pactl", &["set-card-profile", "x", "hwC0D0"]),
            ("amixer", &["-c0", "cset", "name=reConfig"]),
            ("acerultrax-cli", &["mode", "hwc0d0"]),
        ];
        for (program, args) in refused {
            assert!(forbidden(program, args).is_some(), "{program} {args:?} was allowed");
        }
        let allowed: [(&str, &[&str]); 6] = [
            ("pactl", &["list", "cards"]),
            ("amixer", &["-c0", "cget", "iface=CARD,name=Headphone Jack"]),
            ("systemctl", &["--user", "restart", "wireplumber.service"]),
            ("echo", &["hwC"]),
            ("echo", &["hwC0"]),
            ("echo", &["hwCxDy"]),
        ];
        for (program, args) in allowed {
            assert!(forbidden(program, args).is_none(), "{program} {args:?} was refused");
        }
    }

    #[test]
    fn the_system_clock_advances_and_stops_early() {
        let stop = Arc::new(AtomicBool::new(false));
        let clock = SystemClock::new().stoppable(stop.clone());
        let before = clock.now();
        clock.sleep(Duration::from_millis(60));
        assert!(clock.now() - before >= Duration::from_millis(50));
        stop.store(true, Ordering::Relaxed);
        let started = Instant::now();
        clock.sleep(Duration::from_secs(60));
        assert!(started.elapsed() < Duration::from_secs(5), "a stopped clock must not sleep out its pause");
        assert!(SystemClock::default().now() < Duration::from_secs(5));
    }

    #[test]
    fn quiet_runs_its_end_once_when_dropped() {
        let ended = Arc::new(AtomicBool::new(false));
        let e = ended.clone();
        let guard = Quiet::new(move || e.store(true, Ordering::SeqCst));
        assert!(!ended.load(Ordering::SeqCst));
        drop(guard);
        assert!(ended.load(Ordering::SeqCst));
        drop(Quiet::none());
    }

    #[test]
    fn paths_follow_the_environment() {
        let env = |k: &str| match k {
            "HOME" => Some("/home/x".to_owned()),
            "XDG_RUNTIME_DIR" => Some("/run/user/1000".to_owned()),
            _ => None,
        };
        let p = Paths::from_env(&env, 1000, &|p| p == Path::new("/usr/lib64/ladspa/librnnoise_ladspa.so"));
        assert_eq!(p.conf_dir, Path::new("/home/x/.config/pipewire/pipewire.conf.d"));
        assert_eq!(p.dts_config, Path::new("/home/x/.config/acerultrax/config.toml"));
        assert_eq!(p.plugin.as_deref(), Some(Path::new("/usr/lib64/ladspa/librnnoise_ladspa.so")));
        assert_eq!(p.output_lock, Path::new("/run/user/1000/truesense-output.lock"));
        assert_eq!(p.proc_cards, Path::new("/proc/asound/cards"));

        let bare = Paths::from_env(&|_| None, 1000, &|_| false);
        assert_eq!(bare.plugin, None);
        assert_eq!(bare.output_lock, Path::new("/tmp/truesense-output-1000.lock"));
        assert!(
            bare.conf_dir.starts_with("/nonexistent"),
            "no HOME: writes must fail, not land in the working directory"
        );
    }

    #[test]
    fn acerultrax_is_looked_for_in_the_installers_places() {
        let homes = [PathBuf::from("/home/me"), PathBuf::from("/home/sudoer")];
        let only = |want: &'static str| move |p: &Path| p == Path::new(want);
        assert_eq!(
            locate_dts(Some("/a::/opt/bin"), &homes, &only("/opt/bin/acerultrax-cli")),
            Some("/opt/bin/acerultrax-cli".into())
        );
        assert_eq!(
            locate_dts(Some("/a"), &homes, &only("/home/sudoer/.local/bin/acerultrax-cli")),
            Some("/home/sudoer/.local/bin/acerultrax-cli".into())
        );
        assert_eq!(
            locate_dts(None, &homes, &only("/usr/local/bin/acerultrax-cli")),
            Some("/usr/local/bin/acerultrax-cli".into())
        );
        assert_eq!(
            locate_dts(None, &[], &only("/usr/bin/acerultrax-cli")),
            Some("/usr/bin/acerultrax-cli".into())
        );
        assert_eq!(locate_dts(Some("/a"), &homes, &|_| false), None);
        // PATH wins over the fixed places.
        assert_eq!(locate_dts(Some("/p"), &homes, &|_| true), Some("/p/acerultrax-cli".into()));

        let passwd = "root:x:0:0:root:/root:/bin/bash\nme:x:1000:1000:Me,,,:/home/me:/usr/bin/fish\nbroken:x\nempty:x:1:1::: \n";
        assert_eq!(home_of(passwd, "me"), Some("/home/me".into()));
        assert_eq!(home_of(passwd, "root"), Some("/root".into()));
        for user in ["nobody", "", "broken", "empty", "me:x"] {
            assert_eq!(home_of(passwd, user), None, "{user:?}");
        }
    }

    #[test]
    fn error_messages_are_the_python_ones() {
        assert_eq!(AudioError::NoCard.to_string(), "sound card not found");
        assert_eq!(AudioError::BoostNotKept.to_string(), "boost readback mismatch");
        assert_eq!(AudioError::NoSink.to_string(), "no sink to switch to");
        assert_eq!(
            AudioError::ProfileNotSwitched { got: "off".into(), wanted: "speakers" }.to_string(),
            "profile readback off != speakers"
        );
        assert!(
            AudioError::PluginMissing { speaker: false }
                .to_string()
                .contains("pacman -S noise-suppression-for-voice")
        );
        assert_eq!(AudioError::PluginMissing { speaker: true }.to_string(), "RNNoise LADSPA plugin missing");
        assert!(
            AudioError::DtsNotInstalled
                .to_string()
                .starts_with("AcerUltraX is not installed (https://github.com/fawaz7/AcerUltraX)")
        );
    }
}
