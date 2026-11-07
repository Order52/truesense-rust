#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::*;
use crate::keymap::KeyMap;
use crate::keymap::testutil::{TempDir, hex_list};

// ---- a fake hidraw node that records every transfer, in order, across handles ----------------

struct FakeInner {
    log: Vec<String>,
    opens: Vec<usize>,
    closes: Vec<usize>,
    /// The next N transfers fail (a stale handle).
    fail_next: usize,
    open_error: Option<fn() -> KeyboardError>,
    bright: u8,
    /// `None`: the controller answers the timeout query with an all-zero report.
    timeout: Option<u8>,
    last_set: u8,
}

#[derive(Clone)]
struct Fake(Arc<Mutex<FakeInner>>);

impl Fake {
    fn new() -> Fake {
        Fake(Arc::new(Mutex::new(FakeInner {
            log: Vec::new(),
            opens: Vec::new(),
            closes: Vec::new(),
            fail_next: 0,
            open_error: None,
            bright: 25,
            timeout: Some(1),
            last_set: 0,
        })))
    }

    fn with<R>(&self, f: impl FnOnce(&mut FakeInner) -> R) -> R {
        f(&mut self.0.lock().unwrap())
    }

    fn log(&self) -> Vec<String> {
        self.with(|i| i.log.clone())
    }

    fn sets(&self) -> Vec<String> {
        self.log().into_iter().filter_map(|l| l.strip_prefix("set ").map(str::to_owned)).collect()
    }

    fn keyboard(&self, state: &TempDir) -> Keyboard {
        let log = Arc::clone(&self.0);
        Keyboard::new(Arc::new(self.clone()), Arc::new(Store::new(state.join("user.json"))))
            .with_sleeper(Arc::new(move |d| log.lock().unwrap().log.push(format!("sleep {}", d.as_millis()))))
    }
}

struct FakeHandle {
    id: usize,
    shared: Fake,
}

impl HidOpener for Fake {
    fn open(&self) -> Result<Box<dyn HidDevice>, KeyboardError> {
        self.with(|i| {
            if let Some(make) = i.open_error {
                return Err(make());
            }
            let id = 100 + i.opens.len();
            i.opens.push(id);
            Ok(Box::new(FakeHandle { id, shared: self.clone() }) as Box<dyn HidDevice>)
        })
    }
}

impl Drop for FakeHandle {
    fn drop(&mut self) {
        let id = self.id;
        self.shared.with(|i| i.closes.push(id));
    }
}

fn stale() -> io::Error {
    io::Error::from_raw_os_error(5)
}

impl HidDevice for FakeHandle {
    fn set_feature(&mut self, p: &[u8; PACKET_LEN]) -> io::Result<()> {
        self.shared.with(|i| {
            if i.fail_next > 0 {
                i.fail_next -= 1;
                return Err(stale());
            }
            i.log.push(format!("set {}", hex(p)));
            i.last_set = p[0];
            Ok(())
        })
    }

    fn get_feature(&mut self) -> io::Result<[u8; PACKET_LEN]> {
        self.shared.with(|i| {
            if i.fail_next > 0 {
                i.fail_next -= 1;
                return Err(stale());
            }
            i.log.push("get 8".to_owned());
            Ok(match i.last_set {
                0x88 => [0x88, 0, 3, 9, i.bright, 1, 1, 0x50],
                0xB0 => match i.timeout {
                    Some(t) => [0xB0, 0, t, 0, 0, 0, 0, 0x4F],
                    None => [0; 8],
                },
                _ => [0; 8],
            })
        })
    }

