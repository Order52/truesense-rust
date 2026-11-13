//! The renderer on a fake keyboard and a fake clock: when it draws, how often, in what order, and
//! what it does when the keyboard refuses.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;

use ts_domain::keyboard::{self, KeyboardError};
use ts_domain::keymap::KeyMap;
use ts_foundation::userstate::Store;

use super::*;

/// Everything the renderer touches, in one place, so the order of events can be read back.
struct World {
    now: Mutex<f64>,
    /// `upload`, `upload-failed`, `halt`, `replay`, `brightness`, `log: ...`, in order.
    events: Mutex<Vec<String>>,
    uploads: Mutex<Vec<Upload>>,
    active: Mutex<Option<Active>>,
    halts: Mutex<Vec<(f64, String)>>,
    last: Mutex<keyboard::State>,
    replays: AtomicUsize,
    /// What an upload costs in fake time (the real keyboard blocks about 180 ms).
    cost: Mutex<f64>,
    /// The 1-based upload that fails and every one after it (up to `fail_to`, when that is set).
    fail_from: Mutex<Option<usize>>,
    /// The last attempt that still fails: the keyboard is reachable again after it.
    fail_to: Mutex<Option<usize>>,
    /// How a failing upload fails.
    fail_kind: Mutex<Fail>,
    brightness: Mutex<u8>,
    brightness_reads: AtomicUsize,
    sleeps: AtomicUsize,
    attempts: AtomicUsize,
}

/// How an upload fails.
#[derive(Clone, Copy, Default)]
enum Fail {
    /// A real fault (the device answered with an error): the theme must stop.
    #[default]
    Fault,
    /// The interface is not listed yet.
    Missing,
    /// The session has no access to the node yet.
    Denied,
}

#[derive(Clone)]
struct Upload {
    at: f64,
    cells: Vec<Color>,
    brightness: Option<i64>,
}

