//! Tests of the key service on a scripted report stream and a fake clock: every gesture, the
//! timing edges, the config reload, the device going away and coming back, actions that fail or
//! panic or hang, systemd's notifications, and reports that make no sense.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::VecDeque;
use std::io;
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::AtomicU32;

use ts_api::api::testing::RecordingNotifier;

use super::*;
use crate::fnkeys::HidrawFile;

const PRESS: [u8; 3] = [0x04, 0x81, 0xff];
const RELEASE: [u8; 3] = [0x04, 0x00, 0x00];
const FN_F10: [u8; 3] = [0x04, 0x80, 0xff];
const FN_ESC: [u8; 3] = [0x04, 0x8a, 0xff];
const NODE: &str = "/dev/hidraw7";

type Parked = Arc<Mutex<Vec<Box<dyn FnOnce() + Send>>>>;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn poison_free<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

// ---- the scripted world --------------------------------------------------------------------

/// What happens at a time.
#[derive(Clone)]
enum Ev {
    Report(Vec<u8>),
    /// The keyboard appears (`true`) or goes away (`false`).
    Present(bool),
    /// A read fails with an I/O error (not a hang-up).
    ReadError,
    /// The config file changes: a new stamp and what loading it gives.
    Config(Option<u128>, Result<Bindings, String>),
}

/// The keyboard, the clock and the config file in one script. Reads and sleeps move the fake
/// clock; events fall due as it passes them, in order.
struct World {
    now: Mutex<Duration>,
    events: Mutex<VecDeque<(Duration, Ev)>>,
    present: AtomicBool,
    end: Duration,
    stop: Arc<AtomicBool>,
    config: Mutex<(Option<u128>, Result<Bindings, String>)>,
    opens: AtomicUsize,
    actions: Mutex<Vec<(u64, String)>>,
    log: Mutex<Vec<String>>,
}

impl World {
    fn new(end_ms: u64, bindings: Bindings) -> Arc<World> {
        Arc::new(World {
            now: Mutex::new(Duration::ZERO),
            events: Mutex::new(VecDeque::new()),
            present: AtomicBool::new(true),
            end: ms(end_ms),
            stop: Arc::new(AtomicBool::new(false)),
            config: Mutex::new((Some(1), Ok(bindings))),
            opens: AtomicUsize::new(0),
            actions: Mutex::new(Vec::new()),
            log: Mutex::new(Vec::new()),
        })
    }

    fn at(&self, at_ms: u64, ev: Ev) -> &Self {
        let mut events = poison_free(&self.events);
        events.push_back((ms(at_ms), ev));
        events.make_contiguous().sort_by_key(|(at, _)| *at);
        self
    }

    fn report(&self, at_ms: u64, bytes: &[u8]) -> &Self {
        self.at(at_ms, Ev::Report(bytes.to_vec()))
    }

    fn set_time(&self, to: Duration) {
        let mut now = poison_free(&self.now);
        *now = (*now).max(to);
    }

    fn time(&self) -> Duration {
        *poison_free(&self.now)
    }

    fn check_end(&self) {
        let drained = poison_free(&self.events).is_empty();
        if (drained && self.time() >= self.end) || self.time() > self.end + Duration::from_secs(120) {
            self.stop.store(true, Ordering::SeqCst);
        }
    }

    /// Lets time pass for up to `timeout`; `reading` says a stream is open (reports are delivered
    /// and a hang-up is seen), otherwise reports fall on the floor, as with no keyboard.
    fn pump(&self, timeout: Duration, reading: bool) -> Result<Option<Vec<u8>>, HidError> {
        let limit = self.time().saturating_add(timeout);
        loop {
            let next = poison_free(&self.events).front().cloned();
            match next {
                Some((at, ev)) if at <= limit => {
                    poison_free(&self.events).pop_front();
                    self.set_time(at);
                    match ev {
                        Ev::Report(r) => {
                            if reading && self.present.load(Ordering::SeqCst) {
                                return Ok(Some(r));
                            }
                        }
                        Ev::Present(on) => {
                            self.present.store(on, Ordering::SeqCst);
                            if !on && reading {
                                return Err(HidError::Gone);
                            }
                        }
                        Ev::ReadError => {
                            if reading {
                                return Err(HidError::Read(io::Error::other("boom")));
                            }
                        }
                        Ev::Config(stamp, loaded) => *poison_free(&self.config) = (stamp, loaded),
                    }
                }
                _ => {
                    self.set_time(limit);
                    self.check_end();
                    return Ok(None);
                }
            }
        }
    }
}

impl Clock for World {
    fn now(&self) -> Duration {
        self.time()
    }

    fn sleep(&self, how_long: Duration) {
        let _ = self.pump(how_long, false);
    }
}

struct Connection(Arc<World>);

impl HidReports for Connection {
    fn read_report(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, HidError> {
        self.0.pump(timeout, true)
    }
}

struct Opener(Arc<World>);

impl HidOpener for Opener {
    fn open(&self, node: &Path) -> Result<Box<dyn HidReports>, HidError> {
        if !self.0.present.load(Ordering::SeqCst) {
            return Err(HidError::Open { node: node.to_owned(), source: io::Error::from_raw_os_error(19) });
        }
        self.0.opens.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Connection(Arc::clone(&self.0))))
    }
}

struct Source(Arc<World>);

impl BindingSource for Source {
    fn stamp(&self) -> Option<u128> {
        poison_free(&self.0.config).0
    }

    fn load(&self) -> Result<Bindings, String> {
        poison_free(&self.0.config).1.clone()
    }
}

/// Records every action with the fake time it ran at; the spec names what it does.
struct Actions(Arc<World>);

impl KeyActions for Actions {
    fn run(&self, spec: &str) -> Result<String, String> {
        poison_free(&self.0.actions)
            .push((u64::try_from(self.0.time().as_millis()).unwrap_or(u64::MAX), spec.to_owned()));
        match spec {
            "fail" => Err("it broke".to_owned()),
            "panic" => panic!("the action panicked"),
            "say" => Ok("Hello".to_owned()),
            _ => Ok(String::new()),
        }
    }
}

