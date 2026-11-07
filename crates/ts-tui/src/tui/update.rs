//! Keys, mouse and dialogs: how the user's input changes the model.
//!
//! Every key is handled here and nowhere else, so the rule "Enter never writes a value the user did
//! not choose" and the rule "a stop asks first" each have exactly one place to be wrong.

use super::input::{Input, Key};
use super::light::Which;
use super::model::{DELAY_CYCLE_KEY, Drag, Hit, Modal, Model, RowRef, Zone};
use super::pages;
use super::row::{Kind, RowState, Value, choice_index, slider_index, start_index};
use super::style::St;

/// The longest text a prompt takes.
const PROMPT_MAX: usize = 24;

impl Model {
    pub(super) fn input(&mut self, input: Input) {
        self.touch();
        match input {
            Input::Key(k) => self.key(k),
            Input::Press { x, y } => self.press_at(x, y),
            Input::Wheel(d) => self.wheel(d),
            Input::Drag { x, .. } => self.drag_move(x),
            Input::Release => self.release(),
            Input::Resize => {}
            Input::Quit => self.request_quit(),
        }
    }

    fn key(&mut self, k: Key) {
        self.drag = None;
        if self.modal.is_some() {
            self.modal_key(k);
            return;
        }
        let before = self.cursor();
        let pages = self.pages.len();
        match k {
            Key::Char('q' | 'Q') => self.request_quit(),
            Key::Esc => self.undo_all(),
            Key::Char('?') => self.modal = Some(Modal::Help),
            Key::Tab => self.goto_page(self.page as isize + 1),
            Key::BackTab => self.goto_page(self.page as isize - 1),
            Key::Char(c @ '1'..='9') if (c as usize - '1' as usize) < pages => {
                self.goto_page((c as usize - '1' as usize) as isize);
            }
            Key::Up | Key::Char('k') => self.move_cursor(-1),
            Key::Down | Key::Char('j') => self.move_cursor(1),
            Key::Home => {
                let page = self.page;
                self.pages[page].sel = 0;
                if !self.selectable((page, 0)) {
                    self.move_cursor(1);
                }
            }
            Key::End => {
                let page = self.page;
                let last = self.current().rows.len().saturating_sub(1);
                self.pages[page].sel = last;
                if !self.selectable((page, last)) {
                    self.move_cursor(-1);
                }
            }
            Key::PageUp => (0..5).for_each(|_| self.move_cursor(-1)),
            Key::PageDown => (0..5).for_each(|_| self.move_cursor(1)),
            Key::Left | Key::Char('h') => self.adjust(-1),
            Key::Right | Key::Char('l') => self.adjust(1),
            Key::Enter | Key::Char(' ') => self.activate(),
            Key::Redraw => self.request_repaint(),
            Key::Char('r') => {
                self.forget_light_edits_if_idle();
                self.say("Reading everything again…", St::Dim, 2.0, false);
                self.refresh();
            }
            Key::Char('m') => self.cycle_mode(),
            _ => {}
        }
        if self.cursor() != before {
            self.moved();
        }
    }

    /// Steps to the next power mode from the value just chosen (not the last reading), so two
    /// quick presses go two steps.
    fn cycle_mode(&mut self) {
        let row: RowRef = (0, 0);
        let Some(current) = self.shown(row) else { return };
        let Some(at) = super::data::Mode::ALL.iter().position(|m| m.name() == current.as_str()) else {
            return;
        };
        let next = super::data::Mode::ALL[(at + 1) % super::data::Mode::ALL.len()];
        self.edit_after(row, Value::text(next.name()), false, DELAY_CYCLE_KEY);
    }

