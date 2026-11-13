//! renderer: the host that draws a theme on the keyboard, frame by frame, inside the key service.
//!
//! The keyboard's own effects cannot show a theme (a designed palette, an effect with settings on
//! every key), so the key service draws it: the engine of `ts-fx` makes one colour per key, the
//! cell adapter turns that into the picture the keyboard takes, and the picture is uploaded with
//! [`FrameSink::per_key_frame`]. This module is the loop around that, and everything that makes
//! it safe to leave running for weeks:
//!
//! * **Only while a theme is the current look.** The running theme is a record in the user state
//!   ([`ThemeStore::active`]); the renderer looks at it twice a second and is otherwise asleep. A
//!   new record (a new `started`) restarts the theme; no record stops it.
//! * **Rate.** At most `DEFAULT_FPS` frames a second, and in practice fewer: an upload is eight
//!   64-byte reports 16 ms apart and a 50 ms settle (`ts_domain::keyboard::FRAME_SETTLE`), about
//!   180 ms, and `per_key_frame` blocks for all of it. Frames are never queued. Each one is drawn
//!   for the moment it is sent, so a slow upload simply skips the frames that would have been sent
//!   meanwhile, and the animation keeps its speed instead of falling behind.
//! * **Fails closed.** The first failed upload stops the theme: the static look is put back
//!   ([`KeyboardControl::replay`] of what the keyboard held before), the reason is recorded for
//!   `theme show`, and the theme is not drawn again until it is started again. There is no retry
//!   loop to spin on a keyboard that went away (suspend re-enumerates it), and nothing keeps
//!   writing to a device that refused a write.
//! * **Not against other commands.** When the keyboard's remembered look changes while a theme
//!   runs (the user set a colour, an effect, an animation), the theme steps aside the same way
//!   instead of overwriting the user's choice with its next frame.
//! * **Flashing.** The engine's guard (3 flashes a second, WCAG 2.3.1) is part of every frame;
//!   a theme may only be slowed down ([`ThemeSpec::speed_factor`] is at most 1), never sped up,
//!   because the guard counts in the theme's own time; and a theme whose flashing was never
//!   measured is refused when it is started.
//!
//! Everything outside the process (the theme store, the keyboard, the clock, the wait) is behind a
//! trait or closure, so the whole loop runs in tests on a fake keyboard and a fake clock.
#![forbid(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ts_api::api::Api;
use ts_domain::keyboard::{KeyboardError, State as KeyboardState};
use ts_domain::keymap::Color;
use ts_domain::looks::KeyboardControl;
use ts_domain::perkeyfx::{BRIGHTNESS_REFRESH_S, FrameSink, epoch_seconds, hwkeys};
use ts_domain::themes::{Active, Resolved, ThemeSpec, ThemeStore};
use ts_fx::cells::CellPainter;
use ts_fx::engine::{Effect, Input};

/// The frame rate cap unless told otherwise.
pub(crate) const DEFAULT_FPS: u32 = 15;
/// The highest cap accepted: the keyboard cannot take more, and a higher number would only
/// matter for a fake sink.
pub(crate) const MAX_FPS: u32 = 30;
/// How often the running-theme record is read while a theme is drawn.
pub const POLL: Duration = Duration::from_millis(500);
/// How long the loop sleeps while no theme runs.
pub(crate) const IDLE: Duration = Duration::from_millis(500);
/// The longest step of a theme's own clock: after a suspend or a stall the theme resumes where it
/// was instead of jumping.
const MAX_STEP: f64 = 0.5;
/// How long a frame upload may still be on its way (about 180 ms, with room): a frame begun this
/// long before another look was set may land after it.
const UPLOAD_S: f64 = 0.5;
/// The brightness used until the keyboard has been asked.
const DEFAULT_BRIGHTNESS: i64 = 25;
/// The shortest pause between two turns of the loop, so a sink that returns at once can never
/// make it spin.
const MIN_PAUSE: Duration = Duration::from_millis(1);

/// How long a keyboard that is not reachable yet (no permission on its node, the interface not
/// listed) is waited for before the theme gives way. At login the service can start a few seconds
/// before the session's access to the keyboard is in place; a theme that stopped for that would
/// never be there after a restart.
const KEYBOARD_GRACE_S: f64 = 60.0;

/// The pause between two attempts while the keyboard is not reachable.
const KEYBOARD_RETRY: Duration = Duration::from_secs(1);

