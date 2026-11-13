#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use ts_foundation::userstate::Store;

use super::*;
use crate::keymap::testutil::TempDir;
use crate::keymap::{self, KeyMap};

fn golden() -> Value {
    serde_json::from_str(include_str!("../../testdata/perkeyfx_golden.json")).expect("golden json")
}

fn colors(hexes: &[String]) -> Vec<Color> {
    hexes.iter().map(|h| keymap::parse_hex(h).unwrap()).collect()
}

fn cyan() -> Vec<Color> {
    keymap::canvas([0x00, 0xAE, 0xC7])
}

fn hex_frame(cells: &[Color]) -> String {
    cells.iter().map(|c| keymap::hex(*c)).collect()
}

// ---- frames: the Python edition's, bit for bit ------------------------------------------------

#[test]
fn every_golden_frame_matches_python() {
    let g = golden();
    let bases: std::collections::BTreeMap<String, Vec<Color>> = g["bases"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), colors(&serde_json::from_value::<Vec<String>>(v.clone()).unwrap())))
        .collect();
    let frames = g["frames"].as_array().unwrap();
    assert!(frames.len() > 250, "{} golden frames", frames.len());
    let mut bad = Vec::new();
    for f in frames {
        let keys: Vec<usize> = f["keys"]
            .as_array()
            .map(|a| a.iter().map(|k| k.as_u64().unwrap() as usize).collect())
            .unwrap_or_default();
        let got = hex_frame(&render(
            f["mode"].as_str().unwrap(),
            f["t"].as_str().unwrap().parse::<f64>().unwrap(),
            &bases[f["base"].as_str().unwrap()],
            &keys,
            f["speed"].as_i64().unwrap(),
        ));
        let want = f["cells"].as_str().unwrap();
        if got != want {
            let cell = (0..128).find(|c| got[c * 6..c * 6 + 6] != want[c * 6..c * 6 + 6]).unwrap();
            bad.push(format!(
                "{} base={} t={} speed={} keys={:?}: cell {cell} got {} want {}",
                f["mode"],
                f["base"],
                f["t"],
                f["speed"],
                f["keys"],
                &got[cell * 6..cell * 6 + 6],
                &want[cell * 6..cell * 6 + 6]
            ));
        }
    }
    assert!(
        bad.is_empty(),
        "{} of {} frames differ from Python, e.g.\n{}",
        bad.len(),
        frames.len(),
        bad[..bad.len().min(5)].join("\n")
    );
}

#[test]
fn the_hash_matches_python_vectors_including_negative_inputs() {
    let g = golden();
    let vectors = g["hash"].as_array().unwrap();
    assert!(vectors.len() >= 100);
    for v in vectors {
        let (i, k, z) = (v[0].as_i64().unwrap(), v[1].as_i64().unwrap(), v[2].as_i64().unwrap());
        assert_eq!(hash(i, k, z), v[3].as_str().unwrap().parse::<f64>().unwrap(), "hash({i}, {k}, {z})");
    }
    assert!((0..1000).all(|i| (0.0..1.0).contains(&hash(i, -i, i * 3))));
    assert!((0.0..1.0).contains(&hash(i64::MIN, i64::MAX, i64::MIN)), "wrapping never panics");
}

