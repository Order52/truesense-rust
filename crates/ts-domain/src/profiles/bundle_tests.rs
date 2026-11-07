//! Tests of the whole-setup profiles against the fake machine and temporary directories.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::err_expect,
    clippy::field_reassign_with_default
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;

use super::testutil::Tmp;
use super::*;
use crate::actions::fakes::World;
use crate::actions::{ActionError, ActionRunner, HostError, Runner};

/// Records every action it is asked to run instead of running it.
struct Recorder(Arc<World>);

impl ActionRunner for Recorder {
    fn run(&self, spec: &str) -> Result<String, ActionError> {
        self.0.log.lock().unwrap().push(format!("action {spec}"));
        if self.0.fail.lock().unwrap().iter().any(|f| f == "action") {
            return Err(ActionError::Host { op: "action", source: HostError::new("boom") });
        }
        Ok(String::new())
    }
}

fn shipped() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join("profiles")
}

fn host(w: &Arc<World>, actions: Arc<dyn ActionRunner>) -> BundleHost {
    BundleHost {
        actions,
        power: w.clone(),
        daemon: w.clone(),
        lighting: w.clone(),
        audio: w.clone(),
        state: w.clone(),
    }
}

fn store(w: &Arc<World>, tmp: &Tmp) -> BundleStore {
    BundleStore::new(
        BundleFiles::new(tmp.0.join("user"), vec![shipped()]),
        host(w, Arc::new(Recorder(w.clone()))),
    )
}

/// Writes a bundle into the user directory and applies it as `p`.
fn apply_text(w: &Arc<World>, body: &str) -> String {
    let tmp = Tmp::new("apply");
    let s = store(w, &tmp);
    std::fs::create_dir_all(s.files().user_dir()).unwrap();
    std::fs::write(s.files().user_dir().join("p.toml"), body).unwrap();
    s.apply("p").unwrap()
}

fn log(w: &Arc<World>) -> Vec<String> {
    w.calls()
}

#[test]
fn shipped_profiles_load_and_apply() {
    // tests/test_core.py::test_shipped_profiles_load_and_apply
    let w = World::new();
    let tmp = Tmp::new("shipped");
    let s = store(&w, &tmp);
    for n in ["gaming", "quiet", "balanced", "battery-saver"] {
        assert!(s.files().names().contains(&n.to_owned()), "{n}");
        assert_eq!(s.files().source(n), Some(BundleSource::Shipped));
    }
    let msg = s.apply("battery-saver").unwrap();
    assert_eq!(msg, "Profile 'battery-saver' applied (6 settings)");
    assert_eq!(
        log(&w),
        [
            "action power:eco",
            "action fan:auto",
            "kbtimeout:true",
            "action rear:off",
            "battery:true",
            "overdrive:false"
        ]
    );
    assert_eq!(w.state_get("profile"), Some(json!("battery-saver")));
}

#[test]
fn every_shipped_profile_applies_cleanly() {
    let tmp = Tmp::new("every");
    let names = BundleFiles::new(tmp.0.join("none"), vec![shipped()]).names();
    assert_eq!(names, ["balanced", "battery-saver", "gaming", "quiet"], "sorted");
    for n in names {
        let w = World::new();
        let msg = store(&w, &tmp).apply(&n).unwrap();
        assert!(!msg.contains("FAILED"), "{n}: {msg}");
    }
}

#[test]
fn applying_through_the_real_runner_reaches_the_machine() {
    // The whole path: bundle -> action runner -> facades.
    let w = World::new();
    let tmp = Tmp::new("real");
    let slot = crate::actions::RunnerSlot::new();
    let s = BundleStore::new(
        BundleFiles::new(tmp.0.join("user"), vec![shipped()]),
        host(&w, Arc::new(slot.clone())),
    );
    assert!(slot.set(Arc::new(Runner::new(w.env()))));
    let msg = s.apply("battery-saver").unwrap();
    assert_eq!(msg, "Profile 'battery-saver' applied (6 settings)");
    assert_eq!(*w.mode.lock().unwrap(), "eco");
    assert!(*w.battery.lock().unwrap());
    assert_eq!(*w.overdrive.lock().unwrap(), Some(false));
    assert_eq!(w.rear.lock().unwrap().brightness, 0);
    // Unwired actions are a reported failure, not a crash.
    let w2 = World::new();
    let s2 = BundleStore::new(
        BundleFiles::new(tmp.0.join("user"), vec![shipped()]),
        host(&w2, Arc::new(crate::actions::RunnerSlot::new())),
    );
    let msg = s2.apply("quiet").unwrap();
    assert!(msg.contains("3 FAILED"), "{msg}");
}

