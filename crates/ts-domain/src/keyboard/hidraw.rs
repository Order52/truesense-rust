//! The real keyboard: the Chicony controller `04F2:0117`, reached through its `/dev/hidrawN` node.
//!
//! The controller's lighting interface is the hidraw node whose report descriptor starts with
//! usage page `0xFF02` (`06 02 FF`); the keyboard's other interfaces share vendor and product
//! id. This is the only place of the crate that needs `unsafe`: the two ioctls that send and
//! read a feature report (`HIDIOCSFEATURE`, `HIDIOCGFEATURE`) have no safe wrapper in `std`.
#![allow(unsafe_code)]

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use ts_foundation::fsx;

use super::{HidDevice, HidOpener, KeyboardError, OUTPUT_LEN, PACKET_LEN};

/// Vendor and product id of the keyboard controller, as the kernel writes it in `uevent`.
const HID_ID: &str = "000004F2:00000117";

/// The first bytes of the lighting interface's report descriptor: usage page `0xFF02`.
const LIGHTING_USAGE: [u8; 3] = [0x06, 0x02, 0xFF];

/// The largest report descriptor or `uevent` read; the kernel's own limit is 4 KiB.
const SYSFS_LIMIT: u64 = 64 * 1024;

/// Whether a hidraw node's `uevent` text and report descriptor identify the keyboard's lighting
/// interface. Both come from the kernel but are treated as untrusted: short or odd data is "no".
pub fn is_lighting_interface(uevent: &str, descriptor: &[u8]) -> bool {
    uevent.to_uppercase().contains(HID_ID) && descriptor.starts_with(&LIGHTING_USAGE)
}

/// The name (`hidrawN`) of the lighting interface among the nodes in `class_dir`
/// (`/sys/class/hidraw`). Nodes are examined in string order, as the Python edition did, so
/// `hidraw10` comes before `hidraw2`.
pub fn find_node(class_dir: &Path) -> Result<String, KeyboardError> {
    let mut nodes: Vec<String> = fs::read_dir(class_dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| n.starts_with("hidraw"))
                .collect()
        })
        .unwrap_or_default();
    nodes.sort();
    for node in nodes {
        let device = class_dir.join(&node).join("device");
        let Ok(uevent) = fsx::read_limited(&device.join("uevent"), SYSFS_LIMIT) else { continue };
        let Ok(descriptor) = fsx::read_limited(&device.join("report_descriptor"), SYSFS_LIMIT) else {
            continue;
        };
        if is_lighting_interface(&String::from_utf8_lossy(&uevent), &descriptor) {
            return Ok(node);
        }
    }
    Err(KeyboardError::NotFound)
}

/// Opens the real keyboard. The two directories are parameters so that tests can point them at a
/// temporary tree; production uses [`HidrawOpener::system`].
#[derive(Debug, Clone)]
pub struct HidrawOpener {
    class_dir: PathBuf,
    dev_dir: PathBuf,
}

impl HidrawOpener {
    /// The real `/sys/class/hidraw` and `/dev`.
    pub fn system() -> Self {
        Self::with_dirs("/sys/class/hidraw", "/dev")
    }

    /// An opener that looks for the keyboard in `class_dir` and opens its node in `dev_dir`.
    pub fn with_dirs(class_dir: impl Into<PathBuf>, dev_dir: impl Into<PathBuf>) -> Self {
        Self { class_dir: class_dir.into(), dev_dir: dev_dir.into() }
    }
}

impl HidOpener for HidrawOpener {
    fn open(&self) -> Result<Box<dyn HidDevice>, KeyboardError> {
        let node = self.dev_dir.join(find_node(&self.class_dir)?);
        match OpenOptions::new().read(true).write(true).open(&node) {
            Ok(file) => Ok(Box::new(Hidraw { file })),
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => Err(KeyboardError::Permission { node }),
            Err(source) => Err(KeyboardError::Io { op: "open the keyboard", source }),
        }
    }
}

/// An open `/dev/hidrawN`; the descriptor closes when this is dropped.
struct Hidraw {
    file: File,
}

/// The ioctl request number `_IOC(read | write, 'H', nr, len)`: `len` is the size of the buffer
/// including the leading report id. `HIDIOCSFEATURE` is number 6, `HIDIOCGFEATURE` number 7.
fn hidioc(nr: u32, len: usize) -> u32 {
    const READ_WRITE: u32 = 0xC000_0000;
    READ_WRITE | (len as u32) << 16 | u32::from(b'H') << 8 | nr
}

