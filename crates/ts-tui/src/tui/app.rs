//! The loop that joins the pure screen to the world: it takes messages from one channel (keys and
//! clicks from the terminal, readings and results from the background threads), runs them through
//! the model, carries out the work the model asks for, and draws when the model changed.
//!
//! The loop is generic over the ratatui backend, so the tests run the very same loop on a text
//! buffer with the simulated laptop behind it.

use std::io;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::Backend as TermBackend;

use super::backend::Backend;
use super::model::{Effect, Model, Msg};
use super::style::Palette;
use super::view;
use super::worker::{Readers, Worker};

/// How long the loop waits for something to happen when nothing is going on.
const IDLE_TICK: Duration = Duration::from_millis(100);
/// While a change waits for its pause or a drag is on, the loop wakes sooner.
const ACTIVE_TICK: Duration = Duration::from_millis(30);
/// The screen is redrawn at least this often, so that a status line that has run out disappears.
const REDRAW: Duration = Duration::from_millis(250);
/// How long quitting waits for the writes that are still running.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(4);
/// The most messages handled before the screen is drawn again.
const BATCH: usize = 64;

/// The threads that do what the model asks.
pub(crate) struct Runtime {
    worker: Worker,
    readers: Readers,
}

impl Runtime {
    /// Starts the writer and the readers; whatever they find arrives on `results`.
    pub(crate) fn start(backend: &Arc<dyn Backend>, results: &Sender<Msg>) -> Self {
        Self {
            worker: Worker::new(Arc::clone(backend), results.clone()),
            readers: Readers::start(Arc::clone(backend), results.clone()),
        }
    }

    /// Runs a message through the model and does what it asks, including the follow-up messages
    /// that doing it produces (a write that replaced another tells the model so).
    pub(crate) fn dispatch(&self, model: &mut Model, msg: Msg) {
        let mut todo = vec![msg];
        while let Some(msg) = todo.pop() {
            let effects = model.update(Instant::now(), msg);
            todo.extend(self.apply(effects));
        }
    }

    fn apply(&self, effects: Vec<Effect>) -> Vec<Msg> {
        let mut follow = Vec::new();
        for effect in effects {
            match effect {
                Effect::Write(write) => {
                    follow.extend(self.worker.submit(write).into_iter().map(Msg::Dropped));
                }
                Effect::ReadSlow(keys) => self.readers.slow(keys),
                Effect::WakeFast => self.readers.wake(),
            }
        }
        follow
    }

    /// On the way out: send what is still waiting, then wait for the writes to finish (a change
    /// made a moment ago must not be lost).
    pub(crate) fn finish(&self, model: &mut Model) {
        let effects = model.flush(Instant::now());
        let _ = self.apply(effects);
        let _ = self.worker.drain(DRAIN_TIMEOUT);
        self.readers.stop();
    }

    #[cfg(test)]
    pub(crate) fn writes_idle(&self) -> bool {
        self.worker.idle()
    }
}

/// Runs the screen until the user quits or the channel closes. Whatever ends it, what was chosen
/// and is still waiting is sent and the running writes are waited for: a change made a moment ago
/// is not lost because the terminal went away.
pub(crate) fn event_loop<B: TermBackend>(
    terminal: &mut Terminal<B>,
    model: &mut Model,
    runtime: &Runtime,
    messages: &Receiver<Msg>,
    palette: Palette,
) -> io::Result<()> {
    let result = pump(terminal, model, runtime, messages, palette);
    runtime.finish(model);
    result?;
    // Leave the last state on the screen (the terminal is put back right after).
    if model.take_dirty() {
        draw(terminal, model, palette)?;
    }
    Ok(())
}

fn pump<B: TermBackend>(
    terminal: &mut Terminal<B>,
    model: &mut Model,
    runtime: &Runtime,
    messages: &Receiver<Msg>,
    palette: Palette,
) -> io::Result<()> {
    let mut last_draw: Option<Instant> = None;
    while !model.quit() {
        let now = Instant::now();
        if model.take_dirty() || last_draw.is_none_or(|t| now.duration_since(t) >= REDRAW) {
            if model.take_repaint() {
                terminal.clear()?;
            }
            draw(terminal, model, palette)?;
            last_draw = Some(now);
        }
        let wait = if model.needs_fast_tick() { ACTIVE_TICK } else { IDLE_TICK };
        match messages.recv_timeout(wait) {
            Ok(msg) => {
                runtime.dispatch(model, msg);
                for msg in messages.try_iter().take(BATCH) {
                    runtime.dispatch(model, msg);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        runtime.dispatch(model, Msg::Tick);
    }
    Ok(())
}

fn draw<B: TermBackend>(terminal: &mut Terminal<B>, model: &mut Model, palette: Palette) -> io::Result<()> {
    terminal.draw(|frame| {
        let area = frame.area();
        view::render(model, area, frame.buffer_mut(), palette);
    })?;
    Ok(())
}
