//! Mode bindings in the daemon: the `mode_bindings` command and the hook that applies a mode's
//! look and fan curve when the power mode changes.
//!
//! # Who applies what
//!
//! The daemon applies both parts itself. A look is applied through the same port as the boot look
//! ([`BootLookPort::apply`]), which the daemon already runs as root at start and after every
//! resume, and the keyboard and the rear bar are written through the same hid and firmware
//! paths; so nothing needs the user's session, and a binding works with nobody logged in. A curve
//! is made active the way `fan_curve use` with data does it. The copies live in the state file
//! (`mode_bindings`), because the daemon cannot read a user's looks and curves.
//!
//! # When
//!
//! Once, on the change of the power mode, whatever caused it (Smart auto, the user, the PREDATOR
//! key, a game rule): the control loop looks at the mode every tick, and the hook compares it with
//! the mode it saw last. There is no timer and no retry: a failed apply is logged and the next
//! change tries again, so a missing keyboard cannot make the loop spin. The first mode the daemon
//! sees after it starts is not a change.
//!
//! # Precedence
//!
//! For the lights, a binding that was applied for the mode beats `operating_follow`, which would
//! otherwise paint its colour over the look on the next even tick (it is told the mode was
//! already handled). For the fans, a curve is only made active while the fans are in Smart mode;
//! a manual duty, Max or a stop is the user's and stays, and the firmware's own Auto is not
//! turned into Smart.

use serde_json::{Value, json};
use ts_decision::autoprofile::Current;
use ts_decision::bindings::{self, Action, Bindings, Bound, Clear, FanMode, Skip, Target};
use ts_decision::curves::Curve;
use ts_decision::smartprofile::Mode;
use ts_domain::looks::Look;
use ts_ipc::wire::Args;
use ts_platform::hw::FanMode as FirmwareFans;

use super::daemon::{Daemon, Inner};

/// The state key that holds the bindings.
const KEY: &str = "mode_bindings";

/// What the hook remembers between ticks.
#[derive(Debug, Default)]
pub(super) struct Runtime {
    /// The power mode seen on the last tick; `None` until one was read.
    last: Option<Mode>,
}

/// What became of one action, for the journal and for `apply_now`.
struct Report {
    target: Target,
    name: String,
    /// `applied`, `skipped` or `failed`.
    status: &'static str,
    detail: String,
}

impl Report {
    fn to_json(&self) -> Value {
        json!({"target": self.target.key(), "name": self.name, "status": self.status, "detail": self.detail})
    }
}

/// Accepts a look copy only if it decodes as a look that changes something: the same test the boot
/// look gets before it is stored.
pub(super) fn parse_look(value: &Value) -> Result<Look, String> {
    let look: Look = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
    if look.is_empty() {
        return Err("a look needs keyboard and/or rear settings".to_owned());
    }
    Ok(look)
}

fn mode_of(text: &str) -> Result<Mode, String> {
    Mode::parse(text).ok_or_else(|| {
        format!("unknown power mode '{text}' (use eco, quiet, balanced, performance or turbo)")
    })
}

impl Daemon {
    /// The bindings as stored; damaged data is an error with the reason, never a guess.
    fn stored_bindings(&self) -> Result<Bindings, String> {
        Bindings::from_value(&self.state.get(KEY).unwrap_or(Value::Null)).map_err(|e| e.to_string())
    }

    fn save_bindings(&self, bindings: &Bindings) -> Result<(), String> {
        // Not `Daemon::save`: that one keeps a setting in memory when the disk refuses it, which
        // is right for a switch but would tell the person a binding was stored when it was not.
        self.state.set(KEY, bindings.to_value()).map_err(|e| format!("the bindings could not be saved: {e}"))
    }

    /// How the fans are run, in the terms a binding cares about.
    fn bound_fan_mode(&self) -> FanMode {
        if self.state.get_bool("smart_fan", false) {
            return FanMode::Smart;
        }
        match self.hw.fan_state().map(|s| s.mode) {
            Ok(FirmwareFans::Auto) | Err(_) => FanMode::Firmware,
            Ok(_) => FanMode::Manual,
        }
    }

