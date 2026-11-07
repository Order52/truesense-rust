//! Fans: firmware auto, max, manual, the smart curve and the curves it follows.
//!
//! # The rule for stopping a fan
//!
//! A duty is 0 or 20..=100 percent. A duty of 0 stops the fan, and a stopped fan can let the laptop
//! overheat, so it only ever goes through the service (its watchdog gives all fans back to the
//! firmware when the machine gets warm), only with the explicit confirmation, and never on a
//! fallback path. Every path of this module that could write a 0 checks both, *before* anything is
//! sent: [`Refusal::StopNeedsService`] and [`Refusal::StopNeedsConfirmation`].
#![forbid(unsafe_code)]

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use ts_decision::curves::{self, Curve};
use ts_ipc::wire::Args;
use ts_platform::hw::{FanDuty, FanState};

use super::{Api, ApiError, ApiResult, NamedList, Refusal};

/// What the fans are asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanRequest {
    /// The service's smart curve.
    Smart,
    /// The firmware's own control (they stop when cool).
    Auto,
    /// Both fans at full speed until the smart curve (or Auto) takes over again.
    Max,
    /// Both fans stopped. Needs the confirmation and the service.
    Off,
    /// A fixed duty per fan: each 20..=100, or 0 to stop that fan (confirmation and service). A
    /// `gpu` of `None` uses the CPU's duty for both.
    Manual {
        /// CPU fan duty.
        cpu: i64,
        /// GPU fan duty.
        gpu: Option<i64>,
    },
}

impl FanRequest {
    /// The two duties of a manual request, checked: each 0 or 20..=100.
    fn duties(cpu: i64, gpu: Option<i64>) -> Result<(FanDuty, FanDuty), Refusal> {
        let both = |c: i64, g: i64| {
            Ok((
                FanDuty::new(c).map_err(|_| Refusal::FanDuty)?,
                FanDuty::new(g).map_err(|_| Refusal::FanDuty)?,
            ))
        };
        both(cpu, gpu.unwrap_or(cpu))
    }
}

/// How a fan request reached the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FanRoute {
    /// Through the service, in one request.
    Daemon,
    /// Straight to the firmware (no service running).
    Hardware,
    /// A service too old to know `fan`: its smart curve was stopped, one tick waited, then the
    /// firmware written.
    LegacyDaemon,
}

/// What a fan request did. It serialises as its message, which is what the Python edition printed
/// for `--json` (these commands had no value to report); the route is for the callers that care.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FanOutcome {
    /// What was asked.
    pub request: FanRequest,
    /// Which way it was done.
    pub route: FanRoute,
}

impl Serialize for FanOutcome {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// The note added to the answer when a fan was stopped. A manual duty of 0 is not silence on the
/// PH16-71: the firmware keeps a fan at its minimum (about 1700 rpm). Only the firmware's own
/// Auto curve reaches 0 rpm, and only when the laptop is cool.
const STOP_NOTE: &str = "   (a stopped fan: all fans return to Auto at CPU 70 C / GPU 65 C; `truesense fan auto` now. The firmware may keep the fan at its minimum speed: check `truesense fan status`)";

impl fmt::Display for FanOutcome {
    /// The line the command prints.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.request {
            FanRequest::Auto => f.write_str("fans: firmware auto (they stop when cool)"),
            FanRequest::Max => f.write_str("fans: MAX  (`truesense fan smart on` hands them back)"),
            FanRequest::Smart => f.write_str("fans: smart curve"),
            FanRequest::Off => f.write_str(
                "fans: OFF   (back on Auto automatically at CPU 70 C / GPU 65 C; `truesense fan auto` now; the firmware may keep the fans at their minimum speed: check `truesense fan status`)",
            ),
            FanRequest::Manual { cpu, gpu } => {
                let gpu = gpu.unwrap_or(cpu);
                write!(f, "fans: manual {cpu} % / {gpu} %")?;
                if cpu == 0 || gpu == 0 {
                    f.write_str(STOP_NOTE)?;
                }
                Ok(())
            }
        }
    }
}

/// The fans as the firmware reports them (behaviour and duty registers), their speed, and whether
/// the service's smart curve is in control (`null` without a service).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FanStatus {
    /// Behaviour word and duty registers.
    #[serde(flatten)]
    pub state: FanState,
    /// CPU fan speed.
    pub cpu_fan_rpm: i64,
    /// GPU fan speed.
    pub gpu_fan_rpm: i64,
    /// The smart curve is in control; `None` without a service.
    pub smart: Option<bool>,
}

impl fmt::Display for FanStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "fans: {}   CPU {} rpm   GPU {} rpm   smart curve: {}",
            self.state.mode,
            self.cpu_fan_rpm,
            self.gpu_fan_rpm,
            super::on_off_opt(self.smart)
        )
    }
}

