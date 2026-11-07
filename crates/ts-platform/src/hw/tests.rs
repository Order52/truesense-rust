#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use super::*;
use crate::firmware::{Device, FirmwareError, READ_ONLY_COMMANDS, SimFan, Simulator};

// ---- helpers --------------------------------------------------------------------------------

type Sleeps = Arc<Mutex<Vec<Duration>>>;

/// A `Hw` over a fresh simulator that never really sleeps: the pauses are recorded instead.
fn rig() -> (Hw, Arc<Simulator>, Sleeps) {
    let sim = Arc::new(Simulator::new());
    let (hw, sleeps) = rig_over(Arc::clone(&sim) as Arc<dyn Device>);
    (hw, sim, sleeps)
}

fn rig_over(device: Arc<dyn Device>) -> (Hw, Sleeps) {
    let sleeps = Sleeps::default();
    let log = Arc::clone(&sleeps);
    let hw = Hw::new(device).with_sleeper(Arc::new(move |d| log.lock().unwrap().push(d)));
    (hw, sleeps)
}

/// Passes commands to the simulator and records them; `rewrite` may replace a reply.
/// May replace the reply to the nth time a command is sent: a reply line, or a transport failure.
type Rewrite = Box<dyn Fn(&str, usize) -> Option<Result<String, io::ErrorKind>> + Send + Sync>;

struct Spy {
    inner: Simulator,
    sent: Mutex<Vec<String>>,
    rewrite: Rewrite,
    counts: Mutex<BTreeMap<String, usize>>,
}

impl Spy {
    fn new(
        rewrite: impl Fn(&str, usize) -> Option<Result<String, io::ErrorKind>> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner: Simulator::new(),
            sent: Mutex::default(),
            rewrite: Box::new(rewrite),
            counts: Mutex::default(),
        })
    }

    fn passthrough() -> Arc<Self> {
        Self::new(|_, _| None)
    }

    fn sent(&self) -> Vec<String> {
        self.sent.lock().unwrap().clone()
    }
}

impl Device for Spy {
    fn run(&self, command: &str) -> Result<String, FirmwareError> {
        self.sent.lock().unwrap().push(command.to_string());
        let nth = {
            let mut counts = self.counts.lock().unwrap();
            let n = counts.entry(command.to_string()).or_insert(0);
            *n += 1;
            *n
        };
        match (self.rewrite)(command, nth) {
            Some(Ok(line)) => Ok(line),
            Some(Err(kind)) => Err(FirmwareError::Io {
                operation: command.to_string(),
                source: io::Error::new(kind, "scripted"),
            }),
            None => self.inner.run(command),
        }
    }
}

fn spy_rig(spy: &Arc<Spy>) -> (Hw, Sleeps) {
    rig_over(Arc::clone(spy) as Arc<dyn Device>)
}

fn text(e: HwError) -> String {
    e.to_string()
}

// ---- vocabulary -----------------------------------------------------------------------------

#[test]
fn modes_have_the_firmware_numbering_and_the_users_names() {
    let table: [(PowerMode, i64, &str); 5] = [
        (PowerMode::Quiet, 0, "quiet"),
        (PowerMode::Balanced, 1, "balanced"),
        (PowerMode::Performance, 4, "performance"),
        (PowerMode::Turbo, 5, "turbo"),
        (PowerMode::Eco, 6, "eco"),
    ];
    assert_eq!(PowerMode::ALL.len(), 5);
    for (mode, code, name) in table {
        assert_eq!((mode.code(), mode.name()), (code, name));
        assert_eq!(name.parse::<PowerMode>().unwrap(), mode);
        assert_eq!(PowerMode::from_code(code), Some(mode));
    }
    assert_eq!(PowerMode::from_code(2), None);
    assert_eq!(
        PowerMode::ALL.iter().map(|m| m.name()).collect::<Vec<_>>(),
        ["quiet", "balanced", "performance", "turbo", "eco"]
    );
    for (name, code) in [("optimus", 1), ("nvidia", 2), ("auto", 3)] {
        assert_eq!(name.parse::<GpuMode>().unwrap().code(), code);
    }
    for (name, code) in [("static", 0), ("breathing", 1), ("neon", 2), ("wave", 3), ("twinkling", 7)] {
        assert_eq!(name.parse::<RearMode>().unwrap().code(), code);
    }
}

#[test]
fn unknown_names_are_refused_with_the_python_messages() {
    assert_eq!(
        text("x".parse::<PowerMode>().unwrap_err()),
        "power mode must be one of: quiet, balanced, performance, turbo, eco"
    );
    assert_eq!(
        text("x".parse::<GpuMode>().unwrap_err()),
        "gpu mode: optimus | nvidia | auto (applies after reboot)"
    );
    assert_eq!(
        text("x".parse::<RearMode>().unwrap_err()),
        "rear mode: static, breathing, neon, wave, twinkling"
    );
    for bad in ["", "Turbo", " turbo", "turbo ", "turbo\0", "ludicrous", "\u{1F600}"] {
        assert!(bad.parse::<PowerMode>().is_err(), "{bad:?}");
    }
    for percent in [-10, -1, 1, 5, 15, 31, 40, 100] {
        assert_eq!(
            text(UsbLevel::from_percent(percent).unwrap_err()),
            "usb-charge level: 0 (off), 10, 20 or 30"
        );
    }
    for (percent, level) in
        [(0, UsbLevel::Off), (10, UsbLevel::Percent10), (20, UsbLevel::Percent20), (30, UsbLevel::Percent30)]
    {
        assert_eq!(UsbLevel::from_percent(percent).unwrap(), level);
        assert_eq!(i64::from(level.percent()), percent);
    }
    assert_eq!(USB_LEVELS.map(|l| l.percent()), [0, 10, 20, 30]);
}

#[test]
fn the_fan_duty_rule_is_zero_or_twenty_to_a_hundred() {
    for percent in -200..=200 {
        let ok = FanDuty::new(percent).is_ok();
        assert_eq!(ok, percent == 0 || (20..=100).contains(&percent), "{percent}");
    }
    for bad in [i64::MIN, i64::MAX, 256, 276, -236, 19, 101] {
        assert_eq!(
            text(FanDuty::new(bad).unwrap_err()),
            "manual duty must be 20..100 % per fan (or 0 to stop that fan)",
            "{bad}"
        );
    }
    assert!(FanDuty::new(0).unwrap().is_stop());
    assert!(!FanDuty::new(20).unwrap().is_stop());
    assert_eq!(FanDuty::new(100).unwrap().percent(), 100);
}

#[test]
fn the_duty_rule_is_exactly_what_the_kernel_accepts() {
    // The type and the module's validation (as mirrored by the simulator) must never disagree:
    // a duty the type allows but the module refuses would turn into a failed write, and one the
    // type refuses but the module takes would be a rule that is stricter than the hardware's.
    let sim = Simulator::new();
    for cpu in -1..=101i64 {
        for gpu in [-1, 0, 19, 20, 55, 100, 101] {
            let typed = FanDuty::new(cpu).is_ok() && FanDuty::new(gpu).is_ok();
            let kernel = cpu >= 0 && gpu >= 0 && sim.execute(&format!("set_fan_manual {cpu} {gpu}")).is_ok();
            assert_eq!(typed, kernel, "{cpu} {gpu}");
        }
    }
}