#[test]
fn names_cannot_escape() {
    let tmp = Tmp::new("escape");
    let f = BundleFiles::new(tmp.0.join("user"), vec![shipped()]);
    for bad in ["../etc/passwd", "a b", "", "x/y", "a.b", "..", "a\0"] {
        assert_eq!(
            f.load(bad).err().unwrap().to_string(),
            "profile names: letters, digits, - and _",
            "{bad:?}"
        );
        assert!(f.save(bad, &Bundle::new()).is_err(), "save {bad:?}");
        assert!(f.delete(bad).is_err(), "delete {bad:?}");
        assert_eq!(f.source(bad), None);
    }
    for ok in ["gaming", "my-set_2", "\u{dc}n\u{ef}"] {
        assert!(NAME_RULE_OK(ok), "{ok}");
    }
    assert!(!tmp.0.join("etc").exists());
}

#[allow(non_snake_case)]
fn NAME_RULE_OK(n: &str) -> bool {
    super::bundle::NAME_RULE.accepts(n)
}

#[test]
fn user_bundles_win_and_deleting_reveals_the_shipped_one() {
    let tmp = Tmp::new("win");
    let f = BundleFiles::new(tmp.0.join("user"), vec![shipped()]);
    let mut mine = Bundle::new();
    mine.insert("description".into(), "mine".into());
    f.save("gaming", &mine).unwrap();
    let mut extra = Bundle::new();
    extra.insert("description".into(), "x".into());
    f.save("extra", &extra).unwrap();
    assert_eq!(f.source("gaming"), Some(BundleSource::Yours));
    assert_eq!(f.source("gaming").map(BundleSource::as_str), Some("yours"));
    assert_eq!(f.load("gaming").unwrap().get("description").and_then(|v| v.as_str()), Some("mine"));
    let names = f.names();
    assert_eq!(names, ["balanced", "battery-saver", "extra", "gaming", "quiet"]);
    assert_eq!(f.source("nothere"), None);
    assert_eq!(f.source("a/b"), None);
    assert!(f.delete("quiet").err().unwrap().to_string().contains("shipped profile"));
    f.delete("gaming").unwrap();
    assert_eq!(f.source("gaming"), Some(BundleSource::Shipped));
    assert_eq!(f.delete("nothere").err().unwrap().to_string(), "no profile 'nothere'");
    assert_eq!(f.load("nothere").err().unwrap().to_string(), "no profile 'nothere' (truesense profile list)");
}

#[test]
fn a_bad_file_is_reported_with_its_path() {
    let tmp = Tmp::new("bad");
    let f = BundleFiles::new(tmp.0.join("user"), vec![]);
    std::fs::create_dir_all(f.user_dir()).unwrap();
    std::fs::write(f.user_dir().join("bad.toml"), "[power\nmode = ").unwrap();
    let e = f.load("bad").err().unwrap();
    assert!(matches!(e, BundleError::Parse { .. }));
    assert!(e.to_string().starts_with(&f.user_dir().join("bad.toml").display().to_string()), "{e}");
    std::fs::write(f.user_dir().join("bin.toml"), [0xff, 0xfe]).unwrap();
    assert!(matches!(f.load("bin"), Err(BundleError::Parse { .. })));
    std::fs::write(f.user_dir().join("deep.toml"), "a = ".to_owned() + &"[".repeat(5000)).unwrap();
    assert!(f.load("deep").is_err(), "hostile nesting is an error");
}

