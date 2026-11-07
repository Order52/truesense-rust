//! tui: the terminal control surface (`truesense tui`): every setting of the laptop on seven pages,
//! usable over SSH and on a bare console.
//!
//! # Layers
//!
//! The screen is built so that each layer is testable without a terminal:
//!
//! * `model`: the state of the screen and one `update` function (message in, effects out). Pure:
//!   no I/O, no clock of its own, no handle to the laptop.
//! * `pages`, `row`, `light`, `writes`, `update`: what each setting reads, asks and writes.
//! * `view` and `dashboard`: the model drawn on a ratatui buffer, and what the last frame made
//!   clickable.
//! * `backend`: the only module that talks to `ts_api::api::Api`; everything else speaks in the
//!   small types of `data` and in `Op`s.
//! * `worker`: the threads (an ordered, coalescing writer, a dashboard reader, a page reader).
//! * `app` and `term`: the loop, and the terminal set-up and tear-down.
//!
//! # Rules the whole screen follows
//!
//! A value is never written that the user did not choose. Stopping a fan (or anything that needs a
//! restart) asks first and the answer defaults to No. Writes happen one at a time in order, a newer
//! write for the same setting replaces an older one that has not started, and a value that fails to
//! write goes back to what the hardware has.
#![forbid(unsafe_code)]

mod app;
mod backend;
mod dashboard;
mod data;
mod input;
mod light;
mod model;
mod pages;
mod row;
mod style;
mod term;
mod update;
mod view;
mod worker;
mod wrap;
mod writes;

#[cfg(test)]
mod tests;

use std::io::{self, IsTerminal};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ts_api::api::Api;

pub use backend::WindowOpener;
pub use model::WallClock;

use backend::{ApiBackend, Backend};
use model::{Model, Msg};
use style::Palette;

/// How to start the screen.
#[derive(Clone, Default)]
pub struct Options {
    /// Opens a page directly: a name (`fans`) or a number (`2`); empty is the dashboard.
    pub page: Option<String>,
    /// Forces the plain character set (the default follows the locale).
    pub ascii: bool,
    /// Starts the graphical window as a detached process (the "Open the window" button). Without
    /// it the button says the window cannot be opened from here.
    pub window: Option<WindowOpener>,
    /// The wall-clock time as `HH:MM:SS` for "What you did". The default is UTC: the screen does
    /// not know the time zone, the binary does.
    pub wall_clock: Option<WallClock>,
}

/// The names of the pages in tab order.
pub fn page_names() -> [&'static str; 7] {
    pages::PAGE_NAMES
}

/// The index of a page by name (any case) or by its number 1-7.
pub fn page_index(name: &str) -> Option<usize> {
    let name = name.trim();
    pages::PAGE_NAMES.iter().position(|n| n.eq_ignore_ascii_case(name)).or_else(|| {
        name.parse::<usize>().ok().filter(|n| (1..=pages::PAGE_NAMES.len()).contains(n)).map(|n| n - 1)
    })
}

/// Both ends must be a terminal: the screen reads keys and draws on them.
fn ensure_terminal(stdin_is_tty: bool, stdout_is_tty: bool) -> io::Result<()> {
    if stdin_is_tty && stdout_is_tty {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::Unsupported, "truesense tui needs a terminal"))
    }
}

/// Whether the locale says UTF-8 and the terminal is not the bare Linux console (whose font has no
/// box or block characters).
fn unicode_terminal(env: &dyn Fn(&str) -> Option<String>) -> bool {
    let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .find_map(|k| env(k).filter(|v| !v.is_empty()))
        .unwrap_or_default();
    let l = locale.to_lowercase();
    (l.contains("utf-8") || l.contains("utf8")) && env("TERM").as_deref() != Some("linux")
}

/// Colour unless `NO_COLOR` is set or the terminal says it cannot.
fn colour_terminal(env: &dyn Fn(&str) -> Option<String>) -> bool {
    env("NO_COLOR").is_none_or(|v| v.is_empty()) && env("TERM").as_deref() != Some("dumb")
}

fn utc_clock() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    format!("{:02}:{:02}:{:02}", secs / 3600 % 24, secs / 60 % 60, secs % 60)
}

/// Reads the terminal's events and forwards the ones the screen understands, until told to stop.
fn forward_events(tx: &mpsc::Sender<Msg>, done: &AtomicBool) {
    while !done.load(Ordering::SeqCst) {
        match crossterm::event::poll(Duration::from_millis(100)) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(_) => return,
        }
        let Ok(event) = crossterm::event::read() else { return };
        if let Some(input) = input::translate(event) {
            if tx.send(Msg::Input(input)).is_err() {
                return;
            }
        }
    }
}

/// Shows the terminal UI until the user quits. On the way out it sends the changes that are still
/// waiting and waits for the writes to finish.
///
/// Fails without touching the terminal when the page is unknown or stdin/stdout is not a terminal.
pub fn run(api: Arc<Api>, options: Options) -> io::Result<()> {
    let start = match options.page.as_deref().filter(|p| !p.trim().is_empty()) {
        Some(p) => Some(page_index(p).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unknown page {p:?} ({})", pages::PAGE_NAMES.join(", ").to_lowercase()),
            )
        })?),
        None => None,
    };
    ensure_terminal(io::stdin().is_terminal(), io::stdout().is_terminal())?;
    let env = |k: &str| std::env::var(k).ok();
    let unicode = !options.ascii && unicode_terminal(&env);
    let palette = Palette::new(colour_terminal(&env));
    let backend: Arc<dyn Backend> = Arc::new(ApiBackend::new(api, options.window));
    let wall = options.wall_clock.unwrap_or_else(|| Arc::new(utc_clock));
    let mut model = Model::new(&backend.catalog(), Api::version(), unicode, wall, Instant::now());

    let (tx, rx) = mpsc::channel();
    let runtime = app::Runtime::start(&backend, &tx);
    if let Some(page) = start {
        runtime.dispatch(&mut model, Msg::Goto(page));
    }
    let events = EventThread::start(tx);

    let _hook = term::PanicGuard::install();
    let result = {
        let _screen = term::Screen::enter()?;
        let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        app::event_loop(&mut terminal, &mut model, &runtime, &rx, palette)
    };
    drop(events);
    result
}

/// The thread that forwards the terminal's events; it is stopped and joined when dropped, however
/// `run` ends.
struct EventThread {
    done: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl EventThread {
    fn start(tx: mpsc::Sender<Msg>) -> Self {
        let done = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&done);
        Self { done, handle: Some(thread::spawn(move || forward_events(&tx, &flag))) }
    }
}

impl Drop for EventThread {
    fn drop(&mut self) {
        self.done.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}
