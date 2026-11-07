//! selftest: `truesense selftest` sets, reads back and restores every firmware feature, then runs
//! the doctor. It prints PASS / FAIL / SKIP per item and the run fails on any FAIL.
//!
//! Safety is the point of the design: every item that changes something puts the original back
//! even when the change, its read-back or a panic got in the way, and the fans always end on
//! firmware Auto. While it runs the daemon's smart fan and automatic profile are paused (and put
//! back afterwards), so nothing fights the test.
//!
//! The firmware is a [`Hw`]; the daemon, the keyboard controller and the sound system belong to
//! layers this crate cannot see, so they come in through the small traits [`Daemon`], [`Keyboard`]
//! and [`Audio`], which the assembly layer implements. Time comes from a [`Clock`], so the 22
//! second fan watch takes no time in a test.
#![forbid(unsafe_code)]

use std::fmt;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use ts_platform::hw::{Coded, FanDuty, FanMode, Hw, HwError, Reading, UsbLevel};

pub use crate::doctor::CallError;

/// How long the fans are watched after the single manual write.
const FAN_WATCH: Duration = Duration::from_secs(22);
/// The first part of the watch is the spin-up allowance: no sample is taken.
const FAN_SPIN_UP: Duration = Duration::from_secs(8);
/// The manual duty the fan item holds. 60 % is well inside the 20..100 the firmware takes.
const FAN_HOLD_DUTY: i64 = 60;
/// Far above the idle speed of either fan, far below what 60 % gives.
const FAN_MIN_RPM: i64 = 2000;
/// How many samples must show both fans above [`FAN_MIN_RPM`].
const FAN_MIN_PERCENT: usize = 85;
/// How long the daemon is given to notice that its automation was paused.
const PAUSE_SETTLE: Duration = Duration::from_millis(2500);

/// The daemon's two automations, which the self-test pauses so that nothing fights it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Automation {
    /// The smart fan curve.
    pub smart_fan: bool,
    /// The automatic power profile.
    pub auto_profile: bool,
}

/// What the daemon reports about the CPU power limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuInfo {
    /// Sustained limit in watts.
    pub pl1_w: i64,
    /// Burst limit in watts.
    pub pl2_w: i64,
    /// The limits were set by the user, not automatic.
    #[serde(default)]
    pub user: bool,
}

/// The control socket as the self-test uses it. The assembly layer adapts the `ts-ipc` client.
/// Without a daemon (`None` in [`Deps`]) nothing is paused.
pub trait Daemon: Send + Sync {
    /// Reads the smart fan and automatic profile switches.
    fn automation(&self) -> Result<Automation, CallError>;
    /// Sets both switches.
    fn set_automation(&self, automation: Automation) -> Result<(), CallError>;
    /// The `cpu_info` command: a daemon that does not know it is older than this program, which
    /// the error must say as `unknown command` (see [`CallError::new`]).
    fn cpu_info(&self) -> Result<CpuInfo, CallError>;
}

/// The keyboard backlight, read from the keyboard controller itself.
pub trait Keyboard: Send + Sync {
    /// The brightness, 0..=50.
    fn brightness(&self) -> Result<i64, CallError>;
}

/// The sound system as the self-test uses it. The audio module of the domain layer is not visible
/// from here; the assembly layer adapts its controller.
pub trait Audio: Send + Sync {
    /// The active output profile: `speakers` or `headphones`.
    fn output_active(&self) -> Result<String, CallError>;
    /// The headset microphone boost (0..=3); `None` when the card has no such control.
    fn headset_boost(&self) -> Result<Option<u8>, CallError>;
    /// Sets the boost and returns what was read back, so the caller can tell a change that did
    /// not take.
    fn set_headset_boost(&self, level: u8) -> Result<u8, CallError>;
    /// Whether the default microphone is muted.
    fn mic_muted(&self) -> Result<bool, CallError>;
    /// Mutes or unmutes every input.
    fn set_mic_mute(&self, muted: bool) -> Result<(), CallError>;
}

/// Time as the self-test uses it, so that tests need not wait.
pub trait Clock: Send + Sync {
    /// A monotonic time since some fixed start.
    fn now(&self) -> Duration;
    /// Waits.
    fn sleep(&self, duration: Duration);
}

/// The real clock.
#[derive(Debug, Clone, Copy)]
pub struct SystemClock {
    start: Instant,
}

