//! What the PH16-71 findings changed in the daemon (see `docs/PH16-71-NOTES.md`): the GPU
//! sampler that must not keep the card awake, a GPU temperature that is never the CPU's,
//! one write per plug event, the fans written again after a resume, and a watchdog that says
//! why it handed the fans back.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::Ordering;

use serde_json::{Value, json};
use ts_ipc::wire::Args;
use ts_platform::firmware::{Device, SimFan};
use ts_system::sensors::GpuFeed;

use super::testkit::Rig;

fn live(rig: &Rig) -> Value {
    rig.ok("state", Args::new())["live"].clone()
}

// ---- the GPU sampler -------------------------------------------------------------------------

#[test]
fn the_gpu_sampler_is_asked_for_only_while_something_smart_needs_the_gpu() {
    let rig = Rig::new(); // smart fan and Smart auto on
    rig.drive(3, 45, 45, 5.0);
    let asked = rig.sensors.gpu_streams.load(Ordering::SeqCst);
    assert!(asked > 0);
    assert_eq!(rig.sensors.gpu_releases.load(Ordering::SeqCst), 0, "wanted: never released");

    rig.ok_set_smart_fan(false);
    rig.auto_mode("native");
    rig.drive(3, 45, 45, 5.0);
    assert_eq!(
        rig.sensors.gpu_streams.load(Ordering::SeqCst),
        asked,
        "nothing smart is on: the sampler is not asked for any more"
    );
    assert!(
        rig.sensors.gpu_releases.load(Ordering::SeqCst) > 0,
        "and it is told it may end, so the card can go to sleep"
    );
    let released = rig.sensors.gpu_releases.load(Ordering::SeqCst);

    rig.ok_set_smart_fan(true);
    rig.drive(2, 45, 45, 5.0);
    assert!(rig.sensors.gpu_streams.load(Ordering::SeqCst) > asked, "asked for again on demand");
    assert_eq!(rig.sensors.gpu_releases.load(Ordering::SeqCst), released);
}

#[test]
fn the_live_state_says_what_a_zero_gpu_load_means() {
    let rig = Rig::new();
    rig.drive(2, 45, 45, 5.0);
    let l = live(&rig);
    assert_eq!(
        (l["gpu"].clone(), l["gpuw"].clone(), l["gpu_state"].clone()),
        (json!(0), json!(0), json!("unsampled")),
        "awake and nobody reading it: the 0 is unknown"
    );
    rig.sensors.set_gpu_feed(GpuFeed::Suspended);
    rig.drive(1, 45, 45, 5.0);
    let l = live(&rig);
    assert_eq!((l["gpu"].clone(), l["gpu_state"].clone()), (json!(0), json!("suspended")), "{l}");
    rig.sensors.set_gpu_feed(GpuFeed::Live);
    rig.sensors.set_gpu(Some(61.0), 37.0, 28.0, Some(rig.clock.wall_now()));
    rig.drive(1, 45, 45, 5.0);
    let l = live(&rig);
    assert_eq!(
        (l["gpu"].clone(), l["gpuw"].clone(), l["gpu_state"].clone()),
        (json!(37), json!(28), json!("live"))
    );
    rig.sensors.set_gpu_feed(GpuFeed::Absent);
    rig.drive(1, 45, 45, 5.0);
    assert_eq!(live(&rig)["gpu_state"], json!("absent"));
}

#[test]
fn the_displayed_gpu_temperature_is_never_the_cpus() {
    let rig = Rig::new();
    // The firmware gives no GPU temperature (0) and nvidia-smi is not reading: the card is
    // asleep, or there is none.
    rig.drive(10, 70, 0, 90.0);
    let l = live(&rig);
    assert_eq!(l["tgpu"], json!(0), "no reading is 0, not the CPU's 70: {l}");
    assert_eq!(l["tgpu_real"], json!(false));
    assert_eq!(rig.snap().fan, SimFan::Manual, "the fan curve still decides from some temperature");
    assert!(rig.snap().duty[1] >= 20, "{:?}", rig.snap().duty);

    // The card answers: the first real reading is shown as it is, not blended with the CPU's.
    rig.drive(1, 70, 62, 90.0);
    let l = live(&rig);
    assert_eq!((l["tgpu"].clone(), l["tgpu_real"].clone()), (json!(62), json!(true)), "{l}");

    // And it goes to sleep again.
    rig.drive(1, 70, 0, 90.0);
    let l = live(&rig);
    assert_eq!((l["tgpu"].clone(), l["tgpu_real"].clone()), (json!(0), json!(false)), "{l}");
}

#[test]
fn the_fan_curve_keeps_its_cpu_fallback_when_the_gpu_gives_no_temperature() {
    // The same heavy CPU load with and without a GPU reading: the GPU fan must not be left
    // behind because the GPU temperature is unknown.
    let blind = Rig::new();
    blind.drive(15, 70, 0, 90.0);
    let sighted = Rig::new();
    sighted.drive(15, 70, 70, 90.0);
    assert_eq!(blind.snap().duty, sighted.snap().duty, "blind GPU side follows the CPU temperature");
}

// ---- one write per plug event ------------------------------------------------------------------

