//! What each page reads, asks and prints: the behaviour tables of `pages`, row by row.

use ratatui::style::Color;

use super::*;
use crate::tui::data::{
    Data, DoctorRow, FanMode, FanState, Grade, KbLook, Names, ServiceState, SlowValue, SoundCard,
};
use crate::tui::light::Edits;
use crate::tui::pages::{self, Ctx};
use crate::tui::row::Id;

fn ctx<'a>(d: &'a Data, kb: &'a Edits, rear: &'a Edits) -> Ctx<'a> {
    Ctx { data: d, kb, rear, version: "9.9" }
}

fn fast(service: Option<ServiceState>, fan: Option<FanState>) -> Fast {
    Fast {
        service,
        sensors: Ok(crate::tui::data::Sensors {
            cpu_temp: 55,
            sys_temp: 40,
            gpu_temp: 50,
            cpu_fan_rpm: 2000,
            gpu_fan_rpm: 2100,
        }),
        fan: fan.ok_or_else(|| "no fan".to_owned()),
        mode: Ok("balanced".into()),
        ac: Ok(true),
        battery: None,
    }
}

#[test]
fn the_fan_mode_shown_follows_the_firmware_and_the_service() {
    let none = Edits::default();
    let state = |mode, smart_fan, curve: &str, off| {
        let svc =
            ServiceState { smart_fan, fan_curve: curve.to_owned(), fan_off: off, ..ServiceState::default() };
        let mut d = Data::default();
        d.apply_fast(fast(Some(svc), Some(FanState { mode, cpu_duty: 30, gpu_duty: 30 })));
        pages::reading(&Id::FanMode, &ctx(&d, &none, &none))
    };
    let t = |s: &str| Some(Value::text(s));
    assert_eq!(state(FanMode::Auto, false, "native", false), t("auto"));
    assert_eq!(state(FanMode::Max, false, "native", false), t("max"));
    assert_eq!(state(FanMode::Manual, false, "native", false), t("manual"));
    assert_eq!(
        state(FanMode::Manual, true, "truesense", false),
        t("smart"),
        "the service writes manual duties"
    );
    assert_eq!(state(FanMode::Auto, true, "truesense", false), t("smart"));
    assert_eq!(
        state(FanMode::Auto, true, "native", false),
        t("auto"),
        "the native curve is the firmware's own"
    );
    assert_eq!(state(FanMode::Auto, false, "truesense", true), t("off"), "the service keeps both stopped");
    assert_eq!(state(FanMode::Unknown, false, "native", false), None);
    let d = Data::default();
    assert_eq!(pages::reading(&Id::FanMode, &ctx(&d, &none, &none)), None, "nothing read, nothing shown");
}

#[test]
fn only_stopping_a_fan_and_the_gpu_mode_ask_first() {
    let t = Value::text;
    assert!(pages::guard(&Id::FanMode, &t("off")).unwrap().starts_with("Stop BOTH fans?"));
    for other in ["smart", "auto", "max", "manual"] {
        assert_eq!(pages::guard(&Id::FanMode, &t(other)), None, "{other}");
        assert_eq!(pages::guard(&Id::DashFans, &t("off")), None, "the dashboard row has no off");
    }
    assert!(pages::guard(&Id::FanCpu, &Value::Int(0)).unwrap().starts_with("Stop the CPU fan?"));
    assert!(pages::guard(&Id::FanGpu, &Value::Int(0)).unwrap().starts_with("Stop the GPU fan?"));
    for duty in [20, 45, 100] {
        assert_eq!(pages::guard(&Id::FanCpu, &Value::Int(duty)), None);
        assert_eq!(pages::guard(&Id::FanGpu, &Value::Int(duty)), None);
    }
    assert_eq!(pages::guard(&Id::KbBrightness, &Value::Int(0)), None, "zero is how a light goes off");
    assert_eq!(pages::guard(&Id::RearBrightness, &Value::Int(0)), None);
    assert_eq!(pages::guard(&Id::MicVolume, &Value::Int(0)), None);
    assert!(pages::guard(&Id::GpuMode, &t("nvidia")).unwrap().contains("reboot"));
}

