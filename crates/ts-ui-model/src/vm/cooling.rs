//! What the Cooling page decides: the fan curve editor's rules and geometry, the dials, the manual
//! duty request, the curve library rows and the "Fans for [power mode]" bindings.
//!
//! QML calls `Bus.vm("cooling", verb, argsJson)` (synchronously, a pure function of its arguments)
//! and draws what comes back. Nothing here reads a clock or touches the machine.
//!
//! # The curve rules
//!
//! The rules are those of `ts_decision::curves` (the daemon refuses anything else), reproduced here
//! because this crate does not depend on that layer: 2..=12 points per side; temperature 20..=110 C;
//! a duty of 0 (the firmware runs the fan, so it stops when cool) or 20..=100; the duty never falls
//! as the temperature rises; no two points share a temperature; and the side is at 100 % by 95 C
//! (CPU) or 85 C (GPU), because the chip throttles above that. The messages are the daemon's own
//! sentences. Every edit verb ([`drag`], [`nudge`], [`insert`], [`remove`]) returns points that pass
//! these rules whenever the points it was given did, so the editor can never produce a curve the
//! daemon would refuse.
//!
//! # Dragging
//!
//! A drag does not clamp each rule in turn (that leaves a point stuck at an odd place when two rules
//! meet). It looks at every position the point may take, on a grid of whole degrees and of duties
//! 0, 20, 25 ... 100, keeps those that keep the whole curve valid, and picks the one nearest to the
//! pointer in pixels. The grid is at most 91 x 18 positions, so this costs microseconds per move.
//!
//! The verbs: `limits`, `validate`, `geometry`, `hit`, `drag`, `nudge`, `insert`, `insert_middle`,
//! `remove`, `sample`, `preview`, `changed`, `dials`, `manual`, `library`, `copy_name`,
//! `valid_name`, `bindings`, `bind_request`.
#![forbid(unsafe_code)]

use serde_json::{Map, Value, json};

use crate::json::{at, bool_at, f64_at, i64_at, list_at, str_at};

/// The fewest points a side may have.
pub const MIN_POINTS: usize = 2;
/// The most points a side may have.
pub const MAX_POINTS: usize = 12;
/// The lowest temperature of a point (and the left edge of the plot).
pub(crate) const TEMP_MIN: f64 = 20.0;
/// The highest temperature of a point (and the right edge of the plot).
pub(crate) const TEMP_MAX: f64 = 110.0;
/// The lowest duty that holds a fan; below it the only valid value is 0.
pub(crate) const DUTY_MIN: f64 = 20.0;
/// The highest duty.
pub(crate) const DUTY_MAX: f64 = 100.0;
/// Percent per grid step of a dragged duty.
const DUTY_STEP: f64 = 5.0;
/// The CPU side must be at 100 % by this temperature.
pub(crate) const CPU_FULL_BY: f64 = 95.0;
/// The GPU side must be at 100 % by this temperature.
pub(crate) const GPU_FULL_BY: f64 = 85.0;
/// Two points must be at least this many degrees apart when one is inserted (a point closer than
/// that cannot be grabbed again with a pointer).
const MIN_GAP: f64 = 2.0;
/// The speed the dials end at: a fan at 100 % turns about 6050 RPM on this chassis.
const RPM_SCALE: f64 = 6500.0;
/// The reach of a click on a point, in pixels.
const HIT_RADIUS: f64 = 14.0;
/// The power modes in the order the page lists them.
const POWER_MODES: [&str; 5] = ["eco", "quiet", "balanced", "performance", "turbo"];
/// The built-in curves, `native` first (the library lists them before the user's own).
const BUILTIN: [&str; 5] = ["native", "truesense", "quiet", "performance", "max-cooling"];

/// The follower parameters: key, lowest, highest, slider step, default, unit.
const PARAMS: [(&str, f64, f64, f64, f64, &str); 5] = [
    ("hysteresis", 0.0, 15.0, 1.0, 4.0, "C"),
    ("down_rate", 0.2, 20.0, 0.1, 1.5, "%/s"),
    ("predict", 0.0, 15.0, 1.0, 5.0, "C"),
    ("couple", 0.0, 50.0, 1.0, 15.0, "%"),
    ("battery_shift", 0.0, 15.0, 1.0, 3.0, "C"),
];

/// The verbs of this module (see the module documentation).
///
/// # Errors
/// The text to show for an unknown verb or for arguments the verb cannot use (a refused edit says
/// why, in the daemon's words where it has some).
pub fn call(verb: &str, args: &Value) -> Result<Value, String> {
    match verb {
        "limits" => Ok(limits()),
        "validate" => Ok(validate(args)),
        "geometry" => geometry(args),
        "hit" => hit(args),
        "drag" => drag(args),
        "nudge" => nudge(args),
        "insert" => insert(args),
        "insert_middle" => insert_middle(args),
        "remove" => remove(args),
        "sample" => sample(args),
        "preview" => preview(args),
        "changed" => Ok(json!(changed(args))),
        "dials" => Ok(dials(args)),
        "manual" => manual(args),
        "library" => Ok(library(args)),
        "copy_name" => Ok(copy_name(args)),
        "valid_name" => Ok(json!(valid_name(str_at(args, "name").unwrap_or("")))),
        "bindings" => Ok(bindings(args)),
        "bind_request" => bind_request(args),
        _ => Err(format!("unknown cooling verb: {verb}")),
    }
}

// ---- the curve model ----------------------------------------------------------------------------

/// One curve point.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Pt {
    /// Temperature, degC.
    t: f64,
    /// Duty, percent (0 = firmware).
    d: f64,
}

/// Which fan a curve side drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Cpu,
    Gpu,
}

impl Side {
    fn parse(text: &str) -> Result<Side, String> {
        match text {
            "cpu" => Ok(Side::Cpu),
            "gpu" => Ok(Side::Gpu),
            other => Err(format!("side: cpu or gpu, not '{other}'")),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Side::Cpu => "cpu",
            Side::Gpu => "gpu",
        }
    }

    fn full_by(self) -> f64 {
        match self {
            Side::Cpu => CPU_FULL_BY,
            Side::Gpu => GPU_FULL_BY,
        }
    }
}

/// A whole number as a JSON integer, anything else as a float, so the daemon's files keep `56`.
fn num(v: f64) -> Value {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 1e15 {
        // The guard keeps the cast exact.
        json!(v as i64)
    } else {
        json!(v)
    }
}

fn points_json(pts: &[Pt]) -> Value {
    Value::Array(pts.iter().map(|p| json!([num(p.t), num(p.d)])).collect())
}

fn parse_points(value: Option<&Value>, what: &str) -> Result<Vec<Pt>, String> {
    let items = value
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{what}: a list of [temperature, percent] points is needed"))?;
    // A hostile list must not make a drag expensive; no curve may have more than this anyway.
    if items.len() > 64 {
        return Err(format!("{what}: 2 to {MAX_POINTS} points"));
    }
    items
        .iter()
        .map(|item| match item.as_array().map(Vec::as_slice) {
            Some([t, d]) => match (t.as_f64(), d.as_f64()) {
                (Some(t), Some(d)) if t.is_finite() && d.is_finite() => Ok(Pt { t, d }),
                _ => Err(format!("{what}: every point is [temperature, percent]")),
            },
            _ => Err(format!("{what}: every point is [temperature, percent]")),
        })
        .collect()
}

fn arg_points(args: &Value, key: &str) -> Result<Vec<Pt>, String> {
    parse_points(at(args, key), key)
}

fn arg_side(args: &Value) -> Result<Side, String> {
    Side::parse(str_at(args, "side").unwrap_or("cpu"))
}

fn arg_f64(args: &Value, key: &str) -> Result<f64, String> {
    f64_at(args, key).ok_or_else(|| format!("{key}: a number is needed"))
}

fn arg_index(args: &Value, len: usize) -> Result<usize, String> {
    i64_at(args, "index")
        .and_then(|i| usize::try_from(i).ok())
        .filter(|i| *i < len)
        .ok_or_else(|| "index: no such point".to_owned())
}

/// Piecewise-linear duty at `t`, flat before the first and after the last point.
fn interp(pts: &[Pt], t: f64) -> f64 {
    let (Some(first), Some(last)) = (pts.first(), pts.last()) else {
        return 0.0;
    };
    if t <= first.t {
        return first.d;
    }
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t <= b.t {
            return if b.t > a.t { a.d + (b.d - a.d) * (t - a.t) / (b.t - a.t) } else { b.d };
        }
    }
    last.d
}

