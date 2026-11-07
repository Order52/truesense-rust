#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! The rotating sweeps: geometry from the key map, the direction of the turn, colours with and
//! without a base, flash safety and fixed golden frames.

use super::*;
use crate::keymap::{self, KeyMap, LIT_CELLS};

fn cyan() -> Vec<Color> {
    keymap::canvas([0x00, 0xAE, 0xC7])
}

fn black() -> Vec<Color> {
    vec![[0; 3]; 128]
}

fn key_angle(name: &str) -> f64 {
    let cells = KeyMap::verified().key(name).unwrap_or_else(|| panic!("no key {name}"));
    cell_angle(cells[0])
}

fn circular_distance(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(1.0);
    d.min(1.0 - d)
}

fn luminance(c: Color) -> f64 {
    (0.2126 * f64::from(c[0]) + 0.7152 * f64::from(c[1]) + 0.0722 * f64::from(c[2])) / 255.0
}

/// The lit cell with the brightest colour in a frame.
fn brightest(frame: &[Color]) -> usize {
    (0..LIT_CELLS).max_by(|a, b| luminance(frame[*a]).total_cmp(&luminance(frame[*b]))).unwrap()
}

#[test]
fn the_sweeps_are_listed_after_the_earlier_editions_eighteen() {
    let names = mode_names();
    assert_eq!(names.len(), 20);
    assert_eq!(&names[18..], ["wave-clockwise", "wave-counter-clockwise"]);
    assert!(is_mode("wave-clockwise") && is_mode("wave-counter-clockwise"));
    assert_eq!(MODES[18].1, "Clockwise sweep");
    assert_eq!(MODES[19].1, "Counter-clockwise sweep");
}

#[test]
fn every_cell_has_an_angle_around_the_middle_and_the_quarters_are_where_they_should_be() {
    for cell in 0..128 {
        let a = cell_angle(cell);
        assert!((0.0..1.0).contains(&a), "cell {cell}: {a}");
    }
    assert_eq!(cell_angle(5000), 0.0);
    // 0 is right, a quarter is down, a half is left, three quarters is up.
    let quarter = |name: &str, low: f64, high: f64| {
        let a = key_angle(name);
        assert!((low..high).contains(&a), "{name} is at {a}, expected between {low} and {high}");
    };
    quarter("esc", 0.5, 0.75); // top left
    quarter("lshift", 0.25, 0.5); // bottom left
    quarter("num9", 0.75, 1.0); // top right
    quarter("num0", 0.0, 0.25); // bottom right
    quarter("tab", 0.5, 0.75); // left, above the middle
    quarter("f12", 0.75, 1.0); // top, right of the middle
    quarter("right", 0.0, 0.25); // bottom right
    quarter("space", 0.25, 0.5); // bottom, left of the middle
    quarter("ctrl", 0.25, 0.5);
}

#[test]
fn the_phase_moves_with_the_turn_in_the_named_direction() {
    // A feature is where phase == 0: at angle == turn clockwise, at angle == -turn counter-clockwise.
    assert!(sweep_phase(0.3, 0.3, true).abs() < 1e-12);
    assert!(sweep_phase(-0.3, 0.3, false).abs() < 1e-12);
    assert!(sweep_phase(0.0, 0.0, true).abs() < 1e-12 && sweep_phase(0.0, 0.0, false).abs() < 1e-12);
}

#[test]
fn the_bright_band_travels_clockwise_or_counter_clockwise_as_named() {
    // Speed 10 is 0.5 revolutions a second: a quarter turn takes half a second.
    let base = cyan();
    for (mode, quarter_turns, start) in [("wave-clockwise", 1.0, 0.0), ("wave-counter-clockwise", -1.0, 0.0)]
    {
        let mut seen = Vec::new();
        for step in 0..=3 {
            let t = 0.5 * f64::from(step);
            let frame = render(mode, t, &base, &[], 10);
            seen.push(cell_angle(brightest(&frame)));
        }
        for (step, angle) in seen.iter().enumerate() {
            let want = (start + 0.25 * quarter_turns * step as f64).rem_euclid(1.0);
            assert!(
                circular_distance(*angle, want) < 0.07,
                "{mode} step {step}: the band is at {angle}, expected near {want} (all {seen:?})"
            );
        }
    }
}