impl SystemClock {
    /// A clock that starts now.
    pub fn new() -> Self {
        Self { start: Instant::now() }
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

    fn sleep(&self, duration: Duration) {
        thread::sleep(duration);
    }
}

/// Never cancelled: the default of [`Deps::cancelled`].
fn never() -> bool {
    false
}

/// Runs the doctor, writes its report to the writer and returns its exit code.
pub type DoctorRun<'a> = &'a dyn Fn(&mut dyn Write) -> i32;

/// Everything the self-test touches besides the firmware.
pub struct Deps<'a> {
    /// The daemon; `None` means no daemon is running.
    pub daemon: Option<&'a dyn Daemon>,
    /// The keyboard controller.
    pub keyboard: Option<&'a dyn Keyboard>,
    /// The sound system.
    pub audio: Option<&'a dyn Audio>,
    /// The health check; `None` skips the stack health item.
    pub doctor: Option<DoctorRun<'a>>,
    /// Time.
    pub clock: &'a dyn Clock,
    /// Asked during the fan watch: `true` (Ctrl-C) ends the watch early, still handing the fans
    /// back to the firmware.
    pub cancelled: &'a dyn Fn() -> bool,
}

impl<'a> Deps<'a> {
    /// Nothing connected but the clock; fill in the fields that exist.
    pub fn new(clock: &'a dyn Clock) -> Self {
        Self { daemon: None, keyboard: None, audio: None, doctor: None, clock, cancelled: &never }
    }
}

/// What to skip.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// Do not touch the fans (`--skip-fans`).
    pub skip_fans: bool,
}

/// The outcome of one item. A skip is not counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Outcome {
    /// It worked.
    #[serde(rename = "PASS")]
    Pass,
    /// It did not.
    #[serde(rename = "FAIL")]
    Fail,
    /// It was not run.
    #[serde(rename = "SKIP")]
    Skip,
}

/// One line of the run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Item {
    /// What was tested.
    pub name: String,
    /// How it went.
    pub outcome: Outcome,
    /// What happened, or why it failed or was skipped.
    pub detail: String,
}

/// The whole run.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Report {
    /// Every item in order, skips included.
    pub items: Vec<Item>,
    /// How many passed.
    pub passed: usize,
    /// How many were counted (everything but skips).
    pub total: usize,
}

impl Report {
    /// Nothing failed.
    pub fn ok(&self) -> bool {
        self.passed == self.total
    }
}

/// Why an item failed. `Display` is the line after `FAIL  name: `.
#[derive(Debug)]
enum Failure {
    /// The firmware.
    Hw(HwError),
    /// The daemon, keyboard or sound system.
    Call(CallError),
    /// What was found was not what a working laptop gives.
    Wrong(String),
    /// The item failed and so did putting the setting back (or two parts of one item failed).
    Both(Box<Failure>, Box<Failure>),
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hw(e) => e.fmt(f),
            Self::Call(e) => e.fmt(f),
            Self::Wrong(text) => f.write_str(text),
            Self::Both(a, b) => write!(f, "{a}; {b}"),
        }
    }
}

impl std::error::Error for Failure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Hw(e) => Some(e),
            Self::Call(e) => Some(e),
            Self::Wrong(_) | Self::Both(..) => None,
        }
    }
}

impl From<HwError> for Failure {
    fn from(e: HwError) -> Self {
        Self::Hw(e)
    }
}

impl From<CallError> for Failure {
    fn from(e: CallError) -> Self {
        Self::Call(e)
    }
}

fn wrong<T>(text: impl Into<String>) -> Result<T, Failure> {
    Err(Failure::Wrong(text.into()))
}

/// The text of a panic.
fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned())
}

/// Runs `body`, then ALWAYS `restore`, and reports both failures if both failed. A panic in the
/// body is a failure of the item, never the end of the run, and the restore still happens: this
/// is what puts a setting back whatever went wrong in the middle.
fn guarded<T>(
    body: impl FnOnce() -> Result<T, Failure>,
    restore: impl FnOnce() -> Result<(), Failure>,
) -> Result<T, Failure> {
    let result = catch_unwind(AssertUnwindSafe(body))
        .unwrap_or_else(|p| Err(Failure::Wrong(format!("panic: {}", panic_text(p.as_ref())))));
    match (result, restore()) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(restore)) => Err(restore),
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(restore)) => Err(Failure::Both(Box::new(error), Box::new(restore))),
    }
}

