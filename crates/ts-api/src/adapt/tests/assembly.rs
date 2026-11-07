//! The assembly itself: where the stores live, how the real machine value behaves with no daemon,
//! and the key actions, the whole-setup profiles, the health check, the service check and the
//! self-test running through the real service.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ts_platform::firmware::SimFan;
use ts_system::osd::{Invocation, Osd, Outcome as OsdOutcome, PasswdFile, SessionRunner};
use ts_system::selftest::{self, Deps};

use super::super::osd::LiveOsd;
use super::super::selftest::{SelftestAudio, SelftestDaemon, SelftestKeyboard};
use super::super::{Machine, ShellSpawner};
use super::rig::{Rig, put};
use crate::api::{Api, ApiError, ConfigFiles, FanRoute, Notifier, Refusal, Services};

fn home_has(rig: &Rig, relative: &str) -> bool {
    rig.home.join(relative).exists()
}

// ---- where everything lives --------------------------------------------------------------------

#[test]
fn every_store_lives_where_the_python_edition_kept_it() {
    let rig = Rig::new();
    let api = &rig.api;
    api.save_fan_curve(
        "mine",
        &crate::api::CurveEdit { from: Some("truesense".to_owned()), ..Default::default() },
    )
    .unwrap();
    api.save_smart_profile("mine", Some("truesense"), &[]).unwrap();
    api.save_cpu_undervolt("mine", Some(-30), Some(-20)).unwrap();
    api.save_gpu_undervolt("mine", Some(100), 2200).unwrap();
    api.save_look_value("mine", &api.look("native").unwrap()).unwrap();
    api.save_lighting_profile_cells("mine", Some(&["ff0000".to_owned()])).unwrap();
    api.copy_key_profile("native", Some("mine")).unwrap();
    api.save_profile("mine", "a test").unwrap();
    api.save_design_cells("mine", Some(&["00ff00".to_owned()])).unwrap();
    for file in [
        ".config/truesense/curves/mine.json",
        ".config/truesense/smart/mine.json",
        ".config/truesense/undervolt/mine.json",
        ".config/truesense/gpu-undervolt/mine.json",
        ".config/truesense/looks/mine.json",
        ".config/truesense/lighting/mine.json",
        ".config/truesense/keys/mine.json",
        ".config/truesense/profiles/mine.toml",
        ".config/truesense/designs/mine.json",
    ] {
        assert!(home_has(&rig, file), "{file} was not written");
    }
}

#[test]
fn the_user_state_the_configuration_and_the_system_file_follow_the_environment() {
    let rig = Rig::new();
    let s = rig.api.services();
    assert_eq!(s.user.path(), rig.dir.0.join("user.json"));
    assert_eq!(s.config.user, Some(rig.home.join(".config/truesense/config.toml")));
    assert_eq!(s.config.system, Some(rig.root.join("etc/truesense/config.toml")));
    rig.api.set_voice_notify(true).unwrap();
    assert!(rig.home.join(".config/truesense/config.toml").exists());
    assert!(rig.api.load_config().unwrap().audio.notify);
    // The autostart entry is in the user's directory too.
    assert!(rig.api.set_autostart(true).unwrap());
    assert!(home_has(&rig, ".config/autostart/truesense.desktop"));
    assert_eq!(s.euid, std::os::unix::fs::MetadataExt::uid(&std::fs::metadata(&rig.dir.0).unwrap()));
}

#[test]
fn the_repository_comes_from_the_system_file_and_never_from_the_users() {
    let rig = Rig::new();
    assert_eq!(rig.api.update_repo(), "Order52/truesense");
    put(&rig.root.join("etc/truesense/config.toml"), "[update]\nrepo = \"someone/fork\"\n");
    put(&rig.home.join(".config/truesense/config.toml"), "[update]\nrepo = \"evil/fork\"\n");
    let api = Api::new(Services::assemble(&rig.machine).unwrap());
    assert_eq!(api.update_repo(), "someone/fork");
    // A broken system file falls back to the built-in repository.
    put(&rig.root.join("etc/truesense/config.toml"), "[update\nrepo = ");
    let api = Api::new(Services::assemble(&rig.machine).unwrap());
    assert_eq!(api.update_repo(), "Order52/truesense");
}

