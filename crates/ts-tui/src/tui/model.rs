//! The state of the screen and the single function that changes it.
//!
//! The screen is a pure state machine: [`Model::update`] takes a message (a key, a click, a
//! reading, the result of a write) and the time, changes the model and answers the work to do
//! ([`Effect`]s: write this, read that). It never touches the laptop or the terminal, so a test can
//! press keys against it and look at the model and the effects, and a golden snapshot is the view
//! of a model that a test built.
//!
//! The rules the whole screen follows:
//!
//! * a value is never written that the user did not choose (Enter on a row that was not changed
//!   writes nothing);
//! * stopping a fan, or a setting that needs a restart, asks first and the answer defaults to No;
//!   a stop is only ever sent after a Yes, and the status line says what happened;
//! * writes happen one at a time, in the order they were made, and a newer value for the same
//!   setting replaces an older one that has not started (a drag sends the last position);
//! * a value that fails to write goes back to what the hardware has.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::backend::{Catalog, Done, Op};
use super::data::{Data, Fast, ReadKey, SlowReading, SlowValue};
use super::input::Input;
use super::light::Edits;
use super::pages::{self, Ctx, Page};
use super::row::{Row, RowState, Value};
use super::style::St;

/// Identifies a write, from the moment it is queued to the moment its result comes back.
pub(crate) type Ticket = u64;

/// A row: the index of its page and of the row on it.
pub(crate) type RowRef = (usize, usize);

/// How long a status line stays.
pub(crate) const HINT_SECONDS: f64 = 5.0;
pub(crate) const ERROR_SECONDS: f64 = 8.0;
/// How long a hand-picked power mode is held against Smart auto.
pub(crate) const HOLD_MINUTES: f64 = 30.0;
/// How many lines "What you did" remembers.
const LOG_KEEP: usize = 40;

/// The pause after a change before it is written, so that a run of key presses is one write.
pub(crate) const DELAY_SLIDER: Duration = Duration::from_millis(300);
pub(crate) const DELAY_CHOICE: Duration = Duration::from_millis(350);
pub(crate) const DELAY_DEFAULT: Duration = Duration::from_millis(300);
pub(crate) const DELAY_CYCLE_KEY: Duration = Duration::from_millis(250);

/// Tells the wall-clock time for the lines of "What you did" (`15:04:05`). Injected: the screen
/// does not know the time zone, the program and the tests do.
pub type WallClock = Arc<dyn Fn() -> String + Send + Sync>;

/// Something that happened.
#[derive(Debug)]
pub(crate) enum Msg {
    Input(Input),
    /// Time passed: send the changes whose pause is over.
    Tick,
    /// Open a page (the page asked for on the command line).
    Goto(usize),
    /// The dashboard's reading.
    Fast(Box<Fast>),
    /// A setting of the open page was read.
    Slow(SlowReading),
    /// A write finished.
    Written {
        ticket: Ticket,
        result: Result<Done, String>,
    },
    /// A queued write was replaced by a newer one before it started.
    Dropped(Ticket),
}

/// A write to queue: the key says which setting it is for, so the queue can replace an unstarted
/// write of the same setting.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Write {
    pub ticket: Ticket,
    pub key: String,
    pub op: Op,
}

/// Work the model asks the outside to do.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Effect {
    /// Queue a hardware write.
    Write(Write),
    /// Read these settings now.
    ReadSlow(Vec<ReadKey>),
    /// Read the dashboard now instead of at the next second.
    WakeFast,
}

