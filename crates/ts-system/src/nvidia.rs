//! nvidia: the discrete NVIDIA GPU as the Monitor page needs it: whether it is asleep, and what
//! one `nvidia-smi --query-gpu` answer means.
//!
//! # Why this module is careful
//!
//! On the PH16-71 the discrete GPU is runtime-suspended whenever nothing uses it, which is what
//! gives the battery life. Two facts, both measured on this model and reported for the reference
//! project (its issue 79), shape everything here:
//!
//! * asking the card anything (`nvidia-smi`, NVML, even some sysfs files such as the PCIe link
//!   speed) resumes it, and every start of `nvidia-smi` initialises NVML, which on this laptop
//!   raises about 30 system management interrupts per second for as long as the initialisation
//!   lasts and stalls every CPU core;
//! * one *already initialised* session is silent, so a long-lived stream costs the stall once.
//!
//! So this module never queries a sleeping card ([`gpu_power`] reads only
//! `power/runtime_status`, which does not wake it), and callers must space one-shot queries out
//! (the application layer allows one every [`MIN_QUERY_INTERVAL_SECS`] seconds).
//!
//! The query line is parsed here; running the process is the caller's, through an injected
//! `CommandRunner`.
//!
//! Adapted from predator-sense by cleyton1986 (GPL-3.0), used with the author's permission: the
//! field list and the idea of treating `[N/A]` as "unknown" come from his `hardware/gpu.rs`.
#![forbid(unsafe_code)]

use std::fmt;
use std::path::Path;

use serde::Serialize;

use crate::usage::files::{below, dir_names, read_line};

/// The shortest time between two one-shot `nvidia-smi` queries made for the Monitor page. Each
/// query initialises NVML (see the module documentation), so a faster page would stall the CPU
/// cores several times a second and keep the card from suspending.
pub const MIN_QUERY_INTERVAL_SECS: f64 = 5.0;

/// The PCI vendor id of NVIDIA.
const NVIDIA_VENDOR: &str = "0x10de";

/// The fields asked of `nvidia-smi --query-gpu`, in the order [`parse_query`] expects. All of
/// them exist on every driver since 2019; a field the card does not support prints `[N/A]`.
pub const QUERY_FIELDS: [&str; 24] = [
    "name",
    "driver_version",
    "temperature.gpu",
    "utilization.gpu",
    "utilization.memory",
    "memory.total",
    "memory.used",
    "memory.free",
    "power.draw",
    "power.limit",
    "power.default_limit",
    "power.min_limit",
    "power.max_limit",
    "clocks.current.graphics",
    "clocks.current.memory",
    "clocks.max.graphics",
    "clocks.max.memory",
    "pstate",
    "pcie.link.gen.current",
    "pcie.link.gen.max",
    "pcie.link.width.current",
    "pcie.link.width.max",
    "fan.speed",
    "vbios_version",
];

/// The arguments for one query of the first GPU.
pub fn query_args() -> Vec<String> {
    vec![
        format!("--query-gpu={}", QUERY_FIELDS.join(",")),
        "--format=csv,noheader,nounits".to_owned(),
        "--id=0".to_owned(),
    ]
}

/// Whether the discrete GPU can be queried without waking it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuPower {
    /// Powered: querying it costs nothing extra.
    Active,
    /// Runtime-suspended (D3): do not touch it.
    Suspended,
    /// No NVIDIA display device on the PCI bus.
    Absent,
    /// A device exists but its power state cannot be read (no runtime PM); treated as powered
    /// because a card without runtime PM is never asleep.
    Unknown,
}

impl GpuPower {
    /// Whether a query is allowed.
    pub fn may_query(self) -> bool {
        matches!(self, Self::Active | Self::Unknown)
    }
}