#[test]
fn the_updater_asks_curl_and_nothing_else_so_a_dead_network_is_a_status() {
    let rig = Rig::new();
    // The runner of the machine starts nothing: the lookup fails, and says so.
    let status = rig.api.check_update(true);
    assert!(!status.available);
    assert!(status.error.is_some(), "{status:?}");
}

// ---- the real machine value with no daemon -----------------------------------------------------

#[test]
fn the_real_machine_over_the_simulator_falls_back_safely_without_a_daemon() {
    let tmp = super::super::testkit::TempDir::new();
    let env: BTreeMap<&str, String> = BTreeMap::from([
        ("HOME", tmp.0.join("home").display().to_string()),
        ("TRUESENSE_SIM", "1".to_owned()),
        ("TRUESENSE_SOCKET", tmp.0.join("none/truesense.sock").display().to_string()),
        ("TRUESENSE_USERSTATE", tmp.0.join("user.json").display().to_string()),
        ("TRUESENSE_OSD", "none".to_owned()),
    ]);
    // Everything of `Machine::real()` (the real runner, the real keyboard path, the real clock)
    // except the environment, so that nothing leaves the temporary directory.
    let mut machine = Machine::real();
    machine.env = Arc::new(move |k| env.get(k).cloned());
    let api = Api::new(Services::assemble(&machine).unwrap());
    assert!(!api.daemon_available());
    // The firmware is the simulator.
    assert_eq!(api.fan_status().unwrap().state.mode.to_string(), "auto");
    let out = api.fan_max().unwrap();
    assert_eq!(out.route, FanRoute::Hardware);
    assert_eq!(api.fan_status().unwrap().state.mode.to_string(), "max");
    api.fan_auto().unwrap();
    // The rules that need the service hold without it.
    assert_eq!(api.fan_smart().unwrap_err().refusal(), Some(&Refusal::SmartNeedsService));
    assert_eq!(api.fan_off(true).unwrap_err().refusal(), Some(&Refusal::OffNeedsService));
    assert_eq!(api.fan_set(0, None, true).unwrap_err().refusal(), Some(&Refusal::StopNeedsService));
    assert_eq!(api.fan_status().unwrap().state.mode.to_string(), "auto", "nothing was stopped");
    assert!(api.daemon_state().unwrap_err().daemon_missing());
    assert!(!api.daemon_responds());
    // The memory file is the one the environment names.
    api.set_battery_limit(true).unwrap();
    assert!(api.battery_limit().unwrap());
}

// ---- the key actions, end to end ---------------------------------------------------------------

#[test]
fn power_actions_set_the_mode_and_hold_it_with_the_configured_minutes() {
    let rig = Rig::new();
    assert_eq!(rig.api.test_action("power:turbo").unwrap(), "Power mode: turbo");
    assert_eq!(rig.sim.snapshot().mode, 5);
    let left = rig.api.daemon_state().unwrap().hold_left_s;
    // `[profile] hold_minutes` is 10 by default.
    assert!((500..=600).contains(&left), "{left}");
    // From turbo the cycle eco, quiet, balanced, performance, turbo wraps to eco.
    assert_eq!(rig.api.test_action("power-next").unwrap(), "Power mode: eco");
    assert_eq!(rig.sim.snapshot().mode, 6);
    assert_eq!(rig.api.test_action("power-prev").unwrap(), "Power mode: turbo");
    assert_eq!(rig.api.test_action("power-cycle:quiet,balanced").unwrap(), "Power mode: quiet");
    assert_eq!(rig.api.next_power().unwrap(), "Power mode: balanced");
}