fn world() -> Arc<World> {
    Arc::new(World {
        now: Mutex::new(1000.0),
        events: Mutex::new(Vec::new()),
        uploads: Mutex::new(Vec::new()),
        active: Mutex::new(None),
        halts: Mutex::new(Vec::new()),
        last: Mutex::new(keyboard::State {
            mode: "effect".into(),
            name: "static".into(),
            color: "112233".into(),
            ..Default::default()
        }),
        replays: AtomicUsize::new(0),
        cost: Mutex::new(0.001),
        fail_from: Mutex::new(None),
        fail_to: Mutex::new(None),
        fail_kind: Mutex::new(Fail::Fault),
        brightness: Mutex::new(30),
        brightness_reads: AtomicUsize::new(0),
        sleeps: AtomicUsize::new(0),
        attempts: AtomicUsize::new(0),
    })
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct Sink(Arc<World>);

impl FrameSink for Sink {
    fn brightness(&self) -> Result<u8, KeyboardError> {
        self.0.brightness_reads.fetch_add(1, Ordering::SeqCst);
        Ok(*lock(&self.0.brightness))
    }

    fn per_key_frame(&self, cells: &[Color], brightness: Option<i64>) -> Result<(), KeyboardError> {
        let n = self.0.attempts.fetch_add(1, Ordering::SeqCst) + 1;
        let to = *lock(&self.0.fail_to);
        if lock(&self.0.fail_from).is_some_and(|from| n >= from) && to.is_none_or(|to| n <= to) {
            lock(&self.0.events).push("upload-failed".into());
            return Err(match *lock(&self.0.fail_kind) {
                Fail::Fault => {
                    KeyboardError::Io { op: "write", source: std::io::Error::other("device error") }
                }
                Fail::Missing => KeyboardError::NotFound,
                Fail::Denied => KeyboardError::Permission { node: "/dev/hidraw8".into() },
            });
        }
        let at = *lock(&self.0.now);
        lock(&self.0.uploads).push(Upload { at, cells: cells.to_vec(), brightness });
        lock(&self.0.events).push("upload".into());
        // The upload blocks for its own time, as the real one does.
        *lock(&self.0.now) += *lock(&self.0.cost);
        Ok(())
    }

    fn current_cells(&self) -> Vec<Color> {
        vec![[0; 3]; 128]
    }
}

struct Kb(Arc<World>);

impl KeyboardControl for Kb {
    fn last(&self) -> keyboard::State {
        lock(&self.0.last).clone()
    }

    fn replay(&self, _: &keyboard::State) -> Result<Option<String>, KeyboardError> {
        self.0.replays.fetch_add(1, Ordering::SeqCst);
        lock(&self.0.events).push("replay".into());
        Ok(Some("effect static".into()))
    }

    fn off(&self) -> Result<(), KeyboardError> {
        Ok(())
    }
}

struct FakeThemes {
    world: Arc<World>,
    /// Only used to resolve the built-in themes: nothing is read from or written to it.
    store: ThemeStore,
}

impl Themes for FakeThemes {
    fn active(&self) -> Option<Active> {
        lock(&self.world.active).clone()
    }

    fn resolve(&self, spec: &ThemeSpec) -> Result<Resolved, String> {
        self.store.resolve(spec).map_err(|e| e.to_string())
    }

    fn halt(&self, started: f64, reason: &str) {
        lock(&self.world.events).push("halt".into());
        lock(&self.world.halts).push((started, reason.to_owned()));
    }
}

fn renderer(w: &Arc<World>, fps: u32) -> Renderer {
    let (a, b, c, d) = (Arc::clone(w), Arc::clone(w), Arc::clone(w), Arc::clone(w));
    Renderer::new(Env {
        themes: Box::new(FakeThemes {
            world: Arc::clone(w),
            store: ThemeStore::beside(
                None,
                Arc::new(Store::new(PathBuf::from("/nonexistent/truesense-test/user.json"))),
            ),
        }),
        sink: Arc::new(Sink(Arc::clone(w))),
        keyboard: Arc::new(Kb(Arc::clone(w))),
        painter: CellPainter::new(KeyMap::verified()),
        now: Box::new(move || *lock(&a.now)),
        local_hours: Box::new(|| Some(12.0)),
        sleep: Box::new(move |d| {
            b.sleeps.fetch_add(1, Ordering::SeqCst);
            *lock(&c.now) += d.as_secs_f64();
        }),
        log: Box::new(move |line| lock(&d.events).push(format!("log: {line}"))),
        fps,
    })
}

/// Starts `name` at the current fake time.
fn start(w: &Arc<World>, spec: ThemeSpec) -> Active {
    let active = Active { spec, started: *lock(&w.now) };
    *lock(&w.active) = Some(active.clone());
    active
}

/// Turns the loop for `seconds` of fake time.
fn run_for(r: &mut Renderer, w: &Arc<World>, seconds: f64) {
    let end = *lock(&w.now) + seconds;
    while *lock(&w.now) < end {
        let pause = r.step();
        (r.env.sleep)(pause);
    }
}

fn events(w: &Arc<World>) -> Vec<String> {
    lock(&w.events).iter().filter(|e| !e.starts_with("log: ")).cloned().collect()
}

fn logs(w: &Arc<World>) -> Vec<String> {
    lock(&w.events).iter().filter_map(|e| e.strip_prefix("log: ").map(str::to_owned)).collect()
}

fn gaps(w: &Arc<World>) -> Vec<f64> {
    let u = lock(&w.uploads);
    u.windows(2).map(|p| p[1].at - p[0].at).collect()
}

/// A theme that animates all the time on the keys.
fn animated() -> ThemeSpec {
    ThemeSpec::new("predator")
}

// ---- idle and start --------------------------------------------------------------------------

#[test]
fn nothing_is_drawn_and_nothing_is_touched_while_no_theme_runs() {
    let w = world();
    let mut r = renderer(&w, DEFAULT_FPS);
    run_for(&mut r, &w, 30.0);
    assert!(lock(&w.uploads).is_empty());
    assert_eq!(w.replays.load(Ordering::SeqCst), 0);
    assert_eq!(w.brightness_reads.load(Ordering::SeqCst), 0);
    assert!(lock(&w.halts).is_empty());
    // It sleeps between looks at the record (about two a second), it does not spin.
    assert!(w.sleeps.load(Ordering::SeqCst) <= 62, "{}", w.sleeps.load(Ordering::SeqCst));
    assert_eq!(r.step(), IDLE);
}

#[test]
fn a_started_theme_is_drawn_as_a_full_cell_picture_at_the_keyboards_brightness() {
    let w = world();
    let mut r = renderer(&w, DEFAULT_FPS);
    start(&w, animated());
    run_for(&mut r, &w, 1.0);
    let uploads = lock(&w.uploads).clone();
    assert!(!uploads.is_empty());
    let first = &uploads[0];
    assert_eq!(first.cells.len(), 128);
    assert!(first.cells.iter().any(|c| *c != [0, 0, 0]), "the theme lights keys");
    assert_eq!(first.brightness, Some(30), "the level the keyboard reports");
    assert!(logs(&w).iter().any(|l| l.contains("theme predator: started")));
    // The two cells nothing is mapped to stay dark.
    assert!(uploads.iter().all(|u| u.cells[126] == [0, 0, 0] && u.cells[127] == [0, 0, 0]));
}

// ---- rate and back-pressure ------------------------------------------------------------------

#[test]
fn the_frame_rate_is_capped() {
    let w = world();
    let mut r = renderer(&w, 15);
    start(&w, animated());
    run_for(&mut r, &w, 4.0);
    let n = lock(&w.uploads).len();
    assert!((50..=61).contains(&n), "{n} frames in 4 s at a cap of 15 a second");
    let spacing = 1.0 / 15.0 - 0.001;
    assert!(gaps(&w).iter().all(|g| *g >= spacing), "{:?}", gaps(&w));
    // The pictures actually move: a theme that stood still would pass the count above too.
    let u = lock(&w.uploads);
    assert!(u.windows(2).any(|p| p[0].cells != p[1].cells));
}

#[test]
fn the_cap_itself_is_held_between_one_and_the_most_the_keyboard_could_take() {
    let w = world();
    let mut slow = renderer(&w, 0);
    start(&w, animated());
    run_for(&mut slow, &w, 5.0);
    let n = lock(&w.uploads).len();
    assert!((4..=6).contains(&n), "{n} frames in 5 s at 1 a second");
    let w = world();
    let mut fast = renderer(&w, 10_000);
    start(&w, animated());
    run_for(&mut fast, &w, 2.0);
    let n = lock(&w.uploads).len();
    assert!(n <= 2 * MAX_FPS as usize + 1, "{n} frames in 2 s");
}

#[test]
fn a_slow_upload_drops_frames_instead_of_queueing_them() {
    let w = world();
    *lock(&w.cost) = 0.18;
    let mut r = renderer(&w, 15);
    start(&w, animated());
    run_for(&mut r, &w, 3.6);
    let n = lock(&w.uploads).len();
    assert!((18..=21).contains(&n), "{n} uploads in 3.6 s at 180 ms each");
    // Never two uploads closer than the upload takes (nothing is sent in a burst to catch up) and
    // never a long hole either (the loop does not wait on top of the upload).
    for g in gaps(&w) {
        assert!((0.18..0.20).contains(&g), "gap {g}");
    }
    // The theme's own clock followed real time (about 3.6 s), not the number of frames sent.
    let State::Run(run) = &r.state else { panic!("not running") };
    assert!((3.3..=3.7).contains(&run.t), "t = {}", run.t);
}

#[test]
fn a_theme_may_be_played_slower_and_its_clock_follows() {
    let w = world();
    let mut r = renderer(&w, 15);
    start(&w, ThemeSpec { speed: Some(50), ..animated() });
    run_for(&mut r, &w, 4.0);
    let State::Run(run) = &r.state else { panic!("not running") };
    assert!((1.8..=2.1).contains(&run.t), "t = {}", run.t);
}

#[test]
fn a_long_stall_does_not_make_the_theme_jump() {
    let w = world();
    let mut r = renderer(&w, 15);
    start(&w, animated());
    run_for(&mut r, &w, 1.0);
    let before = match &r.state {
        State::Run(run) => run.t,
        _ => panic!("not running"),
    };
    // The machine slept for an hour.
    *lock(&w.now) += 3600.0;
    r.step();
    let State::Run(run) = &r.state else { panic!("not running") };
    assert!(run.t - before <= MAX_STEP + 1e-9, "jumped {}", run.t - before);
}

// ---- brightness ------------------------------------------------------------------------------

#[test]
fn a_brightness_in_the_theme_wins_and_the_keyboard_is_not_asked() {
    let w = world();
    let mut r = renderer(&w, 15);
    start(&w, ThemeSpec { brightness: Some(40), ..animated() });
    run_for(&mut r, &w, 2.0);
    assert!(lock(&w.uploads).iter().all(|u| u.brightness == Some(40)));
    assert_eq!(w.brightness_reads.load(Ordering::SeqCst), 0);
}

#[test]
fn the_keyboards_level_is_followed_every_few_seconds_and_darkness_is_never_adopted() {
    let w = world();
    let mut r = renderer(&w, 15);
    start(&w, animated());
    run_for(&mut r, &w, 2.0);
    assert_eq!(w.brightness_reads.load(Ordering::SeqCst), 1, "one look in two seconds");
    *lock(&w.brightness) = 12;
    run_for(&mut r, &w, 6.0);
    assert_eq!(lock(&w.uploads).last().unwrap().brightness, Some(12), "Fn+F11 is followed");
    *lock(&w.brightness) = 0;
    run_for(&mut r, &w, 6.0);
    assert_eq!(lock(&w.uploads).last().unwrap().brightness, Some(12), "a dark reading is not drawn into");
}

// ---- stopping --------------------------------------------------------------------------------

#[test]
fn stopping_the_theme_puts_the_static_look_back_once_and_draws_no_more() {
    let w = world();
    let mut r = renderer(&w, 15);
    start(&w, animated());
    run_for(&mut r, &w, 2.0);
    let drawn = lock(&w.uploads).len();
    *lock(&w.active) = None;
    run_for(&mut r, &w, 5.0);
    assert_eq!(w.replays.load(Ordering::SeqCst), 1);
    // The stop is noticed at the next look at the record, within half a second.
    let after = lock(&w.uploads).len();
    assert!(after - drawn <= 8, "{} frames after the stop", after - drawn);
    let ev = events(&w);
    assert_eq!(ev.last().map(String::as_str), Some("replay"), "the static look is the last thing written");
    assert!(lock(&w.halts).is_empty(), "a stop is not a halt");
    // Idle again, and the next theme starts cleanly.
    start(&w, animated());
    run_for(&mut r, &w, 1.0);
    assert!(lock(&w.uploads).len() > drawn);
}

#[test]
fn the_stop_flag_takes_a_running_theme_off_the_keyboard() {
    let w = world();
    let mut r = renderer(&w, 15);
    start(&w, animated());
    let stop = Arc::new(AtomicBool::new(false));
    // The sleep after the fifth turn raises the flag, as the service's stop signal would.
    let (turns, flag, clock) = (Arc::new(AtomicUsize::new(0)), Arc::clone(&stop), Arc::clone(&w));
    r.env.sleep = Box::new(move |d| {
        *lock(&clock.now) += d.as_secs_f64();
        if turns.fetch_add(1, Ordering::SeqCst) == 4 {
            flag.store(true, Ordering::SeqCst);
        }
    });
    r.run(&stop);
    assert!(!lock(&w.uploads).is_empty());
    assert_eq!(w.replays.load(Ordering::SeqCst), 1);
    assert_eq!(events(&w).last().map(String::as_str), Some("replay"));
    assert!(logs(&w).iter().any(|l| l.contains("the key service stopped")));
}

#[test]
fn a_new_record_restarts_the_theme_and_another_theme_replaces_it_without_a_handback() {
    let w = world();
    let mut r = renderer(&w, 15);
    start(&w, animated());
    run_for(&mut r, &w, 1.0);
    *lock(&w.now) += 1.0;
    start(&w, ThemeSpec::new("hearth"));
    run_for(&mut r, &w, 1.0);
    let started = logs(&w).into_iter().filter(|l| l.contains(": started")).collect::<Vec<_>>();
    assert_eq!(started.len(), 2, "{started:?}");
    assert!(started[1].contains("hearth"));
    assert_eq!(w.replays.load(Ordering::SeqCst), 0, "the new theme draws over the old one");
    let State::Run(run) = &r.state else { panic!("not running") };
    assert!(run.t < 1.5, "the new theme's clock started again: {}", run.t);
}

// ---- fail closed -----------------------------------------------------------------------------

#[test]
fn a_refused_frame_stops_the_theme_restores_the_look_and_never_retries() {
    let w = world();
    *lock(&w.fail_from) = Some(3);
    let mut r = renderer(&w, 15);
    let active = start(&w, animated());
    run_for(&mut r, &w, 60.0);
    // Two frames went out, the third was refused, and nothing was tried after it.
    assert_eq!(lock(&w.uploads).len(), 2);
    assert_eq!(w.attempts.load(Ordering::SeqCst), 3);
    assert_eq!(w.replays.load(Ordering::SeqCst), 1);
    let ev = events(&w);
    let tail: Vec<&str> = ev.iter().rev().take(3).rev().map(String::as_str).collect();
    assert_eq!(tail, ["upload-failed", "halt", "replay"], "{ev:?}");
    let halts = lock(&w.halts).clone();
    assert_eq!(halts.len(), 1);
    assert_eq!(halts[0].0, active.started);
    assert!(halts[0].1.contains("refused a frame"), "{}", halts[0].1);
    // It did not spin: a minute of fake time took about two turns a second.
    assert!(w.sleeps.load(Ordering::SeqCst) <= 125, "{}", w.sleeps.load(Ordering::SeqCst));
}

#[test]
fn a_keyboard_that_is_not_reachable_yet_is_waited_for_and_the_theme_then_runs() {
    // The login race: the key service starts before the session's access to the keyboard is in
    // place, so the first uploads are refused for a few seconds.
    for kind in [Fail::Denied, Fail::Missing] {
        let w = world();
        *lock(&w.fail_from) = Some(1);
        *lock(&w.fail_to) = Some(5);
        *lock(&w.fail_kind) = kind;
        let mut r = renderer(&w, 15);
        start(&w, animated());
        run_for(&mut r, &w, 10.0);
        assert!(lock(&w.halts).is_empty(), "the theme was not given up");
        assert_eq!(w.replays.load(Ordering::SeqCst), 0, "the static look was not put back over it");
        assert!(matches!(r.state, State::Run(_)));
        assert!(lock(&w.uploads).len() > 20, "it is drawing now");
        let log = logs(&w);
        assert_eq!(log.iter().filter(|l| l.contains("waiting for the keyboard")).count(), 1, "{log:?}");
        assert_eq!(log.iter().filter(|l| l.contains("the keyboard is reachable")).count(), 1, "{log:?}");
        // It waited about a second a time, it did not spin.
        assert_eq!(w.attempts.load(Ordering::SeqCst) - lock(&w.uploads).len(), 5);
    }
}

#[test]
fn a_keyboard_that_never_comes_back_ends_the_wait_after_a_minute() {
    let w = world();
    *lock(&w.fail_from) = Some(1);
    *lock(&w.fail_kind) = Fail::Denied;
    let mut r = renderer(&w, 15);
    let active = start(&w, animated());
    run_for(&mut r, &w, 120.0);
    let halts = lock(&w.halts).clone();
    assert_eq!(halts.len(), 1);
    assert_eq!(halts[0].0, active.started);
    assert!(halts[0].1.contains("refused a frame") && halts[0].1.contains("no permission"), "{}", halts[0].1);
    assert_eq!(w.replays.load(Ordering::SeqCst), 1);
    let attempts = w.attempts.load(Ordering::SeqCst);
    assert!((58..=64).contains(&attempts), "about one try a second for a minute, not {attempts}");
}

#[test]
fn a_halted_theme_runs_again_when_it_is_started_again() {
    let w = world();
    *lock(&w.fail_from) = Some(2);
    let mut r = renderer(&w, 15);
    start(&w, animated());
    run_for(&mut r, &w, 5.0);
    assert_eq!(lock(&w.uploads).len(), 1);
    // The keyboard is back, and the user starts the theme again.
    *lock(&w.fail_from) = None;
    *lock(&w.now) += 1.0;
    start(&w, animated());
    run_for(&mut r, &w, 2.0);
    assert!(lock(&w.uploads).len() > 10);
    assert_eq!(w.replays.load(Ordering::SeqCst), 1, "only the failure handed the look back");
}

#[test]
fn a_halted_theme_that_is_then_stopped_is_forgotten() {
    let w = world();
    *lock(&w.fail_from) = Some(1);
    let mut r = renderer(&w, 15);
    start(&w, animated());
    run_for(&mut r, &w, 2.0);
    assert!(matches!(r.state, State::Halted(_)));
    *lock(&w.active) = None;
    run_for(&mut r, &w, 2.0);
    assert!(matches!(r.state, State::Idle));
    assert_eq!(w.replays.load(Ordering::SeqCst), 1, "no second hand-back for a stop of a halted theme");
}

#[test]
fn a_theme_that_cannot_be_resolved_is_halted_once_with_the_reason_and_never_drawn() {
    let w = world();
    let mut r = renderer(&w, 15);
    let storm = start(&w, ThemeSpec::new("storm"));
    run_for(&mut r, &w, 20.0);
    assert!(lock(&w.uploads).is_empty());
    let halts = lock(&w.halts).clone();
    assert_eq!(halts.len(), 1, "{halts:?}");
    assert_eq!(halts[0].0, storm.started);
    assert!(halts[0].1.contains("flashing has not been measured"), "{}", halts[0].1);
    assert_eq!(w.replays.load(Ordering::SeqCst), 0, "nothing was drawn, so nothing to hand back");
    // A name nobody has, and a setting the theme does not have, end the same way.
    *lock(&w.now) += 1.0;
    start(&w, ThemeSpec::new("nobody"));
    run_for(&mut r, &w, 2.0);
    *lock(&w.now) += 1.0;
    start(&w, ThemeSpec { params: [("nope".to_owned(), 1.into())].into_iter().collect(), ..animated() });
    run_for(&mut r, &w, 2.0);
    assert_eq!(lock(&w.halts).len(), 3);
    assert!(lock(&w.uploads).is_empty());
}

// ---- not against other commands --------------------------------------------------------------

#[test]
fn a_look_set_by_another_command_ends_the_theme_without_being_overwritten() {
    let w = world();
    let mut r = renderer(&w, 15);
    start(&w, animated());
    run_for(&mut r, &w, 2.0);
    let drawn = lock(&w.uploads).len();
    lock(&w.last).color = "ff0000".into();
    run_for(&mut r, &w, 5.0);
    let after = lock(&w.uploads).len();
    // Noticed at the next look at the record (within half a second): a handful of frames at most.
    assert!(after - drawn <= 8, "{} frames after the change", after - drawn);
    assert_eq!(w.replays.load(Ordering::SeqCst), 0, "the user's look is not replaced by the old one");
    let halts = lock(&w.halts).clone();
    assert_eq!(halts.len(), 1);
    assert!(halts[0].1.contains("changed by another command"));
    // Nothing more is drawn afterwards.
    let settled = lock(&w.uploads).len();
    run_for(&mut r, &w, 10.0);
    assert_eq!(lock(&w.uploads).len(), settled);
}

// ---- identical frames ------------------------------------------------------------------------

#[test]
fn a_picture_that_does_not_change_is_sent_once() {
    // A theme whose keyboard look is a still colour: find one in the catalogue by looking at its
    // frames, so the test says so loudly if the catalogue ever has none.
    let still = ts_fx::themes::catalogue()
        .iter()
        .filter(|t| t.id != "storm")
        .find(|t| {
            let Some(mut fx) = t.keyboard_effect(&Default::default()) else { return false };
            let a = fx.frame(0.0, 0.0, &Input::default()).to_vec();
            (1..40).all(|i| fx.frame(f64::from(i) * 0.25, 0.25, &Input::default()) == a.as_slice())
        })
        .map(|t| t.id.clone())
        .expect("the catalogue has a theme whose keyboard is a still picture");
    let w = world();
    let mut r = renderer(&w, 15);
    start(&w, ThemeSpec::new(still));
    run_for(&mut r, &w, 6.0);
    assert_eq!(lock(&w.uploads).len(), 1);
}
