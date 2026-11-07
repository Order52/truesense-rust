//! The capability report against the simulator, against scripted firmware and against fixture
//! trees of a laptop that has everything, nothing, or something in between.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::json;
use ts_platform::firmware::{Device, FirmwareError, ReadOnly, Simulator};
use ts_platform::hw::{Hw, READ_ONLY};
use ts_system::models::{self, Tier};

use super::*;
use crate::api::tests::Fx;

// ---- doubles ----

/// A firmware that answers from a script and records everything it was sent.
struct Script {
    replies: Vec<(&'static str, &'static str)>,
    /// Every command fails like this when set (the module is not loaded).
    absent: bool,
    sent: Mutex<Vec<String>>,
}

impl Script {
    fn new(replies: &[(&'static str, &'static str)]) -> Arc<Self> {
        Arc::new(Self { replies: replies.to_vec(), absent: false, sent: Mutex::default() })
    }

    fn absent() -> Arc<Self> {
        Arc::new(Self { replies: Vec::new(), absent: true, sent: Mutex::default() })
    }

    fn sent(&self) -> Vec<String> {
        self.sent.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl Device for Script {
    fn run(&self, command: &str) -> Result<String, FirmwareError> {
        self.sent.lock().unwrap_or_else(PoisonError::into_inner).push(command.to_owned());
        if self.absent {
            return Err(FirmwareError::NotLoaded { path: PathBuf::from("/dev/truesense") });
        }
        let found = self.replies.iter().find(|(c, _)| *c == command);
        Ok(found.map_or_else(|| "err unknown command\n".to_owned(), |(_, r)| format!("{r}\n")))
    }
}

/// The answers of this laptop's firmware (a PH16-71), as the module prints them.
const PH16: &[(&str, &str)] = &[
    ("version", "ok version=0.5.0"),
    ("caps", "ok game=1 apge=1 battery=1"),
    ("sensors", "ok cpu=50 sys=52 gpu=48 cpu_rpm=2100 gpu_rpm=2200"),
    ("get_fan", "ok behavior=0x41 cpu=30 gpu=30"),
    ("get_mode", "ok mode=1"),
    ("get_modes", "ok mask=0x73"),
    ("get_overdrive", "ok overdrive=1 raw=0x0"),
    ("get_boot", "ok boot=0"),
    ("get_kbtimeout", "ok kbtimeout=1 raw=0x0"),
    ("get_usb", "ok usb=30 raw=0"),
    ("get_battery 1", "ok battery=0 list=3"),
    ("get_battery 2", "ok battery=0 list=3"),
    ("get_gpu", "ok gpu=2"),
    ("get_rear", "ok raw=00000564000100aec700000000000000"),
];

fn script_without(missing: &[&str]) -> Arc<Script> {
    let kept: Vec<_> = PH16.iter().filter(|(c, _)| !missing.contains(c)).copied().collect();
    Script::new(&kept)
}

fn with_firmware(device: Arc<dyn Device>) -> Fx {
    Fx::build(|mut s| {
        s.hw = Hw::new(device);
        s
    })
}

fn put(fx: &Fx, rel: &str, text: &str) -> PathBuf {
    let path = fx.dir.0.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, text).unwrap();
    path
}

fn chmod(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

/// The keyboard controller's lighting interface in `/sys/class/hidraw`.
fn hid_lighting(fx: &Fx) {
    let dir = fx.dir.0.join("sys/class/hidraw/hidraw3/device");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("uevent"), "HID_ID=0003:000004F2:00000117\n").unwrap();
    fs::write(dir.join("report_descriptor"), [0x06, 0x02, 0xFF, 0x09, 0x01]).unwrap();
}

fn rapl(fx: &Fx, mode: u32) {
    for file in RAPL_LIMIT_FILES {
        let existing = fx.dir.0.join(RAPL_ZONE).join(file);
        if existing.exists() {
            chmod(&existing, 0o644);
        }
        let path = put(fx, &format!("{RAPL_ZONE}/{file}"), "55000000\n");
        chmod(&path, mode);
    }
}

fn msr(fx: &Fx, lock_bit: bool) {
    let mut image = vec![0u8; 0x620];
    if lock_bit {
        image[0x610 + 7] = 0x80;
    }
    let path = fx.dir.0.join("dev/cpu/0/msr");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, image).unwrap();
}