/// The service's answer to which fan curve runs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FanCurveInUse {
    /// The curve's name.
    pub name: String,
    /// The curve.
    pub curve: Curve,
    /// Every curve name the service knows.
    pub names: Vec<String>,
}

impl fmt::Display for FanCurveInUse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "fan curve in use: {}\n  cpu {:?}\n  gpu {:?}",
            self.name,
            point_list(&self.curve, true),
            point_list(&self.curve, false)
        )
    }
}

/// A curve as `fan curve show` prints it: both point lists and the follower parameters.
pub fn describe_curve(name: &str, curve: &Curve) -> String {
    format!(
        "{name}\n  cpu {:?}\n  gpu {:?}\n  hysteresis={}  down_rate={}  predict={}  couple={}  battery_shift={}",
        point_list(curve, true),
        point_list(curve, false),
        curve.hysteresis,
        curve.down_rate,
        curve.predict,
        curve.couple,
        curve.battery_shift
    )
}

fn point_list(curve: &Curve, cpu: bool) -> Vec<(f64, f64)> {
    let points = if cpu { &curve.cpu } else { &curve.gpu };
    points.iter().map(|p| (p.temp, p.duty)).collect()
}

/// What `fan curve save` changes: points as `52:0,60:35,...`, the follower parameters when given,
/// and the curve to start from.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CurveEdit {
    /// CPU points, `temperature:percent` pairs (0 = firmware auto).
    pub cpu: Option<String>,
    /// GPU points.
    pub gpu: Option<String>,
    /// Degrees the temperature must fall below a point before the duty follows it down.
    pub hysteresis: Option<f64>,
    /// The fastest the duty may fall, percent per second.
    pub down_rate: Option<f64>,
    /// Degrees added to a rising temperature before the lookup.
    pub predict: Option<f64>,
    /// How far the quieter fan may trail the louder one.
    pub couple: Option<f64>,
    /// Degrees subtracted on battery.
    pub battery_shift: Option<f64>,
    /// The curve to start from; else your own curve of the same name, else nothing.
    pub from: Option<String>,
}