#[test]
fn colours_parse_exactly_six_hex_digits() {
    assert_eq!(Rgb::parse("00FF40").unwrap(), Rgb([0, 255, 64]));
    assert_eq!(Rgb::parse("00FF40").unwrap().to_string(), "00ff40");
    assert_eq!("abcdef".parse::<Rgb>().unwrap(), Rgb([0xab, 0xcd, 0xef]));
    for bad in [
        "",
        "fff",
        "fffffff",
        "gggggg",
        "#ffffff",
        " ffffff",
        "ffffff ",
        "ffffff\n",
        "+fffff",
        "0xffff",
        "\u{e9}\u{e9}\u{e9}",
        "ffff\u{e9}",
        "ff\0fff",
        "\u{ff26}\u{ff26}\u{ff26}\u{ff26}\u{ff26}\u{ff26}",
    ] {
        assert_eq!(text(Rgb::parse(bad).unwrap_err()), "color must be RRGGBB hex", "{bad:?}");
    }
}

#[test]
fn readings_show_unknown_numbers_and_serialise_as_text() {
    let known: Reading<PowerMode> = Reading::from_code(5);
    let unknown: Reading<PowerMode> = Reading::from_code(7);
    assert_eq!((known.to_string(), unknown.to_string()), ("turbo".to_string(), "unknown(7)".to_string()));
    assert_eq!(known.known(), Some(PowerMode::Turbo));
    assert_eq!(unknown.known(), None);
    assert_eq!(serde_json::to_value(unknown).unwrap(), json!("unknown(7)"));
    assert_eq!(serde_json::to_value(PowerMode::Eco).unwrap(), json!("eco"));
    assert_eq!(serde_json::to_value(FanMode::Manual).unwrap(), json!("manual"));
    assert_eq!(serde_json::to_value(Rgb([1, 2, 3])).unwrap(), json!("010203"));
}

// ---- settle ---------------------------------------------------------------------------------

/// Answers the first `n` `get_mode` reads with the old value, as the firmware does right after a write.
fn laggy_mode(n: usize) -> Arc<Spy> {
    Spy::new(move |c, nth| (c == "get_mode" && nth <= n).then(|| Ok("ok mode=6\n".to_string())))
}

#[test]
fn a_slow_firmware_is_waited_for() {
    let spy = laggy_mode(3);
    let (hw, sleeps) = spy_rig(&spy);
    assert_eq!(hw.set_mode(PowerMode::Turbo).unwrap(), PowerMode::Turbo);
    assert_eq!(*sleeps.lock().unwrap(), vec![Duration::from_millis(100); 3]);
    assert_eq!(spy.sent().iter().filter(|c| *c == "get_mode").count(), 4);
}

#[test]
fn a_value_that_never_verifies_is_an_error_after_about_a_second() {
    let spy = laggy_mode(1000);
    let (hw, sleeps) = spy_rig(&spy);
    let err = hw.set_mode(PowerMode::Quiet).unwrap_err();
    assert_eq!(err.to_string(), "power mode readback mismatch (wanted quiet, last read eco)");
    assert!(matches!(err, HwError::Readback { what: "power mode", .. }));
    assert_eq!(spy.sent().iter().filter(|c| *c == "get_mode").count(), 10, "ten reads");
    let total: Duration = sleeps.lock().unwrap().iter().sum();
    assert_eq!(total, Duration::from_millis(900), "nine pauses between ten reads: about a second");
}

#[test]
fn reads_that_fail_count_as_unverified_and_are_retried() {
    let spy = Spy::new(|c, nth| {
        (c == "get_mode" && nth <= 2).then(|| Ok("err get_mode failed (-5)\n".to_string()))
    });
    let (hw, sleeps) = spy_rig(&spy);
    assert_eq!(hw.set_mode(PowerMode::Balanced).unwrap(), PowerMode::Balanced);
    assert_eq!(sleeps.lock().unwrap().len(), 2);

    let spy = Spy::new(|c, _| (c == "get_mode").then_some(Err(io::ErrorKind::BrokenPipe)));
    let (hw, _) = spy_rig(&spy);
    assert_eq!(
        hw.set_mode(PowerMode::Balanced).unwrap_err().to_string(),
        "power mode readback mismatch (wanted balanced, last read nothing)"
    );
}

#[test]
fn the_retry_policy_and_the_clock_are_injectable() {
    let spy = laggy_mode(1000);
    let log = Sleeps::default();
    let hw = {
        let log = Arc::clone(&log);
        Hw::new(Arc::clone(&spy) as Arc<dyn Device>)
            .with_retry(Retry { tries: 3, delay: Duration::from_secs(7) })
            .with_sleeper(Arc::new(move |d| log.lock().unwrap().push(d)))
    };
    assert!(hw.set_mode(PowerMode::Quiet).is_err());
    assert_eq!(spy.sent().iter().filter(|c| *c == "get_mode").count(), 3);
    assert_eq!(*log.lock().unwrap(), vec![Duration::from_secs(7); 2]);
    assert_eq!(Retry::default(), Retry { tries: 10, delay: Duration::from_millis(100) });
}

#[test]
fn a_refused_write_is_not_verified_and_not_retried() {
    let spy = Spy::new(|c, _| (c == "set_mode 5").then(|| Ok("err set_mode failed (-5)\n".to_string())));
    let (hw, sleeps) = spy_rig(&spy);
    let err = hw.set_mode(PowerMode::Turbo).unwrap_err();
    assert_eq!(err.to_string(), "set_mode failed (-5)");
    assert!(matches!(err, HwError::Firmware(FirmwareError::Rejected { .. })));
    assert!(sleeps.lock().unwrap().is_empty());
    assert_eq!(spy.sent(), ["set_mode 5"], "no read-back after a refused write");
}

// ---- the plain features ---------------------------------------------------------------------

#[test]
fn version_and_capabilities() {
    let (hw, ..) = rig();
    assert_eq!(hw.version().unwrap(), "sim");
    let caps = hw.capabilities().unwrap();
    assert_eq!(caps, Capabilities { game: true, apge: true, battery: true });
    assert_eq!(serde_json::to_value(caps).unwrap(), json!({"game": true, "apge": true, "battery": true}));

    let spy = Spy::new(|c, _| match c {
        "caps" => Some(Ok("ok game=1 battery=0 future=1\n".to_string())),
        "version" => Some(Ok("ok\n".to_string())),
        _ => None,
    });
    let (hw, _) = spy_rig(&spy);
    assert_eq!(hw.capabilities().unwrap(), Capabilities { game: true, apge: false, battery: false });
    assert!(matches!(hw.version(), Err(HwError::Firmware(FirmwareError::BadReply { .. }))));
}

#[test]
fn every_power_mode_reads_back() {
    let (hw, sim, _) = rig();
    for mode in PowerMode::ALL {
        assert_eq!(hw.set_mode(*mode).unwrap(), *mode);
        assert_eq!(hw.get_mode().unwrap(), Reading::Known(*mode));
        assert_eq!(i64::from(sim.snapshot().mode), mode.code());
    }
    assert_eq!(hw.supported_modes().unwrap(), PowerMode::ALL);
}

#[test]
fn modes_the_firmware_does_not_offer_and_codes_we_do_not_know() {
    let spy = Spy::new(|c, _| match c {
        "get_modes" => Some(Ok("ok mask=0x13\n".to_string())), // bits 0, 1 and 4
        "get_mode" => Some(Ok("ok mode=3\n".to_string())),
        _ => None,
    });
    let (hw, _) = spy_rig(&spy);
    assert_eq!(
        hw.supported_modes().unwrap(),
        [PowerMode::Quiet, PowerMode::Balanced, PowerMode::Performance]
    );
    assert_eq!(hw.get_mode().unwrap(), Reading::Unknown(3));
    assert_eq!(hw.get_mode().unwrap().to_string(), "unknown(3)");
}

