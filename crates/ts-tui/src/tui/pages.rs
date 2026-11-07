//! The seven pages: which rows each has, and for every setting how to read it from the latest
//! readings, when it is usable, what to ask before writing it and how to print it.
//!
//! Nothing here writes: a row's write is decided in `update` (it needs the screen's state), and
//! carried out by the worker. Everything here is a pure function of the readings, so the same row
//! can be drawn, tested and reasoned about without a laptop.

use super::backend::Catalog;
use super::data::{Data, FanMode, Mode, ReadKey, SlowValue, SoundCard};
use super::light::{Edits, Light};
use super::row::{Choice, Id, Kind, Row, Value, int_range};
use super::style::St;

/// The tabs in order.
pub(crate) const PAGE_NAMES: [&str; 7] =
    ["Dashboard", "Fans", "Power", "Lighting", "Audio", "Profiles", "System"];

/// The colours the colour rows offer (name, `rrggbb`).
pub(crate) const COLORS: [(&str, &str); 9] = [
    ("White", "ffffff"),
    ("Red", "ff0000"),
    ("Orange", "ff7a00"),
    ("Yellow", "ffd000"),
    ("Green", "00ff40"),
    ("Teal", "00aec7"),
    ("Blue", "0050ff"),
    ("Purple", "8000ff"),
    ("Pink", "ff3fa0"),
];

const DTS_MODES: [&str; 7] = ["Music", "Movie", "Voice", "Game1", "Game2", "Game3", "Plain"];
const DIRECTIONS: [&str; 4] = ["Left to right", "Right to left", "Up", "Down"];

/// One screen: its rows, the settings it reads when it opens, and whether the read-only dashboard
/// panels are drawn under the rows.
#[derive(Debug, Clone)]
pub(crate) struct Page {
    pub name: &'static str,
    pub rows: Vec<Row>,
    pub reads: Vec<ReadKey>,
    pub sel: usize,
    pub scroll: usize,
    pub dashboard: bool,
}

/// What the behaviour tables read from.
pub(crate) struct Ctx<'a> {
    pub data: &'a Data,
    pub kb: &'a Edits,
    pub rear: &'a Edits,
    pub version: &'a str,
}

fn colour_options() -> Vec<Choice> {
    COLORS.iter().map(|(name, hex)| Choice::new(Value::text(*hex), *name)).collect()
}

fn named(names: &[String]) -> Vec<Choice> {
    names.iter().map(|n| Choice::named(n)).collect()
}

fn capitalised(names: &[&str]) -> Vec<Choice> {
    names.iter().map(|n| Choice::capitalised(n)).collect()
}

fn on_off_choice(pairs: &[(i64, &str)]) -> Vec<Choice> {
    pairs.iter().map(|(v, l)| Choice::new(Value::Int(*v), *l)).collect()
}

fn mode_options() -> Vec<Choice> {
    Mode::ALL.iter().map(|m| Choice::new(Value::text(m.name()), m.label())).collect()
}

/// The duties a fan slider offers: 0 (a stop, which asks first) and 20 to 100 in steps of 5.
/// A duty between 1 and 19 does not exist: the firmware takes nothing there.
pub(crate) fn duty_values() -> Vec<i64> {
    std::iter::once(0).chain(int_range(20, 100, 5)).collect()
}

