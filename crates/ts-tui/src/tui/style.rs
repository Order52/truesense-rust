//! What the screen is drawn with: the character set, the named looks and the canvas that puts
//! text on a ratatui buffer.
//!
//! The drawing code speaks in names (`St::Good`, `St::Pill`), never in colours, so a test can say
//! "this value is drawn as good" and a terminal without colour (or with `NO_COLOR`) falls back to
//! bold, dim and reverse without the drawing code knowing. Plain ASCII glyphs keep a bare Linux
//! console, whose font has no box or block characters, usable.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use super::data::Mode;

/// A named look.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum St {
    Text,
    Dim,
    Accent,
    Good,
    Warn,
    Bad,
    Pill,
    Tab,
    TabSel,
    Title,
    Key,
    Head,
    On,
    Off,
    Bar,
    BarOff,
    /// A power mode's own colour.
    Mode(Mode),
    /// A power mode's colour as the marked option of its row.
    ModeSel(Mode),
}

/// The characters of the frame, the rails and the sparklines.
#[derive(Debug, Clone)]
pub(crate) struct Glyphs {
    pub h: &'static str,
    pub v: &'static str,
    pub tl: &'static str,
    pub tr: &'static str,
    pub bl: &'static str,
    pub br: &'static str,
    pub full: &'static str,
    pub empty: &'static str,
    pub knob: &'static str,
    pub rail: &'static str,
    pub thin: &'static str,
    pub dot: &'static str,
    pub up: &'static str,
    pub down: &'static str,
    pub bolt: &'static str,
    pub spark: Vec<char>,
    pub cursor: &'static str,
    pub waiting: &'static str,
    pub writing: &'static str,
    pub choice_open: &'static str,
    pub choice_close: &'static str,
    /// The arrow keys in the key line and in the help.
    pub up_down: &'static str,
    pub left_right: &'static str,
    pub help_up_down: &'static str,
    pub help_left_right: &'static str,
}

impl Glyphs {
    /// The Unicode set, or the plain ASCII one.
    pub(crate) fn new(unicode: bool) -> Self {
        if unicode {
            Self {
                h: "─",
                v: "│",
                tl: "╭",
                tr: "╮",
                bl: "╰",
                br: "╯",
                full: "█",
                empty: "░",
                knob: "●",
                rail: "━",
                thin: "─",
                dot: "●",
                up: "▲",
                down: "▼",
                bolt: "↯",
                spark: " ▁▂▃▄▅▆▇█".chars().collect(),
                cursor: "▸",
                waiting: "↵",
                writing: "…",
                choice_open: "‹ ",
                choice_close: " ›",
                up_down: "↑↓",
                left_right: "←→",
                help_up_down: "↑ ↓",
                help_left_right: "← →",
            }
        } else {
            Self {
                h: "-",
                v: "|",
                tl: "+",
                tr: "+",
                bl: "+",
                br: "+",
                full: "#",
                empty: ".",
                knob: "o",
                rail: "=",
                thin: "-",
                dot: "*",
                up: "^",
                down: "v",
                bolt: "AC",
                spark: " .:-=+*#%".chars().collect(),
                cursor: ">",
                waiting: ">",
                writing: "..",
                choice_open: "< ",
                choice_close: " >",
                up_down: "Up/Dn",
                left_right: "L/R",
                help_up_down: "Up Down",
                help_left_right: "Left Right",
            }
        }
    }
}

/// Maps names to terminal looks: colours when the terminal has them, bold, dim and reverse when
/// it does not (or `NO_COLOR` is set).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Palette {
    color: bool,
}

impl Palette {
    pub(crate) fn new(color: bool) -> Self {
        Self { color }
    }