/// Changes a setting, checks that it reads back, and puts the original back, checking that too.
fn toggle<T: Copy + PartialEq + fmt::Display>(
    get: impl Fn() -> Result<T, Failure>,
    set: impl Fn(T) -> Result<(), Failure>,
    alternative: impl Fn(T) -> T,
) -> Result<String, Failure> {
    let original = get()?;
    guarded(
        || {
            let next = alternative(original);
            set(next)?;
            if get()? != next {
                return wrong("readback mismatch");
            }
            Ok(format!("{original} -> {next} -> restored"))
        },
        || {
            set(original).map_err(|e| Failure::Wrong(format!("RESTORE FAILED: {e}")))?;
            match get() {
                Ok(back) if back == original => Ok(()),
                Ok(back) => wrong(format!("RESTORE FAILED: reads {back}, wanted {original}")),
                Err(e) => wrong(format!("RESTORE FAILED: cannot read it back: {e}")),
            }
        },
    )
}

struct Runner<'a, 'w> {
    hw: &'a Hw,
    deps: &'a Deps<'a>,
    out: &'w mut dyn Write,
    report: Report,
}

/// Does the whole self-test, printing to `out` as it goes. Every setting it touches is back to
/// what it was when this returns, whatever happened in between (if that failed, the report says
/// `RESTORE FAILED`).
pub fn run(hw: &Hw, deps: &Deps<'_>, options: Options, out: &mut dyn Write) -> Report {
    let mut r = Runner { hw, deps, out, report: Report::default() };
    r.say("== firmware features (set -> read back -> restore)");
    let paused = r.pause_automation();
    r.check("power modes (all supported)", |r| r.modes());
    r.check("sensors", |r| r.sensors());
    if options.skip_fans {
        r.skip("fans", "--skip-fans");
    } else {
        r.check("fans: manual 60 % holds, then back to auto", |r| r.fans());
    }
    r.resume_automation(paused);

    r.check("LCD overdrive", |r| {
        toggle(
            || r.hw.get_overdrive()?.ok_or_else(|| Failure::Wrong("overdrive unknown".to_owned())),
            |on| Ok(r.hw.set_overdrive(on).map(drop)?),
            |on| !on,
        )
    });
    r.check("boot animation/sound", |r| {
        toggle(|| Ok(r.hw.get_boot_sound()?), |on| Ok(r.hw.set_boot_sound(on).map(drop)?), |on| !on)
    });
    r.check("battery 80 % limit", |r| {
        toggle(|| Ok(r.hw.get_battery_limit()?), |on| Ok(r.hw.set_battery_limit(on).map(drop)?), |on| !on)
    });
    r.check("battery calibration flag", |r| {
        toggle(|| Ok(r.hw.get_calibration()?), |on| Ok(r.hw.set_calibration(on).map(drop)?), |on| !on)
    });
    r.check("USB charging level", |r| {
        toggle(
            || match r.hw.get_usb_charge()? {
                Reading::Known(level) => Ok(level),
                unknown @ Reading::Unknown(_) => wrong(format!("USB charging level is {unknown}")),
            },
            |level| Ok(r.hw.set_usb_charge(level).map(drop)?),
            |level| if level == UsbLevel::Percent10 { UsbLevel::Percent20 } else { UsbLevel::Percent10 },
        )
    });
    r.check("keyboard backlight timeout", |r| {
        toggle(
            || {
                r.hw.get_kb_timeout()?
                    .ok_or_else(|| Failure::Wrong("keyboard backlight timeout unknown".to_owned()))
            },
            |on| Ok(r.hw.set_kb_timeout(on).map(drop)?),
            |on| !on,
        )
    });
    r.check("GPU mode (read)", |r| Ok(r.hw.get_gpu_mode()?.to_string()));
    r.check("rear light (read)", |r| {
        let rear = r.hw.rear_get()?;
        serde_json::to_string(&rear).map_err(|e| Failure::Wrong(e.to_string()))
    });
    r.check("keyboard lighting interface", |r| r.keyboard());
    r.check("audio output profile", |r| r.audio_output());
    r.check("headset mic boost", |r| r.headset_boost());
    r.check("microphone mute", |r| r.mic_mute());
    r.check("CPU power limits (read)", |r| r.cpu_limits());

    r.say("\n== stack health (doctor)");
    match r.deps.doctor {
        None => r.skip("stack health (doctor)", "no health check wired in"),
        Some(doctor) => {
            let mut buf = Vec::new();
            let code = doctor(&mut buf);
            r.say(String::from_utf8_lossy(&buf).trim_end_matches('\n'));
            r.record(
                "stack health (doctor)",
                if code == 0 { Outcome::Pass } else { Outcome::Fail },
                String::new(),
            );
        }
    }
    r.say(&format!("\n{}/{} passed", r.report.passed, r.report.total));
    r.report
}

