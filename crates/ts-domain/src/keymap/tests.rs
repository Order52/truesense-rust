#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use serde_json::Value;

use super::testutil::hex_list;
use super::*;

/// What the Python edition produced (testdata/gen_golden_a.py).
fn golden() -> Value {
    serde_json::from_str(include_str!("../../testdata/keymap_golden.json")).expect("golden json")
}

fn cells_of(v: &Value) -> Vec<usize> {
    v.as_array().expect("array").iter().map(|c| c.as_u64().expect("cell") as usize).collect()
}

fn colors_of(v: &Value) -> Vec<Color> {
    v.as_array()
        .expect("array")
        .iter()
        .map(|c| {
            let c = c.as_array().expect("colour");
            [c[0].as_u64().unwrap() as u8, c[1].as_u64().unwrap() as u8, c[2].as_u64().unwrap() as u8]
        })
        .collect()
}

#[test]
fn names_and_groups_match_python_including_the_order() {
    let g = golden();
    let map = KeyMap::verified();
    let want = g["names"].as_array().unwrap();
    assert_eq!(map.names().len(), want.len());
    for (got, want) in map.names().iter().zip(want) {
        assert_eq!(got.0, want[0].as_str().unwrap());
        assert_eq!(got.1, cells_of(&want[1]), "cells of {}", got.0);
    }
    let groups = g["groups"].as_object().unwrap();
    assert_eq!(map.groups().len(), groups.len());
    for (name, cells) in map.groups() {
        assert_eq!(*cells, cells_of(&groups[*name]), "group {name}");
    }
}

#[test]
fn the_verified_corrections_hold() {
    let m = KeyMap::verified();
    assert_eq!(m.key("esc"), Some(&[2][..]));
    assert_eq!(m.key("caps"), Some(&[5][..]));
    assert_eq!(m.key("minus"), Some(&[70][..]));
    assert_eq!(m.key("ESC"), Some(&[2][..]), "names are matched in lower case");
    assert_eq!(m.key("enter"), m.key("return"), "the friendly name shares the cells");
    assert_eq!(m.key("win"), m.key("windows"));
    assert_eq!(m.key("nosuch"), None);
    assert!(m.groups().iter().any(|(g, c)| *g == "all" && c.len() == LIT_CELLS));
}

#[test]
fn cells_for_matches_python() {
    let g = golden();
    let m = KeyMap::verified();
    for (probe, want) in g["cells_for"].as_object().unwrap() {
        let got = m.cells_for(probe);
        match want {
            Value::String(msg) => {
                assert_eq!(format!("ERR: {}", got.expect_err(probe)), *msg, "probe {probe:?}");
            }
            cells => assert_eq!(got.expect(probe), cells_of(cells), "probe {probe:?}"),
        }
    }
}

#[test]
fn hexcolor_matches_python() {
    for (input, want) in golden()["hexcolor"].as_object().unwrap() {
        match want {
            Value::String(msg) => {
                assert_eq!(format!("ERR: {}", parse_hex(input).expect_err(input)), *msg, "{input:?}")
            }
            c => assert_eq!(
                parse_hex(input).expect(input).to_vec(),
                colors_of(&Value::Array(vec![c.clone()]))[0].to_vec()
            ),
        }
    }
}

#[test]
fn hostile_colours_are_errors_not_panics() {
    for bad in
        ["", "#", "12345", "1234567", "gg0000", "é00000", "ff00 0", "ff0000\n", "\u{0}", "ＦＦ００００"]
    {
        assert!(parse_hex(bad).is_err(), "{bad:?}");
    }
    assert_eq!(parse_hex("###AbCdEf").unwrap(), [0xab, 0xcd, 0xef]);
    assert_eq!(hex([1, 2, 255]), "0102ff");
}

#[test]
fn hsv_matches_python() {
    let g = golden();
    for pair in g["hsv"].as_array().unwrap() {
        let h: f64 = pair[0].as_str().unwrap().parse().unwrap();
        assert_eq!(hsv(h).to_vec(), colors_of(&Value::Array(vec![pair[1].clone()]))[0].to_vec(), "h = {h}");
    }
    assert_eq!(hsv(f64::NAN), hsv(0.0));
    assert_eq!(hsv(f64::INFINITY), hsv(0.0));
}

#[test]
fn lerp_rounds_halves_to_even_like_python() {
    for row in golden()["lerp"].as_array().unwrap() {
        let a = colors_of(&Value::Array(vec![row[0].clone()]))[0];
        let b = colors_of(&Value::Array(vec![row[1].clone()]))[0];
        let want = colors_of(&Value::Array(vec![row[3].clone()]))[0];
        assert_eq!(lerp(a, b, row[2].as_str().unwrap().parse().unwrap()), want, "{row}");
    }
}