    /// Moves a choice or slider one step, or flips a toggle.
    fn adjust(&mut self, d: i32) {
        let Some(r) = self.selected() else { return };
        match self.row(r).kind {
            Kind::Toggle => {
                let on = self.shown(r).is_some_and(|v| v.as_bool());
                self.edit(r, Value::Bool(!on), true);
            }
            Kind::Choice => {
                let options = pages::options(self.row(r), &self.ctx());
                if options.is_empty() {
                    return;
                }
                let shown = self.shown(r);
                let index = choice_index(&options, shown.as_ref());
                let last = options.len() - 1;
                let target = match index {
                    Some(i) => i.saturating_add_signed(d as isize).min(last),
                    None if d < 0 => last,
                    None => 0,
                };
                if index != Some(target) {
                    self.edit(r, options[target].value.clone(), false);
                }
            }
            Kind::Slider => {
                let row = self.row(r);
                if row.values.is_empty() {
                    return;
                }
                let shown = self.shown(r);
                let index = slider_index(&row.values, row.exact0, shown.as_ref());
                let base = index.unwrap_or_else(|| start_index(&row.values, row.start));
                let target = base.saturating_add_signed(d as isize).min(row.values.len() - 1);
                if index != Some(target) {
                    let v = row.values[target];
                    self.edit(r, Value::Int(v), false);
                }
            }
            Kind::Header | Kind::Info | Kind::Button => {}
        }
    }

    /// Enter or Space: run a button, flip a toggle, or send a change that was made. On a choice or
    /// slider that was not changed it does nothing but say how to change it: Enter never writes a
    /// value the user did not choose.
    fn activate(&mut self) {
        let Some(r) = self.selected() else { return };
        match self.row(r).kind {
            Kind::Button => self.press(r),
            Kind::Toggle => {
                let on = self.shown(r).is_some_and(|v| v.as_bool());
                self.edit(r, Value::Bool(!on), true);
            }
            Kind::Choice | Kind::Slider => {
                if self.row(r).state == RowState::Pending {
                    self.commit(r);
                } else if self.unicode {
                    self.say("Use ← → to change this; it is applied by itself", St::Dim, 3.0, true);
                } else {
                    self.say("Use Left/Right to change this; it is applied by itself", St::Dim, 3.0, true);
                }
            }
            Kind::Header | Kind::Info => {}
        }
    }

    /// Runs a button: one at a time, and its result belongs to this row.
    pub(super) fn press(&mut self, r: RowRef) {
        if self.row(r).state == RowState::Writing {
            self.say("Still working on that…", St::Dim, 2.0, true);
            return;
        }
        self.acting = Some(r);
        self.button(r);
    }

    // ---- mouse

    fn wheel(&mut self, d: i32) {
        if self.modal.is_some() {
            return;
        }
        self.move_cursor(d);
        self.moved();
    }

    /// A press at column `x`, row `y`: act on it at once (no click delay).
    fn press_at(&mut self, x: i32, y: i32) {
        if self.modal.is_some() {
            self.modal_press(x, y);
            return;
        }
        let before = self.cursor();
        if y == 1 {
            let tab = self.frame.tabs.iter().find(|t| t.x0 <= x && x < t.x1).map(|t| t.page);
            if let Some(page) = tab {
                self.goto_page(page as isize);
            }
            return;
        }
        let hit: Option<Hit> =
            self.frame.hits.iter().rev().find(|h| h.y == y && h.x0 <= x && x < h.x1).copied();
        if let Some(h) = hit {
            // The frame is one frame old: a row it knows may not exist any more.
            if h.row.0 < self.pages.len() && h.row.1 < self.pages[h.row.0].rows.len() {
                self.click(h, x);
            }
        }
        if self.cursor() != before {
            self.moved();
        }
    }

    fn click(&mut self, h: Hit, x: i32) {
        let r = h.row;
        // The frame is one frame old: a row it drew may have become unusable (the service went
        // away, a list emptied) or belong to another page. Nothing is written for it.
        if r.0 != self.page || !self.selectable(r) {
            return;
        }
        let page = self.page;
        self.pages[page].sel = r.1;
        match h.zone {
            Zone::Row => {}
            Zone::Toggle => {
                let on = self.shown(r).is_some_and(|v| v.as_bool());
                self.edit(r, Value::Bool(!on), true);
            }
            Zone::Button => self.press(r),
            Zone::Option(i) => {
                // The list may have changed since the frame was drawn.
                let options = pages::options(self.row(r), &self.ctx());
                if let Some(o) = options.get(i) {
                    self.edit(r, o.value.clone(), true);
                }
            }
            Zone::Prev => self.adjust(-1),
            Zone::Next => self.adjust(1),
            Zone::Bar => {
                self.drag = Some(Drag { row: r, x0: h.x0, width: h.extra });
                self.drag_to(x, true);
            }
        }
    }