#[test]
fn every_on_off_setting_reads_back() {
    let (hw, sim, _) = rig();
    for want in [true, false, true] {
        assert_eq!(hw.set_boot_sound(want).unwrap(), want);
        assert_eq!(hw.get_boot_sound().unwrap(), want);
        assert_eq!(sim.snapshot().boot, u8::from(want));
        assert_eq!(hw.set_battery_limit(want).unwrap(), want);
        assert_eq!(hw.get_battery_limit().unwrap(), want);
        assert_eq!(sim.snapshot().battery[0], u8::from(want));
        assert_eq!(hw.set_calibration(want).unwrap(), want);
        assert_eq!(hw.get_calibration().unwrap(), want);
        assert_eq!(sim.snapshot().battery[1], u8::from(want));
        assert_eq!(hw.set_overdrive(want).unwrap(), want);
        assert_eq!(hw.get_overdrive().unwrap(), Some(want));
        assert_eq!(hw.set_kb_timeout(want).unwrap(), want);
        assert_eq!(hw.get_kb_timeout().unwrap(), Some(want));
        assert_eq!(sim.snapshot().kbtimeout, u8::from(want));
    }
}

#[test]
fn the_battery_limit_and_calibration_are_separate_flags() {
    let (hw, ..) = rig();
    hw.set_battery_limit(true).unwrap();
    assert!(!hw.get_calibration().unwrap());
    hw.set_calibration(true).unwrap();
    hw.set_battery_limit(false).unwrap();
    assert!(hw.get_calibration().unwrap());
}

#[test]
fn usb_charging_and_gpu_mode_read_back() {
    let (hw, sim, _) = rig();
    for level in USB_LEVELS {
        assert_eq!(hw.set_usb_charge(level).unwrap(), level);
        assert_eq!(hw.get_usb_charge().unwrap(), Reading::Known(level));
        assert_eq!(sim.snapshot().usb, level.percent());
    }
    for mode in [GpuMode::Optimus, GpuMode::Nvidia, GpuMode::Auto] {
        assert_eq!(hw.set_gpu_mode(mode).unwrap(), mode);
        assert_eq!(hw.get_gpu_mode().unwrap(), Reading::Known(mode));
    }
    assert_eq!(i64::from(sim.snapshot().gpu), GpuMode::Auto.code());
}

#[test]
fn values_the_firmware_does_not_know_are_shown_not_guessed() {
    let spy = Spy::new(|c, _| match c {
        "get_usb" => Some(Ok("ok usb=-1 raw=12345\n".to_string())),
        "get_gpu" => Some(Ok("ok gpu=9\n".to_string())),
        "get_overdrive" => Some(Ok("ok overdrive=-1 raw=0x77\n".to_string())),
        "get_kbtimeout" => Some(Ok("ok kbtimeout=-1 raw=0x77\n".to_string())),
        _ => None,
    });
    let (hw, _) = spy_rig(&spy);
    assert_eq!(hw.get_usb_charge().unwrap(), Reading::Unknown(-1));
    assert_eq!(hw.get_gpu_mode().unwrap().to_string(), "unknown(9)");
    assert_eq!(hw.get_overdrive().unwrap(), None);
    assert_eq!(hw.get_kb_timeout().unwrap(), None);
    // A setting whose state stays unknown can never be reported as set.
    let err = hw.set_overdrive(true).unwrap_err();
    assert_eq!(err.to_string(), "overdrive readback mismatch (wanted Some(true), last read None)");
    assert!(hw.set_kb_timeout(false).is_err());
    assert!(hw.set_usb_charge(UsbLevel::Percent20).is_err());
}

#[test]
fn a_reply_without_its_field_is_a_typed_error() {
    let spy = Spy::new(|c, _| (c == "get_mode").then(|| Ok("ok other=1\n".to_string())));
    let (hw, _) = spy_rig(&spy);
    let err = hw.get_mode().unwrap_err();
    assert!(matches!(err, HwError::Firmware(FirmwareError::BadReply { .. })));
    assert!(err.to_string().contains("missing/invalid 'mode'"), "{err}");
    assert!(std::error::Error::source(&err).is_some());
}

// ---- the backlight timeout (two flags) ------------------------------------------------------

#[derive(Default)]
struct FakeController {
    value: Mutex<Option<bool>>,
    set_calls: Mutex<Vec<bool>>,
    get_fails: bool,
    set_fails: bool,
}

impl KbTimeoutFlag for FakeController {
    fn get(&self) -> Result<Option<bool>, HwError> {
        if self.get_fails {
            return Err(HwError::Invalid("hid not available".to_string()));
        }
        Ok(*self.value.lock().unwrap())
    }

    fn set(&self, on: bool) -> Result<(), HwError> {
        self.set_calls.lock().unwrap().push(on);
        if self.set_fails {
            return Err(HwError::Invalid("hid not writable".to_string()));
        }
        *self.value.lock().unwrap() = Some(on);
        Ok(())
    }
}

#[test]
fn the_controllers_flag_is_the_one_that_counts() {
    let controller = Arc::new(FakeController::default());
    *controller.value.lock().unwrap() = Some(false);
    let (hw, sim, _) = rig();
    let hw = hw.with_kb_timeout(Arc::clone(&controller) as Arc<dyn KbTimeoutFlag>);
    // The firmware flag says on, the controller says off: the controller wins.
    assert_eq!(sim.snapshot().kbtimeout, 1);
    assert_eq!(hw.get_kb_timeout().unwrap(), Some(false));
    assert!(hw.set_kb_timeout(true).unwrap());
    assert_eq!(*controller.set_calls.lock().unwrap(), [true]);
    assert_eq!(hw.get_kb_timeout().unwrap(), Some(true));
}

#[test]
fn the_firmware_flag_is_the_fallback_when_the_controller_cannot_say() {
    for controller in [
        FakeController { get_fails: true, ..Default::default() },
        FakeController { value: Mutex::new(None), ..Default::default() },
    ] {
        let (hw, sim, _) = rig();
        let hw = hw.with_kb_timeout(Arc::new(controller));
        assert_eq!(hw.get_kb_timeout().unwrap(), Some(true));
        sim.execute("set_kbtimeout 0").unwrap();
        assert_eq!(hw.get_kb_timeout().unwrap(), Some(false));
    }
}

#[test]
fn a_controller_that_cannot_be_written_does_not_fail_the_setting() {
    let controller = Arc::new(FakeController { set_fails: true, get_fails: true, ..Default::default() });
    let (hw, sim, _) = rig();
    let hw = hw.with_kb_timeout(Arc::clone(&controller) as Arc<dyn KbTimeoutFlag>);
    assert!(!hw.set_kb_timeout(false).unwrap());
    assert_eq!(sim.snapshot().kbtimeout, 0);
    assert_eq!(*controller.set_calls.lock().unwrap(), [false], "it was tried");
}

// ---- sensors and fans -----------------------------------------------------------------------