#[test]
fn save_roundtrips_and_leaves_no_temporary_files() {
    let tmp = Tmp::new("save");
    let f = BundleFiles::new(tmp.0.join("user"), vec![]);
    let d = parse_bundle(
        "description = \"x\"\n[power]\nmode = \"eco\"\n[fan]\nmode = \"manual\"\ncpu = 60\ngpu = 70\n",
        "t",
    )
    .unwrap();
    let path = f.save("rt", &d).unwrap();
    assert_eq!(path, tmp.0.join("user").join("rt.toml"));
    assert_eq!(f.load("rt").unwrap(), d);
    assert_eq!(std::fs::read_dir(f.user_dir()).unwrap().count(), 1);
    // Unknown keys and sections survive a round trip.
    let d2 = parse_bundle("[gpu]\nmode = \"nvidia\"\n[power]\nbogus = 1\n", "t").unwrap();
    f.save("keep", &d2).unwrap();
    assert_eq!(f.load("keep").unwrap(), d2);
}

#[test]
fn default_sys_dirs_follow_the_environment() {
    let none = |_: &str| None;
    assert_eq!(
        default_sys_dirs(&none),
        [
            PathBuf::from("/usr/local/share/truesense/profiles"),
            PathBuf::from("/usr/share/truesense/profiles")
        ]
    );
    let set = |k: &str| (k == "TRUESENSE_PROFILES_DIR").then(|| "/x/p".to_owned());
    assert_eq!(default_sys_dirs(&set), [PathBuf::from("/x/p")]);
    assert_eq!(
        BundleFiles::default_user_dir(Path::new("/h")),
        PathBuf::from("/h/.config/truesense/profiles")
    );
}

#[test]
fn apply_touches_only_what_is_listed() {
    let cases: [(&str, &str, Vec<&str>); 9] = [
        ("empty", "description = \"nothing\"\n", vec![]),
        ("only power", "[power]\nmode = \"eco\"\n", vec!["action power:eco"]),
        ("only battery limit", "[battery]\nlimit = false\n", vec!["battery:false"]),
        ("only usb", "[battery]\nusb_charge = 20\n", vec!["usb:20"]),
        ("only boot", "[boot]\nsound = true\n", vec!["bootsound:true"]),
        ("only mic", "[audio]\nmic_muted = true\n", vec!["mic:true"]),
        ("only fan auto", "[fan]\nmode = \"auto\"\n", vec!["action fan:auto"]),
        (
            "unknown sections and keys",
            "[gpu]\nmode = \"nvidia\"\n[undervolt]\nx = 1\n[power]\nbogus = 1\n",
            vec![],
        ),
        ("sections with nothing we know", "[keyboard]\ncolor = \"ff0000\"\n[rear]\nspeed = 3\n", vec![]),
    ];
    for (name, body, want) in cases {
        let w = World::new();
        let msg = apply_text(&w, body);
        assert_eq!(log(&w), want, "{name}");
        assert!(msg.contains(&format!("({} settings)", want.len())), "{name}: {msg}");
    }
}

#[test]
fn apply_everything_in_a_fixed_order() {
    let body = r#"
description = "all"
[power]
mode = "turbo"
auto = "smart"
[fan]
mode = "manual"
cpu = 60
gpu = 70
curve = "mycurve"
[keyboard]
effect = "wave"
color = "ff00ff"
speed = 7
brightness = 40
timeout = false
[rear]
effect = "breathing"
color = "ff0000"
speed = 2
brightness = 80
direction = 0
[battery]
limit = true
usb_charge = 30
[display]
overdrive = true
[boot]
sound = false
[audio]
output = "headphones"
voice_headset = "conference"
voice_array = "personal"
mic_muted = false
dts_mode = "Movie"
"#;
    let want = [
        "daemon_set:auto_mode=\"smart\"",
        "action power:turbo",
        "curve:mycurve",
        "fan:Manual { cpu: 60, gpu: 70 }",
        "kb_effect:wave:ff00ff:7:40",
        "kbtimeout:false",
        "rear_set:breathing:ff0000:2:80:0",
        "battery:true",
        "usb:30",
        "overdrive:true",
        "bootsound:false",
        "output:headphones",
        "voice:headset:conference",
        "voice:array:personal",
        "mic:false",
        "dts:Movie",
    ];
    // Every run gives exactly the same sequence.
    for _ in 0..20 {
        let w = World::new();
        let msg = apply_text(&w, body);
        assert_eq!(log(&w), want);
        assert_eq!(msg, "Profile 'p' applied (16 settings)");
    }
}

