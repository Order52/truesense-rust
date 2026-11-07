#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex, MutexGuard};

use ts_platform::firmware::{Device, FirmwareError, SimFan, Simulator, Snapshot};
use ts_platform::hw::Retry;

use super::*;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap()
}

// ---- fakes ------------------------------------------------------------------------------------

type Hook = Box<dyn FnMut(&str) -> Option<String> + Send>;

/// The simulator behind a hook that may answer a command itself (to refuse it, or to pretend it
/// worked) and a log of every command sent.
struct Wrap {
    sim: Arc<Simulator>,
    sent: Arc<Mutex<Vec<String>>>,
    hook: Mutex<Hook>,
}

impl Device for Wrap {
    fn run(&self, command: &str) -> Result<String, FirmwareError> {
        lock(&self.sent).push(command.to_owned());
        if let Some(reply) = (lock(&self.hook))(command) {
            return Ok(reply);
        }
        self.sim.run(command)
    }
}

/// Time that moves only when someone sleeps.
#[derive(Default)]
struct FakeClock {
    now: Mutex<Duration>,
    sleeps: Mutex<Vec<Duration>>,
}

impl Clock for FakeClock {
    fn now(&self) -> Duration {
        *lock(&self.now)
    }

    fn sleep(&self, duration: Duration) {
        *lock(&self.now) += duration;
        lock(&self.sleeps).push(duration);
    }
}

struct DaemonFake {
    log: Arc<Mutex<Vec<String>>>,
    current: Automation,
    sets: Vec<Automation>,
    set_fails_at: Option<usize>,
    read_fails: bool,
    cpu: Result<CpuInfo, String>,
}

struct FakeDaemon(Mutex<DaemonFake>);

impl FakeDaemon {
    fn new(log: Arc<Mutex<Vec<String>>>) -> Self {
        Self(Mutex::new(DaemonFake {
            log,
            current: Automation { smart_fan: true, auto_profile: true },
            sets: vec![],
            set_fails_at: None,
            read_fails: false,
            cpu: Ok(CpuInfo { pl1_w: 55, pl2_w: 80, user: false }),
        }))
    }

    fn sets(&self) -> Vec<Automation> {
        lock(&self.0).sets.clone()
    }
}

impl Daemon for FakeDaemon {
    fn automation(&self) -> Result<Automation, CallError> {
        let d = lock(&self.0);
        if d.read_fails { Err(CallError::new("no daemon")) } else { Ok(d.current) }
    }

    fn set_automation(&self, automation: Automation) -> Result<(), CallError> {
        let mut d = lock(&self.0);
        let n = d.sets.len();
        d.sets.push(automation);
        lock(&d.log).push(format!(
            "daemon: smart_fan={} auto_profile={}",
            automation.smart_fan, automation.auto_profile
        ));
        if d.set_fails_at == Some(n) {
            return Err(CallError::new("socket closed"));
        }
        d.current = automation;
        Ok(())
    }

    fn cpu_info(&self) -> Result<CpuInfo, CallError> {
        lock(&self.0).cpu.clone().map_err(CallError::new)
    }
}

struct FakeKeyboard(i64);

impl Keyboard for FakeKeyboard {
    fn brightness(&self) -> Result<i64, CallError> {
        Ok(self.0)
    }
}

struct AudioFake {
    output: String,
    boost: Option<u8>,
    muted: bool,
    boost_sticks_at: Option<u8>,
    boost_set_fails_at: Option<u8>,
    mute_ignored: bool,
    panic_after_mute_set: bool,
    restore_fails: bool,
    boost_sets: Vec<u8>,
    mute_sets: Vec<bool>,
}

struct FakeAudio(Mutex<AudioFake>);

impl FakeAudio {
    fn new() -> Self {
        Self(Mutex::new(AudioFake {
            output: "speakers".to_owned(),
            boost: Some(2),
            muted: false,
            boost_sticks_at: None,
            boost_set_fails_at: None,
            mute_ignored: false,
            panic_after_mute_set: false,
            restore_fails: false,
            boost_sets: vec![],
            mute_sets: vec![],
        }))
    }
}

impl Audio for FakeAudio {
    fn output_active(&self) -> Result<String, CallError> {
        Ok(lock(&self.0).output.clone())
    }

    fn headset_boost(&self) -> Result<Option<u8>, CallError> {
        Ok(lock(&self.0).boost)
    }