const SET_FEATURE: u32 = 6;
const GET_FEATURE: u32 = 7;

impl Hidraw {
    fn ioctl(&self, nr: u32, buf: &mut [u8]) -> io::Result<()> {
        let request = hidioc(nr, buf.len());
        loop {
            // SAFETY: the descriptor stays open for the whole call (`self.file` owns it); `buf`
            // is valid for reads and writes of `buf.len()` bytes and the request number encodes
            // exactly that length, so the kernel never touches memory outside the slice.
            let rc = unsafe { libc::ioctl(self.file.as_raw_fd(), request as libc::Ioctl, buf.as_mut_ptr()) };
            if rc >= 0 {
                return Ok(());
            }
            let err = io::Error::last_os_error();
            if err.kind() != io::ErrorKind::Interrupted {
                return Err(err);
            }
        }
    }
}

impl HidDevice for Hidraw {
    /// Sends report id 0 plus the 8 data bytes.
    fn set_feature(&mut self, payload: &[u8; PACKET_LEN]) -> io::Result<()> {
        let mut buf = [0u8; 1 + PACKET_LEN];
        buf[1..].copy_from_slice(payload);
        self.ioctl(SET_FEATURE, &mut buf)
    }

    /// Reads report id 0 plus 8 data bytes and returns the data.
    fn get_feature(&mut self) -> io::Result<[u8; PACKET_LEN]> {
        let mut buf = [0u8; 1 + PACKET_LEN];
        self.ioctl(GET_FEATURE, &mut buf)?;
        let mut data = [0u8; PACKET_LEN];
        data.copy_from_slice(&buf[1..]);
        Ok(data)
    }

