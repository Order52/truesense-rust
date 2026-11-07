//! Fan curves for the smart fan: temperature (degC) to fan duty (%), separately for the CPU
//! fan and the GPU fan, and the [`Follower`] that turns temperatures into duties.
//!
//! A curve is `{"cpu": [[temp, pct], ...], "gpu": [...], "hysteresis": degC, "down_rate":
//! %/s, "predict": degC, "couple": %, "battery_shift": degC}`. Between points the duty is
//! interpolated; 0 % means "let the firmware run the fans" (the firmware is the only thing
//! that can reach 0 rpm, so they stop when cool). The duty may only rise with temperature.
//! Built-in curves are in this module; the user's own live in
//! `~/.config/truesense/curves/NAME.json` (see [`Store`]).
//!
//! Everything here is a pure function or a state machine over values passed in: the only
//! I/O is [`Store`], whose directory is a field so tests and other users can point it
//! elsewhere. The JSON shape is the Python edition's (it is what the daemon sends on the
//! control socket and what the user's files contain), so whole numbers serialise as
//! integers (`[56,30]`, not `[56.0,30.0]`).

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Serialize, Serializer};
use serde_json::Value;
use ts_foundation::fsx::{self, FsError, MAX_SETTINGS_FILE};
// A curve name becomes a file name: the shared rule keeps everything that could climb out of the
// directory out.
use ts_foundation::names::is_valid as valid_name;

/// The fewest points a side may have.
pub const MIN_POINTS: usize = 2;
/// The most points a side may have.
pub const MAX_POINTS: usize = 12;
/// The lowest temperature a point may sit at.
pub(crate) const MIN_TEMP_C: f64 = 20.0;
/// The highest temperature a point may sit at.
pub(crate) const MAX_TEMP_C: f64 = 110.0;
/// The lowest manual duty a fan can hold: below 20 % the firmware stalls the fan, so a point
/// is either 0 (hand over to the firmware) or 20..=100.
pub(crate) const MIN_DUTY_PCT: f64 = 20.0;
/// The highest duty.
pub(crate) const MAX_DUTY_PCT: f64 = 100.0;
/// The CPU curve must be at 100 % by this temperature: the CPU throttles above it, so a curve
/// that is cooler than that for the load could let the laptop overheat.
pub(crate) const CPU_FULL_BY_C: f64 = 95.0;
/// The GPU curve must be at 100 % by this temperature (the GPU throttles above it).
pub(crate) const GPU_FULL_BY_C: f64 = 85.0;

/// Default `hysteresis` in degC.
pub(crate) const DEFAULT_HYSTERESIS: f64 = 4.0;
/// Default `down_rate` in percent per second.
pub(crate) const DEFAULT_DOWN_RATE: f64 = 1.5;
/// Default `predict` in degC.
pub(crate) const DEFAULT_PREDICT: f64 = 5.0;
/// Default `couple` in percent.
pub(crate) const DEFAULT_COUPLE: f64 = 15.0;
/// Default `battery_shift` in degC.
pub(crate) const DEFAULT_BATTERY_SHIFT: f64 = 3.0;

/// The five follower parameters with their allowed ranges, in the order Python checks them.
const PARAM_RANGES: [(&str, f64, f64); 5] = [
    ("hysteresis", 0.0, 15.0),
    ("down_rate", 0.2, 20.0),
    ("predict", 0.0, 15.0),
    ("couple", 0.0, 50.0),
    ("battery_shift", 0.0, 15.0),
];

/// Why a curve, a point list or a curve file was refused. `Display` is the exact sentence the
/// Python edition raised, because clients show it to the user.
#[derive(Debug)]
pub enum CurveError {
    /// The curve is not a JSON object.
    NotAnObject,
    /// A side has fewer than 2 or more than 12 points (or is not a list).
    PointCount(Side),
    /// A point is not a `[temperature, percent]` pair of numbers.
    PointShape(Side),
    /// A temperature is outside 20..=110 or a duty is neither 0 nor 20..=100.
    PointRange(Side),
    /// The duty falls somewhere as the temperature rises.
    NotMonotonic(Side),
    /// Two points share a temperature.
    DuplicateTemperature(Side),
    /// The side is not at 100 % by the temperature where its chip starts to throttle.
    NotFullByLimit {
        /// The offending side.
        side: Side,
        /// The temperature (degC) by which 100 % is required.
        limit_c: f64,
    },
    /// A follower parameter is not a number or outside its range.
    Param {
        /// The JSON key.
        key: &'static str,
        /// Lowest allowed value.
        lo: f64,
        /// Highest allowed value.
        hi: f64,
    },
    /// The command-line point list (`55:0,62:35`) does not parse.
    BadPointList,
    /// A curve name is empty, too long or has characters outside `A-Za-z0-9._-`.
    BadName,
    /// Saving over a built-in curve was attempted.
    BuiltinSave(String),
    /// Deleting a built-in curve was attempted.
    BuiltinDelete(String),
    /// No curve of this name exists. `listed` is true for a load (the message then points at
    /// `truesense fan curve list`), false for a delete.
    NotFound {
        /// The requested name.
        name: String,
        /// Whether the message carries the hint to list the curves.
        listed: bool,
    },
    /// The curve file is not JSON.
    InvalidJson(String),
    /// Reading or writing a curve file failed.
    Fs(FsError),
    /// A directory operation (listing, deleting) failed.
    Dir {
        /// What was being done.
        op: &'static str,
        /// The path involved.
        path: PathBuf,
        /// The system error.
        source: io::Error,
    },
    /// The curve could not be turned into JSON (cannot happen for a validated curve).
    Encode(String),
}

impl fmt::Display for CurveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CurveError::NotAnObject => f.write_str("a curve is an object with 'cpu' and 'gpu' points"),
            CurveError::PointCount(s) => write!(f, "{s}: 2 to 12 points"),
            CurveError::PointShape(s) => write!(f, "{s}: every point is [temperature, percent]"),
            CurveError::PointRange(s) => {
                write!(f, "{s}: temperature 20..110 C, percent 0 (firmware auto) or 20..100")
            }
            CurveError::NotMonotonic(s) => write!(f, "{s}: the duty may only rise with temperature"),
            CurveError::DuplicateTemperature(s) => write!(f, "{s}: two points share a temperature"),
            CurveError::NotFullByLimit { side, limit_c } => {
                write!(f, "{side}: must reach 100 % by {limit_c} C (the CPU and GPU throttle above that)")
            }
            CurveError::Param { key, lo, hi } => write!(f, "{key}: {lo}..{hi}"),
            CurveError::BadPointList => {
                f.write_str("points look like 55:0,62:35,70:55,80:80,88:100  (temperature:percent)")
            }
            CurveError::BadName => f.write_str("curve names use letters, digits, . _ - (max 40)"),
            CurveError::BuiltinSave(n) => write!(f, "'{n}' is built in; save your curve under another name"),
            CurveError::BuiltinDelete(n) => write!(f, "'{n}' is built in and cannot be deleted"),
            CurveError::NotFound { name, listed: true } => {
                write!(f, "no fan curve '{name}' (truesense fan curve list)")
            }
            CurveError::NotFound { name, listed: false } => write!(f, "no fan curve '{name}'"),
            CurveError::InvalidJson(n) => write!(f, "fan curve '{n}' is not valid JSON"),
            CurveError::Fs(e) => e.fmt(f),
            CurveError::Dir { op, path, source } => write!(f, "{op} {}: {source}", path.display()),
            CurveError::Encode(m) => write!(f, "cannot encode the curve: {m}"),
        }
    }
}

impl std::error::Error for CurveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CurveError::Fs(e) => Some(e),
            CurveError::Dir { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<FsError> for CurveError {
    fn from(e: FsError) -> Self {
        CurveError::Fs(e)
    }
}

/// The CPU fan or the GPU fan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The CPU fan (fan 1 in the firmware).
    Cpu,
    /// The GPU fan (fan 4 in the firmware).
    Gpu,
}

