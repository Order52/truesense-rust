//! The systemd notification protocol (`sd_notify(3)`) for a `Type=notify` service with a watchdog.
//!
//! Why it exists: the daemon is the only thing between a hung control loop and a fan stuck low
//! under load. systemd's watchdog restarts the service when `WATCHDOG=1` stops arriving, and a
//! restart hands the fans back to the firmware. The daemon must therefore ping *only while its
//! loop is healthy*, and the same binary must also run in a terminal and in tests, where
//! `NOTIFY_SOCKET` is not set: every call is then a silent no-op.
//!
//! The protocol is one datagram of newline-separated `KEY=value` lines to the Unix datagram socket
//! named by `NOTIFY_SOCKET`, either a path or, when it starts with `@`, an abstract socket (a
//! name in the kernel namespace, leading NUL byte instead of `@`). std covers both, so there is no
//! `unsafe` here.
#![forbid(unsafe_code)]

use std::ffi::OsStr;
use std::fmt;
use std::io;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::{SocketAddr, UnixDatagram};
use std::path::PathBuf;
use std::time::Duration;

/// Why a notification could not be delivered. Never raised when systemd is simply absent.
#[derive(Debug)]
pub enum NotifyError {
    /// `NOTIFY_SOCKET` names something that cannot be an address (neither `/path` nor `@name`).
    BadAddress(String),
    /// The socket exists but sending failed (systemd gone, buffer full, permission).
    Send(io::Error),
}

impl fmt::Display for NotifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NotifyError::BadAddress(a) => write!(f, "sd_notify: unusable NOTIFY_SOCKET {a:?}"),
            NotifyError::Send(e) => write!(f, "sd_notify: send failed: {e}"),
        }
    }
}

impl std::error::Error for NotifyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            NotifyError::Send(e) => Some(e),
            NotifyError::BadAddress(_) => None,
        }
    }
}

/// Where notifications go: nowhere (not under systemd) or a socket address.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Disabled,
    Path(PathBuf),
    Abstract(Vec<u8>),
}

/// A sender of systemd notifications. Construct it once at start-up, pass it to whoever needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notifier {
    target: Target,
}

impl Notifier {
    /// A notifier that does nothing: for tests and for the terminal.
    pub fn disabled() -> Notifier {
        Notifier { target: Target::Disabled }
    }

    /// Reads `NOTIFY_SOCKET` from the process environment.
    pub fn from_env() -> Notifier {
        Self::from_lookup(&crate::fsx::env_nonempty)
    }

    /// Like [`Notifier::from_env`] with an injected environment. An unset or empty variable
    /// gives a disabled notifier; an unusable one (not `/path`, not `@name`) also does, because
    /// a service must not stop working over a bad notification address. [`Notifier::to_address`]
    /// reports such an address as an error for callers that want to log it.
    pub fn from_lookup(env: &dyn Fn(&str) -> Option<String>) -> Notifier {
        match env("NOTIFY_SOCKET") {
            Some(addr) => Self::to_address(&addr).unwrap_or_else(|_| Self::disabled()),
            None => Self::disabled(),
        }
    }

    /// A notifier for an explicit `NOTIFY_SOCKET` value (`/run/systemd/notify` or `@abstract`).
    pub fn to_address(addr: &str) -> Result<Notifier, NotifyError> {
        let bytes = addr.as_bytes();
        let target = match bytes.first() {
            Some(b'/') => Target::Path(PathBuf::from(OsStr::from_bytes(bytes))),
            Some(b'@') if bytes.len() > 1 => Target::Abstract(bytes[1..].to_vec()),
            _ => return Err(NotifyError::BadAddress(addr.to_owned())),
        };
        Ok(Notifier { target })
    }

    /// Whether notifications will actually be sent.
    pub fn is_enabled(&self) -> bool {
        self.target != Target::Disabled
    }

