//! Locking that survives a panic elsewhere.
//!
//! A panic never crosses the daemon's request or tick boundary (it is caught there), so a mutex
//! that a panicking thread poisoned still holds consistent data: the guarded values are plain
//! state that every writer leaves valid between statements. Refusing the lock would turn one
//! caught panic into a permanent outage of the fan control, so every lock in the workspace takes
//! the data back instead. This is the one place that says so.
#![forbid(unsafe_code)]

use std::sync::{Mutex, MutexGuard, PoisonError};

/// Locks `mutex`, taking the data back even if a thread panicked while holding it.
pub fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn a_poisoned_lock_still_gives_the_data() {
        let m = Arc::new(Mutex::new(7));
        let other = Arc::clone(&m);
        let _ = std::thread::spawn(move || {
            let _guard = other.lock().unwrap();
            panic!("poison it");
        })
        .join();
        assert!(m.is_poisoned());
        assert_eq!(*locked(&m), 7);
        *locked(&m) = 8;
        assert_eq!(*locked(&m), 8);
    }
}
