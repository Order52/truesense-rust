//! The live state: the readings of the last tick, kept for the `state` request and written as
//! `key=value` lines for status bars and `health.sh`.
//!
//! The file is rewritten on every tick: `health.sh` restarts the service when it goes stale, so
//! a daemon that stops publishing is judged dead. A path that cannot be written (a read-only
//! `/tmp`, no run directory) is not worth stopping for.

use std::path::Path;

use ts_foundation::state::LiveState;

use super::daemon::{Daemon, Inner};
use super::step::{Frame, rounded};

impl Daemon {
    /// Records the tick's readings as the live state and writes them to the status files. The
    /// keys and their order are the Python daemon's: status bars and `health.sh` were written
    /// against them.
    pub(super) fn publish(&self, g: &mut Inner, smart: bool, f: &Frame) {
        let kind = self.auto_kind();
        let mut live = LiveState::new();
        live.set("mode", if smart { "smart" } else { "manual" })
            .set("activity", g.activity.name())
            .set("fan", g.writer.written_label())
            .set("tcpu", rounded(f.tcpu))
            .set("tgpu", rounded(f.tgpu))
            .set("chassis", rounded(f.chassis.unwrap_or(0.0)))
            .set("cpu", rounded(f.load))
            .set("gpu", rounded(f.gpu.util))
            .set("gpuw", rounded(f.gpu.power))
            .set("rpm1", g.rpm.0)
            .set("rpm2", g.rpm.1)
            .set("cap", g.governor.cap() / 1000)
            .set("power_mode", g.mode_name().unwrap_or_else(|| "?".to_owned()))
            .set("ac", f.ac)
            .set("auto", kind != ts_decision::autoprofile::AutoKind::Off)
            .set("auto_mode", kind.name())
            .set("fan_curve", self.state.get_str("fan_curve", "native"))
            .set("smart_profile", self.state.get_str("smart_profile", "native"))
            .set("mode_reason", g.engine.reason())
            .set("fan_off", g.stop_watch.is_armed())
            .set("hw_ok", g.hw_ok);
        let mut paths: Vec<&Path> = vec![&self.live_paths.run_state, &self.live_paths.tmp_state];
        if self.cfg.compat.predator_widgets {
            paths.push(&self.live_paths.predator_state);
        }
        let failures = live.publish(&paths);
        g.live = live;
        if let Some(first) = failures.first() {
            self.warn_every("publish", 300.0, format!("could not write the live state: {first}"));
        }
    }
}
