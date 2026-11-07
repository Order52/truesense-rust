//! `alerts get` and `alerts set` through a real loopback request: the settings the page would
//! read and write, validated before anything reaches the configuration file.

use serde_json::json;

use super::harness::Machine;

fn user_config(m: &Machine) -> String {
    std::fs::read_to_string(m.root.0.join("home/config.toml")).unwrap_or_default()
}

#[test]
fn get_answers_the_settings_in_force() {
    let m = Machine::start();
    assert_eq!(
        m.ok("alerts", "get", &json!({})),
        json!({
            "enabled": true,
            "cpu_limit_c": 90,
            "gpu_limit_c": 90,
            "repeat_minutes": 5,
            "debounce_samples": 3,
            "battery_low_pct": 15,
            "throttling": true,
            "sensor_lost": true
        })
    );
}

#[test]
fn set_changes_only_what_is_given_and_answers_the_new_settings() {
    let m = Machine::start();
    let now = m.ok("alerts", "set", &json!({"cpu_limit_c": 85, "enabled": false, "unknown": 1}));
    assert_eq!(now["cpu_limit_c"], json!(85));
    assert_eq!(now["enabled"], json!(false));
    assert_eq!(now["gpu_limit_c"], json!(90));
    let text = user_config(&m);
    assert!(text.contains("[alerts]") && text.contains("cpu_limit_c = 85"), "{text}");
    assert!(!text.contains("gpu_limit_c"), "{text}");
    assert_eq!(m.ok("alerts", "get", &json!({})), now);
    // null counts as absent, as everywhere on the bus.
    let again =
        m.ok("alerts", "set", &json!({"battery_low_pct": 0, "cpu_limit_c": null, "throttling": false}));
    assert_eq!((again["cpu_limit_c"].clone(), again["battery_low_pct"].clone()), (json!(85), json!(0)));
    assert_eq!(again["throttling"], json!(false));
}

#[test]
fn set_refuses_what_is_out_of_range_and_writes_nothing() {
    let m = Machine::start();
    let why = m.refused("alerts", "set", &json!({"cpu_limit_c": 50}));
    assert!(why.contains("cpu_limit_c must be 70..100"), "{why}");
    let why = m.refused("alerts", "set", &json!({"battery_low_pct": 51}));
    assert!(why.contains("battery_low_pct"), "{why}");
    assert_eq!(user_config(&m), "");
    assert!(m.refused("alerts", "set", &json!({})).contains("nothing to change"));
    assert!(m.refused("alerts", "set", &json!({"cpu_limit_c": "hot"})).contains("cpu_limit_c"));
    assert!(m.refused("alerts", "set", &json!({"enabled": "yes"})).contains("enabled"));
    assert!(m.refused("alerts", "set", &json!({"gpu_limit_c": 90.5})).contains("gpu_limit_c"));
    assert_eq!(user_config(&m), "");
}
