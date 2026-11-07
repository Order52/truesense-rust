//! disks: mounted filesystems with their usage, the block devices behind them, and the NVMe
//! drives' temperatures.
//!
//! The list of mounts comes from `/proc/self/mounts` below an injected root; the sizes come
//! through the [`FsStats`] trait. The real implementation, [`DfStats`], asks `df` through the
//! injected `CommandRunner` instead of calling `statvfs(2)`: that keeps this crate free of
//! `unsafe` (the architecture lists the places allowed to have it, and `statvfs` is not one), and
//! it gives a free timeout, which `statvfs` does not have: a hung network mount blocks a
//! `statvfs` call forever but only costs `df` its timeout. One `df` run answers for every mount.
//!
//! Filesystems that are not storage (tmpfs, proc, cgroups, snap loop mounts) are left out, and a
//! device mounted more than once (a Btrfs subvolume at `/` and `/home`, a bind mount) is listed
//! once at its shortest mount point, as `df` lists it.
//!
//! Adapted from predator-sense by cleyton1986 (GPL-3.0), used with the author's permission: the
//! skip list, the octal-escape decoding, the one-line-per-device rule and the `df`-compatible
//! rounding of the percentage are from his `hardware/storage.rs`; the `statvfs` call was replaced.
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use ts_platform::hw::CommandRunner;

use crate::usage::files::{below, dir_names, indexed, read_i64, read_line, read_text, read_u64};

/// How long `df` may run before it is given up on (a stuck network mount).
pub const DF_TIMEOUT: Duration = Duration::from_secs(3);

/// The most mounts looked at.
const MAX_MOUNTS: usize = 512;

/// One line of `/proc/self/mounts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    /// The device or source (`/dev/nvme0n1p2`, `server:/share`).
    pub device: String,
    /// Where it is mounted.
    pub mount: String,
    /// The filesystem type.
    pub fstype: String,
}

/// Decodes the kernel's octal escapes in a mount field (`\040` is a space, `\134` a backslash).
fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let octal = (bytes[i] == b'\\').then(|| bytes.get(i + 1..i + 4)).flatten();
        let code = octal.filter(|d| d.iter().all(|b| (b'0'..=b'7').contains(b))).and_then(|d| {
            let n = d.iter().fold(0u32, |n, b| n * 8 + u32::from(b - b'0'));
            u8::try_from(n).ok()
        });
        if let Some(code) = code {
            out.push(code);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parses `/proc/self/mounts`: `device mountpoint fstype options dump pass`, fields separated by
/// single spaces with spaces inside a field escaped. Short lines are skipped.
pub fn parse_mounts(text: &str) -> Vec<Mount> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some(Mount {
                device: unescape(fields.next()?),
                mount: unescape(fields.next()?),
                fstype: fields.next()?.to_owned(),
            })
        })
        .take(MAX_MOUNTS)
        .collect()
}

/// Whether a mount is not storage a person cares about: virtual, in memory, or the system's own.
pub fn is_skipped(fstype: &str, mount: &str) -> bool {
    matches!(
        fstype,
        "tmpfs"
            | "devtmpfs"
            | "squashfs"
            | "overlay"
            | "efivarfs"
            | "autofs"
            | "proc"
            | "sysfs"
            | "devpts"
            | "fusectl"
            | "fuse.portal"
            | "fuse.gvfsd-fuse"
            | "mqueue"
            | "debugfs"
            | "tracefs"
            | "securityfs"
            | "pstore"
            | "bpf"
            | "configfs"
            | "hugetlbfs"
            | "cgroup"
            | "cgroup2"
            | "binfmt_misc"
            | "rpc_pipefs"
            | "nsfs"
            | "ramfs"
            | "none"
    ) || {
        let under =
            |dir: &str| mount == dir || mount.strip_prefix(dir).is_some_and(|rest| rest.starts_with('/'));
        // Removable media are mounted below /run/media by udisks: those are storage the person cares about.
        ["/snap", "/proc", "/sys", "/dev"].iter().any(|d| under(d))
            || (under("/run") && !mount.starts_with("/run/media/"))
    }
}

/// The size of one filesystem, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FsSize {
    /// Capacity.
    pub total: u64,
    /// In use.
    pub used: u64,
    /// Available to an ordinary user (less than total minus used on ext4, which reserves space for root).
    pub avail: u64,
}

