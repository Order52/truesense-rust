//! The charger signal, with a hold-off against bouncing.
//!
//! A plug or an unplug is an event that moves the power mode once (the native and Smart auto
//! engines act on it with the dwell timers cleared) and makes the daemon forget what it wrote to
//! the fans. A loose barrel connector, a flaky USB-C source or an adapter that renegotiates can
//! flip the sysfs `online` file several times in a second; acting on each flip would write the
//! power mode and the fans each time, and every power-mode write makes the firmware blink the
//! mode key's LED. So the first change is believed at once (a person who plugs in sees the mode
//! follow within the tick), and the next one only after [`PLUG_HOLDOFF_S`] seconds: a bounce
//! that flips and flips back inside the window leaves no trace at all.

/// How long after a believed change the next change has to wait. Three seconds is longer than
/// any contact bounce and shorter than anything a person notices.
pub(super) const PLUG_HOLDOFF_S: f64 = 3.0;

/// Filters the raw charger reading of each tick into the state the decisions use.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct PlugGuard {
    /// The state believed now and the monotonic time the last change was believed (minus
    /// infinity for the first reading); `None` before the first reading.
    accepted: Option<(bool, f64)>,
}

impl PlugGuard {
    /// The state to use for a raw reading `raw` taken at monotonic time `now`. The first reading
    /// is believed at once (there is nothing to bounce against).
    pub(super) fn filter(&mut self, raw: bool, now: f64) -> bool {
        match self.accepted {
            None => {
                // The first reading is no change: it must not hold the first real one off.
                self.accepted = Some((raw, f64::NEG_INFINITY));
                raw
            }
            Some((current, _)) if current == raw => current,
            Some((_, since)) if now - since >= PLUG_HOLDOFF_S => {
                self.accepted = Some((raw, now));
                raw
            }
            Some((current, _)) => current,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_reading_and_the_first_change_are_believed_at_once() {
        let mut p = PlugGuard::default();
        assert!(!p.filter(false, 0.0));
        assert!(p.filter(true, 0.5), "plugged in right after the start: still followed at once");
        let mut p = PlugGuard::default();
        assert!(!p.filter(false, 0.0));
        assert!(p.filter(true, 100.0), "plugged in long after: followed in the same tick");
        let mut p = PlugGuard::default();
        assert!(p.filter(true, 0.0));
        assert!(!p.filter(false, 50.0));
    }

    #[test]
    fn a_second_change_inside_the_hold_off_waits_for_it() {
        let mut p = PlugGuard::default();
        assert!(p.filter(true, 0.0));
        assert!(!p.filter(false, 10.0));
        assert!(!p.filter(true, 11.0), "flipped back one second after: held");
        assert!(!p.filter(true, 12.9), "still held");
        assert!(p.filter(true, 13.0), "three seconds after the change before: believed");
    }

    #[test]
    fn a_bouncing_contact_leaves_one_change_not_a_storm() {
        let mut p = PlugGuard::default();
        assert!(!p.filter(false, 0.0));
        // The cable is pushed in and the contact chatters for ten seconds.
        let mut changes = 0;
        let mut last = false;
        for k in 0..20 {
            let raw = k < 2 || k % 2 == 1;
            let t = 100.0 + f64::from(k) * 0.5;
            let seen = p.filter(raw, t);
            if seen != last {
                changes += 1;
                last = seen;
            }
        }
        assert!(changes <= 4, "{changes} changes in ten seconds of chatter");
        // Once it has settled on "plugged" the guard says so.
        assert!(p.filter(true, 200.0));
    }

    #[test]
    fn a_bounce_that_reverts_inside_the_window_leaves_no_trace() {
        let mut p = PlugGuard::default();
        assert!(p.filter(true, 0.0));
        assert!(!p.filter(false, 10.0), "a real unplug");
        // The chatter after it: on again for a moment, off again.
        assert!(!p.filter(true, 10.5), "held");
        assert!(!p.filter(false, 11.0));
        assert!(!p.filter(false, 20.0));
    }
}