/// Builds the seven pages from the names the laptop offers.
pub(crate) fn build(cat: &Catalog) -> Vec<Page> {
    let duties = duty_values();
    let fan_modes = vec![
        Choice::new(Value::text("smart"), "Smart curve"),
        Choice::new(Value::text("auto"), "Auto"),
        Choice::new(Value::text("max"), "Max"),
        Choice::new(Value::text("manual"), "Manual"),
        Choice::new(Value::text("off"), "Off"),
    ];
    let auto_modes = || {
        Row::choice(
            Id::AutoMode,
            "Automatic power mode",
            vec![Choice::new(Value::text("native"), "Native"), Choice::new(Value::text("smart"), "Smart"), Choice::new(Value::text("off"), "Off")],
        )
        .hint("Native: like PredatorSense (changes with the charger). Smart: also watches heat, load and games. Off: hands off. Needs the TrueSense service.")
    };
    let power_mode = || {
        Row::choice(Id::PowerMode, "Power mode", mode_options())
            .mode_colors()
            .hint("Eco / Quiet / Balanced / Performance / Turbo. Your pick is held for 30 minutes against Smart auto.")
    };

    let dashboard = Page::new(
        "Dashboard",
        vec![
            power_mode(),
            auto_modes(),
            Row::choice(
                Id::DashFans,
                "Fans",
                vec![
                    Choice::new(Value::text("smart"), "Smart"),
                    Choice::new(Value::text("auto"), "Auto"),
                    Choice::new(Value::text("max"), "Max"),
                ],
            )
            .hint(
                "Smart: TrueSense's own curve. Auto: the firmware. Max: full speed. More on the Fans page.",
            ),
        ],
        vec![],
        true,
    );

    let fans = Page::new(
        "Fans",
        vec![
            Row::header("Fan control"),
            Row::choice(Id::FanMode, "Mode", fan_modes)
                .hint("Smart: TrueSense's curve. Auto: firmware. Max: 100 %. Manual: the sliders below. Off: both fans stop (asks first)."),
            Row::slider(Id::FanCpu, "CPU fan", duties.clone())
                .exact0()
                .start(40)
                .hint("Left/Right in steps of 5; the lowest position is OFF and asks first. Moving a slider switches to Manual."),
            Row::slider(Id::FanGpu, "GPU fan", duties)
                .exact0()
                .start(40)
                .hint("Stop one fan and keep the other, for example when external fans cool the laptop (asks first)."),
            Row::info(Id::CpuRpm, "CPU fan speed"),
            Row::info(Id::GpuRpm, "GPU fan speed"),
            Row::header("Fan curve"),
            Row::choice(Id::Curve, "Curve", named(&cat.curves)).hint("Used by the Smart mode. native = the firmware's own curve."),
        ],
        vec![ReadKey::Curves],
        false,
    );

    let power = Page::new(
        "Power",
        vec![
            Row::header("Mode"),
            power_mode(),
            auto_modes(),
            Row::choice(Id::SmartProfile, "Smart profile", named(&cat.smart_profiles)).hint(
                "How Smart auto reacts: when to go up, how steady to stay. 'steady' for long games and external cooling.",
            ),
            Row::info(Id::Hold, "Hold"),
            Row::button(Id::ReleaseHold, "Release the hold", "Release").hint("Let the automatic power mode take over now."),
            Row::header("Battery and charging"),
            Row::toggle(Id::BatteryLimit, "Battery limit (80 %)").hint("Stops charging at 80 % to keep the battery healthy."),
            Row::button(Id::ChargeFull, "Charge to 100 % once", "Start / stop")
                .hint("Needs the charger and the 80 % limit on; it goes back to the limit by itself."),
            Row::toggle(Id::Calibration, "Battery calibration")
                .hint("A slow full charge and discharge cycle that re-learns the capacity."),
            Row::choice(
                Id::UsbCharge,
                "USB charging when off",
                on_off_choice(&[(0, "Off"), (10, "10 %"), (20, "20 %"), (30, "30 %")]),
            )
            .hint("Charge phones from the USB port while the laptop is off, down to this battery level."),
            Row::header("Graphics and display"),
            Row::choice(
                Id::GpuMode,
                "GPU mode",
                vec![
                    Choice::new(Value::text("optimus"), "Hybrid"),
                    Choice::new(Value::text("nvidia"), "NVIDIA only"),
                    Choice::new(Value::text("auto"), "Auto"),
                ],
            )
            .hint("Applies after a reboot."),
            Row::toggle(Id::Overdrive, "Panel overdrive").hint("Faster pixel response; can show slight overshoot."),
            Row::toggle(Id::BootSound, "Boot sound"),
            Row::toggle(Id::KbTimeout, "Keyboard light timeout")
                .hint("Turn the keyboard light off after a while without typing."),
        ],
        vec![
            ReadKey::Limit,
            ReadKey::Calibration,
            ReadKey::UsbCharge,
            ReadKey::GpuMode,
            ReadKey::Overdrive,
            ReadKey::BootSound,
            ReadKey::KbTimeout,
            ReadKey::SmartProfiles,
            ReadKey::ChargeFull,
        ],
        false,
    );

    let lighting = Page::new(
        "Lighting",
        vec![
            Row::header("Keyboard"),
            Row::choice(
                Id::KbEffect,
                "Effect",
                cat.effects.iter().map(|e| Choice::capitalised(e)).collect(),
            )
            .hint("The effects built into the keyboard. Per-key colours and your own images: truesense gui --page keys."),
            Row::choice(Id::KbColor, "Colour", colour_options()).hint("Not used by Wave, Neon and Off."),
            Row::button(Id::KbCustom, "Custom colour", "Type hex…").hint("Any colour as RRGGBB."),
            Row::slider(Id::KbBrightness, "Brightness", int_range(0, cat.max_brightness, 5)),
            Row::slider(Id::KbSpeed, "Speed", int_range(cat.min_speed, cat.max_speed, 1)).hint("Slower is calmer."),
            Row::choice(
                Id::KbDirection,
                "Direction",
                DIRECTIONS.iter().zip(0..).map(|(l, v)| Choice::new(Value::Int(v), *l)).collect(),
            ),
            Row::choice(Id::KbSaved, "Saved lighting", Vec::new()).hint("Per-key lightings you saved in the window."),
            Row::header("Light bar (back of the laptop)"),
            Row::choice(
                Id::RearEffect,
                "Effect",
                cat.rear_modes.iter().map(|e| Choice::capitalised(e)).collect(),
            ),
            Row::choice(Id::RearColor, "Colour", colour_options()),
            Row::button(Id::RearCustom, "Custom colour", "Type hex…"),
            Row::slider(Id::RearBrightness, "Brightness", int_range(0, 100, 10)).hint("0 turns the bar off."),
            Row::slider(Id::RearSpeed, "Speed", vec![1, 2, 3, 4, 5]),
        ],
        vec![ReadKey::KbLast, ReadKey::KbNow, ReadKey::Rear, ReadKey::SavedLightings],
        false,
    );

    let voice = capitalised(&["off", "conference", "personal"]);
    let audio = Page::new(
        "Audio",
        vec![
            Row::header("Sound"),
            Row::info(Id::SoundCard, "Sound card"),
            Row::button(Id::FixSound, "No sound at all?", "Fix sound").hint(
                "Restarts WirePlumber so it probes the sound card again. Nothing is written to the sound chip.",
            ),
            Row::choice(
                Id::Output,
                "Play sound on",
                vec![Choice::new(Value::text("speakers"), "Speakers"), Choice::new(Value::text("headphones"), "Headphones")],
            )
            .hint("This laptop plays on the speakers or the headphone jack, not both."),
            Row::toggle(Id::SpeakerFilter, "Noise filter on the speakers")
                .hint("Off by default: it also gates music. For calls on the speakers."),
            Row::header("Acer DTS sound"),
            Row::toggle(Id::Dts, "DTS").hint("Needs AcerUltraX."),
            Row::choice(Id::DtsMode, "DTS sound mode", DTS_MODES.iter().map(|m| Choice::named(m)).collect()).hint(
                "Applies to every output: speakers, jack, USB and Bluetooth headsets. Plain is no processing.",
            ),
            Row::button(Id::DtsEverywhere, "A headset still sounds flat?", "Use DTS everywhere")
                .hint("New outputs get DTS by themselves; this does it again for all of them now."),
            Row::info(Id::DtsOutputs, "Outputs").wrapped(),
            Row::header("Microphone"),
            Row::toggle(Id::MicMute, "Mute every microphone"),
            Row::slider(Id::MicVolume, "Microphone volume", int_range(0, 150, 5)),
            Row::choice(Id::VoiceHeadset, "Headset mic noise reduction", voice.clone()),
            Row::choice(Id::VoiceArray, "Built-in mic noise reduction", voice),
        ],
        vec![
            ReadKey::Sound,
            ReadKey::Output,
            ReadKey::SpeakerFilter,
            ReadKey::Mic,
            ReadKey::Voice,
            ReadKey::DtsOutputs,
            ReadKey::DtsOn,
        ],
        false,
    );

    let mut rows = vec![Row::header("Profiles: a whole setup in one go (power, fans, lighting, audio)")];
    for (name, line) in &cat.profiles {
        rows.push(Row::button(Id::Profile(name.clone()), name, "Apply").hint(line));
    }
    if cat.profiles.is_empty() {
        rows.push(Row::info(Id::NoProfiles, ""));
    }
    rows.push(Row::header("Looks: the lighting at boot, as saved in the window"));
    for name in &cat.looks {
        rows.push(
            Row::button(Id::Look(name.clone()), name, "Apply")
                .hint("Applies the saved keyboard and light-bar look."),
        );
    }
    let profiles = Page::new("Profiles", rows, vec![], false);

    let mut rows = vec![
        Row::header("TrueSense"),
        Row::info(Id::Version, "Version"),
        Row::info(Id::ModuleVersion, "Kernel module"),
        Row::info(Id::Service, "Service"),
        Row::info(Id::WindowInfo, "Window"),
        Row::button(Id::OpenWindow, "Open the window", "Open")
            .hint("The TrueSense window (the same as running truesense gui)."),
        Row::header("Health check"),
        Row::button(Id::RunDoctor, "Run the check", "Run doctor")
            .hint("Looks at the module, the service, power managers, audio and more. Read-only."),
        Row::info(Id::DoctorHint, ""),
    ];
    for title in &cat.doctor_checks {
        rows.push(Row::info(Id::Doctor(title.clone()), title).wrapped());
    }
    let system = Page::new("System", rows, vec![ReadKey::ModuleVersion], false);

    vec![dashboard, fans, power, lighting, audio, profiles, system]
}

