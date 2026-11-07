//! Smart auto must not flap (the Python `TestSmartAutoSteadiness`): a CPU sitting on the Turbo
//! temperature line, a menu in the middle of a game, the length of a tick, the `steady` profile,
//! and the journal line for every change.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};
use ts_ipc::wire::Args;

use super::testkit::Rig;

fn args() -> Args {
    Args::new()
}

/// Steps the service for `seconds` of game time and returns the `(time, mode)` of every change of
/// the power mode. A GPU at 98 % and 71 W is a game.
fn play(
    rig: &Rig,
    seconds: f64,
    cpu_of: impl Fn(f64) -> f64,
    load: f64,
    gpu_util: f64,
    gpu_power: f64,
    dt: f64,
) -> Vec<(i64, u8)> {
    let mut changes = Vec::new();
    let mut last = rig.snap().mode;
    let mut t = 0.0;
    while t < seconds {
        rig.sim.set_sensors([cpu_of(t).round() as u32, 55, 65], [0, 0]);
        rig.sensors.set_load(load);
        rig.sensors.set_gpu(None, gpu_util, gpu_power, None);
        rig.daemon.guarded_step();
        rig.clock.advance(dt);
        t += dt;
        if rig.snap().mode != last {
            last = rig.snap().mode;
            changes.push((rig.clock_mono().round() as i64, last));
        }
    }
    changes
}

fn game(rig: &Rig, seconds: f64, cpu_of: impl Fn(f64) -> f64) -> Vec<(i64, u8)> {
    play(rig, seconds, cpu_of, 16.0, 98.0, 71.0, 1.0)
}

/// Smart auto on AC after 70 quiet seconds, optionally with a named profile.
fn smart(profile: Option<&str>) -> Rig {
    let rig = Rig::new();
    rig.auto_mode("smart");
    if let Some(name) = profile {
        rig.ok("smart_profile", args().with("action", "use").with("name", name));
    }
    rig.drive(70, 40, 40, 5.0);
    assert_eq!(rig.snap().mode, 1);
    rig
}

fn reason(rig: &Rig) -> String {
    rig.ok("state", args())["live"]["mode_reason"].as_str().unwrap().to_owned()
}

#[test]
fn a_cpu_sitting_on_the_turbo_line_does_not_make_the_mode_flap() {
    let rig = smart(None);
    // The real session: GPU at 98 %, CPU between 80 and 89 C for minutes (the old logic went
    // Turbo, Performance, Turbo ... every few seconds at 85 C).
    let changes = game(&rig, 900.0, |t| 84.5 + 4.5 * (t / 9.0).sin());
    assert!(changes.len() <= 2, "mode changes during 15 minutes at the Turbo line: {changes:?}");
    assert_eq!(rig.snap().mode, 4, "it settles on Performance");
}

#[test]
fn turbo_is_entered_well_below_its_limit_and_left_only_at_it_with_a_long_retry_pause() {
    let rig = smart(None);
    let changes = game(&rig, 200.0, |_| 70.0);
    assert_eq!(rig.snap().mode, 5, "{changes:?}"); // after 30 s of heavy load and the 45 s gap
    game(&rig, 20.0, |_| 86.0);
    assert_eq!(rig.snap().mode, 4, "at the limit it steps down at once");
    let t_down = rig.clock_mono();
    game(&rig, 120.0, |_| 70.0);
    assert_eq!(rig.snap().mode, 4, "and does not climb back for 180 s even though it is cool again");
    game(&rig, 120.0, |_| 70.0);
    assert_eq!(rig.snap().mode, 5, "after the pause it may try again");
    assert!(rig.clock_mono() - t_down >= 180.0);
    game(&rig, 60.0, |_| 82.0);
    assert_eq!(
        rig.snap().mode,
        5,
        "82 C is under the limit: it stays (the old logic left Turbo near its entry temperature)"
    );
}

#[test]
fn a_quiet_moment_in_a_game_keeps_the_mode() {
    let rig = smart(None);
    game(&rig, 40.0, |_| 70.0);
    assert_eq!(
        rig.snap().mode,
        4,
        "40 s of game: Performance (Turbo would need 30 s of load and a 45 s gap)"
    );
    let quiet = play(&rig, 45.0, |_| 70.0, 20.0, 15.0, 18.0, 1.0); // a menu or a loading screen
    assert_eq!((quiet, rig.snap().mode), (vec![], 4), "a 45 s menu does not drop the mode");
    assert!(reason(&rig).contains("holding the busy mode"), "{}", reason(&rig));
    play(&rig, 400.0, |_| 50.0, 3.0, 2.0, 5.0, 1.0);
    assert!(matches!(rig.snap().mode, 1 | 2), "really quiet for minutes: it does come back down");
}

#[test]
fn a_quiet_moment_never_starts_turbo() {
    let rig = smart(None);
    game(&rig, 20.0, |_| 70.0); // busy for 20 s only (Turbo needs 30)
    play(&rig, 120.0, |_| 70.0, 20.0, 15.0, 18.0, 1.0);
    assert_ne!(rig.snap().mode, 5, "a held quiet moment keeps the current mode, it never raises it");
}

#[test]
fn the_steady_profile_never_enters_turbo_and_changes_rarely() {
    let rig = smart(Some("steady"));
    let changes = game(&rig, 900.0, |t| 70.0 + 8.0 * (t / 15.0).sin());
    assert!(!changes.iter().any(|(_, m)| *m == 5), "no Turbo with the steady profile: {changes:?}");
    assert!(changes.len() <= 1, "{changes:?}");
}

#[test]
fn timers_are_real_seconds_whatever_the_tick_length() {
    for dt in [1.0, 2.0] {
        let rig = smart(None);
        let changes = play(&rig, 140.0, |_| 70.0, 16.0, 98.0, 71.0, dt);
        let went_turbo: Vec<i64> = changes.iter().filter(|(_, m)| *m == 5).map(|(t, _)| *t).collect();
        assert!(!went_turbo.is_empty(), "{dt} {changes:?}");
        assert!(went_turbo[0] >= 70 + 30, "Turbo not before 30 s of heavy load (dt={dt}): {changes:?}"); // the game starts at t=70
    }
}

#[test]
fn every_automatic_change_is_in_the_journal_with_its_reason() {
    let rig = smart(None);
    game(&rig, 30.0, |_| 70.0);
    let out = rig.log.all();
    assert!(out.contains("auto power mode: balanced -> performance"), "{out}");
    assert!(out.contains("GPU 98 %"), "{out}");
    assert!(out.contains("smart auto: gaming"), "{out}");
    let line = out.lines().find(|l| l.contains("-> performance")).unwrap();
    assert!(line.contains(" C, GPU ") && line.contains("; CPU "), "{line}");
    assert_eq!(rig.ok("state", args())["live"]["mode_reason"], Value::from(reason(&rig)));
    assert_eq!(json!(reason(&rig).starts_with("gaming, on power")), json!(true));
}
