//! "Charge to 100 % once": when that one-off charge is over.
//!
//! The 80 % battery limit protects the battery, and the user lifts it for a single charge (a
//! trip tomorrow). It must come back by itself or it would stay off for good, so the charge is
//! over when the charger is unplugged, when the battery has been full for five minutes, or
//! after 24 hours, whichever comes first.
//!
//! [`check`] is a pure function of the record the daemon keeps (in its state file under
//! `charge_full`), the charger, the battery level and the clock. Putting the limit back on the
//! firmware and clearing the record is the caller's job; if that fails the record stays and
//! the next check (every five ticks) tries again.
#![forbid(unsafe_code)]

use serde_json::{Map, Value, json};

/// A charge that has not ended after this many seconds ends anyway.
pub(crate) const MAX_DURATION_S: f64 = 86_400.0;
/// The battery must have been seen full for more than this many seconds.
pub(crate) const FULL_FOR_S: f64 = 300.0;
/// The daemon looks at the record every this many ticks.
pub const CHECK_EVERY_TICKS: u64 = 5;

/// The record of a running full charge, as stored (`{"since": ..., "full_at": ...}`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct State {
    /// Unix seconds when the charge started.
    pub since: f64,
    /// Unix seconds when the battery was first seen full.
    pub full_at: Option<f64>,
}

impl State {
    /// A record for a charge that starts now.
    pub fn start(now: f64) -> State {
        State { since: now, full_at: None }
    }

    /// Reads the stored value. `None` means no charge is running: `null`, `{}` and the other
    /// empty values the Python edition treated as false.
    ///
    /// A record that exists but has no usable `since` (damaged by hand) gets `since = 0`, so it
    /// ends at the next check as "24 hours passed" and the limit comes back. The Python edition
    /// raised an error here every tick and never restored the limit.
    pub fn from_json(v: &Value) -> Option<State> {
        let truthy = match v {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
            Value::String(s) => !s.is_empty(),
            Value::Array(a) => !a.is_empty(),
            Value::Object(o) => !o.is_empty(),
        };
        if !truthy {
            return None;
        }
        let num = |key: &str| v.get(key).and_then(Value::as_f64).filter(|f| f.is_finite());
        // A full_at of 0 counts as "not seen full yet", as in Python (`full_at or now`).
        Some(State { since: num("since").unwrap_or(0.0), full_at: num("full_at").filter(|f| *f != 0.0) })
    }

    /// The stored form.
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("since".into(), json!(self.since));
        m.insert("full_at".into(), self.full_at.map_or(Value::Null, |f| json!(f)));
        Value::Object(m)
    }
}

/// Why a full charge ended; [`Reason::text`] is the wording of the journal and the notice
/// ("80 % charge limit is back (unplugged)").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The charger was unplugged.
    Unplugged,
    /// 24 hours passed.
    DayPassed,
    /// The battery has been full for five minutes.
    BatteryFull,
}

impl Reason {
    /// The words that go inside the parentheses of the message.
    pub fn text(self) -> &'static str {
        match self {
            Reason::Unplugged => "unplugged",
            Reason::DayPassed => "24 hours passed",
            Reason::BatteryFull => "the battery is full",
        }
    }
}

/// The outcome of one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verdict {
    /// Why the charge is over, `None` while it continues. The caller then puts the 80 % limit
    /// back, and only when that worked clears the record.
    pub done: Option<Reason>,
    /// The record changed (the battery was just seen full): the caller must store it.
    pub changed: bool,
}

