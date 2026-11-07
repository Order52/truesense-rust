use serde_json::json;
use ts_decision::smartprofile::{self, Mode};

use super::Fx;
use crate::api::{Api, SMART_SETTING_KEYS, describe_smart_profile};

fn in_use(name: &str) -> serde_json::Value {
    let p = smartprofile::builtin(name).unwrap();
    json!({"name": name, "profile": p.to_json(), "names": ["native", "truesense"]})
}

#[test]
fn the_list_marks_the_profile_the_service_runs() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok("smart_profile", in_use("truesense"));
    let list = fx.api.smart_profiles().unwrap();
    assert_eq!(list.current.as_deref(), Some("truesense"));
    assert!(list.descriptions.contains_key("native"));
    assert!(list.to_string().contains("* truesense"));
    assert_eq!(fx.api.smart_profile_in_use().unwrap().profile, smartprofile::builtin("truesense").unwrap());
    assert_eq!(Fx::offline().api.smart_profiles().unwrap().current, None);
    fx.rig.daemon.answer_ok("smart_profile", json!({"name": "x", "profile": {"turbo_after_s": 1}}));
    assert!(fx.api.smart_profile_in_use().is_err());
}

#[test]
fn a_built_in_goes_by_name_and_a_saved_one_travels_as_data() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok("smart_profile", json!({"name": "native"}));
    assert_eq!(fx.api.use_smart_profile("native").unwrap(), "native");
    assert_eq!(fx.rig.daemon.last_args("smart_profile").unwrap().get("data"), Some(&json!(null)));
    fx.api.save_smart_profile("mine", None, &["ac_busy=turbo".into()]).unwrap();
    fx.rig.daemon.answer_ok("smart_profile", json!({"name": "mine"}));
    fx.api.use_smart_profile("mine").unwrap();
    let data = fx.rig.daemon.last_args("smart_profile").unwrap().get("data").cloned().unwrap();
    assert_eq!(data["ac_busy"], json!("turbo"));
    assert_eq!(
        fx.api.use_smart_profile("").unwrap_err().to_string(),
        "which profile? (truesense power smart list)"
    );
    assert!(fx.api.use_smart_profile("nosuch").is_err());
}

#[test]
fn settings_are_applied_by_key_and_checked_by_type_and_range() {
    let fx = Fx::new();
    let p = fx
        .api
        .save_smart_profile(
            "mine",
            Some("truesense"),
            &[
                "ac_turbo=off".into(),
                "turbo_after_s=45".into(),
                "battery_hot=eco".into(),
                "ac_idle=quiet".into(),
            ],
        )
        .unwrap();
    assert!(!p.ac_turbo);
    assert_eq!(p.turbo_after_s, 45);
    assert_eq!((p.battery_hot, p.ac_idle), (Mode::Eco, Mode::Quiet));
    for yes in ["1", "true", "YES", "on"] {
        let p = fx.api.save_smart_profile("mine2", None, &[format!("ac_turbo={yes}")]).unwrap();
        assert!(p.ac_turbo, "{yes}");
    }
    let keys = SMART_SETTING_KEYS.join(", ");
    for bad in ["nokey=1", "ac_turbo=", "ac_turbo", "=5"] {
        let e = fx.api.save_smart_profile("x", None, &[bad.into()]).unwrap_err();
        assert_eq!(e.to_string(), format!("--set key=value, key one of: {keys}"), "{bad}");
    }
    assert_eq!(
        fx.api.save_smart_profile("x", None, &["turbo_after_s=soon".into()]).unwrap_err().to_string(),
        "turbo_after_s must be a whole number, not 'soon'"
    );
    assert!(
        fx.api
            .save_smart_profile("x", None, &["turbo_after_s=1".into()])
            .unwrap_err()
            .to_string()
            .contains("turbo_after_s")
    );
    assert!(fx.api.save_smart_profile("x", None, &["ac_busy=warp".into()]).is_err());
    assert_eq!(fx.api.save_smart_profile("", None, &[]).unwrap_err().to_string(), "save under which name?");
    assert!(fx.api.save_smart_profile("native", None, &[]).is_err(), "built-in profiles are read-only");
}

#[test]
fn editing_your_own_profile_by_name_keeps_the_rest() {
    let fx = Fx::new();
    fx.api.save_smart_profile("mine", None, &["turbo_after_s=77".into()]).unwrap();
    let again = fx.api.save_smart_profile("mine", None, &["ac_busy=performance".into()]).unwrap();
    assert_eq!(again.turbo_after_s, 77);
    assert_eq!(fx.api.smart_profile("mine").unwrap().ac_busy, Mode::Performance);
    assert_eq!(fx.api.copy_smart_profile("mine", None).unwrap(), "mine-copy");
    assert_eq!(fx.api.copy_smart_profile("mine", None).unwrap(), "mine-copy-2");
    assert_eq!(fx.api.copy_smart_profile("", None).unwrap_err().to_string(), "copy which profile?");
    fx.api.delete_smart_profile("mine-copy").unwrap();
    assert!(fx.api.delete_smart_profile("truesense").is_err());
    assert!(Api::smart_profile_is_builtin("truesense"));
}

#[test]
fn an_edited_profile_is_checked_before_it_is_sent_or_saved() {
    let fx = Fx::new();
    let settings = json!({"turbo_after_s": 20, "ac_busy": "turbo"});
    fx.rig.daemon.answer_ok("smart_profile", json!({"name": "custom"}));
    assert_eq!(fx.api.use_smart_profile_data("", &settings).unwrap(), "custom");
    let sent = fx.rig.daemon.last_args("smart_profile").unwrap().get("data").cloned().unwrap();
    assert_eq!(sent["turbo_after_s"], json!(20));
    assert_eq!(
        sent["up_dwell_s"],
        smartprofile::Profile::default().to_json()["up_dwell_s"],
        "missing settings take their defaults"
    );
    fx.rig.daemon.clear_calls();
    assert!(fx.api.use_smart_profile_data("x", &json!({"turbo_after_s": 1})).is_err());
    assert!(fx.api.use_smart_profile_data("x", &json!([1])).is_err());
    assert!(fx.rig.daemon.calls().is_empty());
    assert_eq!(fx.api.save_smart_profile_data("edited", &settings).unwrap().turbo_after_s, 20);
    assert!(fx.api.save_smart_profile_data("", &settings).is_err());
}

#[test]
fn a_profile_prints_every_setting_in_the_old_order() {
    let text = describe_smart_profile(&smartprofile::builtin("truesense").unwrap());
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), SMART_SETTING_KEYS.len());
    assert!(lines[0].starts_with("  ac_idle = "));
    assert!(
        lines[3].starts_with("  ac_turbo = ") && (lines[3].ends_with("True") || lines[3].ends_with("False"))
    );
    let shown = crate::api::SmartProfileInUse {
        name: "truesense".into(),
        profile: smartprofile::builtin("truesense").unwrap(),
        names: vec![],
    };
    assert!(shown.to_string().starts_with("Smart auto profile in use: truesense\n  ac_idle = "));
}
