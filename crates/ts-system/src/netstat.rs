//! netstat: network throughput per interface from the kernel's counters, with peaks and session
//! totals, and which interface is the one in use.
//!
//! `/proc/net/dev` holds cumulative byte counters; a rate is the difference between two samples
//! divided by the time between them, so [`NetSampler`] keeps the previous sample. The awkward
//! cases have fixed answers:
//!
//! * two samples at the same instant (or a clock that stepped back): the previous rates are
//!   held, the baseline is not moved, so the next real interval is measured whole;
//! * a long pause (suspend): the gap is not averaged into a rate, the baseline restarts;
//! * a counter that went down (the interface was removed and re-created, a driver reload): the
//!   new value is the bytes since the re-creation and is used as the delta;
//! * an interface that disappears (USB adapter pulled): it leaves the list and its baseline is
//!   dropped; it keeps its share of the session total; when it returns it starts a new baseline;
//! * garbled lines are skipped.
//!
//! The *kind* of an interface comes from sysfs, not from its name: `wireless/` or `phy80211`
//! means Wi-Fi, ARPHRD type 772 loopback, an interface with no `device` link is virtual (bridge,
//! veth, tunnel, VPN), the rest of the type-1 interfaces are Ethernet.
//!
//! Adapted from predator-sense by cleyton1986 (GPL-3.0), used with the author's permission: the
//! per-interface rate, peak and session-total idea is from his `ui/network_page.rs`; the
//! interface-choice rules were rewritten (default route first, then link state).
#![forbid(unsafe_code)]

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::usage::files::{below, dir_names, read_i64, read_line, read_text};

/// The most interfaces tracked. A machine with thousands of container interfaces cannot make
/// the sampler allocate without bound.
pub const MAX_INTERFACES: usize = 256;

/// A gap longer than this between two samples (suspend) restarts the baseline instead of
/// averaging a rate over the sleep.
const STALE_SECS: f64 = 30.0;

/// Samples closer together than this are one sample: the kernel's counters have not meaningfully moved.
const MIN_STEP_SECS: f64 = 0.05;

/// What an interface is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum InterfaceKind {
    /// 802.11.
    Wifi,
    /// Wired.
    Ethernet,
    /// `lo`.
    Loopback,
    /// Bridges, tunnels, VPNs, container links: no hardware behind them.
    Virtual,
    /// Anything else (Bluetooth networking, WWAN).
    Other,
}

impl InterfaceKind {
    /// Whether traffic on it is traffic to the outside world (hardware, not loopback or virtual).
    pub fn is_physical(self) -> bool {
        matches!(self, Self::Wifi | Self::Ethernet | Self::Other)
    }
}

/// The counters of one interface in `/proc/net/dev`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IfCounters {
    /// Interface name.
    pub name: String,
    /// Bytes received.
    pub rx_bytes: u64,
    /// Bytes sent.
    pub tx_bytes: u64,
}

/// Parses `/proc/net/dev`: two header lines, then `name: rx_bytes packets ... tx_bytes ...`
/// (16 numbers). The colon may touch the first number (`eth0:123`). A line that does not have
/// both counters is skipped; so are names that are empty or too long for the kernel (15).
pub fn parse_net_dev(text: &str) -> Vec<IfCounters> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((name, rest)) = line.split_once(':') else { continue };
        let name = name.trim();
        if name.is_empty() || name.len() > 15 || name.contains(char::is_whitespace) || name.contains('/') {
            continue;
        }
        let fields: Vec<&str> = rest.split_whitespace().collect();
        // rx: bytes packets errs drop fifo frame compressed multicast; tx: bytes ... (index 8)
        let (Some(rx), Some(tx)) = (fields.first(), fields.get(8)) else { continue };
        let (Ok(rx_bytes), Ok(tx_bytes)) = (rx.parse::<u64>(), tx.parse::<u64>()) else { continue };
        if out.len() >= MAX_INTERFACES {
            break;
        }
        out.push(IfCounters { name: name.to_owned(), rx_bytes, tx_bytes });
    }
    out
}

