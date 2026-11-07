//! temps: every temperature sensor the kernel exposes through hwmon, with a label and a kind, and
//! pure helpers that remember the lowest and highest value seen.
//!
//! The Monitor page shows CPU package, board (ACPI), NVMe drives, the Wi-Fi card, memory modules
//! and the GPU. Their hwmon numbers change between boots, so a sensor is identified by the chip's
//! `name` and the file's own number (`nvme/temp1`), and a second chip of the same name gets an
//! index (`nvme#1/temp1`). Readings that are not plausible temperatures are dropped.
//!
//! Cost to know about: an NVMe temperature is an admin command to the drive (about 9 ms) and the
//! Wi-Fi one a firmware call, so the page should not read faster than every two seconds.
//!
//! The NVIDIA GPU has no hwmon entry with the proprietary driver and its temperature is read from
//! the firmware by the application layer, never by waking the card. A chip of a GPU driver that
//! does expose hwmon (`nouveau`, `amdgpu`) is skipped when asked to, because reading it resumes a
//! sleeping card.
//!
//! Adapted from predator-sense by cleyton1986 (GPL-3.0), used with the author's permission: which
//! chips to look for (coretemp, acpitz, nvme, iwlwifi, spd5118) comes from his `hardware/sensors.rs`.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

use crate::usage::files::{below, dir_names, indexed, read_i64, read_line};

/// The lowest and highest temperature a sensor can really report; anything else is a broken read
/// (some drivers answer `-274000` or `0x7fffffff` for "no sensor").
const PLAUSIBLE: std::ops::RangeInclusive<f64> = -50.0..=160.0;

/// The most temperature inputs read from one chip.
const MAX_INPUTS: usize = 64;

/// What a sensor measures, for placing it on the right gauge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SensorKind {
    /// The whole CPU package.
    CpuPackage,
    /// One CPU core.
    CpuCore,
    /// Another CPU sensor (an AMD `Tctl`, a die).
    Cpu,
    /// The motherboard or chassis (ACPI thermal zone, the firmware's system sensor).
    Board,
    /// An NVMe drive.
    Nvme,
    /// The Wi-Fi card.
    Wifi,
    /// The graphics processor.
    Gpu,
    /// A memory module.
    Memory,
    /// Anything else with a temperature.
    Other,
}

/// One temperature reading.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TempSensor {
    /// A name that stays the same between boots (`coretemp/temp1`, `nvme#1/temp1`).
    pub id: String,
    /// The driver's chip name (`coretemp`, `nvme`, `acpitz`).
    pub chip: String,
    /// The sensor's own label (`Package id 0`, `Composite`), or `temp1` when it has none.
    pub label: String,
    /// What it measures.
    pub kind: SensorKind,
    /// Degrees Celsius.
    pub celsius: f64,
    /// The driver's warning threshold, when it gives one.
    pub max_c: Option<f64>,
    /// The driver's critical threshold, when it gives one.
    pub crit_c: Option<f64>,
}

/// Decides what a chip's sensor measures from the chip name and the sensor's label.
pub fn classify(chip: &str, label: &str) -> SensorKind {
    let chip = chip.to_ascii_lowercase();
    let label = label.to_ascii_lowercase();
    match chip.as_str() {
        "coretemp" | "k10temp" | "zenpower" | "cpu_thermal" => {
            if label.starts_with("package") || label == "tctl" || label == "tdie" {
                SensorKind::CpuPackage
            } else if label.starts_with("core") {
                SensorKind::CpuCore
            } else {
                SensorKind::Cpu
            }
        }
        "acpitz" | "thinkpad" | "acer" | "facer" | "truesense" | "pch_cannonlake" | "pch_skylake" => {
            SensorKind::Board
        }
        "nvme" => SensorKind::Nvme,
        "nouveau" | "amdgpu" | "radeon" | "nvidia" => SensorKind::Gpu,
        "spd5118" | "jc42" | "ee1004" => SensorKind::Memory,
        c if c.starts_with("iwlwifi")
            || c.starts_with("mt79")
            || c.starts_with("ath")
            || c.starts_with("rtw") =>
        {
            SensorKind::Wifi
        }
        _ => SensorKind::Other,
    }
}