#[test]
fn keyboard_choice_order_is_profile_then_effect_then_on() {
    let w = World::new();
    apply_text(&w, "[keyboard]\nprofile = \"mine\"\neffect = \"wave\"\non = false\n");
    assert_eq!(log(&w), ["action keyboard-profile:mine"]);
    let w = World::new();
    apply_text(&w, "[keyboard]\neffect = \"wave\"\non = false\n");
    assert_eq!(log(&w), ["kb_effect:wave:00aec7:5:50"], "defaults: colour, speed 5, current brightness");
    let w = World::new();
    *w.kb_brightness.lock().unwrap() = 0;
    apply_text(&w, "[keyboard]\neffect = \"wave\"\n");
    assert_eq!(log(&w), ["kb_effect:wave:00aec7:5:30"], "a dark keyboard starts at 30");
    let w = World::new();
    apply_text(&w, "[keyboard]\non = false\n");
    assert_eq!(log(&w), ["action keyboard:off"]);
    let w = World::new();
    apply_text(&w, "[keyboard]\non = true\n");
    assert_eq!(log(&w), ["action keyboard:on"]);
}

#[test]
fn rear_choice_and_defaults() {
    let w = World::new();
    apply_text(&w, "[rear]\neffect = \"wave\"\non = false\n");
    assert_eq!(log(&w), ["rear_set:wave:ffffff:3:100:1"]);
    let w = World::new();
    apply_text(&w, "[rear]\non = true\n");
    assert_eq!(log(&w), ["action rear:on"]);
}

#[test]
fn a_fan_curve_goes_by_name_to_the_daemon() {
    let w = World::new();
    apply_text(&w, "[fan]\ncurve = \"truesense\"\n");
    assert_eq!(log(&w), ["curve:truesense"]);
}

#[test]
fn failures_are_reported_and_the_rest_still_applies() {
    let w = World::new();
    w.fail("battery");
    let msg = apply_text(
        &w,
        "[power]\nmode = \"eco\"\n[battery]\nlimit = true\nusb_charge = 10\n[display]\noverdrive = true\n",
    );
    assert_eq!(msg, "Profile 'p' applied (3 settings, 1 FAILED: battery limit: battery failed)");
    assert_eq!(w.state_get("profile"), Some(json!("p")), "a partly applied profile is still the current one");
    let w = World::new();
    w.fail("action");
    let msg = apply_text(&w, "[power]\nmode = \"eco\"\n[fan]\nmode = \"auto\"\n");
    assert_eq!(msg, "Profile 'p' applied (0 settings, 2 FAILED: power: boom; fan: boom)");
}

#[test]
fn bad_values_fail_their_setting_only() {
    let w = World::new();
    let msg = apply_text(
        &w,
        "[fan]\nmode = \"manual\"\ncpu = \"abc\"\ngpu = 60\n[battery]\nusb_charge = [1]\nlimit = true\n[rear]\neffect = \"wave\"\ncolor = 5\n[keyboard]\neffect = \"wave\"\nspeed = \"fast\"\n",
    );
    for want in [
        "fan: \"abc\" is not a number",
        "usb charge: ",
        "rear light: color must be text",
        "keyboard effect: \"fast\" is not a number",
        "4 FAILED",
    ] {
        assert!(msg.contains(want), "{msg:?} missing {want:?}");
    }
    assert_eq!(log(&w), ["battery:true"]);
    let w = World::new();
    let msg = apply_text(&w, "[fan]\nmode = \"manual\"\ncpu = 60\n");
    assert!(msg.contains("fan: manual fans need cpu and gpu"), "{msg}");
    assert!(log(&w).is_empty());
}