/// Looks at the situation. Unplugging wins over everything, then the 24 hours, then the
/// battery level (so a charge never needs the battery reading to end).
///
/// The first time the battery is at 100 % the moment is recorded; five minutes (strictly more)
/// after it the charge ends. `now` is unix seconds, `battery_pct` the charge in percent.
pub fn check(state: &mut State, ac: bool, battery_pct: f64, now: f64) -> Verdict {
    if !ac {
        return Verdict { done: Some(Reason::Unplugged), changed: false };
    }
    if now - state.since > MAX_DURATION_S {
        return Verdict { done: Some(Reason::DayPassed), changed: false };
    }
    let mut changed = false;
    if battery_pct >= 100.0 {
        let full_at = match state.full_at {
            Some(t) => t,
            None => {
                state.full_at = Some(now);
                changed = true;
                now
            }
        };
        if now - full_at > FULL_FOR_S {
            return Verdict { done: Some(Reason::BatteryFull), changed };
        }
    }
    Verdict { done: None, changed }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn ends_when_unplugged() {
        let mut s = State::start(10_000.0 - 60.0);
        let v = check(&mut s, false, 50.0, 10_000.0);
        assert_eq!(v, Verdict { done: Some(Reason::Unplugged), changed: false });
        assert_eq!(Reason::Unplugged.text(), "unplugged");
    }

    #[test]
    fn unplugging_wins_even_when_the_day_is_up_and_the_battery_full() {
        let mut s = State { since: 0.0, full_at: Some(1.0) };
        assert_eq!(check(&mut s, false, 100.0, 1e9).done, Some(Reason::Unplugged));
    }

    #[test]
    fn ends_after_a_day() {
        let now = 200_000.0;
        let mut s = State::start(now - 86_401.0);
        assert_eq!(check(&mut s, true, 60.0, now).done, Some(Reason::DayPassed));
        let mut s = State::start(now - 86_400.0);
        assert_eq!(check(&mut s, true, 60.0, now).done, None, "exactly 24 hours is not more than 24 hours");
        let mut s = State::start(now - 3600.0);
        assert_eq!(check(&mut s, true, 60.0, now).done, None, "an hour in is fine");
        assert_eq!(Reason::DayPassed.text(), "24 hours passed");
    }

    #[test]
    fn ends_five_minutes_after_full() {
        let now = 50_000.0;
        let mut s = State::start(now - 600.0);
        let v = check(&mut s, true, 100.0, now);
        assert_eq!(v, Verdict { done: None, changed: true }, "first time full: recorded, not finished");
        assert_eq!(s.full_at, Some(now));
        assert_eq!(
            check(&mut s, true, 100.0, now + 240.0),
            Verdict { done: None, changed: false },
            "four minutes: still going"
        );
        assert_eq!(check(&mut s, true, 100.0, now + 300.0).done, None, "exactly five minutes is not more");
        assert_eq!(check(&mut s, true, 100.0, now + 301.0).done, Some(Reason::BatteryFull));
        assert_eq!(Reason::BatteryFull.text(), "the battery is full");
    }

    #[test]
    fn a_battery_below_full_records_nothing() {
        let mut s = State::start(0.0);
        assert_eq!(check(&mut s, true, 99.9, 100.0), Verdict { done: None, changed: false });
        assert_eq!(s.full_at, None);
    }

    #[test]
    fn a_full_at_from_an_earlier_check_is_kept_even_if_the_level_dips() {
        // the level is only looked at to START the five minutes; Python never forgot full_at
        let mut s = State { since: 0.0, full_at: Some(1_000.0) };
        assert_eq!(
            check(&mut s, true, 97.0, 1_400.0),
            Verdict { done: None, changed: false },
            "below 100: nothing is evaluated"
        );
        assert_eq!(check(&mut s, true, 100.0, 1_400.0).done, Some(Reason::BatteryFull));
    }

    #[test]
    fn the_whole_story_with_a_fake_clock() {
        let mut s = State::start(1_000.0);
        let mut now = 1_000.0;
        let mut checks = 0;
        let reason = loop {
            now += 5.0 * 2.0; // every 5 ticks of 2 s
            checks += 1;
            let pct = if now < 5_000.0 { 70.0 } else { 100.0 };
            let v = check(&mut s, true, pct, now);
            if let Some(r) = v.done {
                break r;
            }
            assert!(checks < 10_000, "must end");
        };
        assert_eq!(reason, Reason::BatteryFull);
        assert!(
            (5_300.0..5_320.0).contains(&now),
            "ended at {now}: five minutes after the battery was first seen full"
        );
    }

    #[test]
    fn the_record_round_trips_through_json() {
        let s = State { since: 1234.5, full_at: Some(99.0) };
        assert_eq!(State::from_json(&s.to_json()), Some(s));
        let fresh = State::start(5.0);
        assert_eq!(fresh.to_json(), json!({"since": 5.0, "full_at": null}));
        assert_eq!(State::from_json(&fresh.to_json()), Some(fresh));
        assert_eq!(
            State::from_json(&json!({"since": 7, "full_at": 8})),
            Some(State { since: 7.0, full_at: Some(8.0) })
        );
    }

    #[test]
    fn empty_stored_values_mean_no_charge_and_damaged_ones_end_it() {
        for none in [json!(null), json!({}), json!(false), json!(0), json!(""), json!([])] {
            assert_eq!(State::from_json(&none), None, "{none}");
        }
        for odd in [
            json!({"since": "yesterday"}),
            json!({"full_at": 5}),
            json!(true),
            json!([1]),
            json!({"since": null, "full_at": "x"}),
        ] {
            let mut s = State::from_json(&odd).unwrap_or_else(|| panic!("{odd} is a record"));
            assert_eq!(
                check(&mut s, true, 50.0, 1e6).done,
                Some(Reason::DayPassed),
                "{odd}: ends, so the limit comes back"
            );
        }
        assert_eq!(
            State::from_json(&json!({"since": 1, "full_at": 0})).expect("record").full_at,
            None,
            "a zero full_at is unset, like Python"
        );
    }

    #[test]
    fn the_schedule_matches_the_daemon_loop() {
        assert_eq!(CHECK_EVERY_TICKS, 5);
        assert_eq!((MAX_DURATION_S, FULL_FOR_S), (86_400.0, 300.0));
    }
}
