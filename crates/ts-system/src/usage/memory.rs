//! Memory and swap from `/proc/meminfo`.
//!
//! Sizes are reported in bytes (the file counts kibibytes) so no client has to know the unit.
//! "Used" is what `free` calls used on current systems: total minus *available*, where available
//! is the kernel's estimate of what can be given to a program without swapping. Cache and
//! buffers are therefore not counted as used, which is what a person reading a gauge expects.
#![forbid(unsafe_code)]

use std::path::Path;

use serde::Serialize;

use super::files::{below, read_text};

/// The raw counters of `/proc/meminfo` that the monitor uses, in kibibytes as the file has them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MemInfo {
    /// `MemTotal`.
    pub total_kb: u64,
    /// `MemAvailable`; absent on kernels older than 3.14 (then derived, see [`parse_meminfo`]).
    pub available_kb: u64,
    /// `MemFree`.
    pub free_kb: u64,
    /// `Buffers`.
    pub buffers_kb: u64,
    /// `Cached`.
    pub cached_kb: u64,
    /// `SwapTotal`.
    pub swap_total_kb: u64,
    /// `SwapFree`.
    pub swap_free_kb: u64,
}

/// Parses `/proc/meminfo`. A line that is not `Key: number [kB]` is skipped. `None` when there is
/// no usable `MemTotal`, because a total of zero would make every percentage meaningless.
/// Without `MemAvailable` it is estimated as free + buffers + cached, as old `free` did.
pub fn parse_meminfo(text: &str) -> Option<MemInfo> {
    let mut info = MemInfo::default();
    let mut available = None;
    for line in text.lines() {
        let Some((key, rest)) = line.split_once(':') else { continue };
        let Some(value) = rest.split_whitespace().next().and_then(|v| v.parse::<u64>().ok()) else {
            continue;
        };
        match key {
            "MemTotal" => info.total_kb = value,
            "MemAvailable" => available = Some(value),
            "MemFree" => info.free_kb = value,
            "Buffers" => info.buffers_kb = value,
            "Cached" => info.cached_kb = value,
            "SwapTotal" => info.swap_total_kb = value,
            "SwapFree" => info.swap_free_kb = value,
            _ => {}
        }
    }
    if info.total_kb == 0 {
        return None;
    }
    info.available_kb = available
        .unwrap_or_else(|| info.free_kb.saturating_add(info.buffers_kb).saturating_add(info.cached_kb));
    Some(info)
}

/// Reads `/proc/meminfo` below `root`.
pub fn read_meminfo(root: &Path) -> Option<MemInfo> {
    parse_meminfo(&read_text(&below(root, "/proc/meminfo"))?)
}

fn pct(part: u64, whole: u64) -> f64 {
    if whole == 0 { 0.0 } else { (part as f64 * 100.0 / whole as f64).clamp(0.0, 100.0) }
}

/// Memory and swap as the page shows them.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct MemoryUsage {
    /// Installed RAM the kernel can use.
    pub total_bytes: u64,
    /// Total minus available.
    pub used_bytes: u64,
    /// What a program can get without swapping.
    pub available_bytes: u64,
    /// Memory that holds nothing at all.
    pub free_bytes: u64,
    /// Page cache and buffers (reclaimable).
    pub cache_bytes: u64,
    /// `used_bytes` as a percentage of `total_bytes`.
    pub used_pct: f64,
    /// Swap space.
    pub swap_total_bytes: u64,
    /// Swap in use.
    pub swap_used_bytes: u64,
    /// Swap in use as a percentage; 0 without swap.
    pub swap_used_pct: f64,
}