#[test]
fn a_profile_can_never_stop_a_fan() {
    // Python passed the numbers to the fan service, which refused 0 without confirmation; a
    // bundle has no confirmation, so it refuses before the request exists.
    for body in [
        "[fan]\nmode = \"manual\"\ncpu = 0\ngpu = 0\n",
        "[fan]\nmode = \"manual\"\ncpu = 0\ngpu = 60\n",
        "[fan]\nmode = \"manual\"\ncpu = 19\ngpu = 60\n",
        "[fan]\nmode = \"manual\"\ncpu = 101\ngpu = 60\n",
        "[fan]\nmode = \"manual\"\ncpu = -5\ngpu = 60\n",
        "[fan]\nmode = \"manual\"\ncpu = 1e30\ngpu = 60\n",
        "[fan]\nmode = \"manual\"\ncpu = false\ngpu = true\n",
        "[fan]\nmode = \"manual\"\ncpu = 0.4\ngpu = 60\n",
    ] {
        let w = World::new();
        let msg = apply_text(&w, body);
        assert!(msg.contains("1 FAILED: fan: "), "{body}: {msg}");
        assert!(!log(&w).iter().any(|l| l.starts_with("fan:")), "no fan command for {body}");
    }
    let w = World::new();
    let msg = apply_text(&w, "[fan]\nmode = \"manual\"\ncpu = 20.9\ngpu = \"100\"\n");
    assert_eq!(msg, "Profile 'p' applied (1 settings)");
    assert_eq!(log(&w), ["fan:Manual { cpu: 20, gpu: 100 }"]);
}

#[test]
fn an_unknown_profile_is_an_error() {
    let w = World::new();
    let tmp = Tmp::new("unknown");
    let e = store(&w, &tmp).apply("nothere").err().unwrap();
    assert!(e.to_string().contains("no profile 'nothere'"), "{e}");
}

#[test]
fn without_a_daemon_the_daemon_settings_fail_and_the_rest_applies() {
    let w = World::new();
    *w.daemon_up.lock().unwrap() = false;
    let msg = apply_text(&w, "[power]\nauto = \"smart\"\nmode = \"eco\"\n[fan]\ncurve = \"truesense\"\n");
    assert!(
        msg.contains("2 FAILED: power auto: ") && msg.contains("fan curve: ") && msg.contains("(1 settings"),
        "{msg}"
    );
}

#[test]
fn next_name_steps_through_the_sorted_names_and_wraps() {
    let w = World::new();
    let tmp = Tmp::new("next");
    let s = store(&w, &tmp);
    let names = s.files().names();
    assert_eq!(s.next_name().unwrap(), names[0]);
    w.state_put("profile", json!(names[0]));
    assert_eq!(s.next_name().unwrap(), names[1]);
    w.state_put("profile", json!(names[names.len() - 1]));
    assert_eq!(s.next_name().unwrap(), names[0], "wraps");
    w.state_put("profile", json!("vanished"));
    assert_eq!(s.next_name().unwrap(), names[0]);
    w.state_put("profile", json!(7));
    assert_eq!(s.next_name().unwrap(), names[0]);
    let empty = BundleStore::new(
        BundleFiles::new(tmp.0.join("empty"), vec![]),
        host(&w, Arc::new(Recorder(w.clone()))),
    );
    assert_eq!(empty.next_name().err().unwrap().to_string(), "no profiles saved");
    // As the facade the actions see.
    let facade: &dyn ProfilesFacadeAlias = &s;
    assert_eq!(facade.apply("quiet").unwrap(), "Profile 'quiet' applied (3 settings)");
    assert_eq!(facade.next_name().unwrap(), "balanced");
    assert_eq!(
        facade.apply("nothere").err().unwrap().message(),
        "no profile 'nothere' (truesense profile list)"
    );
}

use crate::actions::ProfilesFacade as ProfilesFacadeAlias;

