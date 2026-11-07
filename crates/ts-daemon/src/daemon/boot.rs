//! The boot sequence and the graceful shutdown.
//!
//! At start the daemon restores what was saved: the power limits the user chose, the CPU and GPU
//! undervolts the user asked to have applied at boot (behind their crash guards: a flag file
//! that exists from applying until ten minutes of uptime, so an undervolt that freezes the
//! machine costs one crash and never a boot loop), and the lighting chosen for boot. The saved
//! fan curve, Smart auto profile, held power-mode pick and the fan and auto settings are read by
//! [`Daemon::new`] and the loop; nothing is written to the fans or the power mode at boot (the
//! firmware starts in its own control and the loop decides from there, as in the Python daemon).
//!
//! On the way out, [`Daemon::shutdown`] hands the fans back to the firmware and lifts the clock
//! cap. It does the fans first and without the state lock, so a request that is stuck cannot
//! keep them from going back.

use std::sync::atomic::Ordering;

use serde_json::Value;
use ts_decision::crashguard::Start;
use ts_domain::gpuundervolt::Profile as GpuProfile;
use ts_domain::undervolt::Profile as CpuProfile;
use ts_foundation::fsx;

use super::daemon::Daemon;

impl Daemon {
    /// Applies the saved settings. The control socket is already up (clients may connect while
    /// this runs); `READY=1` follows.
    pub(super) fn boot(&self) {
        if let Some(dir) = self.live_paths.run_state.parent().filter(|d| !d.as_os_str().is_empty()) {
            if let Err(e) = fsx::ensure_dir(dir, 0o755) {
                self.warn(format!("could not create the run directory: {e}"));
            }
        }
        // Offsets never survive a reboot, so nothing is applied now.
        self.save("uv_applied", Value::Null);
        self.save("gpu_uv_applied", Value::Null);
        {
            // What the firmware set is captured before anything of ours can write the limits;
            // the user's saved limits are then back in force before the first tick.
            let mut g = self.lock();
            self.capture_power_limits(&mut g);
            if self.user_limits().is_some() {
                self.enforce_power_limit(&mut g, false);
            }
        }
        self.apply_boot_look("boot");
        self.apply_uv_boot();
        self.apply_gpu_uv_boot();
    }

    /// The opt-in CPU undervolt at boot, behind its crash guard: if the flag is still there the
    /// previous boot did not survive, so the undervolt is switched off instead of applied again.
    fn apply_uv_boot(&self) {
        let Some(saved) = self.state.get("uv_boot").filter(|v| !is_empty(v)) else { return };
        let profile = match CpuProfile::from_value(&saved) {
            Ok(p) => p,
            Err(e) => {
                self.warn(format!("undervolt at boot not applied: the saved setting is invalid ({e})"));
                return;
            }
        };
        let mut g = self.lock();
        match g.uv_guard.start(self.clock.mono(), self.clock.wall()) {
            Ok(Start::Tripped) => {
                self.save("uv_boot", Value::Null);
                self.warn("undervolt at boot disabled: the previous boot did not stay up for 10 minutes after applying it");
            }
            Err(e) => {
                // Without the flag there would be no protection against a boot loop.
                self.warn(format!(
                    "undervolt at boot not applied: no crash-guard flag could be written ({e})"
                ));
            }
            Ok(Start::Armed) => match self.cpu_undervolt.apply(profile.core, profile.cache, true) {
                Ok(_) => self
                    .info(format!("undervolt at boot: core {} mV, cache {} mV", profile.core, profile.cache)),
                Err(e) => {
                    self.warn(format!("undervolt at boot failed: {e}"));
                    g.uv_guard.disarm();
                }
            },
        }
    }

    /// The same guard for the GPU undervolt.
    fn apply_gpu_uv_boot(&self) {
        let Some(saved) = self.state.get("gpu_uv_boot").filter(|v| !is_empty(v)) else { return };
        let profile = match GpuProfile::from_value(&saved) {
            Ok(p) => p,
            Err(e) => {
                self.warn(format!("GPU undervolt at boot not applied: the saved setting is invalid ({e})"));
                return;
            }
        };
        let mut g = self.lock();
        match g.gpu_uv_guard.start(self.clock.mono(), self.clock.wall()) {
            Ok(Start::Tripped) => {
                self.save("gpu_uv_boot", Value::Null);
                self.warn("GPU undervolt at boot disabled: the previous boot did not stay up for 10 minutes after applying it");
            }
            Err(e) => {
                self.warn(format!(
                    "GPU undervolt at boot not applied: no crash-guard flag could be written ({e})"
                ));
            }
            Ok(Start::Armed) => match self.gpu_undervolt.apply(profile.offset, profile.cap) {
                Ok(_) => self.info(format!(
                    "GPU undervolt at boot: offset {} MHz, cap {} MHz",
                    profile.offset, profile.cap
                )),
                Err(e) => {
                    self.warn(format!("GPU undervolt at boot failed: {e}"));
                    g.gpu_uv_guard.disarm();
                }
            },
        }
    }

    /// Hands the machine back: the fans to the firmware, the CPU clock cap lifted, `STOPPING=1`.
    /// Safe to call twice (the second call repeats the hand-back for anything a request in flight
    /// changed meanwhile, and sends no second `STOPPING=1`).
    ///
    /// The fans go first and without the state lock: a stuck request must not be able to keep
    /// them from going back.
    pub fn shutdown(&self) {
        if let Err(e) = self.hw.fan_auto() {
            self.warn(format!("could not hand the fans back to the firmware: {e}"));
        }
        self.stop.request();
        self.lock().governor.release();
        for failure in self.cpu_cap.uncap() {
            self.warn(format!("could not lift the CPU clock cap: {failure}"));
        }
        if !self.stopping_sent.swap(true, Ordering::SeqCst) {
            if let Err(e) = self.notifier.stopping() {
                self.warn(format!("sd_notify STOPPING failed: {e}"));
            }
        }
    }
}

/// Python's truthiness for a stored setting: `null`, `{}` and the like mean "not set".
fn is_empty(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Object(o) => o.is_empty(),
        Value::Bool(b) => !b,
        _ => false,
    }
}
