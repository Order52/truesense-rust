//! One guarded tick of the control loop.
//!
//! A tick reads the sensors, smooths them, classifies the activity and then hands the numbers
//! to the engines of `ts-decision`: the smart fan, the thermal governor, the automatic power
//! mode, the RAPL limit enforcement. This file only does that wiring and carries out what the
//! engines answer.
//!
//! The tick runs under [`Daemon::guarded_step`]: a panic anywhere in it is caught, logged, and
//! if the machine is hot (or its temperature is unknown) the fans are driven to 100 %, because
//! a fan stuck low under load is the worst outcome this program can cause. The state lock does
//! not stay held (see [`Daemon::lock`]), so that fallback, the control socket and the shutdown
//! keep working.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::thread;
use std::time::Duration;

use serde_json::Value;
use ts_decision::autoprofile::{self, AutoKind, Readings};
use ts_decision::chargefull;
use ts_decision::fanctl::{self, Duties, Temps};
use ts_decision::gamerules;
use ts_decision::smartprofile::Mode;
use ts_domain::power::RawLimits;
use ts_platform::hw::{Coded, FanDuty, PowerMode, Reading};
use ts_system::sensors::{Ema, GPU_FRESH_S, GpuFeed, GpuReading};

use super::alerts::Observed;
use super::daemon::{Daemon, Inner, TEMP_ALPHA};
use ts_foundation::sync::locked;

/// A step that panics while the CPU is at or above this temperature (or unknown) puts the fans
/// on full.
const EMERGENCY_CPU_C: f64 = 80.0;
/// The wall clock ran this many seconds more than the monotonic one between two ticks: the
/// machine was suspended, and the keyboard light must be put back.
const RESUME_GAP_S: f64 = 5.0;
/// The longest user pick (power mode held against the automatic one) that makes sense. A stored
/// end time further away than this is a clock that jumped back (or a client asking for years):
/// the hold is dropped instead of pinning the power mode for weeks.
pub(super) const MAX_PLAUSIBLE_HOLD_S: f64 = 7.0 * 86_400.0;
/// A settings write that failed (full or read-only disk) is tried again every this many ticks.
const STATE_RETRY_EVERY: u64 = 30;
/// The charge-to-full record is looked at every this many ticks.
const CHARGE_CHECK_EVERY: u64 = chargefull::CHECK_EVERY_TICKS;
/// The power managers are looked at when `tick % 150` is 5 (about every 5 minutes).
const PM_GUARD_EVERY: u64 = 150;
const PM_GUARD_PHASE: u64 = 5;
/// The fan speeds are read from the firmware on the first tick and then every this many ticks;
/// the ticks in between ask for the temperatures only (3 firmware calls instead of 5). The speeds
/// are shown, never decided on, so they may be a few seconds old.
const RPM_EVERY: u64 = 5;
/// The smart fan follower always moves by one second per step, as the Python daemon did, whatever
/// the real pace of the loop (it ticks every one or two seconds).
const FOLLOWER_DT_S: f64 = 1.0;

/// The text of a panic payload.
pub(super) fn panic_text(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "a panic without a message".to_owned()
    }
}

/// What the fan controller calls the activity: the same five words as the profile engine's
/// classification. (They are two types in two crates that must not depend on each other, and the
/// orphan rule forbids a `From` between them here, hence a function.)
pub(super) fn fan_activity(a: autoprofile::Activity) -> fanctl::Activity {
    match a {
        autoprofile::Activity::Idle => fanctl::Activity::Idle,
        autoprofile::Activity::Browsing => fanctl::Activity::Browsing,
        autoprofile::Activity::Heavy => fanctl::Activity::Heavy,
        autoprofile::Activity::Gaming => fanctl::Activity::Gaming,
        autoprofile::Activity::Hot => fanctl::Activity::Hot,
    }
}

/// The readings of one tick.
pub(super) struct Frame {
    pub(super) wall: f64,
    pub(super) mono: f64,
    pub(super) tcpu: f64,
    /// The GPU temperature as really read, `None` while neither the firmware nor `nvidia-smi`
    /// gives one (the card asleep, no stream). This is what is shown.
    pub(super) tgpu_seen: Option<f64>,
    /// What a 0 GPU load or power means (a sleeping card, a live reading, a card nobody reads).
    pub(super) gpu_state: GpuFeed,
    pub(super) chassis: Option<f64>,
    pub(super) load: f64,
    pub(super) gpu: GpuReading,
    pub(super) ac: bool,
}