#[test]
fn native_auto_writes_the_mode_once_per_plug_event_and_never_again() {
    let rig = Rig::new();
    rig.auto_mode("native");
    rig.drive(10, 45, 45, 5.0);
    let on_ac = rig.device.count("set_mode");
    assert_eq!(rig.snap().mode, 1, "balanced on AC");
    rig.sensors.set_ac(false);
    rig.drive(120, 45, 45, 5.0);
    assert_eq!(rig.snap().mode, 6, "eco on battery");
    assert_eq!(
        rig.device.count("set_mode"),
        on_ac + 1,
        "one write at the plug event, none in the two minutes after"
    );
    rig.sensors.set_ac(true);
    rig.drive(120, 45, 45, 5.0);
    assert_eq!(rig.snap().mode, 1);
    assert_eq!(rig.device.count("set_mode"), on_ac + 2);
}

#[test]
fn a_charger_that_bounces_does_not_write_the_mode_every_time_it_flips() {
    let rig = Rig::new();
    rig.auto_mode("native");
    rig.drive(10, 45, 45, 5.0);
    let before = rig.device.count("set_mode");
    rig.sensors.set_ac(false);
    // A contact that chatters for ten seconds: unguarded, that is a write at every flip.
    for k in 0..10 {
        rig.sensors.set_ac(k % 2 == 1);
        rig.drive(1, 45, 45, 5.0);
    }
    rig.sensors.set_ac(false);
    rig.drive(30, 45, 45, 5.0);
    let writes = rig.device.count("set_mode") - before;
    assert!(writes <= 5, "{writes} mode writes for ten flips");
    assert_eq!(rig.snap().mode, 6, "and it settles on battery");
}

// ---- resume --------------------------------------------------------------------------------------

#[test]
fn after_a_resume_the_fans_are_written_again_at_once() {
    let rig = Rig::new();
    rig.drive(8, 70, 60, 90.0);
    assert_eq!(rig.snap().fan, SimFan::Manual);
    // The firmware comes back from the sleep with its own fan control.
    rig.sim.run("set_fan_auto").unwrap();
    rig.clock.suspend(3600.0);
    rig.drive(1, 70, 60, 90.0);
    assert_eq!(rig.snap().fan, SimFan::Manual, "re-asserted on the first tick after the resume");
}

// ---- the fan watchdog -------------------------------------------------------------------------

#[test]
fn the_watchdog_says_a_silent_device_is_not_a_warm_laptop() {
    let rig = Rig::new();
    rig.drive(30, 45, 45, 2.0);
    rig.ok("fan", Args::new().with("mode", "off").with("confirm", true));
    rig.drive(3, 50, 45, 2.0);
    rig.device.sensors_dead.store(true, Ordering::SeqCst);
    rig.drive(3, 50, 45, 2.0);
    assert_eq!(rig.snap().fan, SimFan::Auto, "the fans went back to the firmware");
    assert!(rig.log.has("fans back on Auto: the device stopped answering"), "{}", rig.log.all());
    assert!(!rig.log.has("got warm"));
    assert!(
        rig.osd.shown.lock().unwrap().iter().any(|t| t == "Fans back on Auto: the device stopped answering")
    );
}

#[test]
fn a_busy_cpu_is_not_hot_by_the_gpu_line_when_the_gpu_gives_no_temperature() {
    // CPU at 85 C is busy, not hot (its own line is 92 C). The GPU's line is 82 C: without a
    // GPU reading the CPU's temperature must not be held against it.
    let blind = Rig::new();
    blind.drive(30, 85, 0, 95.0);
    assert_ne!(live(&blind)["activity"], json!("hot"), "{}", live(&blind));
    assert_ne!(blind.snap().duty, [100, 100], "no emergency full speed for a busy CPU");
    // A real GPU reading over its own line is hot, as ever.
    let sighted = Rig::new();
    sighted.drive(30, 85, 85, 95.0);
    assert_eq!(live(&sighted)["activity"], json!("hot"));
    assert_eq!(sighted.snap().duty, [100, 100]);
}

// ---- the smart curve's off switch --------------------------------------------------------------

#[test]
fn switching_the_smart_fan_off_hands_the_fans_back_instead_of_leaving_the_last_duty() {
    let rig = Rig::new();
    rig.drive(10, 75, 60, 90.0);
    assert_eq!(rig.snap().fan, SimFan::Manual, "the curve is running the fans");
    rig.ok_set_smart_fan(false);
    assert_eq!(rig.snap().fan, SimFan::Auto, "off means the firmware has them again, at once");
    rig.drive(5, 75, 60, 90.0);
    assert_eq!(rig.snap().fan, SimFan::Auto, "and stays so: nothing smart is on");
}

#[test]
fn a_manual_duty_survives_the_smart_fan_being_switched_off_again() {
    let rig = Rig::new();
    rig.drive(10, 75, 60, 90.0);
    rig.ok_fan_manual(50); // this also switches the curve off
    assert_eq!((rig.snap().fan, rig.snap().duty), (SimFan::Manual, [50, 50]));
    rig.ok_set_smart_fan(false); // already off: nothing to hand back
    assert_eq!((rig.snap().fan, rig.snap().duty), (SimFan::Manual, [50, 50]));
    rig.drive(5, 60, 50, 20.0);
    assert_eq!(rig.snap().duty, [50, 50], "ticks leave a hand-set duty alone");
}
