//! The limits and name lists the pages show, read from the layers that enforce them.
//!
//! The view-models used to carry their own copies of these numbers ("mirrors") and pin them with
//! tests, because this crate was written as if it could not depend on the decision and domain
//! layers. It can (they are Qt-free), so the page and the daemon can no longer disagree: a limit
//! that changes below changes here. The tests of the view-models still write the important values
//! out literally, so changing a safety limit is a deliberate act that fails a test first.
#![forbid(unsafe_code)]

use ts_decision::smartprofile::{MODE_SETTINGS, Mode, NUM_SETTINGS};

/// The power modes, slowest to fastest: the rank order of the decision layer.
pub(crate) const POWER_MODES: [&str; Mode::ALL.len()] = {
    let mut names = [""; Mode::ALL.len()];
    let mut i = 0;
    while i < names.len() {
        names[i] = Mode::ALL[i].name();
        i += 1;
    }
    names
};

/// The mode-valued settings of a Smart auto profile, in display order.
pub(crate) const SMART_MODE_KEYS: [&str; MODE_SETTINGS.len()] = {
    let mut keys = [""; MODE_SETTINGS.len()];
    let mut i = 0;
    while i < keys.len() {
        keys[i] = MODE_SETTINGS[i].key;
        i += 1;
    }
    keys
};

/// The numeric settings of a Smart auto profile with their inclusive ranges, in the daemon's order.
pub(crate) const SMART_NUMBERS: [(&str, i64, i64); NUM_SETTINGS.len()] = {
    let mut numbers = [("", 0, 0); NUM_SETTINGS.len()];
    let mut i = 0;
    while i < numbers.len() {
        let s = NUM_SETTINGS[i];
        numbers[i] = (s.key, s.min as i64, s.max as i64);
        i += 1;
    }
    numbers
};

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn the_power_modes_are_the_five_in_rank_order() {
        assert_eq!(POWER_MODES, ["eco", "quiet", "balanced", "performance", "turbo"]);
    }

    #[test]
    fn the_smart_settings_are_the_daemons_written_out_once_more_on_purpose() {
        assert_eq!(
            SMART_MODE_KEYS,
            [
                "ac_idle",
                "ac_browsing",
                "ac_busy",
                "battery_idle",
                "battery_browsing",
                "battery_busy",
                "battery_hot"
            ]
        );
        assert_eq!(
            SMART_NUMBERS,
            [
                ("turbo_after_s", 5, 300),
                ("turbo_max_cpu", 60, 95),
                ("turbo_max_gpu", 55, 85),
                ("backoff_hold_s", 10, 600),
                ("idle_after_s", 10, 600),
                ("battery_low_pct", 5, 50),
                ("up_dwell_s", 2, 120),
                ("down_dwell_s", 2, 300),
                ("busy_hold_s", 0, 600),
                ("min_change_s", 2, 600),
                ("turbo_margin_c", 0, 20),
                ("turbo_retry_s", 10, 900),
            ]
        );
    }
}