#[test]
fn rows_that_need_the_service_are_unusable_without_it() {
    let none = Edits::default();
    let mut d = Data::default();
    let c = |d: &Data| pages::enabled(&Id::AutoMode, &ctx(d, &none, &none));
    assert!(!c(&d));
    d.apply_fast(fast(Some(ServiceState::default()), None));
    assert!(c(&d));
    for id in [Id::Curve, Id::SmartProfile] {
        assert!(pages::enabled(&id, &ctx(&d, &none, &none)));
    }
    assert!(!pages::enabled(&Id::ReleaseHold, &ctx(&d, &none, &none)), "nothing is held");
    assert!(!pages::enabled(&Id::ChargeFull, &ctx(&d, &none, &none)), "the 80 % limit is off or unread");
    d.apply_slow(SlowReading { key: ReadKey::Limit, value: Ok(SlowValue::Bool(true)) });
    assert!(pages::enabled(&Id::ChargeFull, &ctx(&d, &none, &none)));
    d.apply_fast(fast(Some(ServiceState { hold_left_s: 90, ..ServiceState::default() }), None));
    assert!(pages::enabled(&Id::ReleaseHold, &ctx(&d, &none, &none)));
    assert_eq!(
        pages::reading(&Id::Hold, &ctx(&d, &none, &none)),
        Some(Value::text("your pick is held for 1 min 30 s"))
    );
}

#[test]
fn lists_read_at_run_time_replace_the_initial_ones() {
    let none = Edits::default();
    let mut h = Harness::new(100, 30);
    h.goto(1);
    let curve = {
        let r = h.sel("Curve", 0);
        h.model.row(r).clone()
    };
    let mut d = Data::default();
    let initial = pages::options(&curve, &ctx(&d, &none, &none));
    assert!(initial.iter().any(|o| o.label == "native"), "the names of the store");
    d.apply_slow(SlowReading {
        key: ReadKey::Curves,
        value: Ok(SlowValue::Names(Names { names: vec!["a".into(), "b".into()], current: Some("b".into()) })),
    });
    let now: Vec<String> =
        pages::options(&curve, &ctx(&d, &none, &none)).into_iter().map(|o| o.label).collect();
    assert_eq!(now, ["a", "b"]);
    assert_eq!(pages::reading(&Id::Curve, &ctx(&d, &none, &none)), Some(Value::text("b")));
}

#[test]
fn values_are_printed_the_way_the_python_screen_printed_them() {
    let mut h = Harness::new(100, 30);
    h.goto(1);
    let cpu = {
        let r = h.sel("CPU fan", 0);
        h.model.row(r).clone()
    };
    assert_eq!(pages::show(&cpu, &Value::Int(0)), "OFF");
    assert_eq!(pages::show(&cpu, &Value::Int(45)), "45 %");
    h.goto(3);
    let kb = {
        let r = h.sel("Brightness", 0);
        h.model.row(r).clone()
    };
    assert_eq!(pages::show(&kb, &Value::Int(25)), "50 %", "the keyboard scale 0-50 is shown as a percentage");
    let rear = {
        let r = h.sel("Brightness", 1);
        h.model.row(r).clone()
    };
    assert_eq!(pages::show(&rear, &Value::Int(60)), "60 %");
}

#[test]
fn the_sound_card_and_the_health_check_rows_say_what_they_know() {
    let none = Edits::default();
    let mut d = Data::default();
    assert_eq!(pages::reading(&Id::SoundCard, &ctx(&d, &none, &none)), None);
    for (card, want) in [
        (SoundCard::Ok, "output present"),
        (SoundCard::Missing, "no sound card"),
        (SoundCard::Broken("no sink".into()), "NO OUTPUT: no sink"),
        (SoundCard::Broken(String::new()), "NO OUTPUT: ?"),
    ] {
        d.apply_slow(SlowReading { key: ReadKey::Sound, value: Ok(SlowValue::Sound(card)) });
        assert_eq!(pages::reading(&Id::SoundCard, &ctx(&d, &none, &none)), Some(Value::text(want)));
    }
    let id = Id::Doctor("Audio".into());
    assert_eq!(pages::reading(&id, &ctx(&d, &none, &none)), Some(Value::text("not checked yet")));
    assert!(pages::hidden(&id, &ctx(&d, &none, &none)));
    assert!(!pages::hidden(&Id::DoctorHint, &ctx(&d, &none, &none)));
    let rows = vec![
        DoctorRow { check: "Kernel".into(), grade: Grade::Ok, detail: "fine".into(), fix: "ignored".into() },
        DoctorRow {
            check: "Audio".into(),
            grade: Grade::Warn,
            detail: "no output".into(),
            fix: "truesense audio repair".into(),
        },
    ];
    d.set(ReadKey::Doctor, SlowValue::Doctor(rows));
    assert_eq!(
        pages::reading(&id, &ctx(&d, &none, &none)),
        Some(Value::text("WARN  no output   -> truesense audio repair"))
    );
    assert_eq!(
        pages::reading(&Id::Doctor("Kernel".into()), &ctx(&d, &none, &none)),
        Some(Value::text("OK  fine")),
        "a fix is shown for WARN and FAIL only"
    );
    assert_eq!(pages::reading(&Id::Doctor("Other".into()), &ctx(&d, &none, &none)), Some(Value::text("…")));
    assert!(
        !pages::hidden(&id, &ctx(&d, &none, &none)) && pages::hidden(&Id::DoctorHint, &ctx(&d, &none, &none))
    );
}

