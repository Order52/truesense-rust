//! Tests of the screen: input-driven behaviour against the simulated laptop, golden snapshots of
//! every page at several sizes, and a seeded random-input run that must never panic and never stop
//! a fan without a Yes.
//!
//! The harness plays the part of the threads synchronously: every write the model asks for runs on
//! the spot against `Services::simulated` (the firmware simulator and a scripted daemon) and its
//! result is fed back, so a key press and its write are one deterministic step. The threads
//! themselves are tested in `worker` and in `run`.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod backend;
mod dialogs;
mod golden;
mod keys;
mod lights;
mod pages;
mod run;
mod stress;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use serde_json::json;
use ts_api::api::testing::{Rig, daemon_state};
use ts_api::api::{Api, Services};

use super::backend::{ApiBackend, Backend, Catalog, Done, Op, WindowOpener};
use super::data::{Fast, ReadKey, SlowReading};
use super::input::{Input, Key};
use super::model::{Effect, Model, Msg, RowRef};
use super::row::{RowState, Value};
use super::style::{Palette, St};
use super::view;

/// A fresh directory under the system temp dir, removed when dropped.
pub(super) struct TempDir(pub PathBuf);

impl TempDir {
    pub(super) fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ts-tui-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A backend that remembers every write it was asked for and can be told to fail one.
pub(super) struct Recorder {
    inner: Arc<dyn Backend>,
    ops: Mutex<Vec<Op>>,
    failing: Mutex<Vec<(String, String)>>,
}

impl Recorder {
    pub(super) fn new(inner: Arc<dyn Backend>) -> Self {
        Self { inner, ops: Mutex::new(Vec::new()), failing: Mutex::new(Vec::new()) }
    }

    /// Makes every write whose `Debug` text starts with `prefix` fail with `message`.
    pub(super) fn fail_on(&self, prefix: &str, message: &str) {
        self.failing
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((prefix.to_owned(), message.to_owned()));
    }

    pub(super) fn ops(&self) -> Vec<Op> {
        self.ops.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// How many writes started with this `Debug` prefix.
    pub(super) fn count(&self, prefix: &str) -> usize {
        self.ops().iter().filter(|o| format!("{o:?}").starts_with(prefix)).count()
    }
}

impl Backend for Recorder {
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
        self.ops.lock().unwrap_or_else(PoisonError::into_inner).push(op.clone());
        let text = format!("{op:?}");
        let refusal = self
            .failing
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|(p, _)| text.starts_with(p))
            .map(|(_, m)| m.clone());
        match refusal {
            Some(m) => Err(m),
            None => self.inner.execute(op),
        }
    }
}

/// A native fan curve in the shape the daemon answers.
fn curve_reply(name: &str) -> serde_json::Value {
    json!({
        "name": name,
        "curve": {"cpu": [[52, 0], [89, 100]], "gpu": [[48, 0], [78, 100]], "native": true},
        "names": ["native", "truesense", "quiet"],
    })
}

/// Makes the scripted daemon behave like a running service in a fixed state.
pub(super) fn script_service(rig: &Rig) {
    let settings = json!({"smart_fan": false, "auto_mode": "smart", "fan_curve": "native", "smart_profile": "steady", "auto_profile": true});
    let live = json!({
        "tcpu": 55, "tgpu": 50, "chassis": 40, "rpm1": 2400, "rpm2": 2100, "power_mode": "performance", "ac": true,
        "cpu": 12, "gpu": 5, "gpuw": 9, "activity": "quiet desktop", "mode_reason": "quiet desktop",
        "auto_mode": "smart", "smart_profile": "steady", "fan_off": false,
    });
    let d = &rig.daemon;
    d.answer_ok("state", daemon_state(settings, live, 0));
    d.answer_ok("fan_curve", curve_reply("native"));
    d.answer_ok(
        "smart_profile",
        json!({"name": "steady", "profile": {}, "names": ["native", "steady", "truesense"]}),
    );
    d.answer_ok("battery_full", json!({"active": false}));
    d.answer_ok("fan", json!(null));
    d.answer_ok("set", json!({}));
    d.answer_ok("hold", json!(null));
}