    fn set_headset_boost(&self, level: u8) -> Result<u8, CallError> {
        let mut a = lock(&self.0);
        a.boost_sets.push(level);
        if a.boost_set_fails_at == Some(level) {
            return Err(CallError::new("amixer failed"));
        }
        if a.boost_sticks_at == Some(level) {
            return Ok(a.boost.unwrap_or(0)); // the card kept its old value
        }
        a.boost = Some(level);
        Ok(level)
    }

    fn mic_muted(&self) -> Result<bool, CallError> {
        let a = lock(&self.0);
        if a.panic_after_mute_set && !a.mute_sets.is_empty() {
            drop(a);
            panic!("the sound system fell over");
        }
        Ok(a.muted)
    }

    fn set_mic_mute(&self, muted: bool) -> Result<(), CallError> {
        let mut a = lock(&self.0);
        let restoring = !a.mute_sets.is_empty();
        a.mute_sets.push(muted);
        if restoring && a.restore_fails {
            return Err(CallError::new("pactl failed"));
        }
        if !a.mute_ignored || restoring {
            a.muted = muted;
        }
        Ok(())
    }
}

struct Rig {
    sim: Arc<Simulator>,
    wrap: Arc<Wrap>,
    hw: Hw,
    clock: FakeClock,
    daemon: FakeDaemon,
    keyboard: FakeKeyboard,
    audio: FakeAudio,
}

fn rig() -> Rig {
    let sim = Arc::new(Simulator::new());
    sim.set_sensors([50, 52, 48], [3000, 3000]);
    let log = Arc::new(Mutex::new(Vec::new()));
    let wrap = Arc::new(Wrap {
        sim: Arc::clone(&sim),
        sent: Arc::clone(&log),
        hook: Mutex::new(Box::new(|_| None)),
    });
    let hw = Hw::new(wrap.clone() as Arc<dyn Device>)
        .with_retry(Retry { tries: 2, delay: Duration::ZERO })
        .with_sleeper(Arc::new(|_| {}));
    Rig {
        sim,
        wrap,
        hw,
        clock: FakeClock::default(),
        daemon: FakeDaemon::new(log),
        keyboard: FakeKeyboard(30),
        audio: FakeAudio::new(),
    }
}

impl Rig {
    fn hook(&self, f: impl FnMut(&str) -> Option<String> + Send + 'static) {
        *lock(&self.wrap.hook) = Box::new(f);
    }

    /// Every firmware command, and the daemon's automation changes among them, in order.
    fn log(&self) -> Vec<String> {
        lock(&self.wrap.sent).clone()
    }

    /// Only the firmware commands.
    fn sent(&self) -> Vec<String> {
        self.log().into_iter().filter(|c| !c.starts_with("daemon:")).collect()
    }

    fn deps(&self) -> Deps<'_> {
        let mut deps = Deps::new(&self.clock);
        deps.daemon = Some(&self.daemon);
        deps.keyboard = Some(&self.keyboard);
        deps.audio = Some(&self.audio);
        deps
    }

    fn run_with(&self, deps: &Deps<'_>, options: Options) -> (Report, String) {
        let mut out = Vec::new();
        let report = run(&self.hw, deps, options, &mut out);
        (report, String::from_utf8(out).unwrap())
    }

    fn run(&self) -> (Report, String) {
        let doctor = |w: &mut dyn Write| {
            let _ = writeln!(w, "doctor says hello");
            0
        };
        let mut deps = self.deps();
        deps.doctor = Some(&doctor);
        self.run_with(&deps, Options::default())
    }

    /// The firmware state with the fan duty registers (which a manual write leaves) set aside.
    fn settings(&self) -> Snapshot {
        let mut s = self.sim.snapshot();
        s.duty = [0, 0];
        s
    }
}

fn item<'a>(report: &'a Report, name: &str) -> &'a Item {
    report
        .items
        .iter()
        .find(|i| i.name == name)
        .unwrap_or_else(|| panic!("no item {name:?}: {:?}", report.items))
}

fn failed(report: &Report) -> Vec<&str> {
    report.items.iter().filter(|i| i.outcome == Outcome::Fail).map(|i| i.name.as_str()).collect()
}

// ---- the whole run ------------------------------------------------------------------------------

