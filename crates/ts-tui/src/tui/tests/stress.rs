//! Random input against the whole screen: thousands of seeded random keys, clicks, drags, ticks and
//! resizes, with and without the service. The screen must never panic, never leave a row marked as
//! asking without a question on screen or as writing with nothing on its way, and never stop a fan
//! without a Yes (and never at all without the service).

use super::*;
use crate::tui::model::Modal;

/// A small deterministic generator (the crate has no `rand`): xorshift64*.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
}

fn keys() -> Vec<Key> {
    let mut ks: Vec<Key> = "jkhlqm r?ynx0123456789\t\n \u{1b}#fYN".chars().map(Key::Char).collect();
    ks.extend([
        Key::Up,
        Key::Down,
        Key::Left,
        Key::Right,
        Key::BackTab,
        Key::Home,
        Key::End,
        Key::PageDown,
        Key::PageUp,
        Key::Redraw,
        Key::Backspace,
        Key::Enter,
        Key::Tab,
        Key::Esc,
    ]);
    ks
}

/// One random event: a key, a press, a drag, a release, the wheel, a tick, a pause or a resize.
fn random_event(rnd: &mut Rng, h: &mut Harness, keys: &[Key], sizes: &[(u16, u16)]) {
    let (w, rows) = (usize::from(h.size.0), usize::from(h.size.1));
    let (x, y) = (rnd.below(w + 2) as i32 - 1, rnd.below(rows + 2) as i32 - 1);
    match rnd.below(100) {
        0..=54 => h.key(keys[rnd.below(keys.len())]),
        55..=71 => h.input(Input::Press { x, y }),
        72..=81 => h.input(Input::Drag { x: rnd.below(w + 10) as i32 - 5, y }),
        82..=86 => h.input(Input::Release),
        87..=88 => h.input(Input::Wheel([-1, 1][rnd.below(2)])),
        89..=90 => h.feed(Msg::Tick),
        91..=94 => h.elapse(),
        95..=96 => {
            let (w, hh) = sizes[rnd.below(sizes.len())];
            h.resize(w, hh);
        }
        _ => h.fast(),
    }
    // Quitting ends the real screen; here the run goes on.
    h.model.cancel_quit();
}

/// The rules that hold after every single event.
fn check_invariants(h: &Harness, step: usize, context: &str) {
    let m = &h.model;
    let asking_modal = matches!(m.modal, Some(Modal::Confirm { .. }));
    for p in &m.pages {
        for r in &p.rows {
            assert!(
                asking_modal || r.state != RowState::Asking,
                "{context} step {step}: row {:?} is asking with no question on screen",
                r.label
            );
            // Every write in this harness is done on the spot: nothing may be left "writing".
            assert!(
                r.state != RowState::Writing,
                "{context} step {step}: row {:?} is writing with nothing on its way",
                r.label
            );
        }
    }
    for &(p, i) in &m.pending {
        assert_eq!(
            m.pages[p].rows[i].state,
            RowState::Pending,
            "{context} step {step}: a pending row is not pending"
        );
    }
    assert!(!m.writes_in_flight(), "{context} step {step}: a write was left behind");
}

/// Every stop that was asked of the backend, and the daemon calls that carried one.
fn stop_ops(h: &Harness) -> Vec<Op> {
    h.backend.ops().into_iter().filter(Op::stops_a_fan).collect()
}

