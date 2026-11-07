//! Mode bindings: per power mode an optional lighting look and an optional fan curve, applied
//! when the power mode changes.
//!
//! The data lives in the daemon's state file as self-contained copies, because the daemon (root)
//! cannot read the looks and curves in a user's home directory and a binding has to work before
//! anyone logs in (the boot look and `fan_curve_data` follow the same rule). Each part is a
//! [`Bound`]: the name it was saved under, for the person's benefit, and the data itself.
//!
//! This module is the pure part: the data model, its validation, and [`edge`], the function that
//! says what to do when the mode changed from A to B. It reads no clock and touches nothing; the
//! daemon feeds it the modes it observed and carries out the answer. The rules it encodes:
//!
//! * a binding is applied once, on the change of the power mode, never on a timer and never for
//!   the first mode the daemon sees after it starts (so a restart does not repaint the lights);
//! * a look is applied whatever sets the mode (Smart auto, the user, the PREDATOR key, a game
//!   rule): the daemon only sees the mode change;
//! * a fan curve becomes the active curve only while the fans are in Smart mode. A mode change
//!   does not end a user's manual fan setting, and it does not switch Smart on;
//! * nothing that fails validation is ever applied, and a binding never stops a fan: a curve goes
//!   through [`curves::validate`] (the same rules as `fan curve use`, 100 % by the throttle
//!   temperatures included), the Smart follower turns any duty under 20 % into "firmware Auto"
//!   and never into a stop, and a native curve (which hands the fans to the firmware) is refused
//!   as a binding.
//!
//! A look is opaque here (`serde_json::Value`): this layer cannot depend on the lighting types.
//! The caller passes a parser for it, which both validates the data and returns the typed look
//! the daemon then applies.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::curves::{self, Curve, CurveError};
use crate::smartprofile::Mode;

/// The longest name a binding may carry: the limit of look and curve names.
pub const MAX_NAME: usize = 40;

/// The most bytes of JSON one look copy may hold. A per-key look is about 2 KB; the bound keeps a
/// hostile or broken client from filling the state file and the 64 KiB control-socket line.
pub(crate) const MAX_LOOK_BYTES: usize = 16 * 1024;

/// What a binding part is, for messages and for the wire (`look`, `fan_curve`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Target {
    /// The keyboard and rear light bar.
    Look,
    /// The fan curve of Smart mode.
    FanCurve,
}

impl Target {
    /// The key used in the state file and on the control socket.
    pub fn key(self) -> &'static str {
        match self {
            Self::Look => "look",
            Self::FanCurve => "fan_curve",
        }
    }

    /// Parses [`Target::key`].
    pub fn parse(text: &str) -> Option<Target> {
        match text {
            "look" => Some(Self::Look),
            "fan_curve" => Some(Self::FanCurve),
            _ => None,
        }
    }
}

impl fmt::Display for Target {
    /// The words a person reads: `look` and `fan curve`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Look => "look",
            Self::FanCurve => "fan curve",
        })
    }
}

/// Why a binding was refused. The text is meant to be shown to a person as it is.
#[derive(Debug)]
pub enum BindingError {
    /// A mode that is not one of the five.
    UnknownMode(String),
    /// A name that is empty, too long or has characters outside `A-Za-z0-9._-`.
    BadName(Target),
    /// A binding with neither a look nor a fan curve.
    Nothing,
    /// The stored value is not a map of mode names to bindings.
    Malformed(String),
    /// The look copy is not usable; the text is the parser's reason.
    Look(String),
    /// The look copy is bigger than [`MAX_LOOK_BYTES`].
    LookTooLarge,
    /// The fan curve copy does not validate.
    Curve(CurveError),
    /// The fan curve is the native one, which gives the fans to the firmware: that would end Smart
    /// mode, and a mode change must never do that.
    NativeCurve,
}

