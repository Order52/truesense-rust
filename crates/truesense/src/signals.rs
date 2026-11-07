//! Ctrl-C and SIGTERM for the foreground commands: a flag that becomes true when one arrives.
#![allow(unsafe_code)]
//!
//! The daemon does not use this (it blocks the signals and waits for them itself, see
//! `ts_daemon::assembly`); neither does the terminal UI, which sees Ctrl-C as a key.

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, OnceLock};

use ts_foundation::wake;

/// Write end of the self-pipe the handler pokes (-1 until the handlers are installed).
static PIPE_WRITE: AtomicI32 = AtomicI32::new(-1);

/// The only thing a handler may safely do is call async-signal-safe functions: one byte into a
/// non-blocking pipe, which wakes the bridge thread that is blocked reading the other end.
extern "C" fn on_signal(_signal: libc::c_int) {
    let fd = PIPE_WRITE.load(Ordering::Relaxed);
    if fd >= 0 {
        let byte = 1u8;
        // SAFETY: `write` is async-signal-safe; `byte` is a valid one-byte buffer; the result (a
        // full pipe already holds a wake-up) is deliberately ignored.
        unsafe {
            libc::write(fd, (&raw const byte).cast(), 1);
        }
    }
}

/// The flag a long-running command watches, with the handlers installed (once per process).
///
/// A thread blocked on a pipe turns a signal into the shared flag and rings the process bell
/// (`ts_foundation::wake`), so sleepers wake at once. It never polls: with no signal it is
/// not scheduled at all (it used to look at a static every 50 ms, 20 wake-ups a second in
/// each service). The command code only ever sees a plain `Arc<AtomicBool>`.
pub(crate) fn interrupt_flag() -> Arc<AtomicBool> {
    static FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    FLAG.get_or_init(|| {
        let flag = Arc::new(AtomicBool::new(false));
        if let Some(read_fd) = open_pipe() {
            let bridge = flag.clone();
            // A small stack: the thread only reads one byte and stores a flag.
            let spawned = std::thread::Builder::new().name("signal-flag".into()).stack_size(32 * 1024).spawn(
                move || {
                    let mut byte = 0u8;
                    loop {
                        // SAFETY: `read_fd` is the read end of a pipe this process owns for its whole
                        // life; `byte` is a valid one-byte buffer.
                        let n = unsafe { libc::read(read_fd, (&raw mut byte).cast(), 1) };
                        if n > 0 {
                            bridge.store(true, Ordering::SeqCst);
                            wake::ring();
                        } else if n == 0
                            || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
                        {
                            return;
                        }
                    }
                },
            );
            if spawned.is_ok() {
                install(libc::SIGINT);
                install(libc::SIGTERM);
            }
            // Without the bridge the flag could never rise, and the signals keep their default
            // action (the program stops at once), which is better than one that cannot be stopped.
        }
        flag
    })
    .clone()
}

/// A pipe whose write end is non-blocking and published for the handler; the read end is returned.
fn open_pipe() -> Option<libc::c_int> {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: `fds` is a valid array of two ints for `pipe2` to fill in.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
        return None;
    }
    // The reader blocks: clear O_NONBLOCK on the read end only.
    // SAFETY: `fds[0]` was just opened by `pipe2`; F_GETFL/F_SETFL take and return plain ints.
    unsafe {
        let flags = libc::fcntl(fds[0], libc::F_GETFL);
        if flags < 0 || libc::fcntl(fds[0], libc::F_SETFL, flags & !libc::O_NONBLOCK) < 0 {
            libc::close(fds[0]);
            libc::close(fds[1]);
            return None;
        }
    }
    PIPE_WRITE.store(fds[1], Ordering::Relaxed);
    Some(fds[0])
}

fn install(signal: libc::c_int) {
    // SAFETY: `on_signal` only calls async-signal-safe `write` and has the signature `signal`
    // expects; installing a handler has no other precondition.
    unsafe {
        libc::signal(signal, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
    }
}

/// Whether `fd` is a terminal.
pub(crate) fn is_terminal(fd: libc::c_int) -> bool {
    // SAFETY: `isatty` only inspects the descriptor and sets errno; any integer is allowed.
    unsafe { libc::isatty(fd) == 1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pipe_is_not_a_terminal() {
        let mut fds = [0 as libc::c_int; 2];
        // SAFETY: `fds` is a valid array of two ints for `pipe` to fill in.
        let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
        assert_eq!(rc, 0);
        assert!(!is_terminal(fds[0]));
        assert!(!is_terminal(fds[1]));
        // SAFETY: both descriptors were just opened by `pipe` and are closed once.
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
    }

    #[test]
    fn the_flag_rises_when_a_signal_arrives() {
        let flag = interrupt_flag();
        assert!(!flag.load(Ordering::SeqCst));
        // SAFETY: raising a signal in this process whose handler is installed above.
        unsafe {
            libc::raise(libc::SIGTERM);
        }
        for _ in 0..100 {
            if flag.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        unreachable!("the flag did not rise");
    }
}