fn touchpad(fx: &Fx, with_switch: bool) {
    put(
        fx,
        "proc/bus/input/devices",
        "I: Bus=0018 Vendor=06cb Product=cd7e Version=0100\nN: Name=\"SYNA2BA6:00 06CB:CD7E Touchpad\"\nH: Handlers=mouse1 event13 \nB: PROP=5\n",
    );
    if with_switch {
        put(fx, "sys/class/input/event13/device/inhibited", "0\n");
    }
}

fn item(c: &Capabilities, f: Feature) -> &Capability {
    c.get(f).unwrap_or_else(|| panic!("{f:?} is missing"))
}

/// Every unsupported entry explains itself in one short sentence; a supported one does not.
fn assert_wellformed(c: &Capabilities) {
    assert_eq!(c.items.len(), Feature::ALL.len());
    for (item, feature) in c.items.iter().zip(Feature::ALL) {
        assert_eq!(item.id, feature);
        assert_eq!(item.label, feature.label());
        assert!(!item.source.is_empty(), "{feature:?} has no source");
        if item.supported {
            assert_eq!(item.why, None, "{feature:?}");
        } else {
            let why = item.why.as_deref().unwrap_or_else(|| panic!("{feature:?} gives no reason"));
            assert!(why.ends_with('.') && !why.contains('\n') && why.len() <= 120, "{feature:?}: {why:?}");
        }
    }
}

// ---- the simulator ----

#[test]
fn the_simulated_laptop_answers_for_everything_the_simulator_implements() {
    let fx = Fx::new();
    fx.rig.daemon.answer_ok("ping", json!("pong"));
    let c = fx.api.capabilities();
    assert_wellformed(&c);
    assert!(c.simulated);
    assert_eq!(c.firmware_answered, "14/14");
    for feature in Feature::ALL {
        let expected = !matches!(feature, Feature::Dts | Feature::Rnnoise | Feature::GpuPowerLimit);
        assert_eq!(c.supported(feature), expected, "{feature:?}: {:?}", item(&c, feature));
    }
    assert_eq!(
        item(&c, Feature::PowerModes).detail.as_deref(),
        Some("quiet, balanced, performance, turbo, eco")
    );
    assert_eq!(item(&c, Feature::PowerModes).source, "firmware get_modes");
    assert_eq!(item(&c, Feature::KeyboardBacklight).source, "simulator");
    assert_eq!(item(&c, Feature::PowerLimits).detail.as_deref(), Some("The limits are not locked."));
}

#[test]
fn the_service_is_a_real_question_even_in_simulation() {
    let fx = Fx::offline();
    let c = fx.api.capabilities();
    let daemon = item(&c, Feature::Daemon);
    assert!(!daemon.supported);
    assert_eq!(daemon.why.as_deref(), Some("The TrueSense service is not running."));
    assert!(c.supported(Feature::PowerModes));
}

#[test]
fn probing_sends_only_read_only_commands_and_changes_nothing() {
    let sim = Arc::new(Simulator::new());
    let recorder = Arc::new(Recorder { inner: sim.clone(), sent: Mutex::default() });
    let before = sim.snapshot();
    let fx = with_firmware(recorder.clone());
    let _ = fx.api.capabilities();
    let _ = fx.api.battery_info();
    assert_eq!(sim.snapshot(), before, "the simulated firmware changed");
    let sent = recorder.sent.lock().unwrap_or_else(PoisonError::into_inner).clone();
    assert!(!sent.is_empty());
    for command in &sent {
        assert!(ReadOnly::allows(command), "{command:?} is not a read-only command");
        let name = command.split_whitespace().next().unwrap();
        assert!(name.starts_with("get_") || ["version", "caps", "sensors"].contains(&name), "{command}");
    }
    // The whole question set is the probe's list, once.
    assert_eq!(sent.len(), READ_ONLY.len(), "{sent:?}");
}

struct Recorder {
    inner: Arc<Simulator>,
    sent: Mutex<Vec<String>>,
}