impl Runner<'_, '_> {
    /// Prints a line of the run. A closed output must not stop a self-test that is restoring
    /// settings, so write errors are ignored.
    fn say(&mut self, line: &str) {
        let _ = writeln!(self.out, "{line}");
    }

    fn record(&mut self, name: &str, outcome: Outcome, detail: String) {
        if outcome != Outcome::Skip {
            self.report.total += 1;
            self.report.passed += usize::from(outcome == Outcome::Pass);
        }
        self.report.items.push(Item { name: name.to_owned(), outcome, detail });
    }

    /// Runs one item. A panic in it is a FAIL of that item, never the end of the run: the
    /// restores inside the item have already run by then.
    fn check(&mut self, name: &str, item: impl FnOnce(&Self) -> Result<String, Failure>) {
        let result = catch_unwind(AssertUnwindSafe(|| item(self)))
            .unwrap_or_else(|p| Err(Failure::Wrong(format!("panic: {}", panic_text(p.as_ref())))));
        match &result {
            Ok(detail) if detail.is_empty() => self.say(&format!("PASS  {name}")),
            Ok(detail) => self.say(&format!("PASS  {name}  ({detail})")),
            Err(e) => self.say(&format!("FAIL  {name}: {e}")),
        }
        match result {
            Ok(detail) => self.record(name, Outcome::Pass, detail),
            Err(e) => self.record(name, Outcome::Fail, e.to_string()),
        }
    }

    fn skip(&mut self, name: &str, why: &str) {
        self.say(&format!("SKIP  {name}: {why}"));
        self.record(name, Outcome::Skip, why.to_owned());
    }

    /// Stops the daemon's smart fan and automatic profile and returns what to put back. The
    /// original is known BEFORE the first change, so a failure half way through pausing still
    /// restores. `None` when there is no daemon to pause.
    fn pause_automation(&mut self) -> Option<Automation> {
        let daemon = self.deps.daemon?;
        let original = daemon.automation().ok()?; // no daemon answering: nothing to pause
        if daemon.set_automation(Automation { smart_fan: false, auto_profile: false }).is_err() {
            self.resume_automation(Some(original));
            return None;
        }
        self.deps.clock.sleep(PAUSE_SETTLE);
        Some(original)
    }

    fn resume_automation(&mut self, original: Option<Automation>) {
        let (Some(original), Some(daemon)) = (original, self.deps.daemon) else { return };
        if let Err(e) = daemon.set_automation(original) {
            self.say(&format!("FAIL  restoring the daemon's smart fan / auto profile: {e}"));
            self.record("restore the daemon's automation", Outcome::Fail, e.to_string());
        }
    }

    /// Cycles every supported power mode and puts the original back.
    fn modes(&self) -> Result<String, Failure> {
        let original = match self.hw.get_mode()? {
            Reading::Known(mode) => mode,
            // A mode this app has no name for cannot be set again, so it must not be left.
            unknown @ Reading::Unknown(_) => {
                return wrong(format!(
                    "the power mode is {unknown}, which cannot be restored: not changing it"
                ));
            }
        };
        guarded(
            || {
                let mut seen = Vec::new();
                for mode in self.hw.supported_modes()? {
                    seen.push(self.hw.set_mode(mode)?.name());
                }
                Ok(format!("cycled {}, restored {original}", seen.join(",")))
            },
            || {
                self.hw
                    .set_mode(original)
                    .map(drop)
                    .map_err(|e| Failure::Wrong(format!("RESTORE FAILED: power mode {original}: {e}")))
            },
        )
    }

    fn sensors(&self) -> Result<String, Failure> {
        let s = self.hw.sensors()?;
        let plausible = |t: i64| t > 10 && t < 110;
        if !plausible(s.cpu_temp) || !plausible(s.gpu_temp) {
            return wrong(format!("temperatures out of range: CPU {} C, GPU {} C", s.cpu_temp, s.gpu_temp));
        }
        Ok(format!(
            "CPU {} C, GPU {} C, fans {}/{} rpm",
            s.cpu_temp, s.gpu_temp, s.cpu_fan_rpm, s.gpu_fan_rpm
        ))
    }