impl Side {
    /// The JSON key and the word used in messages.
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Cpu => "cpu",
            Side::Gpu => "gpu",
        }
    }

    /// The temperature by which this side must be at 100 %.
    fn full_by_c(self) -> f64 {
        match self {
            Side::Cpu => CPU_FULL_BY_C,
            Side::Gpu => GPU_FULL_BY_C,
        }
    }

    fn index(self) -> usize {
        match self {
            Side::Cpu => 0,
            Side::Gpu => 1,
        }
    }
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Writes a whole number as a JSON integer and anything else as a float, because the Python
/// edition's files and socket replies carry `56`, not `56.0`.
fn serialize_number<S: Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 1e15 {
        // The guard keeps the cast exact.
        s.serialize_i64(*v as i64)
    } else {
        s.serialize_f64(*v)
    }
}

/// A number that serialises like [`serialize_number`].
struct Num(f64);

impl Serialize for Num {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        serialize_number(&self.0, s)
    }
}

/// One curve point. It serialises as the two-element array `[temp, duty]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// Temperature in degC.
    pub temp: f64,
    /// Fan duty in percent (0 = firmware Auto).
    pub duty: f64,
}

impl Point {
    /// A point at `temp` degC asking for `duty` percent.
    pub const fn new(temp: f64, duty: f64) -> Point {
        Point { temp, duty }
    }
}

impl Serialize for Point {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq([Num(self.temp), Num(self.duty)])
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A fan curve with its follower parameters. The field names are the Python dict keys: the
/// daemon exposes this structure on the control socket.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Curve {
    /// Free text shown in the curve list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// "Nothing of ours runs": the firmware controls the fans like PredatorSense on Windows.
    #[serde(skip_serializing_if = "is_false")]
    pub native: bool,
    /// CPU fan points, sorted by temperature once validated.
    pub cpu: Vec<Point>,
    /// GPU fan points, sorted by temperature once validated.
    pub gpu: Vec<Point>,
    /// How far (degC) the temperature must fall below a point before the duty may follow it
    /// down, so the fans do not hunt around one temperature.
    #[serde(serialize_with = "serialize_number")]
    pub hysteresis: f64,
    /// The fastest the duty may fall, in percent per second: rising is immediate, falling is
    /// slow, which keeps the noise steady.
    #[serde(serialize_with = "serialize_number")]
    pub down_rate: f64,
    /// Added to a rising temperature before the lookup, so the fans start before the heat.
    #[serde(serialize_with = "serialize_number")]
    pub predict: f64,
    /// How far (percent) the quieter fan may trail the louder one: the two fans share
    /// heatpipes, so one fan alone cannot cool its own chip.
    #[serde(serialize_with = "serialize_number")]
    pub couple: f64,
    /// Subtracted from the temperature on battery (so the fans come in later and the laptop
    /// is quieter).
    #[serde(serialize_with = "serialize_number")]
    pub battery_shift: f64,
}

impl Curve {
    /// A curve with the given points and the default follower parameters. It is not
    /// validated; see [`Curve::validated`].
    pub fn new(cpu: Vec<Point>, gpu: Vec<Point>) -> Curve {
        Curve {
            description: None,
            native: false,
            cpu,
            gpu,
            hysteresis: DEFAULT_HYSTERESIS,
            down_rate: DEFAULT_DOWN_RATE,
            predict: DEFAULT_PREDICT,
            couple: DEFAULT_COUPLE,
            battery_shift: DEFAULT_BATTERY_SHIFT,
        }
    }

    /// The points of one side.
    pub fn points(&self, side: Side) -> &[Point] {
        match side {
            Side::Cpu => &self.cpu,
            Side::Gpu => &self.gpu,
        }
    }

    /// The curve as JSON, for the control socket.
    pub fn to_value(&self) -> Value {
        // A struct of strings and finite numbers always converts; Null is unreachable.
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    /// Checks a typed curve and returns a normalised copy (points sorted by temperature). It
    /// enforces the safety rules in the order Python did: per side (CPU first) the point count,
    /// the ranges (temperature 20..=110, duty 0 or 20..=100), that the duty never falls as the
    /// temperature rises, that no two points share a temperature and that the side reaches
    /// 100 % by 95 degC (CPU) / 85 degC (GPU); then the five parameters.
    pub fn validated(&self) -> Result<Curve, CurveError> {
        let mut out = self.clone();
        for side in [Side::Cpu, Side::Gpu] {
            let pts = check_side(side, self.points(side).to_vec())?;
            match side {
                Side::Cpu => out.cpu = pts,
                Side::Gpu => out.gpu = pts,
            }
        }
        for (key, lo, hi) in PARAM_RANGES {
            // The negated comparison also rejects NaN.
            if !(lo <= self.param(key) && self.param(key) <= hi) {
                return Err(CurveError::Param { key, lo, hi });
            }
        }
        Ok(out)
    }

    fn param(&self, key: &str) -> f64 {
        match key {
            "hysteresis" => self.hysteresis,
            "down_rate" => self.down_rate,
            "predict" => self.predict,
            "couple" => self.couple,
            _ => self.battery_shift,
        }
    }

    fn set_param(&mut self, key: &str, v: f64) {
        match key {
            "hysteresis" => self.hysteresis = v,
            "down_rate" => self.down_rate = v,
            "predict" => self.predict = v,
            "couple" => self.couple = v,
            _ => self.battery_shift = v,
        }
    }

    /// A copy of a built-in curve, or `None` for an unknown name. Built-ins are tuned for the
    /// PH16-71 (i7-13700HX plus RTX 4060, two fans on shared heatpipes): the CPU reaches 90 C
    /// in seconds under load while the GPU side is calmer, so the CPU curve ramps from 56 C
    /// and tops out at 89 C, the GPU curve from 54 C to 78 C. Firmware Auto below the first
    /// point (silent when cool).
    pub fn builtin(name: &str) -> Option<Curve> {
        let p = |v: &[(f64, f64)]| v.iter().map(|&(t, d)| Point::new(t, d)).collect::<Vec<_>>();
        let mut c = match name {
            "native" => {
                let mut c = Curve::new(p(&[(52.0, 0.0), (89.0, 100.0)]), p(&[(48.0, 0.0), (78.0, 100.0)]));
                c.description = Some(
                    "PredatorSense default: the firmware controls the fans exactly as it does on Windows \
                     (they stop when cool). Nothing of ours runs"
                        .into(),
                );
                c.native = true;
                c
            }
            "truesense" => {
                let mut c = Curve::new(
                    p(&[
                        (52.0, 0.0),
                        (56.0, 30.0),
                        (62.0, 38.0),
                        (68.0, 48.0),
                        (74.0, 58.0),
                        (80.0, 70.0),
                        (85.0, 85.0),
                        (89.0, 100.0),
                    ]),
                    p(&[
                        (48.0, 0.0),
                        (54.0, 30.0),
                        (60.0, 38.0),
                        (65.0, 48.0),
                        (70.0, 60.0),
                        (74.0, 78.0),
                        (78.0, 100.0),
                    ]),
                );
                c.description = Some(
                    "Our curve for this laptop: silent when cool, early and smooth ramp, full speed before \
                     throttling"
                        .into(),
                );
                c
            }
            "quiet" => {
                let mut c = Curve::new(
                    p(&[(58.0, 0.0), (66.0, 30.0), (72.0, 40.0), (78.0, 55.0), (84.0, 75.0), (89.0, 100.0)]),
                    p(&[(52.0, 0.0), (60.0, 30.0), (66.0, 40.0), (72.0, 58.0), (77.0, 80.0), (80.0, 100.0)]),
                );
                c.description = Some("Lowest noise: the fans stay off longer and ramp later".into());
                c.predict = 3.0;
                c.down_rate = 1.0;
                c
            }
            "performance" => {
                let mut c = Curve::new(
                    p(&[(45.0, 30.0), (55.0, 40.0), (62.0, 55.0), (70.0, 70.0), (78.0, 85.0), (84.0, 100.0)]),
                    p(&[(42.0, 30.0), (52.0, 40.0), (60.0, 55.0), (66.0, 70.0), (72.0, 85.0), (76.0, 100.0)]),
                );
                c.description = Some("Cooler and louder: starts early and ramps hard".into());
                c.predict = 6.0;
                c.down_rate = 2.5;
                c
            }
            "max-cooling" => {
                let mut c = Curve::new(p(&[(40.0, 40.0), (60.0, 100.0)]), p(&[(40.0, 40.0), (58.0, 100.0)]));
                c.description = Some("Fans at full speed above 60 C".into());
                c.predict = 8.0;
                c.down_rate = 4.0;
                c
            }
            _ => return None,
        };
        // Built-ins are normalised like everything else: whole numbers, sorted.
        c.cpu.sort_by(|a, b| a.temp.total_cmp(&b.temp));
        c.gpu.sort_by(|a, b| a.temp.total_cmp(&b.temp));
        Some(c)
    }
}

/// The built-in curve names in display order.
pub const BUILTIN_NAMES: [&str; 5] = ["native", "truesense", "quiet", "performance", "max-cooling"];

/// Whether `name` is a built-in (read-only) curve.
pub fn is_builtin(name: &str) -> bool {
    BUILTIN_NAMES.contains(&name)
}

/// Piecewise-linear duty for temperature `t`, clamped to the first and last point. The points
/// must be sorted by temperature; an empty list gives 0 (firmware Auto), where Python would
/// have raised.
pub fn interp(points: &[Point], t: f64) -> f64 {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return 0.0;
    };
    if t <= first.temp {
        return first.duty;
    }
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t <= b.temp {
            return if b.temp > a.temp {
                a.duty + (b.duty - a.duty) * (t - a.temp) / (b.temp - a.temp)
            } else {
                b.duty
            };
        }
    }
    last.duty
}