fn write_file(path: PathBuf, text: &str) {
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
    std::fs::write(path, text).expect("write");
}

/// How a harness is set up.
#[derive(Clone, Copy)]
pub(super) struct Setup {
    pub service: bool,
    pub battery: bool,
    pub unicode: bool,
}

impl Default for Setup {
    fn default() -> Self {
        Self { service: false, battery: true, unicode: true }
    }
}

/// The screen on a text grid with the simulated laptop behind it.
pub(super) struct Harness {
    pub model: Model,
    pub backend: Arc<Recorder>,
    pub rig: Rig,
    pub now: Instant,
    pub size: (u16, u16),
    pub window_opened: Arc<AtomicUsize>,
    api: Arc<Api>,
    _dir: TempDir,
}

pub(super) const WALL: &str = "12:00:00";

impl Harness {
    pub(super) fn new(w: u16, h: u16) -> Self {
        Self::with(w, h, Setup::default())
    }

    pub(super) fn with_service(w: u16, h: u16) -> Self {
        Self::with(w, h, Setup { service: true, ..Setup::default() })
    }

    pub(super) fn with(w: u16, h: u16, setup: Setup) -> Self {
        let dir = TempDir::new();
        let (services, rig): (Services, Rig) = Services::simulated(&dir.0).expect("simulated services");
        rig.bundles.add("gaming", "turbo power, smart fans");
        rig.bundles.add("battery", "eco power, dim lights");
        if setup.battery {
            let ps = dir.0.join("sys/class/power_supply");
            write_file(ps.join("BAT0/capacity"), "80\n");
            write_file(ps.join("BAT0/status"), "Charging\n");
            write_file(ps.join("ADP1/online"), "1\n");
        }
        if setup.service {
            script_service(&rig);
        } else {
            rig.daemon.set_available(false);
        }
        let api = Arc::new(Api::new(services));
        let opened = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&opened);
        let window: WindowOpener = Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        let inner: Arc<dyn Backend> = Arc::new(ApiBackend::new(Arc::clone(&api), Some(window)));
        let backend = Arc::new(Recorder::new(inner));
        let now = Instant::now();
        let model = Model::new(&backend.catalog(), "test", setup.unicode, Arc::new(|| WALL.to_owned()), now);
        let mut h = Self { model, backend, rig, now, size: (w, h), window_opened: opened, api, _dir: dir };
        for _ in 0..3 {
            h.feed(Msg::Fast(Box::new(h.backend.fast())));
        }
        h.feed(Msg::Goto(0));
        h
    }

    pub(super) fn api(&self) -> &Api {
        &self.api
    }

    /// Runs a message and, on the spot, everything it asks for.
    pub(super) fn feed(&mut self, msg: Msg) {
        let mut todo = VecDeque::from([msg]);
        while let Some(msg) = todo.pop_front() {
            let effects = self.model.update(self.now, msg);
            todo.extend(self.perform(effects));
        }
    }

    /// Does what the model asked for and answers the messages that doing it produces.
    fn perform(&mut self, effects: Vec<Effect>) -> Vec<Msg> {
        let mut follow = Vec::new();
        for effect in effects {
            match effect {
                Effect::Write(w) => {
                    let result = self.backend.execute(&w.op);
                    follow.push(Msg::Written { ticket: w.ticket, result });
                }
                Effect::ReadSlow(keys) => {
                    follow.extend(keys.into_iter().map(|k| Msg::Slow(self.backend.read(k))))
                }
                Effect::WakeFast => follow.push(Msg::Fast(Box::new(self.backend.fast()))),
            }
        }
        follow
    }

    /// Quits: what is still waiting is sent (or dropped, if it needs a question), and the writes run.
    pub(super) fn flush(&mut self) {
        let effects = self.model.flush(self.now);
        let mut todo: VecDeque<Msg> = self.perform(effects).into();
        while let Some(msg) = todo.pop_front() {
            let effects = self.model.update(self.now, msg);
            todo.extend(self.perform(effects));
        }
    }

    pub(super) fn input(&mut self, input: Input) {
        self.feed(Msg::Input(input));
    }

    pub(super) fn key(&mut self, k: Key) {
        self.input(Input::Key(k));
    }

    /// Presses each character as a key.
    pub(super) fn keys(&mut self, s: &str) {
        for c in s.chars() {
            self.key(Key::Char(c));
        }
    }

    /// Lets the pause after a change run out, so what was chosen is written now.
    pub(super) fn elapse(&mut self) {
        self.now += Duration::from_secs(1);
        self.feed(Msg::Tick);
    }

    /// The user chose `value` for a row, to be written at once.
    pub(super) fn edit(&mut self, r: RowRef, value: Value) {
        self.model.edit(r, value, true);
        self.feed(Msg::Tick);
    }

    pub(super) fn goto(&mut self, page: usize) {
        self.feed(Msg::Goto(page));
    }

    /// A fast reading now.
    pub(super) fn fast(&mut self) {
        self.feed(Msg::Fast(Box::new(self.backend.fast())));
    }

    pub(super) fn resize(&mut self, w: u16, h: u16) {
        self.size = (w, h);
        self.input(Input::Resize);
    }

    /// Draws the screen; the whole buffer (with styles) and its text.
    pub(super) fn draw(&mut self) -> Buffer {
        let area = Rect::new(0, 0, self.size.0, self.size.1);
        let mut buf = Buffer::empty(area);
        view::render(&mut self.model, area, &mut buf, Palette::new(true));
        buf
    }

    pub(super) fn text(&mut self) -> String {
        buffer_text(&self.draw())
    }

    pub(super) fn last_line(&mut self) -> String {
        self.text().lines().last().unwrap_or_default().to_owned()
    }

    /// Selects the nth row with this label on the open page.
    pub(super) fn sel(&mut self, label: &str, nth: usize) -> RowRef {
        let page = self.model.page;
        let mut n = nth;
        for (i, r) in self.model.pages[page].rows.iter().enumerate() {
            if r.label == label {
                if n == 0 {
                    self.model.pages[page].sel = i;
                    return (page, i);
                }
                n -= 1;
            }
        }
        panic!("no row {label:?} (#{nth}) on page {}", self.model.current().name);
    }

    pub(super) fn value(&self, r: RowRef) -> Option<Value> {
        self.model.shown(r)
    }

    pub(super) fn state(&self, r: RowRef) -> RowState {
        self.model.row(r).state
    }

    pub(super) fn message(&self) -> (String, St) {
        (self.model.msg.text.clone(), self.model.msg.st)
    }

    /// Presses the first place `text` is drawn on `line` (any line when `None`).
    pub(super) fn click(&mut self, text: &str, line: Option<usize>) -> bool {
        let rendered = self.text();
        for (y, l) in rendered.lines().enumerate() {
            if line.is_some_and(|want| want != y) {
                continue;
            }
            if let Some(at) = l.find(text) {
                let x = l[..at].chars().count() as i32 + 1;
                self.input(Input::Press { x, y: y as i32 });
                return true;
            }
        }
        false
    }

    /// The simulated firmware's power mode code, duties and fan state.
    pub(super) fn sim(&self) -> ([u8; 2], String, u8) {
        let s = self.rig.sim.snapshot();
        (s.duty, format!("{:?}", s.fan), s.mode)
    }
}

/// The buffer as lines without trailing blanks.
pub(super) fn buffer_text(buf: &Buffer) -> String {
    let area = buf.area;
    (area.top()..area.bottom())
        .map(|y| {
            let line: String = (area.left()..area.right()).map(|x| buf[(x, y)].symbol().to_owned()).collect();
            line.trim_end().to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The widest line of a text, in cells.
pub(super) fn widest(text: &str) -> usize {
    text.lines().map(|l| l.chars().count()).max().unwrap_or(0)
}
