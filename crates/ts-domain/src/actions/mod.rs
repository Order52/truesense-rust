//! Actions: the one vocabulary shared by the PREDATOR key, saved key profiles, whole-setup
//! profiles, `truesense keys test` and the window.
//!
//! An action is `name` or `name:argument`; it runs once and answers with a short message the
//! caller shows on screen. Unknown or malformed actions are rejected when they are configured
//! ([`Validator`]), never at key-press time.
//!
//! Everything an action touches (the firmware, the daemon, the keyboard, the sound system, the
//! user state, the shell) is reached through the traits of [`facade`], so each action is tested
//! with fakes and no process, device or socket. Every multi-step action walks an ordered list
//! (effects, modes, sections), never a hash map: the Go edition once had a map whose iteration
//! order made a profile apply in a different order on each run.
//!
//! * [`parse`]: the vocabulary, the catalogue text, validation.
//! * [`facade`]: the traits the assembly layer implements.
//! * [`runner`]: [`Runner`], which executes actions against an [`Env`]. The favourite actions
//!   need one more handle, added with [`Runner::with_favourites`], so that existing assemblies
//!   of an [`Env`] keep compiling.
#![forbid(unsafe_code)]

mod facade;
mod parse;
mod runner;

#[cfg(test)]
pub(crate) mod fakes;
#[cfg(test)]
mod runner_tests;

use std::fmt;
use std::sync::{Arc, OnceLock};

pub use facade::{
    ActionSettings, ActionState, AudioFacade, DaemonFacade, FanCommand, FanSnapshot, FavouritesFacade,
    HostError, HostResult, LightingFacade, LocalTouchpad, PowerFacade, ProfilesFacade, RearState, RearWrite,
    SettingsSource, Spawner, truthy,
};
pub use parse::{
    ARG_NAMES, Action, AudioOutput, AutoMode, CATALOGUE, FIXED_NAMES, KB_EFFECTS, REAR_EFFECTS, Switch,
    TouchpadAction, Validator, names, split, validate,
};
pub use runner::{Env, Runner};

/// Why an action was refused or failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionError {
    /// The action text is not valid; the message says why, in the words shown to the user.
    Invalid(String),
    /// The action is valid but this process cannot do it (no daemon, no spawner).
    Unavailable(String),
    /// The machine side failed. The Display text is the machine's own message, unchanged,
    /// because it becomes the on-screen message of the key press; `op` says which step failed.
    Host {
        /// The step that failed (`power`, `fan`, `rear light`, ...).
        op: &'static str,
        /// What the machine side said.
        source: HostError,
    },
}

impl fmt::Display for ActionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ActionError::Invalid(m) | ActionError::Unavailable(m) => f.write_str(m),
            ActionError::Host { source, .. } => fmt::Display::fmt(source, f),
        }
    }
}

impl std::error::Error for ActionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ActionError::Host { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Runs one action: what the whole-setup profiles use for the settings that are also key
/// actions (power held like a tile pick, fans, keyboard and rear on/off, lighting profiles),
/// so a profile and a key press cannot drift apart. [`Runner`] implements it.
pub trait ActionRunner: Send + Sync {
    /// Runs the action and returns the message to show (empty for none).
    fn run(&self, spec: &str) -> Result<String, ActionError>;
}

/// A runner that is wired in after the objects that need it exist. The assembly has a cycle
/// to break: a profile applies actions, and the `profile:` action applies profiles. Build the
/// profile store with a clone of the slot, build the [`Runner`], then [`RunnerSlot::set`] it.
#[derive(Clone, Default)]
pub struct RunnerSlot(Arc<OnceLock<Arc<dyn ActionRunner>>>);

impl RunnerSlot {
    /// An empty slot.
    pub fn new() -> RunnerSlot {
        RunnerSlot::default()
    }

    /// Fills the slot; returns `false` (and keeps the first runner) when it was already filled.
    pub fn set(&self, runner: Arc<dyn ActionRunner>) -> bool {
        self.0.set(runner).is_ok()
    }
}

impl ActionRunner for RunnerSlot {
    fn run(&self, spec: &str) -> Result<String, ActionError> {
        match self.0.get() {
            Some(r) => r.run(spec),
            None => Err(ActionError::Unavailable("actions are not wired up yet".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;
    impl ActionRunner for Echo {
        fn run(&self, spec: &str) -> Result<String, ActionError> {
            Ok(format!("ran {spec}"))
        }
    }

    #[test]
    fn slot_refuses_until_filled_then_delegates() {
        let slot = RunnerSlot::new();
        let clone = slot.clone();
        assert!(matches!(clone.run("none"), Err(ActionError::Unavailable(_))));
        assert!(slot.set(Arc::new(Echo)));
        assert!(!slot.set(Arc::new(Echo)), "the first runner stays");
        assert_eq!(clone.run("none").as_deref(), Ok("ran none"));
    }

    #[test]
    fn error_texts_are_the_users_words() {
        assert_eq!(ActionError::Invalid("unknown action 'x'".into()).to_string(), "unknown action 'x'");
        let e = ActionError::Host { op: "power", source: HostError::new("firmware refused") };
        assert_eq!(e.to_string(), "firmware refused");
        assert!(std::error::Error::source(&e).is_some());
    }
}
