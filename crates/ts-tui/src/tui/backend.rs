//! Everything the screen reads from and writes to the laptop, in one place.
//!
//! The screen itself (model, update, view) never sees `ts_api`: it speaks in the small types of
//! `data` and in [`Op`]s, and a [`Backend`] turns them into calls of the application layer. That is
//! what lets the tests run the real screen against the simulated laptop, and a stress test count
//! every write that would have reached a fan.
//!
//! Safety rules live in the application layer (`Api::set_fans` refuses a stop without the service
//! and the confirmation); the backend repeats the confirmation check so that a bug in the screen's
//! confirm path cannot reach the layer below even in a build that has the service running.

use std::fmt::Display;
use std::str::FromStr;
use std::sync::Arc;

use ts_api::api::{Api, AutoMode, ChargeFullAction, FavouriteKind, KeyboardLook, RearRequest, dts_is_on};
use ts_domain::audio::{Output, VoiceDevice, VoiceMode};
use ts_domain::keyboard::{MAX_BRIGHT, SPEED_MAX, SPEED_MIN};
use ts_system::doctor::Status as DoctorStatus;

use super::data::{
    Battery, DoctorRow, DtsOutput, FanMode, FanState, Fast, Grade, KbLook, Microphone, Names, ReadKey,
    RearLook, Sensors, ServiceState, SlowReading, SlowValue, SoundCard, VoiceModes,
};

/// Starts the graphical window as a detached process. The binary provides it (it knows its own
/// path and how to detach); the screen only asks.
pub type WindowOpener = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

/// What a fan request asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FanAction {
    Smart,
    Auto,
    Max,
    Manual,
    Off,
}

/// A fan request. A stop (`Off`, or a manual duty of 0) must carry `confirm`, which the screen sets
/// only after a Yes; the backend refuses it otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FanReq {
    pub action: FanAction,
    pub cpu: i64,
    pub gpu: i64,
    pub confirm: bool,
}

impl FanReq {
    pub(crate) fn mode(action: FanAction) -> Self {
        Self { action, cpu: 0, gpu: 0, confirm: false }
    }

    /// Whether this request stops a fan.
    pub(crate) fn is_stop(&self) -> bool {
        match self.action {
            FanAction::Off => true,
            FanAction::Manual => self.cpu == 0 || self.gpu == 0,
            _ => false,
        }
    }
}

/// Which input a noise-reduction mode is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VoiceInput {
    Headset,
    Array,
}

/// A hardware write. One value per setting, so the queue can replace an unstarted write by a newer
/// one of the same key.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Op {
    PowerMode { mode: String, hold_minutes: f64 },
    AutoMode(String),
    ReleaseHold,
    SmartProfile(String),
    Fan(FanReq),
    Curve(String),
    BatteryLimit(bool),
    ChargeFull { start: bool },
    Calibration(bool),
    UsbCharge(i64),
    GpuMode(String),
    Overdrive(bool),
    BootSound(bool),
    KbTimeout(bool),
    Keyboard(KbLook),
    SavedLighting(String),
    Rear { look: RearLook, keep_zones: bool },
    RepairSound,
    Output(String),
    SpeakerFilter(bool),
    Dts(bool),
    DtsMode(String),
    DtsEverywhere,
    MicMute(bool),
    MicVolume(i64),
    Voice { input: VoiceInput, mode: String },
    Profile(String),
    Look(String),
    Favourite(FavouriteKind, String),
    OpenWindow,
    RunDoctor,
}

impl Op {
    /// Whether this write can stop a fan.
    #[cfg(test)]
    pub(crate) fn stops_a_fan(&self) -> bool {
        matches!(self, Op::Fan(req) if req.is_stop())
    }
}

/// What a write answers, for the screen to say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Done {
    Unit,
    Text(String),
    Repair { fixed: bool, changed: bool, steps: Vec<String> },
    Doctor(Vec<DoctorRow>),
}

