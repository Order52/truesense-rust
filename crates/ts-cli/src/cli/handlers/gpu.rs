//! `gpu`: the MUX mode and the GPU undervolt profiles.
#![forbid(unsafe_code)]

use ts_api::api::{Api, UndervoltRequest};
use ts_platform::hw::GpuMode;

use super::{Cx, Fail, Outcome, fail, narrow};
use crate::cli::argparse::Matches;

/// `none` for a clock cap of 0, the number otherwise.
fn cap_text(cap: i32) -> String {
    if cap == 0 { "none".to_owned() } else { cap.to_string() }
}

pub(super) fn gpu(cx: &mut Cx<'_, '_>, verb: Option<&str>, m: &Matches) -> Outcome {
    match verb {
        Some("undervolt") => undervolt(cx, m)?,
        Some("set") => {
            let mode: GpuMode = Api::parse_gpu_mode(m.str("mode").unwrap_or_default())?;
            let set = cx.api.set_gpu_mode(mode)?;
            cx.emit(&set, Some(&format!("GPU mode: {set}   (takes effect after a reboot)")));
        }
        _ => {
            let mode = cx.api.gpu_mode()?;
            let again = cx.api.gpu_mode()?;
            cx.emit(&mode, Some(&format!("GPU mode: {again}")));
        }
    }
    Ok(0)
}

fn undervolt(cx: &mut Cx<'_, '_>, m: &Matches) -> Result<(), Fail> {
    let api = cx.api;
    let name = m.str("name");
    let request = UndervoltRequest { confirm: m.flag("confirm"), force: m.flag("force") };
    match m.str("action").unwrap_or_default() {
        "get" => {
            let r = api.gpu_undervolt_state()?;
            cx.emit(&r, Some(&r.to_string()));
        }
        "list" => {
            let list = api.gpu_undervolts()?;
            let mut lines = Vec::new();
            for n in &list.names {
                let p = api.gpu_undervolt_profile(n)?;
                let mut line = format!("  {n:11} offset {:>+4} MHz  cap {:>5}", p.offset, cap_text(p.cap));
                if let Some(d) = list.descriptions.get(n) {
                    line.push_str(&format!("  - {d}"));
                }
                lines.push(line);
            }
            cx.emit(&list.names, Some(&lines.join("\n")));
        }
        "show" => {
            let shown = name.unwrap_or("truesense");
            let p = api.gpu_undervolt_profile(shown)?;
            let text = format!(
                "{shown}: offset {:+} MHz, cap {} MHz   {}",
                p.offset,
                cap_text(p.cap),
                p.description
            );
            cx.emit(&p, Some(&text));
        }
        "reset" => {
            let r = api.reset_gpu_undervolt()?;
            cx.emit(&r, Some("GPU undervolt reset: offset 0, no clock cap"));
        }
        "use" => {
            let used = name.unwrap_or_default();
            let r = api.apply_gpu_undervolt(used, request)?;
            let text = format!(
                "applied '{used}': {r}   (a reboot returns to stock; `truesense gpu undervolt reset` does it now)"
            );
            cx.emit(&r, Some(&text));
        }
        "copy" => {
            let from = name.unwrap_or_default();
            let made = api.copy_gpu_undervolt(from, m.str("new"))?;
            cx.emit(&made, Some(&format!("copied '{from}' to '{made}'")));
        }
        "save" => {
            let target = name.unwrap_or_default();
            let offset = m.int("offset").map(|o| narrow(o, "--offset")).transpose()?;
            let cap = narrow(m.int("cap").unwrap_or(0), "--cap")?;
            let saved = api.save_gpu_undervolt(target, offset, cap)?;
            let text = format!(
                "saved GPU undervolt profile '{target}' (use it: truesense gpu undervolt use {target} --confirm)"
            );
            cx.emit(&saved, Some(&text));
        }
        "delete" => {
            api.delete_gpu_undervolt(name.unwrap_or_default())?;
            let shown = name.unwrap_or("None");
            cx.emit(&name, Some(&format!("deleted GPU undervolt profile '{shown}'")));
        }
        "boot" => match name {
            None | Some("off") => {
                api.set_gpu_undervolt_boot(None, request)?;
                cx.emit(&(), Some("no GPU undervolt is applied at boot"));
            }
            Some(profile) => {
                let r = api.set_gpu_undervolt_boot(Some(profile), request)?;
                let text = format!("'{profile}' will be applied at boot (guarded, like the CPU undervolt)");
                cx.emit(&r, Some(&text));
            }
        },
        other => return Err(fail(format!("unknown gpu undervolt action {other}"))),
    }
    Ok(())
}