/// What the renderer needs from the theme store.
pub trait Themes: Send {
    /// The theme that is running, if any.
    fn active(&self) -> Option<Active>;
    /// Checks a theme and its choices (the one gate: refuses what was never measured).
    fn resolve(&self, spec: &ThemeSpec) -> Result<Resolved, String>;
    /// Records that the theme started at `started` stopped by itself, and why.
    fn halt(&self, started: f64, reason: &str);
    /// Ends the theme started at `started` for good (the person replaced it), unless another one
    /// has been started since.
    fn end(&self, started: f64);
}

impl Themes for ThemeStore {
    fn active(&self) -> Option<Active> {
        ThemeStore::active(self)
    }

    fn resolve(&self, spec: &ThemeSpec) -> Result<Resolved, String> {
        ThemeStore::resolve(self, spec).map_err(|e| e.to_string())
    }

    fn halt(&self, started: f64, reason: &str) {
        ThemeStore::halt(self, started, reason);
    }

    fn end(&self, started: f64) {
        ThemeStore::end(self, started);
    }
}

/// Everything the renderer reaches. The key service builds the real one; tests build it with fakes.
pub struct Env {
    /// The running theme and the gate that checks it.
    pub themes: Box<dyn Themes>,
    /// The keyboard as the animation uses it.
    pub sink: Arc<dyn FrameSink>,
    /// The keyboard's remembered look, for putting it back and for noticing that it changed.
    pub keyboard: Arc<dyn KeyboardControl>,
    /// From the engine's keys to the keyboard's cells.
    pub painter: CellPainter,
    /// The wall clock, seconds since the epoch (the theme's `started` is on this clock).
    pub now: Box<dyn Fn() -> f64 + Send>,
    /// Hours of the local day, 0 to 24, for the themes that follow the clock; `None` where the
    /// local time is not known (they then show noon).
    pub local_hours: Box<dyn Fn() -> Option<f64> + Send>,
    /// The wait.
    pub sleep: Box<dyn Fn(Duration) + Send>,
    /// The log.
    pub log: Box<dyn Fn(&str) + Send>,
    /// The frame rate cap, `DEFAULT_FPS` normally.
    pub fps: u32,
    /// How many keys the keyboard handled by itself so far ([`hwkeys::generation`]).
    pub hw_keys: Box<dyn Fn() -> u64 + Send>,
}

impl Renderer {
    /// The renderer over the real machine: the user's theme store and user state, the keyboard
    /// the key service already holds, the cell numbering of the verified key map, the wall clock,
    /// the given wait and standard output as the log (the journal under systemd). The local hour is not
    /// known here, so a theme that follows the day shows noon.
    pub(crate) fn from_api(api: &Api, sleep: Box<dyn Fn(Duration) + Send>) -> Renderer {
        let services = api.services();
        Renderer::new(Env {
            themes: Box::new(ThemeStore::beside(services.config.user.as_deref(), Arc::clone(&services.user))),
            sink: services.keyboard.clone(),
            keyboard: services.keyboard.clone(),
            painter: CellPainter::new(services.keymap),
            now: Box::new(epoch_seconds),
            local_hours: Box::new(|| None),
            sleep,
            log: Box::new(|line| println!("{line}")),
            fps: DEFAULT_FPS,
            hw_keys: Box::new(hwkeys::generation),
        })
    }
}

/// A theme being drawn.
struct Run {
    /// The record this run is of; an equal record means nothing changed.
    active: Active,
    effect: Box<dyn Effect>,
    /// The theme's own clock, seconds.
    t: f64,
    /// The wall clock at the last frame.
    last: f64,
    speed: f64,
    /// The brightness the spec names, if it does.
    fixed_brightness: Option<i64>,
    bright: i64,
    bright_at: f64,
    /// What the keyboard remembered when the theme started, and when it was set; a different value
    /// or a later time means that something else set the keyboard (the same look chosen again is
    /// a choice too).
    baseline: KeyboardState,
    baseline_at: Option<f64>,
    last_frame: Option<(Vec<Color>, i64)>,
    /// When the last frame upload began (wall clock), to know whether it may cover a newer look.
    sent_at: f64,
    /// When the keyboard first could not be reached in the current wait (see `KEYBOARD_GRACE_S`).
    waiting_since: Option<f64>,
    /// The count of the keyboard's own lighting keys seen last, and until when the keyboard's
    /// brightness is followed before every frame after one.
    seen_keys: u64,
    follow_until: f64,
}

/// What a look at the record decided.
enum Next {
    Stay,
    Forget,
    Restore,
    /// Another look took the keyboard; `true` when a frame of the theme may have landed after it.
    GiveWay(Active, bool),
    Start(Active),
}

enum State {
    Idle,
    /// Stopped by itself at the theme that started at this time; left alone until a new start.
    Halted(f64),
    Run(Box<Run>),
}

/// Draws the theme that is running.
pub(crate) struct Renderer {
    env: Env,
    state: State,
    next_poll: f64,
    interval: Duration,
}