/// Whether a chip is a graphics driver whose hwmon read resumes a runtime-suspended card.
pub(crate) fn is_gpu_chip(chip: &str) -> bool {
    classify(chip, "") == SensorKind::Gpu
}

/// Which chips [`read_all`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReadOptions {
    /// Skip the graphics drivers' chips (the card is asleep and must stay so).
    pub skip_gpu_chips: bool,
}

fn celsius(path: &Path) -> Option<f64> {
    let milli = read_i64(path)?;
    let value = milli as f64 / 1000.0;
    PLAUSIBLE.contains(&value).then_some(value)
}

/// Reads every plausible temperature of every hwmon chip under `root`, chips in hwmon order and
/// inputs in number order. A chip or input that cannot be read is skipped; nothing here fails.
pub fn read_all(root: &Path, options: ReadOptions) -> Vec<TempSensor> {
    let base = below(root, "/sys/class/hwmon");
    let mut chips = Vec::new();
    for entry in dir_names(&base) {
        if indexed(&entry, "hwmon").is_none() {
            continue;
        }
        let dir = base.join(&entry);
        let Some(chip) = read_line(&dir.join("name")).filter(|n| !n.is_empty()) else { continue };
        chips.push((chip, dir));
    }
    // The same chip name twice (two NVMe drives) is numbered in hwmon order.
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut out = Vec::new();
    for (chip, dir) in chips {
        let ordinal = {
            let n = seen.entry(chip.clone()).or_insert(0);
            *n += 1;
            *n - 1
        };
        if options.skip_gpu_chips && is_gpu_chip(&chip) {
            continue;
        }
        let prefix = if ordinal == 0 { chip.clone() } else { format!("{chip}#{ordinal}") };
        let mut inputs: Vec<usize> = dir_names(&dir)
            .iter()
            .filter_map(|n| n.strip_suffix("_input").and_then(|s| indexed(s, "temp")))
            .collect();
        inputs.sort_unstable();
        inputs.truncate(MAX_INPUTS);
        for n in inputs {
            let Some(value) = celsius(&dir.join(format!("temp{n}_input"))) else { continue };
            let label = read_line(&dir.join(format!("temp{n}_label"))).filter(|l| !l.is_empty());
            let label = label.unwrap_or_else(|| format!("temp{n}"));
            out.push(TempSensor {
                id: format!("{prefix}/temp{n}"),
                kind: classify(&chip, &label),
                chip: chip.clone(),
                label,
                celsius: value,
                max_c: celsius(&dir.join(format!("temp{n}_max"))),
                crit_c: celsius(&dir.join(format!("temp{n}_crit"))),
            });
        }
    }
    out
}

/// The ACPI thermal zone as a board sensor, for machines that have the zone but no hwmon chip for
/// it. `None` when there is no zone of that type.
pub fn read_thermal_zone(root: &Path, zone_type: &str) -> Option<TempSensor> {
    let base = below(root, "/sys/class/thermal");
    for name in dir_names(&base) {
        if indexed(&name, "thermal_zone").is_none() {
            continue;
        }
        let dir = base.join(&name);
        if read_line(&dir.join("type")).as_deref() != Some(zone_type) {
            continue;
        }
        let Some(value) = celsius(&dir.join("temp")) else { continue };
        return Some(TempSensor {
            id: format!("thermal/{zone_type}"),
            chip: zone_type.to_owned(),
            label: zone_type.to_owned(),
            kind: classify(zone_type, ""),
            celsius: value,
            max_c: None,
            crit_c: None,
        });
    }
    None
}

/// The lowest, highest and latest value of one sensor.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Extremes {
    /// The lowest value seen.
    pub min: f64,
    /// The highest value seen.
    pub max: f64,
    /// The latest value.
    pub last: f64,
    /// How many readings were folded in.
    pub samples: u64,
}

impl Extremes {
    fn new(value: f64) -> Self {
        Self { min: value, max: value, last: value, samples: 1 }
    }

    fn fold(&mut self, value: f64) {
        self.min = self.min.min(value);
        self.max = self.max.max(value);
        self.last = value;
        self.samples = self.samples.saturating_add(1);
    }
}

/// Remembers the lowest and highest value of each sensor since it was created or reset.
#[derive(Debug, Clone, Default)]
pub struct ExtremesTracker {
    seen: BTreeMap<String, Extremes>,
}

