//! touchpad: switching the touchpad on and off through the kernel's per-device `inhibited`
//! file (`/sys/class/input/eventN/device/inhibited`).
//!
//! This works on any compositor, unlike a compositor-specific setting, and the file stays
//! authoritative: what the kernel says is what is reported. Writing needs root, so a normal user
//! goes through the daemon; reading does not.
//!
//! The touchpad is found by reading the kernel's input device list (`/proc/bus/input/devices`)
//! for the block that names a "Touchpad" from Synaptics (`SYNA`): the PH16-71's pad. Both paths
//! are fields so tests point them at a temporary tree.
#![forbid(unsafe_code)]

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use ts_foundation::fsx;

/// Why the touchpad could not be switched.
#[derive(Debug)]
pub enum TouchpadError {
    /// No Synaptics touchpad in the input device list (or the list cannot be read).
    NotFound,
    /// The kernel refused the write; usually "Permission denied" for a user that is not root.
    Switch(io::Error),
    /// The write was accepted but reading the state back says something else.
    Mismatch,
}

impl fmt::Display for TouchpadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("touchpad not found"),
            Self::Switch(e) => write!(
                f,
                "cannot switch the touchpad ({}); this needs root: go through the daemon",
                os_message(e)
            ),
            Self::Mismatch => f.write_str("touchpad readback mismatch"),
        }
    }
}

impl std::error::Error for TouchpadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Switch(e) => Some(e),
            _ => None,
        }
    }
}

/// The text of an OS error without Rust's " (os error 13)" suffix, which is what the Python
/// edition's `strerror` gave ("Permission denied").
fn os_message(e: &io::Error) -> String {
    let text = e.to_string();
    match text.rfind(" (os error ") {
        Some(i) if text.ends_with(')') => text[..i].to_owned(),
        _ => text,
    }
}

/// The touchpad of this machine.
#[derive(Debug, Clone)]
pub struct Touchpad {
    devices_file: PathBuf,
    sys_input: PathBuf,
}

impl Touchpad {
    /// A touchpad found through `devices_file` (the input device list) and switched below
    /// `sys_input` (`/sys/class/input`).
    pub fn new(devices_file: impl Into<PathBuf>, sys_input: impl Into<PathBuf>) -> Self {
        Self { devices_file: devices_file.into(), sys_input: sys_input.into() }
    }

    /// The real machine.
    pub fn system() -> Self {
        Self::new("/proc/bus/input/devices", "/sys/class/input")
    }

    /// The touchpad's `inhibited` file, or `None` when there is no touchpad.
    pub fn inhibit_file(&self) -> Option<PathBuf> {
        let bytes = fsx::read_limited(&self.devices_file, fsx::MAX_SETTINGS_FILE).ok()?;
        let event = find_event(&String::from_utf8_lossy(&bytes))?;
        Some(self.sys_input.join(event).join("device/inhibited"))
    }

    /// Whether the touchpad is on; `None` when it cannot be told (no touchpad, or the file
    /// cannot be read).
    pub fn enabled(&self) -> Option<bool> {
        let file = self.inhibit_file()?;
        read_inhibited(&file)
    }

    /// Switches the touchpad and reads the state back; returns the state reached. The file must
    /// already exist: it is never created, so a missing device stays an error.
    pub fn set_enabled(&self, on: bool) -> Result<bool, TouchpadError> {
        let file = self.inhibit_file().ok_or(TouchpadError::NotFound)?;
        write_existing(&file, if on { "0" } else { "1" }).map_err(TouchpadError::Switch)?;
        if read_inhibited(&file) != Some(on) {
            return Err(TouchpadError::Mismatch);
        }
        Ok(on)
    }
}

/// `inhibited` holds "0" while the device is active.
fn read_inhibited(file: &Path) -> Option<bool> {
    fs::read_to_string(file).ok().map(|t| t.trim() == "0")
}

fn write_existing(file: &Path, value: &str) -> io::Result<()> {
    fs::OpenOptions::new().write(true).open(file)?.write_all(value.as_bytes())
}

