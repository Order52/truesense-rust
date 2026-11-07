//! What happens to the fans and the settings when the daemon, the disk, the clock or the
//! machine misbehave: a second daemon, a stale socket, a full disk, a corrupt settings file, a
//! clock that jumps, a suspend in the middle of a tick, a crash injected into every step, a real
//! SIGTERM and a real SIGKILL in a child process.
//!
//! This is the evidence behind `docs/RESILIENCE.md` (the failure-mode table names these tests).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader};
use std::os::unix::net::{UnixListener, UnixStream};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use ts_foundation::fsx::{self, FileLock, FsError};
use ts_foundation::state::Writer;
use ts_ipc::wire::Args;
use ts_platform::firmware::{Device, SimFan};

use super::DaemonError;
use super::instance::lock_path;
use super::testkit::{Rig, Setup};

fn args() -> Args {
    Args::new()
}

fn live(rig: &Rig) -> Value {
    rig.ok("state", args())["live"].clone()
}

// ---- one daemon per machine ------------------------------------------------------------------

#[test]
fn a_second_daemon_refuses_and_changes_nothing() {
    let first = Rig::new();
    let second = Rig::new();
    second.ok_fan_manual(70);
    let socket = first.path("shared.sock");
    let outcome = Arc::new(Mutex::new(None));
    let seen_alive = Arc::new(AtomicBool::new(false));
    {
        let (daemon, socket, outcome, alive) =
            (second.daemon.clone(), socket.clone(), outcome.clone(), seen_alive.clone());
        first.sleeper.on_sleep(move |n| {
            if n == 1 {
                *outcome.lock().unwrap() = Some(daemon.run(&socket).map_err(|e| e.to_string()));
                alive.store(UnixStream::connect(&socket).is_ok(), Ordering::SeqCst);
            }
        });
    }
    first.sleeper.stop_after(3);
    first.daemon.run(&socket).unwrap();
    let got = outcome.lock().unwrap().take().expect("the second daemon was tried");
    let why = got.expect_err("a second daemon must not start");
    assert!(why.contains("already running"), "{why}");
    assert_eq!(second.snap().fan, SimFan::Manual, "the refused daemon did not hand any fans back");
    assert_eq!(second.cap.uncapped.load(Ordering::SeqCst), 0, "nor lift the clock cap");
    assert_eq!(second.daemon.ticks(), 0, "nor run a tick");
    assert!(seen_alive.load(Ordering::SeqCst), "the first daemon's socket was not deleted by the second");
    assert_eq!(first.daemon.ticks(), 3);
}

#[test]
fn the_lock_names_the_holder_and_goes_with_the_process() {
    let rig = Rig::new();
    let socket = rig.path("s.sock");
    let held = FileLock::try_exclusive(&lock_path(&socket)).unwrap().unwrap();
    rig.ok_fan_manual(70);
    let e = rig.daemon.run(&socket).unwrap_err();
    assert!(matches!(e, DaemonError::AlreadyRunning(_)), "{e}");
    assert!(e.to_string().contains("s.lock"), "{e}");
    assert_eq!(rig.snap().fan, SimFan::Manual);
    drop(held); // the process that held it died: the kernel released the lock
    rig.stop.request();
    rig.daemon.run(&socket).unwrap();
    assert_eq!(rig.snap().fan, SimFan::Auto);
}

#[test]
fn a_daemon_of_another_edition_that_answers_on_the_socket_is_respected() {
    let rig = Rig::new();
    let socket = rig.path("old.sock");
    let _old_daemon = UnixListener::bind(&socket).unwrap();
    let e = rig.daemon.run(&socket).unwrap_err();
    assert!(matches!(e, DaemonError::AlreadyRunning(_)), "{e}");
    assert!(UnixStream::connect(&socket).is_ok(), "its socket file is still there");
}

#[test]
fn a_stale_socket_and_a_stale_lock_file_from_a_crash_do_not_block_the_restart() {
    let rig = Rig::new();
    let socket = rig.path("crashed.sock");
    drop(UnixListener::bind(&socket).unwrap()); // the file stays, nobody listens: what SIGKILL leaves
    std::fs::write(lock_path(&socket), "").unwrap(); // and the lock file, unheld
    rig.sleeper.stop_after(2);
    rig.daemon.run(&socket).unwrap();
    assert_eq!(rig.daemon.ticks(), 2);
    assert!(!socket.exists(), "a clean stop removes the socket");
}

// ---- a disk that fails, a file that is corrupt ---------------------------------------------------