impl ExtremesTracker {
    /// An empty tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Folds one reading in; a value that is not finite is ignored. Answers the sensor's extremes.
    pub fn observe(&mut self, id: &str, value: f64) -> Option<Extremes> {
        if !value.is_finite() {
            return self.seen.get(id).copied();
        }
        let entry = match self.seen.get_mut(id) {
            Some(e) => {
                e.fold(value);
                *e
            }
            None => {
                let e = Extremes::new(value);
                self.seen.insert(id.to_owned(), e);
                e
            }
        };
        Some(entry)
    }

    /// The extremes of a sensor, if it was ever seen.
    pub fn get(&self, id: &str) -> Option<Extremes> {
        self.seen.get(id).copied()
    }

    /// Forgets everything (the page's reset button).
    pub fn reset(&mut self) {
        self.seen.clear();
    }

    /// How many sensors are tracked.
    pub fn len(&self) -> usize {
        self.seen.len()
    }

    /// Whether nothing is tracked.
    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::usage::files::testutil::Tree;

    /// The hwmon tree of a PH16-71: coretemp, acpitz, two NVMe drives, Wi-Fi, two DIMMs, the Acer EC.
    fn laptop() -> Tree {
        let t = Tree::new();
        let chip = |n: u32, name: &str, inputs: &[(u32, Option<&str>, i64)]| {
            t.put(&format!("sys/class/hwmon/hwmon{n}/name"), &format!("{name}\n"));
            for (i, label, milli) in inputs {
                t.put(&format!("sys/class/hwmon/hwmon{n}/temp{i}_input"), &format!("{milli}\n"));
                if let Some(l) = label {
                    t.put(&format!("sys/class/hwmon/hwmon{n}/temp{i}_label"), &format!("{l}\n"));
                }
            }
        };
        chip(0, "acpitz", &[(1, None, 27800)]);
        chip(1, "nvme", &[(1, Some("Composite"), 41850), (2, Some("Sensor 1"), 41850)]);
        chip(2, "nvme", &[(1, Some("Composite"), 36850)]);
        chip(3, "iwlwifi_1", &[(1, None, 39000)]);
        chip(
            4,
            "coretemp",
            &[(1, Some("Package id 0"), 52000), (2, Some("Core 0"), 49000), (3, Some("Core 1"), 51000)],
        );
        chip(10, "spd5118", &[(1, None, 33250)]);
        t.put("sys/class/hwmon/hwmon4/temp1_max", "100000\n");
        t.put("sys/class/hwmon/hwmon4/temp1_crit", "100000\n");
        t
    }

