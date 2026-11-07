//! The terminal: raw mode, the alternate screen, the mouse, and putting everything back.
//!
//! This is the only place that switches the terminal into the full-screen state, so it is also the
//! only place that has to undo it: on a normal exit, on an error and on a panic of the screen's
//! own thread (a panic hook restores the terminal first, or the message would be printed into the
//! alternate screen and lost, and the shell left in raw mode).

use std::io::{self, Write};
use std::panic::{self, PanicHookInfo};
use std::sync::Arc;
use std::thread::{self, ThreadId};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};

/// The full-screen state. Dropping it puts the terminal back.
pub(crate) struct Screen {
    _private: (),
}

impl Screen {
    /// Raw mode, the alternate screen, the mouse (press, drag, release and wheel), no cursor.
    pub(crate) fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        if let Err(e) = execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture, Hide) {
            restore();
            return Err(e);
        }
        Ok(Self { _private: () })
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        restore();
    }
}

/// Puts the terminal back as the shell left it. Every step is tried even if an earlier one fails:
/// a half restored terminal is worse than one that reports nothing.
pub(crate) fn restore() {
    let mut out = io::stdout();
    let _ = execute!(out, DisableMouseCapture, Show, LeaveAlternateScreen);
    let _ = disable_raw_mode();
    let _ = out.flush();
}

type Hook = Box<dyn Fn(&PanicHookInfo<'_>) + Sync + Send + 'static>;

/// While alive, a panic of the thread that created it restores the terminal before the panic
/// message is printed. Panics of other threads (a write that crashed is contained and reported)
/// leave the screen alone.
pub(crate) struct PanicGuard {
    previous: Arc<Hook>,
}

impl PanicGuard {
    pub(crate) fn install() -> Self {
        let main: ThreadId = thread::current().id();
        let previous: Arc<Hook> = Arc::new(panic::take_hook());
        let chained = Arc::clone(&previous);
        panic::set_hook(Box::new(move |info| {
            if thread::current().id() == main {
                restore();
            }
            (*chained)(info);
        }));
        Self { previous }
    }
}

impl Drop for PanicGuard {
    fn drop(&mut self) {
        let _ = panic::take_hook();
        let previous = Arc::clone(&self.previous);
        panic::set_hook(Box::new(move |info| (*previous)(info)));
    }
}
