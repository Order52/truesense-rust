//! Fixture trees: a battery that is there, absent, partial, hostile, and the charge-limit sources.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::models::testutil::TempDir;

const SUPPLY: &str = "sys/class/power_supply";

/// The attributes of this laptop's own pack (BAT1) as the kernel printed them while it charged.
fn laptop(t: &TempDir, name: &str) {
    for (file, value) in [
        ("type", "Battery"),
        ("present", "1"),
        ("status", "Charging"),
        ("technology", "Li-ion"),
        ("cycle_count", "666"),
        ("voltage_min_design", "15400000"),
        ("voltage_now", "17091000"),
        ("current_now", "718000"),
        ("charge_full_design", "5884000"),
        ("charge_full", "4345000"),
        ("charge_now", "4195000"),
        ("capacity", "97"),
        ("capacity_level", "Normal"),
        ("model_name", "AP21A8T"),
        ("manufacturer", "LGC"),
        ("serial_number", "33823"),
    ] {
        t.write(&format!("{SUPPLY}/{name}/{file}"), &format!("{value}\n"));
    }
}

fn adapter(t: &TempDir, online: &str) {
    t.write(&format!("{SUPPLY}/ACAD/type"), "Mains\n");
    t.write(&format!("{SUPPLY}/ACAD/online"), &format!("{online}\n"));
}

fn read(t: &TempDir) -> BatteryReport {
    BatteryReader::new(t.path()).read()
}

#[test]
fn a_full_battery_is_read_with_converted_units() {
    let t = TempDir::new("bat-full");
    laptop(&t, "BAT1");
    adapter(&t, "1");
    let r = read(&t);
    assert!(r.present);
    assert_eq!(r.ac_online, Some(true));
    assert!(r.others.is_empty());
    let b = r.battery.unwrap();
    assert_eq!(b.name, "BAT1");
    assert_eq!(b.percent, Some(97));
    assert_eq!(b.status.as_deref(), Some("Charging"));
    assert_eq!(b.capacity_level.as_deref(), Some("Normal"));
    assert_eq!(b.technology.as_deref(), Some("Li-ion"));
    assert_eq!(b.manufacturer.as_deref(), Some("LGC"));
    assert_eq!(b.model.as_deref(), Some("AP21A8T"));
    assert_eq!(b.voltage_v, Some(17.091));
    assert_eq!(b.voltage_min_design_v, Some(15.4));
    assert_eq!(b.current_ma, Some(718.0));
    let power = b.power_w.unwrap();
    assert!((power - 17.091 * 0.718).abs() < 1e-9, "{power}");
    assert_eq!(b.charge_now_mah, Some(4195.0));
    assert_eq!(b.charge_full_mah, Some(4345.0));
    assert_eq!(b.charge_full_design_mah, Some(5884.0));
    assert_eq!(b.cycle_count, Some(666));
    let health = b.health_pct.unwrap();
    assert!((health - 4345.0 / 5884.0 * 100.0).abs() < 1e-9, "{health}");
    // No energy pair on this driver, no temperature file, no Acer driver.
    assert_eq!(b.energy_now_wh, None);
    assert_eq!(b.energy_full_wh, None);
    assert_eq!(b.temperature_c, None);
    assert_eq!(r.charge_limit, ChargeLimit::default());
}

#[test]
fn the_serial_number_is_never_read() {
    let t = TempDir::new("bat-serial");
    laptop(&t, "BAT1");
    let json = serde_json::to_string(&read(&t)).unwrap();
    assert!(!json.contains("33823"), "{json}");
    assert!(!json.contains("serial"), "{json}");
}

#[test]
fn no_battery_at_all_is_a_report_not_an_error() {
    let missing = TempDir::new("bat-none");
    let r = read(&missing);
    assert!(!r.present && r.battery.is_none() && r.others.is_empty());
    assert_eq!(r.ac_online, None);
    assert_eq!(r.charge_limit.limited, None);

    // The class directory exists but holds only the adapter: a desktop, or the pack removed.
    let ac_only = TempDir::new("bat-ac");
    adapter(&ac_only, "1");
    let r = read(&ac_only);
    assert!(!r.present && r.battery.is_none());
    assert_eq!(r.ac_online, Some(true));
}