impl Api {
    /// Sets the fans. Through the service in one request when it runs; on the firmware when it does
    /// not, except for what only the service can do (the smart curve, stopping a fan).
    ///
    /// `confirm` is the explicit confirmation a stop needs; it is ignored for everything else.
    pub fn set_fans(&self, request: FanRequest, confirm: bool) -> ApiResult<FanOutcome> {
        let routed = |route| FanOutcome { request, route };
        let daemon = self.daemon_available();
        match request {
            FanRequest::Off => {
                if !daemon {
                    return Err(Refusal::OffNeedsService.into());
                }
                if !confirm {
                    return Err(Refusal::StopNeedsConfirmation.into());
                }
                self.ask_value("fan", Args::new().with("mode", "off").with("confirm", true))?;
                return Ok(routed(FanRoute::Daemon));
            }
            FanRequest::Manual { cpu, gpu } => {
                let (c, g) = FanRequest::duties(cpu, gpu)?;
                if c.is_stop() || g.is_stop() {
                    if !daemon {
                        return Err(Refusal::StopNeedsService.into());
                    }
                    if !confirm {
                        return Err(Refusal::StopNeedsConfirmation.into());
                    }
                    let args = Args::new()
                        .with("mode", "manual")
                        .with("cpu", cpu)
                        .with("gpu", gpu)
                        .with("confirm", true);
                    self.ask_value("fan", args)?;
                    return Ok(routed(FanRoute::Daemon));
                }
            }
            FanRequest::Smart | FanRequest::Auto | FanRequest::Max => {}
        }
        if !daemon {
            self.fan_direct(request)?;
            return Ok(routed(FanRoute::Hardware));
        }
        match self.s.daemon.call("fan", fan_args(request)) {
            Ok(_) => Ok(routed(FanRoute::Daemon)),
            Err(e) if e.is_unknown_command() => {
                self.fan_legacy(request)?;
                Ok(routed(FanRoute::LegacyDaemon))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// The firmware path, for what is safe without the service. A stop never gets here.
    fn fan_direct(&self, request: FanRequest) -> ApiResult<()> {
        match request {
            FanRequest::Auto => Ok(self.s.hw.fan_auto()?),
            FanRequest::Max => Ok(self.s.hw.fan_max()?),
            FanRequest::Manual { cpu, gpu } => {
                let (c, g) = FanRequest::duties(cpu, gpu)?;
                if c.is_stop() || g.is_stop() {
                    return Err(Refusal::StopNeedsService.into());
                }
                Ok(self.s.hw.fan_manual(c, g)?)
            }
            FanRequest::Smart => Err(Refusal::SmartNeedsService.into()),
            FanRequest::Off => Err(Refusal::OffNeedsService.into()),
        }
    }

    /// A service too old to know `fan`: stop its smart curve, wait one tick, then write the firmware.
    fn fan_legacy(&self, request: FanRequest) -> ApiResult<()> {
        if request == FanRequest::Smart {
            self.daemon_set("smart_fan", true)?;
            return Ok(());
        }
        if self.daemon_set("smart_fan", false).is_ok() {
            (self.s.sleep)(Duration::from_millis(1200));
        }
        self.fan_direct(request)
    }

    /// Hands the fans to the firmware (they stop when cool).
    pub fn fan_auto(&self) -> ApiResult<FanOutcome> {
        self.set_fans(FanRequest::Auto, false)
    }

    /// Runs both fans at full speed.
    pub fn fan_max(&self) -> ApiResult<FanOutcome> {
        self.set_fans(FanRequest::Max, false)
    }

    /// Hands the fans to the service's smart curve.
    pub fn fan_smart(&self) -> ApiResult<FanOutcome> {
        self.set_fans(FanRequest::Smart, false)
    }

    /// Stops both fans: needs the service and `confirm`.
    pub fn fan_off(&self, confirm: bool) -> ApiResult<FanOutcome> {
        self.set_fans(FanRequest::Off, confirm)
    }

    /// Sets the duty of each fan: 20..=100, or 0 to stop that fan (service and `confirm`). A `gpu`
    /// of `None` uses the CPU's duty for both.
    pub fn fan_set(&self, cpu: i64, gpu: Option<i64>, confirm: bool) -> ApiResult<FanOutcome> {
        self.set_fans(FanRequest::Manual { cpu, gpu }, confirm)
    }

    /// Reads the fans.
    pub fn fan_status(&self) -> ApiResult<FanStatus> {
        let state = self.s.hw.fan_state()?;
        let sensors = self.s.hw.sensors()?;
        let smart = self.daemon_state().ok().map(|d| d.setting_bool("smart_fan").unwrap_or(false));
        Ok(FanStatus { state, cpu_fan_rpm: sensors.cpu_fan_rpm, gpu_fan_rpm: sensors.gpu_fan_rpm, smart })
    }

    /// Whether the service's smart curve is in control.
    pub fn smart_fan(&self) -> ApiResult<bool> {
        self.daemon_bool("smart_fan")
    }

    /// Switches the service's smart curve and answers the value it holds. Switching it off gives
    /// the fans back to the firmware's own control.
    pub fn set_smart_fan(&self, on: bool) -> ApiResult<bool> {
        self.daemon_set_bool("smart_fan", on)
    }

    /// Lists the fan curves and, when the service runs, the one in use.
    pub fn fan_curves(&self) -> ApiResult<NamedList> {
        let names = self.s.curves.names()?;
        let mut list = NamedList { names, ..NamedList::default() };
        for n in &list.names {
            if let Some(d) = Curve::builtin(n).and_then(|c| c.description) {
                list.descriptions.insert(n.clone(), d);
            }
        }
        if self.daemon_available() {
            list.current = Some(self.fan_curve_in_use()?.name);
        }
        Ok(list)
    }

    /// Asks the service which curve runs (it keeps it, so it works before login).
    pub fn fan_curve_in_use(&self) -> ApiResult<FanCurveInUse> {
        let answer: Value = self.ask("fan_curve", Args::new().with("action", "get"))?;
        let name = answer.get("name").and_then(Value::as_str).unwrap_or_default().to_owned();
        let curve = curves::validate(answer.get("curve").unwrap_or(&Value::Null))?;
        let names = answer
            .get("names")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|n| n.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        Ok(FanCurveInUse { name, curve, names })
    }

    /// Reads one curve, built-in or yours.
    pub fn fan_curve(&self, name: &str) -> ApiResult<Curve> {
        Ok(self.s.curves.load(name)?)
    }

    /// Makes a curve the one the service follows. A saved curve travels with the request as data,
    /// so the service (root) does not depend on files in a home directory.
    pub fn use_fan_curve(&self, name: &str) -> ApiResult<String> {
        if name.is_empty() {
            return Err(ApiError::invalid("which curve? (truesense fan curve list)"));
        }
        let data = if curves::is_builtin(name) { Value::Null } else { self.s.curves.load(name)?.to_value() };
        self.use_curve_request(name, data)
    }

    /// Makes a curve that is not saved (the window's editor) the one the service follows; `name`
    /// is only a label. The curve is checked like the service checks it: one that is too cool for
    /// the load is refused.
    pub fn use_fan_curve_data(&self, name: &str, curve: &Curve) -> ApiResult<String> {
        let checked = curve.validated()?;
        let label = if name.is_empty() { "custom" } else { name };
        self.use_curve_request(label, checked.to_value())
    }

    fn use_curve_request(&self, name: &str, data: Value) -> ApiResult<String> {
        let args = Args::new().with("action", "use").with("name", name).with("data", data);
        let answer: Value = self.ask("fan_curve", args)?;
        Ok(answer.get("name").and_then(Value::as_str).unwrap_or(name).to_owned())
    }

    /// Saves a curve: starts from `from` (or from your own curve of that name) and changes only
    /// what the edit gives. A curve that is too cool for the load is refused.
    pub fn save_fan_curve(&self, name: &str, edit: &CurveEdit) -> ApiResult<Curve> {
        if name.is_empty() {
            return Err(ApiError::invalid("save under which name?"));
        }
        let mut curve = match edit.from.as_deref().filter(|f| !f.is_empty()) {
            Some(from) => self.s.curves.load(from)?,
            None if !curves::is_builtin(name) && self.s.curves.names()?.iter().any(|n| n == name) => {
                self.s.curves.load(name)?
            }
            None => Curve::new(Vec::new(), Vec::new()),
        };
        curve.description = None;
        if let Some(text) = edit.cpu.as_deref().filter(|t| !t.is_empty()) {
            curve.cpu = curves::parse_points(text)?;
        }
        if let Some(text) = edit.gpu.as_deref().filter(|t| !t.is_empty()) {
            curve.gpu = curves::parse_points(text)?;
        }
        for (value, slot) in [
            (edit.hysteresis, &mut curve.hysteresis),
            (edit.down_rate, &mut curve.down_rate),
            (edit.predict, &mut curve.predict),
            (edit.couple, &mut curve.couple),
            (edit.battery_shift, &mut curve.battery_shift),
        ] {
            if let Some(v) = value {
                *slot = v;
            }
        }
        if curve.cpu.is_empty() || curve.gpu.is_empty() {
            return Err(ApiError::invalid("give --cpu and --gpu points (or --from an existing curve)"));
        }
        let saved = self.s.curves.save(name, &curve.to_value())?;
        self.refresh_curve_bindings(name);
        Ok(saved)
    }

    /// Saves a whole curve as it was edited (built-in names are refused).
    pub fn save_fan_curve_value(&self, name: &str, curve: &Curve) -> ApiResult<Curve> {
        if name.is_empty() {
            return Err(ApiError::invalid("save under which name?"));
        }
        let mut curve = curve.clone();
        curve.description = None;
        let saved = self.s.curves.save(name, &curve.to_value())?;
        self.refresh_curve_bindings(name);
        Ok(saved)
    }

    /// A saved curve that a mode binding names must reach the service again, or its copy is
    /// stale. The save already succeeded, so a failure here is not the caller's.
    fn refresh_curve_bindings(&self, name: &str) {
        let _ = self.refresh_binding_copies(ts_decision::bindings::Target::FanCurve, name);
    }

    /// Saves a copy under `new_name` (`NAME-copy` when none) and answers the name.
    pub fn copy_fan_curve(&self, name: &str, new_name: Option<&str>) -> ApiResult<String> {
        if name.is_empty() {
            return Err(ApiError::invalid("copy which curve? (truesense fan curve list)"));
        }
        let new = new_name.filter(|n| !n.is_empty()).map_or_else(|| format!("{name}-copy"), str::to_owned);
        let curve = self.s.curves.load(name)?;
        self.s.curves.save(&new, &curve.to_value())?;
        Ok(new)
    }

    /// Removes one of your curves (built-in ones are read-only).
    pub fn delete_fan_curve(&self, name: &str) -> ApiResult<()> {
        Ok(self.s.curves.delete(name)?)
    }

    /// Whether a curve is read-only.
    pub fn fan_curve_is_builtin(name: &str) -> bool {
        curves::is_builtin(name)
    }
}

/// The request as the daemon's `fan` command takes it (`cpu` and `gpu` are null when not given).
fn fan_args(request: FanRequest) -> Args {
    let (mode, cpu, gpu) = match request {
        FanRequest::Smart => ("smart", None, None),
        FanRequest::Auto => ("auto", None, None),
        FanRequest::Max => ("max", None, None),
        FanRequest::Off => ("off", None, None),
        FanRequest::Manual { cpu, gpu } => ("manual", Some(cpu), gpu),
    };
    Args::new().with("mode", mode).with("cpu", cpu).with("gpu", gpu)
}

impl FromStr for FanRequest {
    type Err = ApiError;

    /// `smart`, `auto`, `max` or `off`; a manual duty has its own arguments.
    fn from_str(text: &str) -> Result<Self, ApiError> {
        match text {
            "smart" => Ok(Self::Smart),
            "auto" => Ok(Self::Auto),
            "max" => Ok(Self::Max),
            "off" => Ok(Self::Off),
            other => Err(ApiError::invalid(format!("fan mode: smart | auto | max | off (got '{other}')"))),
        }
    }
}