impl Renderer {
    /// A renderer over `env`. The frame rate is held between 1 and [`MAX_FPS`].
    pub(crate) fn new(env: Env) -> Self {
        let fps = env.fps.clamp(1, MAX_FPS);
        Self {
            env,
            state: State::Idle,
            next_poll: 0.0,
            interval: Duration::from_secs_f64(1.0 / f64::from(fps)),
        }
    }

    /// Runs until `stop` is set. A theme that is still running is taken off the keyboard first (the
    /// static look is put back): stopping the service must not leave the last frame lit.
    pub(crate) fn run(&mut self, stop: &AtomicBool) {
        while !stop.load(Ordering::SeqCst) {
            let pause = self.step();
            (self.env.sleep)(pause);
        }
        if matches!(self.state, State::Run(_)) {
            self.restore("the key service stopped");
            self.state = State::Idle;
        }
    }

    /// One turn of the loop; answers how long to wait before the next.
    pub(crate) fn step(&mut self) -> Duration {
        let now = (self.env.now)();
        // A running theme looks at the record twice a second; an idle renderer on every turn. A
        // clock that was set back also makes a look due.
        let stepped_back = now + POLL.as_secs_f64() < self.next_poll;
        if !matches!(self.state, State::Run(_)) || now >= self.next_poll || stepped_back {
            self.poll(now);
        }
        match self.state {
            State::Run(_) => self.frame(now),
            State::Idle | State::Halted(_) => IDLE,
        }
    }

    /// Reads the running-theme record and moves to the state it asks for.
    fn poll(&mut self, now: f64) {
        self.next_poll = now + POLL.as_secs_f64();
        let active = self.env.themes.active();
        let next = match (&self.state, active) {
            (State::Idle, None) => Next::Stay,
            (State::Halted(_), None) => Next::Forget,
            (State::Run(_), None) => Next::Restore,
            (State::Halted(started), Some(a)) if *started == a.started => Next::Stay,
            (State::Run(run), Some(a)) if run.active == a => {
                let set_at = self.env.keyboard.last_set_at();
                if self.env.keyboard.last() != run.baseline || set_at != run.baseline_at {
                    // The theme keeps drawing until this look (twice a second): frames uploaded
                    // after the new look was set cover it, so then it is sent again.
                    let covered = set_at.is_none_or(|at| run.sent_at + UPLOAD_S >= at);
                    Next::GiveWay(a, covered)
                } else {
                    Next::Stay
                }
            }
            (_, Some(a)) => Next::Start(a),
        };
        match next {
            Next::Stay => {}
            Next::Forget => self.state = State::Idle,
            Next::Restore => {
                self.state = State::Idle;
                self.restore("the theme was stopped");
            }
            Next::GiveWay(a, covered) => {
                self.give_way(&a, "the keyboard look was changed by another command");
                if covered {
                    self.put_back_the_new_look();
                }
            }
            Next::Start(a) => self.start(a, now),
        }
    }

    /// Begins drawing `active`, or halts it with the reason when it will not resolve.
    fn start(&mut self, active: Active, now: f64) {
        if matches!(self.state, State::Run(_)) {
            // Another theme replaces the one drawn: the keyboard is not handed back in between,
            // the next frame simply replaces the picture.
            self.state = State::Idle;
        }
        let resolved = match self.env.themes.resolve(&active.spec) {
            Ok(r) => r,
            Err(why) => return self.refuse(&active, &why),
        };
        let Some(effect) = resolved.effect() else {
            return self.refuse(&active, "the theme names an effect this build does not have");
        };
        let bright = resolved.brightness.unwrap_or(DEFAULT_BRIGHTNESS);
        (self.env.log)(&format!("theme {}: started", active.spec.name));
        self.state = State::Run(Box::new(Run {
            baseline: self.env.keyboard.last(),
            baseline_at: self.env.keyboard.last_set_at(),
            active,
            effect,
            t: 0.0,
            last: now,
            speed: resolved.speed.clamp(0.0, 1.0),
            fixed_brightness: resolved.brightness,
            bright,
            // Read the keyboard's own level on the first frame when the theme names none.
            bright_at: f64::NEG_INFINITY,
            last_frame: None,
            sent_at: f64::NEG_INFINITY,
            waiting_since: None,
            seen_keys: (self.env.hw_keys)(),
            follow_until: f64::NEG_INFINITY,
        }));
    }

    fn refuse(&mut self, active: &Active, why: &str) {
        (self.env.log)(&format!("theme {}: not started: {why}", active.spec.name));
        self.env.themes.halt(active.started, why);
        self.state = State::Halted(active.started);
    }