#[test]
fn fan_actions_go_through_the_service_and_never_stop_a_fan() {
    let rig = Rig::new();
    assert_eq!(rig.api.test_action("fan:max").unwrap(), "Fans: MAX");
    assert_eq!(rig.sim.snapshot().fan, SimFan::Max);
    assert_eq!(rig.api.test_action("fan-max-toggle").unwrap(), "Fans: smart curve");
    assert!(rig.api.smart_fan().unwrap());
    // The smart curve takes the fans over on the service's next tick, as in Python; the toggle
    // reads what the firmware says, so it waits for that.
    rig.wait_for(|| rig.sim.snapshot().fan != SimFan::Max, "the smart curve to take the fans");
    assert_eq!(rig.api.test_action("fan-max-toggle").unwrap(), "Fans: MAX");
    assert_eq!(rig.api.test_action("fan:60").unwrap(), "Fans: 60 %");
    let snap = rig.sim.snapshot();
    assert_eq!((snap.fan, snap.duty), (SimFan::Manual, [60, 60]));
    assert_eq!(rig.api.test_action("fan:auto").unwrap(), "Fans: firmware auto");
    assert_eq!(rig.api.test_action("fan:smart").unwrap(), "Fans: smart curve");
    assert_eq!(rig.api.test_action("smart-fan-toggle").unwrap(), "Fans: smart curve OFF");
    assert!(!rig.api.smart_fan().unwrap());
    // A duty below the range is not an action at all, so it can never be a stop.
    for bad in ["fan:0", "fan:19", "fan:101", "fan:off"] {
        assert!(rig.api.test_action(bad).is_err(), "{bad}");
    }
    assert_ne!(rig.sim.snapshot().duty, [0, 0]);
}

#[test]
fn auto_actions_set_the_mode_control_of_the_service() {
    let rig = Rig::new();
    assert_eq!(rig.api.test_action("auto:smart").unwrap(), "Mode control: smart");
    assert_eq!(rig.state.get_str("auto_mode", ""), "smart");
    // off -> native -> smart -> off.
    assert_eq!(rig.api.test_action("auto-next").unwrap(), "Mode control: off");
    assert_eq!(rig.api.test_action("auto-next").unwrap(), "Mode control: native");
    assert_eq!(rig.api.test_action("auto-next").unwrap(), "Mode control: smart");
}

#[test]
fn the_touchpad_actions_use_the_service() {
    let rig = Rig::new();
    assert_eq!(rig.api.test_action("touchpad:off").unwrap(), "Touchpad: off");
    assert_eq!(rig.api.touchpad(), Some(false));
    assert_eq!(rig.api.test_action("touchpad-toggle").unwrap(), "Touchpad: on");
    assert_eq!(rig.api.touchpad(), Some(true));
}

#[test]
fn the_battery_and_screen_actions_flip_what_the_firmware_reads_back() {
    let rig = Rig::new();
    assert_eq!(rig.api.test_action("battery-limit-toggle").unwrap(), "Battery 80 % limit: on");
    assert!(rig.api.battery_limit().unwrap());
    assert_eq!(rig.api.test_action("battery-limit-toggle").unwrap(), "Battery 80 % limit: off");
    assert_eq!(rig.api.test_action("overdrive-toggle").unwrap(), "LCD overdrive: off");
    assert_eq!(rig.api.test_action("overdrive-toggle").unwrap(), "LCD overdrive: on");
}