/// Where a clickable rectangle drawn on the last frame leads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Zone {
    Row,
    Toggle,
    Button,
    /// The nth option of a choice.
    Option(usize),
    Prev,
    Next,
    /// A slider's bar, `extra` wide.
    Bar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Hit {
    pub y: i32,
    pub x0: i32,
    pub x1: i32,
    pub row: RowRef,
    pub zone: Zone,
    pub extra: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TabHit {
    pub x0: i32,
    pub x1: i32,
    pub page: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ModalHit {
    pub y: i32,
    pub x0: i32,
    pub x1: i32,
    pub yes: bool,
}

/// What the last frame drew that can be clicked.
#[derive(Debug, Clone, Default)]
pub(crate) struct Frame {
    pub hits: Vec<Hit>,
    pub tabs: Vec<TabHit>,
    pub modal: Vec<ModalHit>,
}

/// A dialog that takes every key and click until it is closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Modal {
    /// A question with `[ Yes ] [ No ]`; the focus starts on No (`1`): Enter never says yes by
    /// itself. A Yes sends `value` for `row`, a No forgets it.
    Confirm {
        text: String,
        focus: usize,
        row: RowRef,
        value: Value,
    },
    /// A line of text; the suggested text is replaced by the first key typed.
    Prompt {
        title: String,
        buf: String,
        fresh: bool,
        which: super::light::Which,
    },
    Help,
}

/// The line above the keys; `here` ties a hint to the row it belongs to.
#[derive(Debug, Clone)]
pub(crate) struct Message {
    pub text: String,
    pub st: St,
    pub until: Option<Instant>,
    pub here: Option<RowRef>,
}

#[derive(Debug, Clone)]
pub(crate) struct LogEntry {
    pub at: String,
    pub text: String,
    pub st: St,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Drag {
    pub row: RowRef,
    pub x0: i32,
    pub width: i32,
}

/// What to say when a write succeeded.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum OkText {
    /// The label of the write.
    #[default]
    Label,
    Text(String),
    /// The result of the sound repair.
    Repair,
    /// The result of "DTS everywhere".
    Dts,
    /// The health check: keep its rows.
    Doctor,
}

/// What to put back when a write failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Undo {
    Keyboard(Edits),
    Rear(Edits),
}

/// A write in flight: what the screen needs when its result comes back.
#[derive(Debug, Clone)]
pub(crate) struct Job {
    pub label: String,
    pub row: Option<RowRef>,
    pub ok: OkText,
    pub undo: Option<Undo>,
}

/// The whole screen.
pub(crate) struct Model {
    pub pages: Vec<Page>,
    pub page: usize,
    pub data: Data,
    pub version: String,
    pub unicode: bool,
    pub(super) now: Instant,
    wall: WallClock,
    pub(super) kb: Edits,
    pub(super) rear: Edits,
    pub(super) last_on: [i64; 2],
    pub(super) pending: Vec<RowRef>,
    pub(super) acting: Option<RowRef>,
    pub(super) drag: Option<Drag>,
    pub log: Vec<LogEntry>,
    pub msg: Message,
    pub modal: Option<Modal>,
    pub frame: Frame,
    quit: bool,
    repaint: bool,
    dirty: bool,
    pub(super) jobs: HashMap<Ticket, Job>,
    next_ticket: Ticket,
    yes: usize,
    effects: Vec<Effect>,
}

impl Model {
    /// A screen with nothing read yet. `catalog` names what the pages list.
    pub(crate) fn new(
        catalog: &Catalog,
        version: &str,
        unicode: bool,
        wall: WallClock,
        now: Instant,
    ) -> Self {
        Self {
            pages: pages::build(catalog),
            page: 0,
            data: Data::default(),
            version: version.to_owned(),
            unicode,
            now,
            wall,
            kb: Edits::default(),
            rear: Edits::default(),
            last_on: [25, 100],
            pending: Vec::new(),
            acting: None,
            drag: None,
            log: Vec::new(),
            msg: Message { text: String::new(), st: St::Dim, until: None, here: None },
            modal: None,
            frame: Frame::default(),
            quit: false,
            repaint: false,
            dirty: true,
            jobs: HashMap::new(),
            next_ticket: 1,
            yes: 0,
            effects: Vec::new(),
        }
    }

    /// Changes the model by one message and answers the work to do.
    pub(crate) fn update(&mut self, now: Instant, msg: Msg) -> Vec<Effect> {
        self.now = now;
        match msg {
            Msg::Input(input) => self.input(input),
            Msg::Tick => self.tick(),
            Msg::Goto(page) => self.goto_page(page as isize),
            Msg::Fast(f) => {
                self.data.apply_fast(*f);
                self.dirty = true;
            }
            Msg::Slow(r) => {
                self.data.apply_slow(r);
                self.dirty = true;
            }
            Msg::Written { ticket, result } => self.written(ticket, result),
            Msg::Dropped(ticket) => self.dropped(ticket),
        }
        std::mem::take(&mut self.effects)
    }

