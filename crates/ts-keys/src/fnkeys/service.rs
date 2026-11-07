//! The key service: finds the keyboard's Fn-key interface, reads its reports, resolves the
//! PREDATOR key's gestures and runs the action bound to each.
//!
//! What it must survive, because it runs for weeks in the user's session: the keyboard
//! disappearing and coming back (suspend and resume re-enumerate it), a config file that is
//! half-written or wrong, an action that fails, hangs or panics, and reports it does not
//! understand. None of them ends the service; each is logged, and the ones a person should see
//! are shown on screen. Only the stop flag ends it.
//!
//! One thread reads reports and keeps time; every action runs on a thread of its own so a slow
//! one (a profile that restarts the sound system) never stalls the key. Everything outside the
//! process (the device, the clock, the config, the actions, the screen, systemd) is behind a
//! trait or closure of [`Env`], so the whole loop runs in tests on a scripted report stream and a
//! fake clock.
#![forbid(unsafe_code)]

use std::fmt::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, UNIX_EPOCH};

use ts_api::api::{Api, ConfigFiles, NodeFinder, Notifier as Screen};
use ts_domain::actions::ActionRunner;
use ts_domain::keybinds::{self, ActionCheck, Bindings, Gesture, KeyEvent};
use ts_domain::perkeyfx::{FrameSink, FxStore, Runner as AnimationRunner, epoch_seconds};
use ts_foundation::sdnotify::{self, Notifier as Systemd};
use ts_foundation::wake;

use super::gestures::Gestures;
use super::hid::{HidError, HidOpener, HidReports, HidrawOpener};
use crate::renderer::Renderer;
use crate::watch::Watcher;

/// The icon of every message of this service.
const OSD_ICON: &str = "power";

/// Where the log lines go (standard output: the journal under systemd).
pub type Logger = Arc<dyn Fn(&str) + Send + Sync>;

/// Starts a job on its own thread; the error says why that was not possible.
pub type Spawner = Arc<dyn Fn(Box<dyn FnOnce() + Send>) -> Result<(), String> + Send + Sync>;

/// A loop that runs for as long as the service does (the per-key lighting animation): started
/// once, given the stop flag, and expected to return soon after it is set.
pub type Background = Box<dyn FnOnce(Arc<AtomicBool>) + Send>;

/// Time as the service sees it: a monotonic offset, and a wait.
pub trait Clock: Send + Sync {
    /// Time since an arbitrary start; only differences matter.
    fn now(&self) -> Duration;
    /// Waits (used while there is no keyboard to read from).
    fn sleep(&self, how_long: Duration);
}

/// The real clock.
#[derive(Debug, Clone, Copy)]
pub struct MonotonicClock {
    start: Instant,
}

impl MonotonicClock {
    /// A clock that starts now.
    pub fn new() -> Self {
        Self { start: Instant::now() }
    }
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for MonotonicClock {
    fn now(&self) -> Duration {
        self.start.elapsed()
    }

    fn sleep(&self, how_long: Duration) {
        // A ring of the process bell (the stop signal, a changed file) ends the wait early; the
        // loop looks at everything again either way.
        wake::sleep(how_long);
    }
}

/// Runs one action spec and answers the message to show (empty for none). Errors are the text the
/// person sees.
pub trait KeyActions: Send + Sync {
    /// Runs `spec`.
    fn run(&self, spec: &str) -> Result<String, String>;
}

/// The application's action runner as [`KeyActions`]: a key press and `truesense keys test` run
/// the same code.
pub struct RunnerActions(pub Arc<dyn ActionRunner>);

impl KeyActions for RunnerActions {
    fn run(&self, spec: &str) -> Result<String, String> {
        self.0.run(spec).map_err(|e| e.to_string())
    }
}

/// Where the bindings come from, and how to tell that they changed.
pub trait BindingSource: Send + Sync {
    /// A token that changes whenever the configuration may have changed (the modification time
    /// of the user's file); `None` when there is no file.
    fn stamp(&self) -> Option<u128>;
    /// The bindings in the configuration now; an error when it cannot be read.
    fn load(&self) -> Result<Bindings, String>;
}

/// The bindings of the configuration files. Only the user's file is watched, as the Python
/// edition did: the window and `truesense keys set` write there.
pub struct ConfigBindings(pub ConfigFiles);

impl BindingSource for ConfigBindings {
    fn stamp(&self) -> Option<u128> {
        let modified = std::fs::metadata(self.0.user.as_deref()?).ok()?.modified().ok()?;
        modified.duration_since(UNIX_EPOCH).ok().map(|d| d.as_nanos())
    }