/// The daemon's safety rules for one side, in the daemon's order and words.
fn check_side(side: Side, pts: &[Pt]) -> Result<(), String> {
    let name = side.name();
    if !(MIN_POINTS..=MAX_POINTS).contains(&pts.len()) {
        return Err(format!("{name}: 2 to 12 points"));
    }
    let mut sorted = pts.to_vec();
    sorted.sort_by(|a, b| a.t.total_cmp(&b.t).then(a.d.total_cmp(&b.d)));
    for p in &sorted {
        let temp_ok = TEMP_MIN <= p.t && p.t <= TEMP_MAX;
        let duty_ok = p.d == 0.0 || (DUTY_MIN <= p.d && p.d <= DUTY_MAX);
        if !temp_ok || !duty_ok {
            return Err(format!("{name}: temperature 20..110 C, percent 0 (firmware auto) or 20..100"));
        }
    }
    if sorted.windows(2).any(|w| w[1].d < w[0].d) {
        return Err(format!("{name}: the duty may only rise with temperature"));
    }
    if sorted.windows(2).any(|w| w[0].t == w[1].t) {
        return Err(format!("{name}: two points share a temperature"));
    }
    let limit = side.full_by();
    if interp(&sorted, limit) < 100.0 {
        return Err(format!("{name}: must reach 100 % by {limit} C (the CPU and GPU throttle above that)"));
    }
    Ok(())
}

/// The first rule a whole curve breaks (CPU first, then the GPU, then the parameters).
fn check_curve(curve: &Value) -> Result<(), String> {
    if !curve.is_object() {
        return Err("a curve is an object with 'cpu' and 'gpu' points".to_owned());
    }
    for side in [Side::Cpu, Side::Gpu] {
        let pts = match at(curve, side.name()) {
            Some(Value::Array(a)) if (MIN_POINTS..=MAX_POINTS).contains(&a.len()) => {
                parse_points(at(curve, side.name()), side.name())
                    .map_err(|_| format!("{}: every point is [temperature, percent]", side.name()))?
            }
            _ => return Err(format!("{}: 2 to 12 points", side.name())),
        };
        check_side(side, &pts)?;
    }
    for (key, lo, hi, _, _, _) in PARAMS {
        if let Some(v) = at(curve, key) {
            let ok = v.as_f64().is_some_and(|f| v.is_number() && lo <= f && f <= hi);
            if !ok {
                return Err(format!("{key}: {lo}..{hi}"));
            }
        }
    }
    Ok(())
}

fn validate(args: &Value) -> Value {
    let curve = at(args, "curve").cloned().unwrap_or(Value::Null);
    match check_curve(&curve) {
        Ok(()) => json!({"valid": true, "error": null}),
        Err(e) => json!({"valid": false, "error": e}),
    }
}

fn limits() -> Value {
    let params: Map<String, Value> = PARAMS
        .iter()
        .map(|&(key, lo, hi, step, default, unit)| {
            (key.to_owned(), json!({"min": lo, "max": hi, "step": step, "default": default, "unit": unit}))
        })
        .collect();
    json!({
        "temp_min": TEMP_MIN, "temp_max": TEMP_MAX, "duty_min": DUTY_MIN, "duty_max": DUTY_MAX,
        "duty_step": DUTY_STEP, "min_points": MIN_POINTS, "max_points": MAX_POINTS,
        "cpu_full_by": CPU_FULL_BY, "gpu_full_by": GPU_FULL_BY, "rpm_scale": RPM_SCALE,
        "params": params,
    })
}

// ---- geometry -----------------------------------------------------------------------------------

/// The plot box inside a widget, with the data <-> pixel mapping. The y axis shows duty 0..=100
/// (the 0..20 band is the firmware's "Auto").
#[derive(Debug, Clone, Copy)]
struct Plot {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

impl Plot {
    /// The box for a widget of `width` x `height` pixels and margins `[left, right, top, bottom]`
    /// (room for the axis labels). Never smaller than one pixel inside, so the mapping divides safely.
    fn new(args: &Value) -> Result<Plot, String> {
        let width = arg_f64(args, "width")?.clamp(1.0, 20_000.0);
        let height = arg_f64(args, "height")?.clamp(1.0, 20_000.0);
        let m = list_at(args, "margins")
            .filter(|l| l.len() == 4)
            .map(|l| l.iter().map(|v| v.as_f64().unwrap_or(0.0).clamp(0.0, 400.0)).collect::<Vec<_>>())
            .unwrap_or_else(|| vec![46.0, 14.0, 12.0, 30.0]);
        let (left, right, top, bottom) = (m[0], width - m[1], m[2], height - m[3]);
        Ok(Plot {
            left,
            top,
            right: right.max(left + 1.0),
            bottom: bottom.max(top + 1.0),
        })
    }

    fn w(&self) -> f64 {
        self.right - self.left
    }

    fn h(&self) -> f64 {
        self.bottom - self.top
    }

    fn x(&self, t: f64) -> f64 {
        self.left + (t - TEMP_MIN) / (TEMP_MAX - TEMP_MIN) * self.w()
    }

    fn y(&self, d: f64) -> f64 {
        self.top + (1.0 - d / 100.0) * self.h()
    }

    fn t(&self, x: f64) -> f64 {
        TEMP_MIN + (x - self.left) / self.w() * (TEMP_MAX - TEMP_MIN)
    }

    fn d(&self, y: f64) -> f64 {
        (1.0 - (y - self.top) / self.h()) * 100.0
    }

