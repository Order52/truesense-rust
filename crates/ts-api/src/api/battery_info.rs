//! The Battery page's readings: voltage, current, power, wear, cycles, temperature and the
//! charge-limit state, in one call.
//!
//! The kernel files are read by [`ts_system::battery`]; the one thing only this layer can add is
//! TrueSense's own firmware limit, asked through the firmware's read-only battery query. Nothing
//! here writes anything, and a part that cannot be read is `null`, not an error: a laptop
//! without a battery, a driver without a temperature, or a missing kernel module still gives an
//! answer the page can draw.
#![forbid(unsafe_code)]

use ts_system::battery::{BatteryReader, BatteryReport};

use super::Api;

impl Api {
    /// Everything the machine says about its battery now, with the firmware's 80 % limit folded
    /// into the charge-limit summary. The firmware is only asked when there is a battery, and
    /// an answer it cannot give leaves the field `null`.
    pub fn battery_info(&self) -> BatteryReport {
        let mut report = BatteryReader::new(self.s.root.as_path()).read();
        if report.present {
            let firmware_limit = self.s.hw.get_battery_limit().ok();
            report.charge_limit = report.charge_limit.with_firmware_limit(firmware_limit);
        }
        report
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use serde_json::json;

    use crate::api::tests::Fx;

    fn supply(fx: &Fx, file: &str, value: &str) {
        let path = fx.dir.0.join("sys/class/power_supply/BAT1").join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, value).unwrap();
    }

    #[test]
    fn a_machine_without_a_battery_answers_with_nothing_present() {
        let fx = Fx::offline();
        let report = fx.api.battery_info();
        assert!(!report.present);
        assert!(report.battery.is_none());
        // Without a battery the firmware is not even asked about its limit.
        assert_eq!(report.charge_limit.firmware_limit_80, None);
        assert_eq!(report.charge_limit.limited, None);
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["present"], json!(false));
        assert!(json["battery"].is_null());
    }

    #[test]
    fn the_battery_is_read_below_the_services_root_and_the_firmware_limit_joins_the_summary() {
        let fx = Fx::offline();
        supply(&fx, "capacity", "64\n");
        supply(&fx, "status", "Discharging\n");
        supply(&fx, "voltage_now", "15000000\n");
        supply(&fx, "current_now", "2000000\n");
        let report = fx.api.battery_info();
        let battery = report.battery.as_ref().unwrap();
        assert_eq!(battery.percent, Some(64));
        assert_eq!(battery.power_w, Some(30.0));
        // The simulated firmware starts with the limit off.
        assert_eq!(report.charge_limit.firmware_limit_80, Some(false));
        assert_eq!(report.charge_limit.limited, Some(false));

        fx.api.set_battery_limit(true).unwrap();
        let report = fx.api.battery_info();
        assert_eq!(report.charge_limit.firmware_limit_80, Some(true));
        assert_eq!(report.charge_limit.limited, Some(true));
    }

    #[test]
    fn reading_the_battery_changes_nothing() {
        let fx = Fx::offline();
        supply(&fx, "capacity", "50\n");
        let before = fx.rig.sim.snapshot();
        let _ = fx.api.battery_info();
        assert_eq!(fx.rig.sim.snapshot(), before);
    }
}
