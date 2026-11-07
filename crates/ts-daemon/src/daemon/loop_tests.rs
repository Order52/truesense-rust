//! The control loop and `run`: the pace, the watchdog pings, `READY`, `STOPPING`, the hand-back on
//! every way out, and the control socket's life cycle, all on a fake clock with no real waiting.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::os::unix::net::UnixDatagram;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde_json::json;
use ts_foundation::sdnotify::Notifier;
use ts_ipc::client::Client;
use ts_ipc::wire::Args;
use ts_platform::firmware::SimFan;

use super::DaemonError;
use super::testkit::{Rig, Setup, TempDir};

fn args() -> Args {
    Args::new()
}

/// A rig whose systemd notifications arrive on a socket the test can read.
fn notified(watchdog_every: Option<Duration>) -> (Rig, UnixDatagram, TempDir) {
    let dir = TempDir::new();
    let path = dir.join("notify");
    let socket = UnixDatagram::bind(&path).unwrap();
    socket.set_nonblocking(true).unwrap();
    let setup = Setup {
        notifier: Notifier::to_address(path.to_str().unwrap()).unwrap(),
        watchdog_every,
        ..Setup::default()
    };
    (Rig::with(setup), socket, dir)
}

fn messages(socket: &UnixDatagram) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = [0u8; 64];
    while let Ok(n) = socket.recv(&mut buf) {
        out.push(String::from_utf8_lossy(&buf[..n]).into_owned());
    }
    out
}

#[test]
fn run_serves_the_socket_tells_systemd_and_hands_everything_back() {
    let (rig, notify, _keep) = notified(None);
    rig.sensors.set_load(5.0);
    rig.sim.set_sensors([45, 55, 45], [0, 0]);
    let sock = rig.path("s.sock");
    let client = Client::new(&sock).with_timeout(Duration::from_secs(3));
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen_in = Arc::clone(&seen);
    rig.sleeper.stop_after(4);
    // Between two ticks a client talks to the running daemon.
    rig.sleeper.on_sleep(move |n| {
        if n == 2 {
            let mut seen = seen_in.lock().unwrap();
            seen.push(client.call("ping", args()).unwrap());
            seen.push(client.call("fan", args().with("mode", "manual").with("cpu", 60)).unwrap());
        }
    });
    rig.daemon.run(&sock).unwrap();
    assert_eq!(*seen.lock().unwrap(), [json!("pong"), json!("manual")]);
    assert_eq!(rig.snap().fan, SimFan::Auto, "leaving hands the fans back");
    assert!(rig.cap.uncapped.load(Ordering::SeqCst) >= 1);
    assert!(!sock.exists(), "the socket is removed");
    assert_eq!(rig.daemon.ticks(), 4);
    let sent = messages(&notify);
    assert_eq!(sent.first().map(String::as_str), Some("READY=1"), "{sent:?}");
    assert_eq!(sent.iter().filter(|m| *m == "WATCHDOG=1").count(), 4, "one ping per tick: {sent:?}");
    assert_eq!(sent.iter().filter(|m| *m == "STOPPING=1").count(), 1, "{sent:?}");
    assert_eq!(sent.last().map(String::as_str), Some("STOPPING=1"));
    assert!(rig.state.get("uv_applied").is_some(), "boot ran");
}

#[test]
fn the_pace_follows_the_work_and_never_really_sleeps() {
    let rig = Rig::new();
    rig.sensors.set_load(2.0);
    rig.sim.set_sensors([45, 55, 45], [0, 0]);
    rig.auto_mode("native");
    rig.ok_set_smart_fan(false);
    rig.sleeper.stop_after(3);
    rig.daemon.run(rig.path("s.sock")).unwrap();
    assert_eq!(
        *rig.sleeper.last.lock().unwrap(),
        Duration::from_secs(2),
        "hands off: two seconds between ticks"
    );
    let rig = Rig::new();
    rig.sensors.set_load(50.0);
    rig.sim.set_sensors([60, 55, 55], [0, 0]);
    rig.sleeper.stop_after(3);
    rig.daemon.run(rig.path("s.sock")).unwrap();
    assert_eq!(
        *rig.sleeper.last.lock().unwrap(),
        Duration::from_secs(1),
        "something smart is working: a second"
    );
}