fn bindings() -> Bindings {
    Bindings {
        tap: "A_tap".into(),
        double: "A_double".into(),
        triple: "A_triple".into(),
        long: "A_long".into(),
        fn_esc: "A_esc".into(),
        long_ms: 250,
        multi_ms: 100,
    }
}

struct Rig {
    world: Arc<World>,
    screen: Arc<RecordingNotifier>,
}

impl Rig {
    fn new(end_ms: u64) -> Rig {
        Rig { world: World::new(end_ms, bindings()), screen: Arc::new(RecordingNotifier::default()) }
    }

    fn env(&self) -> Env {
        let world = Arc::clone(&self.world);
        let log_world = Arc::clone(&self.world);
        let finder_world = Arc::clone(&self.world);
        Env {
            find_node: Arc::new(move || {
                finder_world.present.load(Ordering::SeqCst).then(|| PathBuf::from(NODE))
            }),
            opener: Arc::new(Opener(Arc::clone(&self.world))),
            bindings: Arc::new(Source(Arc::clone(&world))),
            check: Arc::new(
                |spec| if spec.starts_with("bad") { Err("not an action".to_owned()) } else { Ok(()) },
            ),
            actions: Arc::new(Actions(Arc::clone(&world))),
            screen: Arc::clone(&self.screen) as Arc<dyn Screen>,
            clock: Arc::clone(&self.world) as Arc<dyn Clock>,
            spawn: Arc::new(|job| {
                job();
                Ok(())
            }),
            log: Arc::new(move |line| poison_free(&log_world.log).push(line.to_owned())),
            systemd: Systemd::disabled(),
            watchdog: None,
            timing: LoopTiming::default(),
            background: Vec::new(),
            watch: None,
        }
    }

    fn run(&self) -> Vec<(u64, String)> {
        self.run_with(self.env())
    }

    fn run_with(&self, env: Env) -> Vec<(u64, String)> {
        Service::new(env).run(&self.world.stop);
        poison_free(&self.world.actions).clone()
    }

    fn log(&self) -> Vec<String> {
        poison_free(&self.world.log).clone()
    }

    fn log_has(&self, needle: &str) -> bool {
        self.log().iter().any(|l| l.contains(needle))
    }
}

fn specs(actions: &[(u64, String)]) -> Vec<&str> {
    actions.iter().map(|(_, s)| s.as_str()).collect()
}

// ---- the gesture machine -------------------------------------------------------------------

mod machine {
    use super::*;

    /// A step of a timeline; `at` in milliseconds.
    #[derive(Clone, Copy)]
    struct Step {
        at: u64,
        what: &'static str,
    }

    fn s(at: u64, what: &'static str) -> Step {
        Step { at, what }
    }

    /// Plays a timeline the way the loop does: deadlines before each event fire first, then the
    /// event; after the last event time runs on for every deadline.
    fn play(g: &mut Gestures, steps: &[Step]) -> Vec<Gesture> {
        let mut out = Vec::new();
        for step in steps {
            while let Some(due) = g.deadline().filter(|d| *d <= ms(step.at)) {
                out.extend(g.advance(due));
            }
            let now = ms(step.at);
            match step.what {
                "down" => out.extend(g.press(now)),
                "up" => out.extend(g.release(now)),
                _ => out.extend(g.advance(now)),
            }
        }
        while let Some(due) = g.deadline() {
            out.extend(g.advance(due));
        }
        out
    }

    fn tap(from: u64) -> Vec<Step> {
        vec![s(from, "down"), s(from + 30, "up")]
    }

    fn cat(parts: &[Vec<Step>]) -> Vec<Step> {
        parts.iter().flatten().copied().collect()
    }

    #[test]
    fn timelines() {
        use Gesture::{Double, Long, Tap, Triple};
        // the timings of tests/test_core.py::TestGestures: long 250 ms, multi 100 ms
        let cases: Vec<(&str, Vec<Step>, Vec<Gesture>)> = vec![
            ("tap", tap(0), vec![Tap]),
            ("double", cat(&[tap(0), tap(50)]), vec![Double]),
            ("triple", cat(&[tap(0), tap(50), tap(100)]), vec![Triple]),
            ("long is not also a tap", vec![s(0, "down"), s(500, "up")], vec![Long]),
            ("two taps too far apart", cat(&[tap(0), tap(280)]), vec![Tap, Tap]),
            ("long then tap", cat(&[vec![s(0, "down"), s(400, "up")], tap(430)]), vec![Long, Tap]),
            (
                "four quick taps are a triple and a tap",
                cat(&[tap(0), tap(50), tap(100), tap(150)]),
                vec![Triple, Tap],
            ),
            ("release without a press is ignored", vec![s(0, "up"), s(20, "up")], vec![]),
            (
                "a stray release before a tap changes nothing",
                cat(&[vec![s(0, "up"), s(20, "up")], tap(40)]),
                vec![Tap],
            ),
            (
                "a tap then a long press: the tap is dropped",
                cat(&[tap(0), vec![s(60, "down"), s(400, "up")]]),
                vec![Long],
            ),
            (
                "a press inside the window keeps counting",
                cat(&[tap(0), vec![s(100, "down"), s(120, "up")]]),
                vec![Double],
            ),
            ("released exactly at the threshold is long", vec![s(0, "down"), s(250, "up")], vec![Long]),
            ("released one ms before the threshold is a tap", vec![s(0, "down"), s(249, "up")], vec![Tap]),
            (
                "a repeated down does not restart or double the long press",
                vec![s(0, "down"), s(100, "down"), s(600, "up")],
                vec![Long],
            ),
            (
                "a repeated down after the long press adds no tap",
                vec![s(0, "down"), s(300, "down"), s(600, "up")],
                vec![Long],
            ),
            ("nothing at all", vec![], vec![]),
            (
                "two long presses",
                vec![s(0, "down"), s(400, "up"), s(500, "down"), s(900, "up")],
                vec![Long, Long],
            ),
            ("a press one ms before the window closes still counts", cat(&[tap(0), tap(129)]), vec![Double]),
            (
                "a press exactly when the window closes is a new gesture",
                cat(&[tap(0), tap(130)]),
                vec![Tap, Tap],
            ),
        ];
        for (name, steps, want) in cases {
            let mut g = Gestures::new(ms(250), ms(100));
            assert_eq!(play(&mut g, &steps), want, "{name}");
        }
    }

