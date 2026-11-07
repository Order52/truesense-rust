//! Ctrl-C and SIGTERM for the foreground commands: a flag that becomes true when one arrives.
#![allow(unsafe_code)]
//!
//! The daemon does not use this (it blocks the signals and waits for them itself, see
//! `ts_daemon::assembly`); neither does the terminal UI, which sees Ctrl-C as a key.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// Set by the signal handler: the only thing a handler may safely do is store to an atomic.
static RECEIVED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_signal: libc::c_int) {
    RECEIVED.store(true, Ordering::SeqCst);
}

/// The flag a long-running command watches, with the handlers installed (once per process).
/// A small thread turns the handler's static into the shared flag, so the command code only
/// ever sees a plain `Arc<AtomicBool>`.
pub fn interrupt_flag() -> Arc<AtomicBool> {
    static FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    FLAG.get_or_init(|| {
        let flag = Arc::new(AtomicBool::new(false));
        install(libc::SIGINT);
        install(libc::SIGTERM);
        let bridge = flag.clone();
        let spawned = std::thread::Builder::new().name("signal-flag".into()).spawn(move || {
            loop {
                if RECEIVED.load(Ordering::SeqCst) {
                    bridge.store(true, Ordering::SeqCst);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        // Without the bridge the flag never rises; the default action of the signal is lost too,
        // so fall back to the default action rather than a program that cannot be stopped.
        if spawned.is_err() {
            reset(libc::SIGINT);
            reset(libc::SIGTERM);
        }
        flag
    })
    .clone()
}

fn install(signal: libc::c_int) {
    // SAFETY: `on_signal` only performs an atomic store, which is async-signal-safe, and has the
    // signature `signal` expects; installing a handler has no other precondition.
    unsafe {
        libc::signal(signal, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
    }
}

fn reset(signal: libc::c_int) {
    // SAFETY: restoring the default disposition of a signal has no preconditions.
    unsafe {
        libc::signal(signal, libc::SIG_DFL);
    }
}

/// Whether `fd` is a terminal.
pub fn is_terminal(fd: libc::c_int) -> bool {
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
            std::thread::sleep(Duration::from_millis(20));
        }
        unreachable!("the flag did not rise");
    }
}