    /// The `mode_bindings` command: `get` | `set` | `clear` | `apply_now` | `refresh`.
    pub(super) fn cmd_mode_bindings(&self, a: &Args) -> Result<Value, String> {
        let action = a.str_or("action", "get")?;
        let mut g = self.lock();
        match action {
            "get" => self.bindings_view(&g, None),
            "set" => {
                let mode = mode_of(a.require_str("mode")?)?;
                let look = part(a, "look")?
                    .map(|b| Bound::look(&b.name, b.data, parse_look).map_err(|e| e.to_string()))
                    .transpose()?;
                let curve = part(a, "fan_curve")?
                    .map(|b| Bound::fan_curve(&b.name, &b.data).map_err(|e| e.to_string()))
                    .transpose()?;
                let mut all = self.stored_bindings()?;
                all.set(mode, look, curve).map_err(|e| e.to_string())?;
                self.save_bindings(&all)?;
                self.info(format!("mode binding set for {mode}"));
                self.bindings_view(&g, None)
            }
            "clear" => {
                let mode = mode_of(a.require_str("mode")?)?;
                let what = a.str_or("what", "all")?;
                let what = Clear::parse(what).ok_or_else(|| "what: look | fan_curve | all".to_owned())?;
                let mut all = self.stored_bindings()?;
                if all.clear(mode, what) {
                    self.save_bindings(&all)?;
                    self.info(format!("mode binding cleared for {mode}"));
                }
                self.bindings_view(&g, None)
            }
            "refresh" => {
                let name = a.require_str("name")?;
                let data = a.get("data").ok_or_else(|| "refresh needs the new data".to_owned())?;
                let (target, fresh) = match a.require_str("kind")? {
                    "look" => (
                        Target::Look,
                        Bound::look(name, data.clone(), parse_look).map_err(|e| e.to_string())?,
                    ),
                    "fan_curve" => {
                        (Target::FanCurve, Bound::fan_curve(name, data).map_err(|e| e.to_string())?)
                    }
                    _ => return Err("kind: look | fan_curve".to_owned()),
                };
                let mut all = self.stored_bindings()?;
                let updated = all.refresh(target, &fresh);
                if updated > 0 {
                    self.save_bindings(&all)?;
                    self.info(format!("mode bindings: the copy of {target} {name} was refreshed"));
                }
                self.bindings_view(&g, Some(updated))
            }
            "apply_now" => {
                let now = match a.opt_str("mode")? {
                    Some(text) => mode_of(text)?,
                    None => match g.current() {
                        Current::Known(m) => m,
                        _ => return Err("the power mode is not known yet".to_owned()),
                    },
                };
                let all = self.stored_bindings()?;
                let actions = bindings::plan(now, self.bound_fan_mode(), &all, parse_look);
                let reports = self.carry_out(&mut g, now, actions);
                Ok(
                    json!({"mode": now.name(), "results": reports.iter().map(Report::to_json).collect::<Vec<_>>()}),
                )
            }
            _ => Err("mode_bindings: get | set | clear | apply_now | refresh".to_owned()),
        }
    }

    /// The answer of every reading and changing action: the power mode now, the bindings and, for
    /// `refresh`, how many copies were replaced.
    fn bindings_view(&self, g: &Inner, updated: Option<usize>) -> Result<Value, String> {
        let all = self.stored_bindings()?;
        let mut view = json!({"mode": g.mode_name(), "bindings": all.to_value()});
        if let (Some(n), Some(map)) = (updated, view.as_object_mut()) {
            map.insert("updated".to_owned(), Value::from(n));
        }
        Ok(view)
    }

    /// Called every tick after the automatic power mode has had its say: applies the binding of a
    /// mode that was just entered.
    pub(super) fn mode_bindings_tick(&self, g: &mut Inner) {
        let Current::Known(now) = g.current() else { return };
        let prev = g.bind.last.replace(now);
        if prev.is_none_or(|before| before == now) {
            return;
        }
        // Reading the stored value is the cheap check that keeps the whole feature free for
        // everyone who has no binding: no fan or lighting query happens without one.
        let Some(stored) = self.state.get(KEY).filter(|v| !v.is_null()) else { return };
        let all = match Bindings::from_value(&stored) {
            Ok(all) => all,
            Err(e) => {
                self.warn(format!("mode bindings not applied: {e}"));
                return;
            }
        };
        if all.get(now).is_none() {
            return;
        }
        let actions = bindings::edge(prev, now, self.bound_fan_mode(), &all, parse_look);
        self.carry_out(g, now, actions);
    }

    /// Does what the actions say, once each, and writes one journal line per action. A failure is
    /// reported and stays a failure: nothing here retries.
    fn carry_out(&self, g: &mut Inner, mode: Mode, actions: Vec<Action<Look>>) -> Vec<Report> {
        let reports: Vec<Report> = actions
            .into_iter()
            .map(|action| match action {
                Action::ApplyLook { name, look } => self.apply_bound_look(g, mode, name, &look),
                Action::ApplyCurve { name, curve } => self.apply_bound_curve(g, name, curve),
                Action::Skip { target, name, why } => {
                    let status = if matches!(why, Skip::Invalid(_)) { "failed" } else { "skipped" };
                    Report { target, name, status, detail: why.to_string() }
                }
            })
            .collect();
        for r in &reports {
            let line = format!("mode binding {mode}: {} {}: {} ({})", r.target, r.name, r.status, r.detail);
            if r.status == "applied" || r.status == "skipped" {
                self.info(line);
            } else {
                self.warn(line);
            }
        }
        reports
    }

    fn apply_bound_look(&self, g: &mut Inner, mode: Mode, name: String, look: &Look) -> Report {
        // The lights and the fans share the daemon lock; the caller holds it.
        match self.looks.apply(look) {
            Ok(done) => {
                // `operating_follow` must not repaint the keyboard for this mode.
                g.op_mode = Some(mode.name().to_owned());
                Report { target: Target::Look, name, status: "applied", detail: done.join("; ") }
            }
            Err(e) => Report { target: Target::Look, name, status: "failed", detail: e.to_string() },
        }
    }

    /// Makes `curve` the active one, as `fan_curve use` with data does: stored with the daemon so
    /// it survives a restart, followed by Smart from the next tick.
    fn apply_bound_curve(&self, g: &mut Inner, name: String, curve: Curve) -> Report {
        self.save("fan_curve_data", curve.to_value());
        self.save("fan_curve", name.as_str());
        g.fan.set_curve(curve);
        g.writer.forget();
        Report {
            target: Target::FanCurve,
            name,
            status: "applied",
            detail: "now the active curve".to_owned(),
        }
    }
}

/// A `{name, data}` argument; absent and `null` are "not given".
fn part(a: &Args, key: &str) -> Result<Option<Bound>, String> {
    match a.get(key).filter(|v| !v.is_null()) {
        None => Ok(None),
        Some(v) => serde_json::from_value(v.clone()).map(Some).map_err(|e| format!("{key}: {e}")),
    }
}
