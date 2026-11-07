//! The Monitor page's commands: `monitor overview|cpu|gpu|memory|network|disks|reset` and
//! `system info`. They are read-only, need no daemon and answer what `Api::monitor_*` returns
//! (see its module documentation for how often each may be called and why the GPU is never woken).
//! `COMMANDS.md` lists them under "Monitor commands (native app)".
#![forbid(unsafe_code)]

use serde_json::Value;
use ts_api::api::{Api, MONITOR_MAX_TOP};

use super::args::Args;
use super::{CommandError, Ctx, Outcome, lift, val};

/// The ranking size when a page does not say.
const DEFAULT_TOP: usize = 8;

/// The handler of one command of this area; `None` when it is not one of ours.
pub(super) fn dispatch(c: &Ctx<'_>, key: &str, a: &Args) -> Option<Outcome> {
    lift(handle(c.api, key, a))
}

fn handle(api: &Api, key: &str, a: &Args) -> Result<Option<Value>, CommandError> {
    Ok(Some(match key {
        "monitor/overview" => val(api.monitor_overview())?,
        "monitor/cpu" => val(api.monitor_cpu(top(a)?))?,
        "monitor/gpu" => val(api.monitor_gpu(detail(a)?))?,
        "monitor/memory" => val(api.monitor_memory(top(a)?))?,
        "monitor/network" => val(api.monitor_network())?,
        "monitor/disks" => val(api.monitor_disks())?,
        "monitor/reset" => {
            api.monitor_reset();
            Value::Bool(true)
        }
        "system/info" => val(api.system_info())?,
        _ => return Ok(None),
    }))
}

/// `top`: how many processes to list, 1 to the application layer's limit.
fn top(a: &Args) -> Result<usize, CommandError> {
    match a.int_opt("top")? {
        None => Ok(DEFAULT_TOP),
        Some(n) => usize::try_from(n)
            .ok()
            .filter(|n| (1..=MONITOR_MAX_TOP).contains(n))
            .ok_or_else(|| CommandError::failed(format!("top: a number from 1 to {MONITOR_MAX_TOP}"))),
    }
}

/// `detail`: whether the GPU tab wants the one-shot `nvidia-smi` query (default yes; the
/// application layer rate limits it and never makes it while the card sleeps).
fn detail(a: &Args) -> Result<bool, CommandError> {
    match a.raw("detail") {
        None => Ok(true),
        Some(_) => a.boolean("detail"),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use std::sync::Arc;

    use serde_json::{Value, json};
    use ts_api::api::{Api, Services};

    use crate::gui::bus::{ApiCommands, Commands, Hooks};
    use crate::gui::testutil::Temp;

    struct Rig {
        commands: ApiCommands,
        root: Temp,
    }

    impl Rig {
        fn new() -> Self {
            let root = Temp::new("monitor");
            let (services, _rig) = Services::simulated(&root.0).expect("services");
            let commands = ApiCommands::new(Arc::new(Api::new(services)), Hooks::default());
            Self { commands, root }
        }

        fn put(&self, rel: &str, text: &str) {
            let path = self.root.0.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        fn run(&self, area: &str, verb: &str, args: Value) -> Result<Value, String> {
            self.commands.run(area, verb, &args).map_err(|e| e.message().to_owned())
        }
    }

    #[test]
    fn every_monitor_command_answers_on_an_empty_machine() {
        let rig = Rig::new();
        for (area, verb) in [
            ("monitor", "overview"),
            ("monitor", "cpu"),
            ("monitor", "gpu"),
            ("monitor", "memory"),
            ("monitor", "network"),
            ("monitor", "disks"),
            ("system", "info"),
        ] {
            let v = rig.run(area, verb, json!({})).unwrap();
            assert!(v.is_object(), "{area} {verb}: {v}");
        }
        assert_eq!(rig.run("monitor", "reset", json!({})), Ok(json!(true)));
        assert_eq!(rig.run("monitor", "gpu", json!({})).unwrap()["state"], "absent");
    }

    #[test]
    fn a_missing_daemon_is_not_an_error() {
        // The simulated services have a running fake daemon that knows no command: the monitor
        // never asks it for anything it cannot do without.
        let rig = Rig::new();
        assert!(rig.run("monitor", "overview", json!({"extra": 1})).is_ok(), "unknown members are ignored");
        assert!(rig.run("monitor", "overview", json!(null)).is_ok());
    }

    #[test]
    fn top_is_checked() {
        let rig = Rig::new();
        rig.put("proc/stat", "cpu 1 0 0 1\n");
        assert!(rig.run("monitor", "memory", json!({"top": 3})).is_ok());
        assert!(rig.run("monitor", "cpu", json!({"top": 25})).is_ok());
        for bad in [
            json!({"top": 0}),
            json!({"top": 26}),
            json!({"top": -1}),
            json!({"top": 2.5}),
            json!({"top": "3"}),
        ] {
            let message = rig.run("monitor", "cpu", bad.clone()).unwrap_err();
            assert!(message.contains("top"), "{bad}: {message}");
        }
    }

    #[test]
    fn detail_is_a_flag_and_defaults_to_on() {
        let rig = Rig::new();
        rig.put("sys/bus/pci/devices/0000:01:00.0/vendor", "0x10de\n");
        rig.put("sys/bus/pci/devices/0000:01:00.0/class", "0x030200\n");
        rig.put("sys/bus/pci/devices/0000:01:00.0/power/runtime_status", "active\n");
        // over a fake root no program runs, so the query reports that instead of a detail
        let with = rig.run("monitor", "gpu", json!({})).unwrap();
        assert_eq!(with["state"], "active");
        assert!(with["error"].as_str().unwrap().contains("not available"), "{with}");
        let without = rig.run("monitor", "gpu", json!({"detail": false})).unwrap();
        assert_eq!(without["error"], Value::Null, "no query was made");
        assert!(rig.run("monitor", "gpu", json!({"detail": "yes"})).is_err());
        rig.put("sys/bus/pci/devices/0000:01:00.0/power/runtime_status", "suspended\n");
        let asleep = rig.run("monitor", "gpu", json!({})).unwrap();
        assert_eq!((asleep["state"].as_str(), asleep["error"].clone()), (Some("suspended"), Value::Null));
    }

    #[test]
    fn unknown_verbs_of_these_areas_are_unknown_commands() {
        let rig = Rig::new();
        for (area, verb) in [("monitor", "nope"), ("system", "nope")] {
            let e = rig.commands.run(area, verb, &json!({})).unwrap_err();
            assert!(e.is_unknown(), "{area} {verb}");
        }
    }

    #[test]
    fn results_carry_the_documented_members() {
        let rig = Rig::new();
        rig.put("proc/meminfo", "MemTotal: 1000 kB\nMemAvailable: 500 kB\n");
        let memory = rig.run("monitor", "memory", json!({"top": 1})).unwrap();
        assert_eq!(memory["memory"]["used_pct"], 50.0);
        let info = rig.run("system", "info", json!({})).unwrap();
        assert_eq!(info["memory_total_bytes"], 1_024_000);
        assert!(info["machine"]["tier"].is_string());
    }
}
