//! The fast path against a scripted daemon and a recording firmware.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::json;
use ts_api::api::testing::{FakeDaemon, Reply};
use ts_platform::firmware::Simulator;
use ts_platform::hw::{FanDuty, Hw, HwError, PowerMode};

use super::{Fast, FastHardware};

/// A firmware that records what was written.
#[derive(Default)]
struct Recorder {
    writes: RefCell<Vec<String>>,
    fail: bool,
}

impl FastHardware for Recorder {
    fn fan_auto(&self) -> Result<(), HwError> {
        self.writes.borrow_mut().push("auto".to_owned());
        Ok(())
    }

    fn fan_max(&self) -> Result<(), HwError> {
        self.writes.borrow_mut().push("max".to_owned());
        Ok(())
    }

    fn fan_manual(&self, cpu: FanDuty, gpu: FanDuty) -> Result<(), HwError> {
        self.writes.borrow_mut().push(format!("manual {cpu} {gpu}"));
        Ok(())
    }

    fn set_mode(&self, mode: PowerMode) -> Result<(), HwError> {
        if self.fail {
            return Err(HwError::Invalid("the firmware said no".to_owned()));
        }
        self.writes.borrow_mut().push(format!("mode {mode}"));
        Ok(())
    }
}

struct Run {
    code: i32,
    err: String,
}

fn run(daemon: &Arc<FakeDaemon>, hw: &Recorder, sleeps: &RefCell<Vec<Duration>>, line: &str) -> Run {
    let sleep = |d: Duration| sleeps.borrow_mut().push(d);
    let fast = Fast { daemon: daemon.as_ref(), hardware: hw, sleep: &sleep };
    let args: Vec<String> = line.split_whitespace().map(str::to_owned).collect();
    let mut err = Vec::new();
    let code = fast.run(&args, &mut err);
    Run { code, err: String::from_utf8_lossy(&err).into_owned() }
}

fn offline(line: &str) -> (Run, Vec<String>) {
    let daemon = FakeDaemon::stopped();
    let hw = Recorder::default();
    let sleeps = RefCell::new(Vec::new());
    let ran = run(&daemon, &hw, &sleeps, line);
    let writes = hw.writes.borrow().clone();
    (ran, writes)
}

#[test]
fn without_a_daemon_the_firmware_does_what_is_safe() {
    for (line, want) in [
        ("fan max", "max"),
        ("fan auto", "auto"),
        ("fan 55", "manual 55 55"),
        ("fan 20", "manual 20 20"),
        ("fan 100", "manual 100 100"),
        ("power turbo", "mode turbo"),
        ("power quiet 15", "mode quiet"),
    ] {
        let (ran, writes) = offline(line);
        assert_eq!((ran.code, ran.err.as_str()), (0, ""), "{line}");
        assert_eq!(writes, [want], "{line}");
    }
}

#[test]
fn the_smart_curve_and_the_automatic_mode_need_the_daemon() {
    let (ran, writes) = offline("fan smart");
    assert_eq!((ran.code, ran.err.as_str()), (1, "error: the smart curve needs the TrueSense daemon\n"));
    assert!(writes.is_empty());
    let (ran, _) = offline("auto smart");
    assert_eq!(ran.code, 1);
    assert_eq!(ran.err, "error: the TrueSense daemon is not running (sudo systemctl start truesense)\n");
}

#[test]
fn a_duty_outside_the_range_never_reaches_the_firmware() {
    for line in ["fan 10", "fan 19", "fan 101", "fan 4000"] {
        let (ran, writes) = offline(line);
        assert_eq!(ran.code, 1, "{line}");
        assert_eq!(ran.err, "error: manual fan duty is 20..100 % per fan, or 0 to stop that fan\n", "{line}");
        assert!(writes.is_empty(), "{line}");
    }
}

#[test]
fn a_button_can_never_stop_a_fan() {
    let (ran, writes) = offline("fan 0");
    assert_eq!(ran.code, 1);
    assert_eq!(
        ran.err,
        "error: stopping a fan needs the TrueSense service (it brings the fans back when it gets warm)\n"
    );
    assert!(writes.is_empty());
    // With the daemon running the answer is the one of `fan set 0` without --confirm, and nothing is sent.
    let daemon = FakeDaemon::running();
    let hw = Recorder::default();
    let ran = run(&daemon, &hw, &RefCell::new(Vec::new()), "fan 0");
    assert_eq!((ran.code, ran.err.as_str()), (1, "error: stopping a fan needs explicit confirmation\n"));
    assert!(daemon.calls().is_empty() && hw.writes.borrow().is_empty());
}