#[test]
fn pictures_match_python() {
    let g = golden();
    assert_eq!(gradient([255, 51, 0], [102, 0, 255]), colors_of(&g["gradient"]));
    assert_eq!(gradient([1, 2, 3], [250, 251, 252]), colors_of(&g["gradient2"]));
    assert_eq!(rainbow(), colors_of(&g["rainbow"]));
    assert_eq!(canvas([7, 8, 9]), colors_of(&g["canvas"]));
    let c = canvas([7, 8, 9]);
    assert_eq!((c.len(), c[125], c[126], c[127]), (128, [7, 8, 9], [0; 3], [0; 3]));
    assert_eq!(hex_list(&c[..1]), ["070809"]);
}

#[test]
fn the_embedded_capture_parses_and_every_cell_is_in_range() {
    let m = KeyMap::parse(VERIFIED_JSON).expect("parse");
    assert!(m.names().len() > 80);
    assert!(m.names().iter().flat_map(|(_, c)| c).all(|c| *c < NUM_CELLS));
    assert!(!m.groups().iter().find(|(g, _)| *g == "numpad").unwrap().1.is_empty());
}

#[test]
fn a_damaged_capture_is_an_error_not_a_panic() {
    for bad in [
        "",
        "null",
        "[]",
        "{}",
        r#"{"cells": []}"#,
        r#"{"cells": {"x": {"label": "a"}}}"#,
        r#"{"cells": {"128": {"label": "a"}}}"#,
        r#"{"cells": {"-1": {"label": "a"}}}"#,
        r#"{"cells": {"1": 5}}"#,
        r#"{"cells": {"1": {"label": 5}}}"#,
    ] {
        assert!(matches!(KeyMap::parse(bad), Err(KeymapError::Corrupt(_))), "{bad}");
    }
    // A capture without labels is a valid, empty table (and its groups are empty, not missing).
    let m = KeyMap::parse(r#"{"cells": {"3": {"label": null}, "4": {}}}"#).expect("empty ok");
    assert!(m.names().is_empty());
    assert_eq!(m.cells_for("wasd").unwrap(), Vec::<usize>::new());
    assert_eq!(m.cells_for("cell3").unwrap(), vec![3]);
}

#[test]
fn labels_are_normalised_and_cell_order_is_numeric() {
    let m = KeyMap::parse(
        r#"{"cells": {"10": {"label": "Retur"}, "9": {"label": "Retur"}, "2": {"label": "CapsLock"}, "3": {"label": "Num5"}, "1": {"label": "NumLk"}, "20": {"label": "Windows"}}}"#,
    )
    .expect("parse");
    assert_eq!(m.key("enter"), Some(&[9, 10][..]), "ascending numeric, not string order");
    assert_eq!(m.key("esc"), Some(&[2][..]), "cell 2 is overridden");
    assert_eq!(m.cells_for("numpad").unwrap(), vec![1, 3], "numlock first (cell 1), then num5");
    assert_eq!(m.cells_for("win").unwrap(), vec![20]);
}

#[test]
fn cell_names_are_range_checked_and_ascii_only() {
    let m = KeyMap::verified();
    assert_eq!(m.cells_for("Cell127").unwrap(), vec![127]);
    for bad in ["cell128", "cell", "cell-1", "cell٣", "cell1x", "cell99999999999999999999999999", " cell1"] {
        assert!(matches!(m.cells_for(bad), Err(KeymapError::UnknownKey(_))), "{bad}");
    }
    assert_eq!(m.cells_for("cell0000003").unwrap(), vec![3]);
}

// ---- geometry ----------------------------------------------------------------------------------

#[test]
fn every_key_of_the_map_has_one_rectangle_and_none_overlap() {
    use super::geometry::{HEIGHT, WIDTH, key_rects};
    let rects = key_rects();
    let names: std::collections::BTreeSet<&str> = rects.iter().map(|r| r.name).collect();
    assert_eq!(names.len(), rects.len(), "a name is drawn once");
    for r in &rects {
        assert!(
            r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= WIDTH + 1e-9 && r.y + r.h <= HEIGHT + 1e-9,
            "{}",
            r.name
        );
    }
    // Every key the map knows, but the aliases of another and the Predator key's twin, has a rectangle.
    let map = KeyMap::verified();
    let missing: Vec<&str> = map
        .names()
        .iter()
        .map(|(n, _)| n.as_str())
        .filter(|n| !names.contains(n) && !matches!(*n, "enter" | "win"))
        .collect();
    assert!(missing.is_empty(), "keys without a rectangle: {missing:?}");
    for (i, a) in rects.iter().enumerate() {
        for b in &rects[i + 1..] {
            let apart = a.x + a.w <= b.x + 1e-9
                || b.x + b.w <= a.x + 1e-9
                || a.y + a.h <= b.y + 1e-9
                || b.y + b.h <= a.y + 1e-9;
            assert!(apart, "{} and {} overlap", a.name, b.name);
        }
    }
    let space = rects.iter().find(|r| r.name == "space").unwrap();
    assert_eq!((space.w, space.centre().0), (4.75, space.x + 2.375));
}
