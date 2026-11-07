//! `alerts`: the desktop alerts the service raises (a hot CPU or GPU, a low battery, a clock held
//! down, a lost sensor) and their settings. New in 0.5: the Python and Go editions have no such
//! command, so its help page is not part of the captured reference (`testdata/help-spec.txt`
//! carries it as an addition).
//!
//! The command only edits the `[alerts]` section of the user's configuration file; the service
//! raises the alerts and picks the change up within about ten seconds.
#![forbid(unsafe_code)]

use ts_api::api::{AlertsEdit, describe_alerts};

use super::argparse::{Arg, Command, Matches};
use super::handlers::{Cx, Outcome, fail};

const ON_OFF: [&str; 2] = ["on", "off"];

/// The area's place in the command tree.
pub(super) fn command() -> Command {
    Command::new("alerts").help("desktop alerts: hot CPU/GPU, low battery, throttling").subs(
        "verb",
        vec![
            Command::new("show").help("the alert settings in force"),
            Command::new("set")
                .help(
                    "change settings (only what you give); the service applies them within about ten seconds",
                )
                .arg(Arg::opt("--enabled").choices(&ON_OFF).help("alerts on or off"))
                .arg(Arg::opt("--cpu-limit").int().help("alert at this CPU temperature, 70..100 C"))
                .arg(Arg::opt("--gpu-limit").int().help("alert at this GPU temperature, 70..100 C"))
                .arg(
                    Arg::opt("--repeat-minutes")
                        .int()
                        .help("least minutes between two messages of one alert, 1..240"),
                )
                .arg(Arg::opt("--debounce-samples").int().help("samples in a row before an alert, 1..30"))
                .arg(Arg::opt("--battery-low").int().help("battery alert level in percent, 5..50, 0 = off"))
                .arg(Arg::opt("--throttling").choices(&ON_OFF).help("tell when the CPU clock is held down"))
                .arg(
                    Arg::opt("--sensor-lost")
                        .choices(&ON_OFF)
                        .help("tell when the CPU temperature cannot be read"),
                ),
        ],
    )
}

/// Runs `alerts show` or `alerts set`.
pub(super) fn run(cx: &mut Cx<'_, '_>, verb: Option<&str>, m: &Matches) -> Outcome {
    let api = cx.api;
    let settings = match verb.unwrap_or("show") {
        "show" => api.alerts()?,
        "set" => api.set_alerts(&edit(m))?,
        other => return Err(fail(format!("unknown alerts command {other}"))),
    };
    cx.emit(&settings, Some(&describe_alerts(&settings)));
    Ok(0)
}

/// The words of `alerts set` as an edit: what was not given stays out.
fn edit(m: &Matches) -> AlertsEdit {
    let on = |dest: &str| m.str(dest).map(|word| word == "on");
    AlertsEdit {
        enabled: on("enabled"),
        cpu_limit_c: m.int("cpu_limit"),
        gpu_limit_c: m.int("gpu_limit"),
        repeat_minutes: m.int("repeat_minutes"),
        debounce_samples: m.int("debounce_samples"),
        battery_low_pct: m.int("battery_low"),
        throttling: on("throttling"),
        sensor_lost: on("sensor_lost"),
    }
}
