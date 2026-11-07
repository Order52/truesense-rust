//! capabilities: what THIS machine supports, answered once so a page can grey a control out with
//! one sentence instead of showing an error after the person clicked it.
//!
//! Idea adapted from the `capabilities.rs` of predator-sense by cleyton1986 (GPL-3.0), used with
//! the author's permission: probe real devices, never a model list. Differences: the answers are
//! typed and carry a reason and a source, nothing is cached for the life of the process (a
//! battery, a keyboard or the kernel module can appear after start-up), and nothing panics.
//!
//! # How an answer is made
//!
//! The firmware is asked ONE question set: the commands of
//! [`ts_platform::hw::READ_ONLY`], through [`probe_firmware`], which runs them behind the
//! `ReadOnly` guard. A setting is never written to find out whether it can be written; where
//! only a write could tell (the BIOS lock on an undervolt, whether a GPU power limit would be
//! accepted) the entry says what is known and errs on the side of "no".
//!
//! Everything that is not firmware is a read below the injected [`Services::root`]: the HID node
//! of the keyboard, the `msr` device, the RAPL files and their permission bits, the NVML helper,
//! the hwmon chips. Reading the CPU's power-limit lock bit needs root; without it the state is
//! "unknown", and unknown never blocks a feature that the files say is writable.
//!
//! # Simulation
//!
//! The firmware simulator answers `version=sim`. Then the machine the page is looking at is
//! imaginary, and the facts that would come from `/sys` and `/dev` (keyboard node, MSR, RAPL,
//! helper, touchpad, Fn-key node) are reported as present, with the source `simulator`: the
//! commands that use them run against simulated hardware too, so greying them out would hide
//! exactly what a developer came to look at. The service, the sound plug-ins and the model
//! identification are always read for real.
//!
//! The decision is a pure function of [`Facts`] ([`decide`]); gathering the facts is the only
//! part that touches files.
#![forbid(unsafe_code)]

use std::fmt;
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde::{Serialize, Serializer};
use ts_domain::gpuundervolt::helper::{helper_candidates, is_executable_file};
use ts_domain::keyboard::find_node;
use ts_domain::power::MsrReader;
use ts_domain::undervolt::DevMsr;
use ts_platform::firmware::{Device, FirmwareError, Reply};
use ts_platform::hw::{Coded, PowerMode, READ_ONLY, probe_firmware};
use ts_system::models::{self, Tier};

use super::Api;

/// What the firmware simulator answers to `version`.
const SIM_VERSION: &str = "sim";

/// `MSR_PKG_POWER_LIMIT`: bit 63 locks PL1 and PL2 until the next reboot.
const MSR_PKG_POWER_LIMIT: u32 = 0x610;

/// The RAPL zone of the CPU package, below the root.
const RAPL_ZONE: &str = "sys/class/powercap/intel-rapl:0";

/// The two RAPL attributes that are PL1 and PL2.
const RAPL_LIMIT_FILES: [&str; 2] = ["constraint_0_power_limit_uw", "constraint_1_power_limit_uw"];

/// The names under which the Acer fan chip registers with hwmon.
const FAN_CHIPS: [&str; 2] = ["acer", "facer"];

/// How much of `/proc/cpuinfo` is read: the vendor is on the first lines, the rest is one block per
/// core and can be hundreds of kilobytes.
const CPUINFO_HEAD: u64 = 8192;

/// One thing a laptop may or may not be able to do. The ids are the stable names pages and
/// scripts use; the labels are English source text for the checklist (the UI translates by id).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Feature {
    /// Quiet, balanced, performance, turbo and eco, as far as the firmware offers them.
    PowerModes,
    /// Setting the fan duty by hand.
    FanControl,
    /// Reading the fan speeds.
    FanRpm,
    /// The panel overdrive switch.
    Overdrive,
    /// The 80 % charge limit.
    BatteryLimit,
    /// The battery calibration cycle.
    BatteryCalibration,
    /// USB charging while the laptop is off.
    UsbCharge,
    /// The Optimus / NVIDIA-only (MUX) switch.
    GpuMode,
    /// The rear light bar.
    RearLight,
    /// The keyboard backlight.
    KeyboardBacklight,
    /// A colour for every key.
    PerKeyRgb,
    /// The firmware's start-up chime.
    BootSound,
    /// "Backlight off after 30 s without typing".
    KeyboardTimeout,
    /// Switching the touchpad off.
    Touchpad,
    /// Lowering the CPU voltage.
    CpuUndervolt,
    /// Lowering the GPU voltage curve.
    GpuUndervolt,
    /// Setting the CPU's PL1 and PL2.
    PowerLimits,
    /// Setting the GPU's power limit.
    GpuPowerLimit,
    /// The PREDATOR key and the Fn combinations.
    FnKeys,
    /// DTS sound through AcerUltraX.
    Dts,
    /// The RNNoise plug-in behind "Purified Voice".
    Rnnoise,
    /// The root service.
    Daemon,
}

