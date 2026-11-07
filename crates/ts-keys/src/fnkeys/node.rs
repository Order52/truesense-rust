//! Finding the keyboard's Fn-key interface among the hidraw nodes.
#![forbid(unsafe_code)]

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// The keyboard controller (Chicony 04F2:0117) as the kernel prints a HID id in `uevent`
/// (`HID_ID=0003:000004F2:00000117`).
const KEYBOARD_ID: &str = "000004F2:00000117";

/// How much of a sysfs file is read: a report descriptor is at most 4 KiB, and a file that is
/// not what it claims must not make this look endless.
const READ_LIMIT: u64 = 64 * 1024;

/// The hidraw node of the Fn-key interface: the node of the keyboard controller whose report
/// descriptor declares usage page 0x88 (the vendor page of the PREDATOR key and Fn+Esc). The
/// keyboard exposes several nodes with the same id; only this one carries those reports.
///
/// `sys_root` and `dev_root` stand for `/sys` and `/dev` (tests pass temporary trees). Nodes are
/// examined in name order as text, as the Python edition did (`hidraw10` before `hidraw2`), and a
/// node that cannot be read is skipped. `None` when nothing matches.
pub fn find_node_in(sys_root: &Path, dev_root: &Path) -> Option<PathBuf> {
    let mut names: Vec<String> = fs::read_dir(sys_root.join("class/hidraw"))
        .ok()?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.starts_with("hidraw"))
        .collect();
    names.sort();
    names.into_iter().find_map(|name| {
        let device = sys_root.join("class/hidraw").join(&name).join("device");
        let uevent = read_limited(&device.join("uevent"))?;
        if !String::from_utf8_lossy(&uevent).to_uppercase().contains(KEYBOARD_ID) {
            return None;
        }
        let descriptor = read_limited(&device.join("report_descriptor"))?;
        let vendor_page = contains(&descriptor, &[0x06, 0x88, 0x00]) || contains(&descriptor, &[0x05, 0x88]);
        vendor_page.then(|| dev_root.join(&name))
    })
}

/// [`find_node_in`] on the real `/sys` and `/dev`: what `Services::fn_key_node` wraps.
pub fn find_node() -> Option<PathBuf> {
    find_node_in(Path::new("/sys"), Path::new("/dev"))
}

fn read_limited(path: &Path) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    fs::File::open(path).ok()?.take(READ_LIMIT).read_to_end(&mut out).ok()?;
    Some(out)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}