    /// The terminal style of a name.
    pub(crate) fn style(&self, st: St) -> Style {
        let d = Style::default();
        let bold = d.add_modifier(Modifier::BOLD);
        let dim = d.add_modifier(Modifier::DIM);
        let reverse = d.add_modifier(Modifier::REVERSED);
        if !self.color {
            return match st {
                St::Text | St::Good | St::Tab => d,
                St::Dim | St::Off | St::BarOff => dim,
                St::Accent | St::Warn | St::Title | St::Key | St::Head | St::Bar | St::Mode(_) => bold,
                St::Bad => bold.add_modifier(Modifier::UNDERLINED),
                St::Pill | St::TabSel | St::ModeSel(_) => reverse.add_modifier(Modifier::BOLD),
                St::On => reverse,
            };
        }
        let on = |fg: Color, bg: Color| d.fg(fg).bg(bg);
        let mode_color = |m: Mode| match m {
            Mode::Eco => Color::Green,
            Mode::Quiet => Color::Blue,
            Mode::Balanced => Color::Cyan,
            Mode::Performance => Color::Yellow,
            Mode::Turbo => Color::Red,
        };
        match st {
            St::Text => d,
            St::Dim | St::Off | St::BarOff | St::Tab => dim,
            St::Accent | St::Title => d.fg(Color::Cyan).add_modifier(Modifier::BOLD),
            St::Good => d.fg(Color::Green),
            St::Warn => d.fg(Color::Yellow),
            St::Bad => d.fg(Color::Red).add_modifier(Modifier::BOLD),
            St::Pill | St::TabSel => on(Color::Black, Color::Cyan).add_modifier(Modifier::BOLD),
            St::Key | St::Bar => d.fg(Color::Cyan),
            St::Head => d.fg(Color::Magenta).add_modifier(Modifier::BOLD),
            St::On => on(Color::Black, Color::Green).add_modifier(Modifier::BOLD),
            St::Mode(m) => d.fg(mode_color(m)).add_modifier(Modifier::BOLD),
            St::ModeSel(m) => on(Color::Black, mode_color(m)).add_modifier(Modifier::BOLD),
        }
    }
}

/// How many cells a string takes (every glyph the screen uses is one cell wide).
pub(crate) fn width(s: &str) -> i32 {
    i32::try_from(s.chars().count()).unwrap_or(i32::MAX)
}

/// Cuts a string to `n` cells.
pub(crate) fn clip(s: &str, n: i32) -> String {
    if n <= 0 { String::new() } else { s.chars().take(usize::try_from(n).unwrap_or(0)).collect() }
}

/// Something text can be put on. Every call clips to the screen, so callers never worry about the
/// edges: the layout code below does arithmetic on a screen of any size, even one too small.
pub(crate) struct Canvas<'a> {
    buf: &'a mut Buffer,
    area: Rect,
    glyphs: &'a Glyphs,
    palette: Palette,
}

impl<'a> Canvas<'a> {
    pub(crate) fn new(buf: &'a mut Buffer, area: Rect, glyphs: &'a Glyphs, palette: Palette) -> Self {
        Self { buf, area, glyphs, palette }
    }

    /// Width and height of the drawing area.
    pub(crate) fn size(&self) -> (i32, i32) {
        (i32::from(self.area.width), i32::from(self.area.height))
    }

    /// Blanks the area.
    pub(crate) fn clear(&mut self) {
        let blank = Style::default();
        for y in self.area.top()..self.area.bottom() {
            for x in self.area.left()..self.area.right() {
                if let Some(cell) = self.buf.cell_mut((x, y)) {
                    cell.reset();
                    cell.set_style(blank);
                }
            }
        }
    }

    /// Puts `text` at row `y`, column `x` (relative to the area), cut at the edges.
    pub(crate) fn put(&mut self, y: i32, x: i32, text: &str, st: St) {
        let (w, h) = self.size();
        if y < 0 || y >= h || x >= w || text.is_empty() {
            return;
        }
        let skip = usize::try_from(-x.min(0)).unwrap_or(0);
        let text: String = text.chars().skip(skip).map(|c| if c.is_control() { ' ' } else { c }).collect();
        let x = x.max(0);
        let room = usize::try_from(w - x).unwrap_or(0);
        let (Ok(cx), Ok(cy)) = (u16::try_from(x), u16::try_from(y)) else { return };
        self.buf.set_stringn(self.area.x + cx, self.area.y + cy, &text, room, self.palette.style(st));
    }

    /// Paints `w` copies of `ch` from (`y`, `x`).
    pub(crate) fn fill(&mut self, y: i32, x: i32, w: i32, st: St, ch: &str) {
        if w > 0 {
            self.put(y, x, &ch.repeat(usize::try_from(w).unwrap_or(0)), st);
        }
    }

    /// A rounded frame at `(y, x, height, width)` with an optional title.
    pub(crate) fn frame(&mut self, at: (i32, i32, i32, i32), title: &str, st: St, title_st: St) {
        let (y, x, h, w) = at;
        let g = self.glyphs;
        if h < 2 || w < 4 {
            return;
        }
        let bar = g.h.repeat(usize::try_from(w - 2).unwrap_or(0));
        self.put(y, x, &format!("{}{}{}", g.tl, bar, g.tr), st);
        self.put(y + h - 1, x, &format!("{}{}{}", g.bl, bar, g.br), st);
        for i in 1..h - 1 {
            self.put(y + i, x, g.v, st);
            self.put(y + i, x + w - 1, g.v, st);
        }
        if !title.is_empty() {
            self.put(y, x + 2, &format!(" {title} "), title_st);
        }
    }
}
