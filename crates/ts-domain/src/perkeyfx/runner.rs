//! The loop that draws the animation: runs inside the key service, which already has the keyboard.
//!
//! It is idle (a cheap look at the settings twice a second) unless an animation is switched on,
//! draws about five frames a second while one is, and puts the base colours back when it is
//! switched off. Each frame is a full upload (the keyboard takes about 190 ms), so the animations
//! are smooth for slow effects, not for fast ones.
//!
//! Everything it depends on is injected (the settings source, the keyboard, the clock and the
//! wait), so the timing logic runs in tests without a keyboard or real time.
#![forbid(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::FrameSink;
use super::frames::{is_mode, render};
use super::hwkeys;
use super::settings::Settings;
use crate::keymap::Color;

/// The pause after each frame (the upload itself already took a good part of it).
pub const FRAME_INTERVAL: Duration = Duration::from_millis(200);
/// How often the settings are looked at while no animation is on.
pub const IDLE_INTERVAL: Duration = Duration::from_millis(500);
/// The extra pause after a failed upload (the keyboard may be re-enumerating after suspend).
pub const ERROR_BACKOFF: Duration = Duration::from_secs(1);
/// How often the keyboard's brightness is read again when nothing happened (a key the keyboard
/// handles itself makes the loop read it at once, see [`super::hwkeys`]).
pub const BRIGHTNESS_REFRESH_S: f64 = 5.0;
/// The brightness used until the keyboard has been asked.
const DEFAULT_BRIGHTNESS: i64 = 25;
/// How long one frame upload may still be on its way: a frame started this long before another
/// look was set may land after it, so that look is sent again.
const UPLOAD_S: f64 = 0.5;

/// How the loop sees that another look took the keyboard while an animation ran (a hardware
/// effect such as Wave, the power-mode colour, off): the animation then gives way instead of
/// drawing over it, and that look is sent again if one of the animation's frames went out after it.
pub struct LookWatch {
    /// The mode of the look last put on the keyboard (`keyboard::mode`) and when, in seconds since
    /// the epoch.
    pub look: Box<dyn Fn() -> (String, Option<f64>) + Send>,
    /// Sends that look again.
    pub replay: Box<dyn Fn() + Send>,
    /// Switches off the animation started at this time (and nothing newer).
    pub end: Box<dyn Fn(f64) + Send>,
}

/// Draws the animation described by the settings.
pub struct Runner {
    settings: Box<dyn Fn() -> Settings + Send>,
    sink: Arc<dyn FrameSink>,
    now: Box<dyn Fn() -> f64 + Send>,
    sleep: Box<dyn Fn(Duration) + Send>,
    running: bool,
    bright: i64,
    bright_at: f64,
    last: Option<(Vec<Color>, i64)>,
    /// The count of the keyboard's own lighting keys ([`hwkeys`]), and the one seen last.
    hw_keys: Box<dyn Fn() -> u64 + Send>,
    seen_keys: u64,
    /// Until when the keyboard's brightness is read before every frame (after such a key).
    follow_until: f64,
    /// Another look taking the keyboard (none: the animation only stops when switched off).
    watch: Option<LookWatch>,
    /// When the animation being drawn was started, and when its last frame upload began.
    started: f64,
    sent_at: f64,
}

impl Runner {
    /// A runner that reads its settings through `settings`, draws on `sink`, reads the wall clock
    /// (seconds since the epoch) from `now` and waits with `sleep`.
    pub fn new(
        settings: impl Fn() -> Settings + Send + 'static,
        sink: Arc<dyn FrameSink>,
        now: impl Fn() -> f64 + Send + 'static,
        sleep: impl Fn(Duration) + Send + 'static,
    ) -> Self {
        Self {
            settings: Box::new(settings),
            sink,
            now: Box::new(now),
            sleep: Box::new(sleep),
            running: false,
            bright: DEFAULT_BRIGHTNESS,
            bright_at: 0.0,
            last: None,
            hw_keys: Box::new(hwkeys::generation),
            seen_keys: hwkeys::generation(),
            follow_until: f64::NEG_INFINITY,
            watch: None,
            started: f64::NEG_INFINITY,
            sent_at: f64::NEG_INFINITY,
        }
    }

    /// Gives way to another look that takes the keyboard (see [`LookWatch`]).
    pub fn with_look_watch(mut self, watch: LookWatch) -> Self {
        self.watch = Some(watch);
        self
    }