#[test]
fn a_known_colour_is_selected_and_an_unknown_one_is_not() {
    let none = Edits::default();
    let mut d = Data::default();
    let known =
        KbLook { name: "static".into(), color: "ff0000".into(), speed: 5, brightness: 20, direction: 0 };
    d.apply_slow(SlowReading { key: ReadKey::KbLast, value: Ok(SlowValue::KbLast(known)) });
    assert_eq!(pages::reading(&Id::KbColor, &ctx(&d, &none, &none)), Some(Value::text("ff0000")));
    let custom = KbLook {
        color: "123456".into(),
        ..KbLook { name: "static".into(), color: String::new(), speed: 5, brightness: 20, direction: 0 }
    };
    d.apply_slow(SlowReading { key: ReadKey::KbLast, value: Ok(SlowValue::KbLast(custom)) });
    assert_eq!(
        pages::reading(&Id::KbColor, &ctx(&d, &none, &none)),
        None,
        "a custom colour matches no option"
    );
}

#[test]
fn every_page_has_its_controls() {
    let want: [(usize, &[&str]); 6] = [
        (1, &["Fan control", "CPU fan", "GPU fan", "Curve"]),
        (2, &["Battery limit", "GPU mode", "USB charging", "Smart profile"]),
        (3, &["Keyboard", "Light bar", "Brightness", "Direction"]),
        (4, &["Acer DTS sound", "Microphone", "Fix sound", "DTS sound mode"]),
        (5, &["Profiles", "Looks"]),
        (6, &["Health check", "Version"]),
    ];
    let mut h = Harness::new(110, 60);
    for (page, words) in want {
        h.goto(page);
        let text = h.text();
        for w in words {
            assert!(text.contains(w), "page {page} lacks {w:?}:\n{text}");
        }
    }
}

#[test]
fn the_audio_page_shows_what_the_sound_system_reports() {
    let mut h = Harness::new(110, 40);
    h.goto(4);
    h.model.data.apply_slow(SlowReading {
        key: ReadKey::DtsOutputs,
        value: Ok(SlowValue::Dts(vec![crate::tui::data::DtsOutput {
            label: "Headset PC38X".into(),
            profile: "Music".into(),
            current: true,
        }])),
    });
    h.model.data.apply_slow(SlowReading { key: ReadKey::Sound, value: Ok(SlowValue::Sound(SoundCard::Ok)) });
    let text = h.text();
    assert!(text.contains("Headset PC38X: Music") && text.contains("output present"), "{text}");
    let mode = h.sel("DTS sound mode", 0);
    assert_eq!(h.value(mode), Some(Value::text("Music")), "the profile of the output playing");
}

#[test]
fn the_sound_repair_reports_what_it_did() {
    for (done, want) in [
        (
            Done::Repair { fixed: true, changed: true, steps: vec!["restarted WirePlumber".into()] },
            "Sound output repaired: restarted WirePlumber",
        ),
        (
            Done::Repair { fixed: true, changed: false, steps: vec![] },
            "The sound output is there: nothing to repair",
        ),
        (
            Done::Repair { fixed: false, changed: false, steps: vec!["no card".into()] },
            "Could not repair: no card",
        ),
    ] {
        let mut h = Harness::new(100, 40);
        h.goto(4);
        h.sel("No sound at all?", 0);
        let effects = h.model.update(h.now, Msg::Input(Input::Key(Key::Enter)));
        let ticket = effects
            .iter()
            .find_map(|e| if let Effect::Write(w) = e { Some(w.ticket) } else { None })
            .expect("a write");
        h.feed(Msg::Written { ticket, result: Ok(done) });
        assert_eq!(h.model.msg.text, want);
    }
}