#[test]
fn a_coloured_key_keeps_its_hue_and_never_exceeds_its_own_brightness() {
    let sunset: Vec<Color> = (0..128).map(|c| [255, (c * 2) as u8, 40]).collect();
    for mode in ["wave-clockwise", "wave-counter-clockwise"] {
        for t in [0.0, 0.7, 3.1, 100.0] {
            let frame = render(mode, t, &sunset, &[], 6);
            for c in 0..LIT_CELLS {
                for ch in 0..3 {
                    assert!(frame[c][ch] <= sunset[c][ch], "{mode} t={t} cell {c}");
                    assert!(
                        f64::from(frame[c][ch]) >= f64::from(sunset[c][ch]) * 0.29 - 1.0,
                        "{mode} t={t} cell {c}: never darker than 30 %"
                    );
                }
            }
        }
    }
}

#[test]
fn a_dark_base_gets_the_rainbow_all_around() {
    for mode in ["wave-clockwise", "wave-counter-clockwise"] {
        let frame = render(mode, 2.0, &black(), &[], 5);
        let colours: std::collections::BTreeSet<Color> = (0..LIT_CELLS).map(|c| frame[c]).collect();
        assert!(colours.len() > 20, "{mode}: {} colours", colours.len());
        assert!(
            frame[..LIT_CELLS].iter().all(|c| c.iter().copied().max() == Some(255)),
            "full value everywhere"
        );
        assert_eq!(frame[126], frame[127].min(frame[126]), "cells 126 and 127 are not keys");
    }
}

#[test]
fn the_two_directions_are_different_pictures_that_mirror_each_other() {
    let base = black();
    let (cw, ccw) =
        (render("wave-clockwise", 3.0, &base, &[], 5), render("wave-counter-clockwise", 3.0, &base, &[], 5));
    assert_ne!(cw, ccw);
    // At time zero a key at angle a is hsv(a) clockwise and hsv(-a) counter-clockwise.
    let esc = KeyMap::verified().key("esc").unwrap()[0];
    assert_eq!(render("wave-clockwise", 0.0, &base, &[], 5)[esc], keymap::hsv(cell_angle(esc)));
    assert_eq!(render("wave-counter-clockwise", 0.0, &base, &[], 5)[esc], keymap::hsv(-cell_angle(esc)));
}

#[test]
fn only_the_chosen_keys_sweep_and_the_rest_keep_their_colour() {
    let map = KeyMap::verified();
    let (w, esc) = (map.key("w").unwrap()[0], map.key("esc").unwrap()[0]);
    let frame = render("wave-clockwise", 4.0, &black(), &[w], 5);
    assert_ne!(frame[w], [0; 3]);
    assert_eq!(frame[esc], [0; 3]);
}

#[test]
fn the_sweep_is_deterministic_and_total() {
    let base = cyan();
    for mode in ["wave-clockwise", "wave-counter-clockwise"] {
        assert_eq!(render(mode, 12.5, &base, &[], 3), render(mode, 12.5, &base, &[], 3));
        for t in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1e300, 1e300, f64::MIN_POSITIVE] {
            assert_eq!(render(mode, t, &base, &[], 5).len(), 128, "{mode} at {t}");
        }
        for speed in [i64::MIN, 0, 1, 10, 11, i64::MAX] {
            assert_eq!(render(mode, 1.0, &base, &[], speed).len(), 128);
        }
    }
}