/// What is listed on the pages and cannot change while the screen runs: the names the pages offer.
#[derive(Debug, Clone, Default)]
pub(crate) struct Catalog {
    pub effects: Vec<String>,
    pub rear_modes: Vec<String>,
    pub max_brightness: i64,
    pub min_speed: i64,
    pub max_speed: i64,
    pub curves: Vec<String>,
    pub smart_profiles: Vec<String>,
    /// Profile names with the line that says what each changes.
    pub profiles: Vec<(String, String)>,
    pub looks: Vec<String>,
    /// The starred items in the user's order, with whether each still exists. Read when the screen
    /// opens, like the other lists: a star set while it runs shows at the next start, and applying
    /// goes by kind and name, so a list that moved cannot apply the wrong item.
    pub favourites: Vec<(FavouriteKind, String, bool)>,
    pub doctor_checks: Vec<String>,
}

/// The laptop as the screen sees it. Reads are cheap enough to call from a background thread;
/// `execute` may take a second (the sound switch, the health check).
pub(crate) trait Backend: Send + Sync {
    /// The names the pages list (local files only: no hardware, no service).
    fn catalog(&self) -> Catalog;
    /// The dashboard's reading.
    fn fast(&self) -> Fast;
    /// One setting of the open page.
    fn read(&self, key: ReadKey) -> SlowReading;
    /// Carries out a write.
    fn execute(&self, op: &Op) -> Result<Done, String>;
}

/// [`Backend`] over the application layer.
pub(crate) struct ApiBackend {
    api: Arc<Api>,
    window: Option<WindowOpener>,
}

impl ApiBackend {
    pub(crate) fn new(api: Arc<Api>, window: Option<WindowOpener>) -> Self {
        Self { api, window }
    }

    fn service_state(&self) -> Option<ServiceState> {
        if !self.api.daemon_available() {
            return None;
        }
        // An unreachable service is the same as none: the screen reads the hardware itself.
        let st = self.api.daemon_state().ok()?;
        let num = |k: &str| st.live_f64(k).map(|v| v.round() as i64);
        let text = |k: &str| st.live_str(k).unwrap_or_default().to_owned();
        let setting = |k: &str| st.setting_str(k).unwrap_or_default().to_owned();
        let or = |a: String, b: String| if a.is_empty() { b } else { a };
        // The service already read these this second when `rpm1` and `tcpu` are there.
        let sensors = (num("rpm1").is_some() && num("tcpu").is_some()).then(|| Sensors {
            cpu_temp: num("tcpu").unwrap_or(0),
            sys_temp: num("chassis").unwrap_or(0),
            gpu_temp: num("tgpu").unwrap_or(0),
            cpu_fan_rpm: num("rpm1").unwrap_or(0),
            gpu_fan_rpm: num("rpm2").unwrap_or(0),
        });
        Some(ServiceState {
            sensors,
            power_mode: Some(text("power_mode")).filter(|m| m != "?").unwrap_or_default(),
            ac: st.live_bool("ac"),
            auto_mode: or(text("auto_mode"), setting("auto_mode")),
            smart_profile: or(text("smart_profile"), setting("smart_profile")),
            fan_off: st.live_bool("fan_off").unwrap_or(false),
            smart_fan: st.setting_bool("smart_fan").unwrap_or(false),
            fan_curve: or(setting("fan_curve"), text("fan_curve")),
            cpu_load: num("cpu"),
            gpu_load: num("gpu"),
            gpu_watts: num("gpuw"),
            activity: text("activity"),
            mode_reason: text("mode_reason"),
            hold_left_s: st.hold_left_s,
        })
    }
}

/// Parses a word into the application layer's own type, whichever it is (the call decides).
fn parsed<T: FromStr>(text: &str) -> Result<T, String>
where
    T::Err: Display,
{
    text.parse::<T>().map_err(|e| e.to_string())
}

fn err<E: Display>(e: E) -> String {
    let text = e.to_string();
    if text.is_empty() { "error".to_owned() } else { text }
}

fn grade(status: DoctorStatus) -> Grade {
    match status {
        DoctorStatus::Ok | DoctorStatus::Info => Grade::Ok,
        DoctorStatus::Warn => Grade::Warn,
        DoctorStatus::Fail => Grade::Fail,
    }
}

