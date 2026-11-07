//! The only glue to the application layer, against the simulated laptop: what is read, what a
//! write does, and that a refused or malformed write is an error, never a panic.

use serde_json::json;
use ts_api::api::testing::Reply;

use super::*;
use crate::tui::backend::{FanAction, FanReq, VoiceInput};
use crate::tui::data::{FanMode, KbLook, SlowValue};

fn plain() -> Harness {
    Harness::new(100, 30)
}

fn read(h: &Harness, key: ReadKey) -> Result<SlowValue, String> {
    h.backend.read(key).value
}

#[test]
fn the_catalog_lists_what_the_pages_offer() {
    let c = plain().backend.catalog();
    assert!(c.effects.iter().any(|e| e == "off") && c.effects.iter().any(|e| e == "wave"), "{:?}", c.effects);
    assert_eq!(c.rear_modes, ["static", "breathing", "neon", "wave", "twinkling"]);
    assert_eq!((c.max_brightness, c.min_speed, c.max_speed), (50, 1, 12));
    assert!(c.curves.iter().any(|n| n == "native") && c.smart_profiles.iter().any(|n| n == "native"));
    assert!(c.looks.iter().any(|n| n == "native"), "{:?}", c.looks);
    assert_eq!(c.profiles.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["battery", "gaming"]);
    assert!(
        c.doctor_checks.len() >= 10 && c.doctor_checks.iter().any(|t| t == "Audio"),
        "{:?}",
        c.doctor_checks
    );
}

#[test]
fn without_the_service_the_dashboard_reads_the_hardware() {
    let h = plain();
    let f = h.backend.fast();
    assert!(f.service.is_none());
    let s = f.sensors.expect("sensors");
    assert_eq!((s.cpu_temp, s.sys_temp, s.gpu_temp), (50, 52, 48));
    let fan = f.fan.expect("fan");
    assert_eq!((fan.mode, fan.cpu_duty, fan.gpu_duty), (FanMode::Auto, 30, 30));
    assert_eq!(f.mode.as_deref(), Ok("eco"));
    assert_eq!(f.ac, Ok(true));
    assert_eq!(f.battery.map(|b| b.percent), Some(80));
}

#[test]
fn a_machine_without_a_battery_shows_none() {
    let h = Harness::with(100, 30, Setup { battery: false, ..Setup::default() });
    assert_eq!(h.backend.fast().battery, None);
}

#[test]
fn with_the_service_the_dashboard_prefers_what_it_already_read() {
    let h = Harness::with_service(100, 30);
    let f = h.backend.fast();
    let svc = f.service.expect("the service");
    assert_eq!(
        (svc.auto_mode.as_str(), svc.smart_profile.as_str(), svc.cpu_load, svc.gpu_watts),
        ("smart", "steady", Some(12), Some(9))
    );
    assert_eq!(
        (svc.activity.as_str(), svc.fan_off, svc.smart_fan, svc.hold_left_s),
        ("quiet desktop", false, false, 0)
    );
    let s = f.sensors.expect("sensors");
    assert_eq!(
        (s.cpu_temp, s.gpu_temp, s.sys_temp, s.cpu_fan_rpm),
        (55, 50, 40, 2400),
        "the service's own reading, not the simulator's"
    );
    assert_eq!(f.mode.as_deref(), Ok("performance"));
    assert_eq!(f.fan.expect("the fan duty still comes from the hardware").cpu_duty, 30);
}

#[test]
fn an_unknown_power_mode_from_the_service_is_read_from_the_hardware() {
    let h = Harness::with_service(100, 30);
    h.rig.daemon.answer_ok(
        "state",
        daemon_state(
            json!({}),
            json!({"tcpu": 55, "tgpu": 50, "rpm1": 1, "rpm2": 1, "power_mode": "?", "ac": false}),
            0,
        ),
    );
    let f = h.backend.fast();
    assert_eq!(f.mode.as_deref(), Ok("eco"), "a '?' from the service means it does not know");
    assert_eq!(f.ac, Ok(false), "the charger comes from the service when it says");
}

#[test]
fn a_service_that_does_not_answer_is_the_same_as_none() {
    let h = Harness::with_service(100, 30);
    h.rig.daemon.answer("state", Reply::Error("busy".into()));
    let f = h.backend.fast();
    assert!(f.service.is_none());
    assert!(f.sensors.is_ok(), "the hardware is read instead");
}

