//! `service`, `config`, `doctor`, `probe`, `selftest`, `completion`, `gui`, `tui`, `daemon` and
//! `version`: the commands about the program and its surroundings.
#![forbid(unsafe_code)]

use std::io::Write;
use std::sync::atomic::Ordering;

use serde_json::{Value, json};
use ts_api::api::Api;
use ts_ipc::wire::Args;
use ts_system::doctor::{CallError, render_json, render_text};
use ts_system::selftest::{self, Automation, CpuInfo, Deps, Options};
use ts_system::services::{Step, render};

use super::{Cx, Obj, Outcome, fail};
use crate::cli::argparse::Matches;
use crate::cli::{completion, tree};

pub(super) fn service(cx: &mut Cx<'_, '_>, verb: Option<&str>, m: &Matches) -> Outcome {
    let api = cx.api;
    match verb.unwrap_or("status") {
        "status" => cx.launcher.service_status().map_err(fail)?,
        "restart" => cx.launcher.service_restart().map_err(fail)?,
        "logs" => cx.launcher.service_logs().map_err(fail)?,
        "check" => {
            let rows = api.check_services()?;
            let all_ok = rows.iter().all(|r| r.ok);
            let footer = if all_ok {
                "\n\nall good"
            } else {
                "\n\nsomething is not running: truesense service repair"
            };
            cx.emit(&rows, Some(&format!("{}{footer}", render(&rows))));
            return Ok(i32::from(!all_ok));
        }
        "repair" => return repair(cx, m.flag("system")),
        other => return Err(fail(format!("unknown service command {other}"))),
    }
    Ok(0)
}

fn repair(cx: &mut Cx<'_, '_>, system_part: bool) -> Outcome {
    let api = cx.api;
    if system_part {
        // The part that needs root; the other part calls this through sudo and reads one JSON line.
        let steps = api.repair_system_services()?;
        let text = crate::cli::out::dumps_inline(&steps);
        cx.say(&text);
        return Ok(i32::from(!steps.iter().all(|s| s.ok)));
    }
    let root = api.services().euid == 0;
    let checks = api.services().service_checks.as_ref();
    let mut steps: Vec<Step> = Vec::new();
    if root {
        steps.push(Step {
            what: "your user services were not touched (you ran this as root): run it again without sudo for them"
                .to_owned(),
            ok: true,
        });
        let checks = checks.ok_or_else(|| fail("the service manager is not available here"))?;
        steps.extend(checks.repair_system());
    } else {
        let checks = checks.ok_or_else(|| fail("the service manager is not available here"))?;
        steps.extend(checks.repair_user());
        match cx.launcher.repair_system_with_sudo() {
            Some(system) => steps.extend(system),
            None => steps.push(Step {
                what: "the system part did not run (sudo was refused or failed)".to_owned(),
                ok: false,
            }),
        }
    }
    let rows: Vec<Value> = steps.iter().map(|s| json!({"step": s.what, "ok": s.ok})).collect();
    let text = steps
        .iter()
        .map(|s| format!("  [{}] {}", if s.ok { "ok " } else { "!! " }, s.what))
        .collect::<Vec<_>>()
        .join("\n");
    cx.emit(&rows, Some(&text));
    Ok(i32::from(!steps.iter().all(|s| s.ok)))
}

pub(super) fn config(cx: &mut Cx<'_, '_>, verb: Option<&str>) -> Outcome {
    let api = cx.api;
    match verb.unwrap_or("show") {
        "path" => {
            let paths: Vec<String> = api.config_paths().iter().map(|p| p.display().to_string()).collect();
            cx.emit(&paths, None);
        }
        "validate" => {
            let problems = api.validate_config();
            let text =
                if problems.is_empty() { "configuration valid".to_owned() } else { problems.join("\n") };
            cx.emit(&problems, Some(&text));
            return Ok(i32::from(!problems.is_empty()));
        }
        _ => {
            let values = api.config_values()?;
            let text = api.config_toml()?;
            cx.emit(&values, Some(&text));
        }
    }
    Ok(0)
}

pub(super) fn doctor(cx: &mut Cx<'_, '_>) -> Outcome {
    let report = cx.api.run_doctor();
    if cx.json {
        let text = render_json(&report).map_err(fail)?;
        cx.say(&text);
    } else {
        let text = render_text(&report, cx.io.colour);
        let _ = cx.io.out.write_all(text.as_bytes());
    }
    Ok(report.exit_code())
}

pub(super) fn probe(cx: &mut Cx<'_, '_>) -> Outcome {
    let report = cx.api.probe();
    let text = if cx.json {
        ts_system::models::to_json(&report).map_err(fail)?
    } else {
        ts_system::models::render(&report)
    };
    cx.say(&text);
    Ok(0)
}