/// Where filesystem sizes come from.
pub trait FsStats: Send + Sync {
    /// The size of each mount point asked for; a mount that cannot be read is absent from the answer.
    fn sizes(&self, mounts: &[&str]) -> HashMap<String, FsSize>;
}

/// Sizes from one `df` run, so no `unsafe` and a timeout.
pub struct DfStats {
    runner: Arc<dyn CommandRunner>,
}

impl DfStats {
    /// Sizes through `df`, started by `runner`.
    pub fn new(runner: Arc<dyn CommandRunner>) -> Self {
        Self { runner }
    }
}

/// Parses `df -B1 --output=size,used,avail,target`: a header and rows of three numbers and a
/// mount point, which may contain spaces. Rows with `-` (a filesystem that reports no size)
/// are skipped.
pub fn parse_df(text: &str) -> HashMap<String, FsSize> {
    let mut out = HashMap::new();
    for line in text.lines().skip(1) {
        let mut parts = line.trim_start().splitn(4, char::is_whitespace);
        let (Some(total), Some(used), Some(avail)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let Some(target) = parts.next().map(str::trim_start).filter(|t| !t.is_empty()) else { continue };
        let (Ok(total), Ok(used), Ok(avail)) = (total.parse(), used.parse(), avail.parse()) else { continue };
        out.insert(target.to_owned(), FsSize { total, used, avail });
    }
    out
}

impl FsStats for DfStats {
    fn sizes(&self, mounts: &[&str]) -> HashMap<String, FsSize> {
        if mounts.is_empty() {
            return HashMap::new();
        }
        let mut args = vec!["-B1", "--output=size,used,avail,target", "--"];
        args.extend_from_slice(mounts);
        let output = self.runner.output("df", &args, DF_TIMEOUT).unwrap_or_default();
        parse_df(&String::from_utf8_lossy(&output))
    }
}

/// One mounted filesystem for the page.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiskUsage {
    /// The device or source.
    pub device: String,
    /// The filesystem type.
    pub fstype: String,
    /// Where it is mounted.
    pub mount: String,
    /// Capacity in bytes.
    pub total_bytes: u64,
    /// Bytes in use.
    pub used_bytes: u64,
    /// Bytes an ordinary user can still write.
    pub avail_bytes: u64,
    /// Use in percent, rounded up like `df` rounds it (used over used plus available).
    pub used_pct: f64,
}

/// `df`'s percentage: used / (used + avail), rounded up.
fn df_percent(size: FsSize) -> f64 {
    let base = size.used.saturating_add(size.avail);
    if base == 0 { 0.0 } else { (size.used as f64 * 100.0 / base as f64).ceil().clamp(0.0, 100.0) }
}

/// The ordering of the page: the system first, then boot, home, then the rest.
fn rank(mount: &str) -> u8 {
    match mount {
        "/" => 0,
        m if m == "/boot" || m.starts_with("/boot/") => 1,
        m if m == "/home" || m.starts_with("/home/") => 2,
        _ => 3,
    }
}

/// The filesystems worth showing, with their sizes. A device mounted twice is listed once at its
/// shortest mount point; the order is stable (system, boot, home, others by mount point).
pub fn list_disks(root: &Path, stats: &dyn FsStats) -> Vec<DiskUsage> {
    let Some(text) = read_text(&below(root, "/proc/self/mounts")) else { return Vec::new() };
    let mounts: Vec<Mount> =
        parse_mounts(&text).into_iter().filter(|m| !is_skipped(&m.fstype, &m.mount)).collect();
    let paths: Vec<&str> = mounts.iter().map(|m| m.mount.as_str()).collect();
    let sizes = stats.sizes(&paths);
    let mut by_device: Vec<DiskUsage> = Vec::new();
    for m in mounts {
        let Some(size) = sizes.get(&m.mount).filter(|s| s.total > 0) else { continue };
        let disk = DiskUsage {
            device: m.device.clone(),
            fstype: m.fstype,
            mount: m.mount,
            total_bytes: size.total,
            used_bytes: size.used,
            avail_bytes: size.avail,
            used_pct: df_percent(*size),
        };
        // Network and FUSE sources are not unique per mount, so only real devices are merged.
        let mergeable = m.device.starts_with("/dev/");
        match by_device.iter_mut().find(|d| mergeable && d.device == m.device) {
            Some(kept) if disk.mount.len() < kept.mount.len() => *kept = disk,
            Some(_) => {}
            None => by_device.push(disk),
        }
    }
    by_device.sort_by(|a, b| rank(&a.mount).cmp(&rank(&b.mount)).then_with(|| a.mount.cmp(&b.mount)));
    by_device
}