impl Backend for ApiBackend {
    fn catalog(&self) -> Catalog {
        let s = self.api.services();
        Catalog {
            effects: Api::keyboard_effects().into_iter().map(str::to_owned).collect(),
            rear_modes: Api::rear_effects().into_iter().map(str::to_owned).collect(),
            max_brightness: MAX_BRIGHT,
            min_speed: SPEED_MIN,
            max_speed: SPEED_MAX,
            curves: s.curves.names().unwrap_or_default(),
            smart_profiles: s.smart.names(),
            profiles: self
                .api
                .profile_names()
                .into_iter()
                .map(|n| {
                    let line = self.api.describe_profile(&n);
                    (n, line)
                })
                .collect(),
            looks: self.api.looks(),
            favourites: self
                .api
                .favourites()
                .into_iter()
                .filter_map(|f| Some((FavouriteKind::parse(f.kind)?, f.name, f.exists)))
                .collect(),
            doctor_checks: Api::doctor_checks().into_iter().map(str::to_owned).collect(),
        }
    }

    fn fast(&self) -> Fast {
        let service = self.service_state();
        let svc_sensors = service.as_ref().and_then(|s| s.sensors);
        let sensors = match svc_sensors {
            Some(s) => Ok(s),
            None => self
                .api
                .sensors()
                .map(|s| Sensors {
                    cpu_temp: s.cpu_temp,
                    sys_temp: s.sys_temp,
                    gpu_temp: s.gpu_temp,
                    cpu_fan_rpm: s.cpu_fan_rpm,
                    gpu_fan_rpm: s.gpu_fan_rpm,
                })
                .map_err(err),
        };
        let hw = &self.api.services().hw;
        let fan = hw
            .fan_state()
            .map(|f| FanState {
                mode: FanMode::from_name(&f.mode.to_string()),
                cpu_duty: f.duty_register_pct.cpu,
                gpu_duty: f.duty_register_pct.gpu,
            })
            .map_err(err);
        let mode = match service
            .as_ref()
            .map(|s| s.power_mode.as_str())
            .filter(|m| super::data::Mode::from_name(m).is_some())
        {
            Some(m) => Ok(m.to_owned()),
            None => hw.get_mode().map(|m| m.to_string()).map_err(err),
        };
        let ac = match service.as_ref().and_then(|s| s.ac) {
            Some(a) => Ok(a),
            None => Ok(self.api.services().linux.on_ac()),
        };
        let battery = self.api.supply().map(|b| Battery { percent: b.percent });
        Fast { service, sensors, fan, mode, ac, battery }
    }