#[test]
fn sensors_are_one_call_with_the_json_field_names_of_the_daemon() {
    let (hw, sim, _) = rig();
    sim.set_sensors([61, 58, 55], [2400, 2300]);
    let s = hw.sensors().unwrap();
    assert_eq!(s, Sensors { cpu_temp: 61, sys_temp: 58, gpu_temp: 55, cpu_fan_rpm: 2400, gpu_fan_rpm: 2300 });
    assert_eq!(
        serde_json::to_value(s).unwrap(),
        json!({"cpu_temp": 61, "sys_temp": 58, "gpu_temp": 55, "cpu_fan_rpm": 2400, "gpu_fan_rpm": 2300})
    );
    let spy = Spy::passthrough();
    let (hw, _) = spy_rig(&spy);
    hw.sensors().unwrap();
    assert_eq!(spy.sent(), ["sensors"]);

    let spy = Spy::new(|c, _| (c == "sensors").then(|| Ok("ok cpu=1 sys=2 gpu=3 cpu_rpm=4\n".to_string())));
    let (hw, _) = spy_rig(&spy);
    assert!(hw.sensors().unwrap_err().to_string().contains("'gpu_rpm'"));
}

#[test]
fn temperatures_alone_are_three_firmware_calls_worth_and_keep_the_last_fan_speeds() {
    let (hw, sim, _) = rig();
    sim.set_sensors([61, 58, 55], [2400, 2300]);
    let s = hw.sensor_temps((111, 222)).unwrap();
    assert_eq!(s, Sensors { cpu_temp: 61, sys_temp: 58, gpu_temp: 55, cpu_fan_rpm: 111, gpu_fan_rpm: 222 });
    let spy = Spy::passthrough();
    let (hw, _) = spy_rig(&spy);
    hw.sensor_temps((0, 0)).unwrap();
    assert_eq!(spy.sent(), ["sensors_temps"]);
}

#[test]
fn a_module_without_the_temperatures_command_is_asked_for_the_full_reading_from_then_on() {
    let spy = Spy::new(|c, _| match c {
        "sensors_temps" => Some(Ok("err unknown command\n".to_string())),
        "sensors" => Some(Ok("ok cpu=1 sys=2 gpu=3 cpu_rpm=4 gpu_rpm=5\n".to_string())),
        _ => None,
    });
    let (hw, _) = spy_rig(&spy);
    let s = hw.sensor_temps((9, 9)).unwrap();
    assert_eq!((s.cpu_temp, s.cpu_fan_rpm, s.gpu_fan_rpm), (1, 4, 5), "the full reading has real speeds");
    hw.sensor_temps((9, 9)).unwrap();
    assert_eq!(spy.sent(), ["sensors_temps", "sensors", "sensors"], "the unknown command is not asked twice");
}

#[test]
fn the_oldest_module_answers_invalid_argument_and_is_treated_the_same_way() {
    // Real behaviour of the shipped module: for an unknown command its write handler replies
    // "unknown command", then overwrites it with "invalid argument" and fails the write.
    let spy = Spy::new(|c, _| match c {
        "sensors_temps" => Some(Ok("err invalid argument\n".to_string())),
        "sensors" => Some(Ok("ok cpu=1 sys=2 gpu=3 cpu_rpm=4 gpu_rpm=5\n".to_string())),
        _ => None,
    });
    let (hw, _) = spy_rig(&spy);
    let s = hw.sensor_temps((9, 9)).unwrap();
    assert_eq!((s.cpu_temp, s.cpu_fan_rpm, s.gpu_fan_rpm), (1, 4, 5));
    hw.sensor_temps((9, 9)).unwrap();
    assert_eq!(spy.sent(), ["sensors_temps", "sensors", "sensors"]);
}

#[test]
fn a_failing_temperature_read_is_an_error_not_a_reason_to_stop_using_the_command() {
    let spy =
        Spy::new(|c, _| (c == "sensors_temps").then(|| Ok("err sensor read failed (-5)\n".to_string())));
    let (hw, _) = spy_rig(&spy);
    assert!(hw.sensor_temps((0, 0)).is_err());
    assert!(hw.sensor_temps((0, 0)).is_err());
    assert_eq!(spy.sent(), ["sensors_temps", "sensors_temps"]);
}

#[test]
fn fan_state_decodes_the_behaviour_word() {
    let (hw, ..) = rig();
    let s = hw.fan_state().unwrap();
    assert_eq!(
        (s.mode, s.behavior_raw.as_str(), s.duty_register_pct),
        (FanMode::Auto, "0x41", Duty { cpu: 30, gpu: 30 })
    );
    hw.fan_max().unwrap();
    let s = hw.fan_state().unwrap();
    assert_eq!((s.mode, s.behavior_raw.as_str()), (FanMode::Max, "0x82"));
    assert_eq!(
        serde_json::to_value(s).unwrap(),
        json!({"behavior_raw": "0x82", "mode": "max", "duty_register_pct": {"cpu": 30, "gpu": 30}})
    );
    let spy = Spy::new(|c, _| (c == "get_fan").then(|| Ok("ok behavior=0x1234 cpu=1 gpu=2\n".to_string())));
    let (hw, _) = spy_rig(&spy);
    let s = hw.fan_state().unwrap();
    assert_eq!((s.mode, s.behavior_raw.as_str()), (FanMode::Unknown, "0x1234"));
}

fn duty(c: i64, g: i64) -> (FanDuty, FanDuty) {
    (FanDuty::new(c).unwrap(), FanDuty::new(g).unwrap())
}

#[test]
fn the_fans_go_auto_max_manual_and_back() {
    let (hw, sim, _) = rig();
    hw.fan_manual(duty(60, 70).0, duty(60, 70).1).unwrap();
    let s = hw.fan_state().unwrap();
    assert_eq!((s.mode, s.duty_register_pct), (FanMode::Manual, Duty { cpu: 60, gpu: 70 }));
    hw.fan_max().unwrap();
    assert_eq!(sim.snapshot().fan, SimFan::Max);
    hw.fan_manual(FanDuty::new(40).unwrap(), FanDuty::new(55).unwrap()).unwrap();
    assert_eq!(sim.snapshot().duty, [40, 55]);
    hw.fan_auto().unwrap();
    assert_eq!(sim.snapshot().fan, SimFan::Auto);
    assert_eq!(hw.fan_state().unwrap().mode, FanMode::Auto);
}

#[test]
fn a_fan_stop_is_proven_by_reading_the_duty_registers() {
    let spy = Spy::passthrough();
    let (hw, _) = spy_rig(&spy);
    let (cpu, gpu) = duty(0, 50);
    hw.fan_manual(cpu, gpu).unwrap();
    assert_eq!(spy.inner.snapshot().duty, [0, 50]);
    assert_eq!(spy.inner.snapshot().fan, SimFan::Manual);
    // Mode and duty were both read back.
    assert!(spy.sent().iter().filter(|c| *c == "get_fan").count() >= 2);
    let (cpu, gpu) = duty(0, 0);
    hw.fan_manual(cpu, gpu).unwrap();
    assert_eq!(spy.inner.snapshot().duty, [0, 0]);
}

#[test]
fn a_fan_stop_the_firmware_ignores_hands_the_fans_back() {
    // The firmware accepts set_fan_manual 0 50 but keeps its old duty registers.
    let spy =
        Spy::new(|c, _| (c == "set_fan_manual 0 50").then(|| Ok("ok fan=manual cpu=0 gpu=50\n".to_string())));
    let (hw, sleeps) = spy_rig(&spy);
    hw.fan_manual(duty(30, 30).0, duty(30, 30).1).unwrap();
    // The mode is manual from the 30/30 command, so only the duty check can catch the stop.
    let (cpu, gpu) = duty(0, 50);
    let err = hw.fan_manual(cpu, gpu).unwrap_err();
    assert!(matches!(err, HwError::FanStopRefused { returned_to_auto: true, .. }), "{err:?}");
    assert!(
        err.to_string().starts_with("the firmware did not accept the stop (fan stop readback mismatch"),
        "{err}"
    );
    assert!(err.to_string().ends_with("the fans were returned to Auto"), "{err}");
    assert_eq!(spy.inner.snapshot().fan, SimFan::Auto, "the fans must be back on the firmware");
    assert_eq!(sleeps.lock().unwrap().len(), 9, "ten reads of the duty registers");
}