    fn load(&self) -> Result<Bindings, String> {
        self.0.load().map(|cfg| keybinds::from_config(&cfg).bindings).map_err(|e| e.to_string())
    }
}

/// The service's own timings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopTiming {
    /// How often the config file is looked at (the Python edition: 1.5 s).
    pub config_poll: Duration,
    /// How long to wait before looking for the keyboard again after it went away.
    pub reconnect: Duration,
    /// The longest the loop waits without looking at the stop flag.
    pub max_wait: Duration,
    /// How many actions may run at once; further presses are dropped with a log line, so a hung
    /// action cannot pile up threads.
    pub max_actions: usize,
}

impl LoopTiming {
    /// The pace when a [`Watcher`] announces changes of the configuration: nothing needs asking
    /// on a short timer any more, so the loop sleeps until a report, a gesture timer, the
    /// watchdog, a changed file or (at the longest) `max_wait`, which only bounds how long a stop
    /// that did not interrupt the wait can go unnoticed. The slow `config_poll` is a safety net
    /// for a change that was not announced.
    pub fn watching() -> Self {
        Self { config_poll: Duration::from_secs(30), max_wait: Duration::from_secs(5), ..Self::default() }
    }
}

impl Default for LoopTiming {
    fn default() -> Self {
        Self {
            config_poll: Duration::from_millis(1500),
            reconnect: Duration::from_secs(2),
            max_wait: Duration::from_millis(500),
            max_actions: 8,
        }
    }
}

/// Everything the service reaches. [`Env::from_api`] fills it with the real ones; tests build it
/// field by field.
pub struct Env {
    /// Finds the keyboard's hidraw node (`Services::fn_key_node`).
    pub find_node: NodeFinder,
    /// Opens the node.
    pub opener: Arc<dyn HidOpener>,
    /// The bindings and whether they changed.
    pub bindings: Arc<dyn BindingSource>,
    /// Checks an action when it is bound, so a typo is `none` and never fails at press time.
    pub check: ActionCheck,
    /// Runs the actions.
    pub actions: Arc<dyn KeyActions>,
    /// Shows messages on screen.
    pub screen: Arc<dyn Screen>,
    /// The clock.
    pub clock: Arc<dyn Clock>,
    /// Starts action threads.
    pub spawn: Spawner,
    /// The log.
    pub log: Logger,
    /// systemd's notification socket (silent when not under systemd).
    pub systemd: Systemd,
    /// How often to ping the systemd watchdog, when one is armed.
    pub watchdog: Option<Duration>,
    /// The loop's timings.
    pub timing: LoopTiming,
    /// Loops that live as long as the service.
    pub background: Vec<Background>,
    /// Announces changes of the configuration and the user state (inotify), when it could be
    /// started; the loop then waits on it together with the keyboard.
    pub watch: Option<Arc<Watcher>>,
}

/// How long the two lighting loops sleep while nothing is running, when a watcher rings the bell
/// on every change of the user state (they used to look twice a second).
const QUIET_HEARTBEAT: Duration = Duration::from_secs(30);

/// The pause both lighting loops return while idle (the same 500 ms in both).
const IDLE_PAUSE: Duration = crate::renderer::IDLE;

