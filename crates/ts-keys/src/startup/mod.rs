//! startup: what the lights and the profile do at login (`truesense startup apply`, run by the
//! `truesense-startup` user service) and the validated setter behind `truesense startup set` and
//! the window.
//!
//! The `[startup]` section of the configuration chooses:
//!
//! * `keyboard`, `rear`: `last` puts back what was set last (saved automatically on every
//!   change), `default` is the stock look, `none` leaves the firmware's own, `off` is dark.
//!   `keyboard` also takes `profile:NAME` (a saved lighting profile); both take `look:NAME`.
//! * `profile`: a whole-setup profile (power, fans, lighting, battery, audio) applied first, or
//!   empty.
//!
//! The login service must leave the machine as usable as it can, so a step that fails never stops
//! the others: [`apply`] answers one line per step, `FAILED (...)` included.
//!
//! The decisions live in [`login::Login`], which reaches the machine through the small
//! [`login::Lights`] trait; [`apply()`] and [`set_option`] are the entry points over an
//! [`Api`](ts_api::api::Api).
#![forbid(unsafe_code)]

pub mod login;
mod set;

#[cfg(test)]
mod tests;

pub use login::{ApplyOptions, Lights, Login, apply, apply_with};
pub use set::{TARGETS, py_repr, set_option};