fn flaky_writer(failing: Arc<AtomicBool>, calls: Arc<AtomicUsize>) -> Writer {
    Arc::new(move |path: &Path, bytes: &[u8]| {
        calls.fetch_add(1, Ordering::SeqCst);
        if failing.load(Ordering::SeqCst) {
            return Err(FsError::Io {
                op: "write",
                path: path.to_path_buf(),
                source: std::io::Error::from_raw_os_error(28),
            });
        }
        fsx::write_atomic_mkdir(path, bytes, 0o644)
    })
}

#[test]
fn a_full_disk_does_not_stop_the_fans_and_the_settings_are_written_once_it_has_room() {
    let failing = Arc::new(AtomicBool::new(true));
    let calls = Arc::new(AtomicUsize::new(0));
    let rig = Rig::with(Setup {
        state_writer: Some(flaky_writer(failing.clone(), calls.clone())),
        ..Setup::default()
    });
    rig.ok("set", args().with("key", "smart_fan").with("value", true));
    rig.ok("set", args().with("key", "operating_follow").with("value", true));
    assert!(rig.state.is_dirty());
    assert!(rig.log.has("could not save the setting"), "the failure is in the journal");
    // The laptop gets hot with the disk still full: the fans must follow.
    rig.drive(70, 88, 70, 90.0);
    assert!(rig.snap().duty[0] >= 70, "the smart fan runs on, duty {:?}", rig.snap().duty);
    assert!(
        calls.load(Ordering::SeqCst) >= 3,
        "the write is retried by the loop: {} calls",
        calls.load(Ordering::SeqCst)
    );
    assert!(!rig.path("state.json").exists());
    failing.store(false, Ordering::SeqCst);
    rig.drive(31, 60, 55, 30.0);
    assert!(!rig.state.is_dirty());
    assert!(rig.log.has("the settings are written to disk again"));
    let on_disk: Value = serde_json::from_str(&rig.read(&rig.path("state.json"))).unwrap();
    assert_eq!(on_disk["operating_follow"], json!(true));
}

#[test]
fn a_corrupt_settings_file_starts_on_the_defaults_says_so_and_is_kept() {
    let rig = Rig::with(Setup { state_file: Some("{\"smart_fan\": tru".to_owned()), ..Setup::default() });
    rig.daemon.boot();
    rig.drive(3, 85, 70, 80.0);
    assert!(rig.log.has("the saved settings were not used"), "{}", rig.log.all());
    let l = live(&rig);
    assert!(l["warning"].as_str().unwrap().contains("not valid JSON"), "{l}");
    assert!(rig.read(&rig.path("run/state")).contains("warning="), "status bars see it too");
    assert_eq!(rig.read(&rig.path("state.json.corrupt")), "{\"smart_fan\": tru");
    assert!(rig.snap().duty[0] >= 50, "the fans are controlled on the defaults");
}

#[test]
fn problems_found_in_the_configuration_are_visible_in_the_live_state_and_the_log() {
    let clean = Rig::new();
    clean.daemon.boot();
    clean.drive(1, 50, 50, 5.0);
    assert!(live(&clean).get("warning").is_none(), "nothing to say on a healthy machine");
    let rig = Rig::with(Setup {
        warnings: vec![
            "config problem: [fan] levels must be 20..100".to_owned(),
            "the built-in defaults are in force".to_owned(),
        ],
        ..Setup::default()
    });
    rig.daemon.boot();
    rig.drive(1, 50, 50, 5.0);
    let l = live(&rig);
    assert_eq!(
        l["warning"],
        json!("config problem: [fan] levels must be 20..100; the built-in defaults are in force")
    );
    assert!(rig.log.has("config problem: [fan] levels must be 20..100"));
    assert!(rig.read(&rig.path("run/state")).contains("warning=config problem: [fan] levels"));
}

#[test]
fn a_crashed_write_leaves_a_temp_file_and_the_next_start_sweeps_it() {
    let rig = Rig::new();
    std::fs::write(rig.path(".state.json.4242.1f.0.tmp"), "{\"half").unwrap();
    std::fs::write(rig.path("keep-me.tmp"), "x").unwrap();
    rig.daemon.boot();
    assert!(!rig.path(".state.json.4242.1f.0.tmp").exists());
    assert!(rig.path("keep-me.tmp").exists(), "only the store's own temp files");
    assert!(rig.log.has("removed 1 temporary file"));
}

// ---- the clock ---------------------------------------------------------------------------------