#[test]
fn the_light_actions_keep_the_look_across_off_and_on() {
    let rig = Rig::new();
    // The rear light starts on (brightness 3 in the power-on state), then off and back on.
    assert_eq!(rig.api.test_action("rear:off").unwrap(), "Rear light: off");
    assert_eq!(rig.api.rear().unwrap().brightness, 0);
    assert_eq!(rig.api.test_action("rear:on").unwrap(), "Rear light: on");
    assert!(rig.api.rear().unwrap().brightness > 0);
    assert_eq!(rig.api.test_action("rear:toggle").unwrap(), "Rear light: off");
    assert_eq!(rig.api.test_action("rear-effect-next").unwrap(), "Rear effect: static");
    assert_eq!(rig.api.test_action("keyboard:off").unwrap(), "Keyboard light: off");
    assert_eq!(rig.api.keyboard_last().name, "off");
    assert_eq!(rig.api.test_action("keyboard:on").unwrap(), "Keyboard light: on");
    assert_eq!(rig.api.test_action("keyboard-effect-next").unwrap(), "Keyboard effect: static");
    assert_eq!(rig.api.test_action("look:native").unwrap(), "Look: native");
    assert_eq!(rig.api.test_action("notify:hello there").unwrap(), "hello there");
    assert_eq!(rig.api.test_action("none").unwrap(), "");
}

#[test]
fn a_saved_look_is_a_valid_action_and_a_missing_one_is_not() {
    let rig = Rig::new();
    assert!(rig.api.test_action("look:mine").unwrap_err().to_string().contains("no saved look 'mine'"));
    rig.api.save_look_value("mine", &rig.api.look("native").unwrap()).unwrap();
    assert_eq!(rig.api.test_action("look:mine").unwrap(), "Look: mine");
    // The key bindings are validated with the same knowledge of the looks.
    rig.api.set_key_action(Api::parse_gesture("tap").unwrap(), "look:mine").unwrap();
}

/// KNOWN MISMATCH, in `api::keys` (not in this module): Python's `actions.validate` refuses
/// `look:NAME` for a look that does not exist ("no saved look 'x' (truesense look list)") when the
/// binding is configured; `Api::set_key_action` checks the syntax only, so the typo is found at
/// the first key press instead. Remove the `ignore` when `set_key_action` uses the runner's
/// validator.
#[test]
#[ignore = "api::keys::set_key_action validates the syntax only (Python also checks the saved looks)"]
fn binding_a_gesture_to_a_missing_look_is_refused_when_it_is_set() {
    let rig = Rig::new();
    let err = rig.api.set_key_action(Api::parse_gesture("tap").unwrap(), "look:ghost").unwrap_err();
    assert!(err.to_string().contains("no saved look 'ghost'"), "{err}");
}

#[test]
fn the_sound_actions_report_the_sound_system_s_own_words() {
    let rig = Rig::new();
    // No sound card on the test machine: every action says so, and nothing crashes.
    let err = rig.api.test_action("output:speakers").unwrap_err();
    assert_eq!(err.to_string(), "sound card not found");
    assert!(rig.api.test_action("output-toggle").is_err());
    assert!(rig.api.test_action("output:lounge").is_err());
    assert!(rig.api.test_action("mic-toggle").is_ok());
    assert!(rig.api.test_action("mic:mute").unwrap().starts_with("Microphones:"));
}

#[test]
fn an_unknown_action_is_refused_before_anything_happens() {
    let rig = Rig::new();
    for bad in ["warp", "power:ludicrous", "power", "fan", "touchpad:toggle", "keyboard:dim", "exec"] {
        let err = rig.api.test_action(bad).unwrap_err();
        assert!(matches!(err, ApiError::Domain { .. } | ApiError::Daemon(_)), "{bad}: {err:?}");
    }
    assert_eq!(rig.sim.snapshot().mode, 6);
}

#[test]
fn exec_starts_the_command_detached_and_returns_at_once() {
    let rig = Rig::new();
    let marker = rig.dir.0.join("exec-marker");
    let message = rig.api.test_action(&format!("exec:sleep 1; echo ran > {}", marker.display())).unwrap();
    assert_eq!(message, "");
    assert!(!marker.exists(), "the key press must not wait for the command to finish");
    // The shell creates the file before `echo` writes into it: wait for the text.
    rig.wait_for(|| std::fs::read_to_string(&marker).is_ok_and(|t| !t.is_empty()), "the command to run");
    assert_eq!(std::fs::read_to_string(&marker).unwrap().trim(), "ran");
    // A command that fails is the command's business, not the key's.
    assert_eq!(rig.api.test_action("exec:exit 3").unwrap(), "");
    ts_domain::actions::Spawner::spawn(&ShellSpawner, "true").unwrap();
}

