//! `profile`: named bundles of settings.
#![forbid(unsafe_code)]

use serde::Serialize;
use ts_api::api::Api;

use super::{Cx, Outcome, fail};
use crate::cli::argparse::Matches;

/// One line of `profile list`.
#[derive(Serialize)]
struct Row<'a> {
    name: &'a str,
    source: &'a str,
    description: &'a str,
}

pub(super) fn profile(cx: &mut Cx<'_, '_>, verb: Option<&str>, m: &Matches) -> Outcome {
    let api = cx.api;
    let name = m.str("name").unwrap_or_default();
    match verb.unwrap_or("list") {
        "list" => {
            let all = api.profiles();
            let rows: Vec<Row<'_>> = all
                .iter()
                .map(|p| Row { name: &p.name, source: &p.source, description: &p.description })
                .collect();
            let text = if all.is_empty() {
                "(no profiles)".to_owned()
            } else {
                all.iter()
                    .map(|p| format!("{:<16} [{:>7}] {}", p.name, p.source, p.description))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            cx.emit(&rows, Some(&text));
        }
        "show" => {
            let bundle = api.profile(name)?;
            cx.emit(&bundle, Some(&Api::profile_toml(&bundle)));
        }
        "apply" => {
            let said = api.apply_profile(name)?;
            cx.emit_text(&said);
        }
        "next" => {
            let said = api.next_profile()?;
            cx.emit_text(&said);
        }
        "save" => {
            let path = api.save_profile(name, m.str("description").unwrap_or_default())?;
            cx.emit(
                &path.display().to_string(),
                Some(&format!("saved the current state as profile '{name}'")),
            );
        }
        "delete" => {
            api.delete_profile(name)?;
            cx.emit_text(&format!("deleted {name}"));
        }
        other => return Err(fail(format!("unknown profile command {other}"))),
    }
    Ok(0)
}