#[test]
fn an_empty_bay_is_not_a_battery() {
    let t = TempDir::new("bat-bay");
    t.write(&format!("{SUPPLY}/BAT0/type"), "Battery\n");
    t.write(&format!("{SUPPLY}/BAT0/present"), "0\n");
    t.write(&format!("{SUPPLY}/BAT0/capacity"), "0\n");
    laptop(&t, "BAT1");
    let r = read(&t);
    assert_eq!(r.battery.unwrap().name, "BAT1");
    assert!(r.others.is_empty());
}

#[test]
fn a_directory_without_readable_attributes_is_skipped() {
    let t = TempDir::new("bat-hollow");
    t.mkdir(&format!("{SUPPLY}/BAT0"));
    assert!(!read(&t).present);
}

#[test]
fn a_partial_battery_keeps_what_it_has() {
    let t = TempDir::new("bat-partial");
    t.write(&format!("{SUPPLY}/BAT0/capacity"), "55\n");
    t.write(&format!("{SUPPLY}/BAT0/status"), "Discharging\n");
    t.write(&format!("{SUPPLY}/BAT0/voltage_now"), "11800000\n");
    let b = read(&t).battery.unwrap();
    assert_eq!(b.percent, Some(55));
    assert_eq!(b.status.as_deref(), Some("Discharging"));
    assert_eq!(b.voltage_v, Some(11.8));
    // The current is missing, so power cannot be derived: it is unknown, not zero.
    assert_eq!((b.current_ma, b.power_w, b.health_pct, b.cycle_count), (None, None, None, None));
    assert_eq!(b.present, None);
}

#[test]
fn power_now_wins_over_voltage_times_current_and_the_energy_pair_over_the_charge_pair() {
    let t = TempDir::new("bat-energy");
    for (f, v) in [
        ("capacity", "80"),
        ("power_now", "23500000"),
        ("voltage_now", "12000000"),
        ("current_now", "-1000000"),
        ("energy_now", "40000000"),
        ("energy_full", "50000000"),
        ("energy_full_design", "60000000"),
        ("charge_full", "1000"),
        ("charge_full_design", "1000"),
    ] {
        t.write(&format!("{SUPPLY}/BAT0/{f}"), v);
    }
    let b = read(&t).battery.unwrap();
    assert_eq!(b.power_w, Some(23.5));
    // The sign of the current is the driver's business: it is a magnitude here.
    assert_eq!(b.current_ma, Some(1000.0));
    assert_eq!(b.energy_now_wh, Some(40.0));
    assert_eq!(b.energy_full_wh, Some(50.0));
    assert_eq!(b.energy_full_design_wh, Some(60.0));
    let health = b.health_pct.unwrap();
    assert!((health - 50.0 / 60.0 * 100.0).abs() < 1e-9, "{health}");
}

#[test]
fn a_new_pack_may_hold_more_than_its_label() {
    let t = TempDir::new("bat-new");
    t.write(&format!("{SUPPLY}/BAT0/charge_full"), "5100000");
    t.write(&format!("{SUPPLY}/BAT0/charge_full_design"), "5000000");
    let health = read(&t).battery.unwrap().health_pct.unwrap();
    assert!((health - 102.0).abs() < 1e-9, "{health}");
}

#[test]
fn hostile_values_become_none() {
    let t = TempDir::new("bat-hostile");
    for (f, v) in [
        ("capacity", "101"),
        ("voltage_now", "99999999999"),
        ("voltage_min_design", "-5"),
        ("current_now", "banana"),
        ("charge_now", "-1"),
        ("charge_full", "0x10"),
        ("charge_full_design", "0"),
        ("cycle_count", "-3"),
        ("temp", "9999"),
        ("status", "   "),
        ("technology", "\u{1b}[31mLi\u{7}-ion\n\n"),
        ("model_name", &"x".repeat(500)),
    ] {
        t.write(&format!("{SUPPLY}/BAT0/{f}"), v);
    }
    let b = read(&t).battery.unwrap();
    assert_eq!(b.percent, None);
    assert_eq!(b.voltage_v, None);
    assert_eq!(b.voltage_min_design_v, None);
    assert_eq!((b.current_ma, b.power_w), (None, None));
    assert_eq!((b.charge_now_mah, b.charge_full_mah), (None, None));
    assert_eq!(b.health_pct, None, "a zero design capacity must not divide");
    assert_eq!(b.cycle_count, None);
    assert_eq!(b.temperature_c, None);
    assert_eq!(b.status, None);
    assert_eq!(b.technology.as_deref(), Some("[31mLi-ion"), "control characters are removed");
    assert_eq!(b.model.as_ref().map(String::len), Some(TEXT_LIMIT));
}