    fn read(&self, key: ReadKey) -> SlowReading {
        let api = &*self.api;
        let value: Result<SlowValue, String> = match key {
            ReadKey::Curves => api
                .fan_curves()
                .map(|l| SlowValue::Names(Names { names: l.names, current: l.current }))
                .map_err(err),
            ReadKey::SmartProfiles => api
                .smart_profiles()
                .map(|l| SlowValue::Names(Names { names: l.names, current: l.current }))
                .map_err(err),
            ReadKey::Limit => api.battery_limit().map(SlowValue::Bool).map_err(err),
            ReadKey::Calibration => api.calibration().map(SlowValue::Bool).map_err(err),
            ReadKey::UsbCharge => api.usb_charge().map(SlowValue::Int).map_err(err),
            ReadKey::GpuMode => api.gpu_mode().map(|m| SlowValue::Text(m.to_string())).map_err(err),
            ReadKey::Overdrive => api.overdrive().map(SlowValue::MaybeBool).map_err(err),
            ReadKey::BootSound => api.boot_sound().map(SlowValue::Bool).map_err(err),
            ReadKey::KbTimeout => api.keyboard_timeout().map(SlowValue::MaybeBool).map_err(err),
            ReadKey::ChargeFull => {
                api.charge_full(ChargeFullAction::Status).map(|c| SlowValue::Bool(c.active)).map_err(err)
            }
            ReadKey::KbLast => {
                let l = api.keyboard_last();
                Ok(SlowValue::KbLast(KbLook {
                    name: l.name,
                    color: l.color.unwrap_or_default(),
                    speed: l.speed.unwrap_or(0),
                    brightness: l.brightness,
                    direction: l.direction,
                }))
            }
            ReadKey::KbNow => api.keyboard_brightness().map(|b| SlowValue::Int(i64::from(b))).map_err(err),
            ReadKey::Rear => api
                .rear()
                .map(|r| {
                    SlowValue::Rear(RearLook {
                        mode: r.mode.to_string(),
                        color: r.color.to_string(),
                        speed: i64::from(r.speed),
                        brightness: i64::from(r.brightness),
                        direction: r.direction.map_or(1, i64::from),
                    })
                })
                .map_err(err),
            ReadKey::SavedLightings => Ok(SlowValue::List(api.lighting_profile_names())),
            ReadKey::Sound => api
                .sound_state()
                .map(|s| {
                    SlowValue::Sound(match &s {
                        ts_domain::audio::SoundState::NoCard { .. } => SoundCard::Missing,
                        ts_domain::audio::SoundState::Ok { .. } => SoundCard::Ok,
                        ts_domain::audio::SoundState::Broken { problem, .. } => {
                            SoundCard::Broken(problem.clone())
                        }
                    })
                })
                .map_err(err),
            ReadKey::Output => api.audio_output().map(|o| SlowValue::Text(o.active)).map_err(err),
            ReadKey::SpeakerFilter => Ok(SlowValue::Bool(api.speaker_filter())),
            ReadKey::Mic => {
                let m = api.mic();
                Ok(SlowValue::Mic(Microphone { muted: m.muted, volume: m.volume.map(i64::from) }))
            }
            ReadKey::Voice => {
                let m = api.voice_modes();
                Ok(SlowValue::Voice(VoiceModes {
                    headset: m.headset.map(|v| v.as_str().to_owned()),
                    array: m.array.map(|v| v.as_str().to_owned()),
                }))
            }
            ReadKey::DtsOutputs => api
                .dts_devices()
                .map(|d| {
                    SlowValue::Dts(
                        d.into_iter()
                            .map(|o| DtsOutput { label: o.label, profile: o.profile, current: o.current })
                            .collect(),
                    )
                })
                .map_err(err),
            ReadKey::DtsOn => api.dts_status().map(|s| SlowValue::Bool(dts_is_on(&s))).map_err(err),
            ReadKey::ModuleVersion => api.module_version().map(SlowValue::Text).map_err(err),
            ReadKey::Doctor => Err("the health check is run on request".to_owned()),
        };
        SlowReading { key, value }
    }