impl Page {
    fn new(name: &'static str, rows: Vec<Row>, reads: Vec<ReadKey>, dashboard: bool) -> Self {
        let data = Data::default();
        let ctx = Ctx { data: &data, kb: &Edits::default(), rear: &Edits::default(), version: "" };
        let sel = rows.iter().position(|r| selectable(r, &ctx)).unwrap_or(0);
        Self { name, rows, reads, sel, scroll: 0, dashboard }
    }
}

/// Whether the cursor can rest on and act on a row.
pub(crate) fn selectable(row: &Row, c: &Ctx) -> bool {
    !matches!(row.kind, Kind::Header | Kind::Info) && enabled(&row.id, c) && !hidden(&row.id, c)
}

/// Whether a row is usable now (the ones that need the service are not without it).
pub(crate) fn enabled(id: &Id, c: &Ctx) -> bool {
    match id {
        Id::AutoMode | Id::Curve | Id::SmartProfile => c.data.daemon(),
        Id::ReleaseHold => c.data.service().hold_left_s > 0,
        Id::ChargeFull => c.data.daemon() && c.data.bool(ReadKey::Limit).unwrap_or(false),
        Id::KbSaved => c.data.list(ReadKey::SavedLightings).is_some_and(|l| !l.is_empty()),
        _ => true,
    }
}