#[test]
fn if_the_fans_cannot_be_handed_back_the_error_says_so() {
    let spy = Spy::new(|c, _| match c {
        "set_fan_manual 0 50" => Some(Ok("ok\n".to_string())),
        "set_fan_auto" => Some(Ok("err set_fan_auto failed (-5)\n".to_string())),
        _ => None,
    });
    let (hw, _) = spy_rig(&spy);
    hw.fan_manual(duty(30, 30).0, duty(30, 30).1).unwrap();
    let (cpu, gpu) = duty(0, 50);
    let err = hw.fan_manual(cpu, gpu).unwrap_err();
    assert!(matches!(err, HwError::FanStopRefused { returned_to_auto: false, .. }), "{err:?}");
    assert!(err.to_string().contains("could NOT be returned to Auto"), "{err}");
    assert!(std::error::Error::source(&err).is_some());
}

#[test]
fn a_stop_that_never_reaches_manual_mode_is_a_plain_readback_error() {
    let spy = Spy::new(|c, _| (c == "set_fan_manual 0 50").then(|| Ok("ok\n".to_string())));
    let (hw, _) = spy_rig(&spy);
    let (cpu, gpu) = duty(0, 50);
    let err = hw.fan_manual(cpu, gpu).unwrap_err();
    assert!(matches!(err, HwError::Readback { what: "fan manual", .. }), "{err:?}");
    assert_eq!(err.to_string(), "fan manual readback mismatch (wanted manual, last read auto)");
}

#[test]
fn only_stops_trigger_the_duty_check() {
    let spy = Spy::passthrough();
    let (hw, _) = spy_rig(&spy);
    let (cpu, gpu) = duty(40, 55);
    hw.fan_manual(cpu, gpu).unwrap();
    assert_eq!(spy.sent(), ["set_fan_manual 40 55", "get_fan"], "one write, one read-back");
}

// ---- the rear light -------------------------------------------------------------------------

#[derive(Default)]
struct Memory(Mutex<Vec<(String, Value)>>);

impl RearMemory for Memory {
    fn remember(&self, key: &str, value: Value) {
        self.0.lock().unwrap().push((key.to_string(), value));
    }
}

fn rear_rig() -> (Hw, Arc<Simulator>, Arc<Memory>) {
    let (hw, sim, _) = rig();
    let memory = Arc::new(Memory::default());
    (hw.with_rear_memory(Arc::clone(&memory) as Arc<dyn RearMemory>), sim, memory)
}

fn settings(mode: RearMode, color: &str, brightness: i64) -> RearSettings {
    RearSettings { mode, color: Rgb::parse(color).unwrap(), brightness, ..RearSettings::default() }
}

#[test]
fn the_rear_light_round_trips() {
    let (hw, ..) = rear_rig();
    let r = hw.rear_set(&RearSettings { speed: 3, ..settings(RearMode::Breathing, "00ff88", 70) }).unwrap();
    assert_eq!(
        (r.mode, r.color.to_string(), r.brightness),
        (Reading::Known(RearMode::Breathing), "00ff88".to_string(), 70)
    );
    assert_eq!((r.speed, r.direction), (3, Some(1)));
    assert!(r.zones.is_none(), "only the static effect has parts");
    let r = hw.rear_set(&settings(RearMode::Wave, "00FF40", 60)).unwrap();
    assert_eq!(
        (r.mode, r.color.to_string(), r.brightness),
        (Reading::Known(RearMode::Wave), "00ff40".to_string(), 60)
    );
    // Brightness 0 is a setting (the light is off), not an error.
    let r = hw.rear_set(&settings(RearMode::Wave, "00FF40", 0)).unwrap();
    assert_eq!(r.brightness, 0);
    assert_eq!(hw.rear_get().unwrap(), Rear { zones: None, ..r });
}

#[test]
fn the_packet_has_the_layout_the_module_validates() {
    let spy = Spy::passthrough();
    let (hw, _) = spy_rig(&spy);
    let s = RearSettings { speed: 5, direction: 0, ..settings(RearMode::Twinkling, "102030", 99) };
    hw.rear_set(&s).unwrap();
    let sent = spy.sent();
    assert_eq!(sent[0], "set_rear 07056300001020300302000000000000");
    assert_eq!(sent[0].len(), "set_rear ".len() + 32);
    assert_eq!(sent.len(), 3, "write, read back, read for the result: {sent:?}");
}

#[test]
fn the_static_effect_writes_each_part_separately_left_to_right() {
    let spy = Spy::passthrough();
    let (hw, _) = spy_rig(&spy);
    let memory = Arc::new(Memory::default());
    let hw = hw.with_rear_memory(Arc::clone(&memory) as Arc<dyn RearMemory>);
    let zones = vec![Rgb([255, 0, 0]), Rgb([0, 255, 0]), Rgb([0, 0, 255])];
    let r = hw.rear_set(&RearSettings { zones: Some(zones.clone()), ..RearSettings::default() }).unwrap();
    assert_eq!(r.zones, Some(zones));
    let sent = spy.sent();
    assert_eq!(sent[1..4], ["set_color ff0000 1", "set_color 00ff00 2", "set_color 0000ff 4"]);
    let snap = spy.inner.snapshot().zones;
    assert_eq!((snap[&1].as_str(), snap[&2].as_str(), snap[&4].as_str()), ("ff0000", "00ff00", "0000ff"));
    assert_eq!(
        *memory.0.lock().unwrap(),
        vec![
            ("rear_zones".to_string(), json!(["ff0000", "00ff00", "0000ff"])),
            (
                "rear_state".to_string(),
                json!({"mode": "static", "color": "ff0000", "speed": 3, "brightness": 100, "direction": 1,
                       "zones": ["ff0000", "00ff00", "0000ff"]})
            ),
        ]
    );
}

#[test]
fn one_zone_colour_fills_all_three_parts() {
    let (hw, sim, _) = rear_rig();
    let r =
        hw.rear_set(&RearSettings { zones: Some(vec![Rgb([1, 2, 3])]), ..RearSettings::default() }).unwrap();
    assert_eq!(r.zones, Some(vec![Rgb([1, 2, 3]); 3]));
    assert_eq!(sim.snapshot().zones.len(), 3);
}

#[test]
fn a_static_effect_without_zones_paints_every_part_with_the_colour_and_remembers_no_zones() {
    let (hw, sim, memory) = rear_rig();
    let r = hw.rear_set(&settings(RearMode::Static, "abcdef", 80)).unwrap();
    assert_eq!(r.zones, Some(vec![Rgb([0xab, 0xcd, 0xef]); 3]));
    assert_eq!(sim.snapshot().zones[&4], "abcdef");
    let remembered = memory.0.lock().unwrap();
    assert_eq!(remembered[0], ("rear_zones".to_string(), json!(["abcdef", "abcdef", "abcdef"])));
    assert_eq!(remembered[1].1["zones"], Value::Null, "Python stores null here: the caller gave no zones");
}