#[test]
fn a_clock_set_back_does_not_pin_a_held_power_mode_for_weeks() {
    let rig = Rig::new();
    rig.drive(3, 45, 45, 5.0);
    assert_eq!(rig.ok("hold", args().with("minutes", 30)), json!(30.0));
    assert!(rig.state.get_f64("profile_hold_until", 0.0) > rig.clock.wall_now());
    rig.clock.step_wall(-30.0 * 86_400.0); // a dead RTC battery, or an administrator
    rig.drive(2, 45, 45, 5.0);
    assert_eq!(rig.state.get_f64("profile_hold_until", 1.0), 0.0, "the hold is dropped");
    assert!(rig.log.has("the clock jumped back"));
}

#[test]
fn a_client_cannot_hold_a_power_mode_for_years() {
    let rig = Rig::new();
    let held = rig.ok("hold", args().with("minutes", 1.0e9)).as_f64().unwrap();
    assert_eq!(held, 7.0 * 24.0 * 60.0);
}

#[test]
fn a_clock_that_jumps_forward_ends_a_hold_and_a_charge_to_full_and_nothing_else() {
    let rig = Rig::new();
    rig.drive(6, 45, 45, 5.0);
    rig.ok("hold", args().with("minutes", 30));
    rig.sim.run("set_battery 1 1").unwrap();
    rig.ok("battery_full", args().with("action", "start"));
    assert_eq!(rig.snap().battery[0], 0);
    rig.clock.step_wall(2.0 * 86_400.0); // NTP after a long suspend with no RTC: the day is up
    rig.drive(6, 45, 45, 5.0);
    assert_eq!(rig.snap().battery[0], 1, "the 80 % limit is back (safe direction)");
    assert_eq!(rig.ok("hold", args().with("minutes", 0)), json!(0));
    // The fans still follow the temperature.
    rig.drive(40, 90, 70, 95.0);
    assert!(rig.snap().duty[0] >= 85);
}

#[test]
fn a_charge_to_full_stamped_in_the_future_ends_instead_of_lasting() {
    let rig = Rig::new();
    rig.drive(6, 45, 45, 5.0);
    rig.sim.run("set_battery 1 1").unwrap();
    rig.ok("battery_full", args().with("action", "start"));
    rig.clock.step_wall(-10.0 * 86_400.0);
    rig.drive(6, 45, 45, 5.0);
    assert_eq!(rig.snap().battery[0], 1, "the limit is back");
    assert_eq!(rig.state.get("charge_full"), Some(Value::Null));
}

#[test]
fn a_gpu_reading_stamped_in_the_future_is_not_fresh() {
    let rig = Rig::new();
    let future = rig.clock.wall_now() + 3_600.0;
    rig.sensors.set_gpu(Some(95.0), 50.0, 40.0, Some(future));
    rig.drive(5, 50, 45, 20.0);
    let l = live(&rig);
    assert!(
        l["tgpu"].as_i64().unwrap() < 60,
        "the firmware's 45 C decides, not the 95 C stamped tomorrow: {l}"
    );
}

// ---- suspend and resume ----------------------------------------------------------------------

#[test]
fn a_suspend_in_the_middle_of_any_command_of_a_tick_still_ends_with_the_fans_following_the_heat() {
    let baseline = Rig::new();
    baseline.drive(3, 88, 75, 90.0);
    let per_run = baseline.device.sent.lock().unwrap().len();
    assert!(per_run > 3, "{per_run} commands in three ticks");
    for at in 1..=per_run + 2 {
        let rig = Rig::new();
        let clock = rig.clock.clone();
        rig.device.on_command(move |n| {
            if n == at {
                clock.suspend(3_600.0);
            }
        });
        rig.drive(6, 88, 75, 90.0);
        assert!(rig.snap().duty[0] >= 60, "suspend at command {at}: duty {:?}", rig.snap().duty);
        assert_ne!(rig.snap().fan, SimFan::Auto, "suspend at command {at}: the smart fan still drives");
    }
}

#[test]
fn after_a_suspend_that_changed_the_firmware_state_the_fans_are_written_again() {
    let rig = Rig::new();
    rig.drive(10, 80, 70, 80.0);
    let before = rig.snap().duty;
    assert_ne!(before, [0, 0]);
    // The firmware came back from S3 on its own control while the clocks were stopped.
    rig.sim.run("set_fan_auto").unwrap();
    rig.clock.suspend(7_200.0);
    rig.drive(2, 80, 70, 80.0);
    assert_ne!(rig.snap().fan, SimFan::Auto, "written again at the first tick after the resume");
}