impl Device for Recorder {
    fn run(&self, command: &str) -> Result<String, FirmwareError> {
        self.sent.lock().unwrap_or_else(PoisonError::into_inner).push(command.to_owned());
        self.inner.run(command)
    }
}

#[test]
fn the_probe_leaves_the_file_tree_as_it_found_it() {
    let fx = with_firmware(script_without(&[]));
    hid_lighting(&fx);
    rapl(&fx, 0o644);
    let listing = |root: &Path| -> BTreeSet<String> {
        fn walk(dir: &Path, out: &mut BTreeSet<String>) {
            for entry in fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                out.insert(format!("{} {}", path.display(), fs::metadata(&path).unwrap().len()));
                if path.is_dir() {
                    walk(&path, out);
                }
            }
        }
        let mut out = BTreeSet::new();
        walk(root, &mut out);
        out
    };
    let before = listing(&fx.dir.0);
    let _ = fx.api.capabilities();
    let _ = fx.api.battery_info();
    assert_eq!(listing(&fx.dir.0), before);
}

// ---- the real laptop's firmware, and the file system around it ----

#[test]
fn a_laptop_with_everything_installed_supports_everything_but_the_gpu_limit() {
    let device = script_without(&[]);
    let fx = with_firmware(device.clone());
    fx.rig.daemon.answer_ok("ping", json!("pong"));
    hid_lighting(&fx);
    touchpad(&fx, true);
    msr(&fx, false);
    rapl(&fx, 0o644);
    put(&fx, "proc/cpuinfo", "processor\t: 0\nvendor_id\t: GenuineIntel\ncpu family\t: 6\n");
    put(&fx, "proc/driver/nvidia/version", "NVRM\n");
    let helper = put(&fx, "usr/lib/truesense/truesense-nvml", "#!/bin/sh\n");
    chmod(&helper, 0o755);
    // A "real" machine (the version is not the simulator's), tier decided by the DMI tables.
    put(&fx, "dmi/sys_vendor", "Acer\n");
    put(&fx, "dmi/product_name", "Predator PH16-71\n");
    put(&fx, "dmi/bios_version", "V1.18\n");
    let c = fx.api.capabilities();
    assert_wellformed(&c);
    assert!(!c.simulated);
    assert_eq!(c.model.tier, Tier::Supported);
    assert_eq!(c.firmware_answered, "14/14");
    for feature in Feature::ALL {
        let expected =
            !matches!(feature, Feature::Dts | Feature::Rnnoise | Feature::GpuPowerLimit | Feature::FnKeys);
        assert_eq!(c.supported(feature), expected, "{feature:?}: {:?}", item(&c, feature));
    }
    assert_eq!(item(&c, Feature::KeyboardBacklight).source, "sysfs hidraw");
    // PL1 and PL2 are writable and, with the msr readable, the lock bit is seen clear.
    assert_eq!(item(&c, Feature::PowerLimits).detail.as_deref(), Some("The limits are not locked."));
    // The GPU limit is a no whatever the driver says.
    let gpu = item(&c, Feature::GpuPowerLimit);
    assert_eq!(gpu.why.as_deref(), Some("The GPU power limit is fixed by this laptop's firmware."));
    assert_eq!(gpu.detail.as_deref(), Some("The limit can be read."));
    // Only reads reached the firmware.
    assert!(device.sent().iter().all(|c| ReadOnly::allows(c)), "{:?}", device.sent());
}

#[test]
fn without_the_kernel_module_every_firmware_feature_says_so_and_nothing_else_is_guessed() {
    let fx = with_firmware(Script::absent());
    fx.rig.daemon.set_available(false);
    let c = fx.api.capabilities();
    assert_wellformed(&c);
    assert_eq!(c.firmware_answered, "0/14");
    assert!(!c.simulated);
    let module = Some("The TrueSense kernel module is not loaded.");
    for feature in [
        Feature::PowerModes,
        Feature::FanControl,
        Feature::Overdrive,
        Feature::BatteryLimit,
        Feature::BatteryCalibration,
        Feature::UsbCharge,
        Feature::GpuMode,
        Feature::RearLight,
        Feature::BootSound,
    ] {
        let it = item(&c, feature);
        assert!(!it.supported, "{feature:?}");
        assert_eq!(it.why.as_deref(), module, "{feature:?}");
        assert!(it.source.starts_with("firmware "), "{feature:?}");
    }
    // Nothing on the file system either: nothing is supported by accident.
    assert_eq!(c.items.iter().filter(|i| i.supported).count(), 0, "{:?}", c.items);
    assert_eq!(c.model.tier, Tier::Unsupported);
}