// ---- the whole-setup profiles ------------------------------------------------------------------

#[test]
fn a_profile_applies_through_the_service_and_the_firmware() {
    let rig = Rig::new();
    put(
        &rig.home.join(".config/truesense/profiles/gaming.toml"),
        r#"description = "Everything up"
[power]
mode = "turbo"
auto = "native"
[fan]
mode = "manual"
cpu = 60
gpu = 55
curve = "truesense"
[battery]
limit = true
usb_charge = 20
[display]
overdrive = false
[boot]
sound = true
"#,
    );
    assert!(rig.api.describe_profile("gaming").contains("Power: "), "{}", rig.api.describe_profile("gaming"));
    let line = rig.api.apply_profile("gaming").unwrap();
    assert_eq!(line, "Profile 'gaming' applied (8 settings)");
    let snap = rig.sim.snapshot();
    assert_eq!(snap.mode, 5);
    assert_eq!((snap.fan, snap.duty), (SimFan::Manual, [60, 55]));
    assert_eq!((snap.battery[0], snap.usb, snap.overdrive, snap.boot), (1, 20, 0, 1));
    let st = rig.api.daemon_state().unwrap();
    assert_eq!(st.setting_str("auto_mode"), Some("native"));
    assert_eq!(st.setting_str("fan_curve"), Some("truesense"));
    assert_eq!(rig.api.current_profile().as_deref(), Some("gaming"));
}

#[test]
fn a_profile_that_fails_in_part_still_applies_the_rest_and_says_what_failed() {
    let rig = Rig::new();
    put(
        &rig.home.join(".config/truesense/profiles/mixed.toml"),
        "[power]\nmode = \"performance\"\n[audio]\noutput = \"speakers\"\n[battery]\nlimit = true\n",
    );
    let line = rig.api.apply_profile("mixed").unwrap();
    assert!(
        line.starts_with("Profile 'mixed' applied (2 settings, 1 FAILED: audio output: sound card not found"),
        "{line}"
    );
    assert_eq!(rig.sim.snapshot().mode, 4);
    assert_eq!(rig.sim.snapshot().battery[0], 1);
}

#[test]
fn the_profile_actions_walk_the_saved_profiles() {
    let rig = Rig::new();
    for (name, mode) in [("aaa", "quiet"), ("bbb", "balanced")] {
        put(
            &rig.home.join(format!(".config/truesense/profiles/{name}.toml")),
            &format!("[power]\nmode = \"{mode}\"\n"),
        );
    }
    assert_eq!(rig.api.test_action("profile:aaa").unwrap(), "Profile 'aaa' applied (1 settings)");
    assert_eq!(rig.sim.snapshot().mode, 0);
    assert_eq!(rig.api.test_action("profile-next").unwrap(), "Profile 'bbb' applied (1 settings)");
    assert_eq!(rig.sim.snapshot().mode, 1);
    assert_eq!(rig.api.next_profile().unwrap(), "Profile 'aaa' applied (1 settings)");
    assert!(rig.api.test_action("profile:nope").is_err());
}