/// The power state of the first NVIDIA display controller (PCI class 0x03xx) below `root`.
///
/// Reads only `vendor`, `class` and `power/runtime_status`, none of which wakes a sleeping device.
/// `suspending` counts as suspended (the card is going to sleep: do not interrupt it);
/// `resuming` counts as active (someone else already woke it).
pub fn gpu_power(root: &Path) -> GpuPower {
    let base = below(root, "/sys/bus/pci/devices");
    for name in dir_names(&base) {
        let dir = base.join(&name);
        if read_line(&dir.join("vendor")).as_deref() != Some(NVIDIA_VENDOR) {
            continue;
        }
        let is_display = read_line(&dir.join("class")).is_some_and(|c| c.starts_with("0x03"));
        if !is_display {
            continue; // the card's audio and USB functions share the vendor id
        }
        return match read_line(&dir.join("power/runtime_status")).as_deref() {
            Some("suspended" | "suspending") => GpuPower::Suspended,
            Some("active" | "resuming") => GpuPower::Active,
            _ => GpuPower::Unknown,
        };
    }
    GpuPower::Absent
}

/// Why a query answer could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryError {
    /// The program printed nothing.
    Empty,
    /// The first line did not have the expected number of fields (a driver message, an error).
    Shape {
        /// Fields found.
        found: usize,
    },
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("nvidia-smi printed nothing"),
            Self::Shape { found } => {
                write!(f, "nvidia-smi answered {found} fields, expected {}", QUERY_FIELDS.len())
            }
        }
    }
}

impl std::error::Error for QueryError {}

/// One answer of `nvidia-smi --query-gpu`. Every number is `None` when the driver printed
/// `[N/A]` or `[Not Supported]` (a laptop GPU often does for the fan and the default power limit).
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct GpuMetrics {
    /// Marketing name (`NVIDIA GeForce RTX 5070 Ti Laptop GPU`).
    pub name: Option<String>,
    /// Driver version.
    pub driver: Option<String>,
    /// VBIOS version.
    pub vbios: Option<String>,
    /// Core temperature in degrees C.
    pub temperature_c: Option<f64>,
    /// Graphics engine utilisation in percent.
    pub utilization_pct: Option<f64>,
    /// Memory controller utilisation in percent.
    pub memory_utilization_pct: Option<f64>,
    /// Video memory in MiB.
    pub vram_total_mib: Option<f64>,
    /// Video memory in use, MiB.
    pub vram_used_mib: Option<f64>,
    /// Video memory free, MiB.
    pub vram_free_mib: Option<f64>,
    /// Power draw in watts.
    pub power_w: Option<f64>,
    /// The enforced power limit in watts.
    pub power_limit_w: Option<f64>,
    /// The default power limit in watts.
    pub power_default_limit_w: Option<f64>,
    /// The lowest limit that can be set, watts.
    pub power_min_limit_w: Option<f64>,
    /// The highest limit that can be set, watts.
    pub power_max_limit_w: Option<f64>,
    /// Graphics clock in MHz.
    pub clock_graphics_mhz: Option<f64>,
    /// Memory clock in MHz.
    pub clock_memory_mhz: Option<f64>,
    /// Highest graphics clock in MHz.
    pub clock_graphics_max_mhz: Option<f64>,
    /// Highest memory clock in MHz.
    pub clock_memory_max_mhz: Option<f64>,
    /// Performance state, `P0` (fastest) to `P12`.
    pub pstate: Option<String>,
    /// PCIe generation now (it drops when the card idles).
    pub pcie_gen: Option<u32>,
    /// Highest PCIe generation of the link.
    pub pcie_gen_max: Option<u32>,
    /// PCIe lanes now.
    pub pcie_width: Option<u32>,
    /// Most PCIe lanes of the link.
    pub pcie_width_max: Option<u32>,
    /// Fan speed in percent; laptops report none.
    pub fan_pct: Option<f64>,
}

