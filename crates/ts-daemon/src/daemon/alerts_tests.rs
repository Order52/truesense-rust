//! The daemon's alerts on the fake laptop: what reaches the on-screen display, what the journal
//! says, how the settings are picked up while it runs, and that a broken notifier or a broken
//! settings file never touches the fans or the tick. The rules themselves are tested in
//! `ts_decision::alerts`; the clock here only moves when a test says so.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::sync::atomic::Ordering;

use ts_foundation::config::AlertsConfig;
use ts_platform::firmware::SimFan;

use super::alerts::{AlertSettings, ConfigFileAlerts};
use super::testkit::{FakeUsers, Rig, Setup, TempDir, my_uid};

/// A rig whose `[alerts]` section is the default one changed by `f`. The throttling alert is off
/// unless a test asks for it, because a hot CPU makes the governor step in after a while and its
/// message would join every long test. The ordinary on-screen messages (`[profile] osd`) are off,
/// which also shows that alerts do not depend on that setting.
fn rig_with(f: impl FnOnce(&mut AlertsConfig)) -> Rig {
    let mut setup = Setup::default();
    setup.config.profile.osd = false;
    setup.config.alerts.throttling = false;
    f(&mut setup.config.alerts);
    Rig::with(setup)
}

/// What the on-screen display was given, after the helper thread finished.
fn shown(rig: &Rig) -> Vec<String> {
    rig.daemon.join_background();
    rig.osd.shown.lock().unwrap().clone()
}

fn alert_lines(rig: &Rig) -> Vec<String> {
    rig.log.lines.lock().unwrap().iter().filter(|l| l.starts_with("alert ")).cloned().collect()
}

#[test]
fn a_hot_cpu_is_announced_once_after_the_debounce_and_logged_once() {
    let rig = rig_with(|_| {});
    rig.drive(2, 95, 60, 50.0);
    assert_eq!(shown(&rig), Vec::<String>::new(), "two samples are not an alert");
    rig.drive(1, 95, 60, 50.0);
    assert_eq!(shown(&rig), ["CPU is hot: 95 C (alert limit 90 C)"]);
    rig.drive(100, 95, 60, 50.0);
    assert_eq!(shown(&rig).len(), 1, "still hot, still one message");
    assert_eq!(
        alert_lines(&rig),
        ["alert cpu-temp-high (warning): CPU is hot: 95 C (alert limit 90 C)"],
        "one journal line per episode"
    );
    assert_ne!(rig.snap().fan, SimFan::Auto, "the fans are still driven by the smart curve");
    // Cooling down 5 C below the limit ends the episode with a line of its own.
    rig.drive(10, 60, 60, 50.0);
    assert!(rig.log.has("alert cpu-temp-high is over"));
}

#[test]
fn a_hot_gpu_is_announced_and_a_missing_gpu_reading_is_not_a_hot_gpu() {
    let rig = rig_with(|_| {});
    rig.drive(3, 50, 95, 50.0);
    assert_eq!(shown(&rig), ["GPU is hot: 95 C (alert limit 90 C)"]);
    // Without a GPU reading the daemon lends the GPU the CPU's temperature for its fan curve; the
    // alert must not take that as a hot GPU. 70 s is past the one-minute gap, so a phantom GPU
    // alert would have shown up as a second message.
    let rig = rig_with(|_| {});
    rig.drive(70, 95, 0, 50.0);
    assert_eq!(shown(&rig), ["CPU is hot: 95 C (alert limit 90 C)"]);
}

#[test]
fn alerts_switched_off_say_nothing() {
    let rig = rig_with(|a| a.enabled = false);
    rig.drive(30, 99, 99, 90.0);
    assert_eq!(shown(&rig), Vec::<String>::new());
    assert_eq!(alert_lines(&rig), Vec::<String>::new());
}

#[test]
fn a_lost_cpu_sensor_is_announced() {
    let rig = rig_with(|_| {});
    rig.drive(3, 0, 0, 5.0);
    assert_eq!(shown(&rig), ["The CPU temperature cannot be read; the fans are left to the firmware"]);
    rig.drive(3, 50, 40, 5.0);
    assert!(rig.log.has("alert sensor-lost is over"));
}