#[test]
fn saving_the_current_setup_records_what_the_service_and_the_firmware_say() {
    let rig = Rig::new();
    rig.api.set_power(ts_platform::hw::PowerMode::Balanced, None).unwrap();
    rig.api.set_auto_mode(crate::api::AutoMode::Smart).unwrap();
    rig.api.use_fan_curve("truesense").unwrap();
    rig.api.fan_smart().unwrap();
    rig.api.set_battery_limit(true).unwrap();
    let path = rig.api.save_profile("snap", "my setup").unwrap();
    assert!(path.ends_with("profiles/snap.toml"), "{}", path.display());
    let text = std::fs::read_to_string(&path).unwrap();
    for line in ["mode = \"balanced\"", "auto = \"smart\"", "curve = \"truesense\"", "limit = true"] {
        assert!(text.contains(line), "{line} missing from\n{text}");
    }
    assert!(text.contains("description = \"my setup\""), "{text}");
}

// ---- the health check, the service check and the self-test -------------------------------------

#[test]
fn the_health_check_reads_the_service_through_its_adapter() {
    let rig = Rig::new();
    let report = rig.api.run_doctor();
    // `state` of the real daemon decodes into the check's own shape, and `fan_curve` and
    // `battery_full` answer a `get` (the version probe).
    for title in ["Daemon", "Service version"] {
        let check = report.checks.iter().find(|c| c.check == title).unwrap();
        assert_eq!(check.status, ts_system::doctor::Status::Ok, "{check:?}");
    }
    let daemon = report.checks.iter().find(|c| c.check == "Daemon").unwrap();
    assert_eq!(daemon.detail, "running; smart_fan=off, auto_profile=off, activity=idle");
    assert!(report.checks.iter().all(|c| !c.detail.contains("crashed")), "{:?}", report.checks);
}

#[test]
fn the_service_check_finds_the_socket_and_the_daemon() {
    let rig = Rig::new();
    let rows = rig.api.check_services().unwrap();
    let socket = rows.iter().find(|u| u.unit == "control socket").unwrap_or_else(|| panic!("{rows:?}"));
    assert_eq!((socket.active.as_str(), socket.ok), ("answers", true), "{socket:?}");
    // The kernel module's node is not in the temporary tree, and the check says so.
    let module = rows.iter().find(|u| u.unit.starts_with("kernel module")).unwrap();
    assert_eq!((module.active.as_str(), module.ok), ("missing", false));
}

struct StepClock(Mutex<Duration>);

impl selftest::Clock for StepClock {
    fn now(&self) -> Duration {
        *self.0.lock().unwrap()
    }

    fn sleep(&self, d: Duration) {
        *self.0.lock().unwrap() += d;
    }
}

#[test]
fn the_selftest_adapters_pause_the_service_and_put_everything_back() {
    let rig = Rig::new();
    rig.api.daemon_set("smart_fan", true).unwrap();
    rig.sim.set_sensors([50, 52, 48], [2400, 2200]);
    let clock = StepClock(Mutex::new(Duration::ZERO));
    let daemon = SelftestDaemon(&rig.api);
    let keyboard = SelftestKeyboard(&rig.api);
    let audio = SelftestAudio(&rig.api);
    let doctor = |out: &mut dyn std::io::Write| {
        let report = rig.api.run_doctor();
        let _ = writeln!(out, "{} checks", report.checks.len());
        report.exit_code()
    };
    let mut deps = Deps::new(&clock);
    deps.daemon = Some(&daemon);
    deps.keyboard = Some(&keyboard);
    deps.audio = Some(&audio);
    deps.doctor = Some(&doctor);
    let mut out = Vec::new();
    let report = selftest::run(&rig.api.services().hw, &deps, selftest::Options::default(), &mut out);
    let text = String::from_utf8_lossy(&out);
    // Whatever the sound system cannot do on a test machine is a skip or a failure of its own
    // item; the firmware items and the daemon-facing ones must pass.
    for item in &report.items {
        if item.name.contains("fans") || item.name.contains("power modes") || item.name.contains("sensors") {
            assert_eq!(item.outcome, selftest::Outcome::Pass, "{item:?}\n{text}");
        }
        assert!(!item.detail.contains("RESTORE FAILED"), "{item:?}");
    }
    // The service's automation is back as it was, and the fans are on the firmware.
    let st = rig.api.daemon_state().unwrap();
    assert_eq!(st.setting_bool("smart_fan"), Some(true), "{text}");
    assert_eq!(rig.sim.snapshot().fan, SimFan::Auto, "{text}");
    assert_eq!(rig.sim.snapshot().mode, 6, "the power mode is what it was");
}