/// What kind of drive a block device is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DriveKind {
    /// NVMe SSD.
    #[serde(rename = "nvme")]
    Nvme,
    /// SATA or USB solid state.
    #[serde(rename = "ssd")]
    Ssd,
    /// Spinning disk.
    #[serde(rename = "hdd")]
    Hdd,
    /// SD card or eMMC.
    #[serde(rename = "mmc")]
    Mmc,
}

/// A physical drive.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BlockDevice {
    /// Kernel name (`nvme0n1`, `sda`).
    pub name: String,
    /// Model string, when the drive gives one.
    pub model: Option<String>,
    /// Capacity in bytes.
    pub size_bytes: u64,
    /// What kind of drive.
    pub kind: DriveKind,
}

/// The drives in `/sys/block` (`sd*`, `nvme*`, `mmcblk*`), skipping anything smaller than 1 GiB
/// (empty card readers report 0). The size file counts 512-byte sectors whatever the drive's
/// own sector size.
pub fn block_devices(root: &Path) -> Vec<BlockDevice> {
    let base = below(root, "/sys/block");
    let mut out = Vec::new();
    for name in dir_names(&base) {
        let kind = if name.starts_with("nvme") {
            DriveKind::Nvme
        } else if name.starts_with("mmcblk") {
            DriveKind::Mmc
        } else if name.starts_with("sd") {
            let dir = base.join(&name);
            if read_u64(&dir.join("queue/rotational")) == Some(0) { DriveKind::Ssd } else { DriveKind::Hdd }
        } else {
            continue;
        };
        let dir = base.join(&name);
        let size_bytes = read_u64(&dir.join("size")).unwrap_or(0).saturating_mul(512);
        if size_bytes < 1 << 30 {
            continue;
        }
        let model = read_line(&dir.join("device/model")).filter(|m| !m.is_empty());
        out.push(BlockDevice { name, model, size_bytes, kind });
    }
    out
}

/// The temperature of one NVMe drive.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NvmeTemp {
    /// The controller (`nvme0`) when the hwmon device link says so.
    pub controller: Option<String>,
    /// The drive's model, when its controller says so.
    pub model: Option<String>,
    /// The composite temperature in degrees C: what the drive reports as its overall temperature.
    pub celsius: f64,
    /// The drive's warning threshold, when known.
    pub max_c: Option<f64>,
    /// The drive's critical threshold, when known.
    pub crit_c: Option<f64>,
}