    /// Stops the running theme for `why` and puts the static look back.
    fn give_way(&mut self, active: &Active, why: &str) {
        (self.env.log)(&format!("theme {}: stopped: {why}", active.spec.name));
        self.env.themes.halt(active.started, why);
        // The person chose another look: the theme is over, not paused. Without this the record
        // stayed "running" and the next start of the key service (every login) drew it again over
        // the look they had chosen.
        self.env.themes.end(active.started);
        // The look that changed it is the keyboard's now: it is not replaced by the old one.
        self.state = State::Halted(active.started);
    }

    /// Sends the look that made the theme give way again, over the theme's last frames.
    fn put_back_the_new_look(&mut self) {
        let last = self.env.keyboard.last();
        if let Err(e) = self.env.keyboard.replay(&last) {
            (self.env.log)(&format!("the new keyboard look could not be sent again: {e}"));
        }
    }

    /// Hands the keyboard back to the static look it held before the theme. Best effort: the
    /// keyboard may be away.
    fn restore(&mut self, why: &str) {
        let last = self.env.keyboard.last();
        let outcome = self.env.keyboard.replay(&last);
        (self.env.log)(&match outcome {
            Ok(Some(what)) => format!("theme stopped ({why}): {what} restored"),
            Ok(None) => format!("theme stopped ({why}): nothing to restore"),
            Err(e) => format!("theme stopped ({why}): the static look could not be restored: {e}"),
        });
    }

    /// Draws and uploads one frame of the running theme; answers the pause before the next.
    fn frame(&mut self, now: f64) -> Duration {
        let State::Run(run) = &mut self.state else { return IDLE };
        let dt = (now - run.last).clamp(0.0, MAX_STEP);
        run.last = now;
        run.t += dt * run.speed;
        let keys = (self.env.hw_keys)();
        if keys != run.seen_keys {
            // The person used the keyboard's own brightness keys: from now on the level is theirs,
            // even when the theme named one.
            run.seen_keys = keys;
            run.follow_until = now + hwkeys::FOLLOW_S;
            run.fixed_brightness = None;
        }
        let following = now < run.follow_until;
        if run.fixed_brightness.is_none() && (following || now - run.bright_at > BRIGHTNESS_REFRESH_S) {
            // An unreadable keyboard keeps the last level, and so does a dark one found by the
            // periodic look (never draw into darkness), unless the person just turned the light
            // down to dark with the keyboard's own keys.
            if let Ok(b) = self.env.sink.brightness()
                && (b > 0 || following)
            {
                run.bright = i64::from(b);
            }
            run.bright_at = now;
        }
        let input = Input { clock: (self.env.local_hours)(), ..Input::default() };
        let picture = self.env.painter.paint(run.effect.frame(run.t, dt * run.speed, &input));
        let current = (picture, run.bright);
        let mut failure = None;
        // An identical picture is not sent again (a still theme costs no USB traffic).
        if run.last_frame.as_ref() != Some(&current) {
            run.sent_at = now;
            match self.env.sink.per_key_frame(&current.0, Some(current.1)) {
                Ok(()) => {
                    if run.waiting_since.take().is_some() {
                        (self.env.log)(&format!("theme {}: the keyboard is reachable", run.active.spec.name));
                    }
                    run.last_frame = Some(current);
                }
                Err(e @ (KeyboardError::Permission { .. } | KeyboardError::NotFound))
                    if run.waiting_since.is_none_or(|since| now - since < KEYBOARD_GRACE_S) =>
                {
                    if run.waiting_since.is_none() {
                        run.waiting_since = Some(now);
                        (self.env.log)(&format!(
                            "theme {}: waiting for the keyboard: {e}",
                            run.active.spec.name
                        ));
                    }
                    // Nothing was drawn: the same picture is tried again after the pause.
                    return KEYBOARD_RETRY;
                }
                Err(e) => failure = Some((run.active.clone(), e.to_string())),
            }
        }
        if let Some((active, why)) = failure {
            let reason = format!("the keyboard refused a frame: {why}");
            (self.env.log)(&format!("theme {}: stopped: {reason}", active.spec.name));
            self.env.themes.halt(active.started, &reason);
            self.state = State::Halted(active.started);
            self.restore(&reason);
            return IDLE;
        }
        // The upload took its own time (the sink blocks for it); only what is left of the frame
        // interval is waited, and none when the upload outlasted it: that frame is simply late and
        // the ones that would have followed are not drawn.
        let spent = Duration::from_secs_f64(((self.env.now)() - now).max(0.0));
        self.interval.saturating_sub(spent).max(MIN_PAUSE)
    }
}

#[cfg(test)]
mod tests;
