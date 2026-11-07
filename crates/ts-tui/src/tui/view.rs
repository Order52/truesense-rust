//! Drawing the model on a ratatui buffer: header, tabs, the open page, the status line, the keys
//! and the dialogs. The dashboard panels are in `dashboard`.
//!
//! The view reads the model and writes only to the buffer, with two exceptions it hands back to
//! the model after the frame: which rectangles can be clicked (the mouse acts on what the last
//! frame showed) and how far the open page is scrolled (so the selected row stays visible).
//! Everything is clipped, and a screen smaller than 44x12 gets a short note instead of a squeezed
//! layout.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::model::{Frame, Hit, Modal, ModalHit, Model, RowRef, TabHit, Zone};
use super::pages;
use super::row::{Kind, Row, RowState, choice_index, slider_index};
use super::style::{Canvas, Glyphs, Palette, St, clip, width};
use super::wrap::wrap;

pub(crate) const MIN_WIDTH: i32 = 44;
pub(crate) const MIN_HEIGHT: i32 = 12;
/// From this width the tabs carry their names; below it only the open page's.
const WIDE_TABS: i32 = 82;

/// One line of a page: a row (or part of a wrapped row), or a blank between sections.
struct Line {
    row: Option<usize>,
    part: usize,
    text: String,
}

/// Draws the screen into `area` of `buf` and records what can be clicked and how far the page is
/// scrolled.
pub(crate) fn render(model: &mut Model, area: Rect, buf: &mut Buffer, palette: Palette) {
    let glyphs = Glyphs::new(model.unicode);
    let (frame, scroll) = {
        let mut p = Painter {
            m: model,
            cv: Canvas::new(buf, area, &glyphs, palette),
            g: &glyphs,
            frame: Frame::default(),
            scroll: None,
        };
        p.draw();
        (p.frame, p.scroll)
    };
    model.frame = frame;
    if let Some(s) = scroll {
        let page = model.page;
        model.pages[page].scroll = s;
    }
}

pub(super) struct Painter<'a, 'b> {
    pub(super) m: &'a Model,
    pub(super) cv: Canvas<'b>,
    pub(super) g: &'b Glyphs,
    frame: Frame,
    scroll: Option<usize>,
}

impl Painter<'_, '_> {
    fn draw(&mut self) {
        self.cv.clear();
        let (w, h) = self.cv.size();
        if w < MIN_WIDTH || h < MIN_HEIGHT {
            self.cv.put(0, 0, "TrueSense needs", St::Warn);
            self.cv.put(1, 0, "a 44x12 terminal", St::Warn);
            self.cv.put(2, 0, &format!("(this is {w}x{h})"), St::Dim);
            return;
        }
        self.header();
        self.page(2, h - 4, w);
        self.footer();
        if self.m.modal.is_some() {
            self.modal();
        }
    }

    fn header(&mut self) {
        let (w, _) = self.cv.size();
        let m = self.m;
        self.cv.put(0, 0, " TrueSense ", St::Title);
        self.cv.put(0, 11, &m.version, St::Dim);
        let mut right: Vec<(String, St)> = Vec::new();
        if let Some(b) = m.data.battery() {
            let pct = b.percent;
            let on_ac = m.data.ac() == Some(true);
            let text = if on_ac { format!("{}{pct} %", self.g.bolt) } else { format!("{pct} %") };
            right.push((text, if on_ac || pct > 30 { St::Good } else { St::Bad }));
        }
        if m.data.daemon() {
            right.push((format!("{} service", self.g.dot), St::Good));
        } else {
            right.push((format!("{} direct", self.g.dot), St::Warn));
        }
        let mut x = w - 1;
        for (text, st) in right.iter().rev() {
            x -= width(text);
            self.cv.put(0, x, text, *st);
            x -= 2;
        }
        let mut x = 1;
        let wide = w >= WIDE_TABS;
        for (i, page) in m.pages.iter().enumerate() {
            let label = if wide || i == m.page {
                format!(" {} {} ", i + 1, page.name)
            } else {
                format!(" {} ", i + 1)
            };
            let lw = width(&label);
            if x + lw >= w - 1 {
                break;
            }
            self.cv.put(1, x, &label, if i == m.page { St::TabSel } else { St::Tab });
            self.frame.tabs.push(TabHit { x0: x, x1: x + lw, page: i });
            x += lw + 1;
        }
    }