#[test]
fn a_low_battery_is_announced_only_on_battery() {
    let rig = rig_with(|_| {});
    rig.sensors.set_battery(10.0);
    rig.drive(10, 50, 40, 5.0);
    assert_eq!(shown(&rig), Vec::<String>::new(), "on the charger a low level is nothing");
    rig.sensors.set_ac(false);
    rig.drive(10, 50, 40, 5.0);
    assert_eq!(shown(&rig), ["Battery low: 10 %, plug in the charger"]);
    rig.sensors.set_ac(true);
    rig.drive(2, 50, 40, 5.0);
    assert!(rig.log.has("alert battery-low is over"));
}

#[test]
fn a_governor_holding_the_clock_down_is_an_info_alert_that_waits_its_turn() {
    let rig = rig_with(|a| a.throttling = true);
    rig.drive(30, 92, 60, 99.0);
    assert_eq!(shown(&rig), ["CPU is hot: 92 C (alert limit 90 C)"], "one message a minute across alerts");
    rig.drive(60, 92, 60, 99.0);
    let all = shown(&rig);
    assert_eq!(all.len(), 2, "{all:?}");
    assert_eq!(all[1], "The CPU clock is held down to keep the laptop cool");
    assert!(rig.log.has("alert throttling (info): "), "{}", rig.log.all());
}

#[test]
fn a_notifier_that_panics_costs_a_journal_line_and_nothing_else() {
    let rig = rig_with(|_| {});
    rig.osd.explode.store(true, Ordering::SeqCst);
    rig.drive(10, 95, 60, 50.0);
    assert_eq!(shown(&rig), Vec::<String>::new());
    assert!(rig.log.has("the notifier failed for alert cpu-temp-high"), "{}", rig.log.all());
    assert!(!rig.log.has("step failed"), "the tick itself did not fail");
    assert_eq!(rig.daemon.ticks(), 10);
    assert_ne!(rig.snap().fan, SimFan::Auto, "fan control went on");
    // The notifier comes back: the next episode is announced normally.
    rig.osd.explode.store(false, Ordering::SeqCst);
    rig.drive(10, 50, 40, 5.0);
    rig.drive(400, 50, 40, 5.0);
    rig.drive(12, 95, 60, 50.0);
    assert_eq!(shown(&rig).len(), 1);
}

#[test]
fn settings_changes_are_picked_up_within_ten_seconds() {
    let rig = rig_with(|_| {});
    rig.drive(15, 80, 60, 50.0);
    assert_eq!(shown(&rig), Vec::<String>::new(), "80 C is under the default limit of 90");
    let lowered = AlertsConfig { cpu_limit_c: 75, throttling: false, ..AlertsConfig::default() };
    rig.alert_settings.set(Ok(lowered));
    // The files are looked at every ten seconds (ticks 11, 21, ...): the change is seen at tick 21
    // and three more samples make the alert.
    rig.drive(5, 80, 60, 50.0);
    assert_eq!(shown(&rig), Vec::<String>::new(), "not looked at yet");
    rig.drive(6, 80, 60, 50.0);
    assert_eq!(shown(&rig), ["CPU is hot: 80 C (alert limit 75 C)"]);
    assert!(rig.log.has("alert settings reloaded"));
    // Switching them off ends the episode and silences everything.
    let off = AlertsConfig { enabled: false, ..AlertsConfig::default() };
    rig.alert_settings.set(Ok(off));
    rig.drive(15, 99, 60, 50.0);
    assert_eq!(shown(&rig).len(), 1);
}

#[test]
fn settings_that_cannot_be_loaded_keep_the_current_ones_and_say_so_once() {
    let rig = rig_with(|_| {});
    rig.drive(1, 50, 40, 5.0);
    rig.alert_settings.set(Err("config.toml: expected a number (line 3, column 1)".to_owned()));
    rig.drive(30, 50, 40, 5.0);
    let warnings: Vec<String> =
        rig.log.lines.lock().unwrap().iter().filter(|l| l.contains("not reloaded")).cloned().collect();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("keeping the current ones") && warnings[0].contains("line 3"));
    // The limit of 90 is still in force (the smoothed temperature needs a few ticks to get there).
    rig.drive(12, 95, 60, 50.0);
    let said = shown(&rig);
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].starts_with("CPU is hot: ") && said[0].ends_with("(alert limit 90 C)"), "{said:?}");
}