    /// Writes ONE manual duty (no refresh) and watches whether the firmware keeps both fans at it.
    /// The fans go back to firmware Auto whatever happens, and that is checked.
    fn fans(&self) -> Result<String, Failure> {
        guarded(
            || {
                let duty = FanDuty::new(FAN_HOLD_DUTY)?;
                self.hw.fan_manual(duty, duty)?;
                let (mut held, mut samples) = (0usize, 0usize);
                let start = self.deps.clock.now();
                while self.deps.clock.now().saturating_sub(start) < FAN_WATCH {
                    if (self.deps.cancelled)() {
                        return wrong("interrupted");
                    }
                    self.deps.clock.sleep(Duration::from_secs(1));
                    if self.deps.clock.now().saturating_sub(start) > FAN_SPIN_UP {
                        let s = self.hw.sensors()?;
                        samples += 1;
                        if s.cpu_fan_rpm > FAN_MIN_RPM && s.gpu_fan_rpm > FAN_MIN_RPM {
                            held += 1;
                        }
                    }
                }
                if samples == 0 {
                    return wrong("no fan speed sample was taken");
                }
                let percent = 100 * held / samples;
                if percent < FAN_MIN_PERCENT {
                    return wrong(format!("fans held {FAN_HOLD_DUTY} % in only {percent} % of samples"));
                }
                Ok(format!(
                    "one write held both fans >{FAN_MIN_RPM} rpm in {percent} % of samples for {} s",
                    FAN_WATCH.as_secs()
                ))
            },
            || {
                self.hw.fan_auto().map_err(|e| Failure::Wrong(format!("FANS NOT BACK ON AUTO: {e}")))?;
                match self.hw.fan_state() {
                    Ok(state) if state.mode == FanMode::Auto => Ok(()),
                    Ok(state) => {
                        wrong(format!("fans not back on firmware auto (state {:?})", state.mode.name()))
                    }
                    Err(e) => wrong(format!("fans not confirmed back on firmware auto: {e}")),
                }
            },
        )
    }

    fn keyboard(&self) -> Result<String, Failure> {
        let Some(keyboard) = self.deps.keyboard else { return wrong("no keyboard interface") };
        let brightness = keyboard.brightness()?;
        if !(0..=50).contains(&brightness) {
            return wrong(format!("brightness {brightness} is outside 0..50"));
        }
        Ok(format!("brightness {brightness}/50 read back from the keyboard"))
    }

    fn sound(&self) -> Result<&dyn Audio, Failure> {
        self.deps.audio.ok_or_else(|| Failure::Wrong("no sound system interface".to_owned()))
    }

    fn audio_output(&self) -> Result<String, Failure> {
        let active = self.sound()?.output_active()?;
        if active != "speakers" && active != "headphones" {
            return wrong(format!("unexpected output profile {active:?}"));
        }
        Ok(format!("active={active}"))
    }

    fn headset_boost(&self) -> Result<String, Failure> {
        let audio = self.sound()?;
        let Some(current) = audio.headset_boost()? else {
            return Ok("no headset mic boost control on this card (skipped)".to_owned());
        };
        guarded(
            || {
                for level in [0u8, 3, 1] {
                    if audio.set_headset_boost(level)? != level {
                        return wrong(format!("boost {level} did not read back"));
                    }
                }
                Ok(format!("0, 3 and 1 read back, restored to {current}"))
            },
            || match audio.set_headset_boost(current) {
                Ok(back) if back == current => Ok(()),
                Ok(back) => {
                    wrong(format!("RESTORE FAILED: headset mic boost reads {back}, wanted {current}"))
                }
                Err(e) => wrong(format!("RESTORE FAILED: headset mic boost {current}: {e}")),
            },
        )
    }

    fn mic_mute(&self) -> Result<String, Failure> {
        let audio = self.sound()?;
        let muted = audio.mic_muted()?;
        guarded(
            || {
                audio.set_mic_mute(!muted)?;
                if audio.mic_muted()? != !muted {
                    return wrong("mute did not read back");
                }
                Ok(format!("mute toggled and restored ({muted})"))
            },
            || {
                audio
                    .set_mic_mute(muted)
                    .map_err(|e| Failure::Wrong(format!("RESTORE FAILED: microphone mute {muted}: {e}")))
            },
        )
    }

    fn cpu_limits(&self) -> Result<String, Failure> {
        let Some(daemon) = self.deps.daemon else { return wrong("the TrueSense daemon is not running") };
        match daemon.cpu_info() {
            Ok(info) => Ok(format!(
                "PL1 {} W / PL2 {} W readable ({})",
                info.pl1_w,
                info.pl2_w,
                if info.user { "yours" } else { "automatic" }
            )),
            Err(e) if e.is_unknown_command() => {
                Ok("the running service is older than this app (skipped: re-run the installer)".to_owned())
            }
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests;
