//! fnkeys: the user key service (`truesense keys run`, the `truesense-keys` user unit).
//!
//! The keyboard's vendor HID interface (usage page 0x88, report id 4) sends `04 80 FF` for Fn+F10,
//! `04 8A FF` for Fn+Esc, `04 81 FF` when the PREDATOR key goes down and `04 00 00` when anything
//! comes up. The PREDATOR key has four gestures (tap, double, triple within `[keys] multi_ms`,
//! and long, held for `[keys] long_ms`, which fires while the key is still held so it feels
//! instant); each runs a configurable action. Fn+Esc runs its own binding, Fn+F10 always toggles
//! the touchpad. Fn+F11 and Fn+F12 are brightness changes the keyboard makes itself and are left
//! alone.
//!
//! * [`find_node`]: which `/dev/hidrawN` is the interface.
//! * [`Gestures`]: the pure press-pattern state machine.
//! * [`HidReports`], [`HidOpener`], [`HidrawFile`]: reading reports with a timeout.
//! * [`Service`], [`Env`], [`run`]: the loop that ties them to the actions, the config reload,
//!   the reconnect after suspend and the systemd notifications.

mod gestures;
mod hid;
mod node;
mod service;

pub use gestures::Gestures;
pub use hid::{HidError, HidOpener, HidReports, HidrawFile, HidrawOpener};
pub use node::{find_node, find_node_in};
pub use service::{
    Background, BindingSource, Clock, ConfigBindings, Env, KeyActions, Logger, LoopTiming, MonotonicClock,
    RunnerActions, Service, Spawner, run, stdout_logger, thread_spawner,
};