#[test]
fn a_device_this_user_may_not_open_is_named_as_such() {
    struct Denied;
    impl Device for Denied {
        fn run(&self, _: &str) -> Result<String, FirmwareError> {
            Err(FirmwareError::PermissionDenied { path: PathBuf::from("/dev/truesense") })
        }
    }
    let fx = with_firmware(Arc::new(Denied));
    let c = fx.api.capabilities();
    assert_eq!(
        item(&c, Feature::PowerModes).why.as_deref(),
        Some("This user may not open the TrueSense firmware device.")
    );
    assert_wellformed(&c);
}

#[test]
fn a_firmware_that_does_not_know_a_setting_gives_the_laptop_specific_sentence() {
    let fx = with_firmware(script_without(&["get_gpu", "get_boot", "get_rear"]));
    let c = fx.api.capabilities();
    assert_wellformed(&c);
    assert_eq!(
        item(&c, Feature::GpuMode).why.as_deref(),
        Some("This laptop's firmware does not offer a GPU mode switch.")
    );
    assert!(!c.supported(Feature::BootSound) && !c.supported(Feature::RearLight));
    assert!(c.supported(Feature::PowerModes) && c.supported(Feature::FanControl));
    assert_eq!(c.firmware_answered, "11/14");
}

#[test]
fn unknown_codes_in_replies_switch_the_matching_features_off() {
    let mut replies: Vec<(&'static str, &'static str)> = PH16
        .iter()
        .filter(|(c, _)| {
            !["get_overdrive", "get_usb", "get_kbtimeout", "get_rear", "get_modes", "caps"].contains(c)
        })
        .copied()
        .collect();
    replies.extend([
        ("get_overdrive", "ok overdrive=-1 raw=0xffff"),
        ("get_usb", "ok usb=-1 raw=-1"),
        ("get_kbtimeout", "ok kbtimeout=-1 raw=0x0"),
        ("get_rear", "ok raw=zz"),
        ("get_modes", "ok mask=0x0"),
        ("caps", "ok game=1 apge=0 battery=0"),
    ]);
    let fx = with_firmware(Script::new(&replies));
    let c = fx.api.capabilities();
    assert_wellformed(&c);
    assert_eq!(
        item(&c, Feature::Overdrive).why.as_deref(),
        Some("The firmware cannot report the panel overdrive on this laptop.")
    );
    assert!(!c.supported(Feature::UsbCharge));
    assert!(!c.supported(Feature::KeyboardTimeout), "no controller and no firmware flag");
    assert!(!c.supported(Feature::RearLight));
    assert_eq!(item(&c, Feature::PowerModes).why.as_deref(), Some("The firmware offers no power modes."));
    let limit = item(&c, Feature::BatteryLimit);
    assert!(!limit.supported);
    assert_eq!(limit.why.as_deref(), Some("This laptop's firmware has no battery interface."));
    assert!(!c.supported(Feature::BatteryCalibration));
}

#[test]
fn the_keyboard_controller_alone_is_enough_for_the_backlight_timeout() {
    let fx = with_firmware(script_without(&["get_kbtimeout"]));
    assert!(!fx.api.capabilities().supported(Feature::KeyboardTimeout));
    hid_lighting(&fx);
    let c = fx.api.capabilities();
    assert!(c.supported(Feature::KeyboardTimeout));
    assert_eq!(item(&c, Feature::KeyboardTimeout).source, "sysfs hidraw");
}

// ---- the parts that come from the file system ----