/// What sysfs says about an interface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InterfaceInfo {
    /// What it is.
    pub kind: InterfaceKind,
    /// The kernel's operational state: `up`, `down`, `dormant`, `unknown` (loopback, many virtual ones).
    pub state: String,
    /// The hardware address, when it has one.
    pub mac: Option<String>,
    /// Link speed in Mbit/s (wired, when the driver reports it).
    pub speed_mbps: Option<u32>,
}

/// Reads an interface's facts from `/sys/class/net/<name>`. Missing files give the neutral
/// answer (`other`, state `unknown`), never an error: an interface can vanish mid-read.
pub fn interface_info(root: &Path, name: &str) -> InterfaceInfo {
    let dir = below(root, "/sys/class/net").join(name);
    let arphrd = read_i64(&dir.join("type"));
    let kind = if dir.join("wireless").is_dir() || dir.join("phy80211").exists() {
        InterfaceKind::Wifi
    } else if arphrd == Some(772) || name == "lo" {
        InterfaceKind::Loopback
    } else if !dir.join("device").exists() {
        // Bridges, bonds, veth, tun, wireguard: no `device` link to hardware.
        if dir.exists() { InterfaceKind::Virtual } else { InterfaceKind::Other }
    } else if arphrd == Some(1) {
        InterfaceKind::Ethernet
    } else {
        InterfaceKind::Other
    };
    let state =
        read_line(&dir.join("operstate")).filter(|s| !s.is_empty()).unwrap_or_else(|| "unknown".to_owned());
    let mac = read_line(&dir.join("address")).filter(|a| !a.is_empty() && a != "00:00:00:00:00:00");
    // `speed` is -1 for a link that is down and fails to read on Wi-Fi.
    let speed_mbps = read_i64(&dir.join("speed")).and_then(|s| u32::try_from(s).ok()).filter(|s| *s > 0);
    InterfaceInfo { kind, state, mac, speed_mbps }
}

/// The interface of the default IPv4 route from `/proc/net/route` (lowest metric first).
/// The file is `Iface Destination Gateway Flags RefCnt Use Metric Mask ...` in hex; a default
/// route has destination `00000000` and the `RTF_UP` (1) and `RTF_GATEWAY` (2) flags.
pub fn default_route_interface(text: &str) -> Option<String> {
    let mut best: Option<(u64, &str)> = None;
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        let [iface, dest, _, flags, _, _, metric, ..] = f.as_slice() else { continue };
        let flags = u32::from_str_radix(flags, 16).unwrap_or(0);
        if *dest != "00000000" || flags & 1 == 0 {
            continue;
        }
        let metric: u64 = metric.parse().unwrap_or(u64::MAX);
        if best.is_none_or(|(m, _)| metric < m) {
            best = Some((metric, iface));
        }
    }
    best.map(|(_, i)| i.to_owned())
}

/// The IPv4 addresses `ip -o -4 addr show` printed, per interface: lines like
/// `2: wlan0    inet 192.168.1.5/24 brd 192.168.1.255 scope global dynamic noprefixroute wlan0`.
pub fn parse_ip_addr(text: &str) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        let (Some(name), Some(at)) = (f.get(1), f.iter().position(|w| *w == "inet")) else { continue };
        let Some(addr) = f.get(at + 1).and_then(|a| a.split('/').next()) else { continue };
        if addr.parse::<std::net::Ipv4Addr>().is_err() {
            continue;
        }
        out.entry(name.trim_end_matches(':').to_owned()).or_default().push(addr.to_owned());
    }
    out
}

/// One interface in a sample.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InterfaceRate {
    /// The interface name.
    pub name: String,
    /// What it is.
    pub kind: InterfaceKind,
    /// Operational state.
    pub state: String,
    /// Hardware address.
    pub mac: Option<String>,
    /// Link speed in Mbit/s.
    pub speed_mbps: Option<u32>,
    /// Receive rate in bytes per second; `null` before there is a baseline.
    pub rx_bps: Option<f64>,
    /// Send rate in bytes per second.
    pub tx_bps: Option<f64>,
    /// Highest receive rate seen since the sampler started or was reset.
    pub peak_rx_bps: f64,
    /// Highest send rate seen.
    pub peak_tx_bps: f64,
    /// Bytes received since the sampler started or was reset.
    pub session_rx_bytes: u64,
    /// Bytes sent since then.
    pub session_tx_bytes: u64,
    /// The kernel's own cumulative counters.
    pub total_rx_bytes: u64,
    /// The kernel's own cumulative counters.
    pub total_tx_bytes: u64,
}