impl fmt::Display for BindingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownMode(m) => {
                write!(f, "unknown power mode '{m}' (use eco, quiet, balanced, performance or turbo)")
            }
            Self::BadName(t) => write!(f, "the {t} name uses letters, digits, . _ - (max {MAX_NAME})"),
            Self::Nothing => f.write_str("a binding needs a look and/or a fan curve"),
            Self::Malformed(why) => write!(f, "mode bindings are malformed: {why}"),
            Self::Look(why) => write!(f, "the look is not usable: {why}"),
            Self::LookTooLarge => write!(f, "the look is too large (over {MAX_LOOK_BYTES} bytes)"),
            Self::Curve(e) => write!(f, "the fan curve is not usable: {e}"),
            Self::NativeCurve => f.write_str(
                "the native curve cannot be bound to a mode: it hands the fans to the firmware (pick a curve of your own)",
            ),
        }
    }
}

impl std::error::Error for BindingError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Curve(e) => Some(e),
            _ => None,
        }
    }
}

fn valid_name(name: &str) -> bool {
    (1..=MAX_NAME).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// One bound part: a self-contained copy of a look or curve and the name it came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bound {
    /// The name in the user's store, shown in lists and used to refresh the copy when that item
    /// is saved again. It is a label here: nothing is looked up by it.
    pub name: String,
    /// The look object, or the curve object as [`Curve::to_value`] writes it.
    pub data: Value,
}

impl Bound {
    /// A look copy, checked with `parse` (which must accept exactly what the lights can apply).
    pub fn look<L>(
        name: &str,
        data: Value,
        parse: impl Fn(&Value) -> Result<L, String>,
    ) -> Result<Bound, BindingError> {
        check_look(name, &data, parse)?;
        Ok(Bound { name: name.to_owned(), data })
    }

    /// A fan curve copy. The data is validated and stored in its normal form (defaults filled in,
    /// points sorted), so what is stored is what will run.
    pub fn fan_curve(name: &str, data: &Value) -> Result<Bound, BindingError> {
        let curve = check_curve(name, data)?;
        Ok(Bound { name: name.to_owned(), data: curve.to_value() })
    }
}

fn check_look<L>(
    name: &str,
    data: &Value,
    parse: impl Fn(&Value) -> Result<L, String>,
) -> Result<L, BindingError> {
    if !valid_name(name) {
        return Err(BindingError::BadName(Target::Look));
    }
    // `to_string` of a `Value` cannot fail; its length is the size the state file will hold.
    if data.to_string().len() > MAX_LOOK_BYTES {
        return Err(BindingError::LookTooLarge);
    }
    parse(data).map_err(BindingError::Look)
}

fn check_curve(name: &str, data: &Value) -> Result<Curve, BindingError> {
    if !valid_name(name) {
        return Err(BindingError::BadName(Target::FanCurve));
    }
    let curve = curves::validate(data).map_err(BindingError::Curve)?;
    if curve.native {
        return Err(BindingError::NativeCurve);
    }
    Ok(curve)
}

/// What one power mode is bound to. Either part may be absent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    /// The look applied when the mode is entered.
    #[serde(default)]
    pub look: Option<Bound>,
    /// The fan curve made active when the mode is entered, if the fans are in Smart mode.
    #[serde(default)]
    pub fan_curve: Option<Bound>,
}

impl Binding {
    /// Whether the mode is bound to nothing.
    pub fn is_empty(&self) -> bool {
        self.look.is_none() && self.fan_curve.is_none()
    }

    /// The part of this kind.
    pub fn part(&self, target: Target) -> Option<&Bound> {
        match target {
            Target::Look => self.look.as_ref(),
            Target::FanCurve => self.fan_curve.as_ref(),
        }
    }

    fn part_mut(&mut self, target: Target) -> &mut Option<Bound> {
        match target {
            Target::Look => &mut self.look,
            Target::FanCurve => &mut self.fan_curve,
        }
    }
}