/// Parses the command-line form `"52:0,60:35,70:55,88:100"`. It only parses; the result still
/// has to pass [`Curve::validated`]. Spaces are ignored and empty items skipped. NaN and
/// infinities are refused (they can never be a temperature or a duty; Python crashed on them).
pub fn parse_points(text: &str) -> Result<Vec<Point>, CurveError> {
    let mut out = Vec::new();
    for item in text.replace(' ', "").split(',').filter(|s| !s.is_empty()) {
        let mut parts = item.split(':');
        let (Some(a), Some(b), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(CurveError::BadPointList);
        };
        match (parse_finite(a), parse_finite(b)) {
            (Some(temp), Some(duty)) => out.push(Point::new(temp, duty)),
            _ => return Err(CurveError::BadPointList),
        }
    }
    Ok(out)
}

fn parse_finite(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Validates a curve that arrived as decoded JSON (a control-socket request, a file) and
/// returns it with the defaults filled in; anything that could hurt (a curve too cool for the
/// load) or is malformed is an error. Unknown keys are ignored, as Python did.
pub fn validate(value: &Value) -> Result<Curve, CurveError> {
    let obj = value.as_object().ok_or(CurveError::NotAnObject)?;
    let mut c = Curve::new(Vec::new(), Vec::new());
    c.description = obj.get("description").and_then(Value::as_str).map(str::to_owned);
    c.native = obj.get("native").and_then(Value::as_bool).unwrap_or(false);
    // Python checked each side completely (shape, then ranges and safety) before it looked at
    // the next one, so the first error reported is the same here.
    for side in [Side::Cpu, Side::Gpu] {
        let pts = check_side(side, decode_points(side, obj.get(side.as_str()))?)?;
        match side {
            Side::Cpu => c.cpu = pts,
            Side::Gpu => c.gpu = pts,
        }
    }
    for (key, lo, hi) in PARAM_RANGES {
        if let Some(v) = obj.get(key) {
            // Numbers only: Python also let a JSON `true` through as 1 (a quirk, not kept).
            match v.as_f64() {
                Some(f) if v.is_number() => c.set_param(key, f),
                _ => return Err(CurveError::Param { key, lo, hi }),
            }
        }
        let have = c.param(key);
        if !(lo <= have && have <= hi) {
            return Err(CurveError::Param { key, lo, hi });
        }
    }
    Ok(c)
}

/// [`validate`] for JSON text. Text that is not JSON is `InvalidJson` with an empty name; a
/// caller that knows the curve name uses [`Store::load`].
pub fn parse(data: &[u8]) -> Result<Curve, CurveError> {
    let value: Value = serde_json::from_slice(data).map_err(|_| CurveError::InvalidJson(String::new()))?;
    validate(&value)
}

/// Reads one side's points: a list of 2..=12 items, each a `[temperature, percent]` pair of
/// numbers.
fn decode_points(side: Side, raw: Option<&Value>) -> Result<Vec<Point>, CurveError> {
    let items = match raw {
        Some(Value::Array(a)) if (MIN_POINTS..=MAX_POINTS).contains(&a.len()) => a,
        _ => return Err(CurveError::PointCount(side)),
    };
    items
        .iter()
        .map(|item| match item.as_array().map(Vec::as_slice) {
            // Numbers only: Python's float() also took numeric strings and booleans here.
            Some([t, d]) if t.is_number() && d.is_number() => match (t.as_f64(), d.as_f64()) {
                (Some(temp), Some(duty)) => Ok(Point::new(temp, duty)),
                _ => Err(CurveError::PointShape(side)),
            },
            _ => Err(CurveError::PointShape(side)),
        })
        .collect()
}

/// The safety rules for one side, on typed points; returns them sorted.
fn check_side(side: Side, mut pts: Vec<Point>) -> Result<Vec<Point>, CurveError> {
    if !(MIN_POINTS..=MAX_POINTS).contains(&pts.len()) {
        return Err(CurveError::PointCount(side));
    }
    pts.sort_by(|a, b| a.temp.total_cmp(&b.temp).then(a.duty.total_cmp(&b.duty)));
    for p in &pts {
        // The negated comparisons also reject NaN.
        let temp_ok = MIN_TEMP_C <= p.temp && p.temp <= MAX_TEMP_C;
        let duty_ok = p.duty == 0.0 || (MIN_DUTY_PCT <= p.duty && p.duty <= MAX_DUTY_PCT);
        if !temp_ok || !duty_ok {
            return Err(CurveError::PointRange(side));
        }
    }
    if pts.windows(2).any(|w| w[1].duty < w[0].duty) {
        return Err(CurveError::NotMonotonic(side));
    }
    if pts.windows(2).any(|w| w[0].temp == w[1].temp) {
        return Err(CurveError::DuplicateTemperature(side));
    }
    let limit_c = side.full_by_c();
    if interp(&pts, limit_c) < 100.0 {
        return Err(CurveError::NotFullByLimit { side, limit_c });
    }
    Ok(pts)
}

/// What [`Store::load_active`] settled on.
#[derive(Debug)]
pub struct ActiveCurve {
    /// The curve to follow.
    pub curve: Curve,
    /// Why the stored curve could not be used and the built-in `truesense` stands in (the
    /// daemon logs it); `None` when the stored curve loaded.
    pub fallback: Option<CurveError>,
}

/// The user's own curves, one JSON file per curve (`NAME.json`) in a directory. The built-in
/// curves are always available and read-only. The directory is a field so tests and other
/// users can point it elsewhere.
#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// A store over `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Store {
        Store { dir: dir.into() }
    }

    /// `$HOME/.config/truesense/curves`, taking the environment through a lookup so tests can
    /// inject one; `None` without `HOME`.
    pub fn default_dir_from(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
        env("HOME").map(|h| PathBuf::from(h).join(".config/truesense/curves"))
    }

    /// [`Store::default_dir_from`] for the running process.
    pub fn default_dir() -> Option<PathBuf> {
        Store::default_dir_from(&fsx::env_nonempty)
    }

    /// The directory the curves live in.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, name: &str) -> Result<PathBuf, CurveError> {
        valid_name(name).then(|| self.dir.join(format!("{name}.json"))).ok_or(CurveError::BadName)
    }

    /// The built-in curve names first, then the user's, sorted. A directory that does not
    /// exist yet is not an error. File names that are not valid curve names are skipped:
    /// listing something that can never be loaded would only mislead (Python listed them).
    pub fn names(&self) -> Result<Vec<String>, CurveError> {
        let mut out: Vec<String> = BUILTIN_NAMES.iter().map(|s| (*s).to_owned()).collect();
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound || e.kind() == io::ErrorKind::NotADirectory => {
                return Ok(out);
            }
            Err(e) => return Err(CurveError::Dir { op: "list", path: self.dir.clone(), source: e }),
        };
        let mut own = Vec::new();
        for entry in entries {
            let entry =
                entry.map_err(|e| CurveError::Dir { op: "list", path: self.dir.clone(), source: e })?;
            let file = entry.file_name();
            if let Some(stem) = file.to_str().and_then(|n| n.strip_suffix(".json")) {
                if valid_name(stem) && !is_builtin(stem) {
                    own.push(stem.to_owned());
                }
            }
        }
        own.sort();
        out.extend(own);
        Ok(out)
    }

    /// A built-in or saved curve, validated.
    pub fn load(&self, name: &str) -> Result<Curve, CurveError> {
        if let Some(c) = Curve::builtin(name) {
            return Ok(c);
        }
        let path = self.path(name)?;
        let bytes = match fsx::read_limited(&path, MAX_SETTINGS_FILE) {
            Ok(b) => b,
            Err(e) if e.is_not_found() => {
                return Err(CurveError::NotFound { name: name.to_owned(), listed: true });
            }
            Err(e) => return Err(e.into()),
        };
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| CurveError::InvalidJson(name.to_owned()))?;
        validate(&value)
    }

    /// The curve the daemon follows: `data` (a curve the daemon was sent and keeps in its
    /// state, so the choice works before anyone logs in) if it is set, else the curve called
    /// `name`, where `native` stands for `truesense` (the native curve is no curve, the
    /// caller handles it). A stored curve that no longer validates falls back to the built-in
    /// `truesense` with the reason in [`ActiveCurve::fallback`]: the fans must never end up
    /// without a curve.
    pub fn load_active(&self, data: Option<&Value>, name: &str) -> ActiveCurve {
        let wanted = match data.filter(|d| truthy(d)) {
            Some(d) => validate(d),
            None => self.load(if name == "native" { "truesense" } else { name }),
        };
        match wanted {
            Ok(curve) => ActiveCurve { curve, fallback: None },
            Err(e) => ActiveCurve { curve: builtin_truesense(), fallback: Some(e) },
        }
    }

    /// Validates a curve and writes it atomically under `name`; returns the validated curve.
    /// Built-in names are refused so a built-in can never be altered: copy it under another
    /// name instead.
    pub fn save(&self, name: &str, curve: &Value) -> Result<Curve, CurveError> {
        if is_builtin(name) {
            return Err(CurveError::BuiltinSave(name.to_owned()));
        }
        let c = validate(curve)?;
        let path = self.path(name)?;
        // One-space indent: the file format of the Python edition (json.dump(indent=1)).
        let mut buf = Vec::new();
        let fmt = serde_json::ser::PrettyFormatter::with_indent(b" ");
        let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
        c.serialize(&mut ser).map_err(|e| CurveError::Encode(e.to_string()))?;
        fsx::write_atomic_mkdir(&path, &buf, 0o644)?;
        Ok(c)
    }

    /// Removes a saved curve. Built-in curves cannot be deleted.
    pub fn delete(&self, name: &str) -> Result<(), CurveError> {
        if is_builtin(name) {
            return Err(CurveError::BuiltinDelete(name.to_owned()));
        }
        let path = self.path(name)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Err(CurveError::NotFound { name: name.to_owned(), listed: false })
            }
            Err(e) => Err(CurveError::Dir { op: "delete", path, source: e }),
        }
    }
}