#[test]
fn the_selftest_adapters_decode_what_the_service_says() {
    let rig = Rig::new();
    let daemon = SelftestDaemon(&rig.api);
    let automation = selftest::Daemon::automation(&daemon).unwrap();
    assert_eq!((automation.smart_fan, automation.auto_profile), (false, false));
    // Read the limits first: with the automation on, the service manages them itself.
    let info = selftest::Daemon::cpu_info(&daemon).unwrap();
    assert_eq!((info.pl1_w, info.pl2_w, info.user), (80, 100, false));
    rig.api.set_cpu_limits(60.0, 70.0, false).unwrap();
    let info = selftest::Daemon::cpu_info(&daemon).unwrap();
    assert_eq!((info.pl1_w, info.pl2_w, info.user), (60, 70, true));
    selftest::Daemon::set_automation(&daemon, selftest::Automation { smart_fan: true, auto_profile: true })
        .unwrap();
    let st = rig.api.daemon_state().unwrap();
    assert_eq!((st.setting_bool("smart_fan"), st.setting_bool("auto_profile")), (Some(true), Some(true)));
    let automation = selftest::Daemon::automation(&daemon).unwrap();
    assert_eq!((automation.smart_fan, automation.auto_profile), (true, true));
    let keyboard = SelftestKeyboard(&rig.api);
    assert!(selftest::Keyboard::brightness(&keyboard).is_ok());
    let audio = SelftestAudio(&rig.api);
    assert!(selftest::Audio::output_active(&audio).unwrap_err().to_string().contains("sound card not found"));
    assert_eq!(selftest::Audio::headset_boost(&audio).unwrap(), None);
    assert!(!selftest::Audio::mic_muted(&audio).unwrap());
}

#[test]
fn without_a_service_the_selftest_adapters_say_it_is_not_running() {
    let rig = Rig::new();
    rig.shutdown().unwrap();
    let daemon = SelftestDaemon(&rig.api);
    let err = selftest::Daemon::automation(&daemon).unwrap_err();
    assert!(err.to_string().contains("not running"), "{err}");
    assert!(!err.is_unknown_command());
}

// ---- the on-screen display ---------------------------------------------------------------------

#[derive(Default)]
struct Recorder(Mutex<Vec<Invocation>>);

impl SessionRunner for Recorder {
    fn run(&self, call: &Invocation) -> Option<OsdOutcome> {
        self.0.lock().unwrap().push(call.clone());
        Some(OsdOutcome { success: true, stdout: "ok".to_owned() })
    }
}

#[test]
fn the_display_reads_its_style_from_the_configuration_every_time() {
    let tmp = super::super::testkit::TempDir::new();
    let config_path = tmp.0.join("config.toml");
    let recorder = Arc::new(Recorder::default());
    let base = Osd::new(recorder.clone(), Arc::new(PasswdFile::new(tmp.0.join("passwd")))).with_env(|_| None);
    let osd = LiveOsd::new(base, ConfigFiles::at(None, Some(config_path.clone())));
    Notifier::show(&osd, "first", "power");
    std::fs::write(&config_path, "[osd]\nstyle = \"none\"\n").unwrap();
    Notifier::show(&osd, "second", "power");
    std::fs::write(&config_path, "[osd]\nstyle = \"noctalia\"\n").unwrap();
    Notifier::show(&osd, "third", "power");
    let calls = recorder.0.lock().unwrap();
    let programs: Vec<(&str, &str)> =
        calls.iter().map(|c| (c.program.as_str(), c.args.last().map_or("", String::as_str))).collect();
    // native: the compositor's notification first; none: silent; noctalia: its own popup.
    assert_eq!(programs, [("hyprctl", "first"), ("noctalia", "third")]);
}