/// `round()` as Python does it (halves go to the even number), for the journal and the live state.
pub(super) fn rounded(v: f64) -> i64 {
    v.round_ties_even() as i64
}

impl Daemon {
    /// Runs one tick and never lets a failure out: a panic is caught and logged, and when the
    /// machine is hot (or its temperature is still unknown) the fans go to 100 %.
    pub fn guarded_step(&self) {
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| self.step())) {
            self.step_failed(&panic_text(payload.as_ref()));
        }
    }

    fn step_failed(&self, why: &str) {
        self.warn(format!("step failed: {why}"));
        let hot = self.lock().tcpu.value().is_none_or(|t| t >= EMERGENCY_CPU_C);
        if hot {
            // The firmware's own "max" mode is the fallback when the manual write is refused.
            let manual = FanDuty::new(100).and_then(|duty| self.hw.fan_manual(duty, duty));
            if manual.is_err() {
                let _ = self.hw.fan_max();
            }
        }
        // Whatever the writer believed about the fans may no longer be true.
        self.lock().writer.forget();
    }

    /// One tick: the watchdogs first, then the readings and decisions, then the periodic jobs.
    fn step(&self) {
        let wall = self.clock.wall();
        let mono = self.clock.mono();
        self.fan_watchdog();
        let resumed = {
            let mut g = self.lock();
            g.gpu_uv_guard.tick(mono);
            g.uv_guard.tick(mono); // the boot undervolt survived ten minutes: its crash-guard flag goes
            // The wall clock ran and the monotonic one did not: the machine was suspended.
            let resumed = g.last_clock.is_some_and(|(w, m)| (wall - w) - (mono - m) > RESUME_GAP_S);
            g.last_clock = Some((wall, mono));
            if g.engine.hold_left_s(wall) as f64 > MAX_PLAUSIBLE_HOLD_S {
                g.engine.restore_hold(0.0);
                self.save("profile_hold_until", hold_value(0.0));
                self.warn("the held power-mode pick ended later than any pick can: the clock jumped back, so it is dropped");
            }
            if resumed {
                // The firmware may have come back with its own fan and mode state: what was
                // written before the sleep is no longer known to hold, so the next decision
                // writes again (one write, exactly as after a plug).
                g.writer.forget();
                g.engine.resync();
                let tick = g.tick;
                g.governor.resync(tick);
            }
            resumed
        };
        if resumed {
            self.apply_boot_look("resume from suspend");
        }
        let (smart, tick) = {
            let mut g = self.lock();
            self.tick_locked(&mut g, wall, mono);
            let smart = self.smart_active() || self.auto_kind() == AutoKind::Smart;
            // Adaptive pace: once a second while something smart is working or the laptop is
            // busy, every two seconds when nothing smart is on or it has been calm for a while.
            let busy = smart && (g.activity != autoprofile::Activity::Idle || g.fan.calm_steps() < 30);
            g.interval = Duration::from_secs(if busy || g.stop_watch.is_armed() { 1 } else { 2 });
            (smart, g.tick)
        };
        if smart {
            self.sensors.ensure_gpu_stream();
        } else {
            // Nothing needs the GPU readings: let the card sleep.
            self.sensors.release_gpu_stream();
        }
        if tick % STATE_RETRY_EVERY == 0 {
            self.retry_state_write();
        }
        if tick % CHARGE_CHECK_EVERY == 0 {
            self.charge_full_check(wall);
        }
        if tick % PM_GUARD_EVERY == PM_GUARD_PHASE && self.state.get_bool("pm_takeover", false) {
            self.pm_guard();
        }
        self.flush_notices();
        self.alerts_deliver();
    }

    /// Writes the settings again when the last write failed. The settings the daemon runs on are
    /// in memory, so this only decides when a reboot would still find them.
    fn retry_state_write(&self) {
        match self.state.flush() {
            Ok(true) => self.info("the settings are written to disk again"),
            Ok(false) => {}
            Err(e) => self.warn_every("save", 60.0, format!("could not save the settings (will retry): {e}")),
        }
    }

    /// Shows what the tick wanted to say, after the lock is released (the on-screen display runs
    /// helper programs).
    fn flush_notices(&self) {
        let notices = std::mem::take(&mut self.lock().notices);
        for (text, icon) in notices {
            self.say(&text, icon);
        }
    }

    /// Queues an on-screen message from code that holds the lock.
    pub(super) fn notice(&self, g: &mut Inner, text: String, icon: &'static str) {
        if self.cfg.profile.osd {
            g.notices.push((text, icon));
        }
    }

    /// A fan the user stopped goes back to the firmware, all of them, when the laptop gets warm
    /// or the device stops answering.
    fn fan_watchdog(&self) {
        let (returned, warm) = {
            let mut g = self.lock();
            let temps = Temps { cpu: g.tcpu.value(), gpu: g.tgpu.value() };
            let hw_ok = g.hw_ok;
            let warm = fanctl::must_return_fans(temps, true);
            (g.stop_watch.check(temps, hw_ok), warm)
        };
        if !returned {
            return;
        }
        // The reason is told as it is: a silent device is not a warm laptop.
        let why = if warm { "the laptop got warm" } else { "the device stopped answering" };
        if let Err(e) = self.hw.fan_auto() {
            // The fans are still stopped: keep watching, so the next tick tries again as soon as
            // the device answers instead of leaving a stopped fan with no watchdog.
            self.warn_every(
                "fans-back-failed",
                10.0,
                format!("could not hand the fans back to the firmware: {e}"),
            );
            self.lock().stop_watch.arm();
            return;
        }
        self.lock().writer.forget();
        self.warn_every("fans-back", 1.0, format!("fans back on Auto: {why}"));
        self.say(&format!("Fans back on Auto: {why}"), "fan");
    }

    /// The body of a tick, with the lock held.
    fn tick_locked(&self, g: &mut Inner, wall: f64, mono: f64) {
        g.tick += 1;
        let load = self.sensors.cpu_load();
        let pkg = self.sensors.package_temp();
        let reading = if g.tick % RPM_EVERY == 1 { self.hw.sensors() } else { self.hw.sensor_temps(g.rpm) };
        if let Err(e) = &reading {
            self.warn_every("device", 60.0, format!("device unavailable, waiting: {e}"));
        }
        g.hw_ok = reading.is_ok();
        let sensors = reading.ok();
        let gpu = self.sensors.gpu();
        let raw_cpu = pkg.unwrap_or(0.0).max(sensors.map_or(0.0, |s| s.cpu_temp as f64));
        let fresh_gpu = gpu
            .temp
            // A reading stamped in the future (the clock jumped back since) is not fresh either.
            .filter(|t| {
                *t != 0.0 && gpu.seen.is_some_and(|seen| (-1.0..=GPU_FRESH_S).contains(&(wall - seen)))
            })
            .unwrap_or(0.0);
        let raw_gpu = sensors.map_or(0.0, |s| s.gpu_temp as f64).max(fresh_gpu);
        if raw_cpu <= 0.0 {
            // No sensors at all: the firmware decides.
            self.write_fans(g, Duties::AUTO);
            self.alerts_observe(g, mono, Observed::LOST);
            return;
        }
        let tcpu = g.tcpu.update(raw_cpu);
        // The fan curve and the classifier always get a GPU temperature (the CPU's when there is
        // no reading), but what is shown is only ever a real one.
        let tgpu = g.tgpu.update(if raw_gpu > 0.0 { raw_gpu } else { raw_cpu });
        let tgpu_seen = if raw_gpu > 0.0 {
            Some(g.tgpu_seen.update(raw_gpu))
        } else {
            g.tgpu_seen = Ema::new(TEMP_ALPHA);
            None
        };
        let gpu_state = self.sensors.gpu_state();
        if let Some(s) = sensors {
            g.rpm = (s.cpu_fan_rpm, s.gpu_fan_rpm);
            if s.sys_temp != 0 {
                g.tchassis.update(s.sys_temp as f64);
            }
        }
        let chassis = g.tchassis.value();
        // The classifier compares the GPU's own temperature with the GPU's own hot line: with no
        // GPU reading that is 0 (never hot by GPU), not the CPU's temperature, which would read
        // "hot" at the GPU's 82 C while the CPU, whose line is 92 C, is merely busy.
        let readings = Readings {
            tcpu,
            tgpu: tgpu_seen.unwrap_or(0.0),
            cpu_load: load,
            gpu_util: gpu.util,
            gpu_power: gpu.power,
        };
        g.activity = g.classifier.classify(&readings);
        if g.tick % 15 == 0 {
            let hw_ok = g.hw_ok;
            g.writer.verify(hw_ok); // the firmware or the driver may have reset the fans behind our back
        }
        let ac = g.plug.filter(self.sensors.on_ac(), mono);
        if g.last_ac.is_some_and(|before| before != ac) {
            // Plugging or unplugging can reset the driver state.
            g.writer.forget();
            g.engine.resync();
            let tick = g.tick;
            g.governor.resync(tick);
        }
        g.last_ac = Some(ac);
        if g.hw_ok && (g.cur_mode.is_none() || g.tick % 3 == 0) {
            if let Ok(mode) = self.hw.get_mode() {
                g.cur_mode = Some(mode);
            }
        }
        let frame = Frame { wall, mono, tcpu, tgpu_seen, gpu_state, chassis, load, gpu, ac };
        let smart = self.state.get_bool("smart_fan", false) && !self.legacy("predator-fan-mode") && g.hw_ok;
        if smart {
            let duties = g.fan.step(&fanctl::Inputs {
                activity: fan_activity(g.activity),
                tcpu,
                tgpu,
                tchassis: chassis,
                gpu_power_w: frame.gpu.power,
                on_ac: ac,
                dt_s: FOLLOWER_DT_S,
            });
            self.write_fans(g, duties);
        } else {
            g.writer.forget();
            g.fan.reset();
        }
        if g.tick % 3 == 1 {
            self.enforce_power_limit(g, false);
        }
        self.thermal_governor(g, tcpu);
        if g.hw_ok {
            self.auto_profile(g, &frame);
            self.mode_bindings_tick(g);
            self.operating_lighting(g);
        }
        self.alerts_observe(g, mono, Observed { cpu_c: Some(tcpu), gpu_c: tgpu_seen });
        self.publish(g, smart, &frame);
    }

    fn write_fans(&self, g: &mut Inner, duties: Duties) {
        if let Err(e) = g.writer.write(duties) {
            self.warn_every("fan-write", 10.0, format!("fan write failed: {e}"));
        }
    }

    fn thermal_governor(&self, g: &mut Inner, tcpu: f64) {
        let active = self.smart_active();
        let tick = g.tick;
        if g.governor.step(tick, tcpu, active) {
            for failure in self.cpu_cap.apply(g.governor.cap()) {
                self.warn_every("cpu-cap", 60.0, format!("could not set the CPU clock cap: {failure}"));
            }
        }
    }

    // ---- CPU power limits --------------------------------------------------------------------------

    /// Remembers what the firmware set for PL1 and PL2 (once), to hand it back when nothing
    /// smart is on. It must happen before anything of ours writes the limits.
    pub(super) fn capture_power_limits(&self, g: &mut Inner) {
        if g.pl_orig.is_none() {
            g.pl_orig = self.power.read_raw().ok();
        }
    }

    /// Keeps the CPU package power limits where the user (or the automatic limits of the
    /// configuration) want them, and hands the firmware's own limits back when nothing smart is on.
    pub(super) fn enforce_power_limit(&self, g: &mut Inner, force_now: bool) {
        self.capture_power_limits(g);
        let user = self.user_limits();
        if user.is_none() && !self.smart_active() && !force_now {
            if g.pl_touched {
                if let Some(original) = g.pl_orig {
                    if self.power.read_raw().ok() != Some(original) {
                        self.write_limits(original);
                    }
                }
                g.pl_touched = false;
            }
            return;
        }
        g.pl_touched = true;
        let want = match user {
            Some((pl1, pl2)) => RawLimits { pl1_uw: watts_to_uw(pl1), pl2_uw: watts_to_uw(pl2) },
            None => {
                let w = if g.activity == autoprofile::Activity::Gaming {
                    self.cfg.cpu.pl_gaming_w
                } else {
                    self.cfg.cpu.pl_normal_w
                };
                let uw = watts_to_uw(w as f64);
                RawLimits { pl1_uw: uw, pl2_uw: uw }
            }
        };
        if self.power.read_raw().ok() != Some(want) {
            self.write_limits(want);
        }
    }

    fn write_limits(&self, limits: RawLimits) {
        if let Err(e) = self.power.write_raw(limits) {
            self.warn_every("power-limits", 60.0, format!("could not write the CPU power limits: {e}"));
        }
    }

    // ---- the automatic power mode --------------------------------------------------------------------

    /// The mode asked for by a running game rule. The process list is read every third tick
    /// (cheap, and games do not start every second).
    fn game_mode(&self, g: &mut Inner) -> Option<Mode> {
        let rules = self.game_rules();
        let tick = g.tick;
        g.watcher.current(tick, &rules, |r| gamerules::running(r, &self.processes.list()))
    }

    /// Asks the engine what the power mode should be and carries the answer out.
    fn auto_profile(&self, g: &mut Inner, f: &Frame) {
        let kind = self.auto_kind();
        let game = if kind == AutoKind::Off { None } else { self.game_mode(g) };
        let decision = g.engine.step(&autoprofile::Inputs {
            kind,
            now: f.mono,
            wall: f.wall,
            activity: g.activity,
            tcpu: f.tcpu,
            tgpu: f.tgpu_seen.unwrap_or(0.0),
            ac: f.ac,
            battery_pct: self.sensors.battery_pct(),
            cur_mode: g.current(),
            game,
            disabled: self.legacy("predator-auto-off"),
        });
        // The engine drops a hold when the situation it was picked in is over: remember that.
        let held = g.engine.hold_until();
        if held != self.state.get_f64("profile_hold_until", 0.0) {
            self.save("profile_hold_until", hold_value(held));
        }
        let Some(target) = decision.mode else { return };
        let before = g.mode_name().unwrap_or_else(|| "?".to_owned());
        let set = PowerMode::from_name(target.name()).and_then(|mode| self.hw.set_mode(mode));
        match set {
            Ok(got) => {
                g.cur_mode = Some(Reading::Known(got));
                let notify = g.engine.applied(f.mono, decision.urgent);
                self.info(format!(
                    "auto power mode: {before} -> {target} ({} auto: {}; CPU {} C, GPU {} C, GPU {} %)",
                    kind.name(),
                    decision.reason,
                    rounded(f.tcpu),
                    f.tgpu_seen.map_or(0, rounded),
                    rounded(f.gpu.util),
                ));
                if notify {
                    let what = match g.activity {
                        autoprofile::Activity::Idle | autoprofile::Activity::Browsing => {
                            if f.ac { "on power" } else { "on battery" }.to_owned()
                        }
                        other => other.name().to_owned(),
                    };
                    self.notice(g, format!("Power mode: {target} ({} auto: {what})", kind.name()), "power");
                }
            }
            Err(e) => self.warn_every("auto-profile", 30.0, format!("auto profile: {e}")),
        }
    }

    /// Opt-in (`[keyboard] operating_follow`): re-sends the operating-mode colour when the power
    /// mode changes, because the keyboard does not track it by itself.
    fn operating_lighting(&self, g: &mut Inner) {
        if !self.state.get_bool("operating_follow", false) || g.tick % 2 != 0 {
            return;
        }
        let Some(mode) = g.mode_name() else { return };
        if g.op_mode.as_deref() == Some(mode.as_str()) {
            return;
        }
        g.op_mode = Some(mode.clone());
        if let Err(e) = self.keyboard.follow_mode(&mode, &self.cfg.keyboard.operating_colors) {
            self.warn_every("operating-light", 30.0, format!("operating lighting failed: {e}"));
        }
    }

    // ---- the periodic jobs ---------------------------------------------------------------------------

    /// "Charge to 100 % once": the 80 % limit is lifted for one charge and comes back by itself
    /// when the charger is unplugged, when the battery has been full for five minutes, or after
    /// 24 hours. The record is only cleared once the firmware really took the limit back.
    fn charge_full_check(&self, wall: f64) {
        let Some(mut record) = self.state.get("charge_full").and_then(|v| chargefull::State::from_json(&v))
        else {
            return;
        };
        let mut g = self.lock();
        let ac = g.last_ac == Some(true);
        let mut verdict = chargefull::check(&mut record, ac, self.sensors.battery_pct(), wall);
        if verdict.done.is_none() && record.since > wall + 3600.0 {
            // Started "in the future": the clock jumped back, so the 24 hours cannot be measured.
            // Bringing the 80 % limit back is the safe answer.
            verdict.done = Some(chargefull::Reason::DayPassed);
        }
        if verdict.changed {
            self.save("charge_full", record.to_json());
        }
        let Some(reason) = verdict.done else { return };
        if let Err(e) = self.hw.set_battery_limit(true) {
            self.warn_every("charge-limit", 30.0, format!("could not bring the 80 % limit back: {e}"));
            return;
        }
        self.save("charge_full", Value::Null);
        let text = format!("80 % charge limit is back ({})", reason.text());
        self.info(&text);
        self.notice(&mut g, text, "battery");
    }

    /// While "TrueSense manages power" is on, a manager that came back (an update, a reboot) is
    /// stopped again. Slow (it runs `systemctl`), so it runs without the state lock.
    fn pm_guard(&self) {
        if self.power_managers.status().is_empty() {
            return;
        }
        match self.power_managers.takeover() {
            Ok(r) => {
                let stopped = if r.stopped.is_empty() { "none".to_owned() } else { r.stopped.join(", ") };
                let failed =
                    if r.failed.is_empty() { String::new() } else { format!("; failed: {:?}", r.failed) };
                self.warn_every("pm-guard", 20.0, format!("power managers stopped again: {stopped}{failed}"));
                if !r.stopped.is_empty() {
                    self.say(&format!("Stopped another power manager: {}", r.stopped.join(", ")), "power");
                }
            }
            Err(e) => self.warn_every("pm-guard-failed", 30.0, format!("power manager check failed: {e}")),
        }
    }

    // ---- the boot lighting ---------------------------------------------------------------------------

    /// Puts the look chosen for boot on the lights. The keyboard may enumerate a little after the
    /// service starts, so it retries for about 20 seconds, on its own thread.
    pub(super) fn apply_boot_look(&self, why: &'static str) {
        let Some(me) = self.me.upgrade() else { return };
        let spawned =
            thread::Builder::new().name("boot-look".to_owned()).spawn(move || me.boot_look_worker(why));
        match spawned {
            Ok(handle) => locked(&self.background).push(handle),
            Err(e) => self.warn(format!("boot look ({why}) could not start: {e}")),
        }
    }

    fn boot_look_worker(&self, why: &str) {
        const ATTEMPTS: u32 = 20;
        let Some(look) = self.looks.read_boot().filter(|l| !l.is_empty()) else { return };
        for attempt in 1..=ATTEMPTS {
            let applied = {
                let _g = self.lock(); // lights and fans share the daemon lock, as in the Python daemon
                self.looks.apply(&look)
            };
            match applied {
                Ok(done) => {
                    self.info(format!("boot look ({why}): {}", done.join("; ")));
                    return;
                }
                Err(e) if attempt == ATTEMPTS => {
                    self.warn(format!("boot look ({why}) failed: {e}"));
                    return;
                }
                Err(_) => {}
            }
            if self.sleeper.sleep(Duration::from_secs(1), &self.stop) {
                return;
            }
        }
    }
}

/// Watts as the RAPL files hold them (microwatts), rounded the way `PowerLimits::set_limits` does
/// so that a limit the user set reads back as the same number and is not rewritten every tick.
fn watts_to_uw(watts: f64) -> u64 {
    (watts * 1e6).round().max(0.0) as u64
}

/// A hold's end time as stored: a whole 0 when there is none (as the Python daemon stored it),
/// otherwise the unix time.
pub(super) fn hold_value(until: f64) -> Value {
    if until == 0.0 { Value::from(0) } else { Value::from(until) }
}