#[test]
fn a_healthy_run_passes_seventeen_checks_and_restores_every_setting() {
    let r = rig();
    let before = r.settings();
    let (report, out) = r.run();
    assert!(report.ok(), "{out}");
    assert_eq!((report.passed, report.total), (17, 17), "{out}");
    assert_eq!(r.settings(), before, "every setting is back");
    assert_eq!(r.sim.snapshot().fan, SimFan::Auto);
    assert!(out.starts_with("== firmware features (set -> read back -> restore)\n"), "{out}");
    assert!(
        out.contains(
            "PASS  power modes (all supported)  (cycled quiet,balanced,performance,turbo,eco, restored eco)"
        ),
        "{out}"
    );
    assert!(out.contains("PASS  sensors  (CPU 50 C, GPU 48 C, fans 3000/3000 rpm)"), "{out}");
    assert!(
        out.contains("PASS  fans: manual 60 % holds, then back to auto  (one write held both fans >2000 rpm in 100 % of samples for 22 s)"),
        "{out}"
    );
    assert!(out.contains("PASS  LCD overdrive  (true -> false -> restored)"), "{out}");
    assert!(out.contains("PASS  USB charging level  (30 -> 10 -> restored)"), "{out}");
    assert!(out.contains("PASS  GPU mode (read)  (nvidia)"), "{out}");
    assert!(out.contains("PASS  rear light (read)  ({\"raw\":\"00000300"), "{out}");
    assert!(
        out.contains("PASS  keyboard lighting interface  (brightness 30/50 read back from the keyboard)"),
        "{out}"
    );
    assert!(out.contains("PASS  audio output profile  (active=speakers)"), "{out}");
    assert!(out.contains("PASS  headset mic boost  (0, 3 and 1 read back, restored to 2)"), "{out}");
    assert!(out.contains("PASS  microphone mute  (mute toggled and restored (false))"), "{out}");
    assert!(
        out.contains("PASS  CPU power limits (read)  (PL1 55 W / PL2 80 W readable (automatic))"),
        "{out}"
    );
    assert!(out.contains("\n== stack health (doctor)\ndoctor says hello\n\n17/17 passed\n"), "{out}");
    let names: Vec<&str> = report.items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names.len(), 17);
    assert_eq!(names[0], "power modes (all supported)");
    assert_eq!(names[16], "stack health (doctor)");
}

#[test]
fn the_selftest_only_sends_commands_the_module_knows_and_never_a_fan_stop_or_a_gpu_or_rear_write() {
    let r = rig();
    r.run();
    for command in r.sent() {
        assert!(
            !command.starts_with("set_gpu")
                && !command.starts_with("set_rear")
                && !command.starts_with("set_color"),
            "{command}"
        );
        if let Some(duties) = command.strip_prefix("set_fan_manual ") {
            assert_eq!(duties, "60 60", "only the one 60 % write");
        }
    }
    assert_eq!(
        r.sent().iter().filter(|c| c.starts_with("set_fan_manual")).count(),
        1,
        "ONE write, no refresh"
    );
}

#[test]
fn every_setting_is_restored_after_a_failure_in_the_middle() {
    for failing in [
        "set_boot 1",
        "set_battery 1 1",
        "set_battery 2 1",
        "set_usb 10",
        "set_kbtimeout 0",
        "set_overdrive 0",
    ] {
        let r = rig();
        let before = r.settings();
        let refuse = failing.to_owned();
        r.hook(move |c| (c == refuse).then(|| "err refused\n".to_owned()));
        let (report, out) = r.run();
        assert_eq!(failed(&report).len(), 1, "{failing}: {out}");
        assert_eq!(r.settings(), before, "{failing}: restored");
        assert_eq!(report.total, 17);
        assert_eq!(report.passed, 16, "the rest of the run went on");
    }
}

#[test]
fn a_power_mode_that_fails_half_way_is_still_put_back() {
    let r = rig();
    let before = r.settings();
    r.hook(|c| (c == "set_mode 5").then(|| "err refused\n".to_owned()));
    let (report, _) = r.run();
    assert_eq!(item(&report, "power modes (all supported)").outcome, Outcome::Fail);
    assert!(item(&report, "power modes (all supported)").detail.contains("refused"));
    assert_eq!(r.settings(), before);
}

