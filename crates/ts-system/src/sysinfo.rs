//! sysinfo: the facts about this machine that do not change while it runs: which laptop, CPU,
//! memory, graphics devices, drives, operating system and kernel. The Monitor page's dashboard
//! and the About box show them.
//!
//! Everything comes from files below an injected root, so a test builds a fake laptop in a
//! temporary directory. Two things need a process and are not done here: the NVIDIA GPU's marketing
//! name and driver (`nvidia-smi` wakes a sleeping card; the application layer asks it only when the
//! card is awake and fills [`GpuDevice::name`]), and anything needing root (the serial number is
//! read when the file is readable and left out otherwise).
//!
//! Adapted from predator-sense by cleyton1986 (GPL-3.0), used with the author's permission: which
//! files tell the CPU model, core count, OS name and drives is from his `hardware/sysinfo.rs`.
#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::path::Path;

use serde::Serialize;

use crate::disks::{BlockDevice, block_devices};
use crate::models::{self, Model};
use crate::usage::cpu::read_freqs;
use crate::usage::files::{below, dir_names, read_line, read_text};
use crate::usage::memory::read_meminfo;

/// The CPU.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct CpuInfo {
    /// `model name` of `/proc/cpuinfo`.
    pub model: Option<String>,
    /// Sockets (almost always 1 on a laptop).
    pub sockets: Option<u32>,
    /// Physical cores: distinct (socket, core id) pairs.
    pub cores: Option<u32>,
    /// Logical processors (hardware threads).
    pub threads: u32,
    /// The highest frequency of any core in MHz.
    pub max_mhz: Option<u32>,
}

/// The operating system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct OsInfo {
    /// `PRETTY_NAME` (`CachyOS Linux`).
    pub pretty_name: String,
    /// `ID` (`cachyos`).
    pub id: Option<String>,
    /// `VERSION_ID`, when the distribution has one (rolling releases do not).
    pub version: Option<String>,
}

/// A graphics device on the PCI bus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GpuDevice {
    /// PCI address (`0000:01:00.0`).
    pub address: String,
    /// `NVIDIA`, `Intel`, `AMD`, or the raw vendor id.
    pub vendor: String,
    /// The PCI device id (`0x2c19`), which identifies the model without a name database.
    pub device_id: Option<String>,
    /// Whether the firmware used this one at boot (the integrated GPU on a hybrid laptop).
    pub boot_vga: bool,
    /// The marketing name, when something asked for it (filled in by the application layer for
    /// the NVIDIA card; the kernel has no name for it).
    pub name: Option<String>,
}

/// Everything static about the machine.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SystemInfo {
    /// The laptop: vendor, product, BIOS, how far TrueSense trusts it.
    pub machine: Model,
    /// The DMI serial number when this user may read it (it is root-only on most systems).
    pub serial: Option<String>,
    /// The processor.
    pub cpu: CpuInfo,
    /// Installed memory in bytes.
    pub memory_total_bytes: Option<u64>,
    /// Graphics devices.
    pub gpus: Vec<GpuDevice>,
    /// Physical drives.
    pub drives: Vec<BlockDevice>,
    /// The operating system.
    pub os: OsInfo,
    /// The kernel release (`6.12.1-1-cachyos`).
    pub kernel: Option<String>,
    /// Seconds since boot.
    pub uptime_s: Option<f64>,
}

/// The first value of `key` in a `key : value` file such as `/proc/cpuinfo`.
fn first_field<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|line| {
        let (k, v) = line.split_once(':')?;
        (k.trim() == key).then(|| v.trim())
    })
}

/// Parses `/proc/cpuinfo`. x86 lists one block per logical processor with `physical id` and
/// `core id`; on a machine without them (a virtual machine, ARM) the core count is unknown, not guessed.
pub fn parse_cpuinfo(text: &str) -> CpuInfo {
    let model = first_field(text, "model name").or_else(|| first_field(text, "Model")).map(str::to_owned);
    let mut threads = 0u32;
    let mut sockets = BTreeSet::new();
    let mut cores = BTreeSet::new();
    let (mut physical, mut core): (Option<&str>, Option<&str>) = (None, None);
    let mut flush = |physical: &mut Option<&str>, core: &mut Option<&str>| {
        if let Some(p) = physical.take() {
            sockets.insert(p.to_owned());
            if let Some(c) = core.take() {
                cores.insert((p.to_owned(), c.to_owned()));
            }
        }
        *core = None;
    };
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            // The blank line between two processors' blocks.
            if line.trim().is_empty() {
                flush(&mut physical, &mut core);
            }
            continue;
        };
        match key.trim() {
            "processor" => threads = threads.saturating_add(1),
            "physical id" => physical = Some(value.trim()),
            "core id" => core = Some(value.trim()),
            _ => {}
        }
    }
    flush(&mut physical, &mut core);
    let count = |n: usize| u32::try_from(n).ok().filter(|n| *n > 0);
    CpuInfo { model, sockets: count(sockets.len()), cores: count(cores.len()), threads, max_mhz: None }
}