    fn px(&self, p: Pt) -> Value {
        json!([round1(self.x(p.t)), round1(self.y(p.d))])
    }
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// The polyline of a side across the whole plot: flat before the first and after the last point.
fn line_px(plot: &Plot, pts: &[Pt]) -> Value {
    let (Some(first), Some(last)) = (pts.first(), pts.last()) else {
        return json!([]);
    };
    let mut out = vec![plot.px(Pt { t: TEMP_MIN, d: first.d })];
    out.extend(pts.iter().map(|p| plot.px(*p)));
    out.push(plot.px(Pt { t: TEMP_MAX, d: last.d }));
    Value::Array(out)
}

/// Everything the editor draws, in pixels: the box, the ticks, both lines, the dots, the line where
/// the side must be at 100 %, and (when `temp` is given) the "now" marker on the curve.
fn geometry(args: &Value) -> Result<Value, String> {
    let plot = Plot::new(args)?;
    let side = arg_side(args)?;
    let pts = arg_points(args, "points")?;
    let other = parse_points(at(args, "other"), "other").unwrap_or_default();
    let x_ticks: Vec<Value> = (0..=9)
        .map(|k| {
            let t = TEMP_MIN + 10.0 * f64::from(k);
            json!({"value": t, "px": round1(plot.x(t))})
        })
        .collect();
    let y_ticks: Vec<Value> = (0..=5)
        .map(|k| {
            let d = 20.0 * f64::from(k);
            json!({"value": d, "px": round1(plot.y(d))})
        })
        .collect();
    let now = f64_at(args, "temp")
        .filter(|t| (TEMP_MIN..=TEMP_MAX).contains(t) && !pts.is_empty())
        .map(|t| {
            let duty = interp(&pts, t);
            json!({"temp": t, "duty": round1(duty), "x": round1(plot.x(t)), "y": round1(plot.y(duty))})
        });
    Ok(json!({
        "plot": {"left": plot.left, "top": plot.top, "right": plot.right, "bottom": plot.bottom},
        "x_ticks": x_ticks,
        "y_ticks": y_ticks,
        "auto_band": {"top": round1(plot.y(DUTY_MIN)), "bottom": round1(plot.y(0.0))},
        "limit": {"temp": side.full_by(), "x": round1(plot.x(side.full_by()))},
        "line": line_px(&plot, &pts),
        "other_line": line_px(&plot, &other),
        "dots": pts.iter().map(|p| plot.px(*p)).collect::<Vec<_>>(),
        "now": now,
    }))
}

/// The point under a pointer: the nearest dot within the click radius, or `null`.
fn hit(args: &Value) -> Result<Value, String> {
    let plot = Plot::new(args)?;
    let pts = arg_points(args, "points")?;
    let (x, y) = (arg_f64(args, "x")?, arg_f64(args, "y")?);
    let radius = f64_at(args, "radius").unwrap_or(HIT_RADIUS).clamp(1.0, 200.0);
    let nearest = pts
        .iter()
        .enumerate()
        .map(|(i, p)| (i, (plot.x(p.t) - x).hypot(plot.y(p.d) - y)))
        .filter(|(_, dist)| *dist <= radius)
        .min_by(|a, b| a.1.total_cmp(&b.1));
    Ok(nearest.map_or(Value::Null, |(i, _)| json!(i)))
}

// ---- editing ------------------------------------------------------------------------------------

/// The duties a point may take: 0 (firmware), then 20 to 100 in steps of 5.
fn duty_grid() -> impl Iterator<Item = f64> {
    std::iter::once(0.0).chain((4..=20).map(|k| f64::from(k) * DUTY_STEP))
}

/// The next grid duty above (`up`) or below `d` (0 and 20 are one step apart).
fn step_duty(d: f64, up: bool) -> f64 {
    if up {
        duty_grid().find(|g| *g > d + 1e-9).unwrap_or(DUTY_MAX)
    } else {
        duty_grid().filter(|g| *g < d - 1e-9).last().unwrap_or(0.0)
    }
}

/// Moves point `i` to the position that keeps the whole curve valid and is nearest to `target`
/// (distance measured with `weight` pixels per degree and per percent). See the module notes.
fn move_point(pts: &[Pt], side: Side, i: usize, target: (f64, f64), weight: (f64, f64)) -> Result<Vec<Pt>, String> {
    if pts.len() > MAX_POINTS {
        return Err(format!("{}: 2 to 12 points", side.name()));
    }
    if pts.windows(2).any(|w| w[1].t < w[0].t) {
        return Err("points: sorted by temperature are needed".to_owned());
    }
    let cur = *pts.get(i).ok_or_else(|| "index: no such point".to_owned())?;
    let (tt, td) = target;
    if !tt.is_finite() || !td.is_finite() {
        return Err("target: a finite position is needed".to_owned());
    }
    // Strictly between the neighbours in temperature, whole degrees inside the plot.
    let t_lo = if i == 0 { TEMP_MIN } else { TEMP_MIN.max(pts[i - 1].t.floor() + 1.0) };
    let t_hi = pts.get(i + 1).map_or(TEMP_MAX, |next| TEMP_MAX.min(next.t.ceil() - 1.0));
    // The duty may not cross a neighbour's (it only rises with temperature).
    let d_lo = if i == 0 { 0.0 } else { pts[i - 1].d };
    let d_hi = pts.get(i + 1).map_or(DUTY_MAX, |n| n.d);
    let limit = side.full_by();
    // The grid duties, and the point's own duty even when it is off the grid (a built-in curve
    // holds 38 or 48): moving a point sideways must not change the duty the person did not touch.
    let mut duties: Vec<f64> = duty_grid().filter(|d| *d >= d_lo && *d <= d_hi).collect();
    if cur.d >= d_lo && cur.d <= d_hi && !duties.contains(&cur.d) {
        duties.push(cur.d);
    }
    let mut work = pts.to_vec();
    let cost = |t: f64, d: f64| (weight.0 * (t - tt)).powi(2) + (weight.1 * (d - td)).powi(2);

    let mut best: Option<(f64, Pt)> = None;
    for enforce_limit in [true, false] {
        let mut consider = |t: f64, d: f64, work: &mut Vec<Pt>| {
            work[i] = Pt { t, d };
            if enforce_limit && interp(work, limit) < 100.0 {
                return;
            }
            let c = cost(t, d);
            if best.is_none_or(|(b, _)| c < b) {
                best = Some((c, Pt { t, d }));
            }
        };
        // The point where it is stays a candidate: a loaded curve may hold a temperature off the
        // grid, and a move must never be worse than none.
        if cur.d >= d_lo && cur.d <= d_hi {
            consider(cur.t, cur.d, &mut work);
        }
        let mut t = t_lo;
        while t <= t_hi {
            for &d in &duties {
                consider(t, d, &mut work);
            }
            t += 1.0;
        }
        // The second pass (without the 100 % rule) is for a curve that was invalid to begin with.
        if best.is_some() {
            break;
        }
    }
    let mut out = pts.to_vec();
    if let Some((_, p)) = best {
        out[i] = p;
    }
    Ok(out)
}

fn edit_reply(pts: &[Pt], index: usize) -> Value {
    json!({"points": points_json(pts), "index": index})
}

/// A pointer drag: `{points, side, index, x, y, width, height, margins?}` gives `{points, index}`.
fn drag(args: &Value) -> Result<Value, String> {
    let plot = Plot::new(args)?;
    let side = arg_side(args)?;
    let pts = arg_points(args, "points")?;
    let i = arg_index(args, pts.len())?;
    let (x, y) = (arg_f64(args, "x")?, arg_f64(args, "y")?);
    let target = (plot.t(x), plot.d(y));
    let weight = (plot.w() / (TEMP_MAX - TEMP_MIN), plot.h() / 100.0);
    let moved = move_point(&pts, side, i, target, weight)?;
    Ok(edit_reply(&moved, i))
}

/// A keyboard step: `{points, side, index, dt, dd}` (each -1, 0 or 1: one degree, one duty step).
fn nudge(args: &Value) -> Result<Value, String> {
    let side = arg_side(args)?;
    let pts = arg_points(args, "points")?;
    let i = arg_index(args, pts.len())?;
    let cur = pts[i];
    let dt = i64_at(args, "dt").unwrap_or(0).clamp(-1, 1);
    let dd = i64_at(args, "dd").unwrap_or(0).clamp(-1, 1);
    let target_d = match dd {
        1 => step_duty(cur.d, true),
        -1 => step_duty(cur.d, false),
        _ => cur.d,
    };
    let moved = move_point(&pts, side, i, (cur.t + dt as f64, target_d), (1.0, 1.0))?;
    Ok(edit_reply(&moved, i))
}

/// The grid duty nearest to `d` inside `[lo, hi]`; `d` itself when no grid duty fits.
fn snap_duty(d: f64, lo: f64, hi: f64) -> f64 {
    duty_grid()
        .filter(|g| *g >= lo && *g <= hi)
        .min_by(|a, b| (a - d).abs().total_cmp(&(b - d).abs()))
        .unwrap_or_else(|| d.clamp(lo, hi))
}

/// Adds a point at temperature `t` on the existing line, so the shape does not change.
fn insert_at(pts: &[Pt], side: Side, t: f64) -> Result<(Vec<Pt>, usize), String> {
    if !t.is_finite() {
        return Err("t: a temperature is needed".to_owned());
    }
    if pts.len() >= MAX_POINTS {
        return Err("A curve has at most 12 points.".to_owned());
    }
    let t = t.round().clamp(TEMP_MIN, TEMP_MAX);
    if pts.iter().any(|p| (p.t - t).abs() < MIN_GAP) {
        return Err("Too close to another point: leave at least 2 degrees between points.".to_owned());
    }
    let index = pts.iter().take_while(|p| p.t < t).count();
    let lo = if index == 0 { 0.0 } else { pts[index - 1].d };
    let hi = pts.get(index).map_or(DUTY_MAX, |p| p.d);
    let d = snap_duty(interp(pts, t), lo, hi);
    let mut out = pts.to_vec();
    out.insert(index, Pt { t, d });
    check_side(side, &out)?;
    Ok((out, index))
}

/// A double click: `{points, side, x, width, height, margins?}` or `{points, side, t}`.
fn insert(args: &Value) -> Result<Value, String> {
    let side = arg_side(args)?;
    let pts = arg_points(args, "points")?;
    let t = match f64_at(args, "t") {
        Some(t) => t,
        None => Plot::new(args)?.t(arg_f64(args, "x")?),
    };
    let (out, index) = insert_at(&pts, side, t)?;
    Ok(edit_reply(&out, index))
}

/// The "Add point" button: a point in the middle of the widest gap.
fn insert_middle(args: &Value) -> Result<Value, String> {
    let side = arg_side(args)?;
    let pts = arg_points(args, "points")?;
    let widest = pts
        .windows(2)
        .max_by(|a, b| (a[1].t - a[0].t).total_cmp(&(b[1].t - b[0].t)))
        .ok_or_else(|| "points: at least 2 are needed".to_owned())?;
    let (out, index) = insert_at(&pts, side, ((widest[0].t + widest[1].t) / 2.0).round())
        .map_err(|e| if e.starts_with("Too close") { "There is no room for another point.".to_owned() } else { e })?;
    Ok(edit_reply(&out, index))
}

/// Removes point `index`; refused when two would not be left or when the rest breaks a rule (the
/// point that reaches 100 % in time cannot go).
fn remove(args: &Value) -> Result<Value, String> {
    let side = arg_side(args)?;
    let pts = arg_points(args, "points")?;
    let i = arg_index(args, pts.len())?;
    if pts.len() <= MIN_POINTS {
        return Err("A curve keeps at least 2 points.".to_owned());
    }
    let mut out = pts;
    out.remove(i);
    check_side(side, &out)?;
    let focus = i.min(out.len() - 1);
    Ok(edit_reply(&out, focus))
}

/// The duty a side asks for at `temp`: `{points, temp}`.
fn sample(args: &Value) -> Result<Value, String> {
    let pts = arg_points(args, "points")?;
    Ok(json!(round1(interp(&pts, arg_f64(args, "temp")?))))
}

/// What the edited curve asks for at the temperatures now: `{curve, cpu_temp, gpu_temp,
/// on_battery}` gives `{cpu: {temp, duty}|null, gpu: ...}`. A temperature outside the plot (0 for a
/// sleeping GPU) is unknown. This is the curve's own answer; the live fan also applies the
/// hysteresis, the prediction and the coupling, so it can differ for a moment.
fn preview(args: &Value) -> Result<Value, String> {
    let curve = at(args, "curve").ok_or_else(|| "curve: the curve is needed".to_owned())?;
    let shift = if bool_at(args, "on_battery") == Some(true) {
        f64_at(curve, "battery_shift").unwrap_or(PARAMS[4].4)
    } else {
        0.0
    };
    let side_reply = |side: Side, temp_key: &str| -> Value {
        let pts = parse_points(at(curve, side.name()), side.name()).unwrap_or_default();
        match f64_at(args, temp_key).filter(|t| (TEMP_MIN..=TEMP_MAX).contains(t)) {
            Some(temp) if !pts.is_empty() => {
                json!({"temp": temp, "duty": round1(interp(&pts, temp - shift))})
            }
            _ => Value::Null,
        }
    };
    Ok(json!({"cpu": side_reply(Side::Cpu, "cpu_temp"), "gpu": side_reply(Side::Gpu, "gpu_temp")}))
}

/// Whether two curves differ (points or parameters; a missing parameter is its default).
fn changed(args: &Value) -> bool {
    let (Some(a), Some(b)) = (at(args, "a"), at(args, "b")) else {
        return false;
    };
    for side in [Side::Cpu, Side::Gpu] {
        let pa = parse_points(at(a, side.name()), "a").unwrap_or_default();
        let pb = parse_points(at(b, side.name()), "b").unwrap_or_default();
        if pa.len() != pb.len() || pa.iter().zip(&pb).any(|(x, y)| (x.t - y.t).abs() > 1e-9 || (x.d - y.d).abs() > 1e-9) {
            return true;
        }
    }
    PARAMS.iter().any(|&(key, _, _, _, default, _)| {
        (f64_at(a, key).unwrap_or(default) - f64_at(b, key).unwrap_or(default)).abs() > 1e-9
    })
}

// ---- the dials and the manual request -------------------------------------------------------------

/// The fan mode of a live frame: `smart` when the service runs the curve, else the firmware's
/// `auto`, `max` or `manual`; `unknown` when the frame does not say.
fn fan_mode(frame: &Value) -> &'static str {
    if bool_at(frame, "daemon.settings.smart_fan") == Some(true) {
        return "smart";
    }
    match str_at(frame, "fans.mode") {
        Some("auto") => "auto",
        Some("max") => "max",
        Some("manual") => "manual",
        _ => "unknown",
    }
}

/// The two dials: `{frame}` gives the mode and, per fan, the speed, the duty register and the
/// temperature. A fan at 0 RPM in `auto` or `smart` is the firmware curve at idle, so it is
/// reported as `idle`, a neutral fact; nothing here is ever an alarm. `stopped` means the person
/// stopped the fans (manual, both duties 0).
fn dials(args: &Value) -> Value {
    let frame = at(args, "frame").unwrap_or(&Value::Null);
    let mode = fan_mode(frame);
    let duty = |k: &str| f64_at(frame, &format!("fans.duty_register_pct.{k}"));
    let stopped = mode == "manual" && duty("cpu") == Some(0.0) && duty("gpu") == Some(0.0);
    let one = |fan: &str, temp: &str| {
        let rpm = f64_at(frame, &format!("sensors.{fan}_fan_rpm"));
        json!({
            "rpm": rpm,
            "duty": duty(fan),
            "temp": f64_at(frame, &format!("sensors.{temp}")),
            "idle": rpm == Some(0.0) && (mode == "auto" || mode == "smart"),
        })
    };
    json!({
        "mode": mode,
        "stopped": stopped,
        "curve": str_at(frame, "daemon.live.fan_curve"),
        "rpm_scale": RPM_SCALE,
        "cpu": one("cpu", "cpu_temp"),
        "gpu": one("gpu", "gpu_temp"),
    })
}

/// A manual duty request: `{cpu, gpu}` (percent; 0 stops a fan) gives `{args, stop}`. With
/// `snap` a value from 1 to 19 becomes 20 (a slider's step), without it that is refused: the
/// firmware stalls a fan below 20 %. `stop` tells the page that the confirmation dialog comes
/// first and the command then carries `confirm: true`; the command layer refuses a stop without it.
fn manual(args: &Value) -> Result<Value, String> {
    let refuse = || "A fan duty is 0 (stop the fan) or 20 to 100 %.".to_owned();
    let snap = bool_at(args, "snap") == Some(true);
    let read = |key: &str| -> Result<i64, String> {
        let v = f64_at(args, key).ok_or_else(refuse)?;
        let v = v.round();
        match v {
            0.0 => Ok(0),
            v if (1.0..DUTY_MIN).contains(&v) && snap => Ok(DUTY_MIN as i64),
            v if (DUTY_MIN..=DUTY_MAX).contains(&v) => Ok(v as i64),
            _ => Err(refuse()),
        }
    };
    let (cpu, gpu) = (read("cpu")?, read("gpu")?);
    Ok(json!({"args": {"cpu": cpu, "gpu": gpu}, "stop": cpu == 0 || gpu == 0}))
}

// ---- library and bindings --------------------------------------------------------------------------

/// A curve name as the daemon accepts it: letters, digits, `.`, `_`, `-`, 1 to 40 characters.
fn valid_name(name: &str) -> bool {
    (1..=40).contains(&name.chars().count())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn strings_at(args: &Value, key: &str) -> Vec<String> {
    list_at(args, key)
        .map(|l| l.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

/// The rows of the curve library: `{names, current, favourites}` (the results of `fan curve list`,
/// the name in use and `favourite list`). Built-in curves come first in the order the daemon lists
/// them; a star is a favourite of kind `fan_curve`.
fn library(args: &Value) -> Value {
    let names = strings_at(args, "names");
    let current = str_at(args, "current").unwrap_or("");
    let starred: Vec<&str> = list_at(args, "favourites")
        .unwrap_or_default()
        .iter()
        .filter(|f| matches!(str_at(f, "kind"), Some("fan_curve" | "curve")))
        .filter_map(|f| str_at(f, "name"))
        .collect();
    let rows: Vec<Value> = names
        .iter()
        .map(|n| {
            json!({
                "name": n,
                "builtin": BUILTIN.contains(&n.as_str()),
                "native": n == "native",
                "current": n == current,
                "starred": starred.contains(&n.as_str()),
            })
        })
        .collect();
    json!({"rows": rows})
}

/// A free name for a copy: `{base, existing}` gives `base`, then `base-2`, `base-3` ... (the
/// daemon's own rule for `fan curve copy`).
fn copy_name(args: &Value) -> Value {
    let base = str_at(args, "base").unwrap_or("curve-copy");
    let existing = strings_at(args, "existing");
    let mut name = base.to_owned();
    let mut n = 2;
    while existing.contains(&name) {
        name = format!("{base}-{n}");
        n += 1;
    }
    json!(name)
}

/// The "Fans for [power mode]" rows: `{bindings, curves, power_mode}` (the result of `mode
/// bindings`, `fan curve list` and the power mode now). One row per power mode with the bound
/// curve's name (empty when none; `missing` when the curve is no longer in the library) and the
/// curves the person may choose (every curve but `native`, which is "no curve" already).
fn bindings(args: &Value) -> Value {
    let curves = strings_at(args, "curves");
    let now = str_at(args, "power_mode").or_else(|| str_at(args, "bindings.mode")).unwrap_or("");
    let rows: Vec<Value> = POWER_MODES
        .iter()
        .map(|mode| {
            let bound = str_at(args, &format!("bindings.bindings.{mode}.fan_curve.name")).unwrap_or("");
            json!({
                "mode": mode,
                "curve": bound,
                "missing": !bound.is_empty() && !curves.iter().any(|c| c == bound),
                "current": *mode == now,
            })
        })
        .collect();
    let options: Vec<&String> = curves.iter().filter(|c| *c != "native").collect();
    json!({"rows": rows, "options": options})
}

/// The command a choice in a binding row sends: `{mode, curve}` gives `{verb, args}` (`bind` for a
/// curve, `unbind` of the curve part for the empty choice).
fn bind_request(args: &Value) -> Result<Value, String> {
    let mode = str_at(args, "mode").unwrap_or("");
    if !POWER_MODES.contains(&mode) {
        return Err(format!("mode: one of {}", POWER_MODES.join(", ")));
    }
    match str_at(args, "curve").unwrap_or("") {
        "" => Ok(json!({"verb": "unbind", "args": {"mode": mode, "what": "curve"}})),
        "native" => Err("The native curve is the firmware's own control: choose no curve instead.".to_owned()),
        curve => Ok(json!({"verb": "bind", "args": {"mode": mode, "curve": curve}})),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// The built-in curves of `ts_decision::curves` (copied: that layer is not a dependency).
    fn builtin(name: &str) -> (Vec<Pt>, Vec<Pt>) {
        let p = |v: &[(f64, f64)]| v.iter().map(|&(t, d)| Pt { t, d }).collect::<Vec<_>>();
        match name {
            "native" => (p(&[(52., 0.), (89., 100.)]), p(&[(48., 0.), (78., 100.)])),
            "truesense" => (
                p(&[(52., 0.), (56., 30.), (62., 38.), (68., 48.), (74., 58.), (80., 70.), (85., 85.), (89., 100.)]),
                p(&[(48., 0.), (54., 30.), (60., 38.), (65., 48.), (70., 60.), (74., 78.), (78., 100.)]),
            ),
            "quiet" => (
                p(&[(58., 0.), (66., 30.), (72., 40.), (78., 55.), (84., 75.), (89., 100.)]),
                p(&[(52., 0.), (60., 30.), (66., 40.), (72., 58.), (77., 80.), (80., 100.)]),
            ),
            "performance" => (
                p(&[(45., 30.), (55., 40.), (62., 55.), (70., 70.), (78., 85.), (84., 100.)]),
                p(&[(42., 30.), (52., 40.), (60., 55.), (66., 70.), (72., 85.), (76., 100.)]),
            ),
            _ => (p(&[(40., 40.), (60., 100.)]), p(&[(40., 40.), (58., 100.)])),
        }
    }

    fn curve_json(name: &str) -> Value {
        let (cpu, gpu) = builtin(name);
        json!({"cpu": points_json(&cpu), "gpu": points_json(&gpu)})
    }

    fn pts(v: &[(f64, f64)]) -> Vec<Pt> {
        v.iter().map(|&(t, d)| Pt { t, d }).collect()
    }

    /// A small deterministic generator (xorshift): the property tests need no crate.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }

        /// A float in `[lo, hi)`.
        fn range(&mut self, lo: f64, hi: f64) -> f64 {
            lo + (self.next() >> 11) as f64 / (1u64 << 53) as f64 * (hi - lo)
        }
    }

    const NAMES: [&str; 5] = ["native", "truesense", "quiet", "performance", "max-cooling"];

    fn run(verb: &str, args: Value) -> Value {
        call(verb, &args).unwrap()
    }

    fn reply_points(v: &Value) -> Vec<Pt> {
        parse_points(at(v, "points"), "points").unwrap()
    }

    // -- the validator --

    #[test]
    fn every_builtin_curve_validates_and_the_messages_are_the_daemons() {
        for name in NAMES {
            let r = run("validate", json!({"curve": curve_json(name)}));
            assert_eq!(r, json!({"valid": true, "error": null}), "{name}");
        }
        let bad = |cpu: Value, gpu: Value| {
            let r = run("validate", json!({"curve": {"cpu": cpu, "gpu": gpu}}));
            r["error"].as_str().map(str::to_owned)
        };
        let ok_gpu = json!([[48, 0], [78, 100]]);
        assert_eq!(bad(json!([[52, 0]]), ok_gpu.clone()).unwrap(), "cpu: 2 to 12 points");
        assert_eq!(
            bad(json!([[10, 0], [89, 100]]), ok_gpu.clone()).unwrap(),
            "cpu: temperature 20..110 C, percent 0 (firmware auto) or 20..100"
        );
        assert_eq!(
            bad(json!([[52, 10], [89, 100]]), ok_gpu.clone()).unwrap(),
            "cpu: temperature 20..110 C, percent 0 (firmware auto) or 20..100"
        );
        assert_eq!(
            bad(json!([[52, 50], [60, 30], [89, 100]]), ok_gpu.clone()).unwrap(),
            "cpu: the duty may only rise with temperature"
        );
        assert_eq!(
            bad(json!([[52, 0], [52, 40], [89, 100]]), ok_gpu.clone()).unwrap(),
            "cpu: two points share a temperature"
        );
        assert_eq!(
            bad(json!([[52, 0], [89, 100]]), json!([[48, 0], [90, 100]])).unwrap(),
            "gpu: must reach 100 % by 85 C (the CPU and GPU throttle above that)"
        );
        assert_eq!(
            bad(json!([[52, 0], [96, 100]]), ok_gpu).unwrap(),
            "cpu: must reach 100 % by 95 C (the CPU and GPU throttle above that)"
        );
    }

    #[test]
    fn validation_reports_the_cpu_before_the_gpu_and_checks_the_parameters() {
        let r = run("validate", json!({"curve": {"cpu": [[52, 0]], "gpu": [[48, 0]]}}));
        assert_eq!(r["error"], json!("cpu: 2 to 12 points"));
        let mut c = curve_json("truesense");
        c["couple"] = json!(60);
        assert_eq!(run("validate", json!({"curve": c}))["error"], json!("couple: 0..50"));
        let mut c = curve_json("truesense");
        c["hysteresis"] = json!("4");
        assert_eq!(run("validate", json!({"curve": c}))["error"], json!("hysteresis: 0..15"));
        let mut c = curve_json("truesense");
        c["down_rate"] = json!(0.1);
        assert_eq!(run("validate", json!({"curve": c}))["error"], json!("down_rate: 0.2..20"));
    }

    #[test]
    fn hostile_curves_are_refused_with_text_and_never_panic() {
        for curve in [
            json!(null),
            json!(7),
            json!("x"),
            json!([]),
            json!({}),
            json!({"cpu": "x", "gpu": []}),
            json!({"cpu": [[1], [2, 3]], "gpu": [[1, 2], [3, 4]]}),
            json!({"cpu": [["a", "b"], [1, 2]], "gpu": [[1, 2], [3, 4]]}),
            json!({"cpu": [[1e308, 1e308], [2, 3]], "gpu": [[1, 2], [3, 4]]}),
            json!({"cpu": (0..40).map(|i| json!([20 + i, 30])).collect::<Vec<_>>(), "gpu": []}),
        ] {
            let r = run("validate", json!({"curve": curve}));
            assert_eq!(r["valid"], json!(false));
            assert!(r["error"].as_str().is_some_and(|e| !e.is_empty()));
        }
        assert_eq!(run("validate", json!({}))["valid"], json!(false));
    }

    // -- geometry --

    fn geo_args(points: Value) -> Value {
        json!({"points": points, "side": "cpu", "width": 500, "height": 300, "margins": [40, 10, 10, 30]})
    }

    #[test]
    fn the_mapping_is_linear_and_both_directions_agree() {
        let plot = Plot::new(&json!({"width": 500, "height": 300, "margins": [40, 10, 10, 30]})).unwrap();
        assert_eq!((plot.left, plot.right, plot.top, plot.bottom), (40.0, 490.0, 10.0, 270.0));
        assert_eq!(plot.x(20.0), 40.0);
        assert_eq!(plot.x(110.0), 490.0);
        assert_eq!(plot.y(100.0), 10.0);
        assert_eq!(plot.y(0.0), 270.0);
        for t in [20.0, 37.5, 64.0, 110.0] {
            assert!((plot.t(plot.x(t)) - t).abs() < 1e-9);
        }
        for d in [0.0, 20.0, 55.5, 100.0] {
            assert!((plot.d(plot.y(d)) - d).abs() < 1e-9);
        }
    }

    #[test]
    fn a_degenerate_box_still_maps_without_dividing_by_zero() {
        let plot = Plot::new(&json!({"width": 10, "height": 10, "margins": [400, 400, 400, 400]})).unwrap();
        assert!(plot.w() >= 1.0 && plot.h() >= 1.0);
        assert!(plot.t(plot.x(50.0)).is_finite());
        let plot = Plot::new(&json!({"width": -5, "height": 0})).unwrap();
        assert!(plot.w() >= 1.0 && plot.h() >= 1.0);
        assert!(call("geometry", &json!({"points": [[52, 0], [89, 100]]})).is_err(), "no size");
    }

    #[test]
    fn geometry_gives_lines_ticks_the_limit_and_the_now_marker() {
        let mut args = geo_args(json!([[52, 0], [89, 100]]));
        args["other"] = json!([[48, 0], [78, 100]]);
        args["temp"] = json!(70.5);
        let g = run("geometry", args);
        let line = g["line"].as_array().unwrap();
        assert_eq!(line.len(), 4, "flat to both edges of the plot");
        assert_eq!(line[0], json!([40.0, 270.0]));
        assert_eq!(line[3], json!([490.0, 10.0]));
        assert_eq!(g["other_line"].as_array().unwrap().len(), 4);
        assert_eq!(g["dots"].as_array().unwrap().len(), 2);
        assert_eq!(g["x_ticks"].as_array().unwrap().len(), 10);
        assert_eq!(g["y_ticks"].as_array().unwrap().len(), 6);
        assert_eq!(g["limit"]["temp"], json!(95.0));
        let now = &g["now"];
        assert_eq!(now["temp"], json!(70.5));
        let expected = interp(&pts(&[(52., 0.), (89., 100.)]), 70.5);
        assert!((now["duty"].as_f64().unwrap() - expected).abs() < 0.06);
        // the Auto band is the 0..20 strip
        assert!(g["auto_band"]["top"].as_f64().unwrap() < g["auto_band"]["bottom"].as_f64().unwrap());
        // no temperature (or one off the plot): no marker
        let g = run("geometry", geo_args(json!([[52, 0], [89, 100]])));
        assert!(g["now"].is_null());
        let mut args = geo_args(json!([[52, 0], [89, 100]]));
        args["temp"] = json!(0);
        assert!(run("geometry", args)["now"].is_null());
        let mut args = geo_args(json!([[52, 0], [89, 100]]));
        args["side"] = json!("gpu");
        assert_eq!(run("geometry", args)["limit"]["temp"], json!(85.0));
    }

    #[test]
    fn hit_testing_picks_the_nearest_point_inside_the_radius() {
        let plot = Plot::new(&geo_args(json!([]))).unwrap();
        let points = json!([[52, 0], [56, 30], [89, 100]]);
        let at_point = |i: usize| {
            let p = pts(&[(52., 0.), (56., 30.), (89., 100.)])[i];
            (plot.x(p.t), plot.y(p.d))
        };
        for i in 0..3 {
            let (x, y) = at_point(i);
            let mut a = geo_args(points.clone());
            a["x"] = json!(x + 3.0);
            a["y"] = json!(y - 2.0);
            assert_eq!(run("hit", a), json!(i));
        }
        let mut a = geo_args(points.clone());
        a["x"] = json!(at_point(2).0 - 60.0);
        a["y"] = json!(at_point(2).1);
        assert!(run("hit", a).is_null(), "empty space");
        // two dots inside the radius: the nearer one wins
        let (x0, y0) = at_point(0);
        let (x1, y1) = at_point(1);
        let mut a = geo_args(points);
        a["x"] = json!(x1 + 1.0);
        a["y"] = json!(y1);
        a["radius"] = json!(((x1 - x0).hypot(y1 - y0)) * 2.0);
        assert_eq!(run("hit", a), json!(1));
    }

    // -- drag --

    fn plot_for(args: &Value) -> Plot {
        Plot::new(args).unwrap()
    }

    fn drag_to(points: &[Pt], side: &str, i: usize, t: f64, d: f64) -> Vec<Pt> {
        let base = json!({"width": 560, "height": 300, "margins": [46, 14, 12, 30]});
        let plot = plot_for(&base);
        let mut a = base;
        a["points"] = points_json(points);
        a["side"] = json!(side);
        a["index"] = json!(i);
        a["x"] = json!(plot.x(t));
        a["y"] = json!(plot.y(d));
        reply_points(&run("drag", a))
    }

    #[test]
    fn a_drag_snaps_to_whole_degrees_and_to_the_duty_grid() {
        let (cpu, _) = builtin("truesense");
        let moved = drag_to(&cpu, "cpu", 3, 69.4, 52.0);
        assert_eq!(moved[3], Pt { t: 69.0, d: 50.0 });
        assert_eq!(moved.len(), cpu.len());
        assert_eq!(moved[..3], cpu[..3]);
        assert_eq!(moved[4..], cpu[4..]);
    }

    #[test]
    fn a_duty_between_0_and_20_goes_to_the_nearer_of_the_two() {
        let (cpu, _) = builtin("truesense");
        assert_eq!(drag_to(&cpu, "cpu", 0, 52.0, 3.0)[0].d, 0.0);
        assert_eq!(drag_to(&cpu, "cpu", 0, 52.0, 17.0)[0].d, 20.0);
    }

    #[test]
    fn a_point_never_crosses_its_neighbours() {
        let (cpu, _) = builtin("truesense");
        // far to the right and left of the plot, and far above and below it
        let moved = drag_to(&cpu, "cpu", 3, 200.0, 50.0);
        assert_eq!(moved[3].t, 73.0, "one degree short of the next point at 74");
        let moved = drag_to(&cpu, "cpu", 3, -50.0, 50.0);
        assert_eq!(moved[3].t, 63.0, "one degree past the previous point at 62");
        let moved = drag_to(&cpu, "cpu", 3, 68.0, 900.0);
        assert_eq!(moved[3].d, 55.0, "the next point's duty (58) is the ceiling; 55 is the grid step below it");
    }

    #[test]
    fn the_duty_stays_between_the_neighbours_duties() {
        let (cpu, _) = builtin("truesense");
        let up = drag_to(&cpu, "cpu", 3, 68.0, 100.0);
        assert!(up[3].d <= cpu[4].d);
        let down = drag_to(&cpu, "cpu", 3, 68.0, 0.0);
        assert!(down[3].d >= cpu[2].d);
    }

    #[test]
    fn the_last_point_cannot_be_dragged_past_the_limit_without_another_reaching_full() {
        // GPU: only the last point is at 100 %, at 78; moving it above 85 or lowering it breaks the rule.
        let gpu = pts(&[(48., 0.), (54., 30.), (78., 100.)]);
        let moved = drag_to(&gpu, "gpu", 2, 100.0, 100.0);
        assert!(moved[2].t <= 85.0, "{moved:?}");
        assert_eq!(moved[2].d, 100.0);
        let moved = drag_to(&gpu, "gpu", 2, 78.0, 40.0);
        assert!(check_side(Side::Gpu, &moved).is_ok());
        assert_eq!(moved[2].d, 100.0, "it is the only point at 100 %, so it holds it");
        // with a second point at 100 % the first one is free to drop
        let gpu = pts(&[(48., 0.), (54., 30.), (70., 100.), (78., 100.)]);
        let moved = drag_to(&gpu, "gpu", 3, 78.0, 40.0);
        assert!(check_side(Side::Gpu, &moved).is_ok());
    }

    #[test]
    fn dragging_the_first_100_percent_point_right_stops_at_the_limit() {
        let cpu = pts(&[(50., 0.), (60., 40.), (89., 100.)]);
        let moved = drag_to(&cpu, "cpu", 2, 110.0, 100.0);
        assert_eq!(moved[2], Pt { t: 95.0, d: 100.0 });
    }

    #[test]
    fn a_curve_that_was_invalid_still_edits_without_panicking() {
        // The CPU curve never reaches 100: no position can fix it by moving one point, so the
        // solver falls back to the neighbour rules and the page shows the validator's message.
        let cpu = pts(&[(50., 0.), (60., 40.), (89., 80.)]);
        let moved = drag_to(&cpu, "cpu", 1, 70.0, 50.0);
        assert_eq!(moved[1], Pt { t: 70.0, d: 50.0 });
        assert!(check_side(Side::Cpu, &moved).is_err());
    }

    #[test]
    fn a_point_off_the_grid_stays_when_the_pointer_stays() {
        let cpu = pts(&[(50.5, 22.0), (89., 100.)]);
        let moved = drag_to(&cpu, "cpu", 0, 50.5, 22.0);
        assert_eq!(moved[0], Pt { t: 50.5, d: 22.0 });
    }

    #[test]
    fn bad_edit_arguments_are_errors_not_panics() {
        let base = |extra: Value| {
            let mut a = json!({"width": 500, "height": 300, "points": [[52, 0], [89, 100]], "side": "cpu", "index": 0, "x": 100, "y": 100});
            for (k, v) in extra.as_object().unwrap() {
                a[k.as_str()] = v.clone();
            }
            a
        };
        assert!(call("drag", &base(json!({"index": 2}))).is_err());
        assert!(call("drag", &base(json!({"index": -1}))).is_err());
        assert!(call("drag", &base(json!({"side": "fan"}))).unwrap_err().contains("side"));
        assert!(call("drag", &base(json!({"points": [[89, 100], [52, 0]]}))).is_err(), "unsorted");
        assert!(call("drag", &base(json!({"points": "x"}))).is_err());
        assert!(call("drag", &base(json!({"x": null}))).is_err());
        assert!(call("drag", &base(json!({"points": (0..70).map(|i| json!([20, i])).collect::<Vec<_>>()}))).is_err());
        assert!(call("nudge", &base(json!({"index": 5}))).is_err());
        assert!(call("insert", &base(json!({"x": null}))).is_err());
        assert!(call("remove", &base(json!({"index": 9}))).is_err());
        assert!(call("nope", &json!({})).unwrap_err().contains("nope"));
    }

    // -- nudge, insert, remove --

    #[test]
    fn the_keyboard_moves_one_degree_or_one_duty_step() {
        let (cpu, _) = builtin("truesense");
        let nudged = |i: usize, dt: i64, dd: i64| {
            reply_points(&run("nudge", json!({"points": points_json(&cpu), "side": "cpu", "index": i, "dt": dt, "dd": dd})))
        };
        assert_eq!(nudged(3, 1, 0)[3], Pt { t: 69.0, d: 48.0 });
        assert_eq!(nudged(3, -1, 0)[3], Pt { t: 67.0, d: 48.0 });
        assert_eq!(nudged(3, 0, 1)[3], Pt { t: 68.0, d: 50.0 });
        assert_eq!(nudged(3, 0, -1)[3], Pt { t: 68.0, d: 45.0 });
        // 0 and 20 are one step apart
        assert_eq!(nudged(0, 0, 1)[0].d, 20.0);
        let low = pts(&[(50., 20.), (60., 40.), (89., 100.)]);
        let r = run("nudge", json!({"points": points_json(&low), "side": "cpu", "index": 0, "dt": 0, "dd": -1}));
        assert_eq!(reply_points(&r)[0].d, 0.0);
        // a step into a neighbour is held back
        let r = run("nudge", json!({"points": points_json(&cpu), "side": "cpu", "index": 7, "dt": 1, "dd": 1}));
        assert_eq!(reply_points(&r)[7], Pt { t: 90.0, d: 100.0 });
    }

    #[test]
    fn insert_puts_a_point_on_the_line_so_the_shape_is_unchanged() {
        let (cpu, _) = builtin("truesense");
        let r = run("insert", json!({"points": points_json(&cpu), "side": "cpu", "t": 65}));
        let out = reply_points(&r);
        assert_eq!(out.len(), cpu.len() + 1);
        assert_eq!(r["index"], json!(3));
        assert_eq!(out[3].t, 65.0);
        assert!((out[3].d - interp(&cpu, 65.0)).abs() <= 2.5, "{out:?}");
        assert!(check_side(Side::Cpu, &out).is_ok());
        // from a pixel
        let base = json!({"width": 560, "height": 300, "margins": [46, 14, 12, 30]});
        let plot = plot_for(&base);
        let mut a = base;
        a["points"] = points_json(&cpu);
        a["side"] = json!("cpu");
        a["x"] = json!(plot.x(65.2));
        assert_eq!(reply_points(&run("insert", a))[3].t, 65.0);
    }

    #[test]
    fn insert_refuses_a_crowded_spot_and_a_full_curve() {
        let (cpu, _) = builtin("truesense");
        let e = call("insert", &json!({"points": points_json(&cpu), "side": "cpu", "t": 63})).unwrap_err();
        assert!(e.starts_with("Too close"), "{e}");
        let duties = [0.0, 0.0, 0.0, 0.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0];
        let full: Vec<Pt> = duties.iter().enumerate().map(|(i, d)| Pt { t: 40.0 + 5.0 * i as f64, d: *d }).collect();
        assert!(check_side(Side::Cpu, &full).is_ok());
        let e = call("insert", &json!({"points": points_json(&full), "side": "cpu", "t": 43})).unwrap_err();
        assert_eq!(e, "A curve has at most 12 points.");
    }

    #[test]
    fn insert_middle_uses_the_widest_gap_and_says_when_there_is_no_room() {
        let (cpu, _) = builtin("native");
        let r = run("insert_middle", json!({"points": points_json(&cpu), "side": "cpu"}));
        let out = reply_points(&r);
        assert_eq!(out.len(), 3);
        assert_eq!(out[1].t, 71.0);
        assert_eq!(r["index"], json!(1));
        let tight = pts(&[(50., 0.), (52., 30.), (89., 100.)]);
        let r = run("insert_middle", json!({"points": points_json(&tight), "side": "cpu"}));
        assert_eq!(reply_points(&r)[2].t, 71.0, "the 52..89 gap is the widest");
        let crowded = pts(&[(50., 0.), (52., 100.)]);
        let e = call("insert_middle", &json!({"points": points_json(&crowded), "side": "cpu"})).unwrap_err();
        assert_eq!(e, "There is no room for another point.");
    }

    #[test]
    fn remove_keeps_two_points_and_the_100_percent_rule() {
        let (cpu, _) = builtin("truesense");
        let r = run("remove", json!({"points": points_json(&cpu), "side": "cpu", "index": 2}));
        assert_eq!(reply_points(&r).len(), cpu.len() - 1);
        let e = call("remove", &json!({"points": points_json(&builtin("native").0), "side": "cpu", "index": 0})).unwrap_err();
        assert_eq!(e, "A curve keeps at least 2 points.");
        // removing the point that reaches 100 % by the limit would leave the GPU too cool
        let gpu = pts(&[(50., 30.), (80., 100.), (95., 100.)]);
        let e = call("remove", &json!({"points": points_json(&gpu), "side": "gpu", "index": 1})).unwrap_err();
        assert!(e.contains("must reach 100 % by 85 C"), "{e}");
        // the same removal is fine on the CPU side (limit 95)
        assert!(call("remove", &json!({"points": points_json(&gpu), "side": "cpu", "index": 1})).is_ok());
        // the focus moves to a neighbour that exists
        let r = run("remove", json!({"points": points_json(&cpu), "side": "cpu", "index": 6}));
        assert_eq!(r["index"], json!(6));
        // the last point (the one at 100 %) cannot go
        assert!(call("remove", &json!({"points": points_json(&cpu), "side": "cpu", "index": 7})).is_err());
    }

    // -- property tests: any sequence of edits yields a valid curve --

    fn random_edit(rng: &mut Rng, points: &[Pt], side: Side) -> Vec<Pt> {
        let sides = side.name();
        let args = |extra: Value| {
            let mut a = json!({"points": points_json(points), "side": sides, "width": 560, "height": 300, "margins": [46, 14, 12, 30]});
            for (k, v) in extra.as_object().unwrap() {
                a[k.as_str()] = v.clone();
            }
            a
        };
        let i = rng.below(points.len());
        let outcome = match rng.below(6) {
            0..=2 => {
                // a pointer drag, sometimes far outside the widget
                let (x, y) = (rng.range(-300.0, 900.0), rng.range(-300.0, 600.0));
                call("drag", &args(json!({"index": i, "x": x, "y": y})))
            }
            3 => call(
                "nudge",
                &args(json!({"index": i, "dt": rng.below(3) as i64 - 1, "dd": rng.below(3) as i64 - 1})),
            ),
            4 => call("insert", &args(json!({"t": rng.range(0.0, 130.0)}))),
            _ => call("remove", &args(json!({"index": i}))),
        };
        match outcome {
            Ok(v) => reply_points(&v),
            // a refused edit leaves the curve as it was
            Err(_) => points.to_vec(),
        }
    }

    #[test]
    fn any_edit_sequence_from_a_valid_curve_yields_a_valid_curve() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for name in NAMES {
            for side in [Side::Cpu, Side::Gpu] {
                for _round in 0..6 {
                    let (cpu, gpu) = builtin(name);
                    let mut points = if side == Side::Cpu { cpu } else { gpu };
                    assert!(check_side(side, &points).is_ok());
                    for step in 0..200 {
                        points = random_edit(&mut rng, &points, side);
                        if let Err(e) = check_side(side, &points) {
                            panic!("{name} {side:?} step {step}: {e}: {points:?}");
                        }
                        assert!(points.windows(2).all(|w| w[0].t < w[1].t), "sorted and distinct: {points:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn edits_from_the_minimal_valid_curves_stay_valid() {
        let mut rng = Rng(42);
        for (side, start) in [
            (Side::Cpu, pts(&[(20., 100.), (110., 100.)])),
            (Side::Cpu, pts(&[(95., 100.), (110., 100.)])),
            (Side::Gpu, pts(&[(85., 100.), (86., 100.)])),
            (Side::Gpu, pts(&[(20., 0.), (85., 100.)])),
        ] {
            let mut points = start;
            for _ in 0..400 {
                points = random_edit(&mut rng, &points, side);
                assert!(check_side(side, &points).is_ok(), "{side:?} {points:?}");
            }
        }
    }

    #[test]
    fn a_drag_is_idempotent_and_never_moves_far_from_the_pointer_when_free() {
        let (cpu, _) = builtin("truesense");
        let once = drag_to(&cpu, "cpu", 3, 66.0, 45.0);
        let twice = drag_to(&once, "cpu", 3, 66.0, 45.0);
        assert_eq!(once, twice);
        assert_eq!(once[3], Pt { t: 66.0, d: 45.0 });
    }

    // -- preview, changed --

    #[test]
    fn the_preview_is_the_curves_answer_at_the_live_temperatures() {
        let c = curve_json("truesense");
        let r = run("preview", json!({"curve": c, "cpu_temp": 62, "gpu_temp": 60, "on_battery": false}));
        assert_eq!(r["cpu"]["duty"], json!(38.0));
        assert_eq!(r["gpu"]["duty"], json!(38.0));
        assert_eq!(r["cpu"]["temp"], json!(62.0));
        // battery_shift cools the lookup by 3 C by default: 65 C reads as 62
        let r = run("preview", json!({"curve": c, "cpu_temp": 65, "gpu_temp": 63, "on_battery": true}));
        assert_eq!(r["cpu"]["duty"], json!(38.0));
        let mut shifted = c.clone();
        shifted["battery_shift"] = json!(6);
        let r = run("preview", json!({"curve": shifted, "cpu_temp": 68, "gpu_temp": null, "on_battery": true}));
        assert_eq!(r["cpu"]["duty"], json!(38.0));
        assert!(r["gpu"].is_null(), "an unknown GPU temperature is not a preview");
        let r = run("preview", json!({"curve": c, "cpu_temp": 5, "gpu_temp": 0}));
        assert!(r["cpu"].is_null() && r["gpu"].is_null(), "outside the plot");
        assert!(call("preview", &json!({})).is_err());
    }

    #[test]
    fn changed_compares_points_and_parameters_with_defaults_filled_in() {
        let a = curve_json("truesense");
        assert!(!changed(&json!({"a": a, "b": a})));
        let mut b = a.clone();
        b["cpu"][1][1] = json!(35);
        assert!(changed(&json!({"a": a, "b": b})));
        let mut b = a.clone();
        b["predict"] = json!(5);
        assert!(!changed(&json!({"a": a, "b": b})), "5 is the default");
        b["predict"] = json!(6);
        assert!(changed(&json!({"a": a, "b": b})));
        let mut b = a.clone();
        b["gpu"].as_array_mut().unwrap().pop();
        assert!(changed(&json!({"a": a, "b": b})));
        assert!(!changed(&json!({"a": a})));
    }

    // -- the dials --

    fn frame(mode: &str, smart: bool, cpu_rpm: Option<i64>, duty: (i64, i64)) -> Value {
        json!({
            "sensors": {"cpu_temp": 41, "gpu_temp": 38, "cpu_fan_rpm": cpu_rpm, "gpu_fan_rpm": 0},
            "fans": {"mode": mode, "duty_register_pct": {"cpu": duty.0, "gpu": duty.1}},
            "daemon": {"settings": {"smart_fan": smart}, "live": {"fan_curve": "quiet"}},
        })
    }

    #[test]
    fn zero_rpm_in_auto_or_smart_is_idle_never_a_fault() {
        let d = run("dials", json!({"frame": frame("auto", false, Some(0), (40, 40))}));
        assert_eq!(d["mode"], json!("auto"));
        assert_eq!(d["cpu"]["idle"], json!(true));
        assert_eq!(d["gpu"]["idle"], json!(true));
        assert_eq!(d["cpu"]["duty"], json!(40.0));
        let d = run("dials", json!({"frame": frame("auto", true, Some(0), (40, 40))}));
        assert_eq!(d["mode"], json!("smart"));
        assert_eq!(d["cpu"]["idle"], json!(true));
        assert_eq!(d["curve"], json!("quiet"));
        let d = run("dials", json!({"frame": frame("auto", false, Some(2150), (40, 40))}));
        assert_eq!(d["cpu"]["idle"], json!(false));
        let d = run("dials", json!({"frame": frame("max", false, Some(0), (100, 100))}));
        assert_eq!(d["cpu"]["idle"], json!(false), "a fan at 0 RPM in Max is not the firmware curve");
        // no key of the answer is an alarm
        let text = d.to_string();
        assert!(!text.contains("alarm") && !text.contains("fault") && !text.contains("warn"));
    }

    #[test]
    fn the_dials_survive_a_frame_with_holes() {
        let d = run("dials", json!({"frame": null}));
        assert_eq!(d["mode"], json!("unknown"));
        assert!(d["cpu"]["rpm"].is_null());
        assert_eq!(d["cpu"]["idle"], json!(false));
        let d = run("dials", json!({"frame": {"fans": {"mode": "wat"}}}));
        assert_eq!(d["mode"], json!("unknown"));
        let d = run("dials", json!({}));
        assert_eq!(d["stopped"], json!(false));
    }

    #[test]
    fn stopped_means_manual_with_both_duties_at_zero() {
        let d = run("dials", json!({"frame": frame("manual", false, Some(1700), (0, 0))}));
        assert_eq!(d["stopped"], json!(true));
        let d = run("dials", json!({"frame": frame("manual", false, Some(1700), (0, 30))}));
        assert_eq!(d["stopped"], json!(false));
        let d = run("dials", json!({"frame": frame("auto", false, Some(0), (0, 0))}));
        assert_eq!(d["stopped"], json!(false), "auto with idle registers is not a stop");
    }

    // -- the manual request --

    #[test]
    fn a_manual_duty_is_zero_or_20_to_100_and_a_zero_needs_the_confirmation_flow() {
        let m = |cpu: Value, gpu: Value, snap: bool| call("manual", &json!({"cpu": cpu, "gpu": gpu, "snap": snap}));
        assert_eq!(m(json!(40), json!(60), false).unwrap(), json!({"args": {"cpu": 40, "gpu": 60}, "stop": false}));
        assert_eq!(m(json!(20), json!(100), false).unwrap()["stop"], json!(false));
        assert_eq!(m(json!(0), json!(60), false).unwrap()["stop"], json!(true));
        assert_eq!(m(json!(60), json!(0), false).unwrap()["stop"], json!(true));
        assert_eq!(m(json!(0), json!(0), false).unwrap()["args"], json!({"cpu": 0, "gpu": 0}));
        for bad in [json!(10), json!(19), json!(101), json!(-5), json!(null), json!("40")] {
            assert_eq!(m(bad.clone(), json!(40), false).unwrap_err(), "A fan duty is 0 (stop the fan) or 20 to 100 %.", "{bad}");
        }
        // a slider position between 1 and 19 reads as 20, never as a stop
        assert_eq!(m(json!(5), json!(15), true).unwrap(), json!({"args": {"cpu": 20, "gpu": 20}, "stop": false}));
        assert!(m(json!(-1), json!(40), true).is_err());
        assert!(m(json!(101), json!(40), true).is_err());
        assert_eq!(m(json!(39.6), json!(40), false).unwrap()["args"]["cpu"], json!(40));
        assert!(call("manual", &json!({"cpu": 40})).is_err(), "both duties are needed");
    }

    // -- library and bindings --

    #[test]
    fn the_library_marks_builtin_native_current_and_starred_rows() {
        let r = run(
            "library",
            json!({
                "names": ["native", "truesense", "quiet", "mine", "other"],
                "current": "mine",
                "favourites": [
                    {"position": 1, "kind": "fan_curve", "name": "mine", "exists": true},
                    {"position": 2, "kind": "look", "name": "quiet", "exists": true},
                    {"position": 3, "kind": "curve", "name": "other", "exists": true},
                ],
            }),
        );
        let rows = r["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0]["native"], json!(true));
        assert_eq!(rows[0]["builtin"], json!(true));
        assert_eq!(rows[2]["starred"], json!(false), "a starred look called quiet is not a starred curve");
        assert_eq!(rows[3], json!({"name": "mine", "builtin": false, "native": false, "current": true, "starred": true}));
        assert_eq!(rows[4]["starred"], json!(true), "kind curve is accepted like fan_curve");
        assert_eq!(run("library", json!({}))["rows"], json!([]));
    }

    #[test]
    fn copy_names_count_up_like_the_daemons_rule() {
        let n = |base: &str, existing: Value| run("copy_name", json!({"base": base, "existing": existing}));
        assert_eq!(n("quiet-copy", json!(["quiet"])), json!("quiet-copy"));
        assert_eq!(n("quiet-copy", json!(["quiet-copy"])), json!("quiet-copy-2"));
        assert_eq!(n("quiet-copy", json!(["quiet-copy", "quiet-copy-2"])), json!("quiet-copy-3"));
    }

    #[test]
    fn names_follow_the_daemons_rule() {
        let v = |s: &str| run("valid_name", json!({"name": s}));
        assert_eq!(v("my-curve_1.2"), json!(true));
        assert_eq!(v(&"a".repeat(40)), json!(true));
        for bad in ["", &"a".repeat(41), "has space", "slash/name", "ünï", "a:b"] {
            assert_eq!(v(bad), json!(false), "{bad}");
        }
    }

    #[test]
    fn bindings_rows_list_every_power_mode_in_order() {
        let r = run(
            "bindings",
            json!({
                "bindings": {"mode": "turbo", "bindings": {
                    "eco": {"look": null, "fan_curve": {"name": "quiet", "data": {}}},
                    "turbo": {"look": {"name": "x", "data": {}}, "fan_curve": {"name": "gone", "data": {}}},
                }},
                "curves": ["native", "truesense", "quiet"],
            }),
        );
        let rows = r["rows"].as_array().unwrap();
        let modes: Vec<&str> = rows.iter().map(|r| r["mode"].as_str().unwrap()).collect();
        assert_eq!(modes, ["eco", "quiet", "balanced", "performance", "turbo"]);
        assert_eq!(rows[0]["curve"], json!("quiet"));
        assert_eq!(rows[0]["missing"], json!(false));
        assert_eq!(rows[1]["curve"], json!(""));
        assert_eq!(rows[4]["curve"], json!("gone"));
        assert_eq!(rows[4]["missing"], json!(true));
        assert_eq!(rows[4]["current"], json!(true));
        assert_eq!(rows[0]["current"], json!(false));
        assert_eq!(r["options"], json!(["truesense", "quiet"]), "native is not an option");
        let r = run("bindings", json!({}));
        assert_eq!(r["rows"].as_array().unwrap().len(), 5);
        // an explicit power mode wins over the one in the answer
        let r = run("bindings", json!({"power_mode": "eco", "bindings": {"mode": "turbo"}}));
        assert_eq!(r["rows"][0]["current"], json!(true));
    }

    #[test]
    fn a_binding_choice_becomes_bind_or_unbind_and_never_binds_native() {
        let b = |mode: &str, curve: &str| call("bind_request", &json!({"mode": mode, "curve": curve}));
        assert_eq!(b("eco", "quiet").unwrap(), json!({"verb": "bind", "args": {"mode": "eco", "curve": "quiet"}}));
        assert_eq!(b("turbo", "").unwrap(), json!({"verb": "unbind", "args": {"mode": "turbo", "what": "curve"}}));
        assert!(b("turbo", "native").unwrap_err().contains("native"));
        assert!(b("warp", "quiet").is_err());
        assert!(call("bind_request", &json!({})).is_err());
    }

    #[test]
    fn limits_describe_every_range_the_editor_uses() {
        let l = run("limits", json!({}));
        assert_eq!(l["max_points"], json!(12));
        assert_eq!(l["cpu_full_by"], json!(95.0));
        assert_eq!(l["gpu_full_by"], json!(85.0));
        for key in ["hysteresis", "down_rate", "predict", "couple", "battery_shift"] {
            let p = &l["params"][key];
            assert!(p["min"].as_f64().unwrap() < p["max"].as_f64().unwrap(), "{key}");
            let d = p["default"].as_f64().unwrap();
            assert!(p["min"].as_f64().unwrap() <= d && d <= p["max"].as_f64().unwrap(), "{key}");
        }
    }

    #[test]
    fn the_duty_grid_is_zero_then_20_to_100_in_fives() {
        let g: Vec<f64> = duty_grid().collect();
        assert_eq!(g.len(), 18);
        assert_eq!(g[0], 0.0);
        assert_eq!(g[1], 20.0);
        assert_eq!(*g.last().unwrap(), 100.0);
        assert_eq!(step_duty(0.0, true), 20.0);
        assert_eq!(step_duty(20.0, false), 0.0);
        assert_eq!(step_duty(22.0, true), 25.0);
        assert_eq!(step_duty(22.0, false), 20.0);
        assert_eq!(step_duty(100.0, true), 100.0);
        assert_eq!(step_duty(0.0, false), 0.0);
    }
}