    #[test]
    fn a_zero_window_makes_every_tap_final_at_its_release() {
        let mut g = Gestures::new(ms(800), Duration::ZERO);
        g.press(ms(0));
        assert!(g.advance(ms(100)).is_empty(), "nothing before the key comes up");
        assert_eq!(g.release(ms(120)), [Gesture::Tap], "the tap is answered by the release itself");
        assert_eq!(g.deadline(), None, "and nothing is left waiting");
        // Two quick presses are two taps, not a double.
        g.press(ms(200));
        assert_eq!(g.release(ms(230)), [Gesture::Tap]);
        g.press(ms(260));
        assert_eq!(g.release(ms(290)), [Gesture::Tap]);
        // A long press is still a long press and its release is silent.
        g.press(ms(1000));
        assert_eq!(g.advance(ms(1800)), [Gesture::Long]);
        assert!(g.release(ms(2000)).is_empty());
    }

    #[test]
    fn deadlines_are_exact() {
        let mut g = Gestures::new(ms(800), ms(450));
        assert_eq!(g.deadline(), None, "an idle machine has none");
        g.press(ms(0));
        assert_eq!(g.deadline(), Some(ms(800)));
        assert!(g.advance(ms(799)).is_empty());
        g.release(ms(100));
        assert_eq!(g.deadline(), Some(ms(550)), "a tap starts the multi window");
        assert!(g.advance(ms(549)).is_empty());
        assert_eq!(g.advance(ms(550)), [Gesture::Tap]);
        assert_eq!(g.deadline(), None);
        assert!(g.advance(ms(10_000)).is_empty(), "a resolved gesture fires once");
    }

    #[test]
    fn long_fires_while_still_held() {
        let mut g = Gestures::new(ms(800), ms(450));
        g.press(ms(0));
        assert_eq!(g.advance(ms(800)), [Gesture::Long]);
        assert!(g.release(ms(5000)).is_empty(), "the release after a long press is silent");
        assert!(g.advance(ms(9000)).is_empty());
    }

    #[test]
    fn an_event_resolves_due_deadlines_first() {
        let mut g = Gestures::new(ms(250), ms(100));
        g.press(ms(0));
        g.release(ms(10));
        assert_eq!(g.press(ms(500)), [Gesture::Tap], "the tap's window closed at 110");
        assert_eq!(g.release(ms(1000)), [Gesture::Long], "held past 250: long, then the release is silent");
    }

    #[test]
    fn new_timing_applies_to_later_presses() {
        let mut g = Gestures::new(ms(800), ms(450));
        g.press(ms(0));
        g.set_timing(ms(300), ms(200));
        assert_eq!(g.deadline(), Some(ms(800)), "a running deadline keeps its time");
        g.release(ms(50));
        assert_eq!(g.deadline(), Some(ms(250)));
        g.advance(ms(250));
        g.press(ms(1000));
        assert_eq!(g.deadline(), Some(ms(1300)));
    }

    #[test]
    fn a_press_lost_with_the_keyboard_is_dropped_and_counted_taps_still_resolve() {
        let mut g = Gestures::new(ms(250), ms(100));
        g.press(ms(0));
        g.abandon_press(ms(50));
        assert_eq!(g.deadline(), None, "nothing is left running for a press nobody will release");
        assert!(g.release(ms(60)).is_empty(), "a release after the return belongs to nothing");

        let mut g = Gestures::new(ms(250), ms(100));
        g.press(ms(0));
        g.release(ms(20));
        g.press(ms(60)); // the second press of a double ...
        g.abandon_press(ms(70)); // ... lost with the keyboard
        assert_eq!(g.advance(ms(170)), [Gesture::Tap], "the one counted tap resolves");

        let mut g = Gestures::new(ms(250), ms(100));
        g.press(ms(0));
        g.advance(ms(250));
        g.abandon_press(ms(300));
        assert!(g.advance(ms(10_000)).is_empty());
        g.abandon_press(ms(400)); // nothing down: nothing happens
    }

    #[test]
    fn extreme_timings_do_not_overflow() {
        let mut g = Gestures::new(Duration::MAX, Duration::MAX);
        g.press(Duration::MAX);
        assert!(g.release(Duration::MAX).len() <= 1);
        assert!(g.advance(Duration::MAX).len() <= 1);
        let mut g = Gestures::new(Duration::ZERO, Duration::ZERO);
        g.press(ms(5));
        assert_eq!(g.advance(ms(5)), [Gesture::Long], "a zero threshold is an instant long press");
    }

    #[test]
    fn any_sequence_ends_drained_and_yields_only_the_four_gestures() {
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..300 {
            let mut g = Gestures::new(ms(250), ms(100));
            let mut now = Duration::ZERO;
            let mut fired = Vec::new();
            for _ in 0..40 {
                let r = next();
                now += ms((r % 7) * 60);
                match (r >> 8) % 3 {
                    0 => fired.extend(g.press(now)),
                    1 => fired.extend(g.release(now)),
                    _ => fired.extend(g.advance(now)),
                }
                if r % 11 == 0 {
                    g.abandon_press(now);
                }
            }
            while let Some(due) = g.deadline() {
                fired.extend(g.advance(due));
            }
            assert_eq!(g.deadline(), None);
            assert!(
                fired
                    .iter()
                    .all(|g| matches!(g, Gesture::Tap | Gesture::Double | Gesture::Triple | Gesture::Long))
            );
        }
    }
}

// ---- finding the node ----------------------------------------------------------------------

mod node {
    use super::*;
    use crate::fnkeys::find_node_in;

    struct Tree(PathBuf);

    impl Tree {
        fn new() -> Tree {
            static N: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "ts-keys-node-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(dir.join("class/hidraw")).expect("tree");
            Tree(dir)
        }

