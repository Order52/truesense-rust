//! The loop with its threads: the same screen on a text terminal, the simulated laptop behind it,
//! the real worker and readers. Plus the small decisions `run` makes before it touches the
//! terminal.

use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::thread;

use ratatui::Terminal;
use ratatui::backend::TestBackend;

use super::*;
use crate::tui::app::{Runtime, event_loop};
use crate::tui::{Options, colour_terminal, ensure_terminal, page_index, page_names, run, unicode_terminal};

/// A screen of the loop over a laptop that is not under a harness.
struct Live {
    dir: TempDir,
    rig: Rig,
    backend: Arc<dyn Backend>,
}

fn live(service: bool) -> Live {
    let dir = TempDir::new();
    let (services, rig) = Services::simulated(&dir.0).expect("services");
    if service {
        script_service(&rig);
    } else {
        rig.daemon.set_available(false);
    }
    let backend: Arc<dyn Backend> = Arc::new(ApiBackend::new(Arc::new(Api::new(services)), None));
    Live { dir, rig, backend }
}

fn run_loop(live: &Live, inputs: Vec<Msg>, size: (u16, u16)) -> (String, Model) {
    let (tx, rx) = mpsc::channel();
    let mut model =
        Model::new(&live.backend.catalog(), "test", true, Arc::new(|| WALL.to_owned()), Instant::now());
    let runtime = Runtime::start(&live.backend, &tx);
    let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).expect("a terminal");
    for m in inputs {
        tx.send(m).expect("send");
    }
    tx.send(Msg::Input(Input::Quit)).expect("quit");
    event_loop(&mut terminal, &mut model, &runtime, &rx, Palette::new(true)).expect("the loop");
    assert!(runtime.writes_idle(), "the loop leaves no write running");
    (buffer_text(terminal.backend().buffer()), model)
}

fn key(c: char) -> Msg {
    Msg::Input(Input::Key(Key::Char(c)))
}

#[test]
fn the_loop_draws_the_dashboard_and_quits() {
    let live = live(false);
    let (text, model) = run_loop(&live, vec![], (100, 30));
    assert!(text.contains("TrueSense") && text.contains("Power mode"), "{text}");
    assert!(model.quit());
}

#[test]
fn a_change_made_just_before_quitting_is_sent_and_waited_for() {
    let live = live(false);
    // The fan slider: left unread it starts in the middle and one step on is chosen, not yet written.
    let (_, model) = run_loop(&live, vec![Msg::Goto(1), key('j'), key('l')], (100, 30));
    let snap = live.rig.sim.snapshot();
    assert_eq!(
        format!("{:?}", snap.fan),
        "Manual",
        "the chosen duty reached the firmware before the loop returned"
    );
    assert!(snap.duty[0] > 30, "{:?}", snap.duty);
    assert!(model.pending_is_empty());
}

#[test]
fn a_question_that_is_open_when_you_quit_is_a_no() {
    let live = live(true);
    let (_, _) = run_loop(
        &live,
        vec![
            Msg::Goto(1),
            key('j'),
            key('h'),
            key('h'),
            key('h'),
            key('h'),
            key('h'),
            key('h'),
            key('h'),
            key('h'),
            Msg::Input(Input::Key(Key::Enter)),
        ],
        (100, 30),
    );
    assert!(live.rig.daemon.last_args("fan").is_none(), "nothing reached the daemon");
    assert_eq!(live.rig.sim.snapshot().duty, [30, 30]);
}

#[test]
fn pages_open_from_the_start_page_and_the_readers_fill_them() {
    let live = live(false);
    let (text, _) = run_loop(&live, vec![Msg::Goto(2)], (100, 30));
    assert!(text.contains("Battery limit"), "{text}");
}

#[test]
fn a_mouse_press_on_a_tab_goes_through_the_whole_loop() {
    let live = live(false);
    // The first frame is drawn before the first message is read, so the tab positions are known.
    let (text, model) = run_loop(&live, vec![Msg::Input(Input::Press { x: 15, y: 1 })], (100, 30));
    assert_eq!(model.page, 1, "{text}");
}

#[test]
fn unknown_pages_are_refused_before_the_terminal_is_touched() {
    let live = live(false);
    let api = Arc::new(Api::new(Services::simulated(&live.dir.0.join("again")).expect("services").0));
    let err = run(api, Options { page: Some("nope".into()), ..Options::default() }).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert!(err.to_string().contains("dashboard, fans, power, lighting, audio, profiles, system"), "{err}");
}

#[test]
fn pages_are_found_by_name_or_number() {
    assert_eq!(page_names()[0], "Dashboard");
    for (name, want) in [
        ("fans", Some(1)),
        ("FANS", Some(1)),
        (" audio ", Some(4)),
        ("1", Some(0)),
        ("7", Some(6)),
        ("0", None),
        ("8", None),
        ("", None),
        ("nope", None),
    ] {
        assert_eq!(page_index(name), want, "{name:?}");
    }
}