/// Which parts of a mode's binding to remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clear {
    /// Only the look.
    Look,
    /// Only the fan curve.
    FanCurve,
    /// Both: the mode is bound to nothing afterwards.
    All,
}

impl Clear {
    /// Parses `look`, `fan_curve` or `all`.
    pub fn parse(text: &str) -> Option<Clear> {
        match text {
            "look" => Some(Self::Look),
            "fan_curve" => Some(Self::FanCurve),
            "all" => Some(Self::All),
            _ => None,
        }
    }
}

/// Every mode's binding. Modes without one are absent from the map, so the stored form is
/// `{"turbo": {"look": {...}, "fan_curve": null}}` and a daemon that never heard of bindings
/// simply carries an unknown key.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Bindings(BTreeMap<String, Binding>);

impl Bindings {
    /// Decodes the stored value. `null` and a missing value are no bindings. Only the shape and
    /// the mode names are checked here: each part is validated when it is used ([`edge`]) or set,
    /// so one damaged copy cannot disable the others.
    pub fn from_value(value: &Value) -> Result<Bindings, BindingError> {
        if value.is_null() {
            return Ok(Bindings::default());
        }
        // A derived struct decoder also takes a JSON list (by position): a binding is an object.
        if let Some(entry) = value.as_object().and_then(|m| m.values().find(|v| !v.is_object())) {
            return Err(BindingError::Malformed(format!("a binding must be an object, not {entry}")));
        }
        let mut bindings: Bindings =
            serde_json::from_value(value.clone()).map_err(|e| BindingError::Malformed(e.to_string()))?;
        if let Some(name) = bindings.0.keys().find(|k| Mode::parse(k).is_none()) {
            return Err(BindingError::UnknownMode(name.clone()));
        }
        bindings.0.retain(|_, b| !b.is_empty());
        Ok(bindings)
    }

    /// The value as it is stored and sent: only modes that are bound appear.
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    /// Whether no mode is bound.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The binding of a mode, if it has one.
    pub fn get(&self, mode: Mode) -> Option<&Binding> {
        self.0.get(mode.name())
    }

    /// The bound modes in rank order, eco first.
    pub fn iter(&self) -> impl Iterator<Item = (Mode, &Binding)> {
        Mode::ALL.into_iter().filter_map(|m| self.get(m).map(|b| (m, b)))
    }

    /// Binds `mode` to the given parts; a part left `None` keeps what the mode had. At least one
    /// part must be given. Returns the new binding of the mode.
    pub fn set(
        &mut self,
        mode: Mode,
        look: Option<Bound>,
        fan_curve: Option<Bound>,
    ) -> Result<&Binding, BindingError> {
        if look.is_none() && fan_curve.is_none() {
            return Err(BindingError::Nothing);
        }
        let entry = self.0.entry(mode.name().to_owned()).or_default();
        if look.is_some() {
            entry.look = look;
        }
        if fan_curve.is_some() {
            entry.fan_curve = fan_curve;
        }
        Ok(entry)
    }

    /// Removes parts of a mode's binding. A mode left with nothing is dropped from the map.
    /// Returns whether anything was removed.
    pub fn clear(&mut self, mode: Mode, what: Clear) -> bool {
        let Some(entry) = self.0.get_mut(mode.name()) else { return false };
        let before = entry.clone();
        match what {
            Clear::Look => entry.look = None,
            Clear::FanCurve => entry.fan_curve = None,
            Clear::All => *entry = Binding::default(),
        }
        let changed = *entry != before;
        if entry.is_empty() {
            self.0.remove(mode.name());
        }
        changed
    }