    /// Sends one raw state string (`"READY=1"`, `"WATCHDOG=1"`, several lines joined by `\n`).
    /// Returns `Ok(false)` when disabled, `Ok(true)` when the datagram was sent.
    pub fn notify(&self, state: &str) -> Result<bool, NotifyError> {
        let addr = match &self.target {
            Target::Disabled => return Ok(false),
            Target::Path(p) => SocketAddr::from_pathname(p).map_err(NotifyError::Send)?,
            Target::Abstract(name) => SocketAddr::from_abstract_name(name).map_err(NotifyError::Send)?,
        };
        let socket = UnixDatagram::unbound().map_err(NotifyError::Send)?;
        socket.send_to_addr(state.as_bytes(), &addr).map_err(NotifyError::Send)?;
        Ok(true)
    }

    /// `READY=1`: start-up finished. Sent after the control socket listens and the boot
    /// settings are applied, so `systemctl start` returns only when the service is usable.
    pub fn ready(&self) -> Result<bool, NotifyError> {
        self.notify("READY=1")
    }

    /// `WATCHDOG=1`: the loop is alive. Call it only from a loop that is making progress.
    pub fn watchdog(&self) -> Result<bool, NotifyError> {
        self.notify("WATCHDOG=1")
    }

    /// `STOPPING=1`: shutdown has begun.
    pub fn stopping(&self) -> Result<bool, NotifyError> {
        self.notify("STOPPING=1")
    }

    /// `STATUS=...`: the free-form line `systemctl status` shows. Line breaks are replaced by
    /// spaces, because a newline would end the assignment and let the text smuggle in other
    /// ones (`"x\nREADY=1"`).
    pub fn status(&self, text: &str) -> Result<bool, NotifyError> {
        self.notify(&format!("STATUS={}", text.replace(['\n', '\r'], " ")))
    }
}

/// How often to send `WATCHDOG=1`: half the timeout systemd configured, as `sd_watchdog_enabled(3)`
/// recommends, or `None` when no watchdog is armed for this process. `env` is the environment
/// lookup and `pid` this process's id: `WATCHDOG_PID`, when set, names the one process the
/// watchdog applies to (a child that inherited the variables must not ping).
pub(crate) fn watchdog_interval(env: &dyn Fn(&str) -> Option<String>, pid: u32) -> Option<Duration> {
    let usec: u64 = env("WATCHDOG_USEC")?.trim().parse().ok().filter(|&u| u > 0)?;
    if let Some(target) = env("WATCHDOG_PID") {
        if target.trim().parse::<u32>().ok() != Some(pid) {
            return None;
        }
    }
    Some(Duration::from_micros(usec / 2))
}