#[test]
fn dts_everywhere_says_what_it_did_or_that_nothing_was_needed() {
    for (text, want) in
        [("", "DTS: DTS is on for every output"), ("Speakers; Headset", "DTS: Speakers; Headset")]
    {
        let mut h = Harness::new(100, 40);
        h.goto(4);
        h.sel("A headset still sounds flat?", 0);
        let effects = h.model.update(h.now, Msg::Input(Input::Key(Key::Enter)));
        let ticket = effects
            .iter()
            .find_map(|e| if let Effect::Write(w) = e { Some(w.ticket) } else { None })
            .expect("a write");
        h.feed(Msg::Written { ticket, result: Ok(Done::Text(text.into())) });
        assert_eq!(h.model.msg.text, want);
    }
}

#[test]
fn the_profile_list_matches_the_backends_profiles() {
    let mut h = Harness::new(110, 40);
    h.goto(5);
    let text = h.text();
    let catalog = h.backend.catalog();
    assert!(catalog.profiles.len() >= 2 && !catalog.looks.is_empty());
    for (name, _) in &catalog.profiles {
        assert!(text.contains(name.as_str()), "profile {name} missing:\n{text}");
    }
    h.sel("gaming", 0);
    h.key(Key::Enter);
    assert_eq!(h.model.msg.text, "Profile gaming applied");
    assert_eq!(h.rig.bundles.applied(), vec!["gaming".to_owned()]);
}

#[test]
fn the_health_check_is_hidden_until_it_has_run_and_then_coloured() {
    let mut h = Harness::new(110, 60);
    h.goto(6);
    let titles: Vec<String> = h
        .model
        .current()
        .rows
        .iter()
        .filter_map(|r| if let Id::Doctor(t) = &r.id { Some(t.clone()) } else { None })
        .collect();
    assert!(titles.len() >= 2, "{titles:?}");
    let text = h.text();
    assert!(
        text.contains("not run yet") && !text.contains(titles[0].as_str()),
        "results shown before the check:\n{text}"
    );
    let rows = vec![
        DoctorRow { check: titles[0].clone(), grade: Grade::Ok, detail: "fine".into(), fix: String::new() },
        DoctorRow {
            check: titles[1].clone(),
            grade: Grade::Fail,
            detail: "silent".into(),
            fix: "repair it".into(),
        },
    ];
    h.model.data.set(ReadKey::Doctor, SlowValue::Doctor(rows));
    let buf = h.draw();
    let text = buffer_text(&buf);
    assert!(
        text.contains("fine") && text.contains("-> repair it") && !text.contains("not run yet"),
        "{text}"
    );
    let colour_of = |word: &str| {
        let (y, line) = text.lines().enumerate().find(|(_, l)| l.contains(word)).expect("a line");
        let x = line[..line.find(word).expect("the word")].chars().count();
        buf[(x as u16, y as u16)].fg
    };
    assert_eq!(colour_of("OK  fine"), Color::Green);
    assert_eq!(colour_of("FAIL  silent"), Color::Red);
}

#[test]
fn running_the_health_check_fills_the_page() {
    let mut h = Harness::new(110, 60);
    h.goto(6);
    h.sel("Run the check", 0);
    h.key(Key::Enter);
    assert_eq!(h.model.msg.text, "Check finished");
    assert!(!h.model.data.doctor().is_empty());
    let text = h.text();
    assert!(!text.contains("not run yet"), "{text}");
}

#[test]
fn open_the_window_runs_the_hook_and_says_so_without_one() {
    let mut h = Harness::new(100, 30);
    h.goto(6);
    h.sel("Open the window", 0);
    h.key(Key::Enter);
    assert_eq!(h.window_opened.load(Ordering::SeqCst), 1);
    assert_eq!(h.message(), ("Opening the window".to_owned(), St::Good));
    let dir = TempDir::new();
    let none = ApiBackend::new(Arc::new(Api::new(Services::simulated(&dir.0).expect("services").0)), None);
    assert!(none.execute(&Op::OpenWindow).unwrap_err().contains("truesense gui"));
}