/// Everything of one sample.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct NetSample {
    /// The interface carrying the traffic: the default route's, else an up physical one.
    pub primary: Option<String>,
    /// Every interface present now (loopback and virtual ones included, so a page can show them).
    pub interfaces: Vec<InterfaceRate>,
    /// Receive rate summed over the physical interfaces.
    pub rx_bps: Option<f64>,
    /// Send rate summed over the physical interfaces.
    pub tx_bps: Option<f64>,
    /// Bytes received over the physical interfaces since the session began, including
    /// interfaces that have since disappeared.
    pub session_rx_bytes: u64,
    /// Bytes sent, likewise.
    pub session_tx_bytes: u64,
    /// Highest summed receive rate.
    pub peak_rx_bps: f64,
    /// Highest summed send rate.
    pub peak_tx_bps: f64,
}

#[derive(Debug, Clone, Copy, Default)]
struct Session {
    rx: u64,
    tx: u64,
    peak_rx: f64,
    peak_tx: f64,
    last_rx_bps: Option<f64>,
    last_tx_bps: Option<f64>,
    physical: bool,
}

/// The counters of the previous sample by interface name: (received, sent).
type Baseline = HashMap<String, (u64, u64)>;

/// Turns successive `/proc/net/dev` readings into rates, peaks and totals.
#[derive(Debug)]
pub struct NetSampler {
    root: PathBuf,
    prev: Option<(f64, Baseline)>,
    sessions: BTreeMap<String, Session>,
    peak_rx: f64,
    peak_tx: f64,
}

impl NetSampler {
    /// A sampler reading below `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), prev: None, sessions: BTreeMap::new(), peak_rx: 0.0, peak_tx: 0.0 }
    }

    /// Whether a baseline exists, so the next sample can have rates.
    pub fn has_baseline(&self) -> bool {
        self.prev.is_some()
    }

    /// Forgets peaks and session totals (the page's reset button); the rate baseline stays.
    pub fn reset(&mut self) {
        self.sessions.clear();
        self.peak_rx = 0.0;
        self.peak_tx = 0.0;
    }

    /// Reads the counters at time `now` (seconds, the caller's clock) and answers the rates
    /// since the previous call.
    pub fn sample(&mut self, now: f64) -> NetSample {
        let Some(text) = read_text(&below(&self.root, "/proc/net/dev")) else {
            return NetSample::default();
        };
        let counters = parse_net_dev(&text);
        let route =
            read_text(&below(&self.root, "/proc/net/route")).and_then(|t| default_route_interface(&t));

        // A zero or backwards step holds the previous rates and keeps the baseline; a long gap
        // restarts it.
        let elapsed = self.prev.as_ref().map(|(then, _)| now - then);
        let hold = elapsed.is_some_and(|e| e < MIN_STEP_SECS);
        let restart = elapsed.is_none_or(|e| e > STALE_SECS);
        let dt = elapsed.filter(|_| !hold && !restart);

        let previous = if hold || restart { None } else { self.prev.as_ref().map(|(_, m)| m) };
        let mut interfaces = Vec::with_capacity(counters.len());
        let mut present = HashMap::with_capacity(counters.len());
        for c in &counters {
            let info = interface_info(&self.root, &c.name);
            let session = self.sessions.entry(c.name.clone()).or_default();
            session.physical = info.kind.is_physical();
            let before = previous.and_then(|m| m.get(&c.name)).copied();
            if let (Some(dt), Some((rx0, tx0))) = (dt, before) {
                // A counter that went down was reset: what it shows is what came after the reset.
                let rx = if c.rx_bytes >= rx0 { c.rx_bytes - rx0 } else { c.rx_bytes };
                let tx = if c.tx_bytes >= tx0 { c.tx_bytes - tx0 } else { c.tx_bytes };
                session.rx = session.rx.saturating_add(rx);
                session.tx = session.tx.saturating_add(tx);
                let (rx_bps, tx_bps) = (rx as f64 / dt, tx as f64 / dt);
                session.peak_rx = session.peak_rx.max(rx_bps);
                session.peak_tx = session.peak_tx.max(tx_bps);
                session.last_rx_bps = Some(rx_bps);
                session.last_tx_bps = Some(tx_bps);
            } else if !hold {
                // New interface or restarted baseline: no rate until the next sample.
                session.last_rx_bps = None;
                session.last_tx_bps = None;
            }
            present.insert(c.name.clone(), (c.rx_bytes, c.tx_bytes));
            interfaces.push(InterfaceRate {
                name: c.name.clone(),
                kind: info.kind,
                state: info.state,
                mac: info.mac,
                speed_mbps: info.speed_mbps,
                rx_bps: session.last_rx_bps,
                tx_bps: session.last_tx_bps,
                peak_rx_bps: session.peak_rx,
                peak_tx_bps: session.peak_tx,
                session_rx_bytes: session.rx,
                session_tx_bytes: session.tx,
                total_rx_bytes: c.rx_bytes,
                total_tx_bytes: c.tx_bytes,
            });
        }
        if !hold {
            self.prev = Some((now, present));
        }

        let physical: Vec<&InterfaceRate> = interfaces.iter().filter(|i| i.kind.is_physical()).collect();
        let sum = |f: fn(&InterfaceRate) -> Option<f64>| {
            let values: Vec<f64> = physical.iter().filter_map(|i| f(i)).collect();
            (!values.is_empty()).then(|| values.iter().sum::<f64>())
        };
        let (rx_bps, tx_bps) = (sum(|i| i.rx_bps), sum(|i| i.tx_bps));
        self.peak_rx = self.peak_rx.max(rx_bps.unwrap_or(0.0));
        self.peak_tx = self.peak_tx.max(tx_bps.unwrap_or(0.0));
        let (session_rx_bytes, session_tx_bytes) = self
            .sessions
            .values()
            .filter(|s| s.physical)
            .fold((0u64, 0u64), |(rx, tx), s| (rx.saturating_add(s.rx), tx.saturating_add(s.tx)));
        let primary = choose_primary(&interfaces, route.as_deref());
        NetSample {
            primary,
            rx_bps,
            tx_bps,
            session_rx_bytes,
            session_tx_bytes,
            peak_rx_bps: self.peak_rx,
            peak_tx_bps: self.peak_tx,
            interfaces,
        }
    }
}