#[test]
fn a_silenced_section_of_the_sound_system_nests_through_the_shared_display() {
    let tmp = super::super::testkit::TempDir::new();
    std::fs::write(tmp.0.join("config.toml"), "[osd]\nstyle = \"noctalia\"\n").unwrap();
    let recorder = Arc::new(Recorder::default());
    let base = Osd::new(recorder.clone(), Arc::new(PasswdFile::new(tmp.0.join("passwd")))).with_env(|_| None);
    let osd = LiveOsd::new(base, ConfigFiles::at(None, Some(tmp.0.join("config.toml"))));
    {
        let _outer = ts_domain::audio::Osd::quiet(&osd);
        let _inner = ts_domain::audio::Osd::quiet(&osd);
        let disables =
            recorder.0.lock().unwrap().iter().filter(|c| c.args.contains(&"osd-disable".to_owned())).count();
        assert_eq!(disables, 1, "only the outermost section disables the popups");
    }
}

/// PYTHON PARITY, believed to be a bug: the self-test pauses and restores the service's
/// automation through the `auto_profile` switch, and the service maps `auto_profile: true` to the
/// *smart* mode, so a laptop on the native automatic mode ends the self-test on smart. Python's
/// `selftest.py` does exactly this; it is ported as it is and reported.
#[test]
fn the_selftest_turns_native_into_smart_like_python_does() {
    let rig = Rig::new();
    rig.api.set_auto_mode(crate::api::AutoMode::Native).unwrap();
    let daemon = SelftestDaemon(&rig.api);
    let before = selftest::Daemon::automation(&daemon).unwrap();
    assert!(before.auto_profile);
    selftest::Daemon::set_automation(&daemon, selftest::Automation { smart_fan: false, auto_profile: false })
        .unwrap();
    selftest::Daemon::set_automation(&daemon, before).unwrap();
    assert_eq!(rig.api.auto_mode().unwrap(), crate::api::AutoMode::Smart);
}

#[test]
fn an_edit_of_the_configuration_applies_to_the_next_key_press_without_a_restart() {
    let rig = Rig::new();
    assert_eq!(rig.api.test_action("power-next").unwrap(), "Power mode: quiet");
    put(&rig.home.join(".config/truesense/config.toml"), "[keys]\npower_cycle = [\"eco\", \"turbo\"]\n");
    // From quiet, which the new cycle does not contain, it starts at the first entry after -1.
    assert_eq!(rig.api.test_action("power-next").unwrap(), "Power mode: eco");
    assert_eq!(rig.api.test_action("power-next").unwrap(), "Power mode: turbo");
    put(&rig.home.join(".config/truesense/config.toml"), "[profile]\nhold_minutes = 2\n");
    rig.api.test_action("power:balanced").unwrap();
    let left = rig.api.daemon_state().unwrap().hold_left_s;
    assert!((60..=120).contains(&left), "{left}");
}

#[test]
fn root_without_a_service_switches_the_touchpad_itself_and_everyone_else_is_told_to_start_it() {
    let rig = Rig::new();
    rig.shutdown().unwrap();
    // A user: the service is the only way (the switch needs root).
    let err = rig.api.test_action("touchpad:off").unwrap_err();
    assert!(err.to_string().contains("not running"), "{err}");
    assert_eq!(rig.api.touchpad(), Some(true));
    // Root: Python's `_touch` writes the kernel's file directly.
    let mut machine = rig.machine.clone();
    machine.euid = 0;
    let root_api = Api::new(Services::assemble(&machine).unwrap());
    assert_eq!(root_api.test_action("touchpad:off").unwrap(), "Touchpad: off");
    assert_eq!(root_api.touchpad(), Some(false));
    assert_eq!(root_api.test_action("touchpad-toggle").unwrap(), "Touchpad: on");
}