/// The wait of a lighting loop: an idle pause becomes [`QUIET_HEARTBEAT`] when `watching` (a
/// change of the user state rings the bell and ends it at once, and so does a stop); every other
/// pause (a frame, a back-off) is kept.
pub fn lighting_sleep(watching: bool) -> impl Fn(Duration) + Send + 'static {
    move |pause| wake::sleep(if watching && pause == IDLE_PAUSE { QUIET_HEARTBEAT } else { pause })
}

impl Env {
    /// The real service environment over the application layer: the real hidraw node, the user's
    /// configuration files, the same action runner as `truesense keys test`, the on-screen
    /// notifier, systemd's socket and watchdog from the environment, the per-key animation
    /// loop (the Python edition started it inside this service) and the theme renderer.
    pub fn from_api(api: &Api) -> Env {
        let services = api.services();
        let sink: Arc<dyn FrameSink> = services.keyboard.clone();
        let fx = FxStore::new(Arc::clone(&services.user), Arc::clone(&sink), Arc::new(epoch_seconds));
        // One inotify thread for the files the loops read: the configuration (the key loop) and the
        // user state (the lighting loops). Without it (no inotify) everything polls as before.
        let watch = Watcher::start(&[
            services.user.path().to_path_buf(),
            services.config.user.clone().unwrap_or_default(),
        ])
        .ok()
        .map(Arc::new);
        let watching = watch.as_ref().is_some_and(|w| w.complete());
        let mut animation =
            AnimationRunner::new(move || fx.get(), sink, epoch_seconds, lighting_sleep(watching));
        // The themes: drawn while one is the current look, asleep (until the user state changes)
        // otherwise.
        let mut themes = Renderer::from_api(api, Box::new(lighting_sleep(watching)));
        Env {
            find_node: Arc::clone(&services.fn_key_node),
            opener: Arc::new(HidrawOpener),
            bindings: Arc::new(ConfigBindings(services.config.clone())),
            check: keybinds::syntax_check(),
            actions: Arc::new(RunnerActions(Arc::clone(&services.actions))),
            screen: Arc::clone(&services.notifier),
            clock: Arc::new(MonotonicClock::new()),
            spawn: thread_spawner(),
            log: stdout_logger(),
            systemd: Systemd::from_env(),
            watchdog: sdnotify::watchdog_interval_from_env(),
            timing: if watching { LoopTiming::watching() } else { LoopTiming::default() },
            watch,
            background: vec![
                Box::new(move |stop| animation.run(&stop)),
                Box::new(move |stop| themes.run(&stop)),
            ],
        }
    }
}