#[test]
fn capture_roundtrips_through_toml() {
    // tests/test_core.py::test_capture_roundtrip
    let w = World::new();
    *w.mode.lock().unwrap() = "turbo".into();
    let tmp = Tmp::new("capture");
    let s = store(&w, &tmp);
    let cap = s.capture("t").unwrap();
    assert_eq!(cap.get("description").and_then(|v| v.as_str()), Some("t"));
    assert_eq!(section(&cap, "power").get("mode").and_then(|v| v.as_str()), Some("turbo"));
    assert_eq!(section(&cap, "power").get("auto").and_then(|v| v.as_str()), Some("off"));
    assert_eq!(section(&cap, "fan").get("mode").and_then(|v| v.as_str()), Some("auto"));
    assert_eq!(section(&cap, "fan").get("curve").and_then(|v| v.as_str()), Some("truesense"));
    let rear = section(&cap, "rear");
    assert_eq!(rear.get("effect").and_then(|v| v.as_str()), Some("static"));
    assert_eq!(rear.get("brightness").and_then(|v| v.as_integer()), Some(80));
    assert_eq!(section(&cap, "battery").get("usb_charge").and_then(|v| v.as_integer()), Some(0));
    assert_eq!(section(&cap, "keyboard").get("timeout").and_then(|v| v.as_bool()), Some(false));
    let audio = section(&cap, "audio");
    assert_eq!(audio.get("output").and_then(|v| v.as_str()), Some("speakers"));
    // The captured text parses back to the same thing, and applying it works.
    s.files().save("cap", &cap).unwrap();
    assert_eq!(s.files().load("cap").unwrap(), cap);
    let msg = s.apply("cap").unwrap();
    assert!(!msg.contains("FAILED"), "{msg}");
}

fn section<'a>(d: &'a Bundle, name: &str) -> &'a ts_foundation::config::Table {
    d.get(name).and_then(|v| v.as_table()).unwrap()
}

#[test]
fn capture_records_smart_manual_and_leaves_out_what_it_cannot_read() {
    let w = World::new();
    w.daemon.lock().unwrap().insert("smart_fan".into(), json!(true));
    w.daemon.lock().unwrap().insert("auto_mode".into(), json!("native"));
    let tmp = Tmp::new("capture2");
    let s = store(&w, &tmp);
    let cap = s.capture("").unwrap();
    assert_eq!(cap.get("description").and_then(|v| v.as_str()), Some("captured"));
    assert_eq!(section(&cap, "fan").get("mode").and_then(|v| v.as_str()), Some("smart"));
    assert_eq!(section(&cap, "power").get("auto").and_then(|v| v.as_str()), Some("native"));
    // Manual fans carry their duties.
    w.daemon.lock().unwrap().insert("smart_fan".into(), json!(false));
    *w.fan.lock().unwrap() = crate::actions::FanSnapshot { mode: "manual".into(), cpu: 55, gpu: 65 };
    let cap = s.capture("m").unwrap();
    let fan = section(&cap, "fan");
    assert_eq!(
        (fan.get("cpu").and_then(|v| v.as_integer()), fan.get("gpu").and_then(|v| v.as_integer())),
        (Some(55), Some(65))
    );
    // No daemon: its settings are left out. No sound: no audio section. A dark rear light has no effect.
    *w.daemon_up.lock().unwrap() = false;
    w.fail("audio_read");
    w.rear.lock().unwrap().brightness = 0;
    let cap = s.capture("n").unwrap();
    assert!(section(&cap, "fan").get("curve").is_none());
    assert!(section(&cap, "power").get("auto").is_none());
    assert!(cap.get("audio").is_none());
    assert_eq!(section(&cap, "rear").get("on").and_then(|v| v.as_bool()), Some(false));
    assert!(section(&cap, "rear").get("effect").is_none());
}

fn d(text: &str) -> Bundle {
    parse_bundle(text, "t").unwrap()
}

#[test]
fn describe_shipped_and_empty() {
    let tmp = Tmp::new("describe");
    let f = BundleFiles::new(tmp.0.join("n"), vec![shipped()]);
    let lines = describe(&f.load("battery-saver").unwrap()).join("\n");
    for want in ["Eco", "80 % limit on", "Rear light: off"] {
        assert!(lines.contains(want), "missing {want:?} in\n{lines}");
    }
    assert_eq!(describe(&Bundle::new()), ["(nothing)"]);
}