impl Feature {
    /// Every feature, in the order the checklist shows them.
    pub const ALL: [Feature; 22] = [
        Self::PowerModes,
        Self::FanControl,
        Self::FanRpm,
        Self::Overdrive,
        Self::BatteryLimit,
        Self::BatteryCalibration,
        Self::UsbCharge,
        Self::GpuMode,
        Self::RearLight,
        Self::KeyboardBacklight,
        Self::PerKeyRgb,
        Self::BootSound,
        Self::KeyboardTimeout,
        Self::Touchpad,
        Self::CpuUndervolt,
        Self::GpuUndervolt,
        Self::PowerLimits,
        Self::GpuPowerLimit,
        Self::FnKeys,
        Self::Dts,
        Self::Rnnoise,
        Self::Daemon,
    ];

    /// The stable id (`power_modes`).
    pub fn id(self) -> &'static str {
        match self {
            Self::PowerModes => "power_modes",
            Self::FanControl => "fan_control",
            Self::FanRpm => "fan_rpm",
            Self::Overdrive => "overdrive",
            Self::BatteryLimit => "battery_limit",
            Self::BatteryCalibration => "battery_calibration",
            Self::UsbCharge => "usb_charge",
            Self::GpuMode => "gpu_mode",
            Self::RearLight => "rear_light",
            Self::KeyboardBacklight => "keyboard_backlight",
            Self::PerKeyRgb => "per_key_rgb",
            Self::BootSound => "boot_sound",
            Self::KeyboardTimeout => "keyboard_timeout",
            Self::Touchpad => "touchpad",
            Self::CpuUndervolt => "cpu_undervolt",
            Self::GpuUndervolt => "gpu_undervolt",
            Self::PowerLimits => "power_limits",
            Self::GpuPowerLimit => "gpu_power_limit",
            Self::FnKeys => "fn_keys",
            Self::Dts => "dts",
            Self::Rnnoise => "rnnoise",
            Self::Daemon => "daemon",
        }
    }

    /// The feature with this id.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.id() == id)
    }

    /// The English name for the checklist.
    pub fn label(self) -> &'static str {
        match self {
            Self::PowerModes => "Power modes",
            Self::FanControl => "Manual fan control",
            Self::FanRpm => "Fan speed reading",
            Self::Overdrive => "Display overdrive",
            Self::BatteryLimit => "Battery 80 % limit",
            Self::BatteryCalibration => "Battery calibration",
            Self::UsbCharge => "USB charging while off",
            Self::GpuMode => "GPU mode switch",
            Self::RearLight => "Rear light bar",
            Self::KeyboardBacklight => "Keyboard backlight",
            Self::PerKeyRgb => "Per-key keyboard colours",
            Self::BootSound => "Start-up sound",
            Self::KeyboardTimeout => "Keyboard backlight timeout",
            Self::Touchpad => "Touchpad switch",
            Self::CpuUndervolt => "CPU undervolt",
            Self::GpuUndervolt => "GPU undervolt",
            Self::PowerLimits => "CPU power limits",
            Self::GpuPowerLimit => "GPU power limit",
            Self::FnKeys => "Predator key and Fn keys",
            Self::Dts => "DTS sound (AcerUltraX)",
            Self::Rnnoise => "Voice noise removal (RNNoise)",
            Self::Daemon => "TrueSense service",
        }
    }
}

impl Serialize for Feature {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.id())
    }
}