/// Parses `/etc/os-release`: `KEY=value` lines, the value optionally in single or double quotes.
pub fn parse_os_release(text: &str) -> OsInfo {
    let value = |key: &str| {
        text.lines().find_map(|line| {
            let rest = line.trim().strip_prefix(key)?.strip_prefix('=')?;
            let v = rest.trim().trim_matches(|c| c == '"' || c == '\'');
            (!v.is_empty()).then(|| v.to_owned())
        })
    };
    OsInfo {
        pretty_name: value("PRETTY_NAME").or_else(|| value("NAME")).unwrap_or_else(|| "Linux".to_owned()),
        id: value("ID"),
        version: value("VERSION_ID"),
    }
}

fn vendor_name(id: &str) -> String {
    match id {
        "0x10de" => "NVIDIA".to_owned(),
        "0x8086" => "Intel".to_owned(),
        "0x1002" | "0x1022" => "AMD".to_owned(),
        other => other.to_owned(),
    }
}

/// The display controllers (PCI class 0x03xx) in address order. Reads only `vendor`, `class`,
/// `device` and `boot_vga`, which do not wake a sleeping card.
pub fn gpu_devices(root: &Path) -> Vec<GpuDevice> {
    let base = below(root, "/sys/bus/pci/devices");
    dir_names(&base)
        .into_iter()
        .filter_map(|address| {
            let dir = base.join(&address);
            if !read_line(&dir.join("class"))?.starts_with("0x03") {
                return None;
            }
            Some(GpuDevice {
                vendor: vendor_name(&read_line(&dir.join("vendor"))?),
                device_id: read_line(&dir.join("device")).filter(|d| !d.is_empty()),
                boot_vga: read_line(&dir.join("boot_vga")).as_deref() == Some("1"),
                address,
                name: None,
            })
        })
        .collect()
}

/// Strings firmware vendors leave in DMI fields they did not fill in.
fn is_placeholder(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    text.is_empty()
        || [
            "default string",
            "to be filled",
            "not applicable",
            "not specified",
            "system serial",
            "none",
            "n/a",
        ]
        .iter()
        .any(|p| lower.contains(p))
}