#[test]
fn the_keyboard_needs_its_hid_node_and_per_key_colours_also_need_a_known_model() {
    let fx = with_firmware(script_without(&[]));
    let c = fx.api.capabilities();
    assert_eq!(
        item(&c, Feature::KeyboardBacklight).why.as_deref(),
        Some("No keyboard backlight controller was found.")
    );
    assert_eq!(
        item(&c, Feature::PerKeyRgb).why.as_deref(),
        Some("No keyboard backlight controller was found.")
    );

    hid_lighting(&fx);
    let c = fx.api.capabilities();
    assert!(c.supported(Feature::KeyboardBacklight));
    // The DMI tables say nothing here, so the model is not one whose key map is verified.
    assert_eq!(
        item(&c, Feature::PerKeyRgb).why.as_deref(),
        Some("TrueSense knows the key positions of the Predator PH16-71 keyboard only.")
    );

    put(&fx, "dmi/sys_vendor", "Acer\n");
    put(&fx, "dmi/product_name", "Predator PH16-71\n");
    assert!(fx.api.capabilities().supported(Feature::PerKeyRgb));
}

#[test]
fn a_lookalike_hid_node_is_not_the_keyboard() {
    let fx = with_firmware(script_without(&[]));
    let dir = fx.dir.0.join("sys/class/hidraw/hidraw1/device");
    fs::create_dir_all(&dir).unwrap();
    // Right controller, wrong interface: the Fn-key page, not the lighting page.
    fs::write(dir.join("uevent"), "HID_ID=0003:000004F2:00000117\n").unwrap();
    fs::write(dir.join("report_descriptor"), [0x06, 0x88, 0x00]).unwrap();
    assert!(!fx.api.capabilities().supported(Feature::KeyboardBacklight));
}

#[test]
fn the_touchpad_switch_needs_a_touchpad_and_a_kernel_that_gives_it_one() {
    let fx = with_firmware(script_without(&[]));
    assert_eq!(
        item(&fx.api.capabilities(), Feature::Touchpad).why.as_deref(),
        Some("No touchpad was found.")
    );
    touchpad(&fx, false);
    assert_eq!(
        item(&fx.api.capabilities(), Feature::Touchpad).why.as_deref(),
        Some("This kernel cannot switch the touchpad off (it needs Linux 5.9 or newer).")
    );
    touchpad(&fx, true);
    assert!(fx.api.capabilities().supported(Feature::Touchpad));
}

#[test]
fn the_cpu_undervolt_needs_an_intel_cpu_and_the_msr_module() {
    let fx = with_firmware(script_without(&[]));
    let why = |fx: &Fx| item(&fx.api.capabilities(), Feature::CpuUndervolt).why.clone();
    assert_eq!(why(&fx).as_deref(), Some("The msr kernel module is not loaded (sudo modprobe msr)."));

    msr(&fx, false);
    // A cpuinfo that says nothing does not block it: the device is the proof.
    assert_eq!(why(&fx), None);
    put(&fx, "proc/cpuinfo", "vendor_id\t: AuthenticAMD\n");
    assert_eq!(why(&fx).as_deref(), Some("Undervolting works only on Intel processors."));
    put(&fx, "proc/cpuinfo", "vendor_id : GenuineIntel\n");
    assert_eq!(why(&fx), None);
    // Hostile text: no vendor line in a huge, binary file.
    let path = fx.dir.0.join("proc/cpuinfo");
    fs::write(path, vec![0xffu8; 100_000]).unwrap();
    assert_eq!(why(&fx), None);
}

#[test]
fn the_gpu_undervolt_needs_the_driver_and_an_executable_helper() {
    let fx = with_firmware(script_without(&[]));
    let why = |fx: &Fx| item(&fx.api.capabilities(), Feature::GpuUndervolt).why.clone();
    assert_eq!(why(&fx).as_deref(), Some("The NVIDIA driver is not loaded."));
    put(&fx, "sys/module/nvidia/version", "550\n");
    assert_eq!(why(&fx).as_deref(), Some("The truesense-nvml helper is not installed."));
    let helper = put(&fx, "usr/local/lib/truesense/truesense-nvml", "x");
    assert_eq!(
        why(&fx).as_deref(),
        Some("The truesense-nvml helper is not installed."),
        "not executable yet"
    );
    chmod(&helper, 0o755);
    assert_eq!(why(&fx), None);
}