// ---- the settings files ------------------------------------------------------------------------

fn files() -> (TempDir, ConfigFileAlerts) {
    let dir = TempDir::new();
    let source = ConfigFileAlerts::new(Some(dir.join("etc.toml")), Some(dir.join("user.toml")));
    (dir, source)
}

#[test]
fn the_files_are_layered_and_a_missing_one_is_the_defaults() {
    let (dir, source) = files();
    assert_eq!(source.load().unwrap(), AlertsConfig::default());
    fs::write(dir.join("etc.toml"), "[alerts]\ncpu_limit_c = 85\nrepeat_minutes = 20\n").unwrap();
    fs::write(dir.join("user.toml"), "[alerts]\ncpu_limit_c = 80\n").unwrap();
    let got = source.load().unwrap();
    assert_eq!(
        (got.cpu_limit_c, got.repeat_minutes, got.gpu_limit_c),
        (80, 20, 90),
        "the user's file wins per key"
    );
}

#[test]
fn a_file_with_mistakes_is_an_error_naming_them() {
    let (dir, source) = files();
    fs::write(dir.join("user.toml"), "[alerts]\ncpu_limit_c = 50\ndebounce_samples = 0\n").unwrap();
    let why = source.load().unwrap_err();
    assert!(why.contains("[alerts] cpu_limit_c must be 70..100"), "{why}");
    assert!(why.contains("[alerts] debounce_samples must be 1..30"), "{why}");
    fs::write(dir.join("user.toml"), "[alerts\n").unwrap();
    let why = source.load().unwrap_err();
    assert!(why.contains("user.toml") && why.contains("line 1"), "{why}");
    fs::write(dir.join("user.toml"), "[alerts]\ncpu_limit_c = \"hot\"\n").unwrap();
    assert!(source.load().is_err());
}

#[test]
fn the_stamp_moves_when_a_file_appears_changes_or_goes() {
    let (dir, source) = files();
    let none = source.stamp();
    assert_eq!(source.stamp(), none, "stable while nothing changes");
    fs::write(dir.join("user.toml"), "[alerts]\nenabled = false\n").unwrap();
    let one = source.stamp();
    assert_ne!(one, none);
    fs::write(dir.join("user.toml"), "[alerts]\nenabled = true\ncpu_limit_c = 71\n").unwrap();
    let two = source.stamp();
    assert_ne!(two, one, "a different size moves it even inside one timestamp tick");
    fs::write(dir.join("etc.toml"), "").unwrap();
    assert_ne!(source.stamp(), two);
    fs::remove_file(dir.join("user.toml")).unwrap();
    fs::remove_file(dir.join("etc.toml")).unwrap();
    assert_eq!(source.stamp(), none);
}

#[test]
fn the_desktop_users_file_is_found_through_the_user_directory() {
    let users = FakeUsers { uid: my_uid(), home: "/home/tester".into() };
    let none = |_: &str| None;
    let at = |source: &ConfigFileAlerts| format!("{source:?}");
    let source =
        ConfigFileAlerts::for_desktop_user("/etc/truesense/config.toml".as_ref(), &users, "tester", &none);
    assert!(at(&source).contains("/home/tester/.config/truesense/config.toml"), "{}", at(&source));
    assert!(at(&source).contains("/etc/truesense/config.toml"));
    // TRUESENSE_CONFIG wins over the directory, HOME is the fallback for an unknown user.
    let over = |k: &str| (k == "TRUESENSE_CONFIG").then(|| "/x/c.toml".to_owned());
    let source = ConfigFileAlerts::for_desktop_user("/etc/t.toml".as_ref(), &users, "tester", &over);
    assert!(at(&source).contains("/x/c.toml") && !at(&source).contains("/home/tester"), "{}", at(&source));
    let home = |k: &str| (k == "HOME").then(|| "/home/other".to_owned());
    let source = ConfigFileAlerts::for_desktop_user("/etc/t.toml".as_ref(), &users, "nobody", &home);
    assert!(at(&source).contains("/home/other/.config/truesense/config.toml"), "{}", at(&source));
    let source = ConfigFileAlerts::for_desktop_user("/etc/t.toml".as_ref(), &users, "nobody", &none);
    assert!(at(&source).contains("user: None"), "{}", at(&source));
}