/// Reads everything static below `root` (`/` on a real machine).
pub fn read(root: &Path) -> SystemInfo {
    let mut cpu = read_text(&below(root, "/proc/cpuinfo")).map(|t| parse_cpuinfo(&t)).unwrap_or_default();
    cpu.max_mhz = read_freqs(root).iter().filter_map(|f| f.max_mhz).max();
    let os_text =
        read_text(&below(root, "/etc/os-release")).or_else(|| read_text(&below(root, "/usr/lib/os-release")));
    SystemInfo {
        machine: models::identify(&below(root, models::DMI_ROOT)),
        serial: read_line(&below(root, models::DMI_ROOT).join("product_serial"))
            .filter(|s| !is_placeholder(s)),
        cpu,
        memory_total_bytes: read_meminfo(root).map(|m| m.total_kb.saturating_mul(1024)),
        gpus: gpu_devices(root),
        drives: block_devices(root),
        os: os_text.map(|t| parse_os_release(&t)).unwrap_or_else(|| parse_os_release("")),
        kernel: read_line(&below(root, "/proc/sys/kernel/osrelease")).filter(|k| !k.is_empty()),
        uptime_s: read_line(&below(root, "/proc/uptime"))
            .and_then(|l| l.split_whitespace().next().and_then(|s| s.parse::<f64>().ok()))
            .filter(|u| u.is_finite() && *u >= 0.0),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::usage::files::testutil::Tree;

    /// Two logical processors of one hyper-threaded core, one more core, and a hybrid E-core.
    const CPUINFO: &str = "processor\t: 0\nvendor_id\t: GenuineIntel\nmodel name\t: Intel(R) Core(TM) Ultra 9 275HX\ncpu MHz\t\t: 800.000\nphysical id\t: 0\nsiblings\t: 4\ncore id\t\t: 0\ncpu cores\t: 3\n\n\
        processor\t: 1\nmodel name\t: Intel(R) Core(TM) Ultra 9 275HX\nphysical id\t: 0\ncore id\t\t: 0\n\n\
        processor\t: 2\nmodel name\t: Intel(R) Core(TM) Ultra 9 275HX\nphysical id\t: 0\ncore id\t\t: 4\n\n\
        processor\t: 3\nmodel name\t: Intel(R) Core(TM) Ultra 9 275HX\nphysical id\t: 0\ncore id\t\t: 8\n";

    #[test]
    fn cpuinfo_counts_physical_cores_by_distinct_ids() {
        let cpu = parse_cpuinfo(CPUINFO);
        assert_eq!(cpu.model.as_deref(), Some("Intel(R) Core(TM) Ultra 9 275HX"));
        assert_eq!((cpu.threads, cpu.cores, cpu.sockets), (4, Some(3), Some(1)));
    }

    #[test]
    fn two_sockets_with_the_same_core_ids_are_four_cores() {
        let text = "processor: 0\nphysical id: 0\ncore id: 0\n\nprocessor: 1\nphysical id: 1\ncore id: 0\n\nprocessor: 2\nphysical id: 0\ncore id: 1\n\nprocessor: 3\nphysical id: 1\ncore id: 1\n";
        let cpu = parse_cpuinfo(text);
        assert_eq!((cpu.cores, cpu.sockets, cpu.threads), (Some(4), Some(2), 4));
    }

    #[test]
    fn a_machine_without_core_ids_has_unknown_cores_not_a_guess() {
        let cpu = parse_cpuinfo(
            "processor : 0\nmodel name : Virtual CPU\n\nprocessor : 1\nmodel name : Virtual CPU\n",
        );
        assert_eq!((cpu.cores, cpu.sockets, cpu.threads), (None, None, 2));
        assert_eq!(cpu.model.as_deref(), Some("Virtual CPU"));
    }

    #[test]
    fn hostile_cpuinfo_never_panics() {
        for text in
            ["", "\n\n\n", "processor", ":::", "model name:", "core id: x\nphysical id\n", "\0\0:\0\n\n\0"]
        {
            let _ = parse_cpuinfo(text);
        }
        assert_eq!(parse_cpuinfo("").threads, 0);
        assert_eq!(parse_cpuinfo("model name:\n").model.as_deref(), Some(""));
    }

    #[test]
    fn os_release_handles_quotes_and_fallbacks() {
        let os = parse_os_release(
            "NAME=\"CachyOS Linux\"\nPRETTY_NAME=\"CachyOS Linux\"\nID=cachyos\nBUILD_ID=rolling\n",
        );
        assert_eq!(
            (os.pretty_name.as_str(), os.id.as_deref(), os.version),
            ("CachyOS Linux", Some("cachyos"), None)
        );
        let single = parse_os_release("PRETTY_NAME='Debian GNU/Linux 12 (bookworm)'\nVERSION_ID=\"12\"\n");
        assert_eq!(
            (single.pretty_name.as_str(), single.version.as_deref()),
            ("Debian GNU/Linux 12 (bookworm)", Some("12"))
        );
        assert_eq!(parse_os_release("NAME=Foo\n").pretty_name, "Foo");
        assert_eq!(parse_os_release("").pretty_name, "Linux");
        assert_eq!(parse_os_release("PRETTY_NAME=\n").pretty_name, "Linux");
        assert_eq!(
            parse_os_release("XPRETTY_NAME=x\n  PRETTY_NAME = spaced\n").pretty_name,
            "Linux",
            "only KEY=value"
        );
    }

    fn laptop() -> Tree {
        let t = Tree::new();
        let dmi = "sys/class/dmi/id";
        t.put(&format!("{dmi}/sys_vendor"), "Acer\n");
        t.put(&format!("{dmi}/product_name"), "Predator PH16-71\n");
        t.put(&format!("{dmi}/bios_version"), "V1.18\n");
        t.put(&format!("{dmi}/bios_date"), "05/12/2025\n");
        t.put("proc/cpuinfo", CPUINFO);
        t.put("proc/meminfo", "MemTotal:       32812116 kB\nMemAvailable: 1 kB\n");
        t.put("proc/sys/kernel/osrelease", "6.12.1-1-cachyos\n");
        t.put("proc/uptime", "12345.67 54321.00\n");
        t.put("etc/os-release", "PRETTY_NAME=\"CachyOS Linux\"\nID=cachyos\n");
        t.put("sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq", "5400000\n");
        t.put("sys/devices/system/cpu/cpu2/cpufreq/cpuinfo_max_freq", "4600000\n");
        let pci = "sys/bus/pci/devices";
        t.put(&format!("{pci}/0000:00:02.0/class"), "0x030000\n");
        t.put(&format!("{pci}/0000:00:02.0/vendor"), "0x8086\n");
        t.put(&format!("{pci}/0000:00:02.0/device"), "0x7d67\n");
        t.put(&format!("{pci}/0000:00:02.0/boot_vga"), "1\n");
        t.put(&format!("{pci}/0000:01:00.0/class"), "0x030200\n");
        t.put(&format!("{pci}/0000:01:00.0/vendor"), "0x10de\n");
        t.put(&format!("{pci}/0000:01:00.0/device"), "0x2c19\n");
        t.put(&format!("{pci}/0000:01:00.1/class"), "0x040300\n"); // HDMI audio: not a GPU
        t.put(&format!("{pci}/0000:01:00.1/vendor"), "0x10de\n");
        t.put("sys/block/nvme0n1/size", "2000409264\n");
        t
    }

    #[test]
    fn a_whole_laptop_is_read_from_its_files() {
        let t = laptop();
        let info = read(t.root());
        assert_eq!(info.machine.product, "Predator PH16-71");
        assert_eq!(info.machine.tier.as_str(), "supported");
        assert_eq!(info.cpu.max_mhz, Some(5400));
        assert_eq!(info.cpu.cores, Some(3));
        assert_eq!(info.memory_total_bytes, Some(32_812_116 * 1024));
        assert_eq!(info.kernel.as_deref(), Some("6.12.1-1-cachyos"));
        assert_eq!(info.uptime_s, Some(12345.67));
        assert_eq!(info.os.pretty_name, "CachyOS Linux");
        assert_eq!(info.drives.len(), 1);
        assert_eq!(info.serial, None, "unreadable or absent");
        let gpus: Vec<(&str, &str, bool)> =
            info.gpus.iter().map(|g| (g.vendor.as_str(), g.address.as_str(), g.boot_vga)).collect();
        assert_eq!(gpus, [("Intel", "0000:00:02.0", true), ("NVIDIA", "0000:01:00.0", false)]);
        assert_eq!(info.gpus[1].device_id.as_deref(), Some("0x2c19"));
        assert_eq!(info.gpus[1].name, None);
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["machine"]["product"], "Predator PH16-71");
        assert_eq!(json["cpu"]["threads"], 4);
    }

    #[test]
    fn the_serial_is_shown_only_when_it_is_real() {
        let t = laptop();
        let path = "sys/class/dmi/id/product_serial";
        t.put(path, "NXABCEK001234567\n");
        assert_eq!(read(t.root()).serial.as_deref(), Some("NXABCEK001234567"));
        for placeholder in
            ["Default string", "To be filled by O.E.M.", "Not Applicable", "System Serial Number", "None", ""]
        {
            t.put(path, placeholder);
            assert_eq!(read(t.root()).serial, None, "{placeholder:?}");
        }
    }

    #[test]
    fn an_empty_tree_gives_neutral_answers_never_a_panic() {
        let t = Tree::new();
        let info = read(t.root());
        assert_eq!(info.machine.tier.as_str(), "unsupported");
        assert_eq!(info.os.pretty_name, "Linux");
        assert_eq!(
            (info.cpu.threads, info.memory_total_bytes, info.kernel, info.uptime_s),
            (0, None, None, None)
        );
        assert!(info.gpus.is_empty() && info.drives.is_empty());
        let t2 = Tree::new();
        t2.put("proc/uptime", "-5 1\n");
        assert_eq!(read(t2.root()).uptime_s, None);
        t2.put("proc/uptime", "NaN 1\n");
        assert_eq!(read(t2.root()).uptime_s, None);
        t2.put("usr/lib/os-release", "PRETTY_NAME=Fallback\n");
        assert_eq!(read(t2.root()).os.pretty_name, "Fallback");
    }
}