#[test]
fn a_write_the_firmware_swallows_is_a_failed_readback_and_nothing_is_left_changed() {
    let r = rig();
    let before = r.settings();
    // The firmware says "ok" to the write but never applies it.
    r.hook(|c| (c.starts_with("set_battery 1")).then(|| "ok battery=1 value=1\n".to_owned()));
    let (report, _) = r.run();
    let battery = item(&report, "battery 80 % limit");
    assert_eq!(battery.outcome, Outcome::Fail);
    assert!(battery.detail.contains("readback mismatch"), "{battery:?}");
    assert_eq!(r.settings(), before);
}

#[test]
fn a_restore_that_does_not_take_is_called_out_loudly() {
    // The write of the changed value works; putting the original back is swallowed.
    let r = rig();
    let n = Mutex::new(0);
    r.hook(move |c| {
        if c.starts_with("set_boot") {
            let mut n = n.lock().unwrap();
            *n += 1;
            return (*n == 2).then(|| "ok boot=0\n".to_owned());
        }
        None
    });
    let (report, out) = r.run();
    let boot = item(&report, "boot animation/sound");
    assert_eq!(boot.outcome, Outcome::Fail, "{out}");
    assert!(boot.detail.contains("RESTORE FAILED"), "{boot:?}");
    assert!(!report.ok());
}

#[test]
fn a_failure_is_reported_with_the_item_and_the_message_and_counts_in_the_summary() {
    let r = rig();
    r.hook(|c| (c == "get_boot").then(|| "err no such thing\n".to_owned()));
    let (report, out) = r.run();
    assert!(out.contains("FAIL  boot animation/sound: no such thing\n"), "{out}");
    assert!(out.ends_with("16/17 passed\n"), "{out}");
    assert!(!report.ok());
    assert_eq!(item(&report, "boot animation/sound").detail, "no such thing");
}

// ---- modes, sensors and reads -------------------------------------------------------------------

#[test]
fn an_unknown_power_mode_is_not_touched() {
    let r = rig();
    r.hook(|c| (c == "get_mode").then(|| "ok mode=9\n".to_owned()));
    let (report, _) = r.run();
    let modes = item(&report, "power modes (all supported)");
    assert_eq!(modes.outcome, Outcome::Fail);
    assert!(modes.detail.contains("unknown(9)") && modes.detail.contains("not changing it"), "{modes:?}");
    assert!(!r.sent().iter().any(|c| c.starts_with("set_mode")), "nothing was written");
}

#[test]
fn sensors_out_of_range_fail() {
    for (temps, ok) in [
        ([50, 50, 50], true),
        ([10, 50, 50], false),
        ([50, 50, 110], false),
        ([111, 50, 50], false),
        ([50, 50, 11], true),
    ] {
        let r = rig();
        r.sim.set_sensors(temps, [3000, 3000]);
        let (report, _) = r.run();
        let sensors = item(&report, "sensors");
        assert_eq!(sensors.outcome == Outcome::Pass, ok, "{temps:?}: {sensors:?}");
        if !ok {
            assert!(sensors.detail.starts_with("temperatures out of range: CPU "), "{sensors:?}");
        }
    }
}

#[test]
fn the_usb_level_alternative_is_20_when_it_is_already_10() {
    let r = rig();
    r.hw.set_usb_charge(UsbLevel::Percent10).unwrap();
    let (report, _) = r.run();
    assert_eq!(item(&report, "USB charging level").detail, "10 -> 20 -> restored");
    assert_eq!(r.sim.snapshot().usb, 10);
}

#[test]
fn unknown_values_cannot_be_toggled_and_are_not_written() {
    let r = rig();
    r.hook(|c| match c {
        "get_overdrive" => Some("ok overdrive=-1 raw=0x0\n".to_owned()),
        "get_kbtimeout" => Some("ok kbtimeout=-1 raw=0x0\n".to_owned()),
        "get_usb" => Some("ok usb=-1 raw=0\n".to_owned()),
        _ => None,
    });
    let (report, _) = r.run();
    assert_eq!(item(&report, "LCD overdrive").detail, "overdrive unknown");
    assert_eq!(item(&report, "keyboard backlight timeout").detail, "keyboard backlight timeout unknown");
    assert_eq!(item(&report, "USB charging level").detail, "USB charging level is unknown(-1)");
    assert!(!r.sent().iter().any(|c| c.starts_with("set_overdrive")
        || c.starts_with("set_kbtimeout")
        || c.starts_with("set_usb")));
}