    /// The keys that matter for the selected row, most useful first.
    fn keys_for(&self, row: Option<RowRef>) -> Vec<(String, &'static str)> {
        let (ud, lr) = (self.g.up_down.to_owned(), self.g.left_right.to_owned());
        let kind = row.filter(|r| self.m.selectable(*r)).map(|r| self.m.row(r).kind);
        let mut keys = match kind {
            None => vec![(ud, "select")],
            Some(Kind::Slider) => vec![(ud, "select"), (lr, "adjust"), ("Enter".to_owned(), "apply now")],
            Some(Kind::Choice) => vec![(ud, "select"), (lr, "choose"), ("Enter".to_owned(), "apply now")],
            Some(Kind::Toggle) => vec![(ud, "select"), ("Space".to_owned(), "switch")],
            Some(_) => vec![(ud, "select"), ("Enter".to_owned(), "run")],
        };
        if self.m.current().rows.iter().any(|r| matches!(r.state, RowState::Pending | RowState::Asking)) {
            keys.push(("Esc".to_owned(), "undo"));
        }
        keys.push(("Tab".to_owned(), "page"));
        keys.push(("?".to_owned(), "help"));
        keys.push(("q".to_owned(), "quit"));
        keys
    }

    fn footer(&mut self) {
        let (w, h) = self.cv.size();
        let m = self.m;
        let page = m.current();
        let row = (!page.rows.is_empty()).then_some((m.page, page.sel));
        let line = |cv: &mut Canvas, t: &str, st: St| {
            let t = if width(t) > w - 2 { format!("{}…", clip(t, w - 3)) } else { t.to_owned() };
            cv.put(h - 2, 1, &t, st);
        };
        let live = !m.msg.text.is_empty() && m.msg.until.is_some_and(|u| m.now < u);
        if live {
            line(&mut self.cv, &m.msg.text, m.msg.st);
        } else if !m.data.error().is_empty() {
            line(&mut self.cv, &format!("No device: {}", m.data.error()), St::Warn);
        } else if let Some(r) = row.filter(|r| !m.row(*r).hint.is_empty()) {
            line(&mut self.cv, &m.row(r).hint, St::Dim);
        }
        let mut x = 1;
        for (key, text) in self.keys_for(row) {
            if x + width(&key) + width(text) + 3 > w {
                break;
            }
            self.cv.put(h - 1, x, &key, St::Key);
            self.cv.put(h - 1, x + width(&key) + 1, text, St::Dim);
            x += width(&key) + width(text) + 4;
        }
    }

    fn info_text(&self, r: RowRef) -> String {
        self.m.shown(r).map_or_else(|| "…".to_owned(), |v| pages::show(self.m.row(r), &v))
    }

