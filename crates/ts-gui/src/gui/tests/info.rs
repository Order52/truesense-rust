//! `battery info` and `capabilities get` through a real loopback request on the simulated laptop.

use serde_json::json;

use super::harness::Machine;

#[test]
fn battery_info_is_null_not_an_error_where_the_kernel_says_nothing() {
    let m = Machine::start();
    let dir = m.root.0.join("sys/class/power_supply/BAT1");
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(dir.join("capacity"), "81\n").expect("capacity");
    std::fs::write(dir.join("status"), "Discharging\n").expect("status");
    let info = m.ok("battery", "info", &json!({}));
    assert_eq!(info["present"], json!(true));
    assert_eq!(info["battery"]["percent"], json!(81));
    assert_eq!(info["battery"]["status"], json!("Discharging"));
    assert!(info["battery"]["voltage_v"].is_null());
    assert!(info["battery"]["health_pct"].is_null());
    assert!(info["ac_online"].is_null());
    assert_eq!(info["others"], json!([]));
    // The simulated firmware's 80 % limit is off, and the summary says so.
    assert_eq!(info["charge_limit"]["firmware_limit_80"], json!(false));
    assert_eq!(info["charge_limit"]["limited"], json!(false));
}

#[test]
fn battery_info_without_a_battery_is_an_answer() {
    let m = Machine::start();
    let info = m.ok("battery", "info", &json!({}));
    assert_eq!(info["present"], json!(false));
    assert!(info["battery"].is_null());
}

#[test]
fn capabilities_get_lists_every_feature_with_a_reason_when_not_supported() {
    let m = Machine::start();
    let caps = m.ok("capabilities", "get", &json!({}));
    assert_eq!(caps["simulated"], json!(true));
    assert_eq!(caps["firmware_answered"], json!("14/14"));
    let items = caps["items"].as_array().expect("items");
    assert_eq!(items.len(), 22);
    for item in items {
        let supported = item["supported"].as_bool().expect("supported");
        assert_eq!(item["why"].is_string(), !supported, "{item}");
        assert!(item["label"].is_string() && item["source"].is_string(), "{item}");
    }
    let find = |id: &str| items.iter().find(|i| i["id"] == id).expect("id");
    assert_eq!(find("power_modes")["supported"], json!(true));
    assert_eq!(find("gpu_power_limit")["supported"], json!(false));
}
