use serde_json::json;

use super::Fx;
use crate::api::testing::Reply;

#[test]
fn the_window_starts_at_login_by_an_autostart_entry() {
    let fx = Fx::offline();
    assert!(!fx.api.autostart());
    assert!(fx.api.set_autostart(true).unwrap());
    assert!(fx.api.autostart());
    let entry = std::fs::read_to_string(fx.dir.0.join("home/autostart/truesense.desktop")).unwrap();
    assert!(entry.starts_with("[Desktop Entry]\n") && entry.contains("Exec=truesense gui\n"));
    assert!(!fx.api.set_autostart(false).unwrap());
    assert!(!fx.api.autostart());
    assert!(!fx.api.set_autostart(false).unwrap(), "switching off twice is fine");
}

#[test]
fn without_a_home_there_is_nowhere_to_put_the_entry() {
    let fx = Fx::build(|mut s| {
        s.autostart_dir = None;
        s
    });
    assert!(!fx.api.autostart());
    assert!(fx.api.set_autostart(true).is_err());
}

#[test]
fn reset_all_runs_every_step_even_when_some_fail() {
    let fx = Fx::new();
    // A daemon that knows only some commands: the others are "unknown command".
    fx.rig.daemon.answer_ok("fan", json!("auto"));
    fx.rig.daemon.answer_ok("fan_curve", json!({"name": "native"}));
    fx.rig.daemon.answer("undervolt", Reply::Error("msr not loaded".into()));
    let steps = fx.api.reset_all();
    let names: Vec<&str> = steps.iter().map(|s| s.step.as_str()).collect();
    assert_eq!(
        names,
        [
            "fan curve: native",
            "Smart auto profile: native",
            "fans: firmware auto",
            "mode control: native",
            "key bindings: native",
            "power limits: automatic",
            "CPU undervolt: no boot profile",
            "CPU undervolt: stock",
            "GPU undervolt: no boot profile",
            "GPU undervolt: stock",
            "keyboard: stock look",
            "light bar: stock"
        ]
    );
    let ok: Vec<bool> = steps.iter().map(|s| s.ok).collect();
    assert_eq!(ok, [true, false, true, false, true, false, false, false, false, false, true, true]);
    assert_eq!(steps[6].error.as_deref(), Some("msr not loaded"));
    assert!(steps[1].error.as_deref().unwrap().starts_with("unknown command 'smart_profile'"));
    assert!(steps[0].error.is_none());
    let json = serde_json::to_value(&steps[0]).unwrap();
    assert!(json.get("error").is_none());
    // The keyboard and the light bar really went back to stock.
    assert_eq!(fx.api.keyboard_cells()[0], "00aec7");
    assert_eq!(fx.api.rear().unwrap().color.to_string(), "3723c7");
    assert_eq!(fx.api.key_settings().unwrap().profile, "native");
}

#[test]
fn reset_all_never_confirms_an_undervolt_it_only_resets() {
    let fx = Fx::new();
    fx.api.reset_all();
    for (cmd, args) in fx.rig.daemon.calls() {
        if cmd == "undervolt" || cmd == "gpu_undervolt" {
            let action = args.get("action").cloned().unwrap();
            assert!(action == json!("reset") || action == json!("boot_clear"), "{cmd} {action}");
        }
        if cmd == "fan" {
            assert_eq!(args.get("mode"), Some(&json!("auto")));
        }
    }
}

#[test]
fn user_services_are_repaired_and_the_rest_is_pointed_out() {
    let fx = Fx::new();
    let steps = fx.api.repair_user_services().unwrap();
    assert_eq!(steps.len(), 2);
    assert!(steps[1].what.contains("sudo truesense service repair") && steps[1].ok);
    let root = Fx::build(|mut s| {
        s.euid = 0;
        s
    });
    assert_eq!(root.api.repair_user_services().unwrap().len(), 1);
    let none = Fx::build(|s| s.without_desktop());
    assert!(none.api.repair_user_services().is_err());
}

#[test]
fn the_profile_applied_last_is_remembered() {
    let fx = Fx::new();
    assert_eq!(fx.api.current_profile(), None);
    fx.api.services().user.set("profile", json!("gaming")).unwrap();
    assert_eq!(fx.api.current_profile().as_deref(), Some("gaming"));
}