#[test]
fn a_moving_effect_remembers_its_state_and_no_zones() {
    let (hw, _, memory) = rear_rig();
    hw.rear_set(&settings(RearMode::Neon, "112233", 40)).unwrap();
    let remembered = memory.0.lock().unwrap();
    assert_eq!(remembered.len(), 1);
    assert_eq!(remembered[0].0, "rear_state");
    assert_eq!(remembered[0].1["mode"], "neon");
    assert_eq!(remembered[0].1["zones"], Value::Null);
}

#[test]
fn live_changes_are_written_without_a_read_back() {
    let spy = Spy::passthrough();
    let (hw, sleeps) = spy_rig(&spy);
    let r = hw.rear_set(&RearSettings { verify: false, ..settings(RearMode::Wave, "ff00ff", 55) }).unwrap();
    assert_eq!(spy.sent().len(), 1, "one write, nothing else");
    assert!(sleeps.lock().unwrap().is_empty());
    assert_eq!((r.raw.clone(), r.direction, r.brightness, r.speed), (None, None, 55, 3));
    let json = serde_json::to_value(&r).unwrap();
    assert_eq!(json, json!({"mode": "wave", "speed": 3, "brightness": 55, "color": "ff00ff"}));
    let r = hw.rear_set(&RearSettings { verify: false, ..RearSettings::default() }).unwrap();
    assert!(r.zones.is_some(), "a static write reports the parts it painted");
}

#[test]
fn bad_rear_settings_never_reach_the_firmware() {
    let spy = Spy::passthrough();
    let (hw, _) = spy_rig(&spy);
    let base = RearSettings::default();
    let red = Rgb([255, 0, 0]);
    let cases: Vec<(&str, RearSettings, &str)> = vec![
        (
            "speed 0",
            RearSettings { speed: 0, ..base.clone() },
            "speed 1..5, brightness 0..100, direction 0|1",
        ),
        (
            "speed 6",
            RearSettings { speed: 6, ..base.clone() },
            "speed 1..5, brightness 0..100, direction 0|1",
        ),
        (
            "speed 9",
            RearSettings { speed: 9, ..base.clone() },
            "speed 1..5, brightness 0..100, direction 0|1",
        ),
        (
            "speed -1",
            RearSettings { speed: -1, ..base.clone() },
            "speed 1..5, brightness 0..100, direction 0|1",
        ),
        (
            "brightness 101",
            RearSettings { brightness: 101, ..base.clone() },
            "speed 1..5, brightness 0..100, direction 0|1",
        ),
        (
            "brightness -1",
            RearSettings { brightness: -1, ..base.clone() },
            "speed 1..5, brightness 0..100, direction 0|1",
        ),
        (
            "brightness 356",
            RearSettings { brightness: 356, ..base.clone() },
            "speed 1..5, brightness 0..100, direction 0|1",
        ),
        (
            "direction 2",
            RearSettings { direction: 2, ..base.clone() },
            "speed 1..5, brightness 0..100, direction 0|1",
        ),
        (
            "direction -1",
            RearSettings { direction: -1, ..base.clone() },
            "speed 1..5, brightness 0..100, direction 0|1",
        ),
        (
            "two zones",
            RearSettings { zones: Some(vec![red, red]), ..base.clone() },
            "zones: give one colour, or three (left middle right)",
        ),
        (
            "four zones",
            RearSettings { zones: Some(vec![red; 4]), ..base.clone() },
            "zones: give one colour, or three (left middle right)",
        ),
        (
            "no zones",
            RearSettings { zones: Some(vec![]), ..base.clone() },
            "zones: give one colour, or three (left middle right)",
        ),
        (
            "zones with a moving effect",
            RearSettings { mode: RearMode::Wave, zones: Some(vec![red]), ..base.clone() },
            "different colours per part only work with the static effect",
        ),
        (
            "three zones with breathing",
            RearSettings { mode: RearMode::Breathing, zones: Some(vec![red; 3]), ..base.clone() },
            "different colours per part only work with the static effect",
        ),
    ];
    for (name, settings, message) in cases {
        assert_eq!(text(hw.rear_set(&settings).unwrap_err()), message, "{name}");
    }
    assert!(spy.sent().is_empty(), "nothing may reach the firmware: {:?}", spy.sent());
}

#[test]
fn every_valid_rear_setting_is_accepted_by_the_module() {
    // The checks above and the module's (mirrored by the simulator) must agree on the edges.
    let (hw, ..) = rig();
    for mode in RearMode::ALL {
        for (speed, brightness, direction) in [(1, 0, 0), (5, 100, 1), (3, 50, 0)] {
            let s = RearSettings { mode: *mode, speed, brightness, direction, ..RearSettings::default() };
            let r = hw.rear_set(&s).unwrap();
            assert_eq!(
                (r.mode, r.speed, r.brightness, r.direction),
                (Reading::Known(*mode), speed as u8, brightness as u8, Some(direction as u8))
            );
        }
    }
}

#[test]
fn a_rear_light_that_never_takes_the_value_is_an_error() {
    let spy = Spy::new(|c, _| {
        (c == "get_rear").then(|| Ok("ok raw=00000300000100aec703000000000000\n".to_string()))
    });
    let (hw, _) = spy_rig(&spy);
    let err = hw.rear_set(&settings(RearMode::Wave, "ffffff", 60)).unwrap_err();
    assert_eq!(err.to_string(), "rear light readback mismatch (wanted (wave, 60), last read (static, 0))");
}

#[test]
fn rear_replies_that_make_no_sense_are_errors_not_panics() {
    let get = |raw: &'static str| {
        let spy = Spy::new(move |c, _| (c == "get_rear").then(|| Ok(format!("ok raw={raw}\n"))));
        let (hw, _) = spy_rig(&spy);
        hw.rear_get()
    };
    for bad in [
        "",
        "00",
        "0000000000000000",
        "000102030405060",
        "zz0102030405060708",
        "00000000000000\u{e9}\u{e9}",
        "0\u{e9}",
    ] {
        assert!(matches!(get(bad), Err(HwError::Firmware(FirmwareError::BadReply { .. }))), "{bad:?}");
    }
    let spy = Spy::new(|c, _| (c == "get_rear").then(|| Ok("ok\n".to_string())));
    let (hw, _) = spy_rig(&spy);
    assert!(hw.rear_get().is_err());
    // Nine bytes (status plus eight) is the shortest reply that carries the colour.
    let r = get("000705640001aabbcc").unwrap();
    assert_eq!(
        (r.mode, r.speed, r.brightness, r.color),
        (Reading::Known(RearMode::Twinkling), 5, 100, Rgb([0xaa, 0xbb, 0xcc]))
    );
    assert_eq!(get("00FF05640001AABBCC").unwrap().raw.as_deref(), Some("00ff05640001aabbcc"));
}

#[test]
fn an_unknown_rear_effect_is_shown_as_its_number() {
    let spy = Spy::new(|c, _| (c == "get_rear").then(|| Ok("ok raw=00090564000100aabbcc\n".to_string())));
    let (hw, _) = spy_rig(&spy);
    let r = hw.rear_get().unwrap();
    assert_eq!(r.mode, Reading::Unknown(9));
    assert_eq!(serde_json::to_value(&r).unwrap()["mode"], json!(9));
    let r = Rear { mode: Reading::Known(RearMode::Neon), ..r };
    assert_eq!(serde_json::to_value(&r).unwrap()["mode"], json!("neon"));
}

#[test]
fn the_initial_rear_state_decodes() {
    let (hw, ..) = rig();
    let r = hw.rear_get().unwrap();
    assert_eq!(
        serde_json::to_value(&r).unwrap(),
        json!({
            "raw": "00000300000100aec703000000000000", "mode": "static", "speed": 3, "brightness": 0,
            "direction": 1, "color": "00aec7"
        })
    );
}