    fn page(&mut self, y0: i32, hh: i32, w: i32) {
        let m = self.m;
        let page_i = m.page;
        let page = m.current();
        let ctx = m.ctx();
        let label_w = {
            let widest = page
                .rows
                .iter()
                .filter(|q| q.kind != Kind::Header)
                .map(|q| width(&q.label))
                .max()
                .unwrap_or(0)
                .max(12);
            (widest + 2).min(34).min(w / 2)
        };
        let mut lines: Vec<Line> = Vec::new();
        for (i, r) in page.rows.iter().enumerate() {
            if pages::hidden(&r.id, &ctx) {
                continue;
            }
            if r.kind == Kind::Header && !lines.is_empty() {
                lines.push(Line { row: None, part: 0, text: String::new() });
            }
            if r.kind == Kind::Info && r.wrap {
                let text = self.info_text((page_i, i));
                let room = usize::try_from((w - label_w - 8).max(10)).unwrap_or(10);
                let mut parts = wrap(&text, room);
                if parts.is_empty() {
                    parts.push(String::new());
                }
                for (k, text) in parts.into_iter().enumerate() {
                    lines.push(Line { row: Some(i), part: k, text });
                }
            } else {
                lines.push(Line { row: Some(i), part: 0, text: String::new() });
            }
        }
        let sel_line = lines.iter().position(|l| l.row == Some(page.sel) && l.part == 0).unwrap_or(0);
        let hh_n = usize::try_from(hh.max(0)).unwrap_or(0);
        let mut scroll = page.scroll;
        if sel_line < scroll {
            scroll = sel_line;
        }
        if sel_line >= scroll + hh_n {
            scroll = sel_line + 1 - hh_n;
        }
        scroll = scroll.min(lines.len().saturating_sub(hh_n));
        self.scroll = Some(scroll);

        for (y, line) in (y0..).zip(lines.iter().skip(scroll).take(hh_n)) {
            if let Some(i) = line.row {
                self.row((page_i, i), i == page.sel, y, w, label_w, line.part, &line.text);
            }
        }
        if lines.len() > hh_n {
            if scroll > 0 {
                self.cv.put(y0, w - 2, self.g.up, St::Dim);
            }
            if scroll + hh_n < lines.len() {
                self.cv.put(y0 + hh - 1, w - 2, self.g.down, St::Dim);
            }
        }
        if page.dashboard {
            let ey = y0 + i32::try_from(lines.len().min(hh_n)).unwrap_or(0) + 1;
            if y0 + hh - ey >= 5 {
                self.dashboard(ey, y0 + hh - ey, w);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn row(&mut self, r: RowRef, sel: bool, y: i32, w: i32, lw: i32, part: usize, text: &str) {
        self.row_body(r, sel, y, w, lw, part, text);
        let row = self.m.row(r);
        if row.state != RowState::Idle && !matches!(row.kind, Kind::Info | Kind::Header) && part == 0 {
            // Last, so nothing overwrites it: a change waiting for Enter, or being written.
            if matches!(row.state, RowState::Pending | RowState::Asking) {
                self.cv.put(y, w - 3, self.g.waiting, St::Warn);
            } else {
                self.cv.put(y, w - 3, self.g.writing, St::Dim);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn row_body(&mut self, r: RowRef, sel: bool, y: i32, w: i32, lw: i32, part: usize, text: &str) {
        let m = self.m;
        let row = m.row(r);
        let g = self.g;
        if row.kind == Kind::Header {
            self.cv.put(y, 1, &row.label, St::Head);
            let rule =
                format!(" {}", g.h.repeat(usize::try_from((w - width(&row.label) - 5).max(0)).unwrap_or(0)));
            self.cv.put(y, 2 + width(&row.label), &rule, St::Dim);
            return;
        }
        let selectable = m.selectable(r);
        let dis = row.kind != Kind::Info && !selectable;
        if part == 0 {
            if sel {
                self.cv.put(y, 0, g.cursor, St::Accent);
            }
            let st = if dis {
                St::Dim
            } else if sel {
                St::Accent
            } else {
                St::Text
            };
            self.cv.put(y, 2, &clip(&row.label, lw - 1), st);
        }
        let cx = 2 + lw + 1;
        let shown = m.shown(r);
        if row.kind == Kind::Info {
            let mut st = St::Text;
            if let Some(v) = &shown {
                st = pages::info_style(&row.id, v, &m.ctx());
            }
            let body = if row.wrap { text.to_owned() } else { self.info_text(r) };
            if shown.is_none() {
                st = St::Dim;
            }
            self.cv.put(y, cx, &clip(&body, (w - cx - 1).max(0)), st);
            return;
        }
        if dis {
            if row.kind == Kind::Button {
                self.cv.put(y, cx, &format!("[ {} ]", row.text), St::Dim);
            } else {
                self.cv.put(y, cx, "(not available)", St::Dim);
            }
            return;
        }
        self.frame.hits.push(Hit { y, x0: 0, x1: w, row: r, zone: Zone::Row, extra: 0 });
        match row.kind {
            Kind::Toggle => {
                let (s, st) = match &shown {
                    None => ("[  …  ]", St::Dim),
                    Some(v) if v.as_bool() => ("[ ON  ]", St::On),
                    Some(_) => ("[ OFF ]", St::Off),
                };
                self.cv.put(y, cx, s, st);
                self.frame.hits.push(Hit {
                    y,
                    x0: cx,
                    x1: cx + width(s),
                    row: r,
                    zone: Zone::Toggle,
                    extra: 0,
                });
            }
            Kind::Button => {
                let s = format!("[ {} ]", row.text);
                self.cv.put(y, cx, &s, if sel { St::Pill } else { St::Accent });
                self.frame.hits.push(Hit {
                    y,
                    x0: cx,
                    x1: cx + width(&s),
                    row: r,
                    zone: Zone::Button,
                    extra: 0,
                });
            }
            Kind::Choice => self.choice(r, row, shown.as_ref(), sel, y, w, cx),
            Kind::Slider => self.slider(r, row, shown.as_ref(), sel, y, w, cx),
            Kind::Header | Kind::Info => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn choice(
        &mut self,
        r: RowRef,
        row: &Row,
        shown: Option<&super::row::Value>,
        sel: bool,
        y: i32,
        w: i32,
        cx: i32,
    ) {
        let g = self.g;
        let options = pages::options(row, &self.m.ctx());
        let index = choice_index(&options, shown);
        let total: i32 = options.iter().map(|o| width(&o.label) + 2).sum::<i32>() + options.len() as i32 - 1;
        if total <= w - cx - 2 && !options.is_empty() {
            let mut x = cx;
            for (i, o) in options.iter().enumerate() {
                let st = if index == Some(i) {
                    if row.mode_colors { pages::mode_style(&o.value, true) } else { St::Pill }
                } else {
                    St::Dim
                };
                self.cv.put(y, x, &format!(" {} ", o.label), st);
                let ow = width(&o.label) + 2;
                self.frame.hits.push(Hit { y, x0: x, x1: x + ow, row: r, zone: Zone::Option(i), extra: 0 });
                x += ow + 1;
            }
            return;
        }
        let label = index.and_then(|i| options.get(i)).map_or("…", |o| o.label.as_str());
        let s = format!("{}{label}{}", g.choice_open, g.choice_close);
        self.cv.put(y, cx, &s, if sel { St::Pill } else { St::Accent });
        let sw = width(&s);
        self.frame.hits.push(Hit { y, x0: cx, x1: cx + 2, row: r, zone: Zone::Prev, extra: 0 });
        self.frame.hits.push(Hit { y, x0: cx + sw - 2, x1: cx + sw, row: r, zone: Zone::Next, extra: 0 });
    }

    #[allow(clippy::too_many_arguments)]
    fn slider(
        &mut self,
        r: RowRef,
        row: &Row,
        shown: Option<&super::row::Value>,
        sel: bool,
        y: i32,
        w: i32,
        cx: i32,
    ) {
        let g = self.g;
        let index = slider_index(&row.values, row.exact0, shown);
        let bw = (w - cx - 14).clamp(8, 36);
        let text = index
            .map_or_else(|| "…".to_owned(), |i| pages::show(row, &super::row::Value::Int(row.values[i])));
        let pos = index.map_or(0, |i| {
            round_even(i as f64 / (row.values.len().saturating_sub(1)).max(1) as f64 * f64::from(bw - 1))
        });
        for i in 0..bw {
            if index.is_some() && i <= pos {
                self.cv.put(y, cx + i, g.rail, St::Bar);
            } else {
                self.cv.put(y, cx + i, g.thin, St::BarOff);
            }
        }
        if index.is_some() {
            self.cv.put(y, cx + pos, g.knob, if sel { St::Accent } else { St::Text });
        }
        self.cv.put(y, cx + bw + 2, &text, if index.is_some() { St::Text } else { St::Dim });
        self.frame.hits.push(Hit { y, x0: cx, x1: cx + bw, row: r, zone: Zone::Bar, extra: bw });
    }

    fn modal(&mut self) {
        let (w, h) = self.cv.size();
        let Some(modal) = &self.m.modal else { return };
        match modal {
            Modal::Help => self.help(w, h),
            Modal::Confirm { text, focus, .. } => {
                let (bw, bh, y, x) = self.dialog(text, w, h);
                self.cv.frame((y, x, bh, bw), "Confirm", St::Warn, St::Accent);
                self.dialog_body(text, y, x, w);
                let (by, mut bx) = (y + bh - 2, x + 3);
                for (i, label) in ["Yes", "No"].iter().enumerate() {
                    let capt = format!("[ {label} ]");
                    self.cv.put(by, bx, &capt, if *focus == i { St::Pill } else { St::Dim });
                    self.frame.modal.push(ModalHit { y: by, x0: bx, x1: bx + width(&capt), yes: i == 0 });
                    bx += width(&capt) + 2;
                }
                self.cv.put(by, bx + 1, "y / n  ·  Enter picks the marked one", St::Dim);
            }
            Modal::Prompt { title, buf, .. } => {
                let (bw, bh, y, x) = self.dialog(title, w, h);
                self.cv.frame((y, x, bh, bw), "Type", St::Accent, St::Accent);
                self.dialog_body(title, y, x, w);
                self.cv.put(y + bh - 2, x + 3, &format!("> {buf}_"), St::Accent);
            }
        }
    }

    /// The size and place of a small dialog for `text`; it is blanked and ready to frame.
    fn dialog(&mut self, text: &str, w: i32, h: i32) -> (i32, i32, i32, i32) {
        let body = wrap(text, usize::try_from((w - 10).clamp(1, 60)).unwrap_or(1));
        let longest = body.iter().map(|b| width(b)).max().unwrap_or(0);
        let bw = (w - 4).min((longest + 6).clamp(24, 66));
        let bh = i32::try_from(body.len().max(1)).unwrap_or(1) + 5;
        let (y, x) = (((h - bh) / 2).max(1), (w - bw) / 2);
        for i in 0..bh {
            self.cv.fill(y + i, x, bw, St::Text, " ");
        }
        (bw, bh, y, x)
    }

    fn dialog_body(&mut self, text: &str, y: i32, x: i32, w: i32) {
        for (i, line) in wrap(text, usize::try_from((w - 10).clamp(1, 60)).unwrap_or(1)).iter().enumerate() {
            self.cv.put(y + 1 + i32::try_from(i).unwrap_or(0), x + 3, line, St::Text);
        }
    }

    fn help(&mut self, w: i32, h: i32) {
        let g = self.g;
        let keys: [(String, &str); 10] = [
            (format!("{}  j k", g.help_up_down), "move between settings"),
            (format!("{}  h l", g.help_left_right), "change the value (sent by itself after a short pause)"),
            ("Enter".to_owned(), "send the change now / run a button"),
            ("Space".to_owned(), "switch a toggle, or send the change"),
            ("Esc".to_owned(), "take back a change that was not sent yet"),
            ("Tab  1-7".to_owned(), "next page, or a page by number"),
            ("q".to_owned(), "quit"),
            ("m".to_owned(), "next power mode"),
            ("r".to_owned(), "read everything again"),
            ("Ctrl-L".to_owned(), "redraw the screen"),
        ];
        let notes = [
            "Mouse: click tabs, rows and options; drag a slider; the wheel moves the selection.",
            "Stopping a fan, or a setting that needs a restart, always asks first. The answer is No unless you choose Yes.",
            "The window has the rest: per-key colours, images, undervolting.",
        ];
        let bw = (w - 4).min(78);
        let inner = bw - 4;
        let kw = keys.iter().map(|k| width(&k.0)).max().unwrap_or(0);
        let kw = (kw + 2).min(inner / 2);
        struct HelpLine {
            key: String,
            text: String,
            col: i32,
        }
        let mut lines: Vec<HelpLine> = Vec::new();
        for (key, text) in &keys {
            let parts = wrap(text, usize::try_from((inner - kw).max(10)).unwrap_or(10));
            for (i, part) in parts.into_iter().enumerate() {
                lines.push(HelpLine {
                    key: if i == 0 { key.clone() } else { String::new() },
                    text: part,
                    col: kw,
                });
            }
        }
        lines.push(HelpLine { key: String::new(), text: String::new(), col: 0 });
        for note in notes {
            for part in wrap(note, usize::try_from(inner.max(1)).unwrap_or(1)) {
                lines.push(HelpLine { key: String::new(), text: part, col: 0 });
            }
            lines.push(HelpLine { key: String::new(), text: String::new(), col: 0 });
        }
        lines.pop();
        let bh = (i32::try_from(lines.len()).unwrap_or(0) + 4).min(h - 2);
        let (y, x) = (((h - bh) / 2).max(1), (w - bw) / 2);
        for i in 0..bh {
            self.cv.fill(y + i, x, bw, St::Text, " ");
        }
        self.cv.frame((y, x, bh, bw), "Keys", St::Accent, St::Accent);
        for (i, l) in lines.iter().enumerate() {
            let i = i32::try_from(i).unwrap_or(0);
            if i >= bh - 4 {
                break;
            }
            if !l.key.is_empty() {
                self.cv.put(y + 2 + i, x + 2, &l.key, St::Key);
            }
            self.cv.put(y + 2 + i, x + 2 + l.col, &l.text, if l.col != 0 { St::Text } else { St::Dim });
        }
    }
}

/// Rounds half to even, as the layout was tuned with.
pub(super) fn round_even(f: f64) -> i32 {
    f.round_ties_even() as i32
}