    fn write_output(&mut self, data: &[u8; OUTPUT_LEN]) -> io::Result<()> {
        self.shared.with(|i| {
            if i.fail_next > 0 {
                i.fail_next -= 1;
                return Err(stale());
            }
            i.log.push(format!("out {}", hex(data)));
            Ok(())
        })
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn rig() -> (Fake, Keyboard, TempDir) {
    let fake = Fake::new();
    let dir = TempDir::new("kb");
    let keyboard = fake.keyboard(&dir);
    (fake, keyboard, dir)
}

fn sim_rig() -> (Keyboard, SimHid, TempDir) {
    let dir = TempDir::new("kb-sim");
    let (keyboard, sim) = Keyboard::simulated(Arc::new(Store::new(dir.join("user.json"))));
    (keyboard, sim, dir)
}

fn store_value(dir: &TempDir, key: &str) -> Option<Value> {
    Store::new(dir.join("user.json")).get(key)
}

// ---- the Python edition's transcripts, byte for byte -----------------------------------------

fn golden() -> Value {
    serde_json::from_str(include_str!("../../testdata/keyboard_golden.json")).expect("golden json")
}

fn color_of(v: &Value) -> Color {
    let s = v.as_str().expect("hex string");
    keymap::parse_hex(s).expect("hex")
}

fn run_step(k: &Keyboard, step: &[Value]) -> Result<Value, KeyboardError> {
    let name = step[0].as_str().expect("step name");
    let a = &step[1..];
    let int = |i: usize| a[i].as_i64().expect("int arg");
    let text = |i: usize| a[i].as_str().expect("string arg");
    let maybe_level = |i: usize| a.get(i).and_then(Value::as_i64);
    match name {
        "set_effect" => k.set_effect(text(0), text(1), int(2), int(3), int(4)).map(|()| Value::Null),
        "set_operating_mode" => k
            .set_operating_mode(text(0), maybe_level(1).unwrap_or(OPERATING_BRIGHTNESS))
            .map(|()| Value::Null),
        "follow_mode" => {
            let colors: BTreeMap<String, String> = a
                .get(1)
                .map(|m| {
                    m.as_object()
                        .expect("map")
                        .iter()
                        .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
                        .collect()
                })
                .unwrap_or_default();
            k.follow_mode(text(0), &colors).map(|()| Value::Null)
        }
        "get_brightness" => k.get_brightness().map(|b| json!(b)),
        "per_key" => {
            let spec = &a[0];
            let cells: Vec<Color> = match spec.get("list") {
                Some(list) => list.as_array().unwrap().iter().map(color_of).collect(),
                None => vec![color_of(&spec["fill"]); spec["count"].as_u64().unwrap() as usize],
            };
            k.per_key(&cells, maybe_level(1)).map(|()| Value::Null)
        }
        "restore_default" => k.restore_default().map(|()| Value::Null),
        "set_brightness" => k.set_brightness(int(0)).map(|()| Value::Null),
        "off" => k.off().map(|()| Value::Null),
        "on" => k.on().map(|()| Value::Null),
        "replay" => {
            let s: State = serde_json::from_value(a[0].clone()).expect("state");
            k.replay(&s).map(|r| json!(r))
        }
        "replay_last" => k.replay_last().map(|r| json!(r)),
        "set_speed" => k.set_speed(int(0)).map(|()| Value::Null),
        "set_color" => k.set_color(text(0)).map(|()| Value::Null),
        "paint" => {
            let items: Vec<(&str, &str)> = a[0]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| (p[0].as_str().unwrap(), p[1].as_str().unwrap()))
                .collect();
            k.paint(KeyMap::verified(), &items, maybe_level(1)).map(|n| json!(n))
        }
        "paint_cells" => {
            let items: Vec<(i64, &str)> = a[0]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| (p[0].as_i64().unwrap(), p[1].as_str().unwrap()))
                .collect();
            k.paint_cells(&items, maybe_level(1)).map(|()| Value::Null)
        }
        "hid_timeout_get" => k.hid_timeout_get().map(|r| json!(r)),
        "hid_timeout_set" => k.hid_timeout_set(a[0].as_bool().unwrap()).map(|()| Value::Null),
        other => panic!("unknown step {other}"),
    }
}

#[test]
fn every_scenario_matches_the_python_transcript() {
    let g = golden();
    let scenarios = g["scenarios"].as_array().unwrap();
    assert!(scenarios.len() > 30);
    for sc in scenarios {
        let name = sc["name"].as_str().unwrap();
        let (fake, k, dir) = rig();
        fake.with(|i| {
            i.bright = sc["bright"].as_u64().unwrap() as u8;
            i.timeout = sc["timeout"].as_u64().map(|t| t as u8);
        });
        let store = Store::new(dir.join("user.json"));
        for (key, value) in sc["init"].as_object().unwrap() {
            store.set(key, value.clone()).unwrap();
        }
        let mut results = Vec::new();
        let mut want_log: Vec<String> = Vec::new();
        for step in sc["steps"].as_array().unwrap() {
            let r = run_step(&k, step.as_array().unwrap());
            results.push(match r {
                Ok(v) => v,
                Err(e) => json!(format!("ERR: {e}")),
            });
            fake.with(|i| i.log.push("--".to_owned()));
        }
        want_log.extend(sc["log"].as_array().unwrap().iter().map(|l| l.as_str().unwrap().to_owned()));
        let got_log = fake.log();
        if got_log != want_log {
            let at = got_log
                .iter()
                .zip(&want_log)
                .position(|(a, b)| a != b)
                .unwrap_or(got_log.len().min(want_log.len()));
            panic!(
                "{name}: transcript differs at line {at}\n  got:  {:?}\n  want: {:?}",
                got_log.get(at),
                want_log.get(at)
            );
        }
        assert_eq!(Value::Array(results), sc["results"], "{name}: results");
        let state = store.load();
        let want_state = sc["state"].as_object().unwrap();
        for key in ["keyboard", "kb_restore"] {
            assert_eq!(state.get(key), want_state.get(key), "{name}: user state {key}");
        }
    }
}