// ---- a crash in every step ---------------------------------------------------------------------

fn loop_with_stop_after(rig: &Rig, ticks: usize) -> Result<(), DaemonError> {
    rig.sim.set_sensors([90, 60, 75], [0, 0]);
    rig.sensors.set_load(90.0);
    rig.sleeper.stop_after(ticks);
    rig.daemon.run(rig.path("crash.sock"))
}

#[test]
fn a_panic_at_every_firmware_command_of_the_loop_costs_one_tick_and_ends_with_the_fans_on_auto() {
    let baseline = Rig::new();
    loop_with_stop_after(&baseline, 6).unwrap();
    let total = baseline.device.sent.lock().unwrap().len();
    assert!(total > 10, "{total} commands in a short run");
    for n in 1..=total {
        let rig = Rig::new();
        rig.device.panic_at.store(n, Ordering::SeqCst);
        let outcome = catch_unwind(AssertUnwindSafe(|| loop_with_stop_after(&rig, 6)));
        // A panic inside the hand-back itself unwinds out of `run`; the unit's ExecStopPost then
        // runs `truesense fan auto`, which is `shutdown` again here.
        if outcome.is_err() {
            rig.daemon.shutdown();
        }
        assert_eq!(rig.snap().fan, SimFan::Auto, "panic at firmware command {n}: fans must end on Auto");
        assert!(rig.cap.uncapped.load(Ordering::SeqCst) >= 1, "panic at command {n}: cap lifted");
        assert!(rig.call("ping", args()).is_ok(), "panic at command {n}: no lock left held");
        if let Ok(result) = outcome {
            assert!(
                result.is_ok(),
                "panic at command {n}: a panic in a tick must not end the daemon: {result:?}"
            );
            assert_eq!(rig.daemon.ticks(), 6, "panic at command {n}: the loop went on");
        }
    }
}

#[test]
fn a_panic_at_every_sensor_read_costs_one_tick_and_a_hot_machine_gets_full_fans() {
    let baseline = Rig::new();
    loop_with_stop_after(&baseline, 6).unwrap();
    let total = baseline.sensors.calls.load(Ordering::SeqCst);
    assert!(total >= 12, "{total} sensor calls");
    for n in 1..=total {
        let rig = Rig::new();
        rig.sensors.panic_at.store(n, Ordering::SeqCst);
        let result = loop_with_stop_after(&rig, 6);
        assert!(result.is_ok(), "panic at sensor call {n}: {result:?}");
        assert_eq!(rig.daemon.ticks(), 6, "panic at sensor call {n}: the loop went on");
        assert_eq!(rig.snap().fan, SimFan::Auto, "panic at sensor call {n}: ended on Auto");
        assert!(rig.log.has("step failed"), "panic at sensor call {n}: the journal says so");
    }
}

#[test]
fn a_hot_machine_whose_tick_panics_gets_the_fans_at_full_at_that_moment() {
    let rig = Rig::new();
    rig.drive(10, 90, 75, 90.0);
    let seen = Arc::new(Mutex::new(Vec::new()));
    {
        let (sim, seen) = (rig.sim.clone(), seen.clone());
        rig.sleeper.on_sleep(move |n| {
            if n == 1 {
                seen.lock().unwrap().push(sim.snapshot().duty);
            }
        });
    }
    rig.sensors.panic_at.store(rig.sensors.calls.load(Ordering::SeqCst) + 1, Ordering::SeqCst);
    rig.sensors.set_load(90.0);
    rig.sim.set_sensors([90, 60, 75], [0, 0]);
    rig.sleeper.stop_after(3);
    rig.daemon.run(rig.path("hot.sock")).unwrap();
    assert!(rig.log.has("step failed"));
    assert_eq!(seen.lock().unwrap()[0], [100, 100], "the step that failed put the fans on full");
}

// ---- SIGTERM and SIGKILL ---------------------------------------------------------------------

#[test]
fn a_stop_during_a_fan_off_episode_returns_the_fans_to_auto_before_the_daemon_ends() {
    let rig = Rig::new();
    rig.drive(30, 45, 45, 2.0);
    assert_eq!(rig.ok("fan", args().with("mode", "off").with("confirm", true)), json!("off"));
    assert_eq!(rig.snap().duty, [0, 0]);
    let at_stop = Arc::new(Mutex::new(None));
    {
        let (sim, at_stop, stop) = (rig.sim.clone(), at_stop.clone(), rig.stop.clone());
        rig.sleeper.on_sleep(move |n| {
            if n == 2 {
                *at_stop.lock().unwrap() = Some(sim.snapshot().duty);
                stop.request(); // what SIGTERM does
            }
        });
    }
    rig.daemon.run(rig.path("off.sock")).unwrap();
    assert_eq!(at_stop.lock().unwrap().unwrap(), [0, 0], "they were stopped when the signal came");
    assert_eq!(rig.snap().fan, SimFan::Auto, "and are on the firmware's control when the daemon is gone");
}