/// The answer for one feature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Capability {
    /// Which feature this is.
    pub id: Feature,
    /// Its English name.
    pub label: &'static str,
    /// This machine can do it (and the control may be enabled).
    pub supported: bool,
    /// When it cannot: one short sentence a person can read. `None` when supported.
    pub why: Option<String>,
    /// Where the answer came from (`firmware get_modes`, `sysfs hidraw`, `simulator`, ...).
    pub source: String,
    /// A fact worth showing next to a supported or unsupported entry (the modes on offer, whether
    /// the power-limit lock could be read); `None` when there is nothing to add.
    pub detail: Option<String>,
}

impl Capability {
    fn yes(id: Feature, source: impl Into<String>) -> Self {
        Self { id, label: id.label(), supported: true, why: None, source: source.into(), detail: None }
    }

    fn no(id: Feature, why: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            id,
            label: id.label(),
            supported: false,
            why: Some(why.into()),
            source: source.into(),
            detail: None,
        }
    }

    fn of(id: Feature, ok: bool, why: impl Into<String>, source: impl Into<String>) -> Self {
        let source = source.into();
        if ok { Self::yes(id, source) } else { Self::no(id, why, source) }
    }

    fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// What the DMI tables say about the machine, for the header of the checklist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelSummary {
    /// `sys_vendor`.
    pub vendor: String,
    /// `product_name`, or `unknown`.
    pub product: String,
    /// `bios_version`, or `?`.
    pub bios: String,
    /// How far TrueSense trusts the model.
    pub tier: Tier,
    /// What to tell the person about it.
    pub message: String,
}

impl From<&models::Model> for ModelSummary {
    fn from(m: &models::Model) -> Self {
        Self {
            vendor: m.vendor.clone(),
            product: m.product.clone(),
            bios: m.bios.clone(),
            tier: m.tier,
            message: m.message.clone(),
        }
    }
}

/// What this machine supports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Capabilities {
    /// Which laptop this is.
    pub model: ModelSummary,
    /// The firmware is the simulator (`TRUESENSE_SIM`): the hardware facts are imaginary.
    pub simulated: bool,
    /// How many of the read-only firmware questions were answered, `12/14`.
    pub firmware_answered: String,
    /// One entry per [`Feature`], in [`Feature::ALL`] order.
    pub items: Vec<Capability>,
}

impl Capabilities {
    /// The entry of one feature.
    pub fn get(&self, feature: Feature) -> Option<&Capability> {
        self.items.iter().find(|c| c.id == feature)
    }

    /// Whether a feature is supported (`false` for one that is not in the report).
    pub fn supported(&self, feature: Feature) -> bool {
        self.get(feature).is_some_and(|c| c.supported)
    }
}

impl fmt::Display for Capabilities {
    /// A checklist: `[x] Power modes` and `[ ] Display overdrive - why`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} ({}), BIOS {}: {}",
            self.model.product,
            self.model.vendor,
            self.model.bios,
            self.model.tier.as_str()
        )?;
        for item in &self.items {
            match (&item.why, item.supported) {
                (Some(why), false) => write!(f, "\n[ ] {} - {why}", item.label)?,
                _ => write!(f, "\n[x] {}", item.label)?,
            }
        }
        Ok(())
    }
}

/// Why one firmware question got no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Failure {
    /// The device node does not exist: the kernel module is not loaded.
    NotLoaded,
    /// The node exists and this user may not open it.
    Denied,
    /// The module or the transport said no, or said something that was not understood.
    Other,
}

impl From<&FirmwareError> for Failure {
    fn from(e: &FirmwareError) -> Self {
        match e {
            FirmwareError::NotLoaded { .. } => Self::NotLoaded,
            FirmwareError::PermissionDenied { .. } => Self::Denied,
            _ => Self::Other,
        }
    }
}

/// The answers to the read-only firmware commands.
#[derive(Debug, Clone, Default)]
pub(crate) struct Firmware {
    answers: Vec<(&'static str, Result<Reply, Failure>)>,
}

impl Firmware {
    /// Asks every read-only command, through the guard that refuses anything else.
    fn ask(device: &dyn Device) -> Self {
        let report = probe_firmware(device);
        let answers = report
            .entries()
            .iter()
            .map(|e| (e.command, e.outcome.as_ref().map(Clone::clone).map_err(Failure::from)))
            .collect();
        Self { answers }
    }

    fn answered(&self) -> usize {
        self.answers.iter().filter(|(_, r)| r.is_ok()).count()
    }

    fn reply(&self, command: &str) -> Option<&Reply> {
        self.answers.iter().find(|(c, _)| *c == command).and_then(|(_, r)| r.as_ref().ok())
    }

