//! The control socket's command table.
//!
//! The wire protocol is the Python daemon's: one JSON line in, one out, with these command
//! names, argument names and result shapes (`daemon.py`, `control.py`), so the Python and Go
//! clients keep working against this daemon. Access control is the server's: it reads the
//! caller's uid from the kernel (`SO_PEERCRED`) and lets only root and the configured desktop
//! user through ([`Daemon::allowed_uids`]); within that set every command is open to the
//! caller, as in Python. What a command may change is limited by the rules inside it (a fan
//! stop needs `confirm` and a cool machine, an undervolt needs `confirm`, a risky value needs
//! `force`), not by who asks.
//!
//! Every handler except `power_managers` (slow: it runs `systemctl`) takes the daemon lock, so
//! the control loop and a request never change the hardware at the same time; a request that
//! changes a fan setting is the one atomic step "smart curve off + hardware write".
//!
//! Arguments are read with the typed accessors of `ts_ipc::wire::Args`: a wrong type is an error
//! reply naming the argument, never a guess. Arguments a command does not know are ignored.

use std::sync::Arc;

use serde::Serialize;
use serde_json::{Map, Value, json};
use ts_decision::fanctl::{self, FanError, Temps};
use ts_decision::gamerules::{self, Rules};
use ts_decision::{curves, smartprofile};
use ts_domain::gpuundervolt::{self, Profile as GpuProfile};
use ts_domain::looks::Look;
use ts_domain::power::{self, Level};
use ts_domain::undervolt::{self, Profile as CpuProfile};
use ts_ipc::server::Router;
use ts_ipc::wire::Args;
use ts_platform::hw::{Coded, FanDuty, PowerMode, Reading};

use super::daemon::{Daemon, Inner};
use super::step::hold_value;

/// The commands of the table, in the order of the Python daemon.
pub const COMMANDS: [&str; 18] = [
    "ping",
    "state",
    "set",
    "fan",
    "fan_curve",
    "smart_profile",
    "undervolt",
    "gpu_undervolt",
    "battery_full",
    "power",
    "boot_look",
    "hold",
    "game_rules",
    "power_managers",
    "cpu_limits",
    "cpu_info",
    "touchpad",
    "mode_bindings",
];

type Reply = Result<Value, String>;