#[test]
fn a_dead_daemons_leftover_socket_does_not_block_handing_the_fans_back() {
    for (line, want) in [
        ("fan auto", Some("auto")),
        ("fan max", Some("max")),
        ("fan 55", Some("manual 55 55")),
        ("fan smart", None),
    ] {
        let daemon = FakeDaemon::running();
        daemon.answer("fan", Reply::Unreachable("daemon not answering: connection refused".to_owned()));
        let hw = Recorder::default();
        let ran = run(&daemon, &hw, &RefCell::new(Vec::new()), line);
        match want {
            Some(write) => {
                assert_eq!((ran.code, ran.err.as_str()), (0, ""), "{line}");
                assert_eq!(hw.writes.borrow().as_slice(), [write], "{line}");
            }
            None => {
                assert_eq!(ran.code, 1, "{line}");
                assert!(hw.writes.borrow().is_empty(), "{line}");
            }
        }
    }
}

#[test]
fn usage_mistakes_are_exit_code_two_and_touch_nothing() {
    for (line, usage) in [
        ("", "usage: truesense-fast fan|power|auto ..."),
        ("nonsense x", "usage: truesense-fast fan|power|auto ..."),
        ("fan", "usage: truesense-fast fan smart|auto|max|<20-100>"),
        ("fan fast", "usage: truesense-fast fan smart|auto|max|<20-100>"),
        ("fan -5", "usage: truesense-fast fan smart|auto|max|<20-100>"),
        ("fan 5x", "usage: truesense-fast fan smart|auto|max|<20-100>"),
        ("fan max now", "usage: truesense-fast fan smart|auto|max|<20-100>"),
        ("power", "usage: truesense-fast power <mode> [hold-minutes]"),
        ("power turbo x", "usage: truesense-fast power <mode> [hold-minutes]"),
        ("power turbo -3", "usage: truesense-fast power <mode> [hold-minutes]"),
        ("power turbo 1 2", "usage: truesense-fast power <mode> [hold-minutes]"),
        ("auto", "usage: truesense-fast auto off|native|smart"),
        ("auto maybe", "usage: truesense-fast auto off|native|smart"),
    ] {
        let daemon = FakeDaemon::running();
        let hw = Recorder::default();
        let ran = run(&daemon, &hw, &RefCell::new(Vec::new()), line);
        assert_eq!(ran.code, 2, "{line}");
        assert_eq!(ran.err, format!("{usage}\n"), "{line}");
        assert!(daemon.calls().is_empty() && hw.writes.borrow().is_empty(), "{line}");
    }
}

#[test]
fn a_mode_that_does_not_exist_is_reported_like_the_command_line_does() {
    let (ran, writes) = offline("power warp");
    assert_eq!(ran.code, 2);
    assert_eq!(
        ran.err,
        "usage: truesense power set [-h] {quiet,balanced,performance,turbo,eco}\ntruesense power set: error: argument mode: invalid choice: 'warp' (choose from 'quiet', 'balanced', 'performance', 'turbo', 'eco')\n"
    );
    assert!(writes.is_empty());
}

#[test]
fn with_a_daemon_it_is_one_request_each() {
    let daemon = FakeDaemon::running();
    for cmd in ["fan", "power", "set"] {
        daemon.answer_ok(cmd, json!(null));
    }
    let hw = Recorder::default();
    let sleeps = RefCell::new(Vec::new());
    for line in ["fan smart", "fan 70", "power quiet 15", "power eco", "auto on", "auto native", "auto off"] {
        let ran = run(&daemon, &hw, &sleeps, line);
        assert_eq!((ran.code, ran.err.as_str()), (0, ""), "{line}");
    }
    assert_eq!(daemon.commands(), ["fan", "fan", "power", "power", "set", "set", "set"]);
    let calls = daemon.calls();
    assert_eq!(calls[0].1.get("mode"), Some(&json!("smart")));
    assert_eq!(
        (calls[1].1.get("mode"), calls[1].1.get("cpu"), calls[1].1.get("gpu")),
        (Some(&json!("manual")), Some(&json!(70)), Some(&json!(null)))
    );
    assert_eq!(
        (calls[2].1.get("mode"), calls[2].1.get("hold_minutes")),
        (Some(&json!("quiet")), Some(&json!(15.0)))
    );
    assert_eq!(calls[3].1.get("hold_minutes"), Some(&json!(30.0)));
    assert_eq!(
        (calls[4].1.get("key"), calls[4].1.get("value")),
        (Some(&json!("auto_mode")), Some(&json!("smart")))
    );
    assert_eq!(calls[6].1.get("value"), Some(&json!("off")));
    assert!(hw.writes.borrow().is_empty(), "with a daemon the firmware is not touched by the client");
    assert!(sleeps.borrow().is_empty(), "and nothing is waited for");
}