#[test]
fn the_settings_of_a_page_are_read_from_the_firmware() {
    let h = plain();
    let table: [(ReadKey, SlowValue); 6] = [
        (ReadKey::Limit, SlowValue::Bool(false)),
        (ReadKey::Calibration, SlowValue::Bool(false)),
        (ReadKey::UsbCharge, SlowValue::Int(30)),
        (ReadKey::GpuMode, SlowValue::Text("nvidia".into())),
        (ReadKey::Overdrive, SlowValue::MaybeBool(Some(true))),
        (ReadKey::BootSound, SlowValue::Bool(false)),
    ];
    for (key, want) in table {
        assert_eq!(read(&h, key), Ok(want), "{key:?}");
    }
    assert!(matches!(read(&h, ReadKey::KbTimeout), Ok(SlowValue::MaybeBool(_))));
    assert!(matches!(read(&h, ReadKey::KbLast), Ok(SlowValue::KbLast(_))));
    assert!(matches!(read(&h, ReadKey::KbNow), Ok(SlowValue::Int(_))));
    assert!(matches!(read(&h, ReadKey::SavedLightings), Ok(SlowValue::List(_))));
    let Ok(SlowValue::Rear(rear)) = read(&h, ReadKey::Rear) else { panic!("the light bar") };
    assert_eq!((rear.mode.as_str(), rear.speed, rear.brightness), ("static", 3, 0));
    assert_eq!(read(&h, ReadKey::Doctor).unwrap_err(), "the health check is run on request");
    assert!(read(&h, ReadKey::ModuleVersion).is_ok());
}

#[test]
fn the_lists_that_need_the_service_say_so_without_it() {
    let h = plain();
    // Without the service the lists are still the store's names, with no current one.
    let Ok(SlowValue::Names(n)) = read(&h, ReadKey::Curves) else { panic!("curves") };
    assert!(n.names.iter().any(|x| x == "native") && n.current.is_none());
    assert!(read(&h, ReadKey::ChargeFull).is_err(), "charge to 100 % once is the service's");
    let h = Harness::with_service(100, 30);
    let Ok(SlowValue::Names(n)) = read(&h, ReadKey::Curves) else { panic!("curves") };
    assert_eq!(n.current.as_deref(), Some("native"));
    let Ok(SlowValue::Names(p)) = read(&h, ReadKey::SmartProfiles) else { panic!("profiles") };
    assert_eq!(p.current.as_deref(), Some("steady"));
    assert_eq!(read(&h, ReadKey::ChargeFull), Ok(SlowValue::Bool(false)));
}

#[test]
fn writes_reach_the_firmware_and_are_verified() {
    let h = plain();
    let b = &h.backend;
    assert_eq!(b.execute(&Op::PowerMode { mode: "turbo".into(), hold_minutes: 30.0 }), Ok(Done::Unit));
    assert_eq!(b.execute(&Op::BatteryLimit(true)), Ok(Done::Unit));
    assert_eq!(b.execute(&Op::Calibration(true)), Ok(Done::Unit));
    assert_eq!(b.execute(&Op::UsbCharge(10)), Ok(Done::Unit));
    assert_eq!(b.execute(&Op::GpuMode("auto".into())), Ok(Done::Unit));
    assert_eq!(b.execute(&Op::Overdrive(false)), Ok(Done::Unit));
    assert_eq!(b.execute(&Op::BootSound(true)), Ok(Done::Unit));
    assert_eq!(b.execute(&Op::KbTimeout(false)), Ok(Done::Unit));
    let s = h.rig.sim.snapshot();
    assert_eq!(
        (s.mode, s.battery, s.usb, s.gpu, s.overdrive, s.boot, s.kbtimeout),
        (5, [1, 1], 10, 3, 0, 1, 0)
    );
    assert_eq!(
        b.execute(&Op::Fan(FanReq { action: FanAction::Manual, cpu: 55, gpu: 40, confirm: false })),
        Ok(Done::Unit)
    );
    assert_eq!(h.rig.sim.snapshot().duty, [55, 40]);
    assert_eq!(b.execute(&Op::Fan(FanReq::mode(FanAction::Max))), Ok(Done::Unit));
    assert_eq!(format!("{:?}", h.rig.sim.snapshot().fan), "Max");
    assert_eq!(b.execute(&Op::Fan(FanReq::mode(FanAction::Auto))), Ok(Done::Unit));
    assert_eq!(format!("{:?}", h.rig.sim.snapshot().fan), "Auto");
}

#[test]
fn the_lights_are_written_from_a_whole_look() {
    let h = plain();
    let look = KbLook { name: "wave".into(), color: "ff0000".into(), speed: 7, brightness: 20, direction: 1 };
    assert_eq!(h.backend.execute(&Op::Keyboard(look)), Ok(Done::Unit));
    assert_eq!(h.api().keyboard_brightness().unwrap(), 20);
    let off = KbLook { name: "off".into(), ..KbLook::default() };
    assert_eq!(h.backend.execute(&Op::Keyboard(off)), Ok(Done::Unit));
    let rear = crate::tui::data::RearLook {
        mode: "wave".into(),
        color: "00ff40".into(),
        speed: 4,
        brightness: 70,
        direction: 0,
    };
    assert_eq!(h.backend.execute(&Op::Rear { look: rear, keep_zones: false }), Ok(Done::Unit));
    let r = h.api().rear().unwrap();
    assert_eq!(
        (r.mode.to_string(), r.speed, r.brightness, r.color.to_string()),
        ("wave".to_owned(), 4, 70, "00ff40".to_owned())
    );
}