pub(super) fn completion(cx: &mut Cx<'_, '_>, m: &Matches) -> Outcome {
    let root = tree::build(ts_foundation::buildinfo::VERSION);
    let script = completion::generate(&root, m.str("shell").unwrap_or_default())
        .ok_or_else(|| fail("shell: bash | zsh | fish"))?;
    cx.say(&script);
    Ok(0)
}

pub(super) fn gui(cx: &mut Cx<'_, '_>, m: &Matches) -> Outcome {
    Ok(cx.launcher.gui(m.str("page")))
}

pub(super) fn tui(cx: &mut Cx<'_, '_>, m: &Matches) -> Outcome {
    Ok(cx.launcher.tui(m.str("page")))
}

pub(super) fn daemon(cx: &mut Cx<'_, '_>) -> Outcome {
    Ok(cx.launcher.daemon())
}

pub(super) fn version(cx: &mut Cx<'_, '_>) -> Outcome {
    let text = format!("truesense {}", Api::version());
    cx.emit(&Obj(vec![("truesense", json!(Api::version()))]), Some(&text));
    Ok(0)
}

// ---- selftest -------------------------------------------------------------------------------------------------------------------------

/// The daemon as the self-test sees it.
struct SelftestDaemon<'a>(&'a Api);

fn call_error(e: impl std::fmt::Display) -> CallError {
    CallError::new(e.to_string())
}

impl selftest::Daemon for SelftestDaemon<'_> {
    fn automation(&self) -> Result<Automation, CallError> {
        let state = self.0.daemon_state().map_err(call_error)?;
        Ok(Automation {
            smart_fan: state.setting_bool("smart_fan").unwrap_or(false),
            auto_profile: state.setting_bool("auto_profile").unwrap_or(false),
        })
    }

    fn set_automation(&self, automation: Automation) -> Result<(), CallError> {
        self.0.daemon_set("smart_fan", automation.smart_fan).map_err(call_error)?;
        self.0.daemon_set("auto_profile", automation.auto_profile).map_err(call_error)?;
        Ok(())
    }

    fn cpu_info(&self) -> Result<CpuInfo, CallError> {
        let value = self.0.services().daemon.call("cpu_info", Args::new()).map_err(call_error)?;
        // The daemon reports the limits as decimal watts (45.0).
        let watts = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_f64)
                .map(|w| w.round() as i64)
                .ok_or_else(|| CallError::new(format!("cpu_info: no {key}")))
        };
        let user = value.get("user").is_some_and(|u| !u.is_null());
        Ok(CpuInfo { pl1_w: watts("pl1_w")?, pl2_w: watts("pl2_w")?, user })
    }
}

/// The keyboard controller as the self-test sees it.
struct SelftestKeyboard<'a>(&'a Api);

impl selftest::Keyboard for SelftestKeyboard<'_> {
    fn brightness(&self) -> Result<i64, CallError> {
        self.0.keyboard_brightness().map(i64::from).map_err(call_error)
    }
}

/// The sound system as the self-test sees it.
struct SelftestAudio<'a>(&'a Api);

impl selftest::Audio for SelftestAudio<'_> {
    fn output_active(&self) -> Result<String, CallError> {
        self.0.audio_output().map(|o| o.active).map_err(call_error)
    }

    fn headset_boost(&self) -> Result<Option<u8>, CallError> {
        Ok(self.0.headset_boost())
    }

    fn set_headset_boost(&self, level: u8) -> Result<u8, CallError> {
        self.0.set_headset_boost(level).map_err(call_error)
    }

    fn mic_muted(&self) -> Result<bool, CallError> {
        Ok(self.0.mic().muted)
    }

    fn set_mic_mute(&self, muted: bool) -> Result<(), CallError> {
        self.0.set_mic_mute(muted, false);
        Ok(())
    }
}

pub(super) fn selftest(cx: &mut Cx<'_, '_>, m: &Matches) -> Outcome {
    let api = cx.api;
    let clock = cx.launcher.clock();
    let interrupt = cx.launcher.interrupt();
    let daemon = SelftestDaemon(api);
    let keyboard = SelftestKeyboard(api);
    let audio = SelftestAudio(api);
    let colour = cx.io.colour;
    let doctor = move |out: &mut dyn Write| -> i32 {
        let report = api.run_doctor();
        let _ = out.write_all(render_text(&report, colour).as_bytes());
        report.exit_code()
    };
    let cancelled = || interrupt.load(Ordering::SeqCst);
    let mut deps = Deps::new(clock.as_ref());
    deps.daemon = api.daemon_available().then_some(&daemon as &dyn selftest::Daemon);
    deps.keyboard = Some(&keyboard);
    deps.audio = Some(&audio);
    deps.doctor = Some(&doctor);
    deps.cancelled = &cancelled;
    let options = Options { skip_fans: m.flag("skip_fans") };
    let report = selftest::run(&api.services().hw, &deps, options, &mut *cx.io.out);
    Ok(i32::from(!report.ok()))
}