#[test]
fn a_refusal_of_the_daemon_is_shown_and_the_firmware_is_left_alone() {
    let daemon = FakeDaemon::running();
    daemon.answer("fan", Reply::Error("the smart curve is busy".to_owned()));
    daemon.answer("power", Reply::Error("power mode readback mismatch".to_owned()));
    let hw = Recorder::default();
    let sleeps = RefCell::new(Vec::new());
    let ran = run(&daemon, &hw, &sleeps, "fan max");
    assert_eq!((ran.code, ran.err.as_str()), (1, "error: the smart curve is busy\n"));
    let ran = run(&daemon, &hw, &sleeps, "power turbo");
    assert_eq!((ran.code, ran.err.as_str()), (1, "error: power mode readback mismatch\n"));
    assert!(hw.writes.borrow().is_empty());
}

#[test]
fn a_daemon_older_than_the_program_gets_the_two_step_path() {
    let daemon = FakeDaemon::running();
    daemon.answer("fan", Reply::UnknownCommand);
    daemon.answer("power", Reply::UnknownCommand);
    daemon.answer_ok("set", json!({}));
    daemon.answer_ok("hold", json!(null));
    let hw = Recorder::default();
    let sleeps = RefCell::new(Vec::new());
    assert_eq!(run(&daemon, &hw, &sleeps, "fan max").code, 0);
    assert_eq!(*hw.writes.borrow(), ["max"]);
    assert_eq!(*sleeps.borrow(), [Duration::from_millis(1200)]);
    let set = daemon.last_args("set").unwrap();
    assert_eq!((set.get("key"), set.get("value")), (Some(&json!("smart_fan")), Some(&json!(false))));
    // The smart curve itself is just switched on.
    assert_eq!(run(&daemon, &hw, &sleeps, "fan smart").code, 0);
    assert_eq!(daemon.last_args("set").unwrap().get("value"), Some(&json!(true)));
    assert_eq!(hw.writes.borrow().len(), 1);
    // A mode is written directly and then held.
    assert_eq!(run(&daemon, &hw, &sleeps, "power turbo 20").code, 0);
    assert_eq!(hw.writes.borrow().last().map(String::as_str), Some("mode turbo"));
    assert_eq!(daemon.last_args("hold").unwrap().get("minutes"), Some(&json!(20.0)));
    // No hold when none is wanted.
    daemon.clear_calls();
    assert_eq!(run(&daemon, &hw, &sleeps, "power eco 0").code, 0);
    assert_eq!(daemon.commands(), ["power"]);
}

#[test]
fn a_firmware_failure_is_reported() {
    let daemon = FakeDaemon::stopped();
    let hw = Recorder { fail: true, ..Recorder::default() };
    let ran = run(&daemon, &hw, &RefCell::new(Vec::new()), "power turbo");
    assert_eq!((ran.code, ran.err.as_str()), (1, "error: the firmware said no\n"));
}

#[test]
fn the_real_firmware_type_is_driven_through_the_same_trait() {
    let sim = Arc::new(Simulator::new());
    let hw = Hw::new(sim.clone());
    let daemon = FakeDaemon::stopped();
    let sleeps = Mutex::new(Vec::new());
    let sleep = |d: Duration| sleeps.lock().unwrap().push(d);
    let fast = Fast { daemon: daemon.as_ref(), hardware: &hw, sleep: &sleep };
    let mut err = Vec::new();
    let args = |line: &str| line.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
    assert_eq!(fast.run(&args("power turbo"), &mut err), 0);
    assert_eq!(fast.run(&args("fan 55"), &mut err), 0);
    assert!(err.is_empty(), "{}", String::from_utf8_lossy(&err));
    let snap = sim.snapshot();
    assert_eq!(snap.mode, 5);
    assert_eq!(snap.duty, [55, 55]);
    assert_eq!(fast.run(&args("fan max"), &mut err), 0);
    assert_eq!(format!("{:?}", sim.snapshot().fan).to_lowercase(), "max");
}