    #[test]
    fn every_chip_of_a_laptop_is_read_in_hwmon_order_with_kinds() {
        let t = laptop();
        let all = read_all(t.root(), ReadOptions::default());
        let ids: Vec<&str> = all.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "acpitz/temp1",
                "nvme/temp1",
                "nvme/temp2",
                "nvme#1/temp1",
                "iwlwifi_1/temp1",
                "coretemp/temp1",
                "coretemp/temp2",
                "coretemp/temp3",
                "spd5118/temp1"
            ],
            "hwmon10 sorts after hwmon4"
        );
        let get = |id: &str| all.iter().find(|s| s.id == id).unwrap();
        assert_eq!(get("coretemp/temp1").kind, SensorKind::CpuPackage);
        assert_eq!(get("coretemp/temp1").label, "Package id 0");
        assert_eq!((get("coretemp/temp1").max_c, get("coretemp/temp1").crit_c), (Some(100.0), Some(100.0)));
        assert_eq!(get("coretemp/temp2").kind, SensorKind::CpuCore);
        assert_eq!(get("acpitz/temp1").kind, SensorKind::Board);
        assert_eq!(get("acpitz/temp1").label, "temp1", "no label file: the input's own name");
        assert_eq!(get("nvme#1/temp1").celsius, 36.85);
        assert_eq!(get("iwlwifi_1/temp1").kind, SensorKind::Wifi);
        assert_eq!(get("spd5118/temp1").kind, SensorKind::Memory);
    }

    #[test]
    fn garbage_and_implausible_values_are_dropped() {
        let t = Tree::new();
        t.put("sys/class/hwmon/hwmon0/name", "weird\n");
        for (i, v) in ["N/A", "-274000", "2147483647", "", "12.5", "50000"].iter().enumerate() {
            t.put(&format!("sys/class/hwmon/hwmon0/temp{}_input", i + 1), v);
        }
        t.put("sys/class/hwmon/hwmon1/name", "\n");
        t.put("sys/class/hwmon/hwmon1/temp1_input", "40000");
        t.put("sys/class/hwmon/hwmonX/name", "x\n");
        t.put("sys/class/hwmon/hwmon2/temp1_input", "40000");
        t.put("sys/class/hwmon/hwmon0/temp99999999999999_input", "40000");
        let all = read_all(t.root(), ReadOptions::default());
        assert_eq!(all.iter().map(|s| s.celsius).collect::<Vec<_>>(), [50.0]);
        assert!(read_all(&t.root().join("nowhere"), ReadOptions::default()).is_empty());
    }

    #[test]
    fn a_sleeping_gpu_chip_can_be_left_alone() {
        let t = Tree::new();
        t.put("sys/class/hwmon/hwmon0/name", "nouveau\n");
        t.put("sys/class/hwmon/hwmon0/temp1_input", "45000");
        t.put("sys/class/hwmon/hwmon1/name", "coretemp\n");
        t.put("sys/class/hwmon/hwmon1/temp1_input", "50000");
        assert_eq!(read_all(t.root(), ReadOptions::default()).len(), 2);
        let skipped = read_all(t.root(), ReadOptions { skip_gpu_chips: true });
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].chip, "coretemp");
    }

    #[test]
    fn a_chip_that_vanishes_between_listing_and_reading_is_skipped() {
        let t = laptop();
        let before = read_all(t.root(), ReadOptions::default()).len();
        t.remove("sys/class/hwmon/hwmon2/temp1_input"); // the drive was unplugged
        assert_eq!(read_all(t.root(), ReadOptions::default()).len(), before - 1);
    }

    #[test]
    fn classification_covers_the_known_chips() {
        assert_eq!(classify("k10temp", "Tctl"), SensorKind::CpuPackage);
        assert_eq!(classify("k10temp", "Tccd1"), SensorKind::Cpu);
        assert_eq!(classify("amdgpu", "edge"), SensorKind::Gpu);
        assert_eq!(classify("mt7921_phy0", ""), SensorKind::Wifi);
        assert_eq!(classify("whatever", ""), SensorKind::Other);
        assert!(is_gpu_chip("nouveau") && !is_gpu_chip("nvme"));
    }

    #[test]
    fn the_acpi_zone_is_found_by_type() {
        let t = Tree::new();
        t.put("sys/class/thermal/thermal_zone0/type", "x86_pkg_temp\n");
        t.put("sys/class/thermal/thermal_zone0/temp", "55000\n");
        t.put("sys/class/thermal/thermal_zone1/type", "acpitz\n");
        t.put("sys/class/thermal/thermal_zone1/temp", "28000\n");
        t.put("sys/class/thermal/cooling_device0/type", "acpitz\n");
        let zone = read_thermal_zone(t.root(), "acpitz").unwrap();
        assert_eq!((zone.celsius, zone.kind, zone.id.as_str()), (28.0, SensorKind::Board, "thermal/acpitz"));
        assert_eq!(read_thermal_zone(t.root(), "missing"), None);
    }

    #[test]
    fn the_tracker_keeps_min_max_and_last_and_resets() {
        let mut t = ExtremesTracker::new();
        assert!(t.is_empty());
        assert_eq!(t.observe("cpu", 50.0), Some(Extremes { min: 50.0, max: 50.0, last: 50.0, samples: 1 }));
        t.observe("cpu", 40.0);
        let e = t.observe("cpu", 45.0).unwrap();
        assert_eq!((e.min, e.max, e.last, e.samples), (40.0, 50.0, 45.0, 3));
        assert_eq!(t.observe("cpu", f64::NAN).unwrap().samples, 3, "NaN is ignored");
        assert_eq!(t.observe("gpu", f64::INFINITY), None);
        assert_eq!(t.len(), 1);
        t.reset();
        assert_eq!(t.get("cpu"), None);
        assert!(t.is_empty());
    }
}
