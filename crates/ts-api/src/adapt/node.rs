//! Finding the keyboard's Fn-key interface.
//!
//! The PREDATOR key and Fn+Esc/Fn+F10 arrive on a second HID interface of the keyboard controller
//! (the vendor usage page `0x88`, report id 4), separate from the lighting interface. The health
//! check wants to know whether that node can be opened, and the key service opens it; both find it
//! with [`find_fn_key_node`], which lives here because the application layer sits below the key
//! service and the health check is built in this layer.
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ts_foundation::fsx;

use super::machine::Machine;
use crate::api::NodeFinder;

/// The keyboard controller (Chicony `04F2:0117`) as the kernel prints it in a hid `uevent`.
const HID_ID: &str = "000004F2:00000117";

/// The most a `uevent` or report descriptor may be: the kernel's own limit is 4 KiB.
const SYSFS_LIMIT: u64 = 64 * 1024;

/// Whether a hidraw node's `uevent` text and report descriptor identify the Fn-key interface: the
/// keyboard controller's id and a descriptor that declares usage page `0x88` (`06 88 00` or
/// `05 88`). Both come from the kernel but are treated as untrusted: short or odd data is "no".
pub fn is_fn_key_interface(uevent: &str, descriptor: &[u8]) -> bool {
    let declares_page = |needle: &[u8]| descriptor.windows(needle.len()).any(|w| w == needle);
    uevent.to_uppercase().contains(HID_ID)
        && (declares_page(&[0x06, 0x88, 0x00]) || declares_page(&[0x05, 0x88]))
}

/// The `/dev/hidrawN` node of the Fn-key interface, looking below `sys_root` (the real `/`, or a
/// temporary tree) and naming the node below `dev_root`. Nodes are examined in name order, as the
/// Python edition did (`hidraw10` before `hidraw2`); `None` when there is none.
pub fn find_fn_key_node(sys_root: &Path, dev_root: &Path) -> Option<PathBuf> {
    let class_dir = sys_root.join("sys/class/hidraw");
    let mut nodes: Vec<String> = std::fs::read_dir(class_dir.as_path())
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("hidraw"))
        .collect();
    nodes.sort();
    nodes.into_iter().find_map(|node| {
        let device = class_dir.join(&node).join("device");
        let uevent = fsx::read_limited(&device.join("uevent"), SYSFS_LIMIT).ok()?;
        let descriptor = fsx::read_limited(&device.join("report_descriptor"), SYSFS_LIMIT).ok()?;
        is_fn_key_interface(&String::from_utf8_lossy(&uevent), &descriptor).then(|| dev_root.join(node))
    })
}

/// The finder for [`Services::fn_key_node`](crate::api::Services): looks every time it is asked,
/// because the keyboard can enumerate late after a resume.
pub(crate) fn finder(machine: &Machine) -> NodeFinder {
    let sys_root = machine.root.clone();
    let dev_root = machine.path("/dev");
    Arc::new(move || find_fn_key_node(&sys_root, &dev_root))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::adapt::testkit::TempDir;
    use std::fs;

    fn node(root: &Path, name: &str, uevent: &str, descriptor: &[u8]) {
        let dir = root.join("sys/class/hidraw").join(name).join("device");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("uevent"), uevent).unwrap();
        fs::write(dir.join("report_descriptor"), descriptor).unwrap();
    }

    const UEVENT: &str = "HID_ID=0003:000004F2:00000117\nHID_NAME=Chicony\n";

    #[test]
    fn the_node_with_the_vendor_page_is_found() {
        let tmp = TempDir::new();
        // The lighting interface of the same keyboard has page 0xFF02, not 0x88.
        node(&tmp.0, "hidraw1", UEVENT, &[0x06, 0x02, 0xFF, 0x09, 0x01]);
        node(&tmp.0, "hidraw3", UEVENT, &[0x05, 0x01, 0x06, 0x88, 0x00, 0x09, 0x01]);
        assert_eq!(find_fn_key_node(&tmp.0, Path::new("/dev")), Some(PathBuf::from("/dev/hidraw3")));
    }

    #[test]
    fn both_spellings_of_the_page_count_and_the_case_of_the_id_does_not() {
        for descriptor in [&[0x06u8, 0x88, 0x00][..], &[0x05, 0x88][..]] {
            assert!(is_fn_key_interface("hid_id=0003:000004f2:00000117", descriptor));
        }
    }

    #[test]
    fn another_device_a_short_or_empty_descriptor_and_missing_files_are_no() {
        let tmp = TempDir::new();
        node(&tmp.0, "hidraw0", "HID_ID=0003:0000046D:0000C52B\n", &[0x06, 0x88, 0x00]);
        node(&tmp.0, "hidraw1", UEVENT, &[]);
        node(&tmp.0, "hidraw2", UEVENT, &[0x06, 0x88]);
        fs::create_dir_all(tmp.0.join("sys/class/hidraw/hidraw4/device")).unwrap();
        fs::create_dir_all(tmp.0.join("sys/class/hidraw/notahidraw/device")).unwrap();
        assert_eq!(find_fn_key_node(&tmp.0, Path::new("/dev")), None);
        assert_eq!(find_fn_key_node(&tmp.0.join("nowhere"), Path::new("/dev")), None);
    }

    #[test]
    fn nodes_are_examined_in_string_order_like_python() {
        let tmp = TempDir::new();
        for name in ["hidraw2", "hidraw10"] {
            node(&tmp.0, name, UEVENT, &[0x05, 0x88]);
        }
        assert_eq!(find_fn_key_node(&tmp.0, &tmp.0.join("dev")), Some(tmp.0.join("dev/hidraw10")));
    }

    #[test]
    fn hostile_descriptors_cannot_panic() {
        for descriptor in [&[][..], &[0x05][..], &[0x06, 0x88][..], &[0xff; 70_000][..]] {
            let _ = is_fn_key_interface(UEVENT, descriptor);
        }
        assert!(!is_fn_key_interface("\u{0}\u{ffff}", &[0x05, 0x88]));
    }
}