/// Whether a row is left out of the page.
pub(crate) fn hidden(id: &Id, c: &Ctx) -> bool {
    match id {
        Id::DoctorHint => !c.data.doctor().is_empty(),
        Id::Doctor(_) => c.data.doctor().is_empty(),
        _ => false,
    }
}

/// The options of a choice row now: some lists are read at run time.
pub(crate) fn options(row: &Row, c: &Ctx) -> Vec<Choice> {
    match row.id {
        Id::Curve => c
            .data
            .names(ReadKey::Curves)
            .filter(|n| !n.names.is_empty())
            .map_or_else(|| row.options.clone(), |n| named(&n.names)),
        Id::SmartProfile => c
            .data
            .names(ReadKey::SmartProfiles)
            .filter(|n| !n.names.is_empty())
            .map_or_else(|| row.options.clone(), |n| named(&n.names)),
        Id::KbSaved => match c.data.list(ReadKey::SavedLightings) {
            Some(l) if !l.is_empty() => named(l),
            _ => vec![Choice::new(Value::text(""), "none saved")],
        },
        _ => row.options.clone(),
    }
}

/// The fan mode the screen shows, from the firmware's behaviour and what the service says.
fn fan_mode(c: &Ctx) -> Option<Value> {
    let fan = c.data.fan();
    if !c.data.daemon() && fan.is_none() {
        return None;
    }
    let svc = c.data.service();
    if svc.fan_off {
        return Some(Value::text("off"));
    }
    let custom = !svc.fan_curve.is_empty() && svc.fan_curve != "native";
    match fan?.mode {
        FanMode::Manual if svc.smart_fan => Some(Value::text("smart")),
        FanMode::Auto if svc.smart_fan && custom => Some(Value::text("smart")),
        m @ (FanMode::Auto | FanMode::Max | FanMode::Manual) => Some(Value::text(m.name())),
        FanMode::Unknown => None,
    }
}

fn some_text(s: &str) -> Option<Value> {
    (!s.is_empty()).then(|| Value::text(s))
}

fn colour_known(color: &str) -> Option<Value> {
    COLORS.iter().any(|(_, hex)| *hex == color).then(|| Value::text(color))
}

fn rpm(c: &Ctx, pick: fn(&super::data::Sensors) -> i64) -> Option<Value> {
    c.data.sensors().map(|s| Value::text(format!("{} rpm", pick(&s))))
}