#[test]
fn toggle_reports_a_readback_mismatch_and_still_restores() {
    let state = Mutex::new(false);
    let sets = Mutex::new(vec![]);
    let result = toggle(
        || Ok(*state.lock().unwrap()),
        |v| {
            sets.lock().unwrap().push(v);
            // the change does not take; the original is fine
            if !v {
                *state.lock().unwrap() = v;
            }
            Ok(())
        },
        |v| !v,
    );
    let error = result.unwrap_err().to_string();
    assert_eq!(error, "readback mismatch");
    assert_eq!(*sets.lock().unwrap(), vec![true, false], "the original was written back");
}

#[test]
fn toggle_reports_a_restore_that_reads_wrong_and_one_that_cannot_be_read() {
    let state = Mutex::new(false);
    let result = toggle(
        || Ok(*state.lock().unwrap()),
        |v| {
            if v {
                *state.lock().unwrap() = true
            }
            Ok(())
        },
        |v| !v,
    );
    assert_eq!(result.unwrap_err().to_string(), "RESTORE FAILED: reads true, wanted false");

    let reads = Mutex::new(0);
    let result = toggle(
        || {
            let mut n = reads.lock().unwrap();
            *n += 1;
            if *n <= 2 { Ok(*n == 2) } else { wrong("unreadable") }
        },
        |_| Ok(()),
        |v| !v,
    );
    assert_eq!(result.unwrap_err().to_string(), "RESTORE FAILED: cannot read it back: unreadable");
}

#[test]
fn both_the_failure_and_the_failed_restore_are_reported() {
    let result: Result<(), Failure> = guarded(|| wrong("first"), || wrong("second"));
    assert_eq!(result.unwrap_err().to_string(), "first; second");
    assert!(guarded(|| Ok(1), || Ok(())).is_ok());
    assert_eq!(guarded(|| Ok(1), || wrong("restore")).unwrap_err().to_string(), "restore");
}

#[test]
fn a_panic_in_the_middle_is_a_fail_of_that_item_and_still_restores() {
    let r = rig();
    r.audio.0.lock().unwrap().panic_after_mute_set = true;
    let (report, out) = r.run();
    let mic = item(&report, "microphone mute");
    assert_eq!(mic.outcome, Outcome::Fail, "{out}");
    assert!(mic.detail.contains("panic: the sound system fell over"), "{mic:?}");
    assert_eq!(lock(&r.audio.0).mute_sets, vec![true, false], "the mute was put back");
    assert!(!lock(&r.audio.0).muted);
    assert_eq!(report.total, 17, "the run went on to the end");
    assert_eq!(item(&report, "CPU power limits (read)").outcome, Outcome::Pass);
}

// ---- fans ---------------------------------------------------------------------------------------

#[test]
fn the_fans_hold_the_duty_and_the_watch_takes_22_virtual_seconds() {
    let r = rig();
    let (report, _) = r.run();
    assert_eq!(item(&report, "fans: manual 60 % holds, then back to auto").outcome, Outcome::Pass);
    let sleeps = lock(&r.clock.sleeps).clone();
    assert_eq!(sleeps.iter().filter(|d| **d == Duration::from_secs(1)).count(), 22);
    assert_eq!(sleeps[0], PAUSE_SETTLE, "the daemon is given time to notice first");
    assert_eq!(r.sim.snapshot().fan, SimFan::Auto);
}

#[test]
fn fans_that_do_not_hold_fail_and_still_go_back_to_auto() {
    let r = rig();
    r.sim.set_sensors([50, 52, 48], [0, 0]);
    let (report, _) = r.run();
    let fans = item(&report, "fans: manual 60 % holds, then back to auto");
    assert_eq!(fans.outcome, Outcome::Fail);
    assert_eq!(fans.detail, "fans held 60 % in only 0 % of samples");
    assert_eq!(r.sim.snapshot().fan, SimFan::Auto);
}

#[test]
fn fans_that_hold_most_of_the_time_pass_and_fewer_than_85_percent_fail() {
    for (slow_samples, ok) in [(2usize, true), (3, false)] {
        let r = rig();
        let n = Mutex::new(0usize);
        let sim = Arc::clone(&r.sim);
        r.hook(move |c| {
            if c == "sensors" {
                let mut n = n.lock().unwrap();
                *n += 1;
                // The first `slow_samples` readings (of 14) show a fan below the threshold.
                sim.set_sensors(
                    [50, 52, 48],
                    if *n <= slow_samples + 1 && *n > 1 { [1000, 3000] } else { [3000, 3000] },
                );
            }
            None
        });
        let (report, _) = r.run();
        let fans = item(&report, "fans: manual 60 % holds, then back to auto");
        assert_eq!(fans.outcome == Outcome::Pass, ok, "{slow_samples}: {fans:?}");
        assert_eq!(r.sim.snapshot().fan, SimFan::Auto);
    }
}