#[test]
fn the_service_writes_go_to_the_service() {
    let h = Harness::with_service(100, 30);
    let b = &h.backend;
    assert_eq!(b.execute(&Op::AutoMode("native".into())), Ok(Done::Unit));
    assert_eq!(b.execute(&Op::ReleaseHold), Ok(Done::Unit));
    assert_eq!(b.execute(&Op::SmartProfile("native".into())), Ok(Done::Unit));
    assert_eq!(b.execute(&Op::Curve("native".into())), Ok(Done::Unit));
    assert_eq!(b.execute(&Op::ChargeFull { start: true }), Ok(Done::Unit));
    assert_eq!(b.execute(&Op::Fan(FanReq::mode(FanAction::Smart))), Ok(Done::Unit));
    let commands = h.rig.daemon.commands();
    for want in ["set", "hold", "smart_profile", "fan_curve", "battery_full", "fan"] {
        assert!(commands.iter().any(|c| c == want), "{want} missing from {commands:?}");
    }
    let hold = h.rig.daemon.last_args("hold").expect("hold");
    assert_eq!(hold.get("minutes"), Some(&serde_json::Value::Null));
}

#[test]
fn what_needs_the_service_is_an_error_without_it_not_a_guess() {
    let h = plain();
    for op in [
        Op::AutoMode("smart".into()),
        Op::ReleaseHold,
        Op::SmartProfile("native".into()),
        Op::Curve("native".into()),
        Op::ChargeFull { start: true },
        Op::Fan(FanReq::mode(FanAction::Smart)),
        Op::Fan(FanReq { action: FanAction::Manual, cpu: 0, gpu: 30, confirm: true }),
        Op::Fan(FanReq { confirm: true, ..FanReq::mode(FanAction::Off) }),
    ] {
        assert!(h.backend.execute(&op).is_err(), "{op:?}");
    }
    assert_eq!(h.sim().0, [30, 30], "no fan was stopped");
}

#[test]
fn malformed_writes_are_errors_never_panics() {
    let h = plain();
    for op in [
        Op::PowerMode { mode: "ludicrous".into(), hold_minutes: 30.0 },
        Op::AutoMode("sometimes".into()),
        Op::GpuMode("both".into()),
        Op::UsbCharge(15),
        Op::Output("ears".into()),
        Op::MicVolume(-5),
        Op::Voice { input: VoiceInput::Headset, mode: "loud".into() },
        Op::Rear {
            look: crate::tui::data::RearLook {
                mode: "disco".into(),
                color: "ffffff".into(),
                speed: 3,
                brightness: 10,
                direction: 0,
            },
            keep_zones: false,
        },
        Op::Rear {
            look: crate::tui::data::RearLook {
                mode: "wave".into(),
                color: "nothex".into(),
                speed: 3,
                brightness: 10,
                direction: 0,
            },
            keep_zones: false,
        },
        Op::Keyboard(KbLook {
            name: "disco".into(),
            color: "ffffff".into(),
            speed: 5,
            brightness: 10,
            direction: 0,
        }),
        Op::Profile("no-such-profile".into()),
        Op::Look("no-such-look".into()),
    ] {
        let e = h.backend.execute(&op).unwrap_err();
        assert!(!e.is_empty(), "{op:?}");
    }
}

#[test]
fn the_health_check_and_the_profiles_answer_through_the_backend() {
    let h = plain();
    let Ok(Done::Doctor(rows)) = h.backend.execute(&Op::RunDoctor) else { panic!("the health check") };
    assert_eq!(rows.len(), h.backend.catalog().doctor_checks.len());
    assert_eq!(h.backend.execute(&Op::Profile("gaming".into())), Ok(Done::Unit));
    assert_eq!(h.rig.bundles.applied(), vec!["gaming".to_owned()]);
    assert!(matches!(h.backend.execute(&Op::RepairSound), Ok(Done::Repair { .. }) | Err(_)));
    assert_eq!(h.backend.execute(&Op::MicMute(true)), Ok(Done::Unit));
    assert!(h.backend.execute(&Op::OpenWindow).is_ok());
    assert_eq!(h.window_opened.load(Ordering::SeqCst), 1);
}

#[test]
fn favourites_are_listed_with_their_existence_and_applied_through_the_backend() {
    use ts_api::api::FavouriteKind;
    let h = plain();
    assert!(h.backend.catalog().favourites.is_empty());
    h.api().favourite_add(FavouriteKind::Profile, "gaming").unwrap();
    h.api().favourite_add(FavouriteKind::Look, "native").unwrap();
    assert_eq!(
        h.backend.catalog().favourites,
        vec![
            (FavouriteKind::Profile, "gaming".to_owned(), true),
            (FavouriteKind::Look, "native".to_owned(), true)
        ]
    );
    assert_eq!(h.backend.execute(&Op::Favourite(FavouriteKind::Profile, "gaming".into())), Ok(Done::Unit));
    assert_eq!(h.rig.bundles.applied(), vec!["gaming".to_owned()]);
    assert_eq!(h.backend.execute(&Op::Favourite(FavouriteKind::Look, "native".into())), Ok(Done::Unit));
    assert!(h.backend.execute(&Op::Favourite(FavouriteKind::Look, "ghost".into())).is_err());
}
