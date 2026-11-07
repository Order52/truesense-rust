//! adapt: the assembly of the application layer for a user-run client.
//!
//! [`crate::api`] says what every operation does and takes its dependencies as a [`Services`]
//! value; this module fills that value with the real ones. The command line, the terminal UI, the
//! window and the key service start here and nowhere else:
//!
//! ```text
//! let api = Api::new(Services::system()?);
//! ```
//!
//! * [`machine`]: everything read from the world (environment, `/sys`, programs, clocks) as one
//!   value, so the assembly itself is testable over a temporary tree;
//! * `system`: [`Services::system`] and [`Services::assemble`], every field of `Services` built
//!   from a [`Machine`], with the cycle between the action runner and the whole-setup profiles
//!   broken by a `RunnerSlot`;
//! * `facade`: the machine side of the PREDATOR-key actions (power, daemon, lighting, audio) over
//!   an `Api`, and the detached `exec:` spawner;
//! * [`osd`]: the on-screen display that reads its style from the configuration for every message;
//! * [`selftest`]: the self-test's daemon, keyboard and sound adapters over an `Api`;
//! * [`node`]: finding the keyboard's Fn-key interface, for the health check and the key service.
//!
//! The integration tests of this module start the real daemon on a temporary socket and drive it
//! through the real client and the `Api`: they are what proves that the two independently ported
//! halves agree on command names, argument names and result shapes.
#![forbid(unsafe_code)]

mod facade;
mod machine;
pub mod node;
pub mod osd;
pub mod selftest;
mod system;

#[cfg(test)]
mod testkit;
#[cfg(test)]
mod tests;

pub use facade::ShellSpawner;
pub use machine::Machine;
pub use node::{find_fn_key_node, is_fn_key_interface};

use crate::api::{Api, ApiResult, Services};

/// The application layer over the real machine: what a client builds first. The key service uses
/// it as it is (its actions, notifier and configuration paths are all in
/// [`Api::services`]) and finds the keyboard's Fn-key interface with [`find_fn_key_node`].
pub fn system_api() -> ApiResult<Api> {
    Ok(Api::new(Services::system()?))
}