    // ---- what the loop asks

    /// Whether the screen needs drawing (and clears the flag).
    pub(crate) fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    pub(crate) fn touch(&mut self) {
        self.dirty = true;
    }

    /// The user asked to leave.
    pub(crate) fn quit(&self) -> bool {
        self.quit
    }

    /// Whether no change waits for its pause.
    #[cfg(test)]
    pub(crate) fn pending_is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Takes back a request to leave (tests keep a run going past `q`).
    #[cfg(test)]
    pub(crate) fn cancel_quit(&mut self) {
        self.quit = false;
    }

    /// Ctrl-L asked for a redraw of the whole terminal (and clears the request).
    pub(crate) fn take_repaint(&mut self) -> bool {
        std::mem::take(&mut self.repaint)
    }

    /// True while a change waits for its pause or a drag is on, so the loop wakes sooner.
    pub(crate) fn needs_fast_tick(&self) -> bool {
        !self.pending.is_empty() || self.drag.is_some()
    }

    /// The Yes answers given: the stress test compares them with the stops that were written.
    #[cfg(test)]
    pub(crate) fn yes_answers(&self) -> usize {
        self.yes
    }

    /// Whether any write is queued or running.
    #[cfg(test)]
    pub(crate) fn writes_in_flight(&self) -> bool {
        !self.jobs.is_empty()
    }

    pub(super) fn count_yes(&mut self) {
        self.yes += 1;
    }

    pub(super) fn request_quit(&mut self) {
        self.quit = true;
    }

    pub(super) fn request_repaint(&mut self) {
        self.repaint = true;
    }

    pub(super) fn emit(&mut self, effect: Effect) {
        self.effects.push(effect);
    }

    // ---- the pages