/// The `eventN` handler of the Synaptics touchpad in the text of `/proc/bus/input/devices`:
/// blocks are separated by a blank line; the right one mentions "Touchpad" and "SYNA" (any
/// case), and its `H: Handlers=` lines name `eventN`.
fn find_event(devices: &str) -> Option<String> {
    for block in devices.split("\n\n") {
        if !block.contains("Touchpad") || !block.to_uppercase().contains("SYNA") {
            continue;
        }
        let handlers: String =
            block.lines().filter(|l| l.starts_with("H: Handlers=")).collect::<Vec<_>>().concat();
        if let Some(event) = first_event(&handlers) {
            return Some(event);
        }
    }
    None
}

/// The first `event` followed by one or more digits.
fn first_event(text: &str) -> Option<String> {
    let mut rest = text;
    while let Some(i) = rest.find("event") {
        let after = &rest[i + "event".len()..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 {
            return Some(format!("event{}", &after[..digits]));
        }
        rest = after;
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let p = std::env::temp_dir().join(format!(
                "ts-system-touchpad-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const KEYBOARD: &str = "I: Bus=0011 Vendor=0001 Product=0001 Version=ab41\nN: Name=\"AT Translated Set 2 keyboard\"\nH: Handlers=sysrq kbd event3 leds\n";
    const MOUSE_ONLY: &str = "I: Bus=0018 Vendor=06cb Product=cd7e Version=0100\nN: Name=\"SYNA2BA6:00 06CB:CD7E Mouse\"\nH: Handlers=mouse0 event12\n";
    const PAD: &str = "I: Bus=0018 Vendor=06cb Product=cd7e Version=0100\nN: Name=\"SYNA2BA6:00 06CB:CD7E Touchpad\"\nP: Phys=\nH: Handlers=mouse1 event13 \nB: PROP=5\n";

    fn machine(devices: &str, inhibited: Option<&str>) -> (TempDir, Touchpad) {
        let tmp = TempDir::new();
        fs::write(tmp.0.join("devices"), devices).unwrap();
        fs::create_dir_all(tmp.0.join("sys/event13/device")).unwrap();
        if let Some(v) = inhibited {
            fs::write(tmp.0.join("sys/event13/device/inhibited"), v).unwrap();
        }
        let tp = Touchpad::new(tmp.0.join("devices"), tmp.0.join("sys"));
        (tmp, tp)
    }

    fn list(blocks: &[&str]) -> String {
        blocks.join("\n")
    }

    #[test]
    fn the_synaptics_touchpad_block_is_found_among_other_devices() {
        let text = list(&[KEYBOARD, MOUSE_ONLY, PAD]);
        assert_eq!(find_event(&text).as_deref(), Some("event13"));
        assert_eq!(find_event(&list(&[KEYBOARD, MOUSE_ONLY])), None, "SYNA but not a Touchpad");
        assert_eq!(
            find_event(&list(&[KEYBOARD, "N: Name=\"ELAN Touchpad\"\nH: Handlers=event5\n"])),
            None,
            "not Synaptics"
        );
        assert_eq!(find_event(""), None);
    }

    #[test]
    fn syna_matches_in_any_case() {
        let pad = "N: Name=\"syna0001 Touchpad\"\nH: Handlers=mouse2 event7\n";
        assert_eq!(find_event(pad).as_deref(), Some("event7"));
    }

    #[test]
    fn handlers_on_several_lines_are_joined_and_the_first_event_wins() {
        let pad = "N: Name=\"SYNA Touchpad\"\nH: Handlers=mouse0\nH: Handlers=event9 event10\n";
        assert_eq!(find_event(pad).as_deref(), Some("event9"));
        assert_eq!(first_event("eventX event 5 event42"), Some("event42".into()));
        assert_eq!(first_event("event"), None);
        assert_eq!(first_event("mouse1 leds"), None);
        // A touchpad block without any event handler is skipped; a later block may still match.
        let two = "N: Name=\"SYNA Touchpad\"\nH: Handlers=mouse0\n\nN: Name=\"SYNA Touchpad\"\nH: Handlers=event2\n";
        assert_eq!(find_event(two).as_deref(), Some("event2"));
    }

    #[test]
    fn enabled_reads_the_inhibit_file() {
        let devices = list(&[KEYBOARD, PAD]);
        let (_t, tp) = machine(&devices, Some("0\n"));
        assert_eq!(tp.enabled(), Some(true));
        let (_t, tp) = machine(&devices, Some("1\n"));
        assert_eq!(tp.enabled(), Some(false));
        let (_t, tp) = machine(&devices, Some("garbage"));
        assert_eq!(tp.enabled(), Some(false), "anything but 0 counts as inhibited, like the Python edition");
        let (_t, tp) = machine(&devices, None);
        assert_eq!(tp.enabled(), None, "no inhibit file: cannot tell");
        let (_t, tp) = machine(KEYBOARD, Some("0"));
        assert_eq!(tp.enabled(), None, "no touchpad");
        let tp = Touchpad::new("/nonexistent/devices", "/nonexistent/sys");
        assert_eq!((tp.enabled(), tp.inhibit_file()), (None, None));
    }

    #[test]
    fn inhibit_file_path_follows_the_event_number() {
        let (t, tp) = machine(&list(&[KEYBOARD, PAD]), Some("0"));
        assert_eq!(tp.inhibit_file(), Some(t.0.join("sys/event13/device/inhibited")));
    }

    #[test]
    fn set_enabled_writes_then_reads_back() {
        let (t, tp) = machine(&list(&[PAD]), Some("0"));
        assert!(!tp.set_enabled(false).unwrap());
        assert_eq!(fs::read_to_string(t.0.join("sys/event13/device/inhibited")).unwrap(), "1");
        assert_eq!(tp.enabled(), Some(false));
        assert!(tp.set_enabled(true).unwrap());
        assert_eq!(fs::read_to_string(t.0.join("sys/event13/device/inhibited")).unwrap(), "0");
    }

    #[test]
    fn set_enabled_without_a_touchpad_or_a_file_is_an_error_and_creates_nothing() {
        let (_t, tp) = machine(KEYBOARD, Some("0"));
        assert!(matches!(tp.set_enabled(true), Err(TouchpadError::NotFound)));
        let (t, tp) = machine(&list(&[PAD]), None);
        let err = tp.set_enabled(true).unwrap_err();
        assert!(matches!(err, TouchpadError::Switch(_)));
        assert!(!t.0.join("sys/event13/device/inhibited").exists(), "a missing file must not be created");
        let text = err.to_string();
        assert!(
            text.starts_with("cannot switch the touchpad (No such file or directory); this needs root"),
            "{text}"
        );
        assert!(std::error::Error::source(&err).is_some());
    }

    #[test]
    fn a_refused_write_is_a_switch_error_and_error_texts_are_fixed() {
        // A read-only file stands for a kernel that refuses the write.
        let (t, tp) = machine(&list(&[PAD]), Some("1"));
        let path = t.0.join("sys/event13/device/inhibited");
        let mut perms = fs::metadata(&path).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o444);
        fs::set_permissions(&path, perms).unwrap();
        // Running as root (a CI container) the write would succeed; then there is nothing to test.
        match tp.set_enabled(true) {
            Err(TouchpadError::Switch(e)) => assert_eq!(e.kind(), io::ErrorKind::PermissionDenied),
            Ok(_) => {}
            Err(other) => panic!("unexpected {other}"),
        }
        // The pure message of the mismatch is fixed text.
        assert_eq!(TouchpadError::Mismatch.to_string(), "touchpad readback mismatch");
        assert_eq!(TouchpadError::NotFound.to_string(), "touchpad not found");
    }

    #[test]
    fn os_messages_lose_their_numeric_suffix() {
        assert_eq!(os_message(&io::Error::from_raw_os_error(13)), "Permission denied");
        assert_eq!(os_message(&io::Error::other("plain")), "plain");
    }

    #[test]
    fn hostile_device_lists_never_panic() {
        for text in [
            "\n\n\n",
            "Touchpad SYNA",
            "Touchpad SYNA\nH: Handlers=event",
            "H: Handlers=event99999999999999999999999 Touchpad SYNA",
            "\u{0}\u{ffff}Touchpad SYNA H: Handlers=",
        ] {
            let _ = find_event(text);
        }
        assert_eq!(
            find_event("N: SYNA Touchpad\nH: Handlers=event99999999999999999999999\n").as_deref(),
            Some("event99999999999999999999999"),
            "the digits are only ever used as a path component below /sys/class/input"
        );
    }
}
