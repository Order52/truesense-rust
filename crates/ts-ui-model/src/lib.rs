//! ts-ui-model: everything of the TrueSense app that is not Qt.
//!
//! The Qt app (`crates/ts-qt`) is a thin skin: QML draws, this crate decides. Keeping the logic
//! here means it is plain Rust, unit-tested without a display, and the same command contract
//! (`crates/ts-gui/src/gui/COMMANDS.md`) serves the window, the web fallback and the terminal UI.
//!
//! * [`bus`]: asynchronous commands over `ts_gui::gui::Commands`, with ordered, coalesced writes;
//! * [`live`] and [`poller`]: the newest live frame, history for sparklines, a ticker that only
//!   runs while the window is visible;
//! * [`json`], [`format`]: reading results and formatting values for display;
//! * `limits`: the limits the pages show, read from the layers that enforce them;
//! * [`routes`], [`capability`]: the page registry and "this machine lacks that" states;
//! * [`settings`], [`single`], [`pseudo`]: the app's own `ui.json`, one window per user, the
//!   pseudo-locale;
//! * [`vm`]: one view-model module per page, found by `build.rs`;
//! * [`app`]: all of it assembled into the one object the Qt bridge holds.
#![forbid(unsafe_code)]

pub mod app;
pub mod bus;
pub mod capability;
pub mod coalesce;
pub mod format;
pub mod hooks;
pub mod json;
pub(crate) mod limits;
pub mod live;
pub mod poller;
pub mod pseudo;
pub mod routes;
pub mod settings;
pub mod single;
pub mod vm;
