//! Reading input reports from the keyboard's hidraw node.
//!
//! The service reads through [`HidReports`] and opens through [`HidOpener`], so its tests feed a
//! scripted stream and never touch `/dev`. The real implementation, [`HidrawFile`], is the one
//! place of this crate that needs `unsafe`: std cannot wait on a file with a timeout, and the
//! service must wake for its gesture timers, the config poll and the systemd watchdog even when no
//! key is pressed. The single call is `poll(2)` on a descriptor this file owns.
#![allow(unsafe_code)]

use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::watch::Watcher;

/// A hidraw node hands out exactly one report per `read`; the vendor reports of this keyboard are
/// 3 bytes, and 64 is the largest a full-speed HID endpoint sends.
const REPORT_BUFFER: usize = 64;

/// Why the keyboard's reports could not be opened or read.
#[derive(Debug)]
pub enum HidError {
    /// The node could not be opened.
    Open {
        /// The node.
        node: PathBuf,
        /// What the system said.
        source: io::Error,
    },
    /// Waiting for or reading a report failed.
    Read(io::Error),
    /// The node is gone: hung up (the keyboard was unplugged or re-enumerated, as after
    /// suspend) or at the end of the stream. The service reconnects.
    Gone,
}

impl fmt::Display for HidError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open { node, source } => write!(f, "open {}: {source}", node.display()),
            Self::Read(source) => write!(f, "read Fn-key report: {source}"),
            Self::Gone => f.write_str("the Fn-key interface went away"),
        }
    }
}

impl std::error::Error for HidError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Open { source, .. } | Self::Read(source) => Some(source),
            Self::Gone => None,
        }
    }
}

/// An open stream of input reports.
pub trait HidReports: Send {
    /// Waits up to `timeout` for the next report. `Ok(None)` means none came in time (the caller
    /// has timers to run). An error ends this stream: the service opens a new one.
    fn read_report(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, HidError>;

    /// [`HidReports::read_report`] that also returns (with `Ok(None)`) as soon as `watch` says a
    /// watched file changed, so the caller can look at it without polling on a timer. A stream
    /// that cannot wait on two things ignores `watch`.
    fn read_report_watching(
        &mut self,
        timeout: Duration,
        _watch: Option<&Watcher>,
    ) -> Result<Option<Vec<u8>>, HidError> {
        self.read_report(timeout)
    }
}

/// Opens the report stream of a node.
pub trait HidOpener: Send + Sync {
    /// Opens `node` (a path found by `find_node`).
    fn open(&self, node: &Path) -> Result<Box<dyn HidReports>, HidError>;
}

/// The real thing: opens `/dev/hidrawN`.
#[derive(Debug, Clone, Copy, Default)]
pub struct HidrawOpener;

impl HidOpener for HidrawOpener {
    fn open(&self, node: &Path) -> Result<Box<dyn HidReports>, HidError> {
        HidrawFile::open(node).map(|f| Box::new(f) as Box<dyn HidReports>)
    }
}

/// A hidraw node opened for reading.
#[derive(Debug)]
pub struct HidrawFile {
    file: File,
}

impl HidrawFile {
    /// Opens `node` read-only and non-blocking, so a wake-up with nothing to read can never hang
    /// the loop. (The service only reads: nothing is ever written to the keyboard from here.)
    pub fn open(node: &Path) -> Result<Self, HidError> {
        let file = File::options()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(node)
            .map_err(|source| HidError::Open { node: node.to_owned(), source })?;
        Ok(Self { file })
    }