#[test]
fn the_mode_list_matches_python() {
    let g = golden();
    let want: Vec<(String, String)> = g["modes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| (m[0].as_str().unwrap().to_owned(), m[1].as_str().unwrap().to_owned()))
        .collect();
    // The earlier editions' 18 come first and unchanged; the sweeps after them are this edition's.
    assert_eq!(
        MODES.iter().take(18).map(|(n, l)| ((*n).to_owned(), (*l).to_owned())).collect::<Vec<_>>(),
        want
    );
    assert_eq!(mode_names().len(), 20);
    assert!(is_mode("pulse") && is_mode("mood"));
    assert!(!is_mode("off") && !is_mode("disco") && !is_mode("") && !is_mode("Pulse"));
}

#[test]
fn animations_only_touch_the_chosen_keys_and_change_over_time() {
    // test_core.py: test_per_key_animations_only_touch_the_chosen_keys
    let map = KeyMap::verified();
    let (w, a, esc) = (map.key("w").unwrap()[0], map.key("a").unwrap()[0], map.key("esc").unwrap()[0]);
    let base = cyan();
    for mode in mode_names() {
        let f0 = render(mode, 0.0, &base, &[w, a], 5);
        assert_eq!(f0.len(), 128);
        assert_eq!(f0[esc], [0x00, 0xAE, 0xC7], "{mode}: keys that were not chosen must keep their colour");
        let seen: std::collections::BTreeSet<[Color; 2]> = (0..60)
            .map(|i| render(mode, 0.37 * f64::from(i), &base, &[w, a], 5))
            .map(|f| [f[w], f[a]])
            .collect();
        assert!(seen.len() > 1, "{mode} must change over time");
    }
    let wave = render("wave", 0.5, &base, &[], 5);
    let distinct: std::collections::BTreeSet<Color> = (0..126).step_by(6).map(|c| wave[c]).collect();
    assert!(distinct.len() > 5, "a wave over all keys shows many colours");
}

#[test]
fn render_edge_cases() {
    let base = cyan();
    // Not an animation: the base, untouched.
    for mode in ["off", "disco", ""] {
        assert_eq!(render(mode, 3.0, &base, &[], 5), base, "{mode:?}");
    }
    // The speed is clamped to 1..10.
    assert_eq!(render("pulse", 1.0, &base, &[], 0), render("pulse", 1.0, &base, &[], 1));
    assert_eq!(render("pulse", 1.0, &base, &[], -99), render("pulse", 1.0, &base, &[], 1));
    assert_eq!(render("pulse", 1.0, &base, &[], 99), render("pulse", 1.0, &base, &[], 10));
    assert_eq!(render("pulse", 1.0, &base, &[], i64::MAX), render("pulse", 1.0, &base, &[], 10));
    // A short base is padded with black, a long one cut; keys outside the keyboard are ignored.
    let short = render("off", 0.0, &[[1, 2, 3]], &[], 5);
    assert_eq!((short.len(), short[0], short[1]), (128, [1, 2, 3], [0; 3]));
    assert_eq!(render("off", 0.0, &vec![[9, 9, 9]; 500], &[], 5).len(), 128);
    let f = render("pulse", 0.0, &base, &[0, 127, 128, 1000, usize::MAX], 5);
    assert_eq!(f.len(), 128);
    assert_ne!(f[0], base[0]);
    assert_ne!(f[127], base[127], "cell 127 can be chosen even though 'all' stops at 125");
    assert_eq!(render("pulse", 0.0, &[], &[], 5).len(), 128, "an empty base is all black but still a frame");
    // Time that is not a number is time zero; huge and negative times do not panic.
    for mode in mode_names() {
        assert_eq!(render(mode, f64::NAN, &base, &[], 5), render(mode, 0.0, &base, &[], 5), "{mode}");
        assert_eq!(render(mode, f64::INFINITY, &base, &[], 5), render(mode, 0.0, &base, &[], 5), "{mode}");
        for t in [-1e9, -0.001, 1e12, f64::MAX, f64::MIN] {
            assert_eq!(render(mode, t, &base, &[], 5).len(), 128, "{mode} at {t}");
        }
    }
}

#[test]
fn a_dark_key_still_shows_the_effect_and_a_bright_one_keeps_its_colour() {
    let dark = vec![[10u8, 10, 10]; 128];
    let bright = vec![[200u8, 0, 0]; 128];
    let pulse_dark = render("pulse", 0.25, &dark, &[], 10)[0];
    let pulse_bright = render("pulse", 0.25, &bright, &[], 10)[0];
    assert_eq!(pulse_dark[0], pulse_dark[1], "white scaled: grey");
    assert!(pulse_bright[0] > 0 && pulse_bright[1] == 0 && pulse_bright[2] == 0, "red scaled: still red");
}

// ---- settings --------------------------------------------------------------------------------

struct FakeSink {
    brightness: Mutex<Result<u8, ()>>,
    fail_frames: AtomicUsize,
    frames: Mutex<Vec<(Vec<Color>, Option<i64>)>>,
    reads: AtomicUsize,
    current: Mutex<Vec<Color>>,
}

impl FakeSink {
    fn new() -> Arc<FakeSink> {
        Arc::new(FakeSink {
            brightness: Mutex::new(Ok(25)),
            fail_frames: AtomicUsize::new(0),
            frames: Mutex::default(),
            reads: AtomicUsize::new(0),
            current: Mutex::new(cyan()),
        })
    }
}

impl FrameSink for FakeSink {
    fn brightness(&self) -> Result<u8, KeyboardError> {
        self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.brightness.lock().unwrap().map_err(|()| KeyboardError::NotFound)
    }

    fn per_key_frame(&self, cells: &[Color], brightness: Option<i64>) -> Result<(), KeyboardError> {
        if self.fail_frames.load(std::sync::atomic::Ordering::SeqCst) > 0 {
            self.fail_frames.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            return Err(KeyboardError::NotFound);
        }
        self.frames.lock().unwrap().push((cells.to_vec(), brightness));
        Ok(())
    }

    fn current_cells(&self) -> Vec<Color> {
        self.current.lock().unwrap().clone()
    }
}

fn store_rig(now: f64) -> (TempDir, FxStore, Arc<FakeSink>, Arc<Store>) {
    let dir = TempDir::new("fx");
    let state = Arc::new(Store::new(dir.join("user.json")));
    let sink = FakeSink::new();
    let fx = FxStore::new(Arc::clone(&state), Arc::clone(&sink) as Arc<dyn FrameSink>, Arc::new(move || now));
    (dir, fx, sink, state)
}

#[test]
fn settings_are_stored_in_the_form_the_earlier_editions_wrote() {
    let (_dir, fx, _sink, state) = store_rig(1234.5);
    assert_eq!(fx.get(), Settings::off(), "nothing switched on");
    let got = fx.set("wave", 4, &[21, 20, 20, 5], &[]).unwrap();
    assert_eq!(
        (got.mode.as_str(), got.speed, got.keys.clone(), got.t0),
        ("wave", 4, Some(vec![5, 20, 21]), Some(1234.5))
    );
    assert_eq!(got.base.len(), 128, "the colours now on the keyboard");
    assert_eq!(got.base[0], "00aec7");
    let raw = state.get("perkey_fx").unwrap();
    assert_eq!(raw["mode"], "wave");
    assert_eq!(raw["keys"], json!([5, 20, 21]), "sorted and without repeats");
    assert_eq!(raw["t0"], json!(1234.5));
    assert_eq!(raw["base"].as_array().unwrap().len(), 128);
    let off = fx.set("off", 5, &[], &[]).unwrap();
    assert_eq!(off.mode, "off");
    assert_eq!(state.get("perkey_fx").unwrap()["keys"], Value::Null, "no keys is null, not an empty list");
}

#[test]
fn a_given_base_replaces_the_keyboards_colours() {
    let (_dir, fx, _sink, _state) = store_rig(1.0);
    let got = fx.set("pulse", 5, &[], &[[1, 2, 3], [4, 5, 6]]).unwrap();
    assert_eq!(got.base, ["010203", "040506"]);
    assert_eq!(got.base_colors().unwrap(), [[1, 2, 3], [4, 5, 6]]);
}

#[test]
fn settings_are_validated_with_the_python_messages() {
    let (_dir, fx, _sink, state) = store_rig(1.0);
    assert_eq!(
        fx.set("disco", 5, &[], &[]).unwrap_err().to_string(),
        "animation: off, pulse, wave, cycle, twinkle, rainbow, breathing, scanner, comet, ripple, rain, matrix, fire, plasma, aurora, lightning, confetti, starlight, mood, wave-clockwise, wave-counter-clockwise"
    );
    for speed in [0, 11, 50, -1, i64::MIN, i64::MAX] {
        assert_eq!(fx.set("wave", speed, &[], &[]).unwrap_err().to_string(), "speed 1..10", "{speed}");
    }
    assert_eq!(fx.set("wave", 5, &[3, 128], &[]).unwrap_err().to_string(), "cell 128: 0..127");
    assert!(fx.set("wave", 5, &[-1], &[]).is_err());
    assert_eq!(state.get("perkey_fx"), None, "nothing was stored by a refused request");
    for ok in [1, 10] {
        assert!(fx.set("wave", ok, &[], &[]).is_ok());
    }
}

#[test]
fn old_and_odd_settings_read_as_well_as_they_can() {
    let (_dir, fx, _sink, state) = store_rig(1.0);
    let read = |v: Value| {
        state.set("perkey_fx", v).unwrap();
        fx.get()
    };
    // What the Python edition wrote.
    let python = read(
        json!({"mode": "wave", "speed": 4, "keys": null, "base": ["00aec7", "000000"], "t0": 1700000000.25}),
    );
    assert_eq!(
        (python.keys.clone(), python.base.len(), python.t0, python.speed),
        (None, 2, Some(1_700_000_000.25), 4)
    );
    // Absent fields: the defaults of the Python loop (speed 5, clock at 0).
    let bare = read(json!({"mode": "pulse"}));
    assert_eq!((bare.speed, bare.keys.clone(), bare.base.len(), bare.t0), (5, None, 0, None));
    // Things that are not settings are "off".
    for v in
        [json!(5), json!("wave"), json!([1]), json!(null), json!({}), json!({"mode": 7}), json!({"mode": ""})]
    {
        assert_eq!(read(v.clone()), Settings::off(), "{v}");
    }
    // Wrong types inside are read as absent, never an error.
    let odd = read(json!({"mode": "wave", "speed": "fast", "keys": "w", "base": 5, "t0": "now"}));
    assert_eq!((odd.speed, odd.keys, odd.base, odd.t0), (5, None, vec![], None));
    let mixed =
        read(json!({"mode": "wave", "keys": [1, "x", 3.5, 200, -4, 2], "base": ["00aec7", 5, "ff0000"]}));
    assert_eq!(mixed.cells(), [1, 2], "only cells that exist");
    assert_eq!(mixed.base, ["00aec7", "ff0000"]);
    assert_eq!(
        read(json!({"mode": "wave", "base": ["00aec7", "zz"]})).base_colors(),
        None,
        "one bad entry spoils the list"
    );
}

// ---- the runner ------------------------------------------------------------------------------

struct Rig {
    runner: Runner,
    sink: Arc<FakeSink>,
    settings: Arc<Mutex<Settings>>,
    now: Arc<Mutex<f64>>,
    sleeps: Arc<Mutex<Vec<Duration>>>,
}

fn base_hex(c: Color) -> Vec<String> {
    keymap::canvas(c).iter().map(|c| keymap::hex(*c)).collect()
}

fn on(mode: &str) -> Settings {
    Settings {
        mode: mode.to_owned(),
        speed: 5,
        keys: Some(vec![21, 20]),
        base: base_hex([0x00, 0xAE, 0xC7]),
        t0: Some(1000.0),
    }
}

fn runner_rig() -> Rig {
    let sink = FakeSink::new();
    let settings = Arc::new(Mutex::new(Settings::off()));
    let now = Arc::new(Mutex::new(1000.0));
    let sleeps = Arc::new(Mutex::new(Vec::new()));
    let (s2, n2, z2) = (Arc::clone(&settings), Arc::clone(&now), Arc::clone(&sleeps));
    let runner = Runner::new(
        move || s2.lock().unwrap().clone(),
        Arc::clone(&sink) as Arc<dyn FrameSink>,
        move || *n2.lock().unwrap(),
        move |d| z2.lock().unwrap().push(d),
    );
    Rig { runner, sink, settings, now, sleeps }
}

impl Rig {
    fn set(&self, s: Settings) {
        *self.settings.lock().unwrap() = s;
    }

    fn advance(&self, seconds: f64) {
        *self.now.lock().unwrap() += seconds;
    }

    fn frames(&self) -> usize {
        self.sink.frames.lock().unwrap().len()
    }
}

#[test]
fn the_runner_is_idle_while_nothing_is_on() {
    let mut rig = runner_rig();
    for _ in 0..3 {
        assert_eq!(rig.runner.step(), IDLE_INTERVAL);
    }
    assert_eq!(
        (rig.frames(), rig.sink.reads.load(std::sync::atomic::Ordering::SeqCst)),
        (0, 0),
        "no frames and no keyboard reads"
    );
    rig.set(Settings { base: vec![], ..on("wave") });
    assert_eq!(rig.runner.step(), IDLE_INTERVAL, "an animation without base colours is not drawn");
    rig.set(on("disco"));
    assert_eq!(rig.runner.step(), IDLE_INTERVAL);
    assert_eq!(rig.frames(), 0);
}

#[test]
fn the_runner_draws_at_a_steady_pace_and_does_not_resend_identical_frames() {
    let mut rig = runner_rig();
    rig.set(on("wave"));
    assert_eq!(rig.runner.step(), FRAME_INTERVAL);
    rig.advance(0.2);
    assert_eq!(rig.runner.step(), FRAME_INTERVAL);
    assert_eq!(rig.frames(), 2, "wave moves: two different frames");
    let first = rig.sink.frames.lock().unwrap()[0].clone();
    assert_eq!(first.1, Some(25), "drawn at the keyboard's brightness");
    assert_eq!(first.0, render("wave", 0.0, &cyan(), &[21, 20], 5), "the first frame is at time zero");
    // A pulse of a key set that does not change, at a time that does not change: not sent again.
    rig.set(Settings { keys: Some(vec![0]), ..on("rain") });
    rig.runner.step();
    let before = rig.frames();
    rig.runner.step();
    rig.runner.step();
    assert_eq!(rig.frames(), before, "the same frame at the same time is sent once");
}

#[test]
fn the_brightness_is_read_at_the_start_and_then_every_five_seconds() {
    let mut rig = runner_rig();
    rig.set(on("pulse"));
    let reads = |rig: &Rig| rig.sink.reads.load(std::sync::atomic::Ordering::SeqCst);
    rig.runner.step();
    assert_eq!(reads(&rig), 1);
    for _ in 0..20 {
        rig.advance(0.2);
        rig.runner.step();
    }
    assert_eq!(reads(&rig), 1, "four seconds later: not read again");
    *rig.sink.brightness.lock().unwrap() = Ok(40);
    rig.advance(1.5);
    rig.runner.step();
    assert_eq!(reads(&rig), 2);
    assert_eq!(
        rig.sink.frames.lock().unwrap().last().unwrap().1,
        Some(40),
        "Fn+F11/F12 changes are followed"
    );
}

#[test]
fn an_unreadable_or_dark_keyboard_keeps_the_last_brightness() {
    let mut rig = runner_rig();
    rig.set(on("pulse"));
    *rig.sink.brightness.lock().unwrap() = Ok(33);
    rig.runner.step();
    for (answer, label) in [(Err(()), "unreadable"), (Ok(0), "dark")] {
        *rig.sink.brightness.lock().unwrap() = answer;
        rig.advance(6.0);
        rig.runner.step();
        assert_eq!(
            rig.sink.frames.lock().unwrap().last().unwrap().1,
            Some(33),
            "{label}: never animate into darkness"
        );
    }
    // Before any read worked the default of 25 is used.
    let mut rig = runner_rig();
    *rig.sink.brightness.lock().unwrap() = Err(());
    rig.set(on("pulse"));
    rig.runner.step();
    assert_eq!(rig.sink.frames.lock().unwrap()[0].1, Some(25));
}

#[test]
fn a_failed_upload_backs_off_and_is_retried() {
    let mut rig = runner_rig();
    rig.set(on("pulse"));
    rig.sink.fail_frames.store(1, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(rig.runner.step(), ERROR_BACKOFF + FRAME_INTERVAL);
    assert_eq!(rig.frames(), 0);
    assert_eq!(rig.runner.step(), FRAME_INTERVAL, "the very same frame is sent again after a failure");
    assert_eq!(rig.frames(), 1);
}

#[test]
fn the_base_colours_are_handed_back_once_when_the_animation_is_switched_off() {
    let mut rig = runner_rig();
    rig.set(Settings { base: base_hex([0x11, 0x22, 0x33]), ..on("pulse") });
    rig.runner.step();
    // Like the settings the window stores when an animation is stopped: off, with the base kept.
    rig.set(Settings {
        mode: "off".to_owned(),
        ..Settings { base: base_hex([0x11, 0x22, 0x33]), ..on("pulse") }
    });
    assert_eq!(rig.runner.step(), IDLE_INTERVAL);
    let frames = rig.sink.frames.lock().unwrap().clone();
    assert_eq!(frames.len(), 2);
    assert_eq!(
        frames[1],
        (keymap::canvas([0x11, 0x22, 0x33]), None),
        "the stored base, at the keyboard's own brightness"
    );
    rig.runner.step();
    rig.runner.step();
    assert_eq!(rig.frames(), 2, "handed back once, not on every idle look");
    // An empty base falls back to what is on the keyboard now.
    let mut rig = runner_rig();
    rig.set(on("pulse"));
    rig.runner.step();
    *rig.sink.current.lock().unwrap() = vec![[7, 7, 7]; 128];
    rig.set(Settings { base: vec![], ..on("off") });
    rig.runner.step();
    assert_eq!(rig.sink.frames.lock().unwrap().last().unwrap().0, vec![[7, 7, 7]; 128]);
}

#[test]
fn a_corrupt_base_stops_the_animation_instead_of_painting_wrong_colours() {
    let mut rig = runner_rig();
    rig.set(on("pulse"));
    rig.runner.step();
    rig.set(Settings { base: vec!["00aec7".to_owned(), "zz".to_owned()], ..on("pulse") });
    assert_eq!(rig.runner.step(), IDLE_INTERVAL);
    assert_eq!(
        rig.sink.frames.lock().unwrap().last().unwrap().0,
        cyan(),
        "the keyboard's own colours came back"
    );
}

#[test]
fn run_draws_until_stopped_then_restores_the_base() {
    let stop = Arc::new(AtomicBool::new(false));
    let sink = FakeSink::new();
    let ticks = Arc::new(AtomicUsize::new(0));
    let (t2, s2) = (Arc::clone(&ticks), Arc::clone(&stop));
    let now = Arc::new(Mutex::new(1000.0));
    let n2 = Arc::clone(&now);
    let mut runner = Runner::new(
        || Settings { base: base_hex([0x11, 0x22, 0x33]), keys: None, ..on("wave") },
        Arc::clone(&sink) as Arc<dyn FrameSink>,
        move || *n2.lock().unwrap(),
        move |d| {
            assert_eq!(d, FRAME_INTERVAL);
            *now.lock().unwrap() += 0.2;
            if t2.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 4 {
                s2.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        },
    );
    runner.run(&stop);
    let frames = sink.frames.lock().unwrap().clone();
    assert_eq!(frames.len(), 5 + 1, "five frames, then the base put back so the last frame does not stay");
    assert_eq!(frames[5], (keymap::canvas([0x11, 0x22, 0x33]), None));
    // Stopped while idle: nothing is drawn.
    let stop = AtomicBool::new(true);
    let sink = FakeSink::new();
    Runner::new(Settings::off, Arc::clone(&sink) as Arc<dyn FrameSink>, || 0.0, |_| {}).run(&stop);
    assert!(sink.frames.lock().unwrap().is_empty());
}

#[test]
fn a_clock_that_runs_from_zero_when_no_start_time_is_stored() {
    let mut rig = runner_rig();
    rig.set(Settings { t0: None, ..on("wave") });
    rig.advance(77.0);
    rig.runner.step();
    assert_eq!(rig.sink.frames.lock().unwrap()[0].0, render("wave", 0.0, &cyan(), &[21, 20], 5));
    assert!(rig.sleeps.lock().unwrap().is_empty(), "step itself never sleeps");
}

#[test]
fn the_runner_works_through_the_simulated_keyboard() {
    let dir = TempDir::new("fx-sim");
    let state = Arc::new(Store::new(dir.join("user.json")));
    let (keyboard, sim) = Keyboard::simulated(Arc::clone(&state));
    let keyboard = Arc::new(keyboard);
    keyboard.per_key(&cyan(), Some(18)).unwrap();
    let remembered = state.get("keyboard");
    let fx =
        FxStore::new(Arc::clone(&state), Arc::clone(&keyboard) as Arc<dyn FrameSink>, Arc::new(|| 500.0));
    fx.set("wave", 5, &[21], &[]).unwrap();
    let settings = Arc::new(fx);
    let s2 = Arc::clone(&settings);
    let mut runner =
        Runner::new(move || s2.get(), Arc::clone(&keyboard) as Arc<dyn FrameSink>, || 501.5, |_| {});
    assert_eq!(runner.step(), FRAME_INTERVAL);
    let drawn = sim.last_cells().unwrap();
    assert_eq!(drawn, render("wave", 1.5, &cyan(), &[21], 5));
    assert_eq!(sim.brightness(), 18, "at the user's brightness");
    assert_eq!(state.get("keyboard"), remembered, "an animation frame is not the look");
    settings.set("off", 5, &[], &[]).unwrap();
    runner.step();
    assert_eq!(sim.last_cells().unwrap(), cyan(), "the base came back");
}

#[test]
fn the_errors_say_what_failed() {
    assert_eq!(FxError::Invalid("speed 1..10".into()).to_string(), "speed 1..10");
    let e = FxError::Save(ts_foundation::fsx::FsError::Io {
        op: "rename to",
        path: "/x".into(),
        source: std::io::Error::from_raw_os_error(13),
    });
    assert!(
        e.to_string().starts_with("save animation settings: ") && std::error::Error::source(&e).is_some()
    );
    assert!(epoch_seconds() > 1.7e9);
    assert_eq!(
        (FRAME_INTERVAL, IDLE_INTERVAL, ERROR_BACKOFF),
        (Duration::from_millis(200), Duration::from_millis(500), Duration::from_secs(1))
    );
    assert_eq!(BRIGHTNESS_REFRESH_S, 5.0);
    assert_eq!((SPEED_MIN, SPEED_MAX, DEFAULT_SPEED), (1, 10, 5));
}

#[test]
fn an_animation_follows_the_keyboards_own_brightness_keys_at_once() {
    use std::sync::atomic::{AtomicU64, Ordering};
    let sink = FakeSink::new();
    let now = Arc::new(Mutex::new(1000.0));
    let keys = Arc::new(AtomicU64::new(0));
    let (n2, k2) = (Arc::clone(&now), Arc::clone(&keys));
    let mut runner = Runner::new(
        || Settings { base: base_hex([0x11, 0x22, 0x33]), keys: None, ..on("wave") },
        Arc::clone(&sink) as Arc<dyn FrameSink>,
        move || *n2.lock().unwrap(),
        |_| {},
    )
    .with_hardware_keys(move || k2.load(Ordering::SeqCst));
    let tick = |runner: &mut Runner| {
        runner.step();
        *now.lock().unwrap() += 0.2;
    };
    tick(&mut runner);
    assert_eq!(sink.frames.lock().unwrap().last().map(|f| f.1), Some(Some(25)));
    // Fn+F12 on the keyboard: 40 now. Without the signal the frame would send 25 back.
    *sink.brightness.lock().unwrap() = Ok(40);
    keys.fetch_add(1, Ordering::SeqCst);
    tick(&mut runner);
    assert_eq!(sink.frames.lock().unwrap().last().map(|f| f.1), Some(Some(40)));
    // Dark with the keys: dark.
    *sink.brightness.lock().unwrap() = Ok(0);
    keys.fetch_add(1, Ordering::SeqCst);
    tick(&mut runner);
    assert_eq!(sink.frames.lock().unwrap().last().map(|f| f.1), Some(Some(0)));
    // With nothing pressed, a dark reading from the periodic look is not followed.
    let mut quiet = Runner::new(
        || Settings { base: base_hex([0x11, 0x22, 0x33]), keys: None, ..on("wave") },
        Arc::clone(&sink) as Arc<dyn FrameSink>,
        || 5000.0,
        |_| {},
    )
    .with_hardware_keys(|| 7);
    quiet.step();
    assert_eq!(sink.frames.lock().unwrap().last().map(|f| f.1), Some(Some(25)), "the default, not darkness");
}