/// The interface to feature: the default route's when it is present, else the first physical
/// interface that is up (Wi-Fi before Ethernet, as a laptop is mostly on Wi-Fi), else none.
fn choose_primary(interfaces: &[InterfaceRate], route: Option<&str>) -> Option<String> {
    if let Some(r) = route.filter(|r| interfaces.iter().any(|i| i.name == *r)) {
        return Some(r.to_owned());
    }
    let up = |i: &&InterfaceRate| i.state == "up" && i.kind.is_physical();
    interfaces
        .iter()
        .filter(up)
        .min_by_key(|i| (i.kind != InterfaceKind::Wifi, i.name.clone()))
        .map(|i| i.name.clone())
}

/// The names of the interfaces in `/sys/class/net`, for callers that want the list without counters.
pub fn list_interfaces(root: &Path) -> Vec<String> {
    dir_names(&below(root, "/sys/class/net"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::usage::files::testutil::Tree;

    fn dev(rows: &[(&str, u64, u64)]) -> String {
        let mut s = String::from(
            "Inter-|   Receive                                                |  Transmit\n \
             face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n",
        );
        for (name, rx, tx) in rows {
            s += &format!("{name:>6}: {rx} 10 0 0 0 0 0 0 {tx} 10 0 0 0 0 0 0\n");
        }
        s
    }

    /// A laptop: Wi-Fi up, Ethernet down, loopback, a Docker bridge.
    fn laptop() -> Tree {
        let t = Tree::new();
        let n = "sys/class/net";
        t.put(&format!("{n}/lo/type"), "772\n");
        t.put(&format!("{n}/lo/operstate"), "unknown\n");
        t.put(&format!("{n}/wlan0/type"), "1\n");
        t.put(&format!("{n}/wlan0/operstate"), "up\n");
        t.put(&format!("{n}/wlan0/address"), "aa:bb:cc:dd:ee:ff\n");
        t.put(&format!("{n}/wlan0/wireless/status"), "0\n");
        t.put(&format!("{n}/wlan0/device/vendor"), "0x8086\n");
        t.put(&format!("{n}/enp3s0/type"), "1\n");
        t.put(&format!("{n}/enp3s0/operstate"), "down\n");
        t.put(&format!("{n}/enp3s0/speed"), "-1\n");
        t.put(&format!("{n}/enp3s0/device/vendor"), "0x10ec\n");
        t.put(&format!("{n}/docker0/type"), "1\n");
        t.put(&format!("{n}/docker0/operstate"), "down\n");
        t.put("proc/net/route", "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\nwlan0\t00000000\t0101A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0\nwlan0\t0001A8C0\t00000000\t0001\t0\t0\t600\t00FFFFFF\t0\t0\t0\n");
        t
    }

    #[test]
    fn parses_proc_net_dev_with_touching_colons_and_skips_garbage() {
        let text = format!(
            "{}eth9:77 1 0 0 0 0 0 0 88 1 0 0 0 0 0 0\nbroken: 1 2 3\nno colon here\n: 1 1 1 1 1 1 1 1 1 1\nbad: x 1 1 1 1 1 1 1 y 1\nthisnameiswaytoolongforkernel: 1 1 1 1 1 1 1 1 1\n",
            dev(&[("lo", 5, 5), ("wlan0", 1_000_000, 250_000)])
        );
        let c = parse_net_dev(&text);
        assert_eq!(c.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["lo", "wlan0", "eth9"]);
        assert_eq!((c[1].rx_bytes, c[1].tx_bytes), (1_000_000, 250_000));
        assert_eq!((c[2].rx_bytes, c[2].tx_bytes), (77, 88));
        assert!(parse_net_dev("").is_empty());
        assert!(parse_net_dev("\0\0\0:\0").is_empty());
        let many: String = (0..1000).map(|i| format!("if{i}: 1 1 1 1 1 1 1 1 1\n")).collect();
        assert_eq!(parse_net_dev(&many).len(), MAX_INTERFACES);
        let _ = parse_net_dev(&format!("x: {} 1 1 1 1 1 1 1 {}\n", u64::MAX, u64::MAX));
        assert_eq!(parse_net_dev("x: 99999999999999999999 1 1 1 1 1 1 1 1").len(), 0);
    }

    #[test]
    fn kinds_come_from_sysfs_not_from_names() {
        let t = laptop();
        let info = |n: &str| interface_info(t.root(), n);
        assert_eq!(info("wlan0").kind, InterfaceKind::Wifi);
        assert_eq!(info("wlan0").mac.as_deref(), Some("aa:bb:cc:dd:ee:ff"));
        assert_eq!(info("enp3s0").kind, InterfaceKind::Ethernet);
        assert_eq!(info("enp3s0").speed_mbps, None, "-1 while the link is down");
        assert_eq!(info("lo").kind, InterfaceKind::Loopback);
        assert_eq!(info("docker0").kind, InterfaceKind::Virtual);
        assert_eq!(info("gone0").kind, InterfaceKind::Other, "not in sysfs at all");
        assert_eq!(info("gone0").state, "unknown");
        // a Wi-Fi card with an odd name is still Wi-Fi, an ethernet-named virtual one is virtual
        t.put("sys/class/net/xyz1/phy80211/index", "0\n");
        t.put("sys/class/net/eth5/type", "1\n");
        assert_eq!(info("xyz1").kind, InterfaceKind::Wifi);
        assert_eq!(info("eth5").kind, InterfaceKind::Virtual);
        t.put("sys/class/net/enp3s0/speed", "1000\n");
        assert_eq!(info("enp3s0").speed_mbps, Some(1000));
    }

    #[test]
    fn default_route_and_addresses_are_parsed() {
        let t = laptop();
        let route = std::fs::read_to_string(t.root().join("proc/net/route")).unwrap();
        assert_eq!(default_route_interface(&route).as_deref(), Some("wlan0"));
        let two = "Iface Destination Gateway Flags RefCnt Use Metric Mask\neth0 00000000 1 0003 0 0 100 0\nwlan0 00000000 1 0003 0 0 50 0\nwlan1 00000000 1 0002 0 0 1 0\n";
        assert_eq!(
            default_route_interface(two).as_deref(),
            Some("wlan0"),
            "lowest metric among routes that are up"
        );
        assert_eq!(default_route_interface("garbage\nmore garbage"), None);
        assert_eq!(default_route_interface(""), None);
        let ip = "1: lo    inet 127.0.0.1/8 scope host lo\n2: wlan0    inet 192.168.1.5/24 brd 192.168.1.255 scope global dynamic noprefixroute wlan0\n2: wlan0    inet 10.0.0.9/8 scope global wlan0\n3: bad    inet notanip/24 x\nshort\n";
        let a = parse_ip_addr(ip);
        assert_eq!(a["wlan0"], ["192.168.1.5", "10.0.0.9"]);
        assert_eq!(a["lo"], ["127.0.0.1"]);
        assert!(!a.contains_key("bad"));
    }

    #[test]
    fn rates_peaks_and_session_totals_follow_the_counters() {
        let t = laptop();
        let mut s = NetSampler::new(t.root());
        assert_eq!(s.sample(0.0), NetSample::default(), "no /proc/net/dev yet");
        t.put("proc/net/dev", &dev(&[("lo", 100, 100), ("wlan0", 1_000, 500), ("docker0", 0, 0)]));
        let first = s.sample(10.0);
        assert!(s.has_baseline());
        assert_eq!(first.rx_bps, None, "no baseline: no rate");
        assert_eq!(first.primary.as_deref(), Some("wlan0"));
        t.put("proc/net/dev", &dev(&[("lo", 900, 900), ("wlan0", 3_000, 1_500), ("docker0", 50, 50)]));
        let second = s.sample(12.0);
        let wlan = second.interfaces.iter().find(|i| i.name == "wlan0").unwrap();
        assert_eq!((wlan.rx_bps, wlan.tx_bps), (Some(1000.0), Some(500.0)));
        assert_eq!((wlan.session_rx_bytes, wlan.session_tx_bytes), (2_000, 1_000));
        assert_eq!((wlan.total_rx_bytes, wlan.kind), (3_000, InterfaceKind::Wifi));
        assert_eq!(second.rx_bps, Some(1000.0), "loopback and the bridge are not the internet");
        assert_eq!((second.session_rx_bytes, second.session_tx_bytes), (2_000, 1_000));
        // a slower interval does not lower the peak
        t.put("proc/net/dev", &dev(&[("lo", 900, 900), ("wlan0", 3_100, 1_550), ("docker0", 50, 50)]));
        let third = s.sample(14.0);
        assert_eq!(third.rx_bps, Some(50.0));
        assert_eq!((third.peak_rx_bps, third.peak_tx_bps), (1000.0, 500.0));
        assert_eq!(third.session_rx_bytes, 2_100);
        s.reset();
        t.put("proc/net/dev", &dev(&[("lo", 900, 900), ("wlan0", 3_200, 1_560), ("docker0", 50, 50)]));
        let after = s.sample(15.0);
        assert_eq!(
            (after.peak_rx_bps, after.session_rx_bytes),
            (100.0, 100),
            "reset starts the totals again"
        );
    }

    #[test]
    fn a_zero_delta_holds_the_rates_and_keeps_the_baseline() {
        let t = laptop();
        let mut s = NetSampler::new(t.root());
        t.put("proc/net/dev", &dev(&[("wlan0", 0, 0)]));
        s.sample(10.0);
        t.put("proc/net/dev", &dev(&[("wlan0", 2_000, 0)]));
        assert_eq!(s.sample(11.0).rx_bps, Some(2000.0));
        // the same instant: nothing divides by zero, the rate is held
        let same = s.sample(11.0);
        assert_eq!(same.rx_bps, Some(2000.0));
        // 10 ms later, same counters: still held
        assert_eq!(s.sample(11.01).rx_bps, Some(2000.0));
        // the next real interval is measured from the last real baseline (t = 11)
        t.put("proc/net/dev", &dev(&[("wlan0", 6_000, 0)]));
        assert_eq!(s.sample(13.0).rx_bps, Some(2000.0), "4000 bytes in 2 s");
        // a clock that stepped back
        assert_eq!(s.sample(5.0).rx_bps, Some(2000.0));
        t.put("proc/net/dev", &dev(&[("wlan0", 7_000, 0)]));
        let after_step = s.sample(14.0);
        assert_eq!(after_step.rx_bps, Some(1000.0), "the baseline stayed at t = 13");
    }

    #[test]
    fn a_long_pause_restarts_the_baseline() {
        let t = laptop();
        let mut s = NetSampler::new(t.root());
        t.put("proc/net/dev", &dev(&[("wlan0", 0, 0)]));
        s.sample(10.0);
        t.put("proc/net/dev", &dev(&[("wlan0", 1_000_000, 0)]));
        let resumed = s.sample(10_000.0);
        assert_eq!(resumed.rx_bps, None, "no rate averaged over a suspend");
        assert_eq!(resumed.session_rx_bytes, 0);
        t.put("proc/net/dev", &dev(&[("wlan0", 1_001_000, 0)]));
        assert_eq!(s.sample(10_001.0).rx_bps, Some(1000.0));
    }

    #[test]
    fn counters_that_reset_use_the_new_value_as_the_delta() {
        let t = laptop();
        let mut s = NetSampler::new(t.root());
        t.put("proc/net/dev", &dev(&[("wlan0", 9_000_000, 9_000_000)]));
        s.sample(1.0);
        t.put("proc/net/dev", &dev(&[("wlan0", 400, 200)]));
        let r = s.sample(3.0);
        assert_eq!((r.rx_bps, r.tx_bps), (Some(200.0), Some(100.0)));
        assert!(r.rx_bps.unwrap() >= 0.0);
    }

    #[test]
    fn an_unplugged_interface_leaves_the_list_but_keeps_its_share_of_the_totals() {
        let t = laptop();
        let mut s = NetSampler::new(t.root());
        t.put("proc/net/dev", &dev(&[("wlan0", 0, 0), ("enp3s0", 0, 0)]));
        s.sample(1.0);
        t.put("proc/net/dev", &dev(&[("wlan0", 100, 0), ("enp3s0", 1000, 0)]));
        let both = s.sample(2.0);
        assert_eq!(both.session_rx_bytes, 1100);
        // the USB adapter is pulled
        t.put("proc/net/dev", &dev(&[("wlan0", 300, 0)]));
        let gone = s.sample(3.0);
        assert_eq!(gone.interfaces.len(), 1);
        assert_eq!(gone.rx_bps, Some(200.0));
        assert_eq!(gone.session_rx_bytes, 1300, "what came over it stays counted");
        // plugged in again with fresh counters: a new baseline first, then rates
        t.put("proc/net/dev", &dev(&[("wlan0", 300, 0), ("enp3s0", 50, 0)]));
        let back = s.sample(4.0);
        let eth = back.interfaces.iter().find(|i| i.name == "enp3s0").unwrap();
        assert_eq!(eth.rx_bps, None);
        t.put("proc/net/dev", &dev(&[("wlan0", 300, 0), ("enp3s0", 150, 0)]));
        let eth = s.sample(5.0).interfaces.into_iter().find(|i| i.name == "enp3s0").unwrap();
        assert_eq!(eth.rx_bps, Some(100.0));
    }

    #[test]
    fn the_primary_follows_the_default_route_then_link_state() {
        let t = laptop();
        let mut s = NetSampler::new(t.root());
        t.put("proc/net/dev", &dev(&[("lo", 1, 1), ("wlan0", 1, 1), ("enp3s0", 1, 1)]));
        assert_eq!(s.sample(1.0).primary.as_deref(), Some("wlan0"), "route");
        t.remove("proc/net/route");
        assert_eq!(s.sample(2.0).primary.as_deref(), Some("wlan0"), "up Wi-Fi, ethernet is down");
        t.put("sys/class/net/enp3s0/operstate", "up\n");
        assert_eq!(s.sample(3.0).primary.as_deref(), Some("wlan0"), "Wi-Fi before Ethernet");
        t.put("sys/class/net/wlan0/operstate", "down\n");
        assert_eq!(s.sample(4.0).primary.as_deref(), Some("enp3s0"));
        t.put("sys/class/net/enp3s0/operstate", "down\n");
        assert_eq!(s.sample(5.0).primary, None);
        assert_eq!(list_interfaces(t.root()), ["docker0", "enp3s0", "lo", "wlan0"]);
    }
}