    pub(crate) fn ctx(&self) -> Ctx<'_> {
        Ctx { data: &self.data, kb: &self.kb, rear: &self.rear, version: &self.version }
    }

    pub(crate) fn current(&self) -> &Page {
        &self.pages[self.page]
    }

    pub(crate) fn row(&self, r: RowRef) -> &Row {
        &self.pages[r.0].rows[r.1]
    }

    pub(super) fn row_mut(&mut self, r: RowRef) -> &mut Row {
        &mut self.pages[r.0].rows[r.1]
    }

    /// Where the cursor is.
    pub(crate) fn cursor(&self) -> RowRef {
        (self.page, self.current().sel)
    }

    /// The value to show for a row: the one just chosen until the hardware caught up, else the
    /// latest reading.
    pub(crate) fn shown(&self, r: RowRef) -> Option<Value> {
        let row = self.row(r);
        row.shown(self.now, pages::reading(&row.id, &self.ctx()))
    }

    pub(crate) fn selectable(&self, r: RowRef) -> bool {
        pages::selectable(self.row(r), &self.ctx())
    }

    /// The selected row, if it can be acted on.
    pub(super) fn selected(&self) -> Option<RowRef> {
        let r = self.cursor();
        (r.1 < self.current().rows.len() && self.selectable(r)).then_some(r)
    }

    /// Moves the cursor to the next usable row in direction `d`, wrapping.
    pub(super) fn move_cursor(&mut self, d: i32) {
        let n = self.current().rows.len();
        if n == 0 {
            return;
        }
        let page = self.page;
        let mut i = self.current().sel;
        for _ in 0..n {
            i = (i + n).wrapping_add_signed(d as isize) % n;
            if self.selectable((page, i)) {
                self.pages[page].sel = i;
                return;
            }
        }
    }

    // ---- feedback

    /// Shows a line at the bottom. `here` ties it to the selected row: it is gone when you move to
    /// another row or page. What was done (a success or a failure) is also kept for "What you did".
    pub(super) fn say(&mut self, text: &str, st: St, secs: f64, here: bool) {
        self.msg = Message {
            text: text.to_owned(),
            st,
            until: Some(self.now + Duration::from_secs_f64(secs)),
            here: here.then(|| self.cursor()),
        };
        if !text.is_empty() && !text.starts_with('…') && matches!(st, St::Good | St::Bad) && !here {
            self.log.push(LogEntry { at: (self.wall)(), text: text.to_owned(), st });
            if self.log.len() > LOG_KEEP {
                self.log.remove(0);
            }
        }
        self.dirty = true;
    }

    /// The selection or the page changed: a hint of the old place goes away.
    pub(super) fn moved(&mut self) {
        if self.msg.here.is_some_and(|at| at != self.cursor()) {
            self.msg = Message { text: String::new(), st: St::Dim, until: None, here: None };
        }
    }

    /// Reads everything again: the dashboard now, and the settings of the open page.
    pub(super) fn refresh(&mut self) {
        self.emit(Effect::WakeFast);
        let reads = self.current().reads.clone();
        if !reads.is_empty() {
            self.emit(Effect::ReadSlow(reads));
        }
        self.dirty = true;
    }

    /// Opens a page (modulo the number of pages).
    pub(crate) fn goto_page(&mut self, i: isize) {
        let n = self.pages.len() as isize;
        self.page = usize::try_from(i.rem_euclid(n)).unwrap_or(0);
        self.frame.hits.clear();
        self.forget_light_edits_if_idle();
        self.moved();
        self.refresh();
    }

    /// Show what the hardware has, not what this screen remembers: once nothing is on its way,
    /// the light edits are dropped.
    pub(super) fn forget_light_edits_if_idle(&mut self) {
        if self.jobs.is_empty() && self.pending.is_empty() {
            self.kb = Edits::default();
            self.rear = Edits::default();
        }
    }

    // ---- writes

    /// Queues `op` behind earlier writes, says what is going on and reads everything again when it
    /// has been done. A write that fails puts the shown value back (and applies `undo`). The row
    /// defaults to the one last acted on.
    pub(super) fn run(&mut self, label: &str, op: Op, o: RunOpts) {
        let row = o.row.or(self.acting);
        let key = o
            .key
            .map_or_else(|| row.map_or_else(String::new, |(p, r)| format!("row:{p}:{r}")), str::to_owned);
        self.say(&format!("… {label}"), St::Dim, 30.0, false);
        if let Some(r) = row {
            self.row_mut(r).state = RowState::Writing;
        }
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        self.jobs.insert(ticket, Job { label: label.to_owned(), row, ok: o.ok, undo: o.undo });
        self.emit(Effect::Write(Write { ticket, key, op }));
    }

    fn written(&mut self, ticket: Ticket, result: Result<Done, String>) {
        let Some(job) = self.jobs.remove(&ticket) else { return };
        let (text, good) = match result {
            Err(e) => (format!("{}: {}", job.label, if e.is_empty() { "error" } else { &e }), false),
            Ok(done) => (self.ok_text(&job, done), true),
        };
        if !good {
            match job.undo {
                Some(Undo::Keyboard(before)) => self.kb = before,
                Some(Undo::Rear(before)) => self.rear = before,
                None => {}
            }
        }
        if let Some(r) = job.row {
            let now = self.now;
            self.row_mut(r).settle(good, now);
        }
        if good {
            self.say(&text, St::Good, HINT_SECONDS, false);
        } else {
            self.say(&text, St::Bad, ERROR_SECONDS, false);
        }
        self.refresh();
    }

    fn ok_text(&mut self, job: &Job, done: Done) -> String {
        match (&job.ok, done) {
            (OkText::Text(t), _) => t.clone(),
            (OkText::Repair, Done::Repair { fixed, changed, steps }) => {
                let steps = steps.join("; ");
                match (fixed, changed) {
                    (true, true) => format!("Sound output repaired: {steps}"),
                    (true, false) => "The sound output is there: nothing to repair".to_owned(),
                    _ => format!("Could not repair: {steps}"),
                }
            }
            (OkText::Dts, Done::Text(done)) => {
                let done = if done.is_empty() { "DTS is on for every output".to_owned() } else { done };
                format!("DTS: {done}")
            }
            (OkText::Doctor, Done::Doctor(rows)) => {
                self.data.set(ReadKey::Doctor, SlowValue::Doctor(rows));
                "Check finished".to_owned()
            }
            _ => job.label.clone(),
        }
    }

    fn dropped(&mut self, ticket: Ticket) {
        if let Some(job) = self.jobs.remove(&ticket) {
            if let Some(r) = job.row {
                let now = self.now;
                self.row_mut(r).settle(true, now);
            }
        }
        self.dirty = true;
    }

    // ---- changing a value: choose (shown at once) -> pause -> guard (ask) -> write (queued) -> settle

    pub(super) fn delay_for(&self, r: RowRef) -> Duration {
        match self.row(r).kind {
            super::row::Kind::Slider => DELAY_SLIDER,
            super::row::Kind::Choice => DELAY_CHOICE,
            _ => DELAY_DEFAULT,
        }
    }

    /// Shows `v` for the row at once and writes it after a short pause (so a run of key presses is
    /// one write), or immediately when `now` is true.
    pub(super) fn edit(&mut self, r: RowRef, v: Value, now: bool) {
        let delay = self.delay_for(r);
        self.edit_after(r, v, now, delay);
    }

    pub(super) fn edit_after(&mut self, r: RowRef, v: Value, now: bool, delay: Duration) {
        let at = self.now;
        self.row_mut(r).set_local(v, at);
        self.acting = Some(r);
        if now {
            self.commit(r);
            return;
        }
        self.row_mut(r).due = Some(at + delay);
        if !self.pending.contains(&r) {
            self.pending.push(r);
        }
        self.dirty = true;
    }

    /// Sends a chosen value, or asks first when the row has a question for it.
    pub(super) fn commit(&mut self, r: RowRef) {
        self.pending.retain(|p| *p != r);
        let value = self.row(r).local().cloned();
        self.row_mut(r).due = None;
        let Some(value) = value else { return };
        if self.row(r).state != RowState::Pending {
            return;
        }
        if let Some(question) = pages::guard(&self.row(r).id, &value) {
            if self.modal.is_some() {
                // One question at a time: this change waits until the dialog on screen is answered.
                let retry = self.now + DELAY_DEFAULT;
                self.row_mut(r).due = Some(retry);
                self.pending.push(r);
                return;
            }
            self.row_mut(r).state = RowState::Asking;
            self.modal = Some(Modal::Confirm { text: question, focus: 1, row: r, value });
            self.dirty = true;
            return;
        }
        self.send(r, value);
    }

    pub(super) fn send(&mut self, r: RowRef, v: Value) {
        self.row_mut(r).state = RowState::Writing;
        self.acting = Some(r);
        self.put(r, v);
    }

    /// Takes back every change that has not been sent yet (Esc).
    pub(super) fn undo_all(&mut self) {
        let mut n = 0;
        for page in &mut self.pages {
            for row in &mut page.rows {
                if matches!(row.state, RowState::Pending | RowState::Asking) {
                    row.forget();
                    n += 1;
                }
            }
        }
        self.pending.clear();
        if n > 0 {
            self.say("Change undone: nothing was sent", St::Dim, 3.0, false);
        }
    }

    /// Sends the changes whose pause is over; the loop calls it every cycle.
    fn tick(&mut self) {
        let now = self.now;
        for page in &mut self.pages {
            for row in &mut page.rows {
                row.expire(now);
            }
        }
        let pages = &self.pages;
        self.pending.retain(|r| pages[r.0].rows[r.1].state == RowState::Pending);
        let due: Vec<RowRef> =
            self.pending.iter().copied().filter(|r| self.row(*r).due.is_some_and(|d| now >= d)).collect();
        for r in due {
            self.commit(r);
        }
    }

    /// Called on quit: send what is still waiting. A question that was not answered is a No.
    /// The caller then waits for the writes to finish.
    pub(crate) fn flush(&mut self, now: Instant) -> Vec<Effect> {
        self.now = now;
        // A question still open is a No: nothing is sent for it.
        if let Some(Modal::Confirm { row, .. }) = self.modal.take() {
            self.row_mut(row).forget();
        }
        for r in self.pending.clone() {
            let value = self.row(r).local().cloned();
            if value.as_ref().is_some_and(|v| pages::guard(&self.row(r).id, v).is_some()) {
                self.row_mut(r).forget();
                self.pending.retain(|p| *p != r);
            } else {
                self.commit(r);
            }
        }
        std::mem::take(&mut self.effects)
    }
}

/// How [`Model::run`] differs from the default.
#[derive(Default)]
pub(super) struct RunOpts {
    pub row: Option<RowRef>,
    pub key: Option<&'static str>,
    pub ok: OkText,
    pub undo: Option<Undo>,
}