    fn drag_to(&mut self, x: i32, now: bool) {
        let Some(d) = self.drag else { return };
        let row = self.row(d.row);
        let n = row.values.len();
        if n == 0 {
            return;
        }
        let f = f64::from(x - d.x0) / f64::from((d.width - 1).max(1)) * (n - 1) as f64;
        let i = (f.round_ties_even().max(0.0) as usize).min(n - 1);
        let value = row.values[i];
        let shown = self.shown(d.row).and_then(|v| v.as_int());
        if shown == Some(value) && !self.row(d.row).busy() {
            return;
        }
        self.edit(d.row, Value::Int(value), now);
    }

    /// The pointer moved with the button held: keep dragging the slider that was pressed.
    fn drag_move(&mut self, x: i32) {
        if self.drag.is_some() && self.modal.is_none() {
            self.drag_to(x, false);
        }
    }

    /// The button came up: end a drag and send the position it ended on.
    fn release(&mut self) {
        if let Some(d) = self.drag.take() {
            if self.row(d.row).state == RowState::Pending {
                self.commit(d.row);
            }
        }
    }

    // ---- dialogs

    fn modal_key(&mut self, k: Key) {
        let Some(modal) = self.modal.clone() else { return };
        match modal {
            Modal::Help => self.modal = None,
            Modal::Confirm { focus, .. } => match k {
                Key::Char('y' | 'Y') => self.answer(true),
                Key::Char('n' | 'N' | 'q') | Key::Esc => self.answer(false),
                Key::Left | Key::Right | Key::Char('h' | 'l') | Key::Tab | Key::BackTab => {
                    if let Some(Modal::Confirm { focus, .. }) = &mut self.modal {
                        *focus = 1 - *focus;
                    }
                }
                Key::Enter | Key::Char(' ') => self.answer(focus == 0),
                _ => {}
            },
            Modal::Prompt { buf, which, .. } => match k {
                Key::Esc => self.modal = None,
                Key::Enter => {
                    self.modal = None;
                    self.submit_colour(which, &buf);
                }
                Key::Backspace => {
                    if let Some(Modal::Prompt { buf, fresh, .. }) = &mut self.modal {
                        buf.pop();
                        *fresh = false;
                    }
                }
                Key::Char(c) if (' '..'\u{7f}').contains(&c) => {
                    if let Some(Modal::Prompt { buf, fresh, .. }) = &mut self.modal {
                        // The first key replaces the suggested text; Backspace edits it instead.
                        if *fresh {
                            buf.clear();
                        }
                        *fresh = false;
                        if buf.chars().count() < PROMPT_MAX {
                            buf.push(c);
                        }
                    }
                }
                _ => {}
            },
        }
    }

    fn modal_press(&mut self, x: i32, y: i32) {
        match &self.modal {
            Some(Modal::Help) => self.modal = None,
            Some(Modal::Confirm { .. }) => {
                let hit = self.frame.modal.iter().find(|h| h.y == y && h.x0 <= x && x < h.x1).map(|h| h.yes);
                if let Some(yes) = hit {
                    self.answer(yes);
                }
            }
            _ => {}
        }
    }

    /// Closes a question: Yes sends the value, No forgets it and says nothing was changed.
    fn answer(&mut self, yes: bool) {
        let Some(Modal::Confirm { row, value, .. }) = self.modal.take() else { return };
        if yes {
            self.count_yes();
            self.send(row, value);
            return;
        }
        self.row_mut(row).forget();
        self.say("Cancelled: nothing was changed", St::Dim, 3.0, false);
    }

    fn submit_colour(&mut self, which: Which, text: &str) {
        let t = text.trim().trim_start_matches('#').to_lowercase();
        if super::light::is_hex_colour(&t) {
            let edit = super::light::Edits { color: Some(t), ..Default::default() };
            match which {
                Which::Keyboard => self.kb_put(edit),
                Which::Rear => self.rear_put(edit),
            }
        } else {
            self.say("A colour is six hex digits, like ff8800", St::Bad, super::model::HINT_SECONDS, false);
        }
    }
}