#[test]
fn an_oversized_or_binary_file_is_ignored() {
    let t = TempDir::new("bat-big");
    t.write(&format!("{SUPPLY}/BAT0/capacity"), "50");
    t.write(&format!("{SUPPLY}/BAT0/status"), &"A".repeat(ATTRIBUTE_LIMIT as usize + 1));
    let path = t.path().join(format!("{SUPPLY}/BAT0/model_name"));
    std::fs::write(path, [0xff, 0xfe, 0x00, 0x41]).unwrap();
    let b = read(&t).battery.unwrap();
    assert_eq!(b.percent, Some(50));
    assert_eq!((b.status, b.model), (None, None));
}

#[test]
fn only_bat_directories_of_type_battery_count_and_they_come_in_name_order() {
    let t = TempDir::new("bat-order");
    laptop(&t, "BAT1");
    laptop(&t, "BAT0");
    // A game controller lists itself under the class as a Battery too, but it is not ours.
    t.write(&format!("{SUPPLY}/ps-controller-battery-aa/type"), "Battery\n");
    t.write(&format!("{SUPPLY}/ps-controller-battery-aa/capacity"), "12\n");
    // A BAT-named thing that says it is something else.
    t.write(&format!("{SUPPLY}/BAT9/type"), "UPS\n");
    t.write(&format!("{SUPPLY}/BAT9/capacity"), "33\n");
    let r = read(&t);
    assert_eq!(r.battery.unwrap().name, "BAT0");
    assert_eq!(r.others.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["BAT1"]);
}

#[test]
fn a_battery_without_a_type_file_still_counts_when_named_bat() {
    let t = TempDir::new("bat-notype");
    t.write(&format!("{SUPPLY}/BAT0/capacity"), "40");
    assert_eq!(read(&t).battery.unwrap().percent, Some(40));
}

#[test]
fn the_adapter_is_on_when_any_mains_supply_is() {
    let t = TempDir::new("bat-mains");
    adapter(&t, "0");
    assert_eq!(read(&t).ac_online, Some(false));
    t.write(&format!("{SUPPLY}/ADP1/type"), "Mains\n");
    t.write(&format!("{SUPPLY}/ADP1/online"), "1\n");
    assert_eq!(read(&t).ac_online, Some(true));
    // An adapter entry with a broken file is unknown, not off.
    let u = TempDir::new("bat-mains-broken");
    u.write(&format!("{SUPPLY}/ACAD/type"), "Mains\n");
    u.write(&format!("{SUPPLY}/ACAD/online"), "maybe\n");
    assert_eq!(read(&u).ac_online, None);
}

#[test]
fn the_temperature_comes_from_the_first_source_that_has_one() {
    // Tenths of a degree in the power-supply file.
    let a = TempDir::new("bat-temp-a");
    laptop(&a, "BAT1");
    a.write(&format!("{SUPPLY}/BAT1/temp"), "312\n");
    a.write(&format!("{SUPPLY}/BAT1/hwmon2/temp1_input"), "40000\n");
    assert_eq!(read(&a).battery.unwrap().temperature_c, Some(31.2));

    // The battery's own hwmon chip, in thousandths.
    let b = TempDir::new("bat-temp-b");
    laptop(&b, "BAT1");
    b.write(&format!("{SUPPLY}/BAT1/hwmon2/temp1_input"), "29500\n");
    assert_eq!(read(&b).battery.unwrap().temperature_c, Some(29.5));

    // The Acer driver, in thousandths.
    let c = TempDir::new("bat-temp-c");
    laptop(&c, "BAT1");
    c.write("sys/bus/wmi/drivers/acer-wmi-battery/temperature", "27300\n");
    assert_eq!(read(&c).battery.unwrap().temperature_c, Some(27.3));

    // A broken first source falls through to the next one.
    let d = TempDir::new("bat-temp-d");
    laptop(&d, "BAT1");
    d.write(&format!("{SUPPLY}/BAT1/temp"), "-9000\n");
    d.write("sys/bus/wmi/drivers/acer-wmi-battery/temperature", "30000\n");
    assert_eq!(read(&d).battery.unwrap().temperature_c, Some(30.0));
}