// ---- the probe ------------------------------------------------------------------------------

#[test]
fn the_probe_only_ever_sends_reads() {
    let spy = Spy::passthrough();
    let report = probe_firmware(spy.as_ref());
    let sent = spy.sent();
    // Exactly the list, in order: nothing else, nothing twice.
    assert_eq!(sent, READ_ONLY.to_vec());
    assert_eq!(report.entries().len(), READ_ONLY.len());
    assert_eq!(report.answered(), READ_ONLY.len());
    assert_eq!(report.entries().iter().map(|e| e.command).collect::<Vec<_>>(), READ_ONLY.to_vec());
    assert!(sent.iter().all(|c| c == "version" || c == "caps" || c == "sensors" || c.starts_with("get_")));
    assert!(sent.iter().all(|c| !c.starts_with("set") && !c.contains(" set") && !c.contains("force")));
    // Nothing of the firmware changed.
    assert_eq!(spy.inner.snapshot(), Simulator::new().snapshot());
}

#[test]
fn the_probe_report_is_what_the_python_probe_printed() {
    // Captured from `python3 -m truesense --json probe` under TRUESENSE_SIM=1 (its "firmware" object).
    let python: Value = serde_json::from_str(
        r#"{"version": {"ok": true, "value": {"version": "sim"}}, "caps": {"ok": true, "value": {"game": "1", "apge": "1", "battery": "1"}},
        "sensors": {"ok": true, "value": {"cpu": "50", "sys": "52", "gpu": "48", "cpu_rpm": "0", "gpu_rpm": "0"}},
        "get_fan": {"ok": true, "value": {"behavior": "0x41", "cpu": "30", "gpu": "30"}},
        "get_mode": {"ok": true, "value": {"mode": "6"}}, "get_modes": {"ok": true, "value": {"mask": "0x73"}},
        "get_overdrive": {"ok": true, "value": {"overdrive": "1", "raw": "0x0"}}, "get_boot": {"ok": true, "value": {"boot": "0"}},
        "get_kbtimeout": {"ok": true, "value": {"kbtimeout": "1", "raw": "0x0"}}, "get_usb": {"ok": true, "value": {"usb": "30", "raw": "0"}},
        "get_battery 1": {"ok": true, "value": {"battery": "0", "list": "3"}}, "get_battery 2": {"ok": true, "value": {"battery": "0", "list": "3"}},
        "get_gpu": {"ok": true, "value": {"gpu": "2"}}, "get_rear": {"ok": true, "value": {"raw": "00000300000100aec703000000000000"}}}"#,
    )
    .unwrap();
    let report = probe_firmware(&Simulator::new());
    assert_eq!(serde_json::to_value(&report).unwrap(), python);
    // The reply fields keep the module's order, which a `Value` cannot show.
    let text = serde_json::to_string(&report).unwrap();
    assert!(text.starts_with(r#"{"version":{"ok":true,"value":{"version":"sim"}},"caps":"#), "{text}");
    assert!(
        text.contains(
            r#""sensors":{"ok":true,"value":{"cpu":"50","sys":"52","gpu":"48","cpu_rpm":"0","gpu_rpm":"0"}}"#
        ),
        "{text}"
    );
}

#[test]
fn every_probe_command_is_a_reader_of_the_module() {
    for command in READ_ONLY {
        let name = command.split(' ').next().unwrap();
        assert!(READ_ONLY_COMMANDS.contains(&name), "{command}");
        assert!(crate::firmware::ReadOnly::allows(command), "{command}");
        assert!(Simulator::new().execute(command).is_ok(), "{command}: the module must accept it");
    }
    let mut unique = READ_ONLY.to_vec();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), READ_ONLY.len());
}

#[test]
fn a_firmware_that_does_not_answer_is_reported_not_fatal() {
    let spy = Spy::new(|_, _| Some(Ok("err no such command\n".to_string())));
    let report = probe_firmware(spy.as_ref());
    assert_eq!(report.entries().len(), READ_ONLY.len());
    assert_eq!(report.answered(), 0);
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["get_fan"], json!({"ok": false, "error": "no such command"}));

    let gone = Spy::new(|_, _| Some(Err(io::ErrorKind::NotFound)));
    assert_eq!(probe_firmware(gone.as_ref()).answered(), 0);

    let long = Spy::new(|_, _| Some(Ok(format!("err {}\n", "x".repeat(500)))));
    let json = serde_json::to_value(probe_firmware(long.as_ref())).unwrap();
    assert_eq!(
        json["version"]["error"].as_str().unwrap().len(),
        120,
        "errors are cut like the Python report"
    );
}

// ---- errors ---------------------------------------------------------------------------------

#[test]
fn errors_carry_their_cause_and_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync + 'static>() {}
    assert_send_sync::<HwError>();
    assert_send_sync::<Hw>();
    let (hw, ..) = rig();
    let shared = hw.clone();
    std::thread::spawn(move || shared.get_mode().unwrap()).join().unwrap();

    let io = FirmwareError::Io {
        operation: "get_mode".into(),
        source: io::Error::from(io::ErrorKind::BrokenPipe),
    };
    let err = HwError::from(io);
    assert!(std::error::Error::source(&err).and_then(std::error::Error::source).is_some());
    let gone = Hw::new(DeviceKind::Kernel(PathBuf::from("/nonexistent/truesense")).open());
    let err = gone.get_mode().unwrap_err();
    assert!(matches!(err, HwError::Firmware(FirmwareError::NotLoaded { .. })));
    assert!(err.to_string().contains("kernel module is not loaded"));
}

use crate::firmware::DeviceKind;

// ---- the Linux readings ---------------------------------------------------------------------

struct FakeRunner {
    answer: Mutex<Result<Vec<u8>, io::ErrorKind>>,
    calls: Mutex<Vec<(String, Vec<String>, Duration)>>,
}

impl FakeRunner {
    fn new(answer: Result<&str, io::ErrorKind>) -> Arc<Self> {
        Arc::new(Self { answer: Mutex::new(answer.map(|s| s.as_bytes().to_vec())), calls: Mutex::default() })
    }
}

impl CommandRunner for FakeRunner {
    fn output(&self, program: &str, args: &[&str], timeout: Duration) -> io::Result<Vec<u8>> {
        self.calls.lock().unwrap().push((
            program.into(),
            args.iter().map(|a| a.to_string()).collect(),
            timeout,
        ));
        self.answer.lock().unwrap().clone().map_err(|k| io::Error::new(k, "scripted"))
    }
}

struct TempTree(PathBuf);

impl TempTree {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("ts-platform-hw-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn write(&self, path: &str, content: &str) {
        let full = self.0.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, content).unwrap();
    }

    fn sensors(&self, runner: Arc<FakeRunner>) -> LinuxSensors {
        LinuxSensors::new(self.0.clone(), runner)
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(Path::new(&self.0));
    }
}