    /// Replaces the copy of every part that carries `name` with `data`, in all modes: what a
    /// saved look or curve does to the bindings that name it, so the daemon's copy is not stale.
    /// `data` must already be validated (use [`Bound::look`] or [`Bound::fan_curve`]). Returns
    /// how many parts were replaced.
    pub fn refresh(&mut self, target: Target, fresh: &Bound) -> usize {
        let mut replaced = 0;
        for binding in self.0.values_mut() {
            if let Some(part) = binding.part_mut(target).as_mut().filter(|p| p.name == fresh.name) {
                part.data = fresh.data.clone();
                replaced += 1;
            }
        }
        replaced
    }
}

/// How the fans are run, as far as a binding cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanMode {
    /// TrueSense's curve runs the fans: the only mode in which a curve binding applies.
    Smart,
    /// The firmware runs the fans (Auto, or the native curve).
    Firmware,
    /// The user set the fans by hand (a duty, Max, or a stop). That choice outranks a binding.
    Manual,
}

/// Why a part was not applied. Not an error: the person should know, and the journal says so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
    /// The fans are not in Smart mode, so a curve would run nothing.
    FanNotSmart,
    /// The user's manual fan setting stays.
    ManualFanKept,
    /// The copy failed validation; the text is why. It is never applied.
    Invalid(String),
}

impl fmt::Display for Skip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FanNotSmart => f.write_str("the fans are not in Smart mode"),
            Self::ManualFanKept => f.write_str("a manual fan setting is held"),
            Self::Invalid(why) => write!(f, "invalid, not applied: {why}"),
        }
    }
}

/// One thing to do for a mode. `L` is the typed look the caller's parser produced.
#[derive(Debug, Clone, PartialEq)]
pub enum Action<L> {
    /// Put the look on the keyboard and the rear light.
    ApplyLook {
        /// The binding's name for it.
        name: String,
        /// The validated look.
        look: L,
    },
    /// Make the curve the active fan curve (Smart mode only).
    ApplyCurve {
        /// The binding's name for it.
        name: String,
        /// The validated curve.
        curve: Curve,
    },
    /// Leave this part alone, and why.
    Skip {
        /// Which part.
        target: Target,
        /// The binding's name for it.
        name: String,
        /// The reason.
        why: Skip,
    },
}

/// What a mode's binding asks for now, without asking whether the mode just changed (the "apply
/// now" request of a person). The look comes first, then the curve.
pub fn plan<L>(
    now: Mode,
    fan: FanMode,
    bindings: &Bindings,
    parse_look: impl Fn(&Value) -> Result<L, String>,
) -> Vec<Action<L>> {
    let Some(binding) = bindings.get(now) else { return Vec::new() };
    let mut actions = Vec::new();
    if let Some(bound) = &binding.look {
        let name = bound.name.clone();
        actions.push(match check_look(&bound.name, &bound.data, &parse_look) {
            Ok(look) => Action::ApplyLook { name, look },
            Err(e) => Action::Skip { target: Target::Look, name, why: Skip::Invalid(e.to_string()) },
        });
    }
    if let Some(bound) = &binding.fan_curve {
        let name = bound.name.clone();
        actions.push(match fan {
            FanMode::Manual => Action::Skip { target: Target::FanCurve, name, why: Skip::ManualFanKept },
            FanMode::Firmware => Action::Skip { target: Target::FanCurve, name, why: Skip::FanNotSmart },
            FanMode::Smart => match check_curve(&bound.name, &bound.data) {
                Ok(curve) => Action::ApplyCurve { name, curve },
                Err(e) => Action::Skip { target: Target::FanCurve, name, why: Skip::Invalid(e.to_string()) },
            },
        });
    }
    actions
}

/// The mode changed from `prev` to `now`: what to do. Nothing when there was no earlier mode (the
/// first mode seen after the daemon started) or the mode is the same.
pub fn edge<L>(
    prev: Option<Mode>,
    now: Mode,
    fan: FanMode,
    bindings: &Bindings,
    parse_look: impl Fn(&Value) -> Result<L, String>,
) -> Vec<Action<L>> {
    match prev {
        Some(before) if before != now => plan(now, fan, bindings, parse_look),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests;
