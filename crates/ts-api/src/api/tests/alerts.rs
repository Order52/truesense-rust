//! The alert settings: read, change, validate before writing, keep the rest of the file.

use ts_foundation::config::AlertsConfig;

use super::Fx;
use crate::api::{AlertsEdit, describe_alerts};

fn edit(f: impl FnOnce(&mut AlertsEdit)) -> AlertsEdit {
    let mut e = AlertsEdit::default();
    f(&mut e);
    e
}

#[test]
fn the_defaults_are_read_without_a_file() {
    let fx = Fx::new();
    assert_eq!(fx.api.alerts().unwrap(), AlertsConfig::default());
    assert_eq!(fx.user_config(), "", "reading writes nothing");
}

#[test]
fn only_the_given_members_are_written_and_the_rest_of_the_file_is_kept() {
    let fx = Fx::new();
    std::fs::create_dir_all(fx.dir.0.join("home")).unwrap();
    std::fs::write(fx.dir.0.join("home/config.toml"), "[startup]\nkeyboard = \"off\"\n").unwrap();
    let now = fx
        .api
        .set_alerts(&edit(|e| {
            e.cpu_limit_c = Some(85);
            e.enabled = Some(false);
        }))
        .unwrap();
    assert!(!now.enabled);
    assert_eq!((now.cpu_limit_c, now.gpu_limit_c), (85, 90));
    let text = fx.user_config();
    assert!(
        text.contains("[alerts]") && text.contains("cpu_limit_c = 85") && text.contains("enabled = false"),
        "{text}"
    );
    assert!(
        !text.contains("gpu_limit_c") && !text.contains("repeat_minutes"),
        "untouched keys stay unwritten:\n{text}"
    );
    assert!(text.contains("keyboard = \"off\""), "{text}");
    // A second change merges with the first.
    let now = fx.api.set_alerts(&edit(|e| e.battery_low_pct = Some(0))).unwrap();
    assert_eq!((now.cpu_limit_c, now.battery_low_pct, now.enabled), (85, 0, false));
    assert_eq!(fx.api.alerts().unwrap(), now, "what is answered is what is read back");
}

#[test]
fn a_bad_value_is_refused_before_anything_is_written() {
    let fx = Fx::new();
    for (e, want) in [
        (edit(|e| e.cpu_limit_c = Some(60)), "cpu_limit_c must be 70..100"),
        (edit(|e| e.gpu_limit_c = Some(101)), "gpu_limit_c must be 70..100"),
        (edit(|e| e.repeat_minutes = Some(0)), "repeat_minutes must be 1..240"),
        (edit(|e| e.debounce_samples = Some(99)), "debounce_samples must be 1..30"),
        (edit(|e| e.battery_low_pct = Some(3)), "battery_low_pct must be 0 (off) or 5..50"),
    ] {
        let err = fx.api.set_alerts(&e).unwrap_err().to_string();
        assert!(err.contains(want), "{err}");
    }
    assert_eq!(fx.user_config(), "");
    let err = fx.api.set_alerts(&AlertsEdit::default()).unwrap_err().to_string();
    assert_eq!(err, "alerts set: nothing to change");
}

#[test]
fn a_broken_configuration_file_is_an_error_not_a_silent_default() {
    let fx = Fx::new();
    std::fs::create_dir_all(fx.dir.0.join("home")).unwrap();
    std::fs::write(fx.dir.0.join("home/config.toml"), "[alerts\n").unwrap();
    assert!(fx.api.alerts().is_err());
    assert!(fx.api.set_alerts(&edit(|e| e.enabled = Some(true))).is_err());
    assert_eq!(fx.user_config(), "[alerts\n", "left as it was");
}

#[test]
fn the_edit_reads_from_json_and_refuses_unknown_members() {
    let ok: AlertsEdit =
        serde_json::from_value(serde_json::json!({"cpu_limit_c": 80, "throttling": false})).unwrap();
    assert_eq!((ok.cpu_limit_c, ok.throttling, ok.enabled), (Some(80), Some(false), None));
    assert!(serde_json::from_value::<AlertsEdit>(serde_json::json!({"cpu_limit": 80})).is_err());
    assert!(serde_json::from_value::<AlertsEdit>(serde_json::json!({"cpu_limit_c": "hot"})).is_err());
    assert!(serde_json::from_value::<AlertsEdit>(serde_json::json!({})).unwrap().is_empty());
}

#[test]
fn the_settings_read_as_lines() {
    let text = describe_alerts(&AlertsConfig::default());
    assert_eq!(
        text,
        "alerts: on\n  CPU limit      90 C (again after it drops 5 C)\n  GPU limit      90 C (again after it drops 5 C)\n  repeat         at most every 5 min per alert\n  debounce       3 samples in a row (one or two seconds each)\n  battery low    15 %\n  throttling     on\n  sensor lost    on"
    );
    let off = AlertsConfig { enabled: false, battery_low_pct: 0, ..AlertsConfig::default() };
    let text = describe_alerts(&off);
    assert!(text.starts_with("alerts: off") && text.contains("battery low    off"), "{text}");
}