#[test]
fn without_a_package_input_the_cpu_temperature_is_the_hottest_coretemp_sensor() {
    let tree = TempTree::new("cpu");
    let sensors = tree.sensors(FakeRunner::new(Err(io::ErrorKind::NotFound)));
    assert_eq!(sensors.cpu_temp(), None, "no hwmon: no temperature");
    tree.write("sys/class/hwmon/hwmon3/name", "coretemp\n");
    tree.write("sys/class/hwmon/hwmon3/temp1_input", "55000\n");
    tree.write("sys/class/hwmon/hwmon3/temp2_input", "71500\n");
    tree.write("sys/class/hwmon/hwmon3/temp3_input", "garbage\n");
    tree.write("sys/class/hwmon/hwmon3/temp4_label", "99999\n");
    tree.write("sys/class/hwmon/hwmon1/name", "nvme\n");
    tree.write("sys/class/hwmon/hwmon1/temp1_input", "99000\n");
    tree.write("sys/class/hwmon/hwmon5/temp1_input", "98000\n"); // no name file
    tree.write("sys/class/hwmon/other/name", "coretemp\n");
    tree.write("sys/class/hwmon/other/temp1_input", "97000\n");
    assert_eq!(sensors.cpu_temp(), Some(71.5), "the hottest coretemp sensor, not another device");
    tree.write("sys/class/hwmon/hwmon3/temp2_input", "60000\n");
    assert_eq!(sensors.cpu_temp(), Some(60.0), "values are read again every time");
    // A sensor that goes away makes the scan run again, which finds the new one.
    tree.write("sys/class/hwmon/hwmon4/name", "coretemp");
    tree.write("sys/class/hwmon/hwmon4/temp1_input", "80000");
    let fresh = tree.sensors(FakeRunner::new(Err(io::ErrorKind::NotFound)));
    assert_eq!(fresh.cpu_temp(), Some(80.0), "any coretemp hwmon counts");
    tree.write("sys/class/hwmon/hwmon4/temp1_input", "-5000");
    assert_eq!(fresh.cpu_temp(), Some(60.0));
}

#[test]
fn with_a_package_input_only_that_input_is_read() {
    let tree = TempTree::new("cpu-package");
    let sensors = tree.sensors(FakeRunner::new(Err(io::ErrorKind::NotFound)));
    tree.write("sys/class/hwmon/hwmon8/name", "coretemp\n");
    tree.write("sys/class/hwmon/hwmon8/temp1_label", "Package id 0\n");
    tree.write("sys/class/hwmon/hwmon8/temp1_input", "52000\n");
    tree.write("sys/class/hwmon/hwmon8/temp2_label", "Core 0\n");
    tree.write("sys/class/hwmon/hwmon8/temp2_input", "90000\n");
    assert_eq!(sensors.cpu_temp(), Some(52.0), "the cores are not read: each read wakes a core");
    // The device is renumbered (module reloaded): the failed read triggers a new scan.
    std::fs::rename(tree.0.join("sys/class/hwmon/hwmon8"), tree.0.join("sys/class/hwmon/hwmon9")).unwrap();
    tree.write("sys/class/hwmon/hwmon9/temp1_input", "53000\n");
    assert_eq!(sensors.cpu_temp(), Some(53.0));
}

#[test]
fn mains_power_is_the_first_supply_that_reports() {
    let tree = TempTree::new("ac");
    let sensors = tree.sensors(FakeRunner::new(Err(io::ErrorKind::NotFound)));
    assert!(sensors.on_ac(), "no supply information means mains");
    tree.write("sys/class/power_supply/ADP1/online", "1\n");
    assert!(sensors.on_ac());
    tree.write("sys/class/power_supply/ADP0/online", "0\n");
    assert!(!sensors.on_ac(), "ADP0 comes before ADP1 and says battery");
    tree.write("sys/class/power_supply/ACAD/online", "1\n");
    assert!(sensors.on_ac(), "ACAD comes first of all");
    tree.write("sys/class/power_supply/ACAD/online", "weird");
    assert!(!sensors.on_ac(), "a supply that does not say 1 is not mains");
}

#[test]
fn the_gpu_temperature_comes_from_nvidia_smi_and_only_if_it_is_a_number() {
    let tree = TempTree::new("gpu");
    let runner = FakeRunner::new(Ok("63\n"));
    let sensors = tree.sensors(Arc::clone(&runner));
    assert_eq!(sensors.gpu_temp(), Some(63.0));
    let calls = runner.calls.lock().unwrap().clone();
    assert_eq!(
        calls,
        [(
            "nvidia-smi".to_string(),
            vec!["--query-gpu=temperature.gpu".to_string(), "--format=csv,noheader,nounits".to_string()],
            Duration::from_secs(3)
        )]
    );
    for (answer, want) in [
        ("  47 \n", Some(47.0)),
        ("52.5\nextra", Some(52.5)),
        ("", None),
        ("\n", None),
        ("N/A", None),
        ("NVIDIA-SMI has failed", None),
        ("nan", None),
        ("inf", None),
        ("-inf", None),
        ("6\u{3}3", None),
    ] {
        *runner.answer.lock().unwrap() = Ok(answer.as_bytes().to_vec());
        assert_eq!(sensors.gpu_temp(), want, "{answer:?}");
    }
    *runner.answer.lock().unwrap() = Ok(vec![0xff, 0xfe, b'5']);
    assert_eq!(sensors.gpu_temp(), None);
    *runner.answer.lock().unwrap() = Err(io::ErrorKind::NotFound);
    assert_eq!(sensors.gpu_temp(), None, "a missing nvidia-smi is not a temperature");
    *runner.answer.lock().unwrap() = Err(io::ErrorKind::TimedOut);
    assert_eq!(sensors.gpu_temp(), None);
}

#[test]
fn a_program_that_does_not_exist_is_an_error_not_a_hang() {
    // Only the failure path is exercised: the tests run no real process.
    let err = ProcessRunner.output("/nonexistent/nvidia-smi", &[], Duration::from_secs(1)).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::NotFound);
    assert_eq!(LinuxSensors::new("/nonexistent-root", Arc::new(ProcessRunner)).cpu_temp(), None);
    assert!(LinuxSensors::new("/nonexistent-root", Arc::new(ProcessRunner)).on_ac());
}

#[cfg(test)]
mod command_output_tests {
    use super::*;

    #[test]
    fn a_real_program_reports_status_and_stderr() {
        let out = ProcessRunner
            .run("/bin/sh", &["-c", "echo out; echo err >&2; exit 3"], Duration::from_secs(5))
            .expect("sh runs");
        assert_eq!(out.code, Some(3));
        assert_eq!(out.stdout, b"out\n");
        assert_eq!(out.stderr, b"err\n");
        assert!(!out.success());
    }

    #[test]
    fn success_means_status_zero() {
        let out = ProcessRunner.run("/bin/sh", &["-c", "exit 0"], Duration::from_secs(5)).expect("sh runs");
        assert!(out.success());
    }

    #[test]
    fn output_still_ignores_the_status() {
        let out = ProcessRunner
            .output("/bin/sh", &["-c", "echo x; exit 9"], Duration::from_secs(5))
            .expect("sh runs");
        assert_eq!(out, b"x\n");
    }

    #[test]
    fn a_runner_that_only_knows_output_reports_an_unknown_status() {
        struct OnlyOutput;
        impl CommandRunner for OnlyOutput {
            fn output(&self, _: &str, _: &[&str], _: Duration) -> io::Result<Vec<u8>> {
                Ok(b"hi".to_vec())
            }
        }
        let out = OnlyOutput.run("x", &[], Duration::from_secs(1)).expect("default run");
        assert_eq!(out, CommandOutput { code: None, stdout: b"hi".to_vec(), stderr: Vec::new() });
        assert!(!out.success());
    }

    #[test]
    fn a_program_that_hangs_is_killed() {
        let err = ProcessRunner.run("/bin/sh", &["-c", "sleep 5"], Duration::from_millis(100)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    }
}