#[test]
fn the_tables_match_python() {
    let g = golden();
    let effects: Vec<(String, u8)> = g["effects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e[0].as_str().unwrap().to_owned(), e[1].as_u64().unwrap() as u8))
        .collect();
    assert_eq!(effects, EFFECTS.iter().map(|(n, c)| ((*n).to_owned(), *c)).collect::<Vec<_>>());
    let no_color: Vec<&str> = g["no_color"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    let mut without_color: Vec<&str> = effect_names().into_iter().filter(|n| !has_color(n)).collect();
    without_color.sort_unstable();
    assert_eq!(without_color, no_color);
    let want: BTreeMap<String, String> = g["operating_colors"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
        .collect();
    assert_eq!(operating_colors(), want);
    assert_eq!(effect_code("wave"), Some(3));
    assert_eq!(effect_code("nosuch"), None);
}

// ---- packets ---------------------------------------------------------------------------------

#[test]
fn checksum_is_0xff_minus_the_sum_of_the_data_modulo_256() {
    assert_eq!(hex(&packet([0x08, 0x02, 0x33, 0x05, 0x14, 0x08, 0x01])), "08023305140801a0");
    assert_eq!(hex(&packet([0; 7])), "00000000000000ff");
    assert_eq!(checksum(&[0xFF; 7]), 0xFF - ((0xFF_u32 * 7) & 0xFF) as u8, "the sum wraps");
    assert_eq!(checksum(&[]), 0xFF);
    assert_eq!(checksum(&[0x88, 0, 0, 0, 0, 0, 0]), 0x77, "the brightness query's own checksum");
    assert_eq!(checksum(&[0xB0, 0, 0, 0, 0, 0, 0]), 0x4F);
}

#[test]
fn the_speed_scale_covers_the_keyboards_whole_range_and_never_sends_an_invalid_byte() {
    let (k, sim, _d) = sim_rig();
    let mut bytes = Vec::new();
    for speed in SPEED_MIN..=SPEED_MAX {
        sim.clear_log();
        k.set_effect("wave", "00aec7", speed, 25, 0).unwrap();
        bytes.push(sim.packets().iter().rev().find(|p| p[0] == 0x08).unwrap()[3]);
    }
    assert_eq!(
        bytes,
        (0..=11).rev().collect::<Vec<u8>>(),
        "every step is one byte step faster than the last"
    );
    for bad in [0, 13, -1, 99, i64::MIN, i64::MAX] {
        assert!(k.set_effect("wave", "00aec7", bad, 25, 0).is_err(), "{bad}");
    }
    assert!(k.set_speed(13).is_err());
}

#[test]
fn hostile_numbers_are_refused_before_anything_is_sent() {
    let (k, sim, _d) = sim_rig();
    sim.clear_log();
    for (brightness, direction) in [(51, 0), (-1, 0), (i64::MAX, 0), (30, 255), (30, -1), (30, i64::MAX)] {
        assert!(
            k.set_effect("static", "ffffff", 5, brightness, direction).is_err(),
            "{brightness} {direction}"
        );
    }
    assert!(k.set_effect("static", "ffffff", 5, 30, 254).is_ok(), "254 + 1 is the last direction byte");
    assert!(k.set_operating_mode("ffffff", 256).is_err());
    assert!(k.set_operating_mode("ffffff", -1).is_err());
    assert!(k.set_operating_mode("ffffff", 255).is_ok());
    assert!(k.per_key(&[], Some(51)).is_err());
    assert!(k.per_key(&[], Some(-1)).is_err());
    assert!(k.set_brightness(51).is_err());
    // Only the two accepted calls above reached the keyboard.
    assert_eq!(sim.packets().len(), 3 + 2);
    for bad in ["", "zz", "ff00", "ff00000", "#", "é"] {
        assert_eq!(k.set_color(bad).unwrap_err().to_string(), "colour must be RRGGBB", "{bad:?}");
    }
}

#[test]
fn a_hash_in_the_colour_is_accepted_and_stored_without_it() {
    // The Python edition crashed here (bytes.fromhex("#...")); the state still never holds a '#'.
    let (k, sim, d) = sim_rig();
    k.set_effect("static", "#AABBCC", 5, 30, 0).unwrap();
    assert!(sim.packets().iter().any(|p| p[..6] == [0x14, 0, 0, 0xAA, 0xBB, 0xCC]));
    assert_eq!(store_value(&d, "keyboard").unwrap()["color"], "aabbcc");
}

// ---- looks and the user state ----------------------------------------------------------------

#[test]
fn an_effect_is_remembered_in_the_form_the_earlier_editions_wrote() {
    let (k, _sim, d) = sim_rig();
    k.set_effect("wave", "00FF00", 4, 20, 1).unwrap();
    assert_eq!(
        store_value(&d, "keyboard").unwrap(),
        json!({"mode": "effect", "name": "wave", "color": "00ff00", "speed": 4, "brightness": 20, "direction": 1})
    );
    k.set_effect("off", "ffffff", 5, 30, 0).unwrap();
    assert_eq!(store_value(&d, "keyboard").unwrap(), json!({"mode": "off"}));
    assert_eq!(k.last().mode, "off");
}

#[test]
fn state_files_of_the_earlier_editions_load_and_absent_fields_stay_absent() {
    let old: State =
        serde_json::from_str(r#"{"mode": "effect", "name": "wave", "color": "00ff00", "extra": [1]}"#)
            .unwrap();
    assert_eq!((old.speed, old.brightness, old.direction), (None, None, None));
    assert_eq!(
        (old.speed_or(5), old.brightness_or(25), old.direction_or(0), old.color_or("x")),
        (5, 25, 0, "00ff00")
    );
    assert_eq!(
        serde_json::to_value(&old).unwrap(),
        json!({"mode": "effect", "name": "wave", "color": "00ff00"})
    );
    assert!(State::default().is_empty() && !old.is_empty());
    let dark: State = serde_json::from_str(r#"{"mode": "perkey", "brightness": 0}"#).unwrap();
    assert_eq!(dark.brightness_or(25), 0, "a brightness of 0 is a real setting");
    assert!(serde_json::from_str::<State>(r#"{"mode": 5}"#).is_err());
    assert!(State { cells: vec!["zz".into()], ..State::default() }.parse_cells().is_err());
}

#[test]
fn a_look_of_the_wrong_shape_in_the_user_state_reads_as_unknown() {
    let (k, _sim, d) = sim_rig();
    let store = Store::new(d.join("user.json"));
    for bad in [json!(5), json!("wave"), json!([1]), json!({"mode": 7}), json!(null)] {
        store.set("keyboard", bad).unwrap();
        assert!(k.last().is_empty());
    }
    // ... and a brightness change then simply sends the per-key packet.
    let (fake, k, d) = rig();
    Store::new(d.join("user.json")).set("keyboard", json!({"mode": 7})).unwrap();
    k.set_brightness(10).unwrap();
    assert_eq!(fake.sets(), [hex(&packet([0x08, 0x02, 0x33, 0x05, 10, 0x08, 0x01]))]);
}

#[test]
fn off_and_on_restore_the_look_through_the_simulator() {
    let (k, sim, d) = sim_rig();
    k.set_effect("breathing", "ff0000", 3, 40, 0).unwrap();
    k.off().unwrap();
    assert_eq!(sim.brightness(), 0, "brightness 0 is allowed and is how the backlight goes off");
    assert_eq!(store_value(&d, "keyboard").unwrap()["brightness"], 0);
    assert_eq!(store_value(&d, "kb_restore").unwrap()["brightness"], 40);
    k.on().unwrap();
    assert_eq!(sim.brightness(), 40);
    assert_eq!(store_value(&d, "keyboard").unwrap()["name"], "breathing");
}

#[test]
fn off_when_already_dark_does_not_overwrite_the_saved_look() {
    let (k, sim, d) = sim_rig();
    k.set_effect("wave", "ffffff", 3, 40, 0).unwrap();
    k.off().unwrap();
    k.off().unwrap();
    assert_eq!(
        store_value(&d, "kb_restore").unwrap()["brightness"],
        40,
        "a dark keyboard has nothing to save"
    );
    k.on().unwrap();
    assert_eq!(sim.brightness(), 40);
}

#[test]
fn current_cells_pads_to_128_and_falls_back_to_the_stock_cyan() {
    let (k, _sim, d) = sim_rig();
    let stock = keymap::canvas([0x00, 0xAE, 0xC7]);
    assert_eq!(k.current_cells(), stock);
    k.per_key(&[[1, 2, 3], [4, 5, 6]], Some(10)).unwrap();
    let cells = k.current_cells();
    assert_eq!(
        (cells.len(), cells[0], cells[1], cells[2], cells[127]),
        (128, [1, 2, 3], [4, 5, 6], [0; 3], [0; 3])
    );
    let state = store_value(&d, "keyboard").unwrap();
    assert_eq!(state["cells"].as_array().unwrap().len(), 128);
    assert_eq!(state["cells"][1], "040506");
    Store::new(d.join("user.json")).set("keyboard", json!({"mode": "perkey", "cells": ["zz0000"]})).unwrap();
    assert_eq!(k.current_cells(), stock, "damaged remembered colours do not break painting");
    assert!(matches!(k.replay_last(), Err(KeyboardError::BadState(_))), "but a replay says so");
}

#[test]
fn per_key_frames_do_not_replace_the_remembered_look() {
    let (k, sim, d) = sim_rig();
    k.set_effect("wave", "ffffff", 3, 40, 0).unwrap();
    sim.clear_log();
    k.per_key_frame(&[[9, 9, 9]], Some(7)).unwrap();
    assert_eq!(store_value(&d, "keyboard").unwrap()["name"], "wave");
    assert_eq!(sim.frames(), 8);
    assert_eq!(sim.last_cells().unwrap()[0], [9, 9, 9]);
    assert_eq!(sim.brightness(), 7);
}

#[test]
fn restore_default_is_the_factory_look() {
    let (k, sim, d) = sim_rig();
    k.restore_default().unwrap();
    let cells = sim.last_cells().unwrap();
    assert_eq!(
        (cells[0], cells[125], cells[126], cells[127]),
        ([0, 0xAE, 0xC7], [0, 0xAE, 0xC7], [0; 3], [0; 3])
    );
    assert_eq!(
        (sim.brightness(), store_value(&d, "keyboard").unwrap()["brightness"].clone()),
        (12, json!(12))
    );
    assert_eq!(hex_list(&cells[..1]), ["00aec7"]);
}

// ---- the kept handle -------------------------------------------------------------------------

#[test]
fn the_device_is_opened_once_and_kept() {
    let (fake, k, _d) = rig();
    for _ in 0..3 {
        assert_eq!(k.get_brightness().unwrap(), 25);
    }
    k.hid_timeout_get().unwrap();
    k.hid_timeout_get().unwrap();
    k.hid_timeout_set(true).unwrap();
    k.per_key_frame(&[[1, 2, 3]; 128], Some(20)).unwrap();
    assert_eq!(
        fake.with(|i| (i.opens.clone(), i.closes.clone())),
        (vec![100], vec![]),
        "one open for all of that, not one per operation"
    );
}

#[test]
fn a_stale_handle_is_reopened_once_and_the_call_still_succeeds() {
    let (fake, k, _d) = rig();
    k.get_brightness().unwrap();
    fake.with(|i| i.fail_next = 1);
    assert_eq!(k.get_brightness().unwrap(), 25);
    assert_eq!(fake.with(|i| (i.opens.clone(), i.closes.clone())), (vec![100, 101], vec![100]));
}

#[test]
fn a_stale_handle_in_the_middle_of_an_upload_restarts_the_whole_upload() {
    let fake = Fake::new();
    let dir = TempDir::new("kb-midway");
    let sleeps = Arc::new(Mutex::new(0usize));
    let (counter, device) = (Arc::clone(&sleeps), fake.clone());
    let k = Keyboard::new(Arc::new(fake.clone()), Arc::new(Store::new(dir.join("user.json")))).with_sleeper(
        Arc::new(move |_| {
            let mut n = counter.lock().unwrap();
            *n += 1;
            if *n == 4 {
                // The 4th report went through; the 5th transfer finds the handle stale.
                device.with(|i| i.fail_next = 1);
            }
        }),
    );
    k.per_key_frame(&[[1, 2, 3]; 128], Some(20)).unwrap();
    let log = fake.log();
    assert_eq!(
        log.iter().filter(|l| l.starts_with("out ")).count(),
        4 + 8,
        "4 reports before the failure, then all 8 again"
    );
    assert_eq!(
        log.iter().filter(|l| l.starts_with("set 88")).count(),
        2,
        "the upload starts over from its first packet"
    );
    assert_eq!(fake.sets().last().unwrap(), &hex(&packet([0x08, 0x02, 0x33, 0x05, 20, 0x08, 0x01])));
    assert_eq!(fake.with(|i| (i.opens.len(), i.closes.len())), (2, 1));
}

#[test]
fn a_second_failure_is_returned_and_nothing_stale_is_kept() {
    let (fake, k, _d) = rig();
    k.get_brightness().unwrap();
    fake.with(|i| i.fail_next = 2);
    assert!(matches!(k.get_brightness(), Err(KeyboardError::Io { .. })));
    assert_eq!(fake.with(|i| (i.opens.len(), i.closes.len())), (2, 2), "both handles are closed");
    assert_eq!(k.get_brightness().unwrap(), 25, "the next call opens a fresh handle");
    assert_eq!(fake.with(|i| i.opens.len()), 3);
}

#[test]
fn failing_to_open_is_reported_at_once_and_not_retried() {
    let (fake, k, _d) = rig();
    fake.with(|i| i.open_error = Some(|| KeyboardError::Permission { node: "/dev/hidraw7".into() }));
    let err = k.get_brightness().unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("no permission") && text.contains("plugdev") && text.contains("/dev/hidraw7"),
        "{text}"
    );
    fake.with(|i| i.open_error = Some(|| KeyboardError::NotFound));
    assert_eq!(
        k.hid_timeout_get().unwrap_err().to_string(),
        "keyboard lighting interface (usage page 0xFF02) not found"
    );
    assert!(fake.log().is_empty(), "nothing was sent");
    fake.with(|i| i.open_error = None);
    assert_eq!(k.get_brightness().unwrap(), 25, "it recovers when the device appears");
}

#[test]
fn close_releases_the_handle_and_the_next_call_reopens() {
    let (fake, k, _d) = rig();
    k.get_brightness().unwrap();
    k.close();
    k.close();
    assert_eq!(fake.with(|i| i.closes.clone()), vec![100]);
    k.get_brightness().unwrap();
    assert_eq!(fake.with(|i| i.opens.clone()), vec![100, 101]);
}

#[test]
fn transfers_of_different_threads_never_interleave() {
    let (fake, k, _d) = rig();
    let k = Arc::new(k);
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let k = Arc::clone(&k);
            thread::spawn(move || {
                for _ in 0..150 {
                    k.get_brightness().unwrap();
                }
            })
        })
        .collect();
    threads.into_iter().for_each(|t| t.join().unwrap());
    let log = fake.log();
    assert_eq!(log.len(), 4 * 150 * 2);
    for pair in log.chunks(2) {
        assert_eq!(pair, ["set 8800000000000077", "get 8"], "a request is always followed by its own reply");
    }
    assert_eq!(fake.with(|i| i.opens.clone()), vec![100]);
}

#[test]
fn a_per_key_upload_is_never_split_by_another_threads_query() {
    let (fake, k, _d) = rig();
    let k = Arc::new(k);
    let uploader = {
        let k = Arc::clone(&k);
        thread::spawn(move || {
            for _ in 0..20 {
                k.per_key_frame(&[[1, 1, 1]; 128], Some(10)).unwrap();
            }
        })
    };
    let reader = {
        let k = Arc::clone(&k);
        thread::spawn(move || {
            for _ in 0..200 {
                k.get_brightness().unwrap();
            }
        })
    };
    uploader.join().unwrap();
    reader.join().unwrap();
    let log: Vec<String> = fake.log().into_iter().filter(|l| !l.starts_with("sleep")).collect();
    let mut i = 0;
    while i < log.len() {
        if log[i] == "set 12000008000000e5" {
            assert!(
                log[i + 1..i + 9].iter().all(|l| l.starts_with("out ")),
                "8 reports in a row after the 12 packet"
            );
            assert_eq!(log[i + 9], format!("set {}", hex(&packet([0x08, 0x02, 0x33, 0x05, 10, 0x08, 0x01]))));
            i += 10;
        } else {
            i += 1;
        }
    }
}

#[test]
fn prewarm_opens_in_the_background_without_a_second_open() {
    let (fake, k, _d) = rig();
    let k = Arc::new(k);
    k.prewarm().join().unwrap();
    assert_eq!(fake.with(|i| i.opens.clone()), vec![100]);
    assert_eq!(k.get_brightness().unwrap(), 25);
    assert_eq!(
        fake.with(|i| i.opens.clone()),
        vec![100],
        "the foreground call reuses what the background open made"
    );
}

#[test]
fn prewarm_swallows_a_failure_and_leaves_it_for_the_first_call() {
    let (fake, k, _d) = rig();
    fake.with(|i| i.open_error = Some(|| KeyboardError::NotFound));
    let k = Arc::new(k);
    k.prewarm().join().unwrap();
    assert!(k.get_brightness().is_err());
}

// ---- pacing ----------------------------------------------------------------------------------

#[test]
fn a_per_key_frame_is_eight_reports_paced_16_ms_apart_and_settles_50_ms() {
    let (fake, k, _d) = rig();
    k.per_key_frame(&[[1, 2, 3]; 128], Some(20)).unwrap();
    let paced: Vec<String> =
        fake.log().into_iter().filter(|l| l.starts_with("out") || l.starts_with("sleep")).collect();
    assert_eq!(paced.len(), 8 * 2 + 1);
    for pair in paced[..16].chunks(2) {
        assert!(pair[0].starts_with("out ") && pair[1] == "sleep 16", "{pair:?}");
    }
    assert_eq!(paced[16], "sleep 50");
    assert_eq!(FRAME_SPACING, Duration::from_millis(16));
    assert_eq!(FRAME_SETTLE, Duration::from_millis(50));
}

#[test]
fn the_real_pacing_is_used_unless_a_sleeper_is_given() {
    let fake = Fake::new();
    let dir = TempDir::new("kb-real-sleep");
    let k = Keyboard::new(Arc::new(fake.clone()), Arc::new(Store::new(dir.join("u.json"))));
    let started = Instant::now();
    k.per_key_frame(&[], Some(1)).unwrap();
    assert!(started.elapsed() >= Duration::from_millis(8 * 16 + 50), "{:?}", started.elapsed());
}

// ---- the simulator ---------------------------------------------------------------------------

#[test]
fn the_simulated_keyboard_tracks_brightness_like_the_hardware() {
    let (k, sim, _d) = sim_rig();
    assert_eq!(k.get_brightness().unwrap(), 25, "factory state");
    k.set_effect("wave", "ffffff", 5, 33, 0).unwrap();
    assert_eq!(k.get_brightness().unwrap(), 33);
    k.set_operating_mode("00aec7", 0x32).unwrap();
    assert_eq!(k.get_brightness().unwrap(), 0x32);
    k.per_key(&[], Some(12)).unwrap();
    assert_eq!((k.get_brightness().unwrap(), sim.brightness()), (12, 12));
    assert_eq!(sim.frames(), 8);
}

#[test]
fn the_simulated_timeout_flag_starts_on_and_follows_writes() {
    let (k, sim, _d) = sim_rig();
    assert!(sim.timeout());
    assert_eq!(k.hid_timeout_get().unwrap(), Some(true));
    k.hid_timeout_set(false).unwrap();
    assert_eq!((k.hid_timeout_get().unwrap(), sim.timeout()), (Some(false), false));
    k.hid_timeout_set(true).unwrap();
    assert_eq!(k.hid_timeout_get().unwrap(), Some(true));
}

#[test]
fn clones_of_the_simulator_are_one_keyboard() {
    let a = SimHid::new();
    let b = a.clone();
    let mut handle = a.open().unwrap();
    handle.set_feature(&packet([0x08, 0x02, 0x33, 0x05, 9, 0x08, 0x01])).unwrap();
    assert_eq!((b.brightness(), b.packets().len()), (9, 1));
    assert!(SimHid::new().packets().is_empty(), "separate simulators are separate keyboards");
    let one = SimHid::shared();
    let two = SimHid::shared();
    one.clear_log();
    assert_eq!(two.frames(), 0);
}

#[test]
fn the_timeout_flag_adapter_speaks_the_hw_trait() {
    let dir = TempDir::new("kb-flag");
    let (k, sim) = Keyboard::simulated(Arc::new(Store::new(dir.join("user.json"))));
    let flag: Arc<dyn KbTimeoutFlag> = Arc::new(Arc::new(k).timeout_flag());
    assert_eq!(flag.get().unwrap(), Some(true));
    flag.set(false).unwrap();
    assert_eq!((flag.get().unwrap(), sim.timeout()), (Some(false), false));
    // An unreadable controller becomes an hw error that carries the operation.
    let fake = Fake::new();
    fake.with(|i| i.open_error = Some(|| KeyboardError::NotFound));
    let d2 = TempDir::new("kb-flag2");
    let broken = Arc::new(fake.keyboard(&d2)).timeout_flag();
    let text = broken.get().unwrap_err().to_string();
    assert!(text.starts_with("read keyboard timeout flag: ") && text.contains("not found"), "{text}");
    assert!(broken.set(true).unwrap_err().to_string().starts_with("set keyboard timeout flag: "));
}

#[test]
fn the_simulator_answers_the_queries_like_the_controller_and_reports_unknown_ones_as_zero() {
    let mut sim = SimHid::new();
    sim.set_feature(&[0x88, 0, 0, 0, 0, 0, 0, 0x77]).unwrap();
    assert_eq!(sim.get_feature().unwrap(), packet([0x88, 0, 0, 0, 25, 0, 0]));
    sim.set_feature(&[0xB0, 0, 0, 0, 0, 0, 0, 0x4F]).unwrap();
    assert_eq!(sim.get_feature().unwrap(), packet([0xB0, 0, 1, 0, 0, 0, 0]));
    sim.set_feature(&[0x99, 0, 0, 0, 0, 0, 0, 0]).unwrap();
    assert_eq!(sim.get_feature().unwrap()[0], 0xB0, "an unknown command leaves the pending query alone");
    let mut fresh = SimHid::new();
    fresh.set_feature(&[0x99, 0, 0, 0, 0, 0, 0, 0]).unwrap();
    assert_eq!(fresh.get_feature().unwrap(), packet([0; 7]), "nothing was asked: an all-zero report");
    assert!(sim.last_cells().is_none());
}

#[test]
fn a_simulated_keyboard_keeps_working_after_a_poisoned_lock() {
    let sim = SimHid::new();
    let s2 = sim.clone();
    let _ = thread::spawn(move || {
        let _guard = s2.inner.lock().unwrap();
        panic!("poison the lock");
    })
    .join();
    assert_eq!(sim.brightness(), 25);
}

// ---- misc ------------------------------------------------------------------------------------

#[test]
fn replaying_an_unknown_or_empty_look_does_nothing() {
    let (k, sim, _d) = sim_rig();
    for s in [
        State::default(),
        State { mode: "perkey".into(), ..State::default() },
        State { mode: "effect".into(), name: "nosuch".into(), ..State::default() },
        State { mode: "off".into(), ..State::default() },
        State { mode: "bogus".into(), ..State::default() },
    ] {
        assert_eq!(k.replay(&s).unwrap(), None, "{s:?}");
    }
    assert!(sim.packets().is_empty());
}

#[test]
fn follow_mode_uses_the_overrides_and_the_stock_cyan_for_an_unknown_mode() {
    let (k, sim, _d) = sim_rig();
    let colors = BTreeMap::from([("turbo".to_owned(), "123456".to_owned())]);
    let colour_sent =
        |sim: &SimHid| sim.packets().iter().rev().find(|p| p[0] == 0x14).map(|p| [p[3], p[4], p[5]]).unwrap();
    k.follow_mode("turbo", &colors).unwrap();
    assert_eq!(colour_sent(&sim), [0x12, 0x34, 0x56]);
    k.follow_mode("eco", &colors).unwrap();
    assert_eq!(colour_sent(&sim), [0x00, 0xCC, 0x66]);
    k.follow_mode("nosuch", &BTreeMap::new()).unwrap();
    assert_eq!(colour_sent(&sim), [0x00, 0xAE, 0xC7]);
}

#[test]
fn painting_does_nothing_when_any_name_is_bad() {
    let (k, sim, _d) = sim_rig();
    let map = KeyMap::verified();
    assert!(k.paint(map, &[("w", "ff0000"), ("nokey", "00ff00")], Some(10)).is_err());
    assert!(k.paint(map, &[("w", "zz")], Some(10)).is_err());
    assert_eq!(sim.frames(), 0, "nothing was uploaded");
    assert_eq!(k.paint(map, &[("w", "ff0000"), ("wasd", "0000ff")], Some(10)).unwrap(), 2);
    let cells = sim.last_cells().unwrap();
    assert_eq!(cells[map.key("w").unwrap()[0]], [0, 0, 255], "the later name wins");
    assert_eq!(cells[map.key("esc").unwrap()[0]], [0x00, 0xAE, 0xC7], "other keys stay as they were");
    assert!(k.paint_cells(&[(128, "ffffff")], None).is_err());
    assert!(k.paint_cells(&[(-1, "ffffff")], None).is_err());
    k.paint_cells(&[(5, "#010203")], Some(10)).unwrap();
    assert_eq!(sim.last_cells().unwrap()[5], [1, 2, 3]);
}

#[test]
fn the_errors_say_what_failed() {
    assert_eq!(KeyboardError::Invalid("x".into()).to_string(), "x");
    let io = KeyboardError::Io { op: "write per-key report", source: io::Error::from_raw_os_error(5) };
    assert!(io.to_string().starts_with("write per-key report: "));
    assert!(std::error::Error::source(&io).is_some());
    let e: KeyboardError = KeymapError::UnknownKey("q".into()).into();
    assert_eq!(e.to_string(), "unknown key 'q' (see: truesense keyboard list-keys)");
    assert!(
        KeyboardError::BadState(KeymapError::BadColour("zz".into()))
            .to_string()
            .starts_with("remembered per-key colours: ")
    );
    assert!(std::error::Error::source(&KeyboardError::NotFound).is_none());
}

#[test]
fn nothing_is_left_running_after_the_keyboard_is_dropped() {
    static DROPPED: AtomicBool = AtomicBool::new(false);
    struct Handle;
    impl HidDevice for Handle {
        fn set_feature(&mut self, _: &[u8; PACKET_LEN]) -> io::Result<()> {
            Ok(())
        }
        fn get_feature(&mut self) -> io::Result<[u8; PACKET_LEN]> {
            Ok([0; PACKET_LEN])
        }
        fn write_output(&mut self, _: &[u8; OUTPUT_LEN]) -> io::Result<()> {
            Ok(())
        }
    }
    impl Drop for Handle {
        fn drop(&mut self) {
            DROPPED.store(true, Ordering::SeqCst);
        }
    }
    struct Opener;
    impl HidOpener for Opener {
        fn open(&self) -> Result<Box<dyn HidDevice>, KeyboardError> {
            Ok(Box::new(Handle))
        }
    }
    let dir = TempDir::new("kb-drop");
    let k = Keyboard::new(Arc::new(Opener), Arc::new(Store::new(dir.join("u.json"))));
    k.get_brightness().unwrap();
    assert!(!DROPPED.load(Ordering::SeqCst));
    drop(k);
    assert!(DROPPED.load(Ordering::SeqCst), "the descriptor is closed with the keyboard");
}

#[test]
fn garbage_arguments_and_garbage_remembered_looks_never_panic() {
    let (k, _sim, d) = sim_rig();
    let names = ["", "wave", "off", "static", "operatingmode", "nosuch", "WAVE", "é", "\0", "a b"];
    let colors = ["", "ffffff", "#", "##ffffff", "zz", "fffff", "fffffff", "ＦＦＦＦＦＦ", "ff00 0", "\n"];
    let ints = [i64::MIN, -1, 0, 1, 5, 12, 13, 50, 51, 254, 255, 256, i64::MAX];
    for name in names {
        for color in colors {
            for speed in ints {
                let _ = k.set_effect(
                    name,
                    color,
                    speed,
                    ints[(speed as usize) % ints.len()],
                    speed.wrapping_sub(3),
                );
            }
            let _ = k.set_operating_mode(color, ints[color.len() % ints.len()]);
            let _ = k.set_color(color);
            let _ = k.paint(KeyMap::verified(), &[(name, color)], None);
        }
    }
    for level in ints {
        let _ = k.set_brightness(level);
        let _ = k.set_speed(level);
        let _ = k.per_key(&[[1, 2, 3]], Some(level));
        let _ = k.paint_cells(&[(level, "ffffff")], None);
        let _ = k.off();
        let _ = k.on();
    }
    let store = Store::new(d.join("user.json"));
    for stored in [
        json!({"mode": "effect", "name": "wave", "speed": i64::MAX, "brightness": i64::MIN, "direction": -5, "color": "zz"}),
        json!({"mode": "operating", "brightness": 1_000_000, "color": "#"}),
        json!({"mode": "perkey", "cells": ["", "zz", "ffffff", "f"], "brightness": 99}),
        json!({"mode": "perkey", "cells": []}),
        json!({"mode": "effect"}),
        json!({"brightness": -1}),
    ] {
        store.set("keyboard", stored.clone()).unwrap();
        store.set("kb_restore", stored).unwrap();
        let _ = k.replay_last();
        let _ = k.set_brightness(10);
        let _ = k.set_speed(3);
        let _ = k.set_color("ffffff");
        let _ = k.off();
        let _ = k.on();
        let _ = k.current_cells();
    }
}