/// The curve that stands in when the stored one is unusable. The built-in always validates
/// (a test proves it); should that ever break, the unvalidated curve is still safer than none.
fn builtin_truesense() -> Curve {
    Curve::builtin("truesense").unwrap_or_else(|| Curve::new(Vec::new(), Vec::new()))
}

/// Python truthiness of a JSON value: the daemon stores "no custom curve" as null, and an
/// empty object counts as none as well.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Turns temperatures into fan duties: rises at once, falls slowly and only after the
/// temperature has dropped by the hysteresis.
///
/// A pure state machine: every input (temperatures, whether they are rising, the seconds
/// since the last step, the battery shift) is passed in; it reads no clock and does no I/O,
/// so the same inputs always give the same outputs. The daemon's control loop owns it.
#[derive(Debug, Clone)]
pub(crate) struct Follower {
    curve: Curve,
    duty: [f64; 2],
}

impl Follower {
    /// A follower on `curve` with both duties at 0 (firmware Auto).
    pub(crate) fn new(curve: Curve) -> Follower {
        Follower { curve, duty: [0.0; 2] }
    }

    /// Switches curve and resets both duties, so the new curve takes effect cleanly.
    pub(crate) fn set_curve(&mut self, curve: Curve) {
        self.curve = curve;
        self.duty = [0.0; 2];
    }

    /// The curve in use.
    pub(crate) fn curve(&self) -> &Curve {
        &self.curve
    }

    /// The current CPU and GPU duties in percent, before quantising.
    pub(crate) fn duty(&self) -> (f64, f64) {
        (self.duty[0], self.duty[1])
    }

    /// Forces both duties. The controller uses it when it overrides the curve: 0, 0 when the
    /// laptop is calm (the fans go back to the firmware) and 100, 100 when it is hot.
    pub(crate) fn set_duty(&mut self, cpu: f64, gpu: f64) {
        self.duty = [cpu, gpu];
    }

    /// The temperature shift to pass to [`Follower::step`]: nothing on AC power, the curve's
    /// `battery_shift` on battery.
    pub(crate) fn battery_shift(&self, on_ac: bool) -> f64 {
        if on_ac { 0.0 } else { self.curve.battery_shift }
    }

    /// The duty the curve asks for right now. A rising temperature is looked up `predict`
    /// degrees ahead so the fans start before the heat arrives; `shift` (degC, positive on
    /// battery) delays them. The additions are in Python's order, `(temp + predict) - shift`,
    /// so the result is bit-identical.
    pub(crate) fn target(&self, side: Side, temp: f64, rising: bool, shift: f64) -> f64 {
        let t = temp + if rising { self.curve.predict } else { 0.0 } - shift;
        interp(self.curve.points(side), t)
    }

    /// Advances one side by `dt_s` seconds and returns its new duty. A higher target is taken
    /// at once; a lower one is followed only when the temperature less the hysteresis would
    /// also give a lower duty, and then no faster than `down_rate` percent per second.
    pub(crate) fn step(&mut self, side: Side, temp: f64, rising: bool, dt_s: f64, shift: f64) -> f64 {
        let up = self.target(side, temp, rising, shift);
        let mut cur = self.duty[side.index()];
        if up >= cur {
            cur = up;
        } else {
            let down = interp(self.curve.points(side), temp - shift - self.curve.hysteresis);
            if down < cur {
                cur = down.max(cur - self.curve.down_rate * dt_s);
            }
        }
        self.duty[side.index()] = cur;
        cur
    }

    /// The (cpu %, gpu %) to write, in steps of 5; `(0, 0)` means firmware Auto. A duty under
    /// 20 counts as 0 because no fan can hold it. `floor` raises both duties to at least that
    /// value (the activity floor; 0 disables it). The two fans share heatpipes, so the quieter
    /// one never trails the louder one by more than `couple`, and when anything runs both
    /// are at least 20.
    pub(crate) fn output(&self, floor: u8) -> (u8, u8) {
        let mut c = quantise(self.duty[0]);
        let mut g = quantise(self.duty[1]);
        if floor != 0 {
            c = c.max(floor);
            g = g.max(floor);
        }
        if c == 0 && g == 0 {
            return (0, 0);
        }
        let (cf, gf) = (f64::from(c), f64::from(g));
        let cp = self.curve.couple;
        (couple_duty(cf, gf - cp), couple_duty(gf, cf - cp))
    }
}

/// `max(own, other - couple, 20)` as a whole percent. A fractional `couple` can give a
/// fractional result; it is truncated (the Go edition does the same, Python would have passed
/// the float on to the driver).
fn couple_duty(own: f64, lifted: f64) -> u8 {
    let d = own.max(lifted).clamp(MIN_DUTY_PCT, MAX_DUTY_PCT);
    d as u8
}

