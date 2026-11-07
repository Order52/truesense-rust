//! The machine as a whole: autostart, updates, health check, services, self-test, configuration.
//!
//! Three things have no operation in the application layer because they need pieces only the
//! binary assembles (the self-test's fixtures, the journal, the start-up settings of the key
//! service). They are injected as [`Hooks`]; a missing hook makes its command say so.
#![forbid(unsafe_code)]

use std::sync::Arc;

use serde_json::{Value, json};
use ts_api::api::Api;

use super::args::{Args, SWITCH_NO_TOGGLE, on_off};
use super::{CommandError, Ctx, Outcome, lift, names, val};

/// Runs the hardware self-test.
pub trait SelfTest: Send + Sync {
    /// Runs it (`skip_fans` leaves every fan alone) and answers whether it passed with its report.
    fn run(&self, skip_fans: bool) -> (bool, String);
}

/// Reads the daemon's journal.
pub trait ServiceLogs: Send + Sync {
    /// The last lines of the daemon's log, or why they cannot be read.
    fn text(&self) -> Result<String, String>;
}

/// Changes and applies what the lights and the profile do at login (the key service owns it).
pub trait StartupControl: Send + Sync {
    /// Sets `target` (`keyboard`, `rear` or `profile`) to `value` and answers the value stored.
    fn set(&self, target: &str, value: &str) -> Result<String, String>;
    /// Applies the start-up settings now and lists what was done.
    fn apply(&self) -> Result<Vec<String>, String>;
}

/// What only the binary can provide; a missing one makes its command fail with a message.
#[derive(Clone, Default)]
pub struct Hooks {
    /// For `selftest run`.
    pub self_test: Option<Arc<dyn SelfTest>>,
    /// For `service logs`.
    pub service_logs: Option<Arc<dyn ServiceLogs>>,
    /// For `startup set` and `startup apply`.
    pub startup: Option<Arc<dyn StartupControl>>,
}

/// The handler of one command of this area; `None` when it is not one of ours.
pub(super) fn dispatch(c: &Ctx<'_>, key: &str, a: &Args) -> Option<Outcome> {
    lift(handle(c, key, a))
}

fn handle(c: &Ctx<'_>, key: &str, a: &Args) -> Result<Option<Value>, CommandError> {
    let api = c.api;
    Ok(Some(match key {
        "gui/autostart" => {
            on_off(a, SWITCH_NO_TOGGLE, || Ok(api.autostart()), |on| Ok(api.set_autostart(on)?))?
        }
        "update/check" => val(api.check_update(a.boolean("force")?))?,
        "update/apply" => update_apply(api, a)?,
        "update/auto" => on_off(
            a,
            SWITCH_NO_TOGGLE,
            || Ok(api.auto_update_check()?),
            |on| Ok(api.set_auto_update_check(on)?),
        )?,
        "doctor/run" => val(api.run_doctor())?,
        "service/check" => val(api.check_services()?)?,
        "service/logs" => service_logs(c)?,
        "service/repair" => {
            let rows: Vec<Value> = api
                .repair_user_services()?
                .into_iter()
                .map(|s| json!({ "step": s.what, "ok": s.ok }))
                .collect();
            Value::Array(rows)
        }
        "selftest/run" => selftest(c, a)?,
        "config/show" => api.config_values()?,
        "config/validate" => names(api.validate_config()),
        "config/reset-all" => json!({ "steps": val(api.reset_all())? }),
        _ => return Ok(None),
    }))
}

fn update_apply(api: &Api, a: &Args) -> Outcome {
    // The page asks the user first; the page is not trusted to be the only guard.
    if !a.boolean("yes")? {
        return Err(CommandError::failed("an update runs an installer as root: it needs yes: true"));
    }
    let plan = api.plan_update(None)?;
    if !plan.needed {
        return Err(CommandError::failed(format!(
            "TrueSense {} is already the newest version",
            plan.status.current
        )));
    }
    val(api.apply_update(Some(&plan.tag))?)
}

fn service_logs(c: &Ctx<'_>) -> Outcome {
    let logs = c
        .hooks
        .service_logs
        .as_ref()
        .ok_or_else(|| CommandError::failed("the service log cannot be read from here"))?;
    logs.text().map(|text| json!({ "text": text })).map_err(CommandError::failed)
}

fn selftest(c: &Ctx<'_>, a: &Args) -> Outcome {
    let skip_fans = a.boolean("skip_fans")?;
    let test = c
        .hooks
        .self_test
        .as_ref()
        .ok_or_else(|| CommandError::failed("the self-test is not available in this build"))?;
    let (ok, output) = test.run(skip_fans);
    Ok(json!({ "ok": ok, "output": output }))
}