#[test]
fn a_failing_manual_write_still_hands_the_fans_back() {
    let r = rig();
    r.hook(|c| c.starts_with("set_fan_manual").then(|| "err refused\n".to_owned()));
    let (report, _) = r.run();
    let fans = item(&report, "fans: manual 60 % holds, then back to auto");
    assert_eq!((fans.outcome, fans.detail.as_str()), (Outcome::Fail, "refused"));
    assert!(r.sent().iter().any(|c| c == "set_fan_auto"), "{:?}", r.sent());
    assert_eq!(r.sim.snapshot().fan, SimFan::Auto);
}

#[test]
fn an_interrupt_during_the_watch_ends_it_and_the_fans_go_back_to_auto() {
    let r = rig();
    let polls = std::sync::atomic::AtomicUsize::new(0);
    let cancelled = || polls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 5;
    let mut deps = r.deps();
    deps.cancelled = &cancelled;
    let (report, _) = r.run_with(&deps, Options::default());
    let fans = item(&report, "fans: manual 60 % holds, then back to auto");
    assert_eq!((fans.outcome, fans.detail.as_str()), (Outcome::Fail, "interrupted"));
    assert_eq!(r.sim.snapshot().fan, SimFan::Auto);
    assert_eq!(report.total, 16, "no doctor wired in: that item is skipped");
}

#[test]
fn fans_that_cannot_be_handed_back_say_so_loudly() {
    let r = rig();
    r.hook(|c| (c == "set_fan_auto").then(|| "err refused\n".to_owned()));
    let (report, _) = r.run();
    let fans = item(&report, "fans: manual 60 % holds, then back to auto");
    assert_eq!(fans.outcome, Outcome::Fail);
    assert!(fans.detail.contains("FANS NOT BACK ON AUTO"), "{fans:?}");
}

#[test]
fn a_fan_state_that_cannot_be_confirmed_is_a_failure() {
    let r = rig();
    let n = Mutex::new(0);
    r.hook(move |c| {
        if c == "get_fan" {
            let mut n = n.lock().unwrap();
            *n += 1;
            // Reads 1 and 2 belong to the manual write's own read-back and to the stop check; later ones fail.
            if *n > 1 {
                return Some("err busy\n".to_owned());
            }
        }
        None
    });
    let (report, _) = r.run();
    let fans = item(&report, "fans: manual 60 % holds, then back to auto");
    assert_eq!(fans.outcome, Outcome::Fail);
}

#[test]
fn skip_fans_never_touches_them() {
    let r = rig();
    let mut deps = r.deps();
    deps.doctor = None;
    let (report, out) = r.run_with(&deps, Options { skip_fans: true });
    assert!(out.contains("SKIP  fans: --skip-fans\n"), "{out}");
    assert!(!r.sent().iter().any(|c| c.starts_with("set_fan")), "{:?}", r.sent());
    assert_eq!(item(&report, "fans").outcome, Outcome::Skip);
    assert_eq!((report.passed, report.total), (15, 15), "a skip is not counted: 16 items less the doctor");
}

// ---- the daemon ---------------------------------------------------------------------------------

#[test]
fn the_daemons_automation_is_paused_and_put_back() {
    let r = rig();
    let (report, _) = r.run();
    assert!(report.ok());
    assert_eq!(
        r.daemon.sets(),
        vec![
            Automation { smart_fan: false, auto_profile: false },
            Automation { smart_fan: true, auto_profile: true }
        ]
    );
    // paused before the first firmware write, and put back after the fan item and before the
    // overdrive item
    let log = r.log();
    let at = |needle: &str| log.iter().position(|c| c == needle || c.starts_with(needle)).unwrap();
    assert!(at("daemon: smart_fan=false auto_profile=false") < at("set_mode"));
    assert!(at("set_fan_auto") < at("daemon: smart_fan=true auto_profile=true"));
    assert!(at("daemon: smart_fan=true auto_profile=true") < at("set_overdrive"));
    assert_eq!(lock(&r.clock.sleeps)[0], Duration::from_millis(2500));
}