/// Rounds a duty to steps of 5 %; anything below the fans' minimum of 20 % is 0. Halves round
/// to even, as the Python `round()` the original used does.
fn quantise(d: f64) -> u8 {
    if d < MIN_DUTY_PCT {
        return 0;
    }
    let stepped = 5.0 * (d / 5.0).round_ties_even();
    stepped.clamp(0.0, MAX_DUTY_PCT) as u8
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn p(t: f64, d: f64) -> Point {
        Point::new(t, d)
    }

    fn builtin(name: &str) -> Curve {
        Curve::builtin(name).expect("built in")
    }

    /// A fresh temp directory removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> TempDir {
            static N: AtomicU32 = AtomicU32::new(0);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.subsec_nanos());
            let dir = std::env::temp_dir().join(format!(
                "ts-curves-{}-{nanos:x}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).expect("temp dir");
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    // ---- built-ins ---------------------------------------------------------------------------

    #[test]
    fn builtins_are_valid_unchanged_and_in_order() {
        assert_eq!(BUILTIN_NAMES, ["native", "truesense", "quiet", "performance", "max-cooling"]);
        for n in BUILTIN_NAMES {
            let c = builtin(n);
            let v = c.validated();
            assert!(v.is_ok(), "{n}: {v:?}");
            let v = v.expect("checked");
            assert_eq!(v, c, "{n}: validation changed a built-in");
            assert!(is_builtin(n));
            assert!(c.description.is_some());
        }
        assert!(builtin("native").native);
        assert!(!builtin("truesense").native);
        assert!(Curve::builtin("nope").is_none());
        assert!(!is_builtin("nope"));
        let q = builtin("quiet");
        assert_eq!(
            (q.predict, q.down_rate, q.hysteresis, q.couple, q.battery_shift),
            (3.0, 1.0, 4.0, 15.0, 3.0)
        );
        let pf = builtin("performance");
        assert_eq!((pf.predict, pf.down_rate), (6.0, 2.5));
        let mc = builtin("max-cooling");
        assert_eq!((mc.predict, mc.down_rate), (8.0, 4.0));
        let t = builtin("truesense");
        assert_eq!((t.predict, t.down_rate, t.hysteresis), (5.0, 1.5, 4.0));
        assert_eq!(t.cpu.len(), 8);
        assert_eq!(t.gpu.len(), 7);
    }

    #[test]
    fn builtin_returns_a_copy() {
        let mut a = builtin("truesense");
        a.cpu[0] = p(99.0, 99.0);
        assert_ne!(builtin("truesense").cpu[0], p(99.0, 99.0));
    }

    #[test]
    fn the_stand_in_curve_is_the_valid_builtin() {
        let c = builtin_truesense();
        assert!(c.validated().is_ok());
        assert_eq!(c, builtin("truesense"));
    }

    // ---- interp and parse_points ------------------------------------------------------------

    #[test]
    fn interp_table() {
        let c = builtin("truesense");
        let cases: [(&str, &[Point], f64, f64); 9] = [
            ("python test: truesense cpu at 80", &c.cpu, 80.0, 70.0),
            ("above the last point clamps (python test)", &c.cpu, 100.0, 100.0),
            ("below the first point clamps", &c.cpu, 10.0, 0.0),
            ("exactly on the first point", &c.cpu, 52.0, 0.0),
            ("midway between 56:30 and 62:38", &c.cpu, 59.0, 34.0),
            ("exactly on an inner point", &c.cpu, 68.0, 48.0),
            ("empty list is firmware auto", &[], 50.0, 0.0),
            ("single point", &[Point::new(50.0, 40.0)], 70.0, 40.0),
            (
                "equal temperatures do not divide by zero",
                &[Point::new(50.0, 20.0), Point::new(50.0, 60.0), Point::new(60.0, 100.0)],
                50.0,
                20.0,
            ),
        ];
        for (name, pts, t, want) in cases {
            let got = interp(pts, t);
            assert!((got - want).abs() < 1e-9, "{name}: {got} != {want}");
        }
        // NaN never gets past the first comparison: it reads as "hotter than everything".
        assert_eq!(interp(&c.cpu, f64::NAN), 100.0);
    }

    #[test]
    fn parse_points_table() {
        let ok: [(&str, Vec<Point>); 4] = [
            ("52:0,60:35,70:55,88:100", vec![p(52.0, 0.0), p(60.0, 35.0), p(70.0, 55.0), p(88.0, 100.0)]),
            (" 52 : 0 , 60:35.5 ,", vec![p(52.0, 0.0), p(60.0, 35.5)]),
            ("", vec![]),
            (",,", vec![]),
        ];
        for (text, want) in ok {
            assert_eq!(parse_points(text).expect(text), want, "{text:?}");
        }
        for text in
            ["52", "52:0:1", "a:b", "52:", ":5", "nan:0", "50:inf", "1e999:1", "0x10:1", "1_0:5", "5:-inf"]
        {
            let e = parse_points(text).expect_err(text);
            assert!(e.to_string().starts_with("points look like 55:0,62:35"), "{text}: {e}");
        }
    }

    // ---- validate -----------------------------------------------------------------------------

    fn ok_curve() -> Value {
        json!({"cpu": [[50, 0], [95, 100]], "gpu": [[50, 0], [85, 100]]})
    }

    fn with(mut base: Value, key: &str, v: Value) -> Value {
        base[key] = v;
        base
    }

    #[test]
    fn validate_table() {
        let many: Vec<Value> = (0..13).map(|i| json!([40 + i * 5, 100])).collect();
        let cases: Vec<(&str, Value, &str)> = vec![
            ("minimal valid curve", ok_curve(), ""),
            ("not an object (list)", json!([1, 2]), "a curve is an object"),
            ("null", Value::Null, "a curve is an object"),
            ("text", json!("curve"), "a curve is an object"),
            (
                "weak curve from the Python test",
                json!({"cpu": [[50, 0], [70, 50]], "gpu": [[50, 0], [78, 100]]}),
                "cpu: must reach 100 % by 95 C (the CPU and GPU throttle above that)",
            ),
            (
                "gpu must reach 100 by 85",
                with(ok_curve(), "gpu", json!([[50, 0], [90, 100]])),
                "gpu: must reach 100 % by 85 C",
            ),
            ("missing cpu", json!({"gpu": [[50, 0], [85, 100]]}), "cpu: 2 to 12 points"),
            ("cpu not a list", with(ok_curve(), "cpu", json!("x")), "cpu: 2 to 12 points"),
            ("cpu null", with(ok_curve(), "cpu", Value::Null), "cpu: 2 to 12 points"),
            ("one point", with(ok_curve(), "cpu", json!([[90, 100]])), "cpu: 2 to 12 points"),
            ("thirteen points", with(ok_curve(), "cpu", Value::Array(many.clone())), "cpu: 2 to 12 points"),
            ("twelve points are fine", with(ok_curve(), "cpu", Value::Array(many[..12].to_vec())), ""),
            (
                "point with three items",
                with(ok_curve(), "cpu", json!([[50, 0, 1], [95, 100]])),
                "cpu: every point is [temperature, percent]",
            ),
            (
                "point with text",
                with(ok_curve(), "cpu", json!([["hot", 0], [95, 100]])),
                "cpu: every point is [temperature, percent]",
            ),
            (
                "point not a list",
                with(ok_curve(), "gpu", json!([5, [85, 100]])),
                "gpu: every point is [temperature, percent]",
            ),
            (
                "point with null",
                with(ok_curve(), "gpu", json!([[null, 0], [85, 100]])),
                "gpu: every point is [temperature, percent]",
            ),
            (
                "temperature below 20",
                with(ok_curve(), "cpu", json!([[19, 0], [95, 100]])),
                "temperature 20..110",
            ),
            ("temperature 20 is the minimum", with(ok_curve(), "cpu", json!([[20, 0], [95, 100]])), ""),
            (
                "temperature above 110",
                with(ok_curve(), "cpu", json!([[50, 0], [111, 100]])),
                "temperature 20..110",
            ),
            (
                "duty 10 is not allowed (fans run at 0 or 20..100)",
                with(ok_curve(), "cpu", json!([[50, 10], [95, 100]])),
                "percent 0 (firmware auto) or 20..100",
            ),
            (
                "duty 19",
                with(ok_curve(), "cpu", json!([[50, 19], [95, 100]])),
                "percent 0 (firmware auto) or 20..100",
            ),
            ("duty 20 is the minimum", with(ok_curve(), "cpu", json!([[50, 20], [95, 100]])), ""),
            ("duty 19.99", with(ok_curve(), "cpu", json!([[50, 19.99], [95, 100]])), "percent 0"),
            ("duty above 100", with(ok_curve(), "cpu", json!([[50, 0], [95, 101]])), "percent 0"),
            ("negative duty", with(ok_curve(), "cpu", json!([[50, -5], [95, 100]])), "percent 0"),
            (
                "duty falls with temperature",
                with(ok_curve(), "cpu", json!([[50, 50], [60, 40], [95, 100]])),
                "cpu: the duty may only rise with temperature",
            ),
            (
                "duplicate temperature",
                with(ok_curve(), "cpu", json!([[50, 20], [50, 40], [95, 100]])),
                "cpu: two points share a temperature",
            ),
            ("unsorted points are sorted", with(ok_curve(), "cpu", json!([[95, 100], [50, 0]])), ""),
            (
                "100 % reached exactly at the cap",
                json!({"cpu": [[50, 0], [95, 100]], "gpu": [[40, 20], [85, 100]]}),
                "",
            ),
            ("hysteresis too big", with(ok_curve(), "hysteresis", json!(16)), "hysteresis: 0..15"),
            ("hysteresis negative", with(ok_curve(), "hysteresis", json!(-1)), "hysteresis: 0..15"),
            ("hysteresis as text", with(ok_curve(), "hysteresis", json!("4")), "hysteresis: 0..15"),
            ("hysteresis as bool", with(ok_curve(), "hysteresis", json!(true)), "hysteresis: 0..15"),
            ("hysteresis null", with(ok_curve(), "hysteresis", Value::Null), "hysteresis: 0..15"),
            ("down_rate too small", with(ok_curve(), "down_rate", json!(0.1)), "down_rate: 0.2..20"),
            ("down_rate too big", with(ok_curve(), "down_rate", json!(21)), "down_rate: 0.2..20"),
            ("predict too big", with(ok_curve(), "predict", json!(15.5)), "predict: 0..15"),
            ("couple too big", with(ok_curve(), "couple", json!(51)), "couple: 0..50"),
            ("battery_shift too big", with(ok_curve(), "battery_shift", json!(16)), "battery_shift: 0..15"),
            (
                "all limits at their edge",
                {
                    let mut c = ok_curve();
                    for (k, v) in [
                        ("hysteresis", 15.0),
                        ("down_rate", 0.2),
                        ("predict", 15.0),
                        ("couple", 50.0),
                        ("battery_shift", 15.0),
                    ] {
                        c[k] = json!(v);
                    }
                    c
                },
                "",
            ),
            ("unknown keys are ignored", with(ok_curve(), "bogus", json!(1)), ""),
        ];
        for (name, input, want) in cases {
            match validate(&input) {
                Ok(c) => {
                    assert_eq!(want, "", "{name}: accepted, wanted error {want:?}");
                    for side in [Side::Cpu, Side::Gpu] {
                        let pts = c.points(side);
                        assert!(pts.windows(2).all(|w| w[0].temp < w[1].temp), "{name}: not sorted {pts:?}");
                    }
                }
                Err(e) => {
                    assert!(!want.is_empty(), "{name}: unexpected error {e}");
                    assert!(e.to_string().contains(want), "{name}: {e} does not contain {want:?}");
                }
            }
        }
    }

    #[test]
    fn the_first_error_is_the_one_python_reported() {
        // Python finished the CPU side (including the 100 % rule) before it looked at the GPU
        // side's shape, so the CPU message wins.
        let both_bad = json!({"cpu": [[50, 0], [70, 50]], "gpu": "x"});
        assert_eq!(
            validate(&both_bad).expect_err("bad").to_string(),
            "cpu: must reach 100 % by 95 C (the CPU and GPU throttle above that)"
        );
        // Sides before parameters.
        let bad_param = json!({"cpu": [[50, 0], [70, 50]], "gpu": [[50, 0], [85, 100]], "couple": 99});
        assert!(validate(&bad_param).expect_err("bad").to_string().starts_with("cpu:"));
    }

    #[test]
    fn validate_fills_defaults_and_keeps_given_values() {
        let c = validate(&with(with(ok_curve(), "description", json!("mine")), "predict", json!(7.5)))
            .expect("valid");
        let mut want = Curve::new(vec![p(50.0, 0.0), p(95.0, 100.0)], vec![p(50.0, 0.0), p(85.0, 100.0)]);
        want.description = Some("mine".into());
        want.predict = 7.5;
        assert_eq!(c, want);
        assert_eq!((c.hysteresis, c.down_rate, c.couple, c.battery_shift), (4.0, 1.5, 15.0, 3.0));
    }

    #[test]
    fn native_and_description_are_taken_only_with_the_right_type() {
        let c = validate(&with(with(ok_curve(), "native", json!(true)), "description", json!("d")))
            .expect("valid");
        assert!(c.native);
        assert_eq!(c.description.as_deref(), Some("d"));
        let c = validate(&with(with(ok_curve(), "native", json!("yes")), "description", json!(7)))
            .expect("valid");
        assert!(!c.native);
        assert_eq!(c.description, None);
    }

    #[test]
    fn typed_validation_matches_json_validation() {
        let c = Curve::new(vec![p(95.0, 100.0), p(50.0, 0.0)], vec![p(50.0, 0.0), p(85.0, 100.0)]);
        let v = c.validated().expect("valid");
        assert_eq!(v.cpu, vec![p(50.0, 0.0), p(95.0, 100.0)]);
        let mut bad = c.clone();
        bad.couple = f64::NAN;
        assert!(bad.validated().expect_err("nan").to_string().starts_with("couple:"));
        bad = c;
        bad.cpu[0] = p(f64::NAN, 50.0);
        assert!(bad.validated().is_err());
    }

    #[test]
    fn hostile_input_is_an_error_never_a_panic() {
        let texts: [&[u8]; 14] = [
            b"",
            b"{",
            b"null",
            b"[]",
            b"{}",
            br#"{"cpu":"x"}"#,
            br#"{"cpu":[[1,2,3]],"gpu":[]}"#,
            br#"{"cpu":[[50,0],[95,100]],"gpu":[[50,0],[85,100]],"hysteresis":"4"}"#,
            br#"{"cpu":[[1e999,0],[95,100]],"gpu":[]}"#,
            br#"{"cpu":[[1e308,0],[1e308,0]],"gpu":[[50,0],[85,100]]}"#,
            br#"{"cpu":[[50,0],[95,100]],"gpu":[[50,0],[85,100]],"couple":1e999}"#,
            b"\xff\xfe\x00",
            br#"{"cpu":[[[[[[]]]]]],"gpu":[]}"#,
            br#"{"cpu":[[50,0],[95,100]],"gpu":[[50,0],[85,100]],"hysteresis":18446744073709551616}"#,
        ];
        for t in texts {
            assert!(parse(t).is_err(), "{}", String::from_utf8_lossy(t));
        }
        let deep = "[".repeat(10_000);
        assert!(parse(deep.as_bytes()).is_err());
    }

    #[test]
    fn accepted_curves_satisfy_every_safety_rule_and_round_trip() {
        for n in BUILTIN_NAMES {
            let c = builtin(n);
            let text = serde_json::to_vec(&c).expect("serialise");
            let back = parse(&text).expect("round trip");
            assert_eq!(back, c, "{n}");
            for side in [Side::Cpu, Side::Gpu] {
                assert!(interp(c.points(side), side.full_by_c()) >= 100.0, "{n} {side}");
            }
        }
    }

    // ---- JSON shape ---------------------------------------------------------------------------

    #[test]
    fn json_keys_and_whole_numbers_match_python() {
        let c = builtin("quiet");
        let text = serde_json::to_string(&c).expect("serialise");
        for key in
            ["description", "cpu", "gpu", "hysteresis", "down_rate", "predict", "couple", "battery_shift"]
        {
            assert!(text.contains(&format!("\"{key}\":")), "missing {key} in {text}");
        }
        assert!(!text.contains("native"), "native is omitted for ordinary curves: {text}");
        assert!(text.contains(r#""cpu":[[58,0],[66,30]"#), "{text}");
        assert!(text.contains(r#""hysteresis":4,"#), "{text}");
        assert!(text.contains(r#""predict":3,"#), "{text}");
        let n = serde_json::to_string(&builtin("native")).expect("serialise");
        assert!(n.contains(r#""native":true"#), "{n}");
        let mut f = builtin("truesense");
        f.predict = 7.5;
        f.cpu[1] = p(56.5, 30.0);
        let text = serde_json::to_string(&f).expect("serialise");
        assert!(text.contains(r#""predict":7.5"#) && text.contains("[56.5,30]"), "{text}");
        assert_eq!(builtin("quiet").to_value()["cpu"][0], json!([58, 0]));
    }

    // ---- store --------------------------------------------------------------------------------

    #[test]
    fn default_dir_follows_home() {
        let env = |k: &str| (k == "HOME").then(|| "/home/u".to_owned());
        assert_eq!(Store::default_dir_from(&env), Some(PathBuf::from("/home/u/.config/truesense/curves")));
        assert_eq!(Store::default_dir_from(&|_| None), None);
    }

    #[test]
    fn store_save_load_names_delete() {
        let tmp = TempDir::new();
        let store = Store::new(tmp.0.join("curves"));
        assert_eq!(store.names().expect("names"), BUILTIN_NAMES.map(String::from));
        let mut json = ok_curve();
        json["description"] = json!("mine");
        let saved = store.save("zeta", &json).expect("save");
        store.save("alpha-1.x_y", &ok_curve()).expect("save");
        assert_eq!(store.load("zeta").expect("load"), saved);
        let mut want = BUILTIN_NAMES.map(String::from).to_vec();
        want.extend(["alpha-1.x_y".into(), "zeta".into()]);
        assert_eq!(store.names().expect("names"), want);
        // The file is the Python format: one-space indent, readable back.
        let text = std::fs::read_to_string(tmp.0.join("curves/zeta.json")).expect("read");
        assert!(text.starts_with("{\n \"description\": \"mine\","), "{text}");
        assert!(text.contains("\n \"hysteresis\": 4,"), "{text}");
        store.delete("zeta").expect("delete");
        assert_eq!(store.delete("zeta").expect_err("gone").to_string(), "no fan curve 'zeta'");
        assert_eq!(
            store.load("zeta").expect_err("gone").to_string(),
            "no fan curve 'zeta' (truesense fan curve list)"
        );
        // No temp files are left behind.
        let left: Vec<_> = std::fs::read_dir(tmp.0.join("curves"))
            .expect("dir")
            .map(|e| e.expect("e").file_name())
            .collect();
        assert_eq!(left, vec![std::ffi::OsString::from("alpha-1.x_y.json")]);
    }

    #[test]
    fn store_refuses_builtins_bad_names_and_bad_curves() {
        let tmp = TempDir::new();
        let store = Store::new(&tmp.0);
        assert_eq!(
            store.save("truesense", &ok_curve()).expect_err("builtin").to_string(),
            "'truesense' is built in; save your curve under another name"
        );
        assert_eq!(
            store.delete("quiet").expect_err("builtin").to_string(),
            "'quiet' is built in and cannot be deleted"
        );
        for bad in ["", "a b", "../x", "a/b", "ü", &"x".repeat(41), "a\0b", "x.json/.."] {
            assert_eq!(
                store.save(bad, &ok_curve()).expect_err(bad).to_string(),
                "curve names use letters, digits, . _ - (max 40)",
                "{bad:?}"
            );
            assert!(matches!(store.load(bad), Err(CurveError::BadName)), "{bad:?}");
            assert!(matches!(store.delete(bad), Err(CurveError::BadName)), "{bad:?}");
        }
        assert!(store.save(&"x".repeat(40), &ok_curve()).is_ok());
        // A weak curve is refused and nothing is written.
        let weak = json!({"cpu": [[50, 0], [70, 50]], "gpu": [[50, 0], [78, 100]]});
        assert!(store.save("weak", &weak).is_err());
        assert!(!tmp.0.join("weak.json").exists());
        // Python validated before it checked the name, so a bad curve under a bad name
        // reports the curve.
        assert!(store.save("a b", &weak).expect_err("weak").to_string().starts_with("cpu:"));
    }

    #[test]
    fn store_load_reports_damaged_files() {
        let tmp = TempDir::new();
        let store = Store::new(&tmp.0);
        std::fs::write(tmp.0.join("junk.json"), "{not json").expect("write");
        std::fs::write(tmp.0.join("bin.json"), [0xff, 0xfe, 0x00]).expect("write");
        std::fs::write(tmp.0.join("weak.json"), r#"{"cpu":[[50,0],[70,50]],"gpu":[[50,0],[85,100]]}"#)
            .expect("write");
        std::fs::write(tmp.0.join("list.json"), "[1]").expect("write");
        assert_eq!(store.load("junk").expect_err("junk").to_string(), "fan curve 'junk' is not valid JSON");
        assert_eq!(store.load("bin").expect_err("bin").to_string(), "fan curve 'bin' is not valid JSON");
        assert!(store.load("weak").expect_err("weak").to_string().starts_with("cpu: must reach 100 %"));
        assert!(store.load("list").expect_err("list").to_string().starts_with("a curve is an object"));
        // A huge file is refused, not read.
        let big = vec![b' '; (MAX_SETTINGS_FILE + 10) as usize];
        std::fs::write(tmp.0.join("big.json"), big).expect("write");
        assert!(matches!(store.load("big"), Err(CurveError::Fs(FsError::TooLarge { .. }))));
    }

    #[test]
    fn store_names_skips_unloadable_files_and_a_missing_directory() {
        let tmp = TempDir::new();
        std::fs::write(tmp.0.join("good.json"), "{}").expect("write");
        std::fs::write(tmp.0.join("has space.json"), "{}").expect("write");
        std::fs::write(tmp.0.join("quiet.json"), "{}").expect("write");
        std::fs::write(tmp.0.join("notes.txt"), "x").expect("write");
        std::fs::write(tmp.0.join(".json"), "x").expect("write");
        let names = Store::new(&tmp.0).names().expect("names");
        assert_eq!(&names[5..], ["good"]);
        // A path below a regular file is "not a directory", which is as good as missing.
        assert_eq!(Store::new(tmp.0.join("good.json/sub")).names().expect("names").len(), 5);
        assert_eq!(Store::new(tmp.0.join("absent")).names().expect("names").len(), 5);
    }

    #[test]
    fn load_active_prefers_data_then_name_and_falls_back() {
        let tmp = TempDir::new();
        let store = Store::new(&tmp.0);
        store.save("mine", &ok_curve()).expect("save");
        // Data wins over the name.
        let a = store.load_active(Some(&ok_curve()), "quiet");
        assert!(a.fallback.is_none());
        assert_eq!(a.curve.cpu, vec![p(50.0, 0.0), p(95.0, 100.0)]);
        // No data (null or empty): the name.
        for none in [None, Some(&Value::Null), Some(&json!({}))] {
            assert_eq!(store.load_active(none, "quiet").curve, builtin("quiet"));
        }
        assert_eq!(store.load_active(None, "mine").curve.cpu, vec![p(50.0, 0.0), p(95.0, 100.0)]);
        // "native" is no curve to follow: the truesense curve stands in.
        let n = store.load_active(None, "native");
        assert!(n.fallback.is_none());
        assert_eq!(n.curve, builtin("truesense"));
        // A broken stored curve or a missing name falls back with the reason.
        let weak = json!({"cpu": [[50, 0], [70, 50]], "gpu": [[50, 0], [78, 100]]});
        let a = store.load_active(Some(&weak), "mine");
        assert_eq!(a.curve, builtin("truesense"));
        assert!(a.fallback.expect("reason").to_string().starts_with("cpu: must reach"));
        let a = store.load_active(None, "missing");
        assert_eq!(a.curve, builtin("truesense"));
        assert!(a.fallback.is_some());
    }

    #[test]
    fn errors_have_sources_for_io_only() {
        use std::error::Error;
        assert!(CurveError::BadName.source().is_none());
        let tmp = TempDir::new();
        let e = Store::new(&tmp.0).load("big").expect_err("missing is not found");
        assert!(matches!(e, CurveError::NotFound { .. }));
        std::fs::write(tmp.0.join("big.json"), vec![b' '; (MAX_SETTINGS_FILE + 1) as usize]).expect("write");
        assert!(Store::new(&tmp.0).load("big").expect_err("big").source().is_some());
    }

    // ---- follower -----------------------------------------------------------------------------

    #[test]
    fn follower_rises_at_once_and_falls_slowly() {
        let mut f = Follower::new(builtin("truesense"));
        // 80 C looks up to 70 % and is taken immediately.
        assert_eq!(f.step(Side::Cpu, 80.0, false, 1.0, 0.0), 70.0);
        // 74 C: the hysteresis lookup (70 C = 52 %) is lower than 70 %, so the duty may fall,
        // but only down_rate (1.5 %/s) per second.
        assert_eq!(f.step(Side::Cpu, 74.0, false, 1.0, 0.0), 68.5);
        assert_eq!(f.step(Side::Cpu, 74.0, false, 4.0, 0.0), 62.5);
        // It keeps falling to what the curve asks at 74 C (58 %); one step may dip at most one
        // rate-step below it before the next takes it back up (the hysteresis lookup only
        // bounds a single step's fall, as in Python).
        for _ in 0..100 {
            f.step(Side::Cpu, 74.0, false, 1.0, 0.0);
        }
        let (c, _) = f.duty();
        assert!((58.0 - 1.5..=58.0).contains(&c), "settled at {c}");
    }

    #[test]
    fn follower_fall_is_bounded_by_the_hysteresis_lookup() {
        let mut c = Curve::new(vec![p(40.0, 20.0), p(60.0, 100.0)], vec![p(40.0, 20.0), p(60.0, 100.0)]);
        c.hysteresis = 5.0;
        c.down_rate = 20.0;
        let mut f = Follower::new(c.clone());
        f.step(Side::Cpu, 60.0, false, 1.0, 0.0);
        // 100 -> 80 is the rate limit; the 40 % floor is not reached in one step.
        assert_eq!(f.step(Side::Cpu, 50.0, false, 1.0, 0.0), 80.0);
        f.set_duty(100.0, 0.0);
        // A long step stops at the hysteresis lookup (45 C = 40 %), not at the target (60 %).
        assert_eq!(f.step(Side::Cpu, 50.0, false, 10.0, 0.0), 40.0);
        c.hysteresis = 0.0;
        c.down_rate = 5.0;
        let mut g = Follower::new(c);
        g.set_duty(60.0, 0.0);
        assert_eq!(g.step(Side::Cpu, 50.0, false, 1.0, 0.0), 60.0, "duty equal to the target stays");
    }

    #[test]
    fn follower_predict_and_battery_shift() {
        let mut c = Curve::new(vec![p(40.0, 20.0), p(60.0, 100.0)], vec![p(40.0, 20.0), p(60.0, 100.0)]);
        c.down_rate = 1.0;
        let f = Follower::new(c);
        assert_eq!(f.target(Side::Cpu, 50.0, false, 0.0), 60.0);
        assert_eq!(f.target(Side::Cpu, 50.0, true, 0.0), 80.0, "rising looks 5 C ahead");
        assert_eq!(f.target(Side::Cpu, 50.0, true, f.battery_shift(false)), 68.0, "on battery 3 C lower");
        assert_eq!(f.battery_shift(true), 0.0);
        assert_eq!(f.battery_shift(false), 3.0);
    }

    #[test]
    fn follower_sides_are_independent_and_set_curve_resets() {
        let mut f = Follower::new(builtin("truesense"));
        f.step(Side::Cpu, 89.0, false, 1.0, 0.0);
        assert_eq!(f.duty(), (100.0, 0.0));
        f.set_curve(builtin("quiet"));
        assert_eq!(f.duty(), (0.0, 0.0));
        assert_eq!(f.curve(), &builtin("quiet"));
    }

    #[test]
    fn max_cooling_gives_full_speed_when_warm() {
        // The Python test: select max-cooling, drive 65 C / 50 C; the CPU fan is at 90 % or more.
        let mut f = Follower::new(builtin("max-cooling"));
        f.step(Side::Cpu, 65.0, false, 1.0, 0.0);
        f.step(Side::Gpu, 50.0, false, 1.0, 0.0);
        let (c, g) = f.output(0);
        assert!(c >= 90, "{c}");
        assert!(g >= 20, "{g}");
    }

    #[test]
    fn output_table() {
        // (name, cpu duty, gpu duty, couple, floor, want cpu, want gpu)
        let cases: [(&str, f64, f64, f64, u8, u8, u8); 15] = [
            ("both below 20 is firmware auto", 19.9, 0.0, 15.0, 0, 0, 0),
            ("zero", 0.0, 0.0, 15.0, 0, 0, 0),
            ("quantised to steps of 5", 41.0, 41.0, 15.0, 0, 40, 40),
            ("rounds up", 43.0, 43.0, 15.0, 0, 45, 45),
            ("halves round to even like Python round()", 22.5, 22.5, 15.0, 0, 20, 20),
            ("halves round to even (up case)", 27.5, 27.5, 15.0, 0, 30, 30),
            ("20.0 is the first duty", 20.0, 0.0, 15.0, 0, 20, 20),
            ("quiet fan is lifted to couple below the loud one", 90.0, 0.0, 15.0, 0, 90, 75),
            ("and the other way", 0.0, 60.0, 15.0, 0, 45, 60),
            ("never below the 20 % minimum", 20.0, 0.0, 15.0, 0, 20, 20),
            ("floor lifts both", 0.0, 0.0, 15.0, 35, 35, 35),
            ("floor does not lower", 80.0, 80.0, 15.0, 35, 80, 80),
            ("floor with a loud side", 80.0, 0.0, 15.0, 35, 80, 65),
            ("couple 0 forces equal duties", 90.0, 40.0, 0.0, 0, 90, 90),
            ("couple 50 leaves them apart", 100.0, 20.0, 50.0, 0, 100, 50),
        ];
        for (name, cpu, gpu, couple, floor, want_c, want_g) in cases {
            let mut curve = Curve::new(vec![], vec![]);
            curve.couple = couple;
            let mut f = Follower::new(curve);
            f.set_duty(cpu, gpu);
            let (c, g) = f.output(floor);
            assert_eq!((c, g), (want_c, want_g), "{name}");
            for d in [c, g] {
                assert!(d == 0 || (20..=100).contains(&d), "{name}: duty {d} violates the fan rule");
            }
        }
    }

    #[test]
    fn output_with_a_fractional_couple_stays_a_valid_duty() {
        let mut curve = Curve::new(vec![], vec![]);
        curve.couple = 7.5;
        let mut f = Follower::new(curve);
        f.set_duty(50.0, 100.0);
        assert_eq!(f.output(0), (92, 100), "truncated: 100 - 7.5 = 92.5");
        f.set_duty(250.0, 0.0);
        assert_eq!(f.output(0).0, 100, "never above 100");
    }

    #[test]
    fn follower_is_deterministic() {
        // Two followers with the same inputs agree at every step, and a reset one reproduces
        // the first run: there is no clock and no hidden state.
        let c = builtin("truesense");
        let run = |f: &mut Follower| {
            let mut out = Vec::new();
            for i in 0..400u32 {
                let phase = f64::from(i % 100);
                let (temp, rising) = if phase < 50.0 {
                    (50.0 + 40.0 * phase / 100.0, true)
                } else {
                    (90.0 - 40.0 * (phase - 50.0) / 50.0, false)
                };
                let dt = f64::from(500 + i % 7 * 100) / 1000.0;
                let shift = f.battery_shift(i % 3 != 0);
                f.step(Side::Cpu, temp, rising, dt, shift);
                f.step(Side::Gpu, temp - 6.0, rising, dt, shift);
                out.push(f.output(0));
            }
            out
        };
        let (mut a, mut b) = (Follower::new(c.clone()), Follower::new(c.clone()));
        let ra = run(&mut a);
        assert_eq!(ra, run(&mut b));
        a.set_curve(c);
        assert_eq!(run(&mut a), ra);
        let peak = ra.iter().map(|o| o.0).max().unwrap_or(0);
        let low = ra.iter().map(|o| o.0).min().unwrap_or(100);
        assert!(peak >= 80 && low == 0, "scenario did not cover the range: {peak}..{low}");
        assert!(
            ra.iter()
                .all(|&(c, g)| (c == 0 && g == 0) || ((20..=100).contains(&c) && (20..=100).contains(&g)))
        );
    }

    #[test]
    fn quantise_steps() {
        let cases = [
            (0.0, 0),
            (19.99, 0),
            (20.0, 20),
            (22.4, 20),
            (22.5, 20),
            (22.6, 25),
            (27.5, 30),
            (97.5, 100),
            (100.0, 100),
            (130.0, 100),
        ];
        for (d, want) in cases {
            assert_eq!(quantise(d), want, "{d}");
        }
    }
}