/// The child half of the signal tests: a whole simulated daemon in its own process, with the
/// real signal handling of `assembly`. Does nothing unless the parent test started it.
#[test]
fn child_daemon_for_the_signal_tests() {
    let Ok(socket) = std::env::var("TS_CHILD_SOCKET") else { return };
    let rig = Rig::new();
    crate::assembly::install_signal_stop(rig.stop.clone()).unwrap();
    rig.ok_fan_manual(70);
    let sim = rig.sim.clone();
    let announced = AtomicBool::new(false);
    rig.sleeper.on_sleep(move |_| {
        if !announced.swap(true, Ordering::SeqCst) {
            println!("CHILD-LOOP pid={}", std::process::id());
            println!("CHILD-FAN-BEFORE {:?}", sim.snapshot().fan);
        }
        std::thread::sleep(Duration::from_millis(20));
    });
    let result = rig.daemon.run(&socket);
    println!("CHILD-RESULT {}", if result.is_ok() { "ok" } else { "error" });
    println!("CHILD-FAN-AFTER {:?}", rig.snap().fan);
    println!("CHILD-CAP-LIFTED {}", rig.cap.uncapped.load(Ordering::SeqCst) >= 1);
    println!("CHILD-SOCKET-GONE {}", !Path::new(&socket).exists());
}

fn start_child(socket: &Path) -> (std::process::Child, BufReader<std::process::ChildStdout>, u32) {
    let exe = std::env::current_exe().unwrap();
    let mut command = Command::new(exe);
    crate::assembly::block_termination_signals_in_child(&mut command);
    let mut child = command
        .args([
            "--exact",
            "daemon::resilience_tests::child_daemon_for_the_signal_tests",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("TS_CHILD_SOCKET", socket)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    // A child that hangs must fail the test, not the whole run.
    let pid = child.id();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(30));
        let _ = Command::new("kill").args(["-KILL", &pid.to_string()]).status();
    });
    let mut line = String::new();
    let pid = loop {
        line.clear();
        assert!(out.read_line(&mut line).unwrap() > 0, "the child ended before its loop ran");
        // libtest prints "test <name> ... " on the same line before the child's own output.
        if let Some(at) = line.find("CHILD-LOOP pid=") {
            break line[at + "CHILD-LOOP pid=".len()..].trim().parse().unwrap();
        }
    };
    (child, out, pid)
}

#[test]
fn a_real_sigterm_makes_the_daemon_hand_everything_back_and_exit_cleanly() {
    let dir = super::testkit::TempDir::new();
    let socket = dir.join("child.sock");
    let (mut child, mut out, pid) = start_child(&socket);
    assert!(socket.exists(), "serving");
    assert!(Command::new("kill").args(["-TERM", &pid.to_string()]).status().unwrap().success());
    let mut rest = String::new();
    std::io::Read::read_to_string(&mut out, &mut rest).unwrap();
    let status = child.wait().unwrap();
    assert!(status.success(), "exit status {status:?}: {rest}");
    for want in ["CHILD-RESULT ok", "CHILD-FAN-AFTER Auto", "CHILD-CAP-LIFTED true", "CHILD-SOCKET-GONE true"]
    {
        assert!(rest.contains(want), "missing '{want}' in:\n{rest}");
    }
}

#[test]
fn after_a_real_sigkill_the_next_daemon_starts_at_once_over_the_stale_socket_and_lock() {
    let dir = super::testkit::TempDir::new();
    let socket = dir.join("child.sock");
    let (mut child, _out, _pid) = start_child(&socket);
    child.kill().unwrap(); // SIGKILL: no handler runs, nothing is cleaned up
    child.wait().unwrap();
    assert!(socket.exists(), "the killed daemon left its socket file");
    assert!(lock_path(&socket).exists(), "and its lock file");
    let rig = Rig::new();
    rig.sleeper.stop_after(2);
    rig.daemon.run(&socket).expect("the restart is not blocked by what the dead daemon left");
    assert_eq!(rig.daemon.ticks(), 2);
}