fn doctor_line(c: &Ctx, title: &str) -> Value {
    let rows = c.data.doctor();
    match rows.iter().find(|r| r.check == title) {
        Some(r) => {
            let mut s = format!("{}  {}", r.grade.word(), r.detail);
            if !r.fix.is_empty() && r.grade != super::data::Grade::Ok {
                s.push_str("   -> ");
                s.push_str(&r.fix);
            }
            Value::text(s)
        }
        None if rows.is_empty() => Value::text("not checked yet"),
        None => Value::text("…"),
    }
}

/// The latest reading of a row (`None` = unknown, drawn as an ellipsis).
pub(crate) fn reading(id: &Id, c: &Ctx) -> Option<Value> {
    let d = c.data;
    let kb = || Light::keyboard(d, c.kb);
    let rear = || Light::rear(d, c.rear);
    match id {
        Id::Header | Id::KbCustom | Id::RearCustom | Id::KbSaved => None,
        Id::PowerMode => some_text(d.power_mode()),
        Id::AutoMode => some_text(&d.service().auto_mode),
        Id::FanMode | Id::DashFans => fan_mode(c),
        Id::FanCpu => d.fan().map(|f| Value::Int(f.cpu_duty)),
        Id::FanGpu => d.fan().map(|f| Value::Int(f.gpu_duty)),
        Id::CpuRpm => rpm(c, |s| s.cpu_fan_rpm),
        Id::GpuRpm => rpm(c, |s| s.gpu_fan_rpm),
        Id::Curve => d
            .names(ReadKey::Curves)
            .and_then(|n| n.current.as_deref())
            .filter(|n| !n.is_empty())
            .map(Value::text)
            .or_else(|| some_text(&d.service().fan_curve)),
        Id::SmartProfile => d
            .names(ReadKey::SmartProfiles)
            .and_then(|n| n.current.as_deref())
            .filter(|n| !n.is_empty())
            .map(Value::text)
            .or_else(|| some_text(&d.service().smart_profile)),
        Id::Hold => {
            let left = d.service().hold_left_s;
            Some(Value::text(if left > 0 {
                format!("your pick is held for {} min {} s", left / 60, left % 60)
            } else {
                "none: automatic power mode is in charge".to_owned()
            }))
        }
        Id::ReleaseHold
        | Id::ChargeFull
        | Id::FixSound
        | Id::DtsEverywhere
        | Id::OpenWindow
        | Id::RunDoctor => None,
        Id::Profile(_) | Id::Look(_) => None,
        Id::BatteryLimit => d.bool(ReadKey::Limit).map(Value::Bool),
        Id::Calibration => d.bool(ReadKey::Calibration).map(Value::Bool),
        Id::UsbCharge => d.int(ReadKey::UsbCharge).map(Value::Int),
        Id::GpuMode => d.text(ReadKey::GpuMode).map(Value::text),
        Id::Overdrive => d.bool(ReadKey::Overdrive).map(Value::Bool),
        Id::BootSound => d.bool(ReadKey::BootSound).map(Value::Bool),
        Id::KbTimeout => d.bool(ReadKey::KbTimeout).map(Value::Bool),
        Id::KbEffect => Some(Value::text(kb().name)),
        Id::KbColor => colour_known(&kb().color),
        Id::KbBrightness => Some(Value::Int(kb().brightness)),
        Id::KbSpeed => Some(Value::Int(kb().speed)),
        Id::KbDirection => Some(Value::Int(kb().direction)),
        Id::RearEffect => Some(Value::text(rear().name)),
        Id::RearColor => colour_known(&rear().color),
        Id::RearBrightness => Some(Value::Int(rear().brightness)),
        Id::RearSpeed => Some(Value::Int(rear().speed)),
        Id::SoundCard => match d.get(ReadKey::Sound)? {
            SlowValue::Sound(SoundCard::Ok) => Some(Value::text("output present")),
            SlowValue::Sound(SoundCard::Missing) => Some(Value::text("no sound card")),
            SlowValue::Sound(SoundCard::Broken(p)) => {
                Some(Value::text(format!("NO OUTPUT: {}", if p.is_empty() { "?" } else { p })))
            }
            _ => None,
        },
        Id::Output => d.text(ReadKey::Output).map(Value::text),
        Id::SpeakerFilter => d.bool(ReadKey::SpeakerFilter).map(Value::Bool),
        Id::Dts => d.bool(ReadKey::DtsOn).map(Value::Bool),
        Id::DtsMode => match d.get(ReadKey::DtsOutputs)? {
            SlowValue::Dts(devs) => devs.iter().find(|o| o.current).map(|o| Value::text(&o.profile)),
            _ => None,
        },
        Id::DtsOutputs => match d.get(ReadKey::DtsOutputs)? {
            SlowValue::Dts(devs) if !devs.is_empty() => Some(Value::text(
                devs.iter().map(|o| format!("{}: {}", o.label, o.profile)).collect::<Vec<_>>().join(" · "),
            )),
            _ => None,
        },
        Id::MicMute => match d.get(ReadKey::Mic)? {
            SlowValue::Mic(m) => Some(Value::Bool(m.muted)),
            _ => None,
        },
        Id::MicVolume => match d.get(ReadKey::Mic)? {
            SlowValue::Mic(m) => m.volume.map(Value::Int),
            _ => None,
        },
        Id::VoiceHeadset => match d.get(ReadKey::Voice)? {
            SlowValue::Voice(v) => v.headset.as_deref().map(Value::text),
            _ => None,
        },
        Id::VoiceArray => match d.get(ReadKey::Voice)? {
            SlowValue::Voice(v) => v.array.as_deref().map(Value::text),
            _ => None,
        },
        Id::NoProfiles => Some(Value::text("no profiles found (truesense profile list)")),
        Id::Version => Some(Value::text(c.version)),
        Id::ModuleVersion => d.text(ReadKey::ModuleVersion).map(Value::text),
        Id::Service => {
            Some(Value::text(if d.daemon() { "running" } else { "not running (direct hardware access)" }))
        }
        Id::WindowInfo => Some(Value::text("truesense gui")),
        Id::DoctorHint => Some(Value::text("not run yet: press Enter on the button above")),
        Id::Doctor(title) => Some(doctor_line(c, title)),
    }
}