/// A [`Spawner`] that starts a detached thread per job.
pub fn thread_spawner() -> Spawner {
    Arc::new(|job| {
        std::thread::Builder::new()
            .name("key-action".to_owned())
            .spawn(job)
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
}

/// A [`Logger`] that prints each line on standard output.
pub fn stdout_logger() -> Logger {
    Arc::new(|line| println!("{line}"))
}

/// The key service over the real machine.
///
/// Runs until `stop` is set; never fails: a missing keyboard is waited for.
pub fn run(api: &Api, stop: Arc<AtomicBool>) {
    Service::new(Env::from_api(api)).run(&stop);
}

/// The connection to the keyboard.
enum Link {
    /// Not connected; look for the keyboard again at this time.
    Down { retry_at: Duration },
    /// Reading reports from a node.
    Up { reports: Box<dyn HidReports> },
}

/// The key service.
pub struct Service {
    env: Env,
    bindings: Bindings,
    gestures: Gestures,
    stamp: Option<u128>,
    link: Link,
    next_poll: Duration,
    next_watchdog: Option<Duration>,
    in_flight: Arc<AtomicUsize>,
    last_problem: Option<String>,
}

impl Service {
    /// Prepares the service: reads the bindings (a config that cannot be read means the defaults,
    /// and the service picks the file up when it is fixed) and replaces any binding that does not
    /// validate by `none`, so a typo in the config cannot break the key at press time.
    pub fn new(env: Env) -> Self {
        let stamp = env.bindings.stamp();
        let loaded = env.bindings.load().unwrap_or_else(|why| {
            (env.log)(&format!("key bindings not read ({why}); using the defaults"));
            Bindings::default()
        });
        let (mut bindings, warnings) = loaded.sanitised(&env.check);
        for warning in warnings {
            (env.log)(&warning);
        }
        normalise_timing(&mut bindings, &Bindings::default());
        let (long, multi) = timing_of(&bindings);
        Self {
            env,
            bindings,
            gestures: Gestures::new(long, multi),
            stamp,
            link: Link::Down { retry_at: Duration::ZERO },
            next_poll: Duration::ZERO,
            next_watchdog: None,
            in_flight: Arc::new(AtomicUsize::new(0)),
            last_problem: None,
        }
    }

    /// The bindings in force (after validation and reloads).
    pub fn bindings(&self) -> &Bindings {
        &self.bindings
    }

    /// Runs until `stop` is set. Tells systemd it is ready at once (waiting for the keyboard is
    /// part of running, and a keyboard that enumerates late must not fail the start), pings the
    /// watchdog while the loop is turning, and hands the background loops the stop flag.
    pub fn run(mut self, stop: &Arc<AtomicBool>) {
        let background = self.start_background(stop);
        self.report_systemd("READY", self.env.systemd.ready());
        self.report_systemd("STATUS", self.env.systemd.status("waiting for the keyboard"));
        let now = self.env.clock.now();
        self.next_poll = now.saturating_add(self.env.timing.config_poll);
        self.next_watchdog = self.env.watchdog.map(|every| now.saturating_add(every));
        self.link = Link::Down { retry_at: now };
        while !stop.load(Ordering::SeqCst) {
            self.step();
        }
        self.report_systemd("STOPPING", self.env.systemd.stopping());
        stop.store(true, Ordering::SeqCst);
        for handle in background {
            // A loop that was between its stop check and its sleep when the bell rang would sleep
            // out its pause: ring until it has ended (a few milliseconds, only while stopping).
            while !handle.is_finished() {
                wake::ring();
                std::thread::sleep(Duration::from_millis(5));
            }
            if handle.join().is_err() {
                self.log("a background loop panicked");
            }
        }
    }

    fn start_background(&mut self, stop: &Arc<AtomicBool>) -> Vec<JoinHandle<()>> {
        let mut handles = Vec::new();
        for job in std::mem::take(&mut self.env.background) {
            let stop = Arc::clone(stop);
            match std::thread::Builder::new().name("key-background".to_owned()).spawn(move || job(stop)) {
                Ok(handle) => handles.push(handle),
                Err(e) => self.log(&format!("could not start a background loop: {e}")),
            }
        }
        handles
    }

    /// One turn of the loop: connect if needed, run what is due, then wait for a report (or, with
    /// no keyboard, for time to pass).
    fn step(&mut self) {
        let now = self.env.clock.now();
        if let Link::Down { retry_at } = self.link
            && now >= retry_at
        {
            self.connect(now);
        }
        self.run_timers(now);
        let wait = self.next_wait(now);
        let read = match &mut self.link {
            Link::Up { reports } => Some(reports.read_report_watching(wait, self.env.watch.as_deref())),
            Link::Down { .. } => {
                self.env.clock.sleep(wait);
                None
            }
        };
        match read {
            Some(Ok(Some(report))) => self.handle_report(&report, self.env.clock.now()),
            Some(Err(e)) => self.disconnect(&e),
            // Woken by the watcher (or by the timeout): has the configuration changed? One `stat`.
            Some(Ok(None)) if self.env.watch.is_some() => self.reload_if_changed(),
            Some(Ok(None)) | None => {}
        }
        let now = self.env.clock.now();
        let due = self.gestures.advance(now);
        self.fire(&due);
    }

    /// Looks for the keyboard and opens it. A failure is logged once per kind, not every two
    /// seconds, and tried again after [`LoopTiming::reconnect`].
    fn connect(&mut self, now: Duration) {
        let retry = Link::Down { retry_at: now.saturating_add(self.env.timing.reconnect) };
        let Some(node) = (self.env.find_node)() else {
            self.note_problem("the Fn-key HID interface (usage page 0x88) was not found; waiting for it");
            self.link = retry;
            return;
        };
        match self.env.opener.open(&node) {
            Ok(reports) => {
                self.last_problem = None;
                self.link = Link::Up { reports };
                self.log(&format!("listening on {} {}", node.display(), self.bindings_text()));
                self.report_systemd(
                    "STATUS",
                    self.env.systemd.status(&format!("listening on {}", node.display())),
                );
            }
            Err(e) => {
                self.note_problem(&format!("{e}; waiting for it"));
                self.link = retry;
            }
        }
    }

    /// The keyboard went away or its stream failed: forget a press that is still down and look
    /// for the keyboard again soon.
    fn disconnect(&mut self, why: &HidError) {
        let now = self.env.clock.now();
        self.gestures.abandon_press(now);
        self.log(&format!("lost the keyboard: {why}; looking for it again"));
        self.report_systemd("STATUS", self.env.systemd.status("waiting for the keyboard"));
        self.last_problem = None;
        self.link = Link::Down { retry_at: now.saturating_add(self.env.timing.reconnect) };
    }

    fn run_timers(&mut self, now: Duration) {
        if let (Some(every), Some(at)) = (self.env.watchdog, self.next_watchdog)
            && now >= at
        {
            self.report_systemd("WATCHDOG", self.env.systemd.watchdog());
            self.next_watchdog = Some(now.saturating_add(every));
        }
        if now >= self.next_poll {
            self.reload_if_changed();
            self.next_poll = now.saturating_add(self.env.timing.config_poll);
        }
        let due = self.gestures.advance(now);
        self.fire(&due);
    }

    /// How long the loop may wait: until the next thing that is due, and never longer than
    /// [`LoopTiming::max_wait`], so the stop flag is looked at.
    fn next_wait(&self, now: Duration) -> Duration {
        let retry = match self.link {
            Link::Down { retry_at } => Some(retry_at),
            Link::Up { .. } => None,
        };
        let mut wake = now.saturating_add(self.env.timing.max_wait);
        for due in
            [self.gestures.deadline(), Some(self.next_poll), self.next_watchdog, retry].into_iter().flatten()
        {
            wake = wake.min(due);
        }
        wake.saturating_sub(now)
    }

    /// Routes one report. Anything that is not a vendor report is ignored without a word, a
    /// vendor report we do not know is logged (Fn+F11 and Fn+F12 change the brightness in the
    /// keyboard itself and are deliberately left alone).
    fn handle_report(&mut self, report: &[u8], now: Duration) {
        let Some(event) = keybinds::decode_report(report) else { return };
        match event {
            KeyEvent::PredatorDown => {
                let fired = self.gestures.press(now);
                self.fire(&fired);
            }
            KeyEvent::Release => {
                let fired = self.gestures.release(now);
                self.fire(&fired);
            }
            KeyEvent::Unknown => self.log(&format!("ignored vendor report {}", hex(report))),
            KeyEvent::FnEsc | KeyEvent::FnF10 => {
                if let Some((label, spec)) = self.bindings.action_for_event(event) {
                    self.dispatch(label, spec.to_owned());
                }
            }
        }
    }

    fn fire(&self, gestures: &[Gesture]) {
        for gesture in gestures {
            self.dispatch(gesture.key(), self.bindings.action(*gesture).to_owned());
        }
    }

    /// Runs the action on a thread of its own, so a slow one never stalls the key; its outcome is
    /// logged and shown by [`run_action`].
    fn dispatch(&self, label: &'static str, spec: String) {
        if self.in_flight.fetch_add(1, Ordering::SeqCst) >= self.env.timing.max_actions {
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            self.log(&format!(
                "{label} -> {spec} dropped: {} actions are still running",
                self.env.timing.max_actions
            ));
            return;
        }
        let guard = InFlight(Arc::clone(&self.in_flight));
        let actions = Arc::clone(&self.env.actions);
        let screen = Arc::clone(&self.env.screen);
        let log = Arc::clone(&self.env.log);
        let job = Box::new(move || {
            let _guard = guard;
            run_action(label, &spec, actions.as_ref(), screen.as_ref(), log.as_ref());
        });
        if let Err(why) = (self.env.spawn)(job) {
            self.log(&format!("{label}: could not start the action: {why}"));
        }
    }

    /// Picks up changes made in the window or with `truesense keys ...` without a restart: when
    /// the user's file has a new modification time, the bindings are read again. An action that no
    /// longer validates keeps its previous binding; a file that cannot be read keeps everything.
    fn reload_if_changed(&mut self) {
        let stamp = self.env.bindings.stamp();
        if stamp == self.stamp {
            return;
        }
        self.stamp = stamp;
        match self.env.bindings.load() {
            Err(why) => self.log(&format!("key bindings not reloaded: {why}")),
            Ok(fresh) => {
                let before = self.bindings.clone();
                self.bindings.reload(&fresh, &self.env.check);
                normalise_timing(&mut self.bindings, &before);
                let (long, multi) = timing_of(&self.bindings);
                self.gestures.set_timing(long, multi);
                self.log(&format!("key bindings reloaded: {}", self.bindings_text()));
            }
        }
    }

    fn bindings_text(&self) -> String {
        let mut text = String::new();
        for gesture in Gesture::ALL {
            let _ = write!(text, "{}='{}' ", gesture.key(), self.bindings.action(gesture));
        }
        let _ = write!(text, "long_ms={} multi_ms={}", self.bindings.long_ms, self.bindings.multi_ms);
        text
    }

    /// Logs a problem unless it is the same as the last one (a keyboard that stays away would
    /// otherwise fill the journal every two seconds).
    fn note_problem(&mut self, text: &str) {
        if self.last_problem.as_deref() != Some(text) {
            self.log(text);
            self.last_problem = Some(text.to_owned());
        }
    }

    fn report_systemd(&self, what: &str, result: Result<bool, sdnotify::NotifyError>) {
        if let Err(e) = result {
            self.log(&format!("systemd {what}: {e}"));
        }
    }

    fn log(&self, line: &str) {
        (self.env.log)(line);
    }
}

/// Counts an action as running until it is dropped (even when the job panics or never starts).
struct InFlight(Arc<AtomicUsize>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Runs one action and reports how it went: a message is shown, a failure is shown as
/// `TrueSense: <why>`, and nothing that happens inside (including a panic) escapes: the service
/// must outlive every action.
fn run_action(
    label: &str,
    spec: &str,
    actions: &dyn KeyActions,
    screen: &dyn Screen,
    log: &(dyn Fn(&str) + Send + Sync),
) {
    let outcome = catch_unwind(AssertUnwindSafe(|| match actions.run(spec) {
        Ok(message) => {
            log(&format!("{label} -> {spec} -> {}", if message.is_empty() { "ok" } else { &message }));
            if !message.is_empty() {
                screen.show(&message, OSD_ICON);
            }
        }
        Err(why) => {
            log(&format!("{label} -> {spec} FAILED: {why}"));
            screen.show(&format!("TrueSense: {why}"), OSD_ICON);
        }
    }));
    if outcome.is_err() {
        log(&format!("{label} -> {spec} FAILED: the action panicked"));
    }
}

/// A timing that is not positive would make every press a long press (or no tap ever count): it
/// keeps `fallback`'s value instead.
fn normalise_timing(bindings: &mut Bindings, fallback: &Bindings) {
    if bindings.long_ms <= 0 {
        bindings.long_ms = fallback.long_ms;
    }
    if bindings.multi_ms <= 0 {
        bindings.multi_ms = fallback.multi_ms;
    }
}

fn timing_of(bindings: &Bindings) -> (Duration, Duration) {
    let millis = |ms: i64| Duration::from_millis(u64::try_from(ms).unwrap_or(0));
    (millis(bindings.long_ms), millis(bindings.multi_ms))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