    fn failure(&self, command: &str) -> Option<&Failure> {
        self.answers.iter().find(|(c, _)| *c == command).and_then(|(_, r)| r.as_ref().err())
    }

    /// An integer field of a reply, or `None` when the command or the field is missing.
    fn int(&self, command: &str, key: &str) -> Option<i64> {
        self.reply(command)?.int(key).ok()
    }

    /// The `1` flags of the `caps` reply.
    fn cap(&self, key: &str) -> bool {
        self.reply("caps").and_then(|r| r.get(key)) == Some("1")
    }

    fn simulated(&self) -> bool {
        self.reply("version").and_then(|r| r.get("version")) == Some(SIM_VERSION)
    }

    /// The sentence for a firmware question that did not come out: the module is missing, the
    /// device is closed to this user, or the laptop's firmware has no such function.
    fn why_not(&self, command: &str, what: &str) -> String {
        match self.failure(command) {
            Some(Failure::NotLoaded) => "The TrueSense kernel module is not loaded.".to_owned(),
            Some(Failure::Denied) => "This user may not open the TrueSense firmware device.".to_owned(),
            _ => format!("This laptop's firmware does not offer {what}."),
        }
    }
}

/// What the files of the machine say. In simulation every fact is a yes
/// ([`Machine::simulated`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Machine {
    /// The keyboard's lighting interface exists in `/sys/class/hidraw`.
    pub hid_lighting: bool,
    /// The Fn-key interface of the keyboard exists.
    pub fn_node: bool,
    /// An input device with a touchpad's properties exists.
    pub touchpad_device: bool,
    /// ...and the kernel gave it an `inhibited` switch (Linux 5.9 and newer).
    pub touchpad_switch: bool,
    /// The CPU vendor is Intel; `None` when `/proc/cpuinfo` does not say.
    pub intel: Option<bool>,
    /// `/dev/cpu/0/msr` exists (the `msr` module is loaded).
    pub msr_node: bool,
    /// The `truesense-nvml` helper is installed and executable.
    pub nvml_helper: bool,
    /// The NVIDIA kernel driver is loaded.
    pub nvidia_driver: bool,
    /// The RAPL zone has both limit files.
    pub rapl_present: bool,
    /// Both limit files carry an owner write bit. The kernel makes them read-only when the BIOS
    /// has locked the limits, so this is the cheapest honest test of "can root change them".
    pub rapl_writable: bool,
    /// Bit 63 of `MSR_PKG_POWER_LIMIT`; `None` without root or the `msr` module.
    pub rapl_lock_bit: Option<bool>,
    /// An Acer hwmon chip exposes a fan speed.
    pub hwmon_fans: bool,
}

impl Machine {
    /// The imaginary machine of the firmware simulator: everything is there, nothing is locked.
    fn simulated() -> Self {
        Self {
            hid_lighting: true,
            fn_node: true,
            touchpad_device: true,
            touchpad_switch: true,
            intel: Some(true),
            msr_node: true,
            nvml_helper: true,
            nvidia_driver: true,
            rapl_present: true,
            rapl_writable: true,
            rapl_lock_bit: Some(false),
            hwmon_fans: true,
        }
    }
}

/// Everything [`decide`] needs. Plain data, so every case can be built in a test.
#[derive(Debug, Clone)]
pub(crate) struct Facts {
    pub model: models::Model,
    pub simulated: bool,
    pub firmware: Firmware,
    pub machine: Machine,
    pub dts: bool,
    pub rnnoise: bool,
    pub daemon: bool,
}

impl Api {
    /// What this machine supports, read now and without changing anything.
    pub fn capabilities(&self) -> Capabilities {
        decide(&self.gather_facts())
    }

    fn gather_facts(&self) -> Facts {
        let firmware = Firmware::ask(self.s.hw.device().as_ref());
        let simulated = firmware.simulated();
        let machine = if simulated { Machine::simulated() } else { self.scan_machine() };
        Facts {
            model: models::identify(&self.s.probe.dmi_root),
            simulated,
            firmware,
            machine,
            dts: self.s.audio.has_dts(),
            rnnoise: self.s.audio.has_rnnoise(),
            daemon: self.daemon_responds(),
        }
    }