/// The NVMe drives' temperatures from hwmon, in hwmon order. Each read is an admin command to
/// the drive (about 9 ms), so call it no faster than every couple of seconds.
pub fn nvme_temps(root: &Path) -> Vec<NvmeTemp> {
    let base = below(root, "/sys/class/hwmon");
    let mut out = Vec::new();
    for entry in dir_names(&base) {
        if indexed(&entry, "hwmon").is_none() {
            continue;
        }
        let dir = base.join(&entry);
        if read_line(&dir.join("name")).as_deref() != Some("nvme") {
            continue;
        }
        let milli = |file: &str| {
            read_i64(&dir.join(file)).map(|m| m as f64 / 1000.0).filter(|c| (-50.0..=160.0).contains(c))
        };
        // `temp1` is the composite sensor; later ones are individual dies.
        let Some(celsius) = milli("temp1_input") else { continue };
        let controller = std::fs::read_link(dir.join("device"))
            .ok()
            .and_then(|l| l.file_name().and_then(|n| n.to_str()).map(str::to_owned))
            .filter(|n| indexed(n, "nvme").is_some());
        let model = read_line(&dir.join("device/model")).filter(|m| !m.is_empty());
        out.push(NvmeTemp {
            controller,
            model,
            celsius,
            max_c: milli("temp1_max"),
            crit_c: milli("temp1_crit"),
        });
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::usage::files::testutil::Tree;
    use std::io;
    use std::sync::Mutex;

    /// Sizes from a table, recording what was asked.
    struct Table(HashMap<String, FsSize>, Mutex<Vec<Vec<String>>>);

    impl Table {
        fn new(rows: &[(&str, u64, u64, u64)]) -> Self {
            let map = rows
                .iter()
                .map(|(m, t, u, a)| ((*m).to_owned(), FsSize { total: *t, used: *u, avail: *a }))
                .collect();
            Self(map, Mutex::new(Vec::new()))
        }
    }

    impl FsStats for Table {
        fn sizes(&self, mounts: &[&str]) -> HashMap<String, FsSize> {
            self.1.lock().unwrap().push(mounts.iter().map(|m| (*m).to_owned()).collect());
            mounts.iter().filter_map(|m| self.0.get(*m).map(|s| ((*m).to_owned(), *s))).collect()
        }
    }

    const MOUNTS: &str = "sysfs /sys sysfs rw,nosuid 0 0\nproc /proc proc rw 0 0\n\
        /dev/nvme1n1p2 / btrfs rw,relatime,subvolid=256 0 0\n\
        /dev/nvme1n1p2 /home btrfs rw,relatime,subvolid=257 0 0\n\
        /dev/nvme1n1p2 /var/log btrfs rw,subvolid=258 0 0\n\
        /dev/nvme1n1p1 /boot vfat rw 0 0\n\
        tmpfs /tmp tmpfs rw 0 0\n\
        /dev/sda1 /mnt/My\\040Disk ext4 rw 0 0\n\
        server:/share /mnt/nas nfs4 rw 0 0\n\
        server:/share /mnt/nas2 nfs4 rw 0 0\n\
        /dev/loop3 /snap/core/1 squashfs ro 0 0\n\
        /dev/sdb1 /run/media/me/usb vfat rw 0 0\n";

    #[test]
    fn mounts_are_parsed_with_escapes_decoded() {
        let m = parse_mounts(MOUNTS);
        assert_eq!(m[2].device, "/dev/nvme1n1p2");
        assert_eq!(m[7].mount, "/mnt/My Disk");
        assert_eq!(unescape("a\\134b"), "a\\b");
        assert_eq!(unescape("a\\9zb\\04"), "a\\9zb\\04", "not an octal escape: kept as it is");
        assert_eq!(unescape("\\400"), "\\400", "does not fit a byte");
        assert!(parse_mounts("short\n\n/dev/x /only\n").is_empty());
        assert_eq!(parse_mounts(&"/dev/a /a ext4 rw 0 0\n".repeat(2000)).len(), MAX_MOUNTS);
        let _ = parse_mounts("\\\\\\\n\0\0\0 \0 \0\n");
    }

    #[test]
    fn virtual_and_system_mounts_are_skipped() {
        for (t, m) in [
            ("tmpfs", "/tmp"),
            ("devtmpfs", "/dev"),
            ("efivarfs", "/sys/firmware/efi/efivars"),
            ("fuse.portal", "/x"),
            ("btrfs", "/snap/core"),
            ("ext4", "/run/user/1000/doc"),
            ("cgroup2", "/sys/fs/cgroup"),
        ] {
            assert!(is_skipped(t, m), "{t} {m}");
        }
        for (t, m) in [
            ("btrfs", "/"),
            ("vfat", "/boot"),
            ("btrfs", "/mnt/data"),
            ("vfat", "/run/media/me/usb"),
            ("ext4", "/runner"),
            ("ext4", "/snapshots"),
        ] {
            assert!(!is_skipped(t, m), "{t} {m}");
        }
    }

    #[test]
    fn a_device_mounted_twice_is_listed_once_and_the_order_is_the_pages() {
        let t = Tree::new();
        t.put("proc/self/mounts", MOUNTS);
        let stats = Table::new(&[
            ("/", 1_000_000_000_000, 200_000_000_000, 800_000_000_000),
            ("/home", 1_000_000_000_000, 200_000_000_000, 800_000_000_000),
            ("/var/log", 1_000_000_000_000, 200_000_000_000, 800_000_000_000),
            ("/boot", 1_073_741_824, 100, 1_073_741_000),
            ("/mnt/My Disk", 500, 250, 250),
            ("/mnt/nas", 800, 100, 700),
            ("/mnt/nas2", 800, 100, 700),
        ]);
        let disks = list_disks(t.root(), &stats);
        let mounts: Vec<&str> = disks.iter().map(|d| d.mount.as_str()).collect();
        assert_eq!(
            mounts,
            ["/", "/boot", "/mnt/My Disk", "/mnt/nas", "/mnt/nas2"],
            "one line per real device; two NFS shares are two lines"
        );
        assert_eq!(disks[0].used_pct, 20.0);
        assert_eq!(disks[2].used_pct, 50.0);
        let asked = stats.1.lock().unwrap();
        assert_eq!(asked.len(), 1, "one stats call for every mount");
        assert!(!asked[0].iter().any(|m| m == "/tmp" || m == "/sys"), "skipped mounts are not asked");
    }

    #[test]
    fn the_shortest_mount_point_wins_whatever_the_order() {
        let t = Tree::new();
        t.put("proc/self/mounts", "/dev/sda1 /home/deep/er ext4 rw 0 0\n/dev/sda1 /home ext4 rw 0 0\n");
        let stats = Table::new(&[("/home/deep/er", 10, 5, 5), ("/home", 10, 5, 5)]);
        let disks = list_disks(t.root(), &stats);
        assert_eq!(disks.len(), 1);
        assert_eq!(disks[0].mount, "/home");
    }

    #[test]
    fn unreadable_sizes_and_empty_filesystems_are_dropped() {
        let t = Tree::new();
        assert!(list_disks(t.root(), &Table::new(&[])).is_empty(), "no mounts file");
        t.put("proc/self/mounts", "/dev/a /a ext4 rw 0 0\n/dev/b /b ext4 rw 0 0\n/dev/c /c ext4 rw 0 0\n");
        let disks = list_disks(t.root(), &Table::new(&[("/a", 0, 0, 0), ("/c", 100, 100, 0)]));
        assert_eq!(disks.iter().map(|d| d.mount.as_str()).collect::<Vec<_>>(), ["/c"]);
        assert_eq!(disks[0].used_pct, 100.0);
    }

    /// The `/` line `df -B1` printed on a PH16-71: 1019913506816 total, 206531506176 used,
    /// 811842162688 available, 21 %.
    #[test]
    fn percent_reads_as_df_shows_it() {
        let size = FsSize { total: 1_019_913_506_816, used: 206_531_506_176, avail: 811_842_162_688 };
        assert_eq!(df_percent(size), 21.0);
        assert_eq!(df_percent(FsSize { total: 0, used: 0, avail: 0 }), 0.0);
        assert_eq!(df_percent(FsSize { total: 1, used: u64::MAX, avail: u64::MAX }), 100.0);
    }

    #[test]
    fn df_output_is_parsed_including_mounts_with_spaces_and_junk_rows() {
        let out = "        1B-blocks          Used         Avail Mounted on\n 1019913506816 206531506176 811842162688 /\n 500 250 250 /mnt/My Disk\n - - - /proc/foo\n short\n 10 x 5 /bad\n 7 1 6 \n";
        let sizes = parse_df(out);
        assert_eq!(sizes.len(), 2);
        assert_eq!(sizes["/"].total, 1_019_913_506_816);
        assert_eq!(sizes["/mnt/My Disk"].avail, 250);
        assert!(parse_df("").is_empty());
        assert!(parse_df("header only").is_empty());
    }

    struct Runner(Mutex<Vec<String>>, io::Result<Vec<u8>>);
    impl CommandRunner for Runner {
        fn output(&self, program: &str, args: &[&str], timeout: Duration) -> io::Result<Vec<u8>> {
            assert_eq!(timeout, DF_TIMEOUT);
            self.0.lock().unwrap().push(format!("{program} {}", args.join(" ")));
            match &self.1 {
                Ok(v) => Ok(v.clone()),
                Err(e) => Err(io::Error::new(e.kind(), "x")),
            }
        }
    }

    #[test]
    fn df_stats_runs_df_once_with_the_mounts_after_a_double_dash() {
        let runner = Arc::new(Runner(Mutex::new(Vec::new()), Ok(b"h\n 10 5 5 /a\n".to_vec())));
        let stats = DfStats::new(runner.clone());
        assert_eq!(stats.sizes(&["/a", "-weird mount"])["/a"].used, 5);
        assert_eq!(runner.0.lock().unwrap()[0], "df -B1 --output=size,used,avail,target -- /a -weird mount");
        assert!(stats.sizes(&[]).is_empty());
        assert_eq!(runner.0.lock().unwrap().len(), 1, "nothing to ask: no process");
        let broken =
            DfStats::new(Arc::new(Runner(Mutex::new(Vec::new()), Err(io::ErrorKind::NotFound.into()))));
        assert!(broken.sizes(&["/a"]).is_empty(), "df missing or timed out: no sizes, no panic");
    }

    #[test]
    fn block_devices_are_typed_and_tiny_ones_skipped() {
        let t = Tree::new();
        let b = "sys/block";
        t.put(&format!("{b}/nvme0n1/size"), "2000409264\n");
        t.put(&format!("{b}/nvme0n1/device/model"), "Samsung SSD 990 PRO 1TB   \n");
        t.put(&format!("{b}/sda/size"), "976773168\n");
        t.put(&format!("{b}/sda/queue/rotational"), "1\n");
        t.put(&format!("{b}/sdb/size"), "976773168\n");
        t.put(&format!("{b}/sdb/queue/rotational"), "0\n");
        t.put(&format!("{b}/sdc/size"), "0\n"); // empty card reader
        t.put(&format!("{b}/mmcblk0/size"), "62333952\n");
        t.put(&format!("{b}/loop0/size"), "999999999\n");
        t.put(&format!("{b}/zram0/size"), "999999999\n");
        t.put(&format!("{b}/sdd/size"), "garbage\n");
        let devices = block_devices(t.root());
        let summary: Vec<(&str, DriveKind)> = devices.iter().map(|d| (d.name.as_str(), d.kind)).collect();
        assert_eq!(
            summary,
            [
                ("mmcblk0", DriveKind::Mmc),
                ("nvme0n1", DriveKind::Nvme),
                ("sda", DriveKind::Hdd),
                ("sdb", DriveKind::Ssd)
            ]
        );
        let nvme = devices.iter().find(|d| d.name == "nvme0n1").unwrap();
        assert_eq!(nvme.size_bytes, 2_000_409_264 * 512);
        assert_eq!(nvme.model.as_deref(), Some("Samsung SSD 990 PRO 1TB"));
        assert!(block_devices(&t.root().join("nowhere")).is_empty());
    }

    #[test]
    fn nvme_temperatures_come_from_hwmon_with_the_controller_name() {
        let t = Tree::new();
        let h = "sys/class/hwmon";
        t.put(&format!("{h}/hwmon0/name"), "acpitz\n");
        t.put(&format!("{h}/hwmon0/temp1_input"), "27000\n");
        t.put(&format!("{h}/hwmon1/name"), "nvme\n");
        t.put(&format!("{h}/hwmon1/temp1_input"), "41850\n");
        t.put(&format!("{h}/hwmon1/temp1_max"), "81850\n");
        t.put(&format!("{h}/hwmon1/temp1_crit"), "84850\n");
        t.symlink(&format!("{h}/hwmon1/device"), "../../nvme/nvme0");
        t.put(&format!("{h}/hwmon2/name"), "nvme\n");
        t.put(&format!("{h}/hwmon2/temp1_input"), "36850\n");
        t.put(&format!("{h}/hwmon3/name"), "nvme\n");
        t.put(&format!("{h}/hwmon3/temp1_input"), "-274000\n");
        let temps = nvme_temps(t.root());
        assert_eq!(temps.len(), 2, "the drive with a broken reading is skipped");
        assert_eq!(temps[0].controller.as_deref(), Some("nvme0"));
        assert_eq!((temps[0].celsius, temps[0].max_c, temps[0].crit_c), (41.85, Some(81.85), Some(84.85)));
        assert_eq!(temps[1].controller, None);
        assert_eq!(temps[1].model, None);
        assert!(nvme_temps(&t.root().join("nowhere")).is_empty());
    }
}
