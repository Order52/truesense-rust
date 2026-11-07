#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use serde_json::Value;

use super::*;
use crate::keymap::testutil::{TempDir, hex_list};

fn golden() -> Value {
    serde_json::from_str(include_str!("../../testdata/designs_golden.json")).expect("golden json")
}

fn store() -> (TempDir, DesignStore) {
    let dir = TempDir::new("designs");
    let store = DesignStore::new(dir.join("designs"));
    (dir, store)
}

fn write(dir: &TempDir, name: &str, text: &str) {
    std::fs::create_dir_all(dir.join("designs")).unwrap();
    std::fs::write(dir.join(format!("designs/{name}.json")), text).unwrap();
}

/// The files the Python run created (testdata/gen_golden_a.py). They are written as text because
/// the order of the entries matters and a `serde_json::Value` would sort them.
fn write_the_python_files(dir: &TempDir) {
    let info = "x".repeat(100);
    write(
        dir,
        "overlap",
        &format!(
            r##"{{"info": "{info}", "base": "#112233", "keys": {{"arrows": "ff0000", "up": "00ff00", "wasd": "0000ff", "w": "ffffff"}}, "cells": {{"3": "010203", "03": "040506", "125": "070809"}}}}"##
        ),
    );
    write(dir, "minimal", "{}");
    write(dir, "infonum", r#"{"info": 5, "base": "000001"}"#);
    write(dir, "bad1", r#"{"cells": {"128": "ffffff"}}"#);
    write(dir, "bad2", r#"{"cells": {"x": "ffffff"}}"#);
    write(dir, "bad3", r#"{"base": "12"}"#);
    write(dir, "bad4", r#"{"keys": {"w": "zz"}}"#);
}

/// The six of the earlier editions and the eight templates.
const BUILTIN_COUNT: usize = 14;

fn value_of(d: &Design) -> Value {
    serde_json::to_value(d).unwrap()
}

#[test]
fn the_builtin_designs_match_python_exactly() {
    let g = golden();
    let (_dir, store) = store();
    let all = store.load_all();
    let want_names = ["gaming", "coding", "arrows", "ice", "ember", "stealth"];
    // The earlier editions' six come first and unchanged; the templates follow them.
    assert_eq!(all.iter().take(EARLIER_BUILTIN).map(|(n, _)| n.as_str()).collect::<Vec<_>>(), want_names);
    for (name, design) in all.iter().take(EARLIER_BUILTIN) {
        let want = &g[name.as_str()];
        // Python's dicts keep the order of the keys; serde_json values compare by content, so
        // the order of the painted keys is checked through the cells below.
        assert_eq!(value_of(design), want["design"], "{name}");
        assert_eq!(
            hex_list(&design.cells_of(store.key_map()).unwrap()),
            serde_json::from_value::<Vec<String>>(want["cells"].clone()).unwrap(),
            "{name}"
        );
        assert_eq!(design.cells_of(store.key_map()).unwrap().len(), 128);
    }
}

#[test]
fn user_files_win_broken_ones_are_skipped_and_everything_matches_python() {
    let g = golden();
    let (dir, store) = store();
    write_the_python_files(&dir);
    let all = store.load_all();
    let names: Vec<&str> = all.iter().map(|(n, _)| n.as_str()).collect();
    let mut yours: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| ["overlap", "minimal", "infonum", "bad1", "bad2", "bad3", "bad4"].contains(n))
        .collect();
    yours.sort_unstable();
    let want: Vec<&str> = g["user:_names"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(yours, want, "the invalid files are skipped");
    for name in want {
        let design = store.get(name).unwrap();
        let python = &g[format!("user:{name}")];
        assert_eq!(value_of(&design), python["design"], "{name}");
        assert_eq!(
            hex_list(&design.cells_of(store.key_map()).unwrap()),
            serde_json::from_value::<Vec<String>>(python["cells"].clone()).unwrap(),
            "{name}"
        );
    }
    assert_eq!(store.get("overlap").unwrap().info.chars().count(), 80, "info is cut to 80 characters");
    assert_eq!(store.get("infonum").unwrap().info, "5", "info is str() of whatever the file had");
    assert_eq!(store.get("overlap").unwrap().base, "#112233", "the base is kept as written");
}

#[test]
fn a_user_file_with_the_name_of_a_builtin_replaces_it_in_place() {
    let (dir, store) = store();
    write(&dir, "gaming", r#"{"base": "ffffff"}"#);
    write(&dir, "zzz", r#"{"base": "000000"}"#);
    write(&dir, "aaa", r#"{"base": "000000"}"#);
    let all = store.load_all();
    assert_eq!(all[0].0, "gaming");
    assert_eq!(all[0].1.source, Source::Yours);
    assert_eq!(all[0].1.source.name(), "yours");
    assert_eq!(all[1].1.source, Source::BuiltIn);
    assert_eq!(
        all.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>()[BUILTIN_COUNT..],
        ["aaa", "zzz"],
        "then yours in file-name order"
    );
}

#[test]
fn the_order_of_keys_decides_overlaps() {
    let (dir, store) = store();
    let map = store.key_map();
    let up = map.key("up").unwrap()[0];
    let later_wins = |text: &str| -> Color {
        write(&dir, "o", text);
        store.get("o").unwrap().cells_of(map).unwrap()[up]
    };
    assert_eq!(later_wins(r#"{"keys": {"arrows": "ff0000", "up": "00ff00"}}"#), [0, 255, 0]);
    assert_eq!(later_wins(r#"{"keys": {"up": "00ff00", "arrows": "ff0000"}}"#), [255, 0, 0]);
    assert_eq!(
        later_wins(r#"{"keys": {"up": "00ff00", "arrows": "ff0000", "up": "0000ff"}}"#),
        [255, 0, 0],
        "a repeated name keeps its first place (with the last colour), so 'arrows' still paints over it"
    );
    assert_eq!(
        store.get("o").unwrap().keys.0,
        vec![("up".to_owned(), "0000ff".to_owned()), ("arrows".to_owned(), "ff0000".to_owned())]
    );
}

#[test]
fn parse_cleans_like_python() {
    let map = KeyMap::verified();
    let ok = |t: &str| Design::parse(map, t, Source::Yours).unwrap();
    assert_eq!((ok("{}").base.as_str(), ok("{}").info.as_str()), ("000000", ""));
    assert_eq!(ok(r#"{"keys": null, "cells": null}"#).keys, Colors::default());
    assert_eq!(ok(r#"{"info": null}"#).info, "None");
    assert_eq!(ok(r#"{"info": true}"#).info, "True");
    assert_eq!(
        ok(r#"{"unknown": 1, "source": "x"}"#).source,
        Source::Yours,
        "unknown fields and a source in the file are ignored"
    );
    assert_eq!(ok(r#"{"cells": {"0": "ffffff", "127": "000000", "000": "010101"}}"#).cells.0.len(), 3);
    for bad in [
        "5",
        "[]",
        "null",
        "\"x\"",
        "",
        "{",
        r#"{"base": 5}"#,
        r#"{"base": "12"}"#,
        r#"{"base": null}"#,
        r#"{"keys": []}"#,
        r#"{"keys": {"w": 5}}"#,
        r#"{"keys": {"nokey": "ffffff"}}"#,
        r#"{"keys": {"w": "gg0000"}}"#,
        r#"{"cells": {"128": "ffffff"}}"#,
        r#"{"cells": {"-1": "ffffff"}}"#,
        r#"{"cells": {"1.5": "ffffff"}}"#,
        r#"{"cells": {"": "ffffff"}}"#,
        r#"{"cells": {"٣": "ffffff"}}"#,
        r#"{"cells": {"99999999999999999999": "ffffff"}}"#,
        r#"{"cells": {"1": "ffffff "}}"#,
    ] {
        assert!(Design::parse(map, bad, Source::Yours).is_err(), "{bad}");
    }
    assert_eq!(
        Design::parse(map, "[1]", Source::Yours).unwrap_err().to_string(),
        "a design is a JSON object"
    );
    assert_eq!(
        Design::parse(map, r#"{"cells": {"128": "ffffff"}}"#, Source::Yours).unwrap_err().to_string(),
        "cell '128': 0..127"
    );
    assert_eq!(
        Design::parse(map, r#"{"keys": {"nokey": "ffffff"}}"#, Source::Yours).unwrap_err().to_string(),
        "unknown key 'nokey' (see: truesense keyboard list-keys)"
    );
}

#[test]
fn design_json_round_trips() {
    let map = KeyMap::verified();
    let design = Design::parse(
        map,
        r#"{"info": "mine", "base": "112233", "keys": {"w": "ff0000"}, "cells": {"5": "00ff00"}}"#,
        Source::Yours,
    )
    .unwrap();
    let text = serde_json::to_string(&design).unwrap();
    assert!(text.contains(r#""keys":{"w":"ff0000"}"#) && text.contains(r#""source":"yours""#), "{text}");
    assert_eq!(Design::parse(map, &text, Source::Yours).unwrap(), design);
}

#[test]
fn save_writes_what_python_writes_and_checks_the_name() {
    let g = golden();
    let (dir, store) = store();
    let full: Vec<Color> = (0..128u32).map(|i| [i as u8, 2, 3]).collect();
    let path = store.save("mine", &full).unwrap();
    assert_eq!(path, dir.join("designs/mine.json"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), g["saved:mine"].as_str().unwrap());
    store.save("short", &[[9, 9, 9]; 3]).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("designs/short.json")).unwrap(),
        g["saved:short"].as_str().unwrap()
    );
    for (bad, message) in g["save_errors"].as_object().unwrap() {
        assert_eq!(store.save(bad, &[]).unwrap_err().to_string(), message.as_str().unwrap(), "{bad:?}");
    }
    assert_eq!(store.get("nosuch").unwrap_err().to_string(), g["get_error"].as_str().unwrap());
    // A saved design is a design again: its picture comes back.
    let again = store.get("mine").unwrap().cells_of(store.key_map()).unwrap();
    assert_eq!(&again[..126], &full[..126]);
    assert_eq!(again[126..], [[0; 3]; 2]);
    assert_eq!(store.get("mine").unwrap().source, Source::Yours);
}

#[test]
fn saved_design_of_python_test_core() {
    // test_core.py: designs.save("mine", [(1, 2, 3)] * 128) and cells_of(get("mine"))[7] == (1, 2, 3).
    let (_dir, store) = store();
    store.save("mine", &[[1, 2, 3]; 128]).unwrap();
    assert_eq!(store.get("mine").unwrap().cells_of(store.key_map()).unwrap()[7], [1, 2, 3]);
    assert!(store.save("../x", &[]).is_err());
}

#[test]
fn the_user_folder_follows_xdg() {
    let home = Path::new("/home/me");
    assert_eq!(user_dir(None, home), PathBuf::from("/home/me/.config/truesense/designs"));
    assert_eq!(user_dir(Some(""), home), PathBuf::from("/home/me/.config/truesense/designs"));
    assert_eq!(user_dir(Some("/x/cfg"), home), PathBuf::from("/x/cfg/truesense/designs"));
}

#[test]
fn hostile_files_never_break_listing() {
    let (dir, store) = store();
    write(&dir, "binary", "\u{0}");
    std::fs::write(dir.join("designs/invalid-utf8.json"), [0xff, 0xfe]).unwrap();
    std::fs::write(dir.join("designs/big.json"), vec![b' '; 2 << 20]).unwrap();
    write(&dir, "deep", &"[".repeat(100_000));
    write(&dir, "empty", "");
    std::fs::create_dir(dir.join("designs/dir.json")).unwrap();
    std::fs::write(dir.join("designs/readme.txt"), "{}").unwrap();
    assert_eq!(store.load_all().len(), BUILTIN_COUNT, "only the built-in designs");
    assert!(DesignStore::new(dir.join("missing")).load_all().len() == BUILTIN_COUNT);
}

#[test]
fn the_errors_say_what_failed() {
    assert_eq!(DesignError::Image("no such file".into()).to_string(), "cannot read the image: no such file");
    let e: DesignError = KeymapError::BadColour("zz".into()).into();
    assert_eq!(e.to_string(), "bad colour 'zz', use RRGGBB");
    assert!(std::error::Error::source(&e).is_some());
    assert!(
        DesignError::Parse(serde_json::from_str::<Value>("{").unwrap_err())
            .to_string()
            .starts_with("parse design: ")
    );
    assert!(std::error::Error::source(&DesignError::Invalid("x".into())).is_none());
}

// ---- pictures --------------------------------------------------------------------------------

/// The pixel formula of testdata/gen_golden_a.py.
fn pixel(x: usize, y: usize, c: usize) -> u8 {
    ((x * 37 + y * 91 + c * 53 + (x * y) % 17 * 11) % 256) as u8
}

fn raster(w: usize, h: usize) -> Raster {
    let mut rgb = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            for c in 0..3 {
                rgb.push(pixel(x, y, c));
            }
        }
    }
    Raster { width: w, height: h, rgb }
}

#[test]
fn pictures_scale_exactly_like_pillows_lanczos() {
    let g: Value = serde_json::from_str(include_str!("../../testdata/image_golden.json")).unwrap();
    let sizes = g.as_object().unwrap();
    assert!(sizes.len() >= 10);
    for (name, case) in sizes {
        let (w, h) = (case["w"].as_u64().unwrap() as usize, case["h"].as_u64().unwrap() as usize);
        let got = hex_list(&image_cells_from_raster(&raster(w, h)).unwrap());
        let want: Vec<String> = serde_json::from_value(case["cells"].clone()).unwrap();
        let diff: Vec<usize> = (0..128).filter(|i| got[*i] != want[*i]).collect();
        assert!(
            diff.is_empty(),
            "{name}: cells {diff:?} differ, first got {} want {}",
            got[diff[0]],
            want[diff[0]]
        );
    }
}

#[test]
fn a_solid_picture_stays_solid_and_the_last_two_cells_are_black() {
    let solid = Raster { width: 40, height: 12, rgb: [200, 10, 20].repeat(40 * 12) };
    let cells = image_cells_from_raster(&solid).unwrap();
    assert_eq!((cells.len(), cells[5], cells[125]), (128, [200, 10, 20], [200, 10, 20]));
    assert_eq!((cells[126], cells[127]), ([0; 3], [0; 3]));
}

#[test]
fn the_picture_column_c_row_r_lands_on_cell_c_times_6_plus_r() {
    let mut rgb = vec![0u8; 22 * 6 * 3];
    let at = (4 * 22 + 3) * 3;
    rgb[at] = 255;
    let cells = image_cells_from_raster(&Raster { width: 22, height: 6, rgb }).unwrap();
    assert_eq!(cells[3 * 6 + 4], [255, 0, 0]);
    assert_eq!(cells[4 * 6 + 3], [0; 3]);
    assert_eq!(cells.iter().filter(|c| **c != [0; 3]).count(), 1);
}

#[test]
fn a_picture_of_the_grid_size_is_used_as_it_is() {
    let r = raster(22, 6);
    let cells = image_cells_from_raster(&r).unwrap();
    for col in 0..22 {
        for row in 0..6 {
            let cell = col * 6 + row;
            if cell < 126 {
                let at = (row * 22 + col) * 3;
                assert_eq!(cells[cell], [r.rgb[at], r.rgb[at + 1], r.rgb[at + 2]]);
            }
        }
    }
}

#[test]
fn unusable_rasters_are_refused_before_anything_is_allocated() {
    for (w, h, len) in
        [(0, 5, 0), (5, 0, 0), (usize::MAX, 2, 0), (100_000, 100_000, 3), (2, 2, 11), (2, 2, 13), (1, 1, 0)]
    {
        let err = image_cells_from_raster(&Raster { width: w, height: h, rgb: vec![0; len] }).unwrap_err();
        assert!(err.to_string().starts_with("cannot read the image: "), "{w}x{h}: {err}");
    }
    assert!(image_cells_from_raster(&Raster { width: 1, height: 1, rgb: vec![9, 8, 7] }).is_ok());
}

struct FakeDecoder;

impl ImageDecoder for FakeDecoder {
    fn decode(&self, path: &Path) -> Result<Raster, String> {
        match path.file_name().and_then(|n| n.to_str()) {
            Some("a.png") => Ok(Raster { width: 40, height: 12, rgb: [200, 10, 20].repeat(40 * 12) }),
            _ => Err("No such file or directory".to_owned()),
        }
    }
}

#[test]
fn a_decoder_supplies_the_picture_and_its_failure_is_reported() {
    let cells = image_cells(&FakeDecoder, Path::new("/pictures/a.png")).unwrap();
    assert_eq!((cells.len(), cells[5], cells[127]), (128, [200, 10, 20], [0; 3]));
    let err = image_cells(&FakeDecoder, Path::new("/pictures/missing.png")).unwrap_err();
    assert_eq!(err.to_string(), "cannot read the image: No such file or directory");
}

// ---- the templates -----------------------------------------------------------------------------

const TEMPLATE_NAMES: [&str; 8] =
    ["fps", "moba", "typing", "media", "sunset", "neon-grid", "rainbow-rows", "night"];

fn cell_of(map: &KeyMap, key: &str) -> usize {
    map.key(key).unwrap_or_else(|| panic!("no key {key}"))[0]
}

fn template_cells(name: &str) -> Vec<Color> {
    let (_dir, store) = store();
    store.get(name).unwrap().cells_of(store.key_map()).unwrap()
}

#[test]
fn eight_templates_follow_the_six_earlier_designs_and_all_of_them_check_out() {
    let (_dir, store) = store();
    let all = store.load_all();
    assert_eq!(all.len(), 14);
    let names: Vec<&str> = all.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names[EARLIER_BUILTIN..], TEMPLATE_NAMES);
    let unique: std::collections::BTreeSet<&&str> = names.iter().collect();
    assert_eq!(unique.len(), names.len());
    for (name, design) in &all {
        assert_eq!(design.source, Source::BuiltIn, "{name}");
        assert!(design.info.chars().count() <= 80 && !design.info.is_empty(), "{name}: {}", design.info);
        assert!(name.chars().all(|c| c.is_ascii_lowercase() || c == '-'), "{name}");
        let cells = design.cells_of(store.key_map()).unwrap();
        assert_eq!(cells.len(), 128, "{name}");
        assert_eq!(cells[126..], [[0; 3]; 2], "{name}: cells 126 and 127 are not keys");
    }
}

#[test]
fn every_template_is_a_real_picture_with_several_colours_and_every_key_is_lit_or_deliberately_dark() {
    for name in TEMPLATE_NAMES {
        let cells = template_cells(name);
        let colours: std::collections::BTreeSet<Color> = cells[..LIT_CELLS].iter().copied().collect();
        assert!(colours.len() >= 3, "{name}: {} colours", colours.len());
    }
}

#[test]
fn the_row_looks_paint_every_labelled_key_and_each_row_its_own_colour() {
    let map = KeyMap::verified();
    let key_cells: std::collections::BTreeSet<usize> =
        map.names().iter().flat_map(|(_, c)| c.iter().copied()).collect();
    for name in ["sunset", "neon-grid", "rainbow-rows"] {
        let cells = template_cells(name);
        let base = store().1.get(name).unwrap().base;
        let base = keymap::parse_hex(&base).unwrap();
        let unpainted: Vec<usize> = key_cells.iter().copied().filter(|c| cells[*c] == base).collect();
        assert!(unpainted.is_empty(), "{name}: keys left at the base colour: {unpainted:?}");
    }
    let rainbow = template_cells("rainbow-rows");
    let colour_of = |k: &str| rainbow[cell_of(map, k)];
    assert_eq!(colour_of("q"), colour_of("tab"));
    assert_eq!(colour_of("a"), colour_of("return"));
    assert_ne!(colour_of("q"), colour_of("a"));
    assert_ne!(colour_of("a"), colour_of("z"));
    assert_ne!(colour_of("1"), colour_of("f5"));
    assert_eq!(colour_of("space"), colour_of("ctrl"));
    let rows: std::collections::BTreeSet<Color> =
        ["esc", "1", "q", "a", "z", "space", "num5"].iter().map(|k| colour_of(k)).collect();
    assert_eq!(rows.len(), 7, "seven rows, seven colours");
}

#[test]
fn the_game_templates_pick_out_the_keys_a_player_uses() {
    let map = KeyMap::verified();
    let fps = template_cells("fps");
    let at = |cells: &[Color], k: &str| cells[cell_of(map, k)];
    for k in ["w", "a", "s", "d"] {
        assert_eq!(at(&fps, k), [0xff, 0x2a, 0x2a], "{k}");
    }
    assert_eq!(at(&fps, "space"), [0xff, 0x88, 0x00]);
    assert_eq!(at(&fps, "3"), [0x00, 0xe0, 0xff], "a weapon number");
    assert_ne!(at(&fps, "8"), at(&fps, "3"), "only the first five numbers");
    assert_eq!(at(&fps, "p"), [0x06, 0x06, 0x0c], "the rest is the dark base");
    let moba = template_cells("moba");
    for k in ["q", "w", "e", "r"] {
        assert_eq!(at(&moba, k), [0xb4, 0x4d, 0xff], "{k}");
    }
    assert_eq!((at(&moba, "d"), at(&moba, "f")), ([0, 0xd1, 0xff], [0, 0xd1, 0xff]));
    assert_eq!(at(&moba, "6"), [0xff, 0xcc, 0x00]);
    assert_ne!(at(&moba, "7"), at(&moba, "6"));
}

#[test]
fn the_typing_template_marks_the_home_row_and_the_two_bumps() {
    let map = KeyMap::verified();
    let typing = template_cells("typing");
    let at = |k: &str| typing[cell_of(map, k)];
    assert_eq!((at("f"), at("j")), ([255, 255, 255], [255, 255, 255]));
    for k in ["a", "s", "d", "g", "h", "k", "l"] {
        assert_eq!(at(k), [0x3a, 0xb0, 0xff], "{k}");
    }
    assert_ne!(at("q"), at("a"));
    assert_ne!(at("q"), at("f"));
}

#[test]
fn the_media_template_has_the_scrub_keys_the_play_key_and_the_media_keys() {
    let map = KeyMap::verified();
    let media = template_cells("media");
    let at = |k: &str| media[cell_of(map, k)];
    for k in ["up", "down", "left", "right"] {
        assert_eq!(at(k), [255, 255, 255], "{k}");
    }
    assert_eq!(at("space"), [0, 0xe6, 0x76]);
    assert_eq!(at("playpause"), at("space"));
    assert_eq!((at("j"), at("k"), at("l")), ([0xff, 0x91, 0], [0xff, 0x91, 0], [0xff, 0x91, 0]));
}

#[test]
fn the_night_template_is_dim_everywhere() {
    let night = template_cells("night");
    for (c, rgb) in night[..LIT_CELLS].iter().enumerate() {
        assert!(rgb.iter().copied().max().unwrap_or(0) <= 0x4a, "cell {c}: {rgb:?}");
    }
    assert!(night[..LIT_CELLS].iter().any(|c| *c != [0; 3]));
}