    fn scan_machine(&self) -> Machine {
        let root = self.s.root.as_path();
        let inhibit = self.s.touchpad.inhibit_file();
        let (rapl_present, rapl_writable) = rapl(root);
        Machine {
            hid_lighting: find_node(&root.join("sys/class/hidraw")).is_ok(),
            fn_node: (self.s.fn_key_node)().is_some(),
            touchpad_device: inhibit.is_some(),
            touchpad_switch: inhibit.is_some_and(|p| p.exists()),
            intel: cpu_is_intel(root),
            msr_node: root.join("dev/cpu/0/msr").exists(),
            nvml_helper: nvml_helper(root, (self.s.config.env)("TRUESENSE_NVML").as_deref()),
            nvidia_driver: root.join("proc/driver/nvidia").exists()
                || root.join("sys/module/nvidia").exists(),
            rapl_present,
            rapl_writable,
            rapl_lock_bit: rapl_lock_bit(root),
            hwmon_fans: hwmon_fans(root),
        }
    }
}

/// The path `absolute` below `root`.
fn below(root: &Path, absolute: &Path) -> PathBuf {
    root.join(absolute.strip_prefix("/").unwrap_or(absolute))
}

/// Whether the first `vendor_id` line of `/proc/cpuinfo` says Intel; `None` when it says nothing
/// (an unreadable file, or a CPU architecture with no such line).
fn cpu_is_intel(root: &Path) -> Option<bool> {
    let mut head = Vec::new();
    fs::File::open(root.join("proc/cpuinfo")).ok()?.take(CPUINFO_HEAD).read_to_end(&mut head).ok()?;
    String::from_utf8_lossy(&head)
        .lines()
        .find_map(|line| line.strip_prefix("vendor_id").and_then(|rest| rest.split_once(':')))
        .map(|(_, vendor)| vendor.trim() == "GenuineIntel")
}

/// Whether the helper is installed anywhere the daemon would look.
fn nvml_helper(root: &Path, env_override: Option<&str>) -> bool {
    helper_candidates(env_override, None).iter().any(|c| is_executable_file(&below(root, c)))
}

/// Whether the RAPL zone has both limit files, and whether both can be written by their owner.
fn rapl(root: &Path) -> (bool, bool) {
    let zone = root.join(RAPL_ZONE);
    let modes: Vec<Option<u32>> = RAPL_LIMIT_FILES
        .iter()
        .map(|f| fs::metadata(zone.join(f)).ok().filter(|m| m.is_file()).map(|m| m.permissions().mode()))
        .collect();
    let present = modes.iter().all(Option::is_some);
    let writable = present && modes.iter().flatten().all(|mode| mode & 0o200 != 0);
    (present, writable)
}

/// The lock bit, through a reader that cannot write.
fn rapl_lock_bit(root: &Path) -> Option<bool> {
    let msr: &dyn MsrReader = &DevMsr::new(root.join("dev/cpu/0/msr"));
    msr.read_msr(MSR_PKG_POWER_LIMIT).ok().map(|value| value >> 63 != 0)
}

/// Whether an Acer hwmon chip has a fan input.
fn hwmon_fans(root: &Path) -> bool {
    let Ok(chips) = fs::read_dir(root.join("sys/class/hwmon")) else { return false };
    chips.filter_map(Result::ok).any(|chip| {
        let dir = chip.path();
        let name = fs::read_to_string(dir.join("name")).unwrap_or_default();
        FAN_CHIPS.contains(&name.trim()) && ["fan1_input", "fan2_input"].iter().any(|f| dir.join(f).exists())
    })
}

/// The report for these facts.
pub(crate) fn decide(facts: &Facts) -> Capabilities {
    let items = Feature::ALL.iter().map(|f| decide_one(*f, facts)).collect();
    Capabilities {
        model: ModelSummary::from(&facts.model),
        simulated: facts.simulated,
        firmware_answered: format!("{}/{}", facts.firmware.answered(), READ_ONLY.len()),
        items,
    }
}

/// The source text for a firmware command.
fn from_firmware(command: &str) -> String {
    format!("firmware {command}")
}

