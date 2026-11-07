//! The crash guard for settings applied at boot (the CPU and GPU undervolts).
//!
//! An undervolt that is too aggressive freezes this laptop within minutes. A setting applied at
//! every boot would then freeze it at every boot. So a flag file exists from the moment the
//! setting is applied until the machine has stayed up for ten minutes. If the flag is still
//! there at the next start, the previous boot did not survive: the setting is dropped (the
//! caller forgets it) instead of being applied again. A bad value costs one crash, never a boot
//! loop.
//!
//! [`Guard`] is the little state machine around one flag file. The path is injected (the CPU
//! and the GPU each have their own flag), and the stability timer runs on a monotonic clock
//! value passed in, so tests need neither a real file in `/var/lib` nor ten real minutes.
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use ts_foundation::fsx::{self, FsError};

/// How long the machine must stay up after applying before the setting counts as stable.
pub(crate) const STABLE_AFTER_S: f64 = 600.0;

/// Where the CPU undervolt flag lives unless `TRUESENSE_UV_PENDING` says otherwise.
pub(crate) const UV_PENDING_PATH: &str = "/var/lib/truesense/uv-pending";
/// Where the GPU undervolt flag lives unless `TRUESENSE_GPU_UV_PENDING` says otherwise.
pub(crate) const GPU_UV_PENDING_PATH: &str = "/var/lib/truesense/gpu-uv-pending";

/// The CPU flag path: `$TRUESENSE_UV_PENDING` or `UV_PENDING_PATH`.
pub fn uv_pending_path(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    env("TRUESENSE_UV_PENDING").map_or_else(|| PathBuf::from(UV_PENDING_PATH), PathBuf::from)
}

/// The GPU flag path: `$TRUESENSE_GPU_UV_PENDING` or `GPU_UV_PENDING_PATH`.
pub fn gpu_uv_pending_path(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    env("TRUESENSE_GPU_UV_PENDING").map_or_else(|| PathBuf::from(GPU_UV_PENDING_PATH), PathBuf::from)
}

/// What starting a boot-time setting found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    /// The flag survived from the previous boot, so that boot crashed: it has been removed
    /// (the boot after this one is a normal one) and the caller must forget the setting
    /// instead of applying it.
    Tripped,
    /// The flag is written: apply the setting now. If applying fails, call [`Guard::disarm`].
    Armed,
}

/// One flag file.
#[derive(Debug, Clone)]
pub struct Guard {
    path: PathBuf,
    armed_at: Option<f64>,
}

impl Guard {
    /// A guard for the flag at `path`.
    pub fn new(path: impl Into<PathBuf>) -> Guard {
        Guard { path: path.into(), armed_at: None }
    }

    /// The flag file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the flag is waiting to be proven stable.
    pub fn armed(&self) -> bool {
        self.armed_at.is_some()
    }

    /// Whether a flag file exists right now (the `pending` field of the undervolt reply).
    pub fn pending(&self) -> bool {
        fsx::exists(&self.path)
    }

    /// Begins a boot-time apply. `now` is a monotonic clock in seconds and `wall` the unix time
    /// written into the flag (for whoever reads it after a crash).
    ///
    /// A flag already there means the last boot never proved stable: it is consumed and
    /// [`Start::Tripped`] returned. Otherwise the flag is written first and [`Start::Armed`]
    /// returned. When the flag cannot be written the error is returned and the setting must
    /// **not** be applied: without a flag there would be no protection against a boot loop.
    pub fn start(&mut self, now: f64, wall: f64) -> Result<Start, FsError> {
        if fsx::exists(&self.path) {
            // A failure to remove it is not worth stopping for: the caller forgets the setting,
            // so a flag that stays only costs one more skipped boot-time apply.
            let _ = std::fs::remove_file(&self.path);
            self.armed_at = None;
            return Ok(Start::Tripped);
        }
        fsx::write_atomic_mkdir(&self.path, format!("{wall}").as_bytes(), 0o644)?;
        self.armed_at = Some(now);
        Ok(Start::Armed)
    }

    /// Removes the flag: the setting failed to apply (nothing to protect against), or it
    /// proved stable. Harmless when there is no flag.
    pub fn disarm(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        self.armed_at = None;
    }