/// A question to ask before this value is written, or `None`. Stopping a fan, and anything that
/// needs a reboot to take effect, asks first; the answer defaults to No.
pub(crate) fn guard(id: &Id, v: &Value) -> Option<String> {
    match id {
        Id::FanMode if v.as_str() == "off" => {
            Some("Stop BOTH fans? The service brings them back at CPU 70 °C / GPU 65 °C.".to_owned())
        }
        Id::FanCpu if v.as_int() == Some(0) => {
            Some("Stop the CPU fan? The service brings it back when the laptop gets warm.".to_owned())
        }
        Id::FanGpu if v.as_int() == Some(0) => {
            Some("Stop the GPU fan? The service brings it back when the laptop gets warm.".to_owned())
        }
        Id::GpuMode => {
            Some(format!("Switch the GPU mode to {}? It takes effect after a reboot.", v.as_str()))
        }
        _ => None,
    }
}

/// A value as the row prints it.
pub(crate) fn show(row: &Row, v: &Value) -> String {
    let n = v.as_int().unwrap_or(0);
    match row.id {
        Id::FanCpu | Id::FanGpu => {
            if n == 0 {
                "OFF".to_owned()
            } else {
                format!("{n} %")
            }
        }
        Id::KbBrightness => format!("{} %", n * 2),
        Id::RearBrightness | Id::MicVolume => format!("{n} %"),
        _ => v.plain(),
    }
}

/// The look of an info row's value.
pub(crate) fn info_style(id: &Id, v: &Value, c: &Ctx) -> St {
    match id {
        Id::SoundCard => {
            if v.as_str() == "output present" {
                St::Good
            } else {
                St::Bad
            }
        }
        Id::Service => {
            if v.as_str() == "running" {
                St::Good
            } else {
                St::Warn
            }
        }
        Id::Doctor(title) => match c.data.doctor().iter().find(|r| &r.check == title).map(|r| r.grade) {
            None => St::Dim,
            Some(super::data::Grade::Ok) => St::Good,
            Some(super::data::Grade::Warn) => St::Warn,
            Some(super::data::Grade::Fail) => St::Bad,
        },
        _ => St::Text,
    }
}

/// The look of one option of a power-mode row.
pub(crate) fn mode_style(option: &Value, selected: bool) -> St {
    match Mode::from_name(option.as_str()) {
        Some(m) if selected => St::ModeSel(m),
        Some(m) => St::Mode(m),
        None if selected => St::Pill,
        None => St::Accent,
    }
}