#[test]
fn the_character_set_and_colour_follow_the_environment() {
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| (*v).to_owned())
    };
    assert!(unicode_terminal(&env(&[("LANG", "en_US.UTF-8")])));
    assert!(unicode_terminal(&env(&[("LC_ALL", "de_DE.utf8"), ("LANG", "C")])), "LC_ALL wins");
    assert!(!unicode_terminal(&env(&[("LANG", "C")])));
    assert!(!unicode_terminal(&env(&[])));
    assert!(
        !unicode_terminal(&env(&[("LANG", "en_US.UTF-8"), ("TERM", "linux")])),
        "the bare console has no box characters"
    );
    assert!(colour_terminal(&env(&[("TERM", "xterm-256color")])));
    assert!(!colour_terminal(&env(&[("NO_COLOR", "1")])));
    assert!(colour_terminal(&env(&[("NO_COLOR", "")])), "an empty NO_COLOR does not count");
    assert!(!colour_terminal(&env(&[("TERM", "dumb")])));
}

#[test]
fn a_terminal_is_needed() {
    assert!(ensure_terminal(true, true).is_ok());
    for (a, b) in [(false, true), (true, false), (false, false)] {
        let err = ensure_terminal(a, b).unwrap_err();
        assert_eq!(err.to_string(), "truesense tui needs a terminal");
    }
}

/// A backend whose first write waits for the gate to open: the writes behind it queue up.
struct Gated {
    inner: Arc<Recorder>,
    open: std::sync::Mutex<bool>,
    changed: std::sync::Condvar,
    started: AtomicBool,
}

impl Gated {
    fn new(inner: Arc<Recorder>) -> Self {
        Self {
            inner,
            open: std::sync::Mutex::new(false),
            changed: std::sync::Condvar::new(),
            started: AtomicBool::new(false),
        }
    }

    fn open(&self) {
        *self.open.lock().unwrap() = true;
        self.changed.notify_all();
    }
}

impl Backend for Gated {
    fn catalog(&self) -> Catalog {
        self.inner.catalog()
    }

    fn fast(&self) -> Fast {
        self.inner.fast()
    }

    fn read(&self, key: ReadKey) -> SlowReading {
        self.inner.read(key)
    }

    fn execute(&self, op: &Op) -> Result<Done, String> {
        if !self.started.swap(true, Ordering::SeqCst) {
            let mut open = self.open.lock().unwrap();
            while !*open {
                open = self.changed.wait(open).unwrap();
            }
        }
        self.inner.execute(op)
    }
}

#[test]
fn a_drag_is_many_edits_and_few_writes_and_the_last_value_wins_through_the_whole_chain() {
    let live = live(false);
    let recorder = Arc::new(Recorder::new(Arc::clone(&live.backend)));
    let gated = Arc::new(Gated::new(Arc::clone(&recorder)));
    let backend: Arc<dyn Backend> = gated.clone();
    let (tx, rx) = mpsc::channel();
    let mut model =
        Model::new(&backend.catalog(), "test", true, Arc::new(|| WALL.to_owned()), Instant::now());
    let runtime = Runtime::start(&backend, &tx);
    runtime.dispatch(&mut model, Msg::Goto(3));
    let row = {
        let page = model.page;
        let i = model.pages[page].rows.iter().position(|r| r.label == "Brightness").expect("a slider");
        (page, i)
    };
    // The first write starts and is held at the gate; the five behind it are one setting.
    for value in [5, 10, 15, 20, 25, 30] {
        model.edit(row, Value::Int(value), true);
        runtime.dispatch(&mut model, Msg::Tick);
        // The first write must really have started (and be held) before the next one is queued.
        while !gated.started.load(Ordering::SeqCst) {
            thread::yield_now();
        }
    }
    gated.open();
    // Collect every result: the finished writes and the ones that were replaced before they began.
    let mut seen = 0;
    while model.writes_in_flight() && seen < 100 {
        if let Ok(msg) = rx.recv_timeout(Duration::from_secs(5)) {
            runtime.dispatch(&mut model, msg);
        }
        seen += 1;
    }
    assert!(!model.writes_in_flight(), "every write was answered, or reported as replaced");
    let writes: Vec<i64> = recorder
        .ops()
        .into_iter()
        .filter_map(|o| if let Op::Keyboard(l) = o { Some(l.brightness) } else { None })
        .collect();
    assert_eq!(writes.first(), Some(&5), "the write that had started finished");
    assert_eq!(writes.last(), Some(&30), "the last value must win");
    assert_eq!(writes.len(), 2, "the four in between were replaced: {writes:?}");
    assert_eq!(model.row(row).state, crate::tui::row::RowState::Idle, "the row settled");
    runtime.finish(&mut model);
}
