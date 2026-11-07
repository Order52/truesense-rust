#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! The wave directions: the logical-to-wire mapping is pinned against the hardware-confirmed
//! values of the reference project (a PH16-71, 2026-10-15), and checked through the packets the
//! simulated keyboard receives.

use super::*;
use crate::keymap::testutil::TempDir;

fn rig() -> (Keyboard, SimHid, TempDir) {
    let dir = TempDir::new("kb-dir");
    let (keyboard, sim) = Keyboard::simulated(Arc::new(Store::new(dir.join("user.json"))));
    (keyboard, sim, dir)
}

/// The direction byte of the last effect packet (`08 02 <effect> <speed> <brightness> 01 <dir>`).
fn last_direction_byte(sim: &SimHid) -> u8 {
    sim.packets().iter().rev().find(|p| p[0] == 0x08 && p[1] == 0x02).expect("an effect packet")[6]
}

/// What the reference project's `KEYBOARD_DIRECTIONS` and `DIR_*` constants say, as the wire byte:
/// 1 left to right, 2 right to left, 3 bottom to top, 4 top to bottom, 5 clockwise,
/// 6 counter-clockwise.
const REFERENCE: [(&str, u8); 6] = [
    ("left-to-right", 1),
    ("right-to-left", 2),
    ("bottom-to-top", 3),
    ("top-to-bottom", 4),
    ("clockwise", 5),
    ("counter-clockwise", 6),
];

#[test]
fn the_table_is_the_reference_projects_hardware_confirmed_one() {
    let ours: Vec<(&str, u8)> = Direction::ALL.iter().map(|d| (d.id(), d.wire_byte())).collect();
    assert_eq!(ours, REFERENCE);
    for (n, d) in Direction::ALL.iter().enumerate() {
        assert_eq!(d.logical(), n as i64);
        assert_eq!(Direction::from_logical(n as i64), Some(*d));
    }
}

#[test]
fn logical_4_is_wire_byte_5_clockwise_and_5_is_6_counter_clockwise() {
    let (k, sim, _d) = rig();
    k.set_effect("wave", "ffffff", 5, 30, 4).unwrap();
    assert_eq!(last_direction_byte(&sim), 5);
    assert_eq!(Direction::from_logical(4), Some(Direction::Clockwise));
    k.set_effect("wave", "ffffff", 5, 30, 5).unwrap();
    assert_eq!(last_direction_byte(&sim), 6);
    assert_eq!(Direction::from_logical(5), Some(Direction::CounterClockwise));
}

#[test]
fn every_direction_reaches_the_keyboard_as_its_reference_byte_with_a_valid_checksum() {
    for (logical, (_, byte)) in REFERENCE.iter().enumerate() {
        let (k, sim, _d) = rig();
        k.set_effect("wave", "00aec7", 7, 30, logical as i64).unwrap();
        let packet = *sim.packets().iter().rev().find(|p| p[0] == 0x08 && p[1] == 0x02).unwrap();
        assert_eq!(packet[2], 3, "the wave's effect code");
        assert_eq!(packet[6], *byte);
        assert_eq!(packet[7], checksum(&packet[..7]));
    }
}

#[test]
fn only_the_wave_takes_the_six_other_effects_keep_the_old_rule() {
    let (k, sim, _d) = rig();
    assert!(takes_direction("wave"));
    for name in effect_names().into_iter().filter(|n| *n != "wave") {
        assert!(!takes_direction(name), "{name}");
    }
    for bad in [6, 7, 254, -1, 255, i64::MAX] {
        let e = k.set_effect("wave", "ffffff", 5, 30, bad).unwrap_err().to_string();
        assert!(e.starts_with("wave direction"), "{bad}: {e}");
        assert!(e.contains("0..5") && e.contains("clockwise"), "{e}");
    }
    sim.clear_log();
    // The other effects still accept the whole byte range and still send logical + 1.
    for name in ["snake", "static", "neon"] {
        k.set_effect(name, "ffffff", 5, 30, 200).unwrap();
        assert_eq!(last_direction_byte(&sim), 201, "{name}");
    }
    assert!(k.set_effect("snake", "ffffff", 5, 30, 255).is_err());
}

#[test]
fn a_refused_direction_sends_nothing_and_remembers_nothing() {
    let (k, sim, _d) = rig();
    sim.clear_log();
    assert!(k.set_effect("wave", "ffffff", 5, 30, 6).is_err());
    assert!(sim.packets().is_empty());
    assert!(k.last().is_empty());
}

#[test]
fn the_wave_keeps_its_direction_through_colour_speed_and_brightness_changes() {
    let (k, sim, _d) = rig();
    k.set_effect("wave", "ffffff", 5, 30, 5).unwrap();
    k.set_speed(9).unwrap();
    assert_eq!(last_direction_byte(&sim), 6);
    k.set_brightness(20).unwrap();
    assert_eq!(last_direction_byte(&sim), 6);
    assert_eq!(k.last().direction, Some(5));
}

#[test]
fn names_and_numbers_parse_to_the_same_directions() {
    for (n, d) in Direction::ALL.iter().enumerate() {
        assert_eq!(Direction::parse(d.id()).unwrap(), *d);
        assert_eq!(Direction::parse(&n.to_string()).unwrap(), *d);
    }
    for (text, want) in [
        ("Clockwise", Direction::Clockwise),
        ("CW", Direction::Clockwise),
        ("ccw", Direction::CounterClockwise),
        ("counterclockwise", Direction::CounterClockwise),
        ("counter_clockwise", Direction::CounterClockwise),
        ("anticlockwise", Direction::CounterClockwise),
        (" bottom to top ", Direction::BottomToTop),
        ("Top_To_Bottom", Direction::TopToBottom),
    ] {
        assert_eq!(Direction::parse(text).unwrap(), want, "{text}");
    }
}

#[test]
fn hostile_direction_texts_are_errors_not_panics() {
    for bad in ["", " ", "6", "-1", "99999999999999999999", "left", "up", "é", "0x1", "1.5", "clock wise!"] {
        let e = Direction::parse(bad).unwrap_err().to_string();
        assert!(e.contains("0..5"), "{bad:?}: {e}");
    }
}