/// A number, or `None` for the driver's "no value" words and anything that is not a finite number.
fn number(field: &str) -> Option<f64> {
    field.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn count(field: &str) -> Option<u32> {
    field.parse().ok()
}

/// Text, or `None` for the driver's "no value" words (`[N/A]`, `N/A`, `[Not Supported]`, empty).
fn text(field: &str) -> Option<String> {
    let unusable = field.is_empty() || field.starts_with('[') || field.eq_ignore_ascii_case("n/a");
    (!unusable).then(|| field.to_owned())
}

/// Parses the first line of `nvidia-smi --query-gpu=... --format=csv,noheader,nounits`. Later
/// lines (a second GPU) are ignored: the page shows one. The line must have exactly the fields
/// of [`QUERY_FIELDS`]; anything else (an error message, a changed driver) is a [`QueryError`].
pub fn parse_query(output: &str) -> Result<GpuMetrics, QueryError> {
    let line = output.lines().map(str::trim).find(|l| !l.is_empty()).ok_or(QueryError::Empty)?;
    let fields: Vec<&str> = line.split(',').map(str::trim).collect();
    if fields.len() != QUERY_FIELDS.len() {
        return Err(QueryError::Shape { found: fields.len() });
    }
    let f = |i: usize| fields[i];
    Ok(GpuMetrics {
        name: text(f(0)),
        driver: text(f(1)),
        temperature_c: number(f(2)),
        utilization_pct: number(f(3)),
        memory_utilization_pct: number(f(4)),
        vram_total_mib: number(f(5)),
        vram_used_mib: number(f(6)),
        vram_free_mib: number(f(7)),
        power_w: number(f(8)),
        power_limit_w: number(f(9)),
        power_default_limit_w: number(f(10)),
        power_min_limit_w: number(f(11)),
        power_max_limit_w: number(f(12)),
        clock_graphics_mhz: number(f(13)),
        clock_memory_mhz: number(f(14)),
        clock_graphics_max_mhz: number(f(15)),
        clock_memory_max_mhz: number(f(16)),
        pstate: text(f(17)),
        pcie_gen: count(f(18)),
        pcie_gen_max: count(f(19)),
        pcie_width: count(f(20)),
        pcie_width_max: count(f(21)),
        fan_pct: number(f(22)),
        vbios: text(f(23)),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::usage::files::testutil::Tree;

    /// What a PH16-71 printed (values rounded): the fan and the default limit are not supported.
    const LINE: &str = "NVIDIA GeForce RTX 5070 Ti Laptop GPU, 580.95.05, 47, 3, 1, 12227, 1210, 10780, 14.86, 115.00, [N/A], 5.00, 115.00, 705, 405, 3105, 12001, P8, 1, 5, 16, 16, [N/A], 98.05.50.00.4B\n";

    #[test]
    fn a_real_line_parses_with_not_available_fields_as_none() {
        let m = parse_query(LINE).unwrap();
        assert_eq!(m.name.as_deref(), Some("NVIDIA GeForce RTX 5070 Ti Laptop GPU"));
        assert_eq!((m.temperature_c, m.utilization_pct, m.power_w), (Some(47.0), Some(3.0), Some(14.86)));
        assert_eq!(
            (m.vram_total_mib, m.vram_used_mib, m.vram_free_mib),
            (Some(12227.0), Some(1210.0), Some(10780.0))
        );
        assert_eq!(
            (m.power_limit_w, m.power_default_limit_w, m.power_max_limit_w),
            (Some(115.0), None, Some(115.0))
        );
        assert_eq!((m.clock_graphics_mhz, m.clock_memory_mhz), (Some(705.0), Some(405.0)));
        assert_eq!(m.pstate.as_deref(), Some("P8"));
        assert_eq!(
            (m.pcie_gen, m.pcie_gen_max, m.pcie_width, m.pcie_width_max),
            (Some(1), Some(5), Some(16), Some(16))
        );
        assert_eq!(m.fan_pct, None);
        assert_eq!(m.vbios.as_deref(), Some("98.05.50.00.4B"));
    }

    #[test]
    fn query_arguments_match_the_parser() {
        let args = query_args();
        assert!(args[0].starts_with("--query-gpu=name,driver_version,"));
        assert_eq!(args[0].matches(',').count() + 1, QUERY_FIELDS.len());
        assert!(args.contains(&"--format=csv,noheader,nounits".to_owned()));
    }

    #[test]
    fn hostile_answers_are_errors_or_unknowns_never_panics() {
        assert_eq!(parse_query(""), Err(QueryError::Empty));
        assert_eq!(parse_query("\n  \n"), Err(QueryError::Empty));
        assert!(matches!(parse_query("No devices were found"), Err(QueryError::Shape { found: 1 })));
        assert!(
            parse_query("NVIDIA-SMI has failed because it could not communicate with the NVIDIA driver.")
                .is_err()
        );
        assert!(matches!(parse_query("a, b, c"), Err(QueryError::Shape { found: 3 })));
        let too_many = LINE.replace('\n', ", extra\n");
        assert!(matches!(parse_query(&too_many), Err(QueryError::Shape { found: 25 })));
        // every numeric field garbled: all None, name kept
        let garbled = "n, d, NaN, inf, -inf, 1e999, abc, , [Not Supported], [Unknown Error], x, y, z, 1.2.3, --, N/A, n/a, [N/A], -1, 99999999999, 4294967296, , 7, v";
        let m = parse_query(garbled).unwrap();
        assert_eq!(
            (m.temperature_c, m.utilization_pct, m.memory_utilization_pct, m.vram_total_mib),
            (None, None, None, None)
        );
        assert_eq!((m.pcie_gen, m.pcie_gen_max, m.pcie_width), (None, None, None));
        assert_eq!(m.pstate, None);
        assert_eq!(m.name.as_deref(), Some("n"));
        assert_eq!(m.fan_pct, Some(7.0));
        // a driver warning before the data line is a shape error, not data: first non-empty line is used
        assert!(parse_query(&format!("WARNING: something\n{LINE}")).is_err());
        // a second GPU is ignored
        assert!(parse_query(&format!("{LINE}{LINE}")).is_ok());
        let _ = parse_query("\0\0\0,\0");
    }

    fn pci(t: &Tree, address: &str, vendor: &str, class: &str, status: Option<&str>) {
        t.put(&format!("sys/bus/pci/devices/{address}/vendor"), &format!("{vendor}\n"));
        t.put(&format!("sys/bus/pci/devices/{address}/class"), &format!("{class}\n"));
        if let Some(s) = status {
            t.put(&format!("sys/bus/pci/devices/{address}/power/runtime_status"), &format!("{s}\n"));
        }
    }

    #[test]
    fn runtime_status_decides_whether_the_card_may_be_asked() {
        let t = Tree::new();
        assert_eq!(gpu_power(t.root()), GpuPower::Absent);
        pci(&t, "0000:00:02.0", "0x8086", "0x030000", Some("active")); // the Intel iGPU
        assert_eq!(gpu_power(t.root()), GpuPower::Absent, "another vendor's display is not it");
        pci(&t, "0000:01:00.0", "0x10de", "0x030200", Some("suspended"));
        assert_eq!(gpu_power(t.root()), GpuPower::Suspended);
        assert!(!GpuPower::Suspended.may_query());
        for (status, expected) in [
            ("active", GpuPower::Active),
            ("resuming", GpuPower::Active),
            ("suspending", GpuPower::Suspended),
            ("unsupported", GpuPower::Unknown),
            ("error", GpuPower::Unknown),
            ("", GpuPower::Unknown),
            ("SUSPENDED", GpuPower::Unknown),
        ] {
            t.put("sys/bus/pci/devices/0000:01:00.0/power/runtime_status", &format!("{status}\n"));
            assert_eq!(gpu_power(t.root()), expected, "{status:?}");
            assert_eq!(expected.may_query(), expected != GpuPower::Suspended);
        }
        t.remove("sys/bus/pci/devices/0000:01:00.0/power");
        assert_eq!(gpu_power(t.root()), GpuPower::Unknown, "no runtime PM files");
    }

    #[test]
    fn the_cards_audio_function_is_not_the_gpu() {
        let t = Tree::new();
        pci(&t, "0000:01:00.0", "0x10de", "0x040300", Some("suspended")); // HDMI audio, same vendor
        assert_eq!(gpu_power(t.root()), GpuPower::Absent);
        pci(&t, "0000:01:00.1", "0x10de", "0x030000", Some("active"));
        assert_eq!(gpu_power(t.root()), GpuPower::Active);
    }
}