    fn execute(&self, op: &Op) -> Result<Done, String> {
        let api = &*self.api;
        let unit = |r: Result<(), ts_api::api::ApiError>| r.map(|()| Done::Unit).map_err(err);
        match op {
            Op::PowerMode { mode, hold_minutes } => {
                unit(api.set_power(parsed(mode)?, Some(*hold_minutes)).map(drop))
            }
            Op::AutoMode(m) => unit(api.set_auto_mode(parsed::<AutoMode>(m)?)),
            Op::ReleaseHold => unit(api.release_hold().map(drop)),
            Op::SmartProfile(n) => unit(api.use_smart_profile(n).map(drop)),
            Op::Fan(req) => {
                if req.is_stop() && !req.confirm {
                    return Err("stopping a fan needs explicit confirmation".to_owned());
                }
                let done = match req.action {
                    FanAction::Smart => api.fan_smart(),
                    FanAction::Auto => api.fan_auto(),
                    FanAction::Max => api.fan_max(),
                    FanAction::Off => api.fan_off(req.confirm),
                    FanAction::Manual => api.fan_set(req.cpu, Some(req.gpu), req.confirm),
                };
                unit(done.map(drop))
            }
            Op::Curve(n) => unit(api.use_fan_curve(n).map(drop)),
            Op::BatteryLimit(on) => unit(api.set_battery_limit(*on).map(drop)),
            Op::ChargeFull { start } => {
                let action = if *start { ChargeFullAction::Start } else { ChargeFullAction::Cancel };
                unit(api.charge_full(action).map(drop))
            }
            Op::Calibration(on) => unit(api.set_calibration(*on).map(drop)),
            Op::UsbCharge(n) => unit(api.set_usb_charge(*n).map(drop)),
            Op::GpuMode(m) => unit(api.set_gpu_mode(Api::parse_gpu_mode(m).map_err(err)?).map(drop)),
            Op::Overdrive(on) => unit(api.set_overdrive(*on).map(drop)),
            Op::BootSound(on) => unit(api.set_boot_sound(*on).map(drop)),
            Op::KbTimeout(on) => unit(api.set_keyboard_timeout(*on).map(drop)),
            Op::Keyboard(look) => {
                let off = look.name == "off";
                unit(api.apply_keyboard_look(&KeyboardLook {
                    name: look.name.clone(),
                    color: (!off && !look.color.is_empty()).then(|| look.color.clone()),
                    speed: (!off && look.speed > 0).then_some(look.speed),
                    brightness: look.brightness,
                    direction: look.direction,
                }))
            }
            Op::SavedLighting(n) => unit(api.apply_lighting_profile(n)),
            Op::Rear { look, keep_zones } => {
                let request = RearRequest::parse(
                    &look.mode,
                    &look.color,
                    look.speed,
                    look.brightness,
                    look.direction,
                    None,
                )
                .map_err(err)?;
                unit(api.set_rear_live(&request, *keep_zones).map(drop))
            }
            Op::RepairSound => api
                .repair_sound()
                .map(|r| Done::Repair { fixed: r.fixed, changed: r.changed, steps: r.steps })
                .map_err(err),
            Op::Output(o) => {
                let which = match o.as_str() {
                    "speakers" => Output::Speakers,
                    "headphones" => Output::Headphones,
                    other => return Err(format!("output: speakers | headphones (got '{other}')")),
                };
                unit(api.set_audio_output(which).map(drop))
            }
            Op::SpeakerFilter(on) => unit(api.set_speaker_filter(*on).map(drop)),
            Op::Dts(on) => unit(api.set_dts_power(*on).map(drop)),
            Op::DtsMode(m) => unit(api.set_dts_mode(m).map(drop)),
            Op::DtsEverywhere => api.dts_everywhere(None).map(|d| Done::Text(d.join("; "))).map_err(err),
            Op::MicMute(muted) => {
                api.set_mic_mute(*muted, false);
                Ok(Done::Unit)
            }
            Op::MicVolume(n) => {
                let percent = u32::try_from(*n).map_err(|_| "microphone volume is 0..150 %".to_owned())?;
                unit(api.set_mic_volume(percent).map(drop))
            }
            Op::Voice { input, mode } => {
                let device = match input {
                    VoiceInput::Headset => VoiceDevice::Headset,
                    VoiceInput::Array => VoiceDevice::Array,
                };
                let mode = match mode.as_str() {
                    "off" => VoiceMode::Off,
                    "conference" => VoiceMode::Conference,
                    "personal" => VoiceMode::Personal,
                    other => {
                        return Err(format!("noise reduction: off | conference | personal (got '{other}')"));
                    }
                };
                unit(api.set_voice(device, mode).map(drop))
            }
            Op::Profile(n) => unit(api.apply_profile(n).map(drop)),
            Op::Look(n) => unit(api.apply_look(n).map(drop)),
            Op::Favourite(kind, n) => unit(api.favourite_apply(*kind, n).map(drop)),
            Op::OpenWindow => match &self.window {
                Some(open) => open().map(|()| Done::Unit),
                None => Err("the window cannot be opened from here (run: truesense gui)".to_owned()),
            },
            Op::RunDoctor => {
                let report = api.run_doctor();
                Ok(Done::Doctor(
                    report
                        .checks
                        .into_iter()
                        .map(|c| DoctorRow {
                            check: c.check,
                            grade: grade(c.status),
                            detail: c.detail,
                            fix: c.fix,
                        })
                        .collect(),
                ))
            }
        }
    }
}