    /// Writes the 64 data bytes behind a leading 0: that is the report id, which hidraw strips,
    /// so the device gets exactly 64 data bytes.
    fn write_output(&mut self, data: &[u8; OUTPUT_LEN]) -> io::Result<()> {
        let mut buf = [0u8; 1 + OUTPUT_LEN];
        buf[1..].copy_from_slice(data);
        let written = loop {
            match self.file.write(&buf) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                other => break other?,
            }
        };
        if written == buf.len() {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::WriteZero,
                format!("short write: {written} of {} bytes", buf.len()),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    use super::*;
    use crate::keymap::testutil::TempDir;

    const LIGHTING_UEVENT: &str =
        "DRIVER=hid-generic\nHID_ID=0003:000004F2:00000117\nHID_NAME=Chicony Acer\n";

    #[test]
    fn ioctl_request_numbers_are_those_of_linux_hidraw_h() {
        // HIDIOCSFEATURE(9) and HIDIOCGFEATURE(9) as the kernel header computes them.
        assert_eq!(hidioc(SET_FEATURE, 9), 0xC009_4806);
        assert_eq!(hidioc(GET_FEATURE, 9), 0xC009_4807);
        assert_eq!(hidioc(SET_FEATURE, 65), 0xC041_4806);
    }

    #[test]
    fn only_the_vendor_interface_of_the_keyboard_is_the_lighting_interface() {
        let ok = [0x06, 0x02, 0xFF, 0x09, 0x01];
        assert!(is_lighting_interface(LIGHTING_UEVENT, &ok));
        assert!(
            is_lighting_interface(&LIGHTING_UEVENT.to_lowercase(), &ok),
            "the id is matched in either case"
        );
        assert!(!is_lighting_interface(LIGHTING_UEVENT, &[0x05, 0x01, 0x09, 0x06]), "the typing interface");
        assert!(!is_lighting_interface("HID_ID=0003:0000046D:0000C52B", &ok), "another device");
        for odd in [&[][..], &[0x06], &[0x06, 0x02], &[0xFF, 0xFF, 0xFF]] {
            assert!(!is_lighting_interface(LIGHTING_UEVENT, odd), "{odd:?}");
        }
        assert!(!is_lighting_interface("", &ok));
    }

    fn add_node(class: &Path, name: &str, uevent: &str, descriptor: Option<&[u8]>) {
        let dev = class.join(name).join("device");
        fs::create_dir_all(&dev).unwrap();
        fs::write(dev.join("uevent"), uevent).unwrap();
        if let Some(d) = descriptor {
            fs::write(dev.join("report_descriptor"), d).unwrap();
        }
    }

    #[test]
    fn the_node_is_found_among_siblings_in_string_order() {
        let t = TempDir::new("hid-find");
        let class = t.join("class");
        let lighting = [0x06, 0x02, 0xFF];
        assert!(matches!(find_node(&class), Err(KeyboardError::NotFound)), "no directory at all");
        fs::create_dir_all(&class).unwrap();
        assert!(matches!(find_node(&class), Err(KeyboardError::NotFound)), "empty directory");
        add_node(&class, "hidraw0", "HID_ID=0003:0000046D:0000C52B", Some(&lighting));
        add_node(&class, "hidraw1", LIGHTING_UEVENT, Some(&[0x05, 0x01]));
        add_node(&class, "hidraw2", LIGHTING_UEVENT, None); // no descriptor: skipped, not fatal
        add_node(&class, "other", LIGHTING_UEVENT, Some(&lighting)); // not a hidraw node
        assert!(matches!(find_node(&class), Err(KeyboardError::NotFound)));
        add_node(&class, "hidraw3", LIGHTING_UEVENT, Some(&lighting));
        add_node(&class, "hidraw10", LIGHTING_UEVENT, Some(&lighting));
        assert_eq!(find_node(&class).unwrap(), "hidraw10", "string order, as Python's sorted(glob(...))");
        assert_eq!(
            KeyboardError::NotFound.to_string(),
            "keyboard lighting interface (usage page 0xFF02) not found"
        );
    }

    #[test]
    fn a_missing_node_or_permission_is_reported_clearly() {
        let t = TempDir::new("hid-open");
        let class = t.join("class");
        let dev = t.join("dev");
        fs::create_dir_all(&dev).unwrap();
        add_node(&class, "hidraw4", LIGHTING_UEVENT, Some(&[0x06, 0x02, 0xFF]));
        let opener = HidrawOpener::with_dirs(&class, &dev);
        let missing = opener.open().err().expect("the node does not exist");
        assert!(matches!(missing, KeyboardError::Io { .. }), "{missing}");

        let node = dev.join("hidraw4");
        fs::write(&node, b"").unwrap();
        fs::set_permissions(&node, fs::Permissions::from_mode(0o000)).unwrap();
        let root = fs::metadata("/proc/self").map(|m| m.uid() == 0).unwrap_or(false);
        if !root {
            let denied = opener.open().err().expect("no permission");
            assert!(matches!(denied, KeyboardError::Permission { .. }), "{denied}");
            let text = denied.to_string();
            assert!(
                text.contains("no permission") && text.contains("plugdev") && text.contains("hidraw4"),
                "{text}"
            );
        }
    }

    #[test]
    fn output_reports_are_written_with_the_leading_report_id_and_feature_ioctls_fail_cleanly() {
        let t = TempDir::new("hid-io");
        let class = t.join("class");
        let dev = t.join("dev");
        fs::create_dir_all(&dev).unwrap();
        add_node(&class, "hidraw0", LIGHTING_UEVENT, Some(&[0x06, 0x02, 0xFF]));
        // A regular file stands in for the node: it takes the write, and refuses the ioctls
        // with ENOTTY, which is what a descriptor of the wrong kind must produce.
        fs::write(dev.join("hidraw0"), b"").unwrap();
        let mut device = HidrawOpener::with_dirs(&class, &dev).open().expect("open");
        let mut report = [0u8; OUTPUT_LEN];
        report[0] = 0xAA;
        report[63] = 0xBB;
        device.write_output(&report).expect("write");
        let written = fs::read(dev.join("hidraw0")).unwrap();
        assert_eq!(written.len(), 65);
        assert_eq!(
            (written[0], written[1], written[64]),
            (0, 0xAA, 0xBB),
            "a 0 report id first, then the 64 data bytes"
        );
        let err = device.set_feature(&[1, 2, 3, 4, 5, 6, 7, 8]).expect_err("not a hid device");
        assert_eq!(err.raw_os_error(), Some(libc::ENOTTY));
        let err = device.get_feature().expect_err("not a hid device");
        assert_eq!(err.raw_os_error(), Some(libc::ENOTTY));
    }
}