    /// A stream over any readable descriptor that `poll` understands (a socket or pipe in tests).
    pub fn from_file(file: File) -> Self {
        Self { file }
    }
}

/// What `poll` said about the descriptor.
#[derive(Debug, PartialEq, Eq)]
enum Readiness {
    Timeout,
    Readable,
    HungUp,
}

/// Waits until `fd` is readable, hung up, or `timeout` has passed (rounded up to whole
/// milliseconds: `poll` has no finer step and a zero would spin). With `watch`, also returns when
/// it has something to say (after emptying it).
fn wait_readable(fd: RawFd, watch: Option<&Watcher>, timeout: Duration) -> io::Result<Readiness> {
    let millis = libc::c_int::try_from(timeout.as_nanos().div_ceil(1_000_000)).unwrap_or(libc::c_int::MAX);
    let mut pfds = [
        libc::pollfd { fd, events: libc::POLLIN, revents: 0 },
        libc::pollfd { fd: watch.map_or(-1, Watcher::fd), events: libc::POLLIN, revents: 0 },
    ];
    // SAFETY: `pfds` holds two initialised `pollfd`s that live until the call returns, and `nfds`
    // is 2, the number of elements behind the pointer. `poll` only writes `revents`; a negative
    // `fd` (no watcher) is ignored by `poll`, and a descriptor that is not open is reported
    // through `POLLNVAL`, not by undefined behaviour.
    let ready = unsafe { libc::poll(pfds.as_mut_ptr(), 2, millis) };
    if ready < 0 {
        let error = io::Error::last_os_error();
        // A signal interrupted the wait: the caller's loop simply comes round again.
        return if error.kind() == io::ErrorKind::Interrupted { Ok(Readiness::Timeout) } else { Err(error) };
    }
    if pfds[1].revents != 0 {
        if let Some(watch) = watch {
            watch.drain();
        }
    }
    let pfd = &pfds[0];
    Ok(if pfd.revents == 0 {
        // Nothing from the keyboard: a timeout, or only the watcher spoke (the caller looks).
        Readiness::Timeout
    } else if pfd.revents & libc::POLLIN != 0 {
        // Data first: a hang-up with a report still queued delivers the report, and the next
        // read ends the stream.
        Readiness::Readable
    } else {
        Readiness::HungUp
    })
}

impl HidReports for HidrawFile {
    fn read_report(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, HidError> {
        self.read_report_watching(timeout, None)
    }

    fn read_report_watching(
        &mut self,
        timeout: Duration,
        watch: Option<&Watcher>,
    ) -> Result<Option<Vec<u8>>, HidError> {
        match wait_readable(self.file.as_raw_fd(), watch, timeout).map_err(HidError::Read)? {
            Readiness::Timeout => return Ok(None),
            Readiness::HungUp => return Err(HidError::Gone),
            Readiness::Readable => {}
        }
        let mut buffer = [0u8; REPORT_BUFFER];
        match (&self.file).read(&mut buffer) {
            Ok(0) => Err(HidError::Gone),
            Ok(n) => Ok(Some(buffer[..n].to_vec())),
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => Ok(None),
            // A removed device answers ENODEV (or EIO while it is going): the stream is over.
            Err(e) if matches!(e.raw_os_error(), Some(libc::ENODEV | libc::EIO)) => Err(HidError::Gone),
            Err(e) => Err(HidError::Read(e)),
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use std::io::Write;
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::time::Instant;

    use super::*;

    fn pair() -> (HidrawFile, UnixStream) {
        let (device, keyboard) = UnixStream::pair().expect("socket pair");
        (HidrawFile::from_file(File::from(OwnedFd::from(device))), keyboard)
    }

    #[test]
    fn a_quiet_stream_times_out_after_about_the_timeout() {
        let (mut reports, _keyboard) = pair();
        let started = Instant::now();
        assert!(matches!(reports.read_report(Duration::from_millis(30)), Ok(None)));
        assert!(started.elapsed() >= Duration::from_millis(25), "waited {:?}", started.elapsed());
    }

    #[test]
    fn a_zero_timeout_returns_at_once() {
        let (mut reports, _keyboard) = pair();
        assert!(matches!(reports.read_report(Duration::ZERO), Ok(None)));
    }

    #[test]
    fn a_report_is_delivered_whole() {
        let (mut reports, mut keyboard) = pair();
        keyboard.write_all(&[0x04, 0x81, 0xff]).expect("write");
        assert_eq!(reports.read_report(Duration::from_secs(1)).expect("read"), Some(vec![0x04, 0x81, 0xff]));
        assert!(matches!(reports.read_report(Duration::ZERO), Ok(None)));
    }

    #[test]
    fn an_unplugged_keyboard_ends_the_stream() {
        let (mut reports, keyboard) = pair();
        drop(keyboard);
        assert!(matches!(reports.read_report(Duration::from_secs(1)), Err(HidError::Gone)));
    }

    #[test]
    fn a_queued_report_is_read_before_the_hang_up() {
        let (mut reports, mut keyboard) = pair();
        keyboard.write_all(&[0x04, 0x00, 0x00]).expect("write");
        drop(keyboard);
        assert_eq!(reports.read_report(Duration::from_secs(1)).expect("read"), Some(vec![0x04, 0x00, 0x00]));
        assert!(matches!(reports.read_report(Duration::from_secs(1)), Err(HidError::Gone)));
    }

    #[test]
    fn a_huge_timeout_does_not_overflow() {
        let (mut reports, mut keyboard) = pair();
        keyboard.write_all(&[1, 2, 3]).expect("write");
        assert!(reports.read_report(Duration::MAX).expect("read").is_some());
    }

    #[test]
    fn opening_a_missing_node_says_which() {
        let error = HidrawOpener.open(Path::new("/nonexistent/hidraw99")).err().expect("must fail");
        assert!(error.to_string().starts_with("open /nonexistent/hidraw99: "), "{error}");
    }
}