    /// Call it from the main loop. Once the machine has been up for more than
    /// `STABLE_AFTER_S` since the setting was applied, the flag is removed; returns true on
    /// that tick.
    pub fn tick(&mut self, now: f64) -> bool {
        match self.armed_at {
            Some(t) if now - t > STABLE_AFTER_S => {
                self.disarm();
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::smartprofile::testdir::TempDir;

    #[test]
    fn a_normal_boot_arms_then_proves_stable_after_ten_minutes() {
        let dir = TempDir::new("cg-normal");
        let p = dir.path().join("sub/uv-pending");
        let mut g = Guard::new(&p);
        assert!(!g.pending());
        assert_eq!(g.start(100.0, 1_700_000_000.0).expect("start"), Start::Armed);
        assert!(p.exists() && g.armed() && g.pending());
        assert_eq!(
            std::fs::read_to_string(&p).expect("flag"),
            "1700000000",
            "the flag holds the time it was set"
        );
        assert!(!g.tick(100.0 + 540.0), "nine minutes: not stable yet");
        assert!(p.exists());
        assert!(!g.tick(100.0 + 600.0), "exactly ten minutes is not more than ten");
        assert!(g.tick(100.0 + 601.0), "after ten minutes the flag goes");
        assert!(!p.exists() && !g.armed());
        assert!(!g.tick(100.0 + 9999.0), "and it is only announced once");
    }

    #[test]
    fn a_crash_leaves_the_flag_and_the_next_start_drops_the_setting() {
        let dir = TempDir::new("cg-crash");
        let p = dir.path().join("uv-pending");
        // the previous boot armed it and never got to remove it
        assert_eq!(Guard::new(&p).start(0.0, 1.0).expect("arm"), Start::Armed);
        let mut g = Guard::new(&p);
        assert_eq!(
            g.start(0.0, 2.0).expect("start"),
            Start::Tripped,
            "the surviving flag means the last boot crashed"
        );
        assert!(!p.exists(), "tripping consumes the flag so the boot after that is normal");
        assert!(!g.armed(), "nothing is armed, nothing is applied");
        assert_eq!(
            g.start(0.0, 3.0).expect("start"),
            Start::Armed,
            "the boot after the crash is a normal one"
        );
    }

    #[test]
    fn a_failed_apply_disarms() {
        let dir = TempDir::new("cg-fail");
        let p = dir.path().join("uv-pending");
        let mut g = Guard::new(&p);
        g.start(0.0, 1.0).expect("arm");
        g.disarm();
        assert!(!p.exists() && !g.armed(), "a setting that failed to apply must not leave the flag");
        g.disarm(); // idempotent
        assert!(!g.tick(10_000.0), "a disarmed guard never fires");
    }

    #[test]
    fn an_unwritable_flag_refuses_to_arm() {
        let mut g = Guard::new("/proc/nonexistent/uv-pending");
        assert!(
            g.start(0.0, 1.0).is_err(),
            "without a flag there is no protection: start must fail so the caller does not apply"
        );
        assert!(!g.armed());
    }

    #[test]
    fn the_cpu_and_gpu_guards_are_independent() {
        let dir = TempDir::new("cg-two");
        let mut cpu = Guard::new(dir.path().join("uv-pending"));
        let mut gpu = Guard::new(dir.path().join("gpu-uv-pending"));
        cpu.start(0.0, 1.0).expect("arm");
        assert_eq!(
            gpu.start(0.0, 1.0).expect("arm"),
            Start::Armed,
            "the CPU flag does not trip the GPU guard"
        );
        assert!(cpu.tick(601.0));
        assert!(gpu.pending() && !cpu.pending());
    }

    #[test]
    fn a_restart_forgets_the_timer_but_keeps_the_flag() {
        // A daemon restart (not a reboot) re-reads the settings and finds the flag: it trips. That
        // is deliberate in the Python edition too, and the safe direction: the setting is dropped.
        let dir = TempDir::new("cg-restart");
        let p = dir.path().join("uv-pending");
        Guard::new(&p).start(0.0, 1.0).expect("arm");
        assert_eq!(Guard::new(&p).start(5.0, 2.0).expect("start"), Start::Tripped);
    }

    #[test]
    fn paths_follow_the_environment() {
        let none = |_: &str| None;
        assert_eq!(uv_pending_path(&none), PathBuf::from("/var/lib/truesense/uv-pending"));
        assert_eq!(gpu_uv_pending_path(&none), PathBuf::from("/var/lib/truesense/gpu-uv-pending"));
        let env = |k: &str| match k {
            "TRUESENSE_UV_PENDING" => Some("/x/uv".to_owned()),
            "TRUESENSE_GPU_UV_PENDING" => Some("/x/gpu".to_owned()),
            _ => None,
        };
        assert_eq!(uv_pending_path(&env), PathBuf::from("/x/uv"));
        assert_eq!(gpu_uv_pending_path(&env), PathBuf::from("/x/gpu"));
        assert_eq!(Guard::new("/x/uv").path(), Path::new("/x/uv"));
    }
}
