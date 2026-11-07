//! `mode`: bind a look and a fan curve to a power mode (`mode bind turbo --look red --curve loud`).
//!
//! The service applies the binding once when the power mode changes, whoever changes it (Smart
//! auto, `power set`, the PREDATOR key, a game rule). This file holds the command tree of the
//! area and its handlers, apart from the shared ones because the area is new; the work, and every
//! rule, is in the application layer.
#![forbid(unsafe_code)]

use ts_platform::hw::{Coded, PowerMode};

use super::argparse::{Arg, Command, Matches};
use super::handlers::{Cx, Outcome, fail};

/// The area and its verbs.
pub(super) fn command() -> Command {
    let modes: Vec<String> = PowerMode::ALL.iter().map(|m| m.name().to_owned()).collect();
    Command::new("mode")
        .help("a look and a fan curve for each power mode, applied when the mode changes")
        .subs(
            "verb",
            vec![
                Command::new("bindings").help("what each power mode is bound to"),
                Command::new("bind")
                    .help("bind a power mode to a saved look and/or a fan curve (the service keeps a copy and applies it when the mode changes; a curve only while the fans are on Smart)")
                    .arg(Arg::pos("mode").choices_owned(modes.clone()))
                    .arg(Arg::opt("--look").help("a saved or built-in look"))
                    .arg(Arg::opt("--curve").help("a saved or built-in fan curve (not native)")),
                Command::new("unbind")
                    .help("remove a mode's binding (nothing is changed on the machine)")
                    .arg(Arg::pos("mode").choices_owned(modes.clone()))
                    .arg(Arg::flag("--look").help("remove only the look"))
                    .arg(Arg::flag("--curve").help("remove only the fan curve")),
                Command::new("apply")
                    .help("apply a mode's binding now (default: the current mode)")
                    .arg(Arg::pos("mode").optional().choices_owned(modes)),
            ],
        )
}

/// Runs a verb of the area.
pub(super) fn run(cx: &mut Cx<'_, '_>, verb: Option<&str>, m: &Matches) -> Outcome {
    let api = cx.api;
    match verb.unwrap_or("bindings") {
        "bindings" => {
            let shown = api.mode_bindings()?;
            cx.emit(&shown, Some(&shown.to_string()));
        }
        "bind" => {
            let (look, curve) = (m.str("look"), m.str("curve"));
            if look.is_none() && curve.is_none() {
                return Err(fail("give --look NAME and/or --curve NAME"));
            }
            let shown = api.bind_mode(m.str("mode").unwrap_or_default(), look, curve)?;
            cx.emit(&shown, Some(&shown.to_string()));
        }
        "unbind" => {
            let what = match (m.flag("look"), m.flag("curve")) {
                (true, false) => "look",
                (false, true) => "fan_curve",
                _ => "all",
            };
            let shown = api.unbind_mode(m.str("mode").unwrap_or_default(), what)?;
            cx.emit(&shown, Some(&shown.to_string()));
        }
        "apply" => {
            let done = api.apply_mode_binding(m.str("mode"))?;
            cx.emit(&done, Some(&done.to_string()));
        }
        other => return Err(fail(format!("unknown mode command {other}"))),
    }
    Ok(0)
}
