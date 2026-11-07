//! `update`: look for a newer release and install it.
#![forbid(unsafe_code)]

use ts_api::api::{describe_apply, describe_update};

use super::{Cx, Outcome, fail};
use crate::cli::argparse::Matches;

pub(super) fn update(cx: &mut Cx<'_, '_>, verb: Option<&str>, m: &Matches) -> Outcome {
    let api = cx.api;
    match verb.unwrap_or("check") {
        "check" => {
            let status = api.check_update(m.flag("force"));
            cx.emit(&status, Some(&describe_update(&status)));
        }
        "apply" => return apply(cx, m),
        "auto" => {
            cx.tri(
                m.str("action"),
                "check for updates once a day",
                || Ok(Some(api.auto_update_check()?)),
                |on| Ok(api.set_auto_update_check(on)?),
            )?;
        }
        "auto-run" => {
            api.notify_new_update()?;
        }
        other => return Err(fail(format!("unknown update command {other}"))),
    }
    Ok(0)
}

/// Asks the question of `update apply` and reads the answer; end of input counts as no.
fn confirmed(cx: &mut Cx<'_, '_>) -> bool {
    let _ = write!(cx.io.out, "Continue? [y/N] ");
    let _ = cx.io.out.flush();
    let mut line = String::new();
    if cx.io.input.read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
}

fn apply(cx: &mut Cx<'_, '_>, m: &Matches) -> Outcome {
    let api = cx.api;
    let plan = api.plan_update(m.str("version"))?;
    if !plan.needed {
        let text = format!("TrueSense {} is already the newest version", plan.status.current);
        cx.emit(&plan.status, Some(&text));
        return Ok(0);
    }
    if !m.flag("yes") {
        let prompt = format!(
            "Install TrueSense {} (you have {})?\nThis downloads the release from github.com/{} and runs its installer as root (the service restarts; fans return to the firmware meanwhile).",
            plan.tag.trim_start_matches('v'),
            plan.status.current,
            api.update_repo()
        );
        cx.say(&prompt);
        if !confirmed(cx) {
            return Err(fail("cancelled"));
        }
    }
    let result = api.apply_update(Some(&plan.tag))?;
    cx.emit(&result, Some(&describe_apply(&result)));
    if result.returncode != 0 {
        return Err(fail(format!("installer exit code {}", result.returncode)));
    }
    Ok(0)
}