/// `watchdog_interval` for the running process.
pub fn watchdog_interval_from_env() -> Option<Duration> {
    watchdog_interval(&crate::fsx::env_nonempty, std::process::id())
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::fsx::testing::TempDir;
    use std::collections::HashMap;
    use std::time::SystemTime;

    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        let m: HashMap<&str, &str> = pairs.iter().copied().collect();
        move |k| m.get(k).filter(|v| !v.is_empty()).map(|v| (*v).to_owned())
    }

    fn recv(sock: &UnixDatagram) -> String {
        sock.set_read_timeout(Some(Duration::from_secs(2))).expect("timeout");
        let mut buf = [0u8; 512];
        let n = sock.recv(&mut buf).expect("datagram");
        String::from_utf8(buf[..n].to_vec()).expect("utf8")
    }

    #[test]
    fn unset_or_empty_socket_is_a_silent_noop() {
        for pairs in [&[][..], &[("NOTIFY_SOCKET", "")][..]] {
            let n = Notifier::from_lookup(&env_of(pairs));
            assert!(!n.is_enabled());
            assert!(matches!(n.ready(), Ok(false)));
            assert!(matches!(n.watchdog(), Ok(false)));
            assert!(matches!(n.stopping(), Ok(false)));
            assert!(matches!(n.status("x"), Ok(false)));
        }
        assert!(!Notifier::disabled().is_enabled());
    }

    #[test]
    fn messages_reach_a_path_socket() {
        let d = TempDir::new("sdnotify-path");
        let path = d.join("notify.sock");
        let server = UnixDatagram::bind(&path).expect("bind");
        let n = Notifier::from_lookup(&env_of(&[("NOTIFY_SOCKET", path.to_str().expect("utf8 path"))]));
        assert!(n.is_enabled());
        assert!(n.ready().expect("ready"));
        assert_eq!(recv(&server), "READY=1");
        n.watchdog().expect("watchdog");
        assert_eq!(recv(&server), "WATCHDOG=1");
        n.stopping().expect("stopping");
        assert_eq!(recv(&server), "STOPPING=1");
        n.status("fans on auto").expect("status");
        assert_eq!(recv(&server), "STATUS=fans on auto");
    }

    #[test]
    fn messages_reach_an_abstract_socket() {
        let name = format!(
            "truesense-test-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos())
        );
        let server = UnixDatagram::bind_addr(&SocketAddr::from_abstract_name(name.as_bytes()).expect("addr"))
            .expect("bind");
        let n = Notifier::from_lookup(&env_of(&[("NOTIFY_SOCKET", &format!("@{name}"))]));
        assert!(n.is_enabled());
        n.ready().expect("ready");
        assert_eq!(recv(&server), "READY=1");
    }

    #[test]
    fn status_cannot_smuggle_other_assignments() {
        let d = TempDir::new("sdnotify-status");
        let path = d.join("notify.sock");
        let server = UnixDatagram::bind(&path).expect("bind");
        let n = Notifier::to_address(path.to_str().expect("utf8 path")).expect("address");
        n.status("a\nREADY=1\r\nWATCHDOG=trigger").expect("status");
        assert_eq!(recv(&server), "STATUS=a READY=1  WATCHDOG=trigger");
    }

    #[test]
    fn hostile_addresses_are_errors_or_disabled_never_panics() {
        for bad in ["", "relative/path", "@", "no-slash", "\0", "ftp://x"] {
            assert!(matches!(Notifier::to_address(bad), Err(NotifyError::BadAddress(_))), "{bad:?}");
        }
        // From the environment a bad address disables notification instead of failing the service.
        assert!(!Notifier::from_lookup(&env_of(&[("NOTIFY_SOCKET", "garbage")])).is_enabled());
        // A name too long for sockaddr_un is a send error, not a panic.
        let long = Notifier::to_address(&format!("/{}", "a".repeat(5000))).expect("address");
        assert!(matches!(long.ready(), Err(NotifyError::Send(_))));
    }

    #[test]
    fn a_vanished_socket_is_an_error_the_caller_may_ignore() {
        let d = TempDir::new("sdnotify-gone");
        let n = Notifier::to_address(d.join("gone.sock").to_str().expect("utf8 path")).expect("address");
        let err = n.watchdog().expect_err("nobody listens");
        assert!(err.to_string().starts_with("sd_notify: send failed"), "{err}");
    }

    #[test]
    fn watchdog_interval_is_half_the_timeout() {
        type Case<'a> = (&'a [(&'a str, &'a str)], u32, Option<Duration>);
        let cases: [Case; 9] = [
            (&[], 10, None),
            (&[("WATCHDOG_USEC", "10000000")], 10, Some(Duration::from_secs(5))),
            (&[("WATCHDOG_USEC", " 20000000 ")], 10, Some(Duration::from_secs(10))),
            (&[("WATCHDOG_USEC", "10000000"), ("WATCHDOG_PID", "10")], 10, Some(Duration::from_secs(5))),
            (&[("WATCHDOG_USEC", "10000000"), ("WATCHDOG_PID", "11")], 10, None),
            (&[("WATCHDOG_USEC", "10000000"), ("WATCHDOG_PID", "junk")], 10, None),
            (&[("WATCHDOG_USEC", "0")], 10, None),
            (&[("WATCHDOG_USEC", "-5")], 10, None),
            (&[("WATCHDOG_USEC", "soon")], 10, None),
        ];
        for (pairs, pid, want) in cases {
            assert_eq!(watchdog_interval(&env_of(pairs), pid), want, "{pairs:?} pid {pid}");
        }
    }
}