#[test]
fn the_charge_limit_sources_are_read_and_summarised() {
    let t = TempDir::new("bat-limit");
    laptop(&t, "BAT1");
    t.write(&format!("{SUPPLY}/BAT1/charge_control_end_threshold"), "80\n");
    t.write(&format!("{SUPPLY}/BAT1/charge_control_start_threshold"), "75\n");
    t.write("sys/bus/wmi/drivers/acer-wmi-battery/health_mode", "0\n");
    t.write("sys/bus/wmi/drivers/acer-wmi-battery/calibration_mode", "1\n");
    let limit = read(&t).charge_limit;
    assert_eq!(limit.end_threshold_pct, Some(80));
    assert_eq!(limit.start_threshold_pct, Some(75));
    assert_eq!(limit.health_mode, Some(false));
    assert_eq!(limit.calibration_mode, Some(true));
    assert_eq!(limit.limited, Some(true), "an end threshold below 100 is a limit");
}

#[test]
fn health_mode_minus_one_means_the_firmware_has_no_such_function() {
    let t = TempDir::new("bat-limit-unsupported");
    laptop(&t, "BAT1");
    t.write("sys/bus/wmi/drivers/acer-wmi-battery/health_mode", "-1\n");
    let limit = read(&t).charge_limit;
    assert_eq!(limit.health_mode, None);
    assert_eq!(limit.limited, None);
}

#[test]
fn the_linuwu_sense_name_of_the_health_mode_is_the_fallback() {
    let t = TempDir::new("bat-limit-linuwu");
    laptop(&t, "BAT1");
    t.write("sys/bus/platform/drivers/acer-wmi/acer-wmi/predator_sense/battery_limiter", "1\n");
    let limit = read(&t).charge_limit;
    assert_eq!(limit.health_mode, Some(true));
    assert_eq!(limit.limited, Some(true));
}

#[test]
fn a_threshold_of_one_hundred_is_no_limit_and_the_firmware_answer_joins_the_summary() {
    let t = TempDir::new("bat-limit-100");
    laptop(&t, "BAT1");
    t.write(&format!("{SUPPLY}/BAT1/charge_control_end_threshold"), "100\n");
    t.write(&format!("{SUPPLY}/BAT1/charge_control_end_threshold_junk"), "1\n");
    let limit = read(&t).charge_limit;
    assert_eq!(limit.limited, Some(false));
    assert_eq!(limit.with_firmware_limit(Some(true)).limited, Some(true));
    assert_eq!(limit.with_firmware_limit(Some(false)).limited, Some(false));
    assert_eq!(limit.with_firmware_limit(None).limited, Some(false));
    // Nothing known at all stays unknown.
    assert_eq!(ChargeLimit::default().with_firmware_limit(None).limited, None);
    assert_eq!(ChargeLimit::default().with_firmware_limit(Some(true)).limited, Some(true));
}

#[test]
fn an_out_of_range_threshold_is_ignored() {
    let t = TempDir::new("bat-limit-range");
    laptop(&t, "BAT1");
    t.write(&format!("{SUPPLY}/BAT1/charge_control_end_threshold"), "250\n");
    assert_eq!(read(&t).charge_limit.end_threshold_pct, None);
}

#[test]
fn the_report_survives_a_json_round_trip_with_nulls() {
    let t = TempDir::new("bat-json");
    t.write(&format!("{SUPPLY}/BAT0/capacity"), "60");
    let report = read(&t);
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["battery"]["percent"], 60);
    assert!(json["battery"]["voltage_v"].is_null(), "{json}");
    assert!(json["ac_online"].is_null());
    let back: BatteryReport = serde_json::from_value(json).unwrap();
    assert_eq!(back, report);
}

#[test]
fn the_one_line_summary_leaves_out_what_is_unknown() {
    let t = TempDir::new("bat-display");
    assert_eq!(read(&t).to_string(), "no battery");
    laptop(&t, "BAT1");
    assert_eq!(read(&t).to_string(), "97 % Charging, 17.09 V, 12.3 W, health 73.8 %, 666 cycles");
    let u = TempDir::new("bat-display-partial");
    u.write(&format!("{SUPPLY}/BAT0/status"), "Full");
    assert_eq!(read(&u).to_string(), "Full");
}