#[test]
fn a_half_failed_pause_still_restores_and_the_run_goes_on() {
    let r = rig();
    r.daemon.0.lock().unwrap().set_fails_at = Some(0);
    let (report, out) = r.run();
    assert_eq!(
        r.daemon.sets(),
        vec![
            Automation { smart_fan: false, auto_profile: false },
            Automation { smart_fan: true, auto_profile: true }
        ],
        "the original was written back even though the pause failed"
    );
    assert!(report.ok(), "{out}");
    assert!(lock(&r.clock.sleeps).iter().all(|d| *d != PAUSE_SETTLE), "no settle wait without a pause");
}

#[test]
fn a_restore_of_the_daemon_that_fails_is_reported() {
    let r = rig();
    r.daemon.0.lock().unwrap().set_fails_at = Some(1);
    let (report, out) = r.run();
    assert!(out.contains("FAIL  restoring the daemon's smart fan / auto profile: socket closed\n"), "{out}");
    assert_eq!(item(&report, "restore the daemon's automation").outcome, Outcome::Fail);
    assert!(!report.ok());
}

#[test]
fn without_a_daemon_nothing_is_paused_and_the_cpu_limits_item_fails() {
    let r = rig();
    let mut deps = r.deps();
    deps.daemon = None;
    let (report, _) = r.run_with(&deps, Options::default());
    assert!(r.daemon.sets().is_empty());
    assert_eq!(item(&report, "CPU power limits (read)").detail, "the TrueSense daemon is not running");
    assert_eq!(failed(&report), ["CPU power limits (read)"]);
}

#[test]
fn a_daemon_that_does_not_answer_the_state_is_not_paused() {
    let r = rig();
    r.daemon.0.lock().unwrap().read_fails = true;
    let (report, _) = r.run();
    assert!(r.daemon.sets().is_empty());
    assert!(report.ok());
}

#[test]
fn the_cpu_limits_item() {
    let cases: [(Result<CpuInfo, &str>, Outcome, &str); 4] = [
        (
            Ok(CpuInfo { pl1_w: 45, pl2_w: 95, user: true }),
            Outcome::Pass,
            "PL1 45 W / PL2 95 W readable (yours)",
        ),
        (
            Ok(CpuInfo { pl1_w: 55, pl2_w: 80, user: false }),
            Outcome::Pass,
            "PL1 55 W / PL2 80 W readable (automatic)",
        ),
        (
            Err("unknown command 'cpu_info'"),
            Outcome::Pass,
            "the running service is older than this app (skipped: re-run the installer)",
        ),
        (Err("socket closed"), Outcome::Fail, "socket closed"),
    ];
    for (cpu, outcome, detail) in cases {
        let r = rig();
        r.daemon.0.lock().unwrap().cpu = cpu.map_err(str::to_owned);
        let (report, _) = r.run();
        let got = item(&report, "CPU power limits (read)");
        assert_eq!((got.outcome, got.detail.as_str()), (outcome, detail));
    }
}

// ---- keyboard, audio and the doctor -------------------------------------------------------------------------

#[test]
fn the_keyboard_item() {
    for (brightness, ok) in [(0, true), (50, true), (51, false), (-1, false)] {
        let mut r = rig();
        r.keyboard = FakeKeyboard(brightness);
        let (report, _) = r.run();
        let got = item(&report, "keyboard lighting interface");
        assert_eq!(got.outcome == Outcome::Pass, ok, "{brightness}: {got:?}");
        if !ok {
            assert_eq!(got.detail, format!("brightness {brightness} is outside 0..50"));
        }
    }
    let r = rig();
    let mut deps = r.deps();
    deps.keyboard = None;
    let (report, _) = r.run_with(&deps, Options::default());
    assert_eq!(item(&report, "keyboard lighting interface").detail, "no keyboard interface");
}