        fn node(&self, name: &str, uevent: &str, descriptor: &[u8]) {
            let device = self.0.join("class/hidraw").join(name).join("device");
            std::fs::create_dir_all(&device).expect("device");
            std::fs::write(device.join("uevent"), uevent).expect("uevent");
            std::fs::write(device.join("report_descriptor"), descriptor).expect("descriptor");
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const KEYBOARD: &str = "DRIVER=hid-generic\nHID_ID=0003:000004F2:00000117\nHID_NAME=CHICONY\n";

    #[test]
    fn finds_the_vendor_page_of_the_keyboard() {
        let t = Tree::new();
        t.node("hidraw0", KEYBOARD, &[0x05, 0x01, 0x09, 0x06]); // the plain keyboard interface
        t.node("hidraw1", KEYBOARD, &[0x06, 0x88, 0x00, 0x09, 0x01]); // the vendor interface
        t.node("hidraw2", "HID_ID=0003:0000046D:0000C52B\n", &[0x06, 0x88, 0x00]); // another device
        assert_eq!(find_node_in(&t.0, Path::new("/dev")), Some(PathBuf::from("/dev/hidraw1")));
    }

    #[test]
    fn accepts_the_short_form_of_the_usage_page_and_any_case() {
        let t = Tree::new();
        t.node("hidraw3", &KEYBOARD.to_lowercase(), &[0x01, 0x05, 0x88, 0x09]);
        assert_eq!(find_node_in(&t.0, Path::new("/dev")), Some(PathBuf::from("/dev/hidraw3")));
    }

    #[test]
    fn nodes_are_examined_in_name_order_as_text() {
        let t = Tree::new();
        t.node("hidraw2", KEYBOARD, &[0x06, 0x88, 0x00]);
        t.node("hidraw10", KEYBOARD, &[0x06, 0x88, 0x00]);
        assert_eq!(
            find_node_in(&t.0, Path::new("/dev")),
            Some(PathBuf::from("/dev/hidraw10")),
            "as 0.3.x did"
        );
    }

    #[test]
    fn nothing_matches_gives_none() {
        let t = Tree::new();
        assert_eq!(find_node_in(&t.0, Path::new("/dev")), None, "no nodes");
        t.node("hidraw0", KEYBOARD, &[0x05, 0x01]);
        assert_eq!(find_node_in(&t.0, Path::new("/dev")), None, "no vendor page");
        t.node("hidraw1", KEYBOARD, &[]);
        assert_eq!(find_node_in(&t.0, Path::new("/dev")), None, "an empty descriptor");
        assert_eq!(find_node_in(Path::new("/nonexistent"), Path::new("/dev")), None, "no sysfs");
    }

    #[test]
    fn unreadable_and_odd_entries_are_skipped() {
        let t = Tree::new();
        std::fs::create_dir_all(t.0.join("class/hidraw/hidraw0/device")).expect("dir"); // no uevent
        std::fs::create_dir_all(t.0.join("class/hidraw/other")).expect("dir");
        std::fs::write(t.0.join("class/hidraw/hidrawX"), "a file, not a node").expect("file");
        t.node("hidraw5", "\u{ff}\u{fe} not utf-8 HID_ID=0003:000004F2:00000117", &[0x05, 0x88]);
        assert_eq!(find_node_in(&t.0, Path::new("/dev")), Some(PathBuf::from("/dev/hidraw5")));
    }
}

// ---- the service ---------------------------------------------------------------------------

#[test]
fn every_gesture_runs_its_own_binding_at_the_right_time() {
    let rig = Rig::new(3000);
    // tap: press 0, release 30 -> resolves 100 ms after the release
    rig.world.report(0, &PRESS).report(30, &RELEASE);
    // double: 500/530 and 600/630 -> resolves at 730
    rig.world.report(500, &PRESS).report(530, &RELEASE).report(600, &PRESS).report(630, &RELEASE);
    // triple: the third release resolves at once
    rig.world.report(1000, &PRESS).report(1030, &RELEASE).report(1060, &PRESS).report(1090, &RELEASE);
    rig.world.report(1120, &PRESS).report(1150, &RELEASE);
    // long: fires 250 ms into the press, while the key is held; the release is silent
    rig.world.report(2000, &PRESS).report(2600, &RELEASE);
    let actions = rig.run();
    assert_eq!(
        actions,
        [
            (130, "A_tap".to_owned()),
            (730, "A_double".to_owned()),
            (1150, "A_triple".to_owned()),
            (2250, "A_long".to_owned()),
        ]
    );
}

#[test]
fn with_no_double_or_triple_tap_bound_a_tap_runs_as_the_key_comes_up() {
    // The default setup: only the tap does anything. Waiting the multi-tap window (450 ms by
    // default) for a second press that can mean nothing was most of the delay of the PREDATOR key.
    let none = || Bindings { double: "none".into(), triple: "none".into(), multi_ms: 450, ..bindings() };
    let rig = Rig { world: World::new(3000, none()), screen: Arc::new(RecordingNotifier::default()) };
    rig.world.report(0, &PRESS).report(30, &RELEASE);
    rig.world.report(500, &PRESS).report(520, &RELEASE).report(560, &PRESS).report(580, &RELEASE);
    rig.world.report(2000, &PRESS).report(2600, &RELEASE);
    assert_eq!(
        rig.run(),
        [
            (30, "A_tap".to_owned()),
            (520, "A_tap".to_owned()),
            (580, "A_tap".to_owned()),
            (2250, "A_long".to_owned()),
        ]
    );
    // Binding a double tap brings the window back, without a restart.
    let rig = Rig { world: World::new(8000, none()), screen: Arc::new(RecordingNotifier::default()) };
    rig.world
        .at(100, Ev::Config(Some(2), Ok(Bindings { double: "B_double".into(), multi_ms: 450, ..none() })));
    rig.world.report(5000, &PRESS).report(5030, &RELEASE);
    assert_eq!(rig.run(), [(5480, "A_tap".to_owned())], "a tap waits 450 ms again once a double tap exists");
}

#[test]
fn fn_f10_toggles_the_touchpad_and_fn_esc_runs_its_binding() {
    let rig = Rig::new(2000);
    rig.world.report(100, &FN_F10).report(130, &RELEASE).report(500, &FN_ESC).report(530, &RELEASE);
    let actions = rig.run();
    assert_eq!(actions, [(100, "touchpad-toggle".to_owned()), (500, "A_esc".to_owned())]);
    // the releases that end Fn+F10 and Fn+Esc are not PREDATOR taps
}

#[test]
fn the_releases_of_other_keys_never_count_as_taps_even_between_real_taps() {
    let rig = Rig::new(2000);
    rig.world.report(0, &FN_F10).report(10, &RELEASE).report(20, &RELEASE);
    rig.world.report(300, &PRESS).report(330, &RELEASE);
    assert_eq!(specs(&rig.run()), ["touchpad-toggle", "A_tap"]);
}

#[test]
fn reports_that_are_not_ours_are_ignored_and_unknown_vendor_codes_are_logged() {
    let rig = Rig::new(1000);
    rig.world.report(10, &[0x04, 0x82, 0xff]); // Fn+F11 or Fn+F12: the keyboard does it itself
    rig.world.report(20, &[0x04, 0x81]); // too short
    rig.world.report(30, &[]);
    rig.world.report(40, &[0x05, 0x81, 0xff]); // another report id
    rig.world.report(50, &[0x01, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a]);
    assert!(rig.run().is_empty());
    assert!(rig.log_has("ignored vendor report 0482ff"), "{:?}", rig.log());
    assert_eq!(rig.log().iter().filter(|l| l.contains("ignored vendor report")).count(), 1);
}

#[test]
fn a_report_longer_than_three_bytes_still_decodes() {
    let rig = Rig::new(1000);
    rig.world.report(10, &[0x04, 0x80, 0xff, 0x00, 0x00]);
    assert_eq!(specs(&rig.run()), ["touchpad-toggle"]);
}

#[test]
fn a_message_is_shown_and_a_failure_is_shown_too() {
    let rig = Rig::new(2000);
    {
        let mut b = bindings();
        b.tap = "say".into();
        b.double = "fail".into();
        *poison_free(&rig.world.config) = (Some(1), Ok(b));
    }
    rig.world.report(0, &PRESS).report(30, &RELEASE);
    rig.world.report(500, &PRESS).report(530, &RELEASE).report(600, &PRESS).report(630, &RELEASE);
    rig.world.report(1000, &PRESS).report(1030, &RELEASE).report(1060, &PRESS).report(1090, &RELEASE);
    rig.world.report(1120, &PRESS).report(1150, &RELEASE);
    assert_eq!(specs(&rig.run()), ["say", "fail", "A_triple"]);
    assert_eq!(
        rig.screen.shown(),
        [("Hello".to_owned(), "power".to_owned()), ("TrueSense: it broke".to_owned(), "power".to_owned())]
    );
    assert!(rig.log_has("tap -> say -> Hello"));
    assert!(rig.log_has("double -> fail FAILED: it broke"));
    assert!(rig.log_has("triple -> A_triple -> ok"));
}

#[test]
fn a_panicking_action_does_not_end_the_service() {
    let rig = Rig::new(2000);
    *poison_free(&rig.world.config) = (Some(1), Ok(Bindings { fn_esc: "panic".into(), ..bindings() }));
    rig.world.report(100, &FN_ESC).report(200, &FN_ESC).report(300, &FN_F10);
    assert_eq!(specs(&rig.run()), ["panic", "panic", "touchpad-toggle"]);
    assert!(rig.log_has("fn_esc -> panic FAILED: the action panicked"));
}

#[test]
fn a_binding_that_does_not_validate_is_none_from_the_start() {
    let rig = Rig::new(1000);
    *poison_free(&rig.world.config) = (Some(1), Ok(Bindings { tap: "bad-thing".into(), ..bindings() }));
    rig.world.report(0, &PRESS).report(30, &RELEASE);
    assert_eq!(specs(&rig.run()), ["none"]);
    assert!(
        rig.log_has("keys.tap = 'bad-thing' is invalid (not an action); using 'none'"),
        "{:?}",
        rig.log()
    );
}

#[test]
fn nonsense_timings_fall_back_to_the_defaults() {
    for (long_ms, multi_ms) in [(0, 0), (-5, -5), (i64::MIN, i64::MIN)] {
        let rig = Rig::new(3000);
        *poison_free(&rig.world.config) = (Some(1), Ok(Bindings { long_ms, multi_ms, ..bindings() }));
        // a 40 ms press must be a tap (a zero threshold would make it long), and 800 ms is long
        rig.world.report(0, &PRESS).report(40, &RELEASE).report(2000, &PRESS).report(2900, &RELEASE);
        assert_eq!(specs(&rig.run()), ["A_tap", "A_long"], "{long_ms} {multi_ms}");
    }
}

#[test]
fn an_unreadable_config_at_start_means_the_defaults_until_it_is_fixed() {
    let rig = Rig::new(3000);
    *poison_free(&rig.world.config) = (Some(1), Err("[keys] tap: wrong type".into()));
    rig.world.report(0, &PRESS).report(30, &RELEASE);
    rig.world.at(1000, Ev::Config(Some(2), Ok(bindings())));
    rig.world.report(2000, &PRESS).report(2030, &RELEASE);
    assert_eq!(specs(&rig.run()), ["power-next", "A_tap"]);
    assert!(rig.log_has("key bindings not read ([keys] tap: wrong type); using the defaults"));
}

#[test]
fn a_changed_config_is_picked_up_without_a_restart() {
    let rig = Rig::new(10_000);
    rig.world.report(0, &PRESS).report(30, &RELEASE);
    let changed =
        Bindings { tap: "B_tap".into(), fn_esc: "B_esc".into(), long_ms: 400, multi_ms: 150, ..bindings() };
    rig.world.at(1000, Ev::Config(Some(2), Ok(changed)));
    rig.world.report(5000, &PRESS).report(5030, &RELEASE); // tap with the new binding and window
    rig.world.report(6000, &FN_ESC);
    rig.world.report(7000, &PRESS).report(7350, &RELEASE); // 350 ms: long before, a tap now (threshold 400)
    let actions = rig.run();
    assert_eq!(
        actions,
        [
            (130, "A_tap".to_owned()),
            (5180, "B_tap".to_owned()),
            (6000, "B_esc".to_owned()),
            (7500, "B_tap".to_owned()),
        ]
    );
    assert!(rig.log_has("key bindings reloaded: tap='B_tap'"));
}

#[test]
fn a_reload_keeps_what_is_running_when_the_new_file_is_wrong() {
    let rig = Rig::new(10_000);
    rig.world.at(1000, Ev::Config(Some(2), Err("half-written".into())));
    rig.world.at(
        3000,
        Ev::Config(
            Some(3),
            Ok(Bindings { tap: "bad-tap".into(), double: "B_double".into(), long_ms: 0, ..bindings() }),
        ),
    );
    rig.world.report(5000, &PRESS).report(5030, &RELEASE);
    rig.world.report(6000, &PRESS).report(6030, &RELEASE).report(6060, &PRESS).report(6090, &RELEASE);
    rig.world.report(7000, &PRESS).report(7300, &RELEASE);
    let actions = rig.run();
    assert_eq!(
        specs(&actions),
        ["A_tap", "B_double", "A_long"],
        "an invalid action keeps its old binding, a valid one changes, a zero timing is ignored"
    );
    assert!(rig.log_has("key bindings not reloaded: half-written"));
}

#[test]
fn an_unchanged_stamp_is_not_a_reload() {
    let rig = Rig::new(5000);
    rig.world.at(1000, Ev::Config(Some(1), Ok(Bindings { tap: "B_tap".into(), ..bindings() })));
    rig.world.report(3000, &PRESS).report(3030, &RELEASE);
    assert_eq!(specs(&rig.run()), ["A_tap"]);
    assert!(!rig.log_has("reloaded"));
}

#[test]
fn a_deleted_config_file_is_a_change_too() {
    let rig = Rig::new(5000);
    rig.world.at(1000, Ev::Config(None, Ok(Bindings::default())));
    rig.world.report(3000, &PRESS).report(3030, &RELEASE);
    assert_eq!(specs(&rig.run()), ["power-next"], "the defaults apply when the user's file is gone");
}

#[test]
fn the_keyboard_going_away_and_coming_back_is_followed() {
    let rig = Rig::new(20_000);
    rig.world.report(0, &PRESS).report(30, &RELEASE);
    rig.world.at(1000, Ev::Present(false)); // suspend
    rig.world.at(9000, Ev::Present(true)); // resume
    rig.world.report(15_000, &PRESS).report(15_030, &RELEASE);
    let actions = rig.run();
    assert_eq!(specs(&actions), ["A_tap", "A_tap"]);
    assert_eq!(rig.world.opens.load(Ordering::SeqCst), 2, "opened again after the return");
    assert!(rig.log_has("lost the keyboard: the Fn-key interface went away; looking for it again"));
    assert_eq!(rig.log().iter().filter(|l| l.starts_with("listening on /dev/hidraw7")).count(), 2);
    // while away it asked once, not every two seconds
    assert_eq!(rig.log().iter().filter(|l| l.contains("was not found")).count(), 1, "{:?}", rig.log());
}

#[test]
fn the_return_is_noticed_within_the_reconnect_delay() {
    let rig = Rig::new(20_000);
    rig.world.at(1000, Ev::Present(false)).at(5000, Ev::Present(true));
    rig.world.report(7000, &PRESS).report(7030, &RELEASE);
    assert_eq!(rig.run(), [(7130, "A_tap".to_owned())]);
}

#[test]
fn a_keyboard_that_is_not_there_at_the_start_is_waited_for() {
    let rig = Rig::new(10_000);
    rig.world.present.store(false, Ordering::SeqCst);
    rig.world.at(3000, Ev::Present(true));
    rig.world.report(6000, &PRESS).report(6030, &RELEASE);
    assert_eq!(rig.run(), [(6130, "A_tap".to_owned())]);
    assert!(rig.log_has("the Fn-key HID interface (usage page 0x88) was not found; waiting for it"));
}

#[test]
fn a_keyboard_that_never_comes_back_does_not_end_the_service() {
    let rig = Rig::new(120_000);
    rig.world.at(1000, Ev::Present(false));
    assert!(rig.run().is_empty());
    assert_eq!(
        rig.log().iter().filter(|l| l.contains("was not found")).count(),
        1,
        "logged once, not every try"
    );
}

#[test]
fn a_read_error_reconnects_too() {
    let rig = Rig::new(20_000);
    rig.world.at(1000, Ev::ReadError);
    rig.world.report(8000, &PRESS).report(8030, &RELEASE);
    assert_eq!(specs(&rig.run()), ["A_tap"]);
    assert!(rig.log_has("lost the keyboard: read Fn-key report: boom"));
    assert_eq!(rig.world.opens.load(Ordering::SeqCst), 2);
}

#[test]
fn a_press_in_progress_when_the_keyboard_vanishes_is_not_a_phantom_long_press() {
    let rig = Rig::new(20_000);
    rig.world.report(1000, &PRESS); // never released: the keyboard goes
    rig.world.at(1100, Ev::Present(false)).at(3000, Ev::Present(true));
    rig.world.report(8000, &RELEASE); // a release of nothing
    assert!(rig.run().is_empty());
}

#[test]
fn a_tap_counted_before_the_keyboard_vanished_still_resolves() {
    let rig = Rig::new(20_000);
    rig.world.report(1000, &PRESS).report(1030, &RELEASE);
    rig.world.at(1050, Ev::Present(false)).at(3000, Ev::Present(true));
    assert_eq!(rig.run(), [(1130, "A_tap".to_owned())]);
}

#[test]
fn hostile_report_streams_never_panic_and_only_bound_actions_run() {
    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let rig = Rig::new(200_000);
    let mut at = 0;
    for _ in 0..4000 {
        let r = next();
        at += r % 90;
        let len = usize::try_from((r >> 8) % 70).unwrap_or(0);
        let mut report: Vec<u8> = (0..len).map(|_| u8::try_from(next() % 256).unwrap_or(0)).collect();
        // make a fair share of them almost valid
        if r % 3 == 0 && report.len() >= 3 {
            report[0] = 0x04;
            let pick = [PRESS, RELEASE, FN_F10, FN_ESC][usize::try_from(r % 4).unwrap_or(0)];
            report[1] = pick[1];
            report[2] = pick[2];
        }
        rig.world.report(at, &report);
        if r % 97 == 0 {
            rig.world.at(at, Ev::Present(false));
            rig.world.at(at + 2500, Ev::Present(true));
        }
    }
    let actions = rig.run();
    assert!(!actions.is_empty());
    let allowed = ["A_tap", "A_double", "A_triple", "A_long", "A_esc", "touchpad-toggle"];
    assert!(actions.iter().all(|(_, spec)| allowed.contains(&spec.as_str())), "{:?}", specs(&actions));
}

#[test]
fn actions_are_started_on_their_own_so_a_slow_one_never_stalls_the_key() {
    // With a spawner that parks the jobs, the loop carries on and the later gestures still resolve.
    let rig = Rig::new(5000);
    let parked: Parked = Arc::new(Mutex::new(Vec::new()));
    let mut env = rig.env();
    let sink = Arc::clone(&parked);
    env.spawn = Arc::new(move |job| {
        poison_free(&sink).push(job);
        Ok(())
    });
    rig.world.report(0, &PRESS).report(30, &RELEASE).report(1000, &FN_ESC).report(1100, &FN_F10);
    let ran = rig.run_with(env);
    assert!(ran.is_empty(), "nothing ran yet, but the loop did not wait for it");
    assert_eq!(poison_free(&parked).len(), 3);
    for job in poison_free(&parked).drain(..) {
        job();
    }
    assert_eq!(specs(&poison_free(&rig.world.actions)), ["A_tap", "A_esc", "touchpad-toggle"]);
}

#[test]
fn hung_actions_cannot_pile_up_threads() {
    let rig = Rig::new(5000);
    let parked: Parked = Arc::new(Mutex::new(Vec::new()));
    let mut env = rig.env();
    let sink = Arc::clone(&parked);
    env.spawn = Arc::new(move |job| {
        poison_free(&sink).push(job);
        Ok(())
    });
    env.timing.max_actions = 3;
    for i in 0..6 {
        rig.world.report(100 + i * 10, &FN_ESC);
    }
    rig.run_with(env);
    assert_eq!(poison_free(&parked).len(), 3, "only three are running");
    let dropped = rig.log().iter().filter(|l| l.contains("dropped: 3 actions are still running")).count();
    assert_eq!(dropped, 3);
    // finishing the running ones frees the room again: the counter follows the jobs
    let jobs: Vec<_> = poison_free(&parked).drain(..).collect();
    for job in jobs {
        job();
    }
}

#[test]
fn an_action_that_cannot_be_started_is_logged_and_frees_its_slot() {
    let rig = Rig::new(5000);
    let mut env = rig.env();
    env.spawn = Arc::new(|_job| Err("no threads left".to_owned()));
    env.timing.max_actions = 2;
    for i in 0..6 {
        rig.world.report(100 + i * 10, &FN_ESC);
    }
    rig.run_with(env);
    let failed =
        rig.log().iter().filter(|l| l.contains("could not start the action: no threads left")).count();
    assert_eq!(failed, 6, "every press is tried: a failed start does not use up a slot");
    assert!(!rig.log_has("dropped"));
}

// ---- systemd -------------------------------------------------------------------------------

#[test]
fn systemd_is_told_ready_status_watchdog_and_stopping() {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "ts-keys-notify-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("notify.sock");
    let socket = UnixDatagram::bind(&path).expect("bind");
    socket.set_nonblocking(true).expect("nonblocking");

    let rig = Rig::new(3500);
    let mut env = rig.env();
    env.systemd = Systemd::to_address(path.to_str().expect("utf-8 path")).expect("address");
    env.watchdog = Some(Duration::from_secs(1));
    rig.world.at(1500, Ev::Present(false)).at(2500, Ev::Present(true)); // the watchdog keeps pinging while it waits
    rig.run_with(env);

    let mut got = Vec::new();
    let mut buf = [0u8; 256];
    while let Ok(n) = socket.recv(&mut buf) {
        got.push(String::from_utf8_lossy(&buf[..n]).into_owned());
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.first().map(String::as_str), Some("READY=1"), "{got:?}");
    assert_eq!(got.last().map(String::as_str), Some("STOPPING=1"), "{got:?}");
    let pings = got.iter().filter(|m| *m == "WATCHDOG=1").count();
    assert!(
        (3..=4).contains(&pings),
        "one ping a second for 3.5 s, also while the keyboard is away: {got:?}"
    );
    assert!(got.iter().any(|m| m == "STATUS=waiting for the keyboard"), "{got:?}");
    assert!(got.iter().any(|m| m == "STATUS=listening on /dev/hidraw7"), "{got:?}");
}

#[test]
fn without_systemd_nothing_is_sent_and_nothing_fails() {
    let rig = Rig::new(1500);
    let mut env = rig.env();
    env.watchdog = Some(ms(100));
    rig.run_with(env);
    assert!(!rig.log_has("systemd"), "{:?}", rig.log());
}

#[test]
fn background_loops_get_the_stop_flag_and_are_joined() {
    let rig = Rig::new(1000);
    let mut env = rig.env();
    let ended = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&ended);
    env.background.push(Box::new(move |stop| {
        while !stop.load(Ordering::SeqCst) {
            std::thread::sleep(ms(1));
        }
        flag.store(true, Ordering::SeqCst);
    }));
    env.background.push(Box::new(|_| panic!("a loop that panics")));
    rig.run_with(env);
    assert!(ended.load(Ordering::SeqCst), "joined after the stop flag");
    assert!(rig.log_has("a background loop panicked"));
}

// ---- over the real pieces ------------------------------------------------------------------

#[test]
fn config_bindings_follow_the_user_file() {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "ts-keys-config-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("dir");
    let user = dir.join("config.toml");
    let source = ConfigBindings(ConfigFiles::at(None, Some(user.clone())));
    assert_eq!(source.stamp(), None, "no file");
    assert_eq!(source.load().expect("defaults").tap, Bindings::default().tap);
    std::fs::write(&user, "[keys]\ntap = \"fan:max\"\nlong_ms = 900\n").expect("write");
    assert!(source.stamp().is_some());
    let loaded = source.load().expect("load");
    assert_eq!(
        (loaded.tap.as_str(), loaded.long_ms, loaded.double.as_str()),
        ("fan:max", 900, "output-toggle")
    );
    std::fs::write(&user, "[keys\n").expect("write");
    assert!(source.load().is_err(), "half-written file");
    let no_home = ConfigBindings(ConfigFiles::at(None, None));
    assert_eq!(no_home.stamp(), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_service_over_the_simulated_machine_waits_for_the_keyboard_and_stops() {
    use ts_api::api::{Api, Services};
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "ts-keys-sim-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("dir");
    let (services, _rig) = Services::simulated(&dir).expect("services");
    let api = Api::new(services);
    let mut env = Env::from_api(&api);
    assert_eq!(
        env.background.len(),
        2,
        "the per-key animation loop and the theme renderer come with the service"
    );
    env.background.clear();
    env.timing.max_wait = ms(5);
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = Arc::clone(&lines);
    env.log = Arc::new(move |l| poison_free(&sink).push(l.to_owned()));
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let watched = Arc::clone(&lines);
    // Stop the service once it has said what this test is about, however long that takes.
    // (If it never does, the stop comes after a minute and the assertion below says so.)
    let stopper = std::thread::spawn(move || {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(60)
            && !poison_free(&watched).iter().any(|l| l.contains("was not found; waiting for it"))
        {
            std::thread::sleep(ms(1));
        }
        flag.store(true, Ordering::SeqCst);
    });
    Service::new(env).run(&stop);
    stopper.join().expect("stopper");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        poison_free(&lines).iter().any(|l| l.contains("was not found; waiting for it")),
        "{:?}",
        poison_free(&lines)
    );
}

/// A clock that only the test moves, so that no gesture depends on how fast the machine runs.
struct ManualClock(Mutex<Duration>);

impl ManualClock {
    fn advance(&self, by: Duration) {
        *poison_free(&self.0) += by;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Duration {
        *poison_free(&self.0)
    }

    fn sleep(&self, how_long: Duration) {
        // Only used while there is no keyboard: really wait a little, never move the fake time.
        std::thread::sleep(how_long.min(ms(5)));
    }
}

/// A real descriptor that counts what the service does with it: `calls` is how often the service
/// asked for a report, `taken` how many it got. A report is fully handled once the service asks
/// again, which is how the test knows it may let time pass.
struct Counted {
    inner: HidrawFile,
    calls: Arc<AtomicUsize>,
    taken: Arc<AtomicUsize>,
}

impl HidReports for Counted {
    fn read_report(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, HidError> {
        self.read_report_watching(timeout, None)
    }

    fn read_report_watching(
        &mut self,
        timeout: Duration,
        watch: Option<&Watcher>,
    ) -> Result<Option<Vec<u8>>, HidError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let report = self.inner.read_report_watching(timeout, watch)?;
        if report.is_some() {
            self.taken.fetch_add(1, Ordering::SeqCst);
        }
        Ok(report)
    }
}

/// Waits (up to a minute, which only a failure ever uses) until `condition` holds.
fn eventually(what: &str, condition: impl Fn() -> bool) {
    let started = Instant::now();
    while !condition() {
        assert!(started.elapsed() < Duration::from_secs(60), "waited for {what}");
        std::thread::sleep(ms(1));
    }
}

#[test]
fn real_reports_over_a_real_descriptor_run_the_bound_action_when_the_clock_says_so() {
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;

    struct OnceOpener(Mutex<Option<Counted>>);
    impl HidOpener for OnceOpener {
        fn open(&self, _node: &Path) -> Result<Box<dyn HidReports>, HidError> {
            poison_free(&self.0).take().map(|f| Box::new(f) as Box<dyn HidReports>).ok_or(HidError::Gone)
        }
    }

    let (device, mut keyboard) = UnixStream::pair().expect("pair");
    let (calls, taken) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let counted = Counted {
        inner: HidrawFile::from_file(std::fs::File::from(OwnedFd::from(device))),
        calls: Arc::clone(&calls),
        taken: Arc::clone(&taken),
    };
    let clock = Arc::new(ManualClock(Mutex::new(Duration::ZERO)));
    let rig = Rig::new(0);
    let mut env = rig.env();
    env.opener = Arc::new(OnceOpener(Mutex::new(Some(counted))));
    env.clock = Arc::clone(&clock) as Arc<dyn Clock>;
    env.timing.max_wait = ms(10);
    *poison_free(&rig.world.config) = (Some(1), Ok(Bindings { long_ms: 400, multi_ms: 40, ..bindings() }));
    let stop = Arc::clone(&rig.world.stop);
    let world = Arc::clone(&rig.world);
    let service = std::thread::spawn(move || Service::new(env).run(&world.stop));

    // Sends one report and waits until the service has taken it and come back for the next one:
    // the stream is a byte stream, so reports must not run together, and the time must not move
    // while the service is still deciding what the report meant.
    let mut sent = 0;
    let mut send = |bytes: &[u8]| {
        keyboard.write_all(bytes).expect("report");
        sent += 1;
        eventually("the report to be taken", || taken.load(Ordering::SeqCst) == sent);
        let seen = calls.load(Ordering::SeqCst);
        eventually("the report to be handled", || calls.load(Ordering::SeqCst) > seen);
    };
    send(&PRESS);
    send(&RELEASE);
    // A short press, and the multi-tap window (40 ms) still open: nothing has run yet.
    assert!(poison_free(&rig.world.actions).is_empty());
    clock.advance(ms(100));
    eventually("the single tap", || !poison_free(&rig.world.actions).is_empty());
    send(&FN_F10);
    eventually("fn+f10", || poison_free(&rig.world.actions).len() >= 2);
    stop.store(true, Ordering::SeqCst);
    service.join().expect("service thread");
    assert_eq!(specs(&poison_free(&rig.world.actions).clone()), ["A_tap", "touchpad-toggle"]);
}

use std::io::Write as _;

#[test]
fn both_lighting_loops_idle_with_the_same_pause_that_the_quiet_sleep_stretches() {
    assert_eq!(ts_domain::perkeyfx::IDLE_INTERVAL, crate::renderer::IDLE);
    // A frame pause is never stretched: only a ring ends it early.
    let started = std::time::Instant::now();
    super::lighting_sleep(true)(Duration::from_millis(20));
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn the_watching_pace_sleeps_long_and_the_default_pace_polls() {
    let watching = LoopTiming::watching();
    assert!(watching.max_wait >= Duration::from_secs(5) && watching.config_poll >= Duration::from_secs(30));
    assert_eq!(
        watching.reconnect,
        LoopTiming::default().reconnect,
        "a missing keyboard is looked for as before"
    );
    assert!(LoopTiming::default().max_wait <= Duration::from_millis(500));
}
