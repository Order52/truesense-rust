//! Mode bindings: per power mode a look and a fan curve that the service applies when the mode
//! changes.
//!
//! The service (root) keeps the bindings as self-contained copies, because it cannot read the
//! looks and curves in this user's home directory and a binding has to work before anyone logs
//! in. So binding a mode resolves the named look and curve here, validates them, and sends the
//! data; saving a look or a curve that a binding names sends the new data again
//! ([`Api::refresh_binding_copies`]), otherwise the service's copy would go stale.
#![forbid(unsafe_code)]

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use ts_decision::bindings::{BindingError, Bindings, Bound, Clear, Target};
use ts_decision::smartprofile::Mode;
use ts_domain::looks::Look;
use ts_ipc::wire::Args;

use super::{Api, ApiError, ApiResult};

/// What the service has bound, and the power mode it is in now.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModeBindings {
    /// The power mode the service last saw; `None` while it is unread.
    pub mode: Option<String>,
    /// The bound modes (a mode without a binding is absent).
    pub bindings: Bindings,
}

impl fmt::Display for ModeBindings {
    /// One line per bound mode: `turbo: look red, fan curve loud`; a note when nothing is bound.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.bindings.is_empty() {
            return f.write_str("no power mode is bound to a look or a fan curve");
        }
        for (i, (mode, binding)) in self.bindings.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            let current = if self.mode.as_deref() == Some(mode.name()) { "* " } else { "  " };
            let part = |what: &str, bound: &Option<Bound>| {
                bound.as_ref().map_or_else(|| format!("{what} -"), |b| format!("{what} {}", b.name))
            };
            write!(
                f,
                "{current}{mode}: {}, {}",
                part("look", &binding.look),
                part("fan curve", &binding.fan_curve)
            )?;
        }
        Ok(())
    }
}

/// What became of one part when a binding was applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindingResult {
    /// `look` or `fan_curve`.
    pub target: String,
    /// The binding's name for it.
    pub name: String,
    /// `applied`, `skipped` (fans not in Smart mode, a manual fan setting held) or `failed`.
    pub status: String,
    /// What was done, or why not.
    pub detail: String,
}

/// The answer of "apply this mode's binding now".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindingsApplied {
    /// The mode whose binding was applied.
    pub mode: String,
    /// One entry per bound part; empty when the mode has no binding.
    pub results: Vec<BindingResult>,
}

impl fmt::Display for BindingsApplied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.results.is_empty() {
            return write!(f, "{} has no binding", self.mode);
        }
        for (i, r) in self.results.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(
                f,
                "{} {} {}: {} ({})",
                self.mode,
                r.target.replace('_', " "),
                r.name,
                r.status,
                r.detail
            )?;
        }
        Ok(())
    }
}

/// The lighting parser every copy goes through: the same test the service applies before it uses a
/// look (it must decode and change something).
fn parse_look(value: &Value) -> Result<Look, String> {
    let look: Look = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
    if look.is_empty() {
        return Err("a look needs keyboard and/or rear settings".to_owned());
    }
    Ok(look)
}

fn bad(e: BindingError) -> ApiError {
    ApiError::domain("mode bindings", e)
}

fn mode_of(text: &str) -> ApiResult<Mode> {
    Mode::parse(text).ok_or_else(|| bad(BindingError::UnknownMode(text.to_owned())))
}

impl Api {
    /// The look and fan curve copies of a name, as they would be bound now.
    fn bound_look(&self, name: &str) -> ApiResult<Bound> {
        let look = self.s.looks.load(name)?;
        Bound::look(name, serde_json::to_value(&look)?, parse_look).map_err(bad)
    }

    fn bound_curve(&self, name: &str) -> ApiResult<Bound> {
        let curve = self.s.curves.load(name)?;
        Bound::fan_curve(name, &curve.to_value()).map_err(bad)
    }

    /// What the service has bound to each power mode.
    pub fn mode_bindings(&self) -> ApiResult<ModeBindings> {
        self.ask("mode_bindings", Args::new().with("action", "get"))
    }

    /// Binds `mode` to a saved look and/or a saved fan curve (built-in ones too, except the native
    /// curve). A part left out keeps what the mode had. Both are validated here and again by the
    /// service, which stores copies and applies them when the mode changes.
    pub fn bind_mode(&self, mode: &str, look: Option<&str>, curve: Option<&str>) -> ApiResult<ModeBindings> {
        let mode = mode_of(mode)?;
        let look = look.map(|n| self.bound_look(n)).transpose()?;
        let curve = curve.map(|n| self.bound_curve(n)).transpose()?;
        if look.is_none() && curve.is_none() {
            return Err(bad(BindingError::Nothing));
        }
        let mut args = Args::new().with("action", "set").with("mode", mode.name());
        for (key, part) in [("look", look), ("fan_curve", curve)] {
            if let Some(b) = part {
                args.insert(key, json!({"name": b.name, "data": b.data}));
            }
        }
        self.ask("mode_bindings", args)
    }

    /// Removes the look, the fan curve or both from a mode's binding. `what` is `look`,
    /// `fan_curve` or `all`; nothing is applied or undone on the machine.
    pub fn unbind_mode(&self, mode: &str, what: &str) -> ApiResult<ModeBindings> {
        let mode = mode_of(mode)?;
        let what = if what.is_empty() { "all" } else { what };
        if Clear::parse(what).is_none() {
            return Err(ApiError::invalid("what: look | fan_curve | all"));
        }
        self.ask(
            "mode_bindings",
            Args::new().with("action", "clear").with("mode", mode.name()).with("what", what),
        )
    }

    /// Applies a mode's binding now (the current mode's when `mode` is `None`): the look, and the
    /// curve if the fans are in Smart mode. A refusal of a part is a result, not an error.
    pub fn apply_mode_binding(&self, mode: Option<&str>) -> ApiResult<BindingsApplied> {
        let mut args = Args::new().with("action", "apply_now");
        if let Some(text) = mode.filter(|m| !m.is_empty()) {
            args.insert("mode", mode_of(text)?.name());
        }
        self.ask("mode_bindings", args)
    }

    /// Sends the service the current data of a saved look or fan curve, so that the copies of
    /// bindings that name it are not stale. Answers how many copies were replaced; `0` without a
    /// service, and for a name no binding uses. Saving a look or a curve calls this, and ignores a
    /// failure: the save itself succeeded and the bindings keep working with the older copy.
    pub fn refresh_binding_copies(&self, target: Target, name: &str) -> ApiResult<usize> {
        if !self.daemon_available() {
            return Ok(0);
        }
        let fresh = match target {
            Target::Look => self.bound_look(name)?,
            Target::FanCurve => self.bound_curve(name)?,
        };
        let args = Args::new()
            .with("action", "refresh")
            .with("kind", target.key())
            .with("name", name)
            .with("data", fresh.data);
        let answer = self.ask_value("mode_bindings", args)?;
        Ok(answer.get("updated").and_then(Value::as_u64).map_or(0, |n| usize::try_from(n).unwrap_or(0)))
    }
}