#[test]
fn describe_wording() {
    let table: Vec<(&str, &str, Vec<&str>)> = vec![
        (
            "power",
            "[power]\nmode = \"TURBO\"\nauto = \"smart\"\n",
            vec!["Power: Turbo mode (held for a while), mode control smart"],
        ),
        ("power auto only", "[power]\nauto = \"off\"\n", vec!["Power: mode control off"]),
        (
            "fan smart",
            "[fan]\nmode = \"smart\"\ncurve = \"truesense\"\n",
            vec!["Fans: smart curve, curve truesense"],
        ),
        ("fan auto", "[fan]\nmode = \"auto\"\n", vec!["Fans: firmware Auto"]),
        ("fan max", "[fan]\nmode = \"max\"\n", vec!["Fans: maximum"]),
        ("fan manual", "[fan]\nmode = \"manual\"\ncpu = 60\ngpu = 70\n", vec!["Fans: manual 60 % / 70 %"]),
        ("fan manual unknown duty", "[fan]\nmode = \"manual\"\n", vec!["Fans: manual ? % / ? %"]),
        ("fan curve only", "[fan]\ncurve = \"x\"\n", vec!["Fans: curve x"]),
        (
            "keyboard",
            "[keyboard]\neffect = \"wave\"\nprofile = \"mine\"\non = false\ntimeout = true\n",
            vec!["Keyboard: wave effect, key set mine, backlight off, light off after 30 s on"],
        ),
        ("keyboard with only unknown keys", "[keyboard]\ncolor = \"x\"\n", vec!["Keyboard: "]),
        (
            "rear effect",
            "[rear]\neffect = \"breathing\"\ncolor = \"ff0000\"\n",
            vec!["Rear light: breathing ff0000"],
        ),
        ("rear effect without colour", "[rear]\neffect = \"wave\"\n", vec!["Rear light: wave"]),
        ("rear on", "[rear]\non = true\n", vec!["Rear light: on"]),
        ("rear off", "[rear]\non = false\n", vec!["Rear light: off"]),
        (
            "battery",
            "[battery]\nlimit = false\nusb_charge = 30\n",
            vec!["Battery: 80 % limit off, USB charging while off 30"],
        ),
        ("screen", "[display]\noverdrive = true\n", vec!["Screen: overdrive on"]),
        ("boot", "[boot]\nsound = false\n", vec!["Boot sound off"]),
        (
            "audio",
            "[audio]\noutput = \"speakers\"\nvoice_headset = \"conference\"\nvoice_array = \"off\"\n",
            vec!["Audio: speakers, headset noise reduction conference, built-in noise reduction off"],
        ),
        ("audio with only a mic", "[audio]\nmic_muted = true\n", vec!["Audio: "]),
        ("unknown sections say nothing", "[gpu]\nmode = \"x\"\n", vec!["(nothing)"]),
        ("empty sections say nothing", "[power]\n[display]\n", vec!["(nothing)"]),
        (
            "python-style booleans and floats",
            "[power]\nmode = true\n[fan]\nmode = \"manual\"\ncpu = 1.5\n",
            vec!["Power: True mode (held for a while)", "Fans: manual 1.5 % / ? %"],
        ),
        ("a section that is not a table", "power = 3\nfan = \"x\"\n", vec!["(nothing)"]),
    ];
    for (name, input, want) in table {
        assert_eq!(describe(&d(input)), want, "{name}");
    }
}

#[test]
fn describe_never_panics_whatever_the_types() {
    for input in [
        "[power]\nmode = [1, 2]\nauto = {a = 1}\n",
        "[fan]\nmode = 5\ncurve = 1979-05-27T07:32:00Z\n",
        "[rear]\neffect = []\non = {}\n",
        "[keyboard]\non = \"\"\n[battery]\nlimit = 0.0\n",
        "[audio]\noutput = {}\n",
    ] {
        let lines = describe(&d(input));
        assert!(!lines.is_empty());
    }
}

#[test]
fn apply_never_panics_whatever_the_types() {
    for input in [
        "[power]\nmode = 5\n",
        "[fan]\nmode = \"manual\"\ncpu = 1.5e300\ngpu = true\n",
        "[keyboard]\non = \"yes\"\neffect = 3\n",
        "[rear]\neffect = []\n",
        "[audio]\noutput = {}\n",
        "[battery]\nusb_charge = \"x\"\n",
        "[power]\nauto = 1979-05-27T07:32:00Z\nmode = {a = 1}\n",
    ] {
        let w = World::new();
        let msg = apply_text(&w, input);
        assert!(msg.starts_with("Profile 'p' applied ("), "{msg}");
    }
}