fn decide_one(feature: Feature, facts: &Facts) -> Capability {
    let fw = &facts.firmware;
    let m = &facts.machine;
    let sim = facts.simulated;
    let hardware_source = |real: &str| if sim { "simulator".to_owned() } else { real.to_owned() };
    match feature {
        Feature::PowerModes => {
            let mask = fw.int("get_modes", "mask").unwrap_or(0);
            let modes: Vec<&str> = PowerMode::ALL
                .iter()
                .filter(|mode| mask >> mode.code() & 1 == 1)
                .map(|mode| mode.name())
                .collect();
            let source = from_firmware("get_modes");
            if modes.is_empty() {
                let why = if fw.reply("get_modes").is_some() {
                    "The firmware offers no power modes.".to_owned()
                } else {
                    fw.why_not("get_modes", "power modes")
                };
                Capability::no(feature, why, source)
            } else {
                Capability::yes(feature, source).with_detail(modes.join(", "))
            }
        }
        Feature::FanControl => {
            let ok = ["behavior", "cpu", "gpu"].iter().all(|k| fw.int("get_fan", k).is_some());
            Capability::of(feature, ok, fw.why_not("get_fan", "fan control"), from_firmware("get_fan"))
        }
        Feature::FanRpm => {
            let firmware = ["cpu_rpm", "gpu_rpm"].iter().all(|k| fw.int("sensors", k).is_some());
            if firmware {
                Capability::yes(feature, from_firmware("sensors"))
            } else if m.hwmon_fans {
                Capability::yes(feature, hardware_source("sysfs hwmon"))
            } else {
                Capability::no(feature, "No fan speed sensor was found.", "firmware sensors, sysfs hwmon")
            }
        }
        Feature::Overdrive => {
            let value = fw.int("get_overdrive", "overdrive");
            let why = match value {
                Some(_) => "The firmware cannot report the panel overdrive on this laptop.".to_owned(),
                None => fw.why_not("get_overdrive", "a panel overdrive switch"),
            };
            Capability::of(feature, value.is_some_and(|v| v >= 0), why, from_firmware("get_overdrive"))
        }
        Feature::BatteryLimit => battery_interface(feature, fw, "get_battery 1", "the 80 % charge limit"),
        Feature::BatteryCalibration => battery_interface(feature, fw, "get_battery 2", "battery calibration"),
        Feature::UsbCharge => {
            let ok = fw.cap("apge") && fw.int("get_usb", "usb").is_some_and(|v| v >= 0);
            Capability::of(
                feature,
                ok,
                fw.why_not("get_usb", "USB charging while the laptop is off"),
                from_firmware("get_usb"),
            )
        }
        Feature::GpuMode => {
            let ok = fw.int("get_gpu", "gpu").is_some_and(|v| v > 0);
            Capability::of(feature, ok, fw.why_not("get_gpu", "a GPU mode switch"), from_firmware("get_gpu"))
        }
        Feature::RearLight => {
            let raw = fw.reply("get_rear").and_then(|r| r.get("raw"));
            let ok = fw.cap("game")
                && raw.is_some_and(|r| r.len() == 32 && r.bytes().all(|b| b.is_ascii_hexdigit()));
            Capability::of(feature, ok, fw.why_not("get_rear", "a rear light bar"), from_firmware("get_rear"))
        }
        Feature::BootSound => {
            let ok = fw.int("get_boot", "boot").is_some_and(|v| v == 0 || v == 1);
            Capability::of(
                feature,
                ok,
                fw.why_not("get_boot", "a start-up sound switch"),
                from_firmware("get_boot"),
            )
        }
        Feature::KeyboardBacklight => Capability::of(
            feature,
            m.hid_lighting,
            "No keyboard backlight controller was found.",
            hardware_source("sysfs hidraw"),
        ),
        Feature::PerKeyRgb => {
            // The key-name table is an owner-verified capture of the PH16-71 keyboard; on another
            // model the same controller may exist but the cells would not match the keys.
            let known = facts.model.tier == Tier::Supported || sim;
            let why = if m.hid_lighting {
                "TrueSense knows the key positions of the Predator PH16-71 keyboard only."
            } else {
                "No keyboard backlight controller was found."
            };
            Capability::of(feature, m.hid_lighting && known, why, hardware_source("sysfs hidraw, model"))
        }
        Feature::KeyboardTimeout => {
            // The keyboard controller's own flag is the one that acts, so its presence is
            // enough; otherwise the firmware's flag has to be one it can read.
            let firmware = fw.int("get_kbtimeout", "kbtimeout").is_some_and(|v| v >= 0);
            let source =
                if m.hid_lighting { hardware_source("sysfs hidraw") } else { from_firmware("get_kbtimeout") };
            Capability::of(
                feature,
                m.hid_lighting || firmware,
                "No keyboard backlight timeout switch was found.",
                source,
            )
        }
        Feature::Touchpad => {
            let why = if m.touchpad_device {
                "This kernel cannot switch the touchpad off (it needs Linux 5.9 or newer)."
            } else {
                "No touchpad was found."
            };
            Capability::of(
                feature,
                m.touchpad_switch,
                why,
                hardware_source("proc input devices, sysfs inhibited"),
            )
        }
        Feature::CpuUndervolt => {
            let (ok, why) = match (m.intel, m.msr_node) {
                (Some(false), _) => (false, "Undervolting works only on Intel processors."),
                (_, false) => (false, "The msr kernel module is not loaded (sudo modprobe msr)."),
                _ => (true, ""),
            };
            // Whether the BIOS has locked the voltage plane can only be found out by writing, which
            // a capability probe never does.
            Capability::of(feature, ok, why, hardware_source("proc cpuinfo, dev cpu msr"))
                .with_detail("A BIOS lock on the voltage cannot be seen without trying.")
        }
        Feature::GpuUndervolt => {
            let (ok, why) = match (m.nvidia_driver, m.nvml_helper) {
                (false, _) => (false, "The NVIDIA driver is not loaded."),
                (_, false) => (false, "The truesense-nvml helper is not installed."),
                _ => (true, ""),
            };
            Capability::of(feature, ok, why, hardware_source("proc nvidia, helper file"))
        }
        Feature::PowerLimits => power_limits(m, hardware_source("sysfs powercap, dev cpu msr")),
        Feature::GpuPowerLimit => {
            // TrueSense reads the limit with nvidia-smi but has never been shown to change it on
            // this laptop: the vBIOS fixes the board power (TGP). Nothing short of a write could
            // prove otherwise, so the answer is no, whatever the driver says.
            let readable =
                if m.nvidia_driver { "The limit can be read." } else { "The NVIDIA driver is not loaded." };
            Capability::no(
                feature,
                "The GPU power limit is fixed by this laptop's firmware.",
                hardware_source("not probed: writing would be the only proof"),
            )
            .with_detail(readable)
        }
        Feature::FnKeys => Capability::of(
            feature,
            m.fn_node,
            "The keyboard's Fn-key interface was not found.",
            hardware_source("sysfs hidraw"),
        ),
        Feature::Dts => {
            Capability::of(feature, facts.dts, "AcerUltraX (DTS sound) is not installed.", "file system")
        }
        Feature::Rnnoise => Capability::of(
            feature,
            facts.rnnoise,
            "The RNNoise plug-in (noise-suppression-for-voice) is not installed.",
            "file system",
        ),
        Feature::Daemon => {
            Capability::of(feature, facts.daemon, "The TrueSense service is not running.", "control socket")
        }
    }
}