#[test]
fn the_watchdog_is_pinged_only_as_often_as_systemd_wants() {
    let (rig, notify, _keep) = notified(Some(Duration::from_secs(10)));
    rig.sensors.set_load(50.0);
    rig.sim.set_sensors([60, 55, 55], [0, 0]);
    rig.sleeper.stop_after(25);
    rig.daemon.run(rig.path("s.sock")).unwrap();
    let pings = messages(&notify).iter().filter(|m| *m == "WATCHDOG=1").count();
    assert!((2..=4).contains(&pings), "25 one-second ticks, an interval of 10 s: {pings} pings");
}

#[test]
fn a_loop_that_wakes_late_resyncs_instead_of_racing_to_catch_up() {
    let rig = Rig::new();
    rig.sensors.set_load(50.0);
    rig.sim.set_sensors([60, 55, 55], [0, 0]);
    rig.sleeper.stop_after(3);
    let clock = Arc::clone(&rig.clock);
    rig.sleeper.on_sleep(move |n| {
        if n == 1 {
            clock.advance(3600.0); // the machine slept for an hour in the middle of the sleep
        }
    });
    rig.daemon.run(rig.path("s.sock")).unwrap();
    assert_eq!(rig.daemon.ticks(), 3, "three ticks, not thousands");
    assert!(rig.sleeper.last.lock().unwrap().as_secs() <= 2, "the schedule restarted from now");
}

#[test]
fn a_panic_outside_the_guarded_tick_still_hands_the_machine_back() {
    let rig = Rig::new();
    rig.ok_fan_manual(70);
    rig.sleeper.on_sleep(|_| panic!("the pause blew up"));
    let outcome = rig.daemon.run(rig.path("s.sock"));
    match outcome {
        Err(DaemonError::Panicked(why)) => assert!(why.contains("the pause blew up"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(rig.snap().fan, SimFan::Auto, "a panic leaves the fans on the firmware");
    assert!(rig.cap.uncapped.load(Ordering::SeqCst) >= 1);
    assert!(!rig.path("s.sock").exists());
}

#[test]
fn a_stop_requested_before_the_loop_starts_ends_it_cleanly() {
    let (rig, notify, _keep) = notified(None);
    rig.stop.request();
    rig.daemon.run(rig.path("s.sock")).unwrap();
    assert_eq!(rig.daemon.ticks(), 0);
    let sent = messages(&notify);
    assert_eq!(sent, ["READY=1", "STOPPING=1"]);
}

#[test]
fn a_socket_that_cannot_start_is_an_error_before_anything_is_touched() {
    let rig = Rig::new();
    std::fs::write(rig.path("s.sock"), "not a socket").unwrap();
    rig.ok_fan_manual(70);
    let e = rig.daemon.run(rig.path("s.sock")).unwrap_err();
    assert!(matches!(e, DaemonError::Socket(_)), "{e}");
    assert!(e.to_string().contains("is not a socket"), "{e}");
    assert_eq!(rig.snap().fan, SimFan::Manual, "nothing ran, nothing was handed back");
    assert!(rig.path("s.sock").exists(), "a file that is not ours is never deleted");
}

#[test]
fn the_real_sleeper_wakes_up_at_once_when_asked_to_stop() {
    use super::deps::{RealSleeper, Sleeper, StopToken};
    let stop = StopToken::new();
    assert!(!RealSleeper.sleep(Duration::from_millis(1), &stop), "the time ran out");
    stop.request();
    assert!(
        RealSleeper.sleep(Duration::from_secs(3600), &stop),
        "an hour is cut short by a stop that is already there"
    );
    let late = StopToken::new();
    let clone = late.clone();
    let waker = std::thread::spawn(move || clone.request());
    assert!(late.wait_timeout(Duration::from_secs(30)), "woken by the request");
    waker.join().unwrap();
}