/// How many times the light of one key reverses direction by at least 8 % of the range inside one
/// second, the way WCAG 2.3.1 counts a flash (a pair of opposite changes is one flash).
fn worst_reversals_per_second(frames: &[Vec<Color>], cell: usize, fps: usize) -> usize {
    let lum: Vec<f64> = frames.iter().map(|f| luminance(f[cell])).collect();
    // Extremes that differ from the previous extreme by 8 % or more.
    let mut extremes: Vec<(usize, f64)> = vec![(0, lum[0])];
    let mut rising = None;
    for i in 1..lum.len() {
        let d = lum[i] - lum[i - 1];
        if d.abs() < 1e-9 {
            continue;
        }
        let now_rising = d > 0.0;
        if let Some(was) = rising
            && was != now_rising
            && (lum[i - 1] - extremes.last().unwrap().1).abs() >= 0.08
        {
            extremes.push((i - 1, lum[i - 1]));
        }
        rising = Some(now_rising);
    }
    (0..extremes.len())
        .map(|i| extremes[i..].iter().take_while(|(at, _)| *at < extremes[i].0 + fps).count())
        .max()
        .unwrap_or(0)
}

#[test]
fn at_the_fastest_speed_no_key_flashes_more_than_three_times_a_second() {
    // The key service draws five frames a second; sample twenty seconds at that rate.
    for base in [cyan(), black()] {
        for mode in ["wave-clockwise", "wave-counter-clockwise"] {
            let frames: Vec<Vec<Color>> =
                (0..100).map(|i| render(mode, f64::from(i) * 0.2, &base, &[], 10)).collect();
            let worst = (0..LIT_CELLS).map(|c| worst_reversals_per_second(&frames, c, 5)).max().unwrap();
            assert!(worst <= 3, "{mode}: {worst} reversals inside one second");
        }
    }
}

#[test]
fn a_brightness_of_the_keyboard_is_the_keyboards_business_so_frames_stay_full_scale() {
    // The runner passes the keyboard's own level with the frame; the colours are never pre-dimmed.
    let frame = render("wave-clockwise", 0.0, &black(), &[], 5);
    assert!(frame[..LIT_CELLS].iter().any(|c| c.iter().copied().max() == Some(255)));
}

/// A frame as one hex string and a short fingerprint, for the golden checks below.
fn fingerprint(frame: &[Color]) -> (String, u64) {
    let text: String = frame.iter().map(|c| keymap::hex(*c)).collect();
    let sum = text
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    (text, sum)
}

#[test]
fn golden_frames_at_a_fixed_time() {
    // Half a second at speed 10 is a quarter turn. Fixed once, checked against the properties above.
    let golden: [(&str, bool, [&str; 6], u64); 4] = [
        (
            "wave-clockwise",
            true,
            ["008093", "005d6a", "005b68", "00434d", "007f91", "006877"],
            0x84e0_a8d2_51ac_e802,
        ),
        (
            "wave-counter-clockwise",
            true,
            ["00616f", "008497", "008699", "009eb5", "006371", "00798b"],
            0xa440_5fd6_db6f_2164,
        ),
        (
            "wave-clockwise",
            false,
            ["c0ff00", "2fff00", "28ff00", "004eff", "b800ff", "5d00ff"],
            0xe87b_faae_8baf_8965,
        ),
        (
            "wave-counter-clockwise",
            false,
            ["3fff00", "d0ff00", "d7ff00", "ff00b1", "4700ff", "a200ff"],
            0xbab3_08f1_59bf_a418,
        ),
    ];
    for (mode, on_cyan, cells, sum) in golden {
        let frame = render(mode, 0.5, &if on_cyan { cyan() } else { black() }, &[], 10);
        let got: Vec<String> =
            [0usize, 17, 40, 70, 100, 125].iter().map(|c| keymap::hex(frame[*c])).collect();
        assert_eq!(got, cells, "{mode} cyan={on_cyan}");
        assert_eq!(fingerprint(&frame).1, sum, "{mode} cyan={on_cyan}: the whole frame");
    }
}