impl MemoryUsage {
    /// The page's figures from the raw counters. Inconsistent counters (available above total,
    /// free swap above total swap) are clamped rather than reported as negative use.
    pub fn from_info(info: &MemInfo) -> Self {
        let kib = |kb: u64| kb.saturating_mul(1024);
        let used = info.total_kb.saturating_sub(info.available_kb);
        let swap_used = info.swap_total_kb.saturating_sub(info.swap_free_kb);
        Self {
            total_bytes: kib(info.total_kb),
            used_bytes: kib(used),
            available_bytes: kib(info.available_kb.min(info.total_kb)),
            free_bytes: kib(info.free_kb),
            cache_bytes: kib(info.buffers_kb.saturating_add(info.cached_kb)),
            used_pct: pct(used, info.total_kb),
            swap_total_bytes: kib(info.swap_total_kb),
            swap_used_bytes: kib(swap_used),
            swap_used_pct: pct(swap_used, info.swap_total_kb),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::super::files::testutil::Tree;
    use super::*;

    /// A real `/proc/meminfo` of a 32 GB laptop (cut to the interesting part).
    const MEMINFO: &str = "MemTotal:       32812116 kB\nMemFree:         9120456 kB\nMemAvailable:   24012344 kB\n\
        Buffers:            2048 kB\nCached:         14210004 kB\nSwapCached:            0 kB\nActive:  1 kB\n\
        SwapTotal:       8388604 kB\nSwapFree:        8000000 kB\nHugePages_Total:       0\n";

    #[test]
    fn a_real_meminfo_becomes_bytes_and_percentages() {
        let m = MemoryUsage::from_info(&parse_meminfo(MEMINFO).unwrap());
        assert_eq!(m.total_bytes, 32_812_116 * 1024);
        assert_eq!(m.available_bytes, 24_012_344 * 1024);
        assert_eq!(m.used_bytes, (32_812_116 - 24_012_344) * 1024);
        assert!((m.used_pct - 26.82).abs() < 0.01, "{}", m.used_pct);
        assert_eq!(m.cache_bytes, (2048 + 14_210_004) * 1024);
        assert_eq!(m.swap_used_bytes, (8_388_604 - 8_000_000) * 1024);
        assert!((m.swap_used_pct - 4.63).abs() < 0.01);
    }

    #[test]
    fn without_memavailable_it_is_estimated() {
        let m =
            parse_meminfo("MemTotal: 1000 kB\nMemFree: 100 kB\nBuffers: 50 kB\nCached: 150 kB\n").unwrap();
        assert_eq!(m.available_kb, 300);
    }

    #[test]
    fn no_swap_is_zero_percent_not_nan() {
        let m = MemoryUsage::from_info(&parse_meminfo("MemTotal: 1000 kB\nMemAvailable: 500 kB\n").unwrap());
        assert_eq!(m.swap_used_pct, 0.0);
        assert_eq!(m.used_pct, 50.0);
    }

    #[test]
    fn inconsistent_counters_are_clamped() {
        let info =
            parse_meminfo("MemTotal: 100 kB\nMemAvailable: 500 kB\nSwapTotal: 10 kB\nSwapFree: 99 kB\n")
                .unwrap();
        let m = MemoryUsage::from_info(&info);
        assert_eq!((m.used_bytes, m.used_pct), (0, 0.0));
        assert_eq!((m.available_bytes, m.swap_used_bytes, m.swap_used_pct), (100 * 1024, 0, 0.0));
    }

    #[test]
    fn hostile_meminfo_never_panics() {
        for text in [
            "",
            "MemTotal:",
            "MemTotal: x kB",
            "MemTotal: -5 kB",
            "MemTotal: 0 kB",
            "no colon here",
            "MemTotal: 18446744073709551615 kB\nMemAvailable: 1 kB",
            ":::::\n\0\n",
        ] {
            let _ = parse_meminfo(text).map(|i| MemoryUsage::from_info(&i));
        }
        assert_eq!(parse_meminfo("MemTotal: 0 kB"), None);
        let huge = MemoryUsage::from_info(&parse_meminfo("MemTotal: 18446744073709551615 kB").unwrap());
        assert_eq!(huge.total_bytes, u64::MAX, "saturates instead of overflowing");
    }

    #[test]
    fn reads_below_the_root() {
        let t = Tree::new();
        assert_eq!(read_meminfo(t.root()), None);
        t.put("proc/meminfo", MEMINFO);
        assert_eq!(read_meminfo(t.root()).unwrap().total_kb, 32_812_116);
    }
}