/// A switch of the firmware's battery interface: `caps` must list it and the read must answer.
fn battery_interface(feature: Feature, fw: &Firmware, command: &str, what: &str) -> Capability {
    let ok = fw.cap("battery") && fw.int(command, "battery").is_some_and(|v| v >= 0);
    let why = if fw.reply("caps").is_some() && !fw.cap("battery") {
        "This laptop's firmware has no battery interface.".to_owned()
    } else {
        fw.why_not(command, what)
    };
    Capability::of(feature, ok, why, from_firmware(command))
}

fn power_limits(m: &Machine, source: String) -> Capability {
    let lock = match m.rapl_lock_bit {
        Some(true) => "The firmware has locked the limits until the next restart.",
        Some(false) => "The limits are not locked.",
        None => "The limits' lock state needs root to read.",
    };
    if !m.rapl_present {
        return Capability::no(
            Feature::PowerLimits,
            "This kernel exposes no CPU power limits (the intel_rapl driver is missing).",
            source,
        );
    }
    if m.rapl_lock_bit == Some(true) || !m.rapl_writable {
        return Capability::no(Feature::PowerLimits, "The firmware has locked the CPU power limits.", source)
            .with_detail(lock);
    }
    Capability::yes(Feature::PowerLimits, source).with_detail(lock)
}

#[cfg(test)]
mod tests;