#[test]
fn audio_items() {
    // no boost control on this card
    let r = rig();
    r.audio.0.lock().unwrap().boost = None;
    let (report, _) = r.run();
    assert_eq!(
        item(&report, "headset mic boost").detail,
        "no headset mic boost control on this card (skipped)"
    );
    assert!(lock(&r.audio.0).boost_sets.is_empty(), "nothing written");

    // a boost that fails in the middle is restored
    let r = rig();
    r.audio.0.lock().unwrap().boost_set_fails_at = Some(3);
    let (report, _) = r.run();
    let boost = item(&report, "headset mic boost");
    assert_eq!(boost.outcome, Outcome::Fail);
    assert!(boost.detail.contains("amixer failed"), "{boost:?}");
    assert_eq!(lock(&r.audio.0).boost, Some(2), "restored to 2");
    assert_eq!(lock(&r.audio.0).boost_sets.last(), Some(&2));

    // a boost that does not read back
    let r = rig();
    r.audio.0.lock().unwrap().boost_sticks_at = Some(3);
    let (report, _) = r.run();
    assert_eq!(item(&report, "headset mic boost").detail, "boost 3 did not read back");
    assert_eq!(lock(&r.audio.0).boost, Some(2));

    // a mute that does not read back is restored
    let r = rig();
    r.audio.0.lock().unwrap().mute_ignored = true;
    let (report, _) = r.run();
    assert_eq!(item(&report, "microphone mute").detail, "mute did not read back");
    assert!(!lock(&r.audio.0).muted);
    assert_eq!(lock(&r.audio.0).mute_sets, vec![true, false]);

    // a mute that cannot be restored
    let r = rig();
    r.audio.0.lock().unwrap().restore_fails = true;
    let (report, _) = r.run();
    assert!(
        item(&report, "microphone mute")
            .detail
            .contains("RESTORE FAILED: microphone mute false: pactl failed")
    );

    // an output profile that is neither
    let r = rig();
    r.audio.0.lock().unwrap().output = "hdmi".to_owned();
    let (report, _) = r.run();
    assert_eq!(item(&report, "audio output profile").detail, "unexpected output profile \"hdmi\"");
    r.audio.0.lock().unwrap().output = "headphones".to_owned();
    let (report, _) = r.run();
    assert_eq!(item(&report, "audio output profile").detail, "active=headphones");
}

#[test]
fn without_a_sound_system_the_audio_items_fail_and_touch_nothing() {
    let r = rig();
    let mut deps = r.deps();
    deps.audio = None;
    let (report, _) = r.run_with(&deps, Options::default());
    for name in ["audio output profile", "headset mic boost", "microphone mute"] {
        assert_eq!(item(&report, name).detail, "no sound system interface");
    }
}

#[test]
fn the_doctor_item_follows_its_exit_code_and_is_skipped_when_absent() {
    let r = rig();
    let doctor = |w: &mut dyn Write| {
        let _ = writeln!(w, "line one\nline two\n\n");
        1
    };
    let mut deps = r.deps();
    deps.doctor = Some(&doctor);
    let (report, out) = r.run_with(&deps, Options::default());
    assert_eq!(item(&report, "stack health (doctor)").outcome, Outcome::Fail);
    assert!(out.contains("== stack health (doctor)\nline one\nline two\n\n16/17 passed\n"), "{out}");
    assert!(!report.ok());

    let mut deps = r.deps();
    deps.doctor = None;
    let (report, out) = r.run_with(&deps, Options::default());
    assert!(out.contains("SKIP  stack health (doctor): no health check wired in\n"), "{out}");
    assert_eq!(item(&report, "stack health (doctor)").outcome, Outcome::Skip);
    assert_eq!(report.total, 16);
}

#[test]
fn a_closed_output_does_not_stop_the_run() {
    struct Closed;
    impl Write for Closed {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let r = rig();
    let before = r.settings();
    let report = run(&r.hw, &r.deps(), Options::default(), &mut Closed);
    assert_eq!(report.total, 16);
    assert_eq!(r.settings(), before);
}

#[test]
fn the_report_serialises_for_machines() {
    let r = rig();
    let (report, _) = r.run();
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["passed"], 17);
    assert_eq!(json["items"][0]["outcome"], "PASS");
    assert_eq!(json["items"][0]["name"], "power modes (all supported)");
}

#[test]
fn the_real_clock_moves_forward() {
    let clock = SystemClock::new();
    let a = clock.now();
    clock.sleep(Duration::from_millis(5));
    assert!(clock.now() > a);
    assert!(!never());
}

#[test]
fn the_failure_texts() {
    let hw = Failure::from(HwError::Invalid("bad value".to_owned()));
    assert_eq!(hw.to_string(), "bad value");
    assert!(std::error::Error::source(&hw).is_some());
    let call = Failure::from(CallError::new("down"));
    assert_eq!(call.to_string(), "down");
    assert!(std::error::Error::source(&call).is_some());
    assert!(std::error::Error::source(&Failure::Wrong("x".to_owned())).is_none());
}