#[test]
fn the_cpu_power_limits_need_writable_rapl_files_and_an_unlocked_register() {
    let fx = with_firmware(script_without(&[]));
    let c = fx.api.capabilities();
    assert_eq!(
        item(&c, Feature::PowerLimits).why.as_deref(),
        Some("This kernel exposes no CPU power limits (the intel_rapl driver is missing).")
    );

    // The kernel makes the files read-only when the BIOS has locked them.
    rapl(&fx, 0o444);
    let c = fx.api.capabilities();
    assert!(!c.supported(Feature::PowerLimits));
    assert_eq!(
        item(&c, Feature::PowerLimits).why.as_deref(),
        Some("The firmware has locked the CPU power limits.")
    );

    rapl(&fx, 0o644);
    let c = fx.api.capabilities();
    assert!(c.supported(Feature::PowerLimits));
    assert_eq!(
        item(&c, Feature::PowerLimits).detail.as_deref(),
        Some("The limits' lock state needs root to read.")
    );

    msr(&fx, true);
    let c = fx.api.capabilities();
    assert!(!c.supported(Feature::PowerLimits), "the lock bit wins over a writable-looking file");
    assert_eq!(
        item(&c, Feature::PowerLimits).detail.as_deref(),
        Some("The firmware has locked the limits until the next restart.")
    );

    msr(&fx, false);
    assert!(fx.api.capabilities().supported(Feature::PowerLimits));
}

#[test]
fn one_rapl_file_alone_is_not_a_power_limit_interface() {
    let fx = with_firmware(script_without(&[]));
    put(&fx, &format!("{RAPL_ZONE}/{}", RAPL_LIMIT_FILES[0]), "1\n");
    assert!(!fx.api.capabilities().supported(Feature::PowerLimits));
    // A directory with the right name is not a file.
    fs::create_dir_all(fx.dir.0.join(RAPL_ZONE).join(RAPL_LIMIT_FILES[1])).unwrap();
    assert!(!fx.api.capabilities().supported(Feature::PowerLimits));
}

#[test]
fn a_short_msr_image_leaves_the_lock_unknown() {
    let fx = with_firmware(script_without(&[]));
    let path = fx.dir.0.join("dev/cpu/0/msr");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, [0u8; 16]).unwrap();
    rapl(&fx, 0o644);
    let c = fx.api.capabilities();
    assert_eq!(
        item(&c, Feature::PowerLimits).detail.as_deref(),
        Some("The limits' lock state needs root to read.")
    );
}

#[test]
fn the_fan_speed_falls_back_to_the_acer_hwmon_chip() {
    let fx = with_firmware(script_without(&["sensors"]));
    let why = |fx: &Fx| item(&fx.api.capabilities(), Feature::FanRpm).why.clone();
    assert_eq!(why(&fx).as_deref(), Some("No fan speed sensor was found."));
    put(&fx, "sys/class/hwmon/hwmon4/name", "coretemp\n");
    put(&fx, "sys/class/hwmon/hwmon4/fan1_input", "1000\n");
    assert!(why(&fx).is_some(), "a fan on another chip is not the laptop's");
    put(&fx, "sys/class/hwmon/hwmon5/name", "acer\n");
    assert!(why(&fx).is_some(), "the chip has no fan input yet");
    put(&fx, "sys/class/hwmon/hwmon5/fan2_input", "2000\n");
    let c = fx.api.capabilities();
    assert!(c.supported(Feature::FanRpm));
    assert_eq!(item(&c, Feature::FanRpm).source, "sysfs hwmon");
}

#[test]
fn the_fn_key_node_comes_from_the_finder_of_the_services() {
    let fx = Fx::build(|mut s| {
        s.hw = Hw::new(script_without(&[]));
        s.fn_key_node = Arc::new(|| Some(PathBuf::from("/dev/hidraw4")));
        s
    });
    assert!(fx.api.capabilities().supported(Feature::FnKeys));
    let without = with_firmware(script_without(&[]));
    assert_eq!(
        item(&without.api.capabilities(), Feature::FnKeys).why.as_deref(),
        Some("The keyboard's Fn-key interface was not found.")
    );
}

// ---- the pure decision ----

