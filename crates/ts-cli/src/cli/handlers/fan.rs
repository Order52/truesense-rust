//! `fan`: firmware auto, max, manual duties, the smart curve and its curves.
#![forbid(unsafe_code)]

use serde_json::Value;
use ts_api::api::CurveEdit;

use super::{Cx, Fail, Obj, Outcome, fail};
use crate::cli::argparse::Matches;
use crate::cli::argparse::pyfmt::float_repr;

/// A number as the Python dictionary held it: whole numbers without a point.
fn whole_or_float(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 { format!("{n:.0}") } else { float_repr(n) }
}

/// A list of `[temperature, duty]` points as Python prints a list of lists.
fn points_repr(points: &[(f64, f64)]) -> String {
    let rows: Vec<String> =
        points.iter().map(|(t, d)| format!("[{}, {}]", whole_or_float(*t), whole_or_float(*d))).collect();
    format!("[{}]", rows.join(", "))
}

/// The keys of a curve in the order the Python dictionary had them.
const CURVE_KEYS: [&str; 9] =
    ["hysteresis", "down_rate", "predict", "couple", "battery_shift", "description", "native", "cpu", "gpu"];

fn ordered(value: &Value, keys: &[&'static str]) -> Obj {
    Obj(keys.iter().filter_map(|k| value.get(*k).map(|v| (*k, v.clone()))).collect())
}

pub(super) fn fan(cx: &mut Cx<'_, '_>, verb: Option<&str>, m: &Matches) -> Outcome {
    match verb.unwrap_or("status") {
        "status" => {
            let st = cx.api.fan_status()?;
            cx.emit(&st, Some(&st.to_string()));
        }
        "auto" => {
            let r = cx.api.fan_auto()?;
            cx.emit_text(&r.to_string());
        }
        "off" => {
            let r = cx.api.fan_off(m.flag("confirm"))?;
            cx.emit_text(&r.to_string());
        }
        "max" => {
            let r = cx.api.fan_max()?;
            cx.emit_text(&r.to_string());
        }
        "set" => {
            let cpu = m.int("cpu").unwrap_or_default();
            let r = cx.api.fan_set(cpu, m.int("gpu"), m.flag("confirm"))?;
            cx.emit_text(&r.to_string());
        }
        "curve" => curve(cx, m)?,
        "smart" => {
            let api = cx.api;
            cx.tri(
                m.str("action"),
                "smart fan curve",
                || Ok(Some(api.smart_fan()?)),
                |on| Ok(api.set_smart_fan(on)?),
            )?;
        }
        other => return Err(fail(format!("unknown fan command {other}"))),
    }
    Ok(0)
}

fn curve(cx: &mut Cx<'_, '_>, m: &Matches) -> Result<(), Fail> {
    let name = m.str("name");
    match m.str("action").unwrap_or("current") {
        "list" => {
            let list = cx.api.fan_curves()?;
            cx.emit(&list.names, Some(&list.to_string()));
        }
        "current" => {
            let d = cx.api.fan_curve_in_use()?;
            let cpu: Vec<(f64, f64)> = d.curve.cpu.iter().map(|p| (p.temp, p.duty)).collect();
            let gpu: Vec<(f64, f64)> = d.curve.gpu.iter().map(|p| (p.temp, p.duty)).collect();
            let text = format!(
                "fan curve in use: {}\n  cpu {}\n  gpu {}",
                d.name,
                points_repr(&cpu),
                points_repr(&gpu)
            );
            cx.emit(&d, Some(&text));
        }
        "show" => {
            let shown = name.unwrap_or("truesense");
            let c = cx.api.fan_curve(shown)?;
            let cpu: Vec<(f64, f64)> = c.cpu.iter().map(|p| (p.temp, p.duty)).collect();
            let gpu: Vec<(f64, f64)> = c.gpu.iter().map(|p| (p.temp, p.duty)).collect();
            let text = format!(
                "{shown}\n  cpu {}\n  gpu {}\n  hysteresis={}  down_rate={}  predict={}  couple={}  battery_shift={}",
                points_repr(&cpu),
                points_repr(&gpu),
                float_repr(c.hysteresis),
                float_repr(c.down_rate),
                float_repr(c.predict),
                whole_or_float(c.couple),
                float_repr(c.battery_shift)
            );
            cx.emit(&ordered(&c.to_value(), &CURVE_KEYS), Some(&text));
        }
        "use" => {
            let used = cx.api.use_fan_curve(name.unwrap_or_default())?;
            cx.emit(&used, Some(&format!("fan curve in use: {used}")));
        }
        "save" => {
            let target = name.unwrap_or_default();
            let edit = CurveEdit {
                cpu: m.str("cpu").map(str::to_owned),
                gpu: m.str("gpu").map(str::to_owned),
                hysteresis: m.float("hysteresis"),
                down_rate: m.float("down_rate"),
                predict: m.float("predict"),
                couple: m.float("couple"),
                battery_shift: m.float("battery_shift"),
                from: m.str("base").map(str::to_owned),
            };
            let saved = cx.api.save_fan_curve(target, &edit)?;
            let text = format!("saved fan curve '{target}' (use it: truesense fan curve use {target})");
            cx.emit(&ordered(&saved.to_value(), &CURVE_KEYS), Some(&text));
        }
        "copy" => {
            let from = name.unwrap_or_default();
            let made = cx.api.copy_fan_curve(from, m.str("new"))?;
            let text =
                format!("copied '{from}' to '{made}' (edit it: truesense fan curve save {made} --cpu ...)");
            cx.emit(&made, Some(&text));
        }
        _ => {
            cx.api.delete_fan_curve(name.unwrap_or_default())?;
            let shown = name.unwrap_or("None");
            cx.emit(&name, Some(&format!("deleted fan curve '{shown}'")));
        }
    }
    Ok(())
}