/// The text of any failure, for an error reply.
fn text(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// A typed result as JSON (every type here serialises to an object; `null` is unreachable).
fn to_json<T: Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

/// `base` (an object) with extra keys added or replaced.
fn merged(mut base: Value, extra: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    if let Value::Object(map) = &mut base {
        map.extend(extra.into_iter().map(|(k, v)| (k.to_owned(), v)));
    }
    base
}

/// Python's `name or default`: an absent, null or empty text means the default.
fn name_or<'a>(name: Option<&'a str>, default: &'a str) -> &'a str {
    name.filter(|n| !n.is_empty()).unwrap_or(default)
}

impl Daemon {
    /// The command table for the control socket. Each route holds the daemon, so the table lives
    /// as long as the server that runs it.
    pub fn router(self: &Arc<Self>) -> Router {
        let mut router = Router::new();
        let mut add = |name: &str, handler: fn(&Daemon, &Args) -> Reply| {
            let daemon = Arc::clone(self);
            router.insert(name, move |args: &Args| handler(&daemon, args));
        };
        add("ping", Daemon::cmd_ping);
        add("state", Daemon::cmd_state);
        add("set", Daemon::cmd_set);
        add("fan", Daemon::cmd_fan);
        add("fan_curve", Daemon::cmd_fan_curve);
        add("smart_profile", Daemon::cmd_smart_profile);
        add("undervolt", Daemon::cmd_undervolt);
        add("gpu_undervolt", Daemon::cmd_gpu_undervolt);
        add("battery_full", Daemon::cmd_battery_full);
        add("power", Daemon::cmd_power);
        add("boot_look", Daemon::cmd_boot_look);
        add("hold", Daemon::cmd_hold);
        add("game_rules", Daemon::cmd_game_rules);
        add("power_managers", Daemon::cmd_power_managers);
        add("cpu_limits", Daemon::cmd_cpu_limits);
        add("cpu_info", Daemon::cmd_cpu_info);
        add("touchpad", Daemon::cmd_touchpad);
        add("mode_bindings", Daemon::cmd_mode_bindings);
        router
    }

    /// Makes the user's own power-mode pick override the automatic one for `minutes` (0 or less
    /// clears it), and stores when it ends so it survives a restart. Returns the minutes held.
    pub(super) fn set_hold(&self, g: &mut Inner, minutes: f64) -> f64 {
        let ac = g.last_ac == Some(true);
        let held = g.engine.set_hold(self.clock.wall(), minutes, ac, g.activity);
        self.save("profile_hold_until", hold_value(g.engine.hold_until()));
        held
    }

    fn cmd_ping(&self, _: &Args) -> Reply {
        Ok(Value::from("pong"))
    }

    fn cmd_state(&self, _: &Args) -> Reply {
        let g = self.lock();
        let until = self.state.get_f64("profile_hold_until", 0.0);
        let left = (until - self.clock.wall()).max(0.0) as i64;
        Ok(
            json!({"live": g.live.to_json(), "settings": Value::Object(self.state.snapshot()), "hold_left_s": left}),
        )
    }

    fn cmd_set(&self, a: &Args) -> Reply {
        let key = a.require_str("key")?;
        let mut g = self.lock();
        if key == "auto_mode" {
            let Some(mode) =
                a.get("value").and_then(Value::as_str).filter(|v| matches!(*v, "off" | "native" | "smart"))
            else {
                return Err("auto_mode: off | native | smart".to_owned());
            };
            self.save("auto_mode", mode);
            self.save("auto_profile", mode != "off");
            self.info(format!("setting auto_mode = {mode}"));
            if mode != "off" {
                // Choosing auto means: take over now.
                g.engine.take_over_now();
                self.save("profile_hold_until", 0);
            }
            return Ok(Value::Object(self.state.snapshot()));
        }
        if !matches!(key, "smart_fan" | "auto_profile" | "operating_follow") {
            return Err(format!("unknown setting '{key}'"));
        }
        let Some(&Value::Bool(on)) = a.get("value") else {
            return Err("value must be true or false".to_owned());
        };
        if key == "auto_profile" {
            self.save("auto_mode", if on { "smart" } else { "off" });
        }
        self.save(key, on);
        self.info(format!("setting {key} = {on}"));
        if key == "smart_fan" && !on {
            g.writer.forget();
        }
        Ok(Value::Object(self.state.snapshot()))
    }

    /// One request = smart curve off + hardware write, atomically (the loop takes the same lock).
    /// A fan stop needs the explicit `confirm` and a cool machine, and is watched by the
    /// temperature watchdog until it ends.
    fn cmd_fan(&self, a: &Args) -> Reply {
        let mut mode = a.require_str("mode")?;
        let confirm = a.bool_or("confirm", false)?;
        let mut g = self.lock();
        if mode == "smart" {
            self.save("smart_fan", true);
            g.stop_watch.disarm();
            self.info("setting smart_fan = True");
            return Ok(Value::from("smart"));
        }
        let (mut cpu, mut gpu) = (None, None);
        if mode == "off" {
            mode = "manual";
            (cpu, gpu) = (Some(0), Some(0));
        }
        if !matches!(mode, "auto" | "max" | "manual") {
            return Err(format!("fan mode '{mode}'"));
        }
        let mut plan = None;
        if mode == "manual" {
            if cpu.is_none() {
                cpu = a.opt_int("cpu")?;
                gpu = a.opt_int("gpu")?;
            }
            let Some(cpu) = cpu else { return Err(FanError::BadDuty.to_string()) };
            let temps = Temps { cpu: g.tcpu.value(), gpu: g.tgpu.value() };
            plan = Some(fanctl::plan_manual(cpu, gpu, confirm, temps).map_err(text)?);
        }
        g.stop_watch.disarm();
        self.save("smart_fan", false);
        self.info(format!("setting smart_fan = False (fan {mode})"));
        g.writer.forget();
        g.fan.reset();
        match plan {
            None if mode == "auto" => self.hw.fan_auto().map_err(text)?,
            None => self.hw.fan_max().map_err(text)?,
            Some(p) => {
                let duty = |percent: u8| FanDuty::new(i64::from(percent)).map_err(text);
                self.hw.fan_manual(duty(p.cpu)?, duty(p.gpu)?).map_err(text)?;
            }
        }
        if let Some(p) = plan.filter(|p| p.stops_a_fan()) {
            g.stop_watch.arm();
            self.warn_every(
                "fan-stopped",
                5.0,
                format!(
                    "{} stopped by you; watchdog: back to Auto at CPU {} C / GPU {} C",
                    p.stopped_which().unwrap_or("a fan"),
                    fanctl::STOP_MAX_CPU_C,
                    fanctl::STOP_MAX_GPU_C
                ),
            );
            return Ok(Value::from(p.outcome()));
        }
        Ok(Value::from(mode))
    }

    fn cmd_power(&self, a: &Args) -> Reply {
        let mode = a.require_str("mode")?;
        let hold = a.opt_float("hold_minutes")?;
        let mut g = self.lock();
        let wanted = PowerMode::from_name(mode).map_err(text)?;
        let new = self.hw.set_mode(wanted).map_err(text)?;
        g.cur_mode = Some(Reading::Known(new));
        if let Some(minutes) = hold.filter(|m| *m != 0.0) {
            self.set_hold(&mut g, minutes);
        }
        Ok(Value::from(new.name()))
    }

    fn cmd_hold(&self, a: &Args) -> Reply {
        let minutes = a.opt_float("minutes")?.unwrap_or(0.0);
        let mut g = self.lock();
        let held = self.set_hold(&mut g, minutes);
        Ok(if held == 0.0 { Value::from(0) } else { Value::from(held) })
    }

    /// `get` shows the active curve; `use` switches to a built-in or saved one by name, or to a
    /// curve sent as data (kept by the daemon, so the choice works before anyone logs in).
    fn cmd_fan_curve(&self, a: &Args) -> Reply {
        let action = a.str_or("action", "get")?;
        let name = a.opt_str("name")?;
        let data = a.get("data").filter(|v| !v.is_null());
        let mut g = self.lock();
        match action {
            "get" => {
                let names = self.curves.names().map_err(text)?;
                return Ok(json!({
                    "name": self.state.get_str("fan_curve", "native"),
                    "curve": g.fan.curve().to_value(),
                    "names": names,
                }));
            }
            "use" => {}
            _ => return Err("fan_curve: get | use".to_owned()),
        }
        let curve = if let Some(data) = data {
            let curve = curves::validate(data).map_err(text)?;
            self.save("fan_curve_data", curve.to_value());
            self.save("fan_curve", name_or(name, "custom"));
            curve
        } else {
            let label = name_or(name, "native");
            let curve = self.curves.load(label).map_err(text)?;
            self.save(
                "fan_curve_data",
                if curves::is_builtin(label) { Value::Null } else { curve.to_value() },
            );
            self.save("fan_curve", label);
            curve
        };
        if curve.native {
            // The native curve is no curve: the firmware runs the fans, like PredatorSense on Windows.
            self.save("smart_fan", false);
            g.stop_watch.disarm();
            g.writer.forget();
            if let Err(e) = self.hw.fan_auto() {
                self.warn_every("fan-auto", 10.0, format!("fan auto: {e}"));
            }
            return Ok(json!({"name": "native", "curve": curve.to_value()}));
        }
        g.fan.set_curve(curve.clone());
        g.writer.forget();
        self.info(format!("setting fan_curve = {}", self.state.get_str("fan_curve", "")));
        Ok(json!({"name": self.state.get_str("fan_curve", ""), "curve": curve.to_value()}))
    }

    /// `get` shows the active Smart auto profile; `use` switches to a saved one by name, or to a
    /// profile sent as data (kept by the daemon).
    fn cmd_smart_profile(&self, a: &Args) -> Reply {
        let action = a.str_or("action", "get")?;
        let name = a.opt_str("name")?;
        let data = a.get("data").filter(|v| !v.is_null());
        let mut g = self.lock();
        match action {
            "get" => {
                return Ok(json!({
                    "name": self.state.get_str("smart_profile", "truesense"),
                    "profile": g.engine.profile().to_json(),
                    "names": self.smart_profiles.names(),
                }));
            }
            "use" => {}
            _ => return Err("smart_profile: get | use".to_owned()),
        }
        let profile = if let Some(data) = data {
            let profile = smartprofile::validate(data).map_err(text)?;
            self.save("smart_profile_data", profile.to_json());
            self.save("smart_profile", name_or(name, "custom"));
            profile
        } else {
            let label = name_or(name, "truesense");
            let profile = self.smart_profiles.load(label).map_err(text)?;
            let stored = if smartprofile::is_builtin(label) { Value::Null } else { profile.to_json() };
            self.save("smart_profile_data", stored);
            self.save("smart_profile", label);
            profile
        };
        g.engine.set_profile(profile.clone());
        self.info(format!("setting smart_profile = {}", self.state.get_str("smart_profile", "")));
        Ok(json!({"name": self.state.get_str("smart_profile", ""), "profile": profile.to_json()}))
    }

    /// `get` | `apply` (core and cache in mV, needs `confirm`) | `reset` | `boot` (apply at boot,
    /// needs `confirm`) | `boot_clear`. Offsets never persist across a reboot.
    fn cmd_undervolt(&self, a: &Args) -> Reply {
        let action = a.str_or("action", "get")?;
        let g = self.lock();
        match action {
            "get" => {
                let current = match self.cpu_undervolt.read() {
                    Ok(offsets) => to_json(&offsets),
                    Err(e) => json!({"error": e.to_string()}),
                };
                return Ok(merged(
                    current,
                    [
                        ("applied", self.state.get("uv_applied").unwrap_or(Value::Null)),
                        ("boot", self.state.get("uv_boot").unwrap_or(Value::Null)),
                        ("pending", Value::Bool(g.uv_guard.pending())),
                    ],
                ));
            }
            "reset" => {
                let r = self.cpu_undervolt.reset().map_err(text)?;
                self.save("uv_applied", Value::Null);
                self.info("undervolt reset to stock");
                return Ok(to_json(&r));
            }
            "boot_clear" => {
                self.save("uv_boot", Value::Null);
                return Ok(Value::Null);
            }
            "apply" | "boot" => {}
            _ => return Err("undervolt: get | apply | reset | boot | boot_clear".to_owned()),
        }
        if !a.bool_or("confirm", false)? {
            return Err("an undervolt needs explicit confirmation (it can freeze or reboot the laptop if too aggressive)".to_owned());
        }
        let wanted = json!({"core": a.get("core").cloned().unwrap_or(Value::Null), "cache": a.get("cache").cloned().unwrap_or(Value::Null)});
        let p = CpuProfile::from_value(&wanted).map_err(text)?;
        let rating = undervolt::risk(p.core, p.cache);
        if rating.level == Level::Danger && !a.bool_or("force", false)? {
            return Err(format!(
                "{} Add the explicit force flag if you really mean it (CLI: --force).",
                rating.message
            ));
        }
        let name = a.opt_str("name")?;
        let record = json!({"name": name, "core": p.core, "cache": p.cache});
        if action == "boot" {
            self.save("uv_boot", record.clone());
            return Ok(record);
        }
        let applied = match self.cpu_undervolt.apply(p.core, p.cache, true) {
            Ok(applied) => applied,
            Err(e) => {
                // A failed apply must not leave half an offset behind.
                let _ = self.cpu_undervolt.reset();
                return Err(e.to_string());
            }
        };
        self.save("uv_applied", record);
        self.info(format!("undervolt applied: core {} mV, cache {} mV", p.core, p.cache));
        Ok(to_json(&applied))
    }

    /// `get` | `apply` (offset and cap in MHz, needs `confirm`) | `reset` | `boot` | `boot_clear`.
    /// Neither setting survives a reboot.
    fn cmd_gpu_undervolt(&self, a: &Args) -> Reply {
        let action = a.str_or("action", "get")?;
        let g = self.lock();
        match action {
            "get" => {
                let current = match self.gpu_undervolt.read() {
                    Ok(state) => to_json(&state),
                    Err(e) => json!({"error": e.to_string()}),
                };
                return Ok(merged(
                    current,
                    [
                        ("applied", self.state.get("gpu_uv_applied").unwrap_or(Value::Null)),
                        ("boot", self.state.get("gpu_uv_boot").unwrap_or(Value::Null)),
                        ("pending", Value::Bool(g.gpu_uv_guard.pending())),
                    ],
                ));
            }
            "reset" => {
                let r = self.gpu_undervolt.reset().map_err(text)?;
                self.save("gpu_uv_applied", Value::Null);
                self.info("GPU undervolt reset");
                return Ok(to_json(&r));
            }
            "boot_clear" => {
                self.save("gpu_uv_boot", Value::Null);
                return Ok(Value::Null);
            }
            "apply" | "boot" => {}
            _ => return Err("gpu_undervolt: get | apply | reset | boot | boot_clear".to_owned()),
        }
        if !a.bool_or("confirm", false)? {
            return Err("a GPU undervolt needs explicit confirmation (it can crash games or the driver if too aggressive)".to_owned());
        }
        // An absent cap means "none" (0); a cap that is present must be valid.
        let mut wanted = Map::new();
        wanted.insert("offset".to_owned(), a.get("offset").cloned().unwrap_or(Value::Null));
        if let Some(cap) = a.get("cap") {
            wanted.insert("cap".to_owned(), cap.clone());
        }
        let p = GpuProfile::from_value(&Value::Object(wanted)).map_err(text)?;
        let rating = gpuundervolt::risk(p.offset, p.cap);
        if rating.level == Level::Danger && !a.bool_or("force", false)? {
            return Err(format!(
                "{} Add the explicit force flag if you really mean it (CLI: --force).",
                rating.message
            ));
        }
        let name = a.opt_str("name")?;
        // The record keeps the validated profile's fields, description included, as Python did.
        let record = json!({"name": name, "description": p.description, "offset": p.offset, "cap": p.cap});
        if action == "boot" {
            self.save("gpu_uv_boot", record.clone());
            return Ok(record);
        }
        let applied = match self.gpu_undervolt.apply(p.offset, p.cap) {
            Ok(applied) => applied,
            Err(e) => {
                let _ = self.gpu_undervolt.reset();
                return Err(e.to_string());
            }
        };
        self.save("gpu_uv_applied", record);
        self.info(format!("GPU undervolt applied: offset {} MHz, cap {} MHz", p.offset, p.cap));
        Ok(to_json(&applied))
    }

    /// `get` | `start` | `cancel`: charge to 100 % once, then the 80 % limit returns by itself.
    fn cmd_battery_full(&self, a: &Args) -> Reply {
        let action = a.str_or("action", "get")?;
        let g = self.lock();
        let ac = g.last_ac == Some(true);
        match action {
            "get" => {
                let active = self
                    .state
                    .get("charge_full")
                    .and_then(|v| ts_decision::chargefull::State::from_json(&v))
                    .is_some();
                let limit = self.hw.get_battery_limit().map_err(text)?;
                Ok(
                    json!({"active": active, "limit": limit, "capacity": self.sensors.battery_pct(), "ac": ac}),
                )
            }
            "start" => {
                if !ac {
                    return Err("plug the charger in first".to_owned());
                }
                if !self.hw.get_battery_limit().map_err(text)? {
                    return Err("the 80 % limit is off already: the battery charges to 100 %".to_owned());
                }
                self.hw.set_battery_limit(false).map_err(text)?;
                self.save("charge_full", ts_decision::chargefull::State::start(self.clock.wall()).to_json());
                self.info("charging to 100 % once");
                Ok(json!({"active": true}))
            }
            "cancel" => {
                self.hw.set_battery_limit(true).map_err(text)?;
                self.save("charge_full", Value::Null);
                Ok(json!({"active": false}))
            }
            _ => Err("battery_full: get | start | cancel".to_owned()),
        }
    }

    fn cmd_boot_look(&self, a: &Args) -> Reply {
        match a.str_or("action", "get")? {
            "get" => Ok(self.looks.read_boot().map_or(Value::Null, |look| to_json(&look))),
            "clear" => {
                self.looks.write_boot(None).map_err(text)?;
                Ok(Value::Null)
            }
            "set" => {
                const NEEDS: &str = "a look needs keyboard and/or rear settings";
                let raw = a.get("look").filter(|v| v.is_object()).ok_or_else(|| NEEDS.to_owned())?;
                let look: Look = serde_json::from_value(raw.clone()).map_err(text)?;
                if look.is_empty() {
                    return Err(NEEDS.to_owned());
                }
                self.looks.write_boot(Some(&look)).map_err(text)?;
                Ok(raw.clone())
            }
            _ => Err("boot_look: get | set | clear".to_owned()),
        }
    }

    /// A program name mapped to a power mode while it runs: `list` | `add` | `remove` | `clear`.
    fn cmd_game_rules(&self, a: &Args) -> Reply {
        let action = a.str_or("action", "list")?;
        let name = a.str_or("name", "")?;
        let mut g = self.lock();
        let rules = self.game_rules();
        let rules = match action {
            "list" => rules,
            "add" => rules.add(name, a.str_or("mode", "")?).map_err(text)?,
            "remove" => rules.remove(name).map_err(text)?,
            "clear" => Rules::new(),
            _ => return Err("game_rules: list | add | remove | clear".to_owned()),
        };
        if action != "list" {
            self.save("game_rules", rules.to_json());
            g.engine.resync();
        }
        let running = gamerules::running(&rules, &self.processes.list());
        Ok(json!({"rules": rules.to_json(), "running": running.map(|m| m.name())}))
    }

    /// Other power managers (a fixed list): `status` | `takeover` (stop them, fix the CPU
    /// defaults, remember) | `restore` (undo exactly that). Slow, so it runs without the lock.
    fn cmd_power_managers(&self, a: &Args) -> Reply {
        match a.str_or("action", "status")? {
            "status" => Ok(json!({
                "on": self.state.get_bool("pm_takeover", false),
                "found": to_json(&self.power_managers.status()),
                "recorded": self.power_managers.recorded(),
            })),
            "takeover" => {
                self.save("pm_takeover", true);
                let r = self.power_managers.takeover().map_err(text)?;
                if !r.failed.is_empty() {
                    self.warn(format!("power managers: could not stop {:?}", r.failed));
                }
                let stopped = if r.stopped.is_empty() {
                    "nothing (none running)".to_owned()
                } else {
                    format!("{:?}", r.stopped)
                };
                self.info(format!("TrueSense manages power: stopped {stopped}"));
                Ok(merged(to_json(&r), [("on", Value::Bool(true))]))
            }
            "restore" => {
                self.save("pm_takeover", false);
                let r = self.power_managers.restore();
                let restored = if r.restored.is_empty() {
                    "nothing to restore".to_owned()
                } else {
                    format!("{:?}", r.restored)
                };
                self.info(format!("power managers restored: {restored}"));
                Ok(merged(to_json(&r), [("on", Value::Bool(false))]))
            }
            _ => Err("power_managers: status | takeover | restore".to_owned()),
        }
    }

    /// Sets the CPU power limits and keeps them (the loop re-asserts them every few seconds);
    /// `reset` goes back to the automatic limits from the configuration.
    fn cmd_cpu_limits(&self, a: &Args) -> Reply {
        let reset = a.bool_or("reset", false)?;
        let pl1 = a.opt_float("pl1")?;
        let pl2 = a.opt_float("pl2")?;
        let force = a.bool_or("force", false)?;
        let mut g = self.lock();
        if reset {
            self.save("pl_user", Value::Null);
            self.enforce_power_limit(&mut g, true);
            return Ok(merged(to_json(&self.power.info()), [("automatic", Value::Bool(true))]));
        }
        let (Some(pl1), Some(pl2)) = (pl1, pl2) else {
            return Err("give pl1 and pl2 (watts), or reset".to_owned());
        };
        let rating = power::risk(pl1, pl2);
        if rating.level == Level::Danger && !force {
            return Err(format!("{} Add the explicit force flag if you really mean it.", rating.message));
        }
        let applied = self.power.set_limits(pl1, pl2).map_err(text)?;
        self.save("pl_user", json!({"pl1": pl1, "pl2": pl2}));
        self.info(format!("setting power limits = {pl1}/{pl2} W"));
        Ok(to_json(&applied))
    }

    fn cmd_cpu_info(&self, _: &Args) -> Reply {
        let _g = self.lock();
        let cpu = &self.cfg.cpu;
        Ok(merged(
            to_json(&self.power.info()),
            [
                ("user", self.state.get("pl_user").unwrap_or(Value::Null)),
                ("auto", json!({"normal_w": cpu.pl_normal_w, "gaming_w": cpu.pl_gaming_w})),
            ],
        ))
    }

    fn cmd_touchpad(&self, a: &Args) -> Reply {
        let action = a.str_or("action", "status")?;
        let _g = self.lock();
        let current = self.touchpad.enabled();
        let want = match action {
            "status" => return Ok(current.map_or(Value::Null, Value::Bool)),
            "on" => true,
            "off" => false,
            "toggle" => !current.unwrap_or(false),
            _ => return Err("touchpad: status | on | off | toggle".to_owned()),
        };
        self.touchpad.set_enabled(want).map(Value::Bool).map_err(text)
    }
}