    /// Takes the count of the keyboard's own lighting keys from `source` instead of the key
    /// service's ([`hwkeys::generation`]): for tests.
    pub fn with_hardware_keys(mut self, source: impl Fn() -> u64 + Send + 'static) -> Self {
        self.seen_keys = source();
        self.hw_keys = Box::new(source);
        self
    }

    /// Runs until `stop` is set. The base colours are handed back if it stops in the middle of an
    /// animation: a stop must not leave the last frame on the keyboard. (The Python edition did
    /// not; the Go edition does.)
    pub fn run(&mut self, stop: &AtomicBool) {
        while !stop.load(Ordering::SeqCst) {
            let pause = self.step();
            (self.sleep)(pause);
        }
        if self.running {
            let settings = (self.settings)();
            self.hand_back(&settings);
        }
    }

    /// One iteration of the loop; returns how long to wait before the next.
    pub fn step(&mut self) -> Duration {
        let settings = (self.settings)();
        let animating = is_mode(&settings.mode) && settings.base_colors().is_some_and(|b| !b.is_empty());
        if (animating || self.running) && self.given_way(&settings, animating) {
            return IDLE_INTERVAL;
        }
        match settings.base_colors() {
            Some(base) if animating => self.frame(&settings, &base),
            _ => {
                if self.running {
                    self.hand_back(&settings);
                }
                IDLE_INTERVAL
            }
        }
    }

    fn frame(&mut self, settings: &Settings, base: &[Color]) -> Duration {
        let now = (self.now)();
        let keys = (self.hw_keys)();
        if keys != self.seen_keys {
            self.seen_keys = keys;
            self.follow_until = now + hwkeys::FOLLOW_S;
        }
        let following = now < self.follow_until;
        if following || now - self.bright_at > BRIGHTNESS_REFRESH_S {
            // An unreadable keyboard keeps the last level, and so does a dark one found by the
            // periodic look (never animate into darkness), unless the person just turned the
            // light down to dark with the keyboard's own keys.
            if let Ok(b) = self.sink.brightness()
                && (b > 0 || following)
            {
                self.bright = i64::from(b);
            }
            self.bright_at = now;
        }
        let t = settings.t0.map_or(0.0, |t0| now - t0);
        let frame = render(&settings.mode, t, base, &settings.cells(), settings.speed);
        // An identical frame is not sent again (less USB traffic).
        let current = (frame, self.bright);
        self.started = settings.t0.unwrap_or(f64::NEG_INFINITY);
        if self.last.as_ref() != Some(&current) {
            self.sent_at = now;
            match self.sink.per_key_frame(&current.0, Some(current.1)) {
                Ok(()) => {
                    self.running = true;
                    self.last = Some(current);
                }
                Err(_) => {
                    self.last = None;
                    return ERROR_BACKOFF + FRAME_INTERVAL;
                }
            }
        }
        FRAME_INTERVAL
    }

    /// Whether another look took the keyboard after this animation started; if so the animation is
    /// switched off without putting its colours back (they would cover that look), and that look is
    /// sent again if a frame of the animation may have landed after it.
    fn given_way(&mut self, settings: &Settings, animating: bool) -> bool {
        let Some(watch) = self.watch.as_ref() else { return false };
        let since = if animating { settings.t0.unwrap_or(f64::NEG_INFINITY) } else { self.started };
        let (mode, at) = (watch.look)();
        let Some(at) = at.filter(|at| mode != crate::keyboard::mode::PERKEY && *at > since) else {
            return false;
        };
        if animating && let Some(t0) = settings.t0 {
            (watch.end)(t0);
        }
        if self.sent_at + UPLOAD_S >= at {
            (watch.replay)();
        }
        self.running = false;
        self.last = None;
        self.sent_at = f64::NEG_INFINITY;
        true
    }

    /// Puts the base colours back (or what is on the keyboard now, if there are none).
    fn hand_back(&mut self, settings: &Settings) {
        let cells =
            settings.base_colors().filter(|b| !b.is_empty()).unwrap_or_else(|| self.sink.current_cells());
        // Best effort: the keyboard may be away.
        let _ = self.sink.per_key_frame(&cells, None);
        self.running = false;
        self.last = None;
    }
}