fn facts(model_dir: &Fx, device: Arc<dyn Device>, machine: Machine) -> Facts {
    Facts {
        model: models::identify(&model_dir.dir.0.join("dmi")),
        simulated: false,
        firmware: Firmware::ask(device.as_ref()),
        machine,
        dts: false,
        rnnoise: false,
        daemon: false,
    }
}

fn nothing() -> Machine {
    Machine {
        hid_lighting: false,
        fn_node: false,
        touchpad_device: false,
        touchpad_switch: false,
        intel: None,
        msr_node: false,
        nvml_helper: false,
        nvidia_driver: false,
        rapl_present: false,
        rapl_writable: false,
        rapl_lock_bit: None,
        hwmon_fans: false,
    }
}

#[test]
fn an_unverified_model_does_not_gate_features_the_firmware_itself_answers() {
    let fx = Fx::offline();
    put(&fx, "dmi/sys_vendor", "Acer\n");
    put(&fx, "dmi/product_name", "Predator PHN16-71\n");
    let mut f = facts(&fx, script_without(&[]), nothing());
    f.machine.hid_lighting = true;
    let c = decide(&f);
    assert_eq!(c.model.tier, Tier::Unverified);
    assert!(c.supported(Feature::PowerModes) && c.supported(Feature::KeyboardBacklight));
    assert!(!c.supported(Feature::PerKeyRgb), "the key map is verified for one model only");
}

#[test]
fn the_decision_is_deterministic_and_total() {
    let fx = Fx::offline();
    let f = facts(&fx, script_without(&[]), nothing());
    assert_eq!(decide(&f), decide(&f));
    assert_eq!(decide(&f).items.len(), Feature::ALL.len());
}

// ---- shape ----

#[test]
fn feature_ids_are_unique_stable_snake_case_names() {
    let ids: BTreeSet<&str> = Feature::ALL.iter().map(|f| f.id()).collect();
    assert_eq!(ids.len(), Feature::ALL.len());
    for feature in Feature::ALL {
        let id = feature.id();
        assert!(id.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'), "{id}");
        assert_eq!(Feature::from_id(id), Some(feature));
        assert!(!feature.label().is_empty());
    }
    assert_eq!(Feature::from_id("nope"), None);
    assert_eq!(Feature::from_id(""), None);
    for expected in [
        "power_modes",
        "fan_control",
        "fan_rpm",
        "overdrive",
        "battery_limit",
        "battery_calibration",
        "usb_charge",
        "gpu_mode",
        "rear_light",
        "keyboard_backlight",
        "per_key_rgb",
        "boot_sound",
        "keyboard_timeout",
        "touchpad",
        "cpu_undervolt",
        "gpu_undervolt",
        "power_limits",
        "gpu_power_limit",
        "fn_keys",
        "dts",
        "rnnoise",
        "daemon",
    ] {
        assert!(ids.contains(expected), "{expected}");
    }
}

#[test]
fn the_report_serialises_with_null_reasons_for_supported_features() {
    let fx = Fx::new();
    let json = serde_json::to_value(fx.api.capabilities()).unwrap();
    assert_eq!(json["simulated"], json!(true));
    assert_eq!(json["firmware_answered"], json!("14/14"));
    assert!(json["model"]["tier"].is_string());
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 22);
    assert_eq!(items[0]["id"], json!("power_modes"));
    assert_eq!(items[0]["supported"], json!(true));
    assert!(items[0]["why"].is_null());
    assert_eq!(items[0]["label"], json!("Power modes"));
    let gpu = items.iter().find(|i| i["id"] == "gpu_power_limit").unwrap();
    assert_eq!(gpu["supported"], json!(false));
    assert!(gpu["why"].is_string());
}

#[test]
fn the_checklist_prints_a_mark_and_the_reason() {
    let fx = Fx::offline();
    let text = fx.api.capabilities().to_string();
    assert!(text.contains("\n[x] Power modes"), "{text}");
    assert!(
        text.contains("\n[ ] GPU power limit - The GPU power limit is fixed by this laptop's firmware."),
        "{text}"
    );
    assert_eq!(text.lines().count(), 1 + Feature::ALL.len());
}