fn check_fan_safety(h: &Harness, service: bool, context: &str) {
    let stops = stop_ops(h);
    for op in &stops {
        let Op::Fan(req) = op else { continue };
        assert!(req.confirm, "{context}: a stop was written without confirmation: {req:?}");
    }
    assert!(
        stops.len() <= h.model.yes_answers(),
        "{context}: {} stops but only {} Yes answers",
        stops.len(),
        h.model.yes_answers()
    );
    let (duty, fan, _) = h.sim();
    if !service {
        assert!(
            duty[0] != 0 && duty[1] != 0 && fan != "Off",
            "{context}: a fan was stopped without the service: {duty:?}"
        );
    }
    for (cmd, args) in h.rig.daemon.calls() {
        if cmd != "fan" {
            continue;
        }
        let mode = args.get("mode").and_then(|v| v.as_str()).unwrap_or_default().to_owned();
        let zero = |k: &str| args.get(k).and_then(serde_json::Value::as_i64) == Some(0);
        if mode == "off" || (mode == "manual" && (zero("cpu") || zero("gpu"))) {
            assert_eq!(
                args.get("confirm"),
                Some(&serde_json::json!(true)),
                "{context}: a stop reached the daemon unconfirmed: {args:?}"
            );
        }
    }
}

#[test]
fn random_input_never_panics_leaves_no_work_behind_and_never_stops_a_fan_without_a_yes() {
    let sizes = [(100, 30), (60, 20), (44, 12), (140, 50), (80, 24), (30, 8), (200, 12), (40, 12)];
    let keys = keys();
    for service in [false, true] {
        for (seed, &(w, hh)) in sizes[..5].iter().enumerate() {
            let context = format!("{} seed {seed}", if service { "service" } else { "no service" });
            let mut h = if service { Harness::with_service(w, hh) } else { Harness::new(w, hh) };
            let mut rnd = Rng::new(seed as u64 * 2 + 1);
            for step in 0..1000 {
                random_event(&mut rnd, &mut h, &keys, &sizes);
                if step % 10 == 0 {
                    let text = h.text();
                    assert!(
                        widest(&text) <= usize::from(h.size.0),
                        "{context} step {step}: a line wider than the screen"
                    );
                }
                check_invariants(&h, step, &context);
            }
            h.model.modal = None;
            h.flush();
            h.elapse();
            h.draw();
            assert!(
                h.model.pending.is_empty(),
                "{context}: work left behind: {} pending",
                h.model.pending.len()
            );
            check_fan_safety(&h, service, &context);
        }
    }
}

#[test]
fn fan_stops_happen_only_through_a_confirmed_question() {
    // The keys that lead to a stop: down to the bottom of a slider, Enter, y, n, Esc, arrows in the question.
    let alphabet = [
        Key::Char('j'),
        Key::Char('k'),
        Key::Char('h'),
        Key::Char('h'),
        Key::Char('h'),
        Key::Char('l'),
        Key::Enter,
        Key::Enter,
        Key::Char(' '),
        Key::Char('y'),
        Key::Char('n'),
        Key::Esc,
        Key::Left,
        Key::Right,
        Key::Tab,
        Key::Char('2'),
    ];
    let mut reached = 0;
    for seed in 0..6u64 {
        let mut rnd = Rng::new(seed + 100);
        let mut h = Harness::with_service(100, 30);
        h.goto(1);
        for step in 0..3000 {
            match rnd.below(10) {
                0 => h.elapse(),
                1 => h.goto(1),
                _ => h.key(alphabet[rnd.below(alphabet.len())]),
            }
            h.model.cancel_quit();
            check_invariants(&h, step, "fan alphabet");
        }
        h.model.modal = None;
        h.flush();
        reached += stop_ops(&h).len();
        check_fan_safety(&h, true, &format!("fan alphabet seed {seed}"));
    }
    assert!(reached > 0, "the test never reached a stop: it proves nothing");
}

#[test]
fn the_generator_is_deterministic_and_covers_its_range() {
    let (mut a, mut b) = (Rng::new(7), Rng::new(7));
    let first: Vec<usize> = (0..20).map(|_| a.below(10)).collect();
    assert_eq!(first, (0..20).map(|_| b.below(10)).collect::<Vec<_>>());
    assert!(first.contains(&0) || first.iter().any(|&n| n > 5));
    assert!((0..1000).any(|_| a.chance(1)) && (0..1000).any(|_| !a.chance(99)));
}
