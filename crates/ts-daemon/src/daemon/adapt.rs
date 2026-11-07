//! adapt: the ports of [`super::deps`] implemented for the concrete domain and system types.
//!
//! These are one-line forwarders that turn each crate's own error enum into a [`PortError`]
//! carrying the same message. They live here, not in the assembly layer, so that putting the
//! daemon together is only a matter of building the concrete values (the sysfs tree, the
//! command runner, the directories) and handing them over.
//!
//! The one adapter with behaviour of its own is [`SystemSensors`], which owns the thread that
//! feeds the GPU sampler and starts it only when asked.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use ts_decision::gamerules::{self, Process};
use ts_decision::thermal::CapWriter;
use ts_domain::gpuundervolt::{Applied as GpuApplied, GpuUndervolt, State as GpuState};
use ts_domain::keyboard::Keyboard;
use ts_domain::looks::{Look, LookStore};
use ts_domain::power::{Applied as PowerApplied, Info as PowerInfo, PowerLimits, RawLimits};
use ts_domain::powermgr::{PowerManagers, RestoreReport, TakeoverReport, UnitStatus};
use ts_domain::undervolt::{Offsets, Undervolt};
use ts_platform::hw::{LinuxSensors, Sleeper as FnSleeper};
use ts_system::osd::Osd;
use ts_system::sensors::{GpuReading, LineSource, Sampler};
use ts_system::touchpad::Touchpad;

use super::deps::{
    BootLookPort, ClockCap, CpuUndervoltPort, GpuUndervoltPort, Notice, OperatingLight, PortError,
    PortResult, PowerLimitsPort, PowerManagersPort, ProcessList, Sensors, StopToken, TouchpadPort,
};

impl ClockCap for CapWriter {
    fn apply(&self, khz: i64) -> Vec<String> {
        CapWriter::apply(self, khz).iter().map(ToString::to_string).collect()
    }

    fn uncap(&self) -> Vec<String> {
        CapWriter::uncap(self).iter().map(ToString::to_string).collect()
    }
}

impl PowerLimitsPort for PowerLimits {
    fn info(&self) -> PowerInfo {
        PowerLimits::info(self)
    }

    fn read_raw(&self) -> PortResult<RawLimits> {
        PowerLimits::read_raw(self).map_err(|e| PortError::of(&e))
    }

    fn write_raw(&self, limits: RawLimits) -> PortResult<()> {
        PowerLimits::write_raw(self, limits).map_err(|e| PortError::of(&e))
    }

    fn set_limits(&self, pl1: f64, pl2: f64) -> PortResult<PowerApplied> {
        PowerLimits::set_limits(self, pl1, pl2).map_err(|e| PortError::of(&e))
    }
}

impl CpuUndervoltPort for Undervolt {
    fn read(&self) -> PortResult<Offsets> {
        Undervolt::read(self).map_err(|e| PortError::of(&e))
    }

    fn apply(&self, core_mv: i32, cache_mv: i32, confirm: bool) -> PortResult<Offsets> {
        Undervolt::apply(self, core_mv, cache_mv, confirm).map_err(|e| PortError::of(&e))
    }

    fn reset(&self) -> PortResult<Offsets> {
        Undervolt::reset(self).map_err(|e| PortError::of(&e))
    }
}

impl GpuUndervoltPort for GpuUndervolt {
    fn read(&self) -> PortResult<GpuState> {
        GpuUndervolt::read(self).map_err(|e| PortError::of(&e))
    }

    fn apply(&self, offset_mhz: i32, cap_mhz: i32) -> PortResult<GpuApplied> {
        GpuUndervolt::apply(self, offset_mhz, cap_mhz).map_err(|e| PortError::of(&e))
    }

    fn reset(&self) -> PortResult<GpuApplied> {
        GpuUndervolt::reset(self).map_err(|e| PortError::of(&e))
    }
}

impl PowerManagersPort for PowerManagers {
    fn status(&self) -> Vec<UnitStatus> {
        PowerManagers::status(self)
    }

    fn takeover(&self) -> PortResult<TakeoverReport> {
        PowerManagers::takeover(self).map_err(|e| PortError::of(&e))
    }

    fn restore(&self) -> RestoreReport {
        PowerManagers::restore(self)
    }

    fn recorded(&self) -> bool {
        PowerManagers::recorded(self)
    }
}

impl BootLookPort for LookStore {
    fn read_boot(&self) -> Option<Look> {
        LookStore::read_boot(self)
    }

    fn write_boot(&self, look: Option<&Look>) -> PortResult<()> {
        LookStore::write_boot(self, look).map_err(|e| PortError::of(&e))
    }

    fn apply(&self, look: &Look) -> PortResult<Vec<String>> {
        LookStore::apply(self, look).map_err(|e| PortError::of(&e))
    }
}

impl OperatingLight for Keyboard {
    fn follow_mode(&self, mode: &str, colors: &BTreeMap<String, String>) -> PortResult<()> {
        Keyboard::follow_mode(self, mode, colors).map_err(|e| PortError::of(&e))
    }
}

impl TouchpadPort for Touchpad {
    fn enabled(&self) -> Option<bool> {
        Touchpad::enabled(self)
    }

    fn set_enabled(&self, on: bool) -> PortResult<bool> {
        Touchpad::set_enabled(self, on).map_err(|e| PortError::of(&e))
    }
}

impl Notice for Osd {
    fn show(&self, text: &str, icon: &str) {
        let _ = Osd::show(self, text, icon);
    }
}

/// The running programs, read from a `/proc` tree.
#[derive(Debug, Clone)]
pub struct ProcFs {
    root: PathBuf,
}

impl ProcFs {
    /// The processes listed below `root` (`/proc` in production, a fake tree in tests).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The real `/proc`.
    pub fn system() -> Self {
        Self::new(gamerules::PROC_DIR)
    }
}

impl ProcessList for ProcFs {
    fn list(&self) -> Vec<Process> {
        gamerules::list_processes(&self.root)
    }
}

/// The machine's sensors: `/proc/stat` and the battery through [`Sampler`], the CPU package
/// temperature and the charger through [`LinuxSensors`], and the GPU readings of a background
/// `nvidia-smi` stream that only starts when the daemon asks for it.
pub struct SystemSensors {
    sampler: Arc<Sampler>,
    linux: LinuxSensors,
    source: Arc<dyn LineSource>,
    sleeper: FnSleeper,
    stop: StopToken,
    started: AtomicBool,
}

impl SystemSensors {
    /// Sensors over `sampler` and `linux`; the GPU stream reads lines from `source`, waits
    /// between restarts with `sleeper`, and ends when `stop` is requested.
    pub fn new(
        sampler: Arc<Sampler>,
        linux: LinuxSensors,
        source: Arc<dyn LineSource>,
        sleeper: FnSleeper,
        stop: StopToken,
    ) -> Self {
        Self { sampler, linux, source, sleeper, stop, started: AtomicBool::new(false) }
    }
}

impl Sensors for SystemSensors {
    fn cpu_load(&self) -> f64 {
        self.sampler.cpu_load()
    }

    fn package_temp(&self) -> Option<f64> {
        self.linux.cpu_temp()
    }

    fn battery_pct(&self) -> f64 {
        self.sampler.battery_pct()
    }

    fn gpu(&self) -> GpuReading {
        self.sampler.gpu()
    }

    fn on_ac(&self) -> bool {
        self.linux.on_ac()
    }

    fn ensure_gpu_stream(&self) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let sampler = Arc::clone(&self.sampler);
        let source = Arc::clone(&self.source);
        let sleeper = Arc::clone(&self.sleeper);
        let stop = self.stop.clone();
        let spawned = thread::Builder::new()
            .name("gpu-sampler".to_owned())
            .spawn(move || sampler.run_gpu_stream(source.as_ref(), &|| stop.is_requested(), &sleeper));
        if spawned.is_err() {
            // The next smart tick may try again.
            self.started.store(false, Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::io;
    use std::sync::Mutex;
    use std::sync::mpsc;
    use std::time::Duration;

    /// A line source that reports each start on a channel and feeds one reading.
    struct Source(Mutex<mpsc::Sender<()>>);

    impl LineSource for Source {
        fn stream(&self, _p: &str, _a: &[&str], on_line: &mut dyn FnMut(&str) -> bool) -> io::Result<()> {
            let _ = self.0.lock().unwrap().send(());
            on_line("70, 40, 55.5");
            Ok(())
        }
    }

    #[test]
    fn the_gpu_stream_starts_once_and_only_when_asked() {
        let (tx, rx) = mpsc::channel();
        let stop = StopToken::new();
        let stopper = stop.clone();
        let sampler = Arc::new(Sampler::new("/nonexistent-root", Arc::new(|| 100.0)));
        let sensors = SystemSensors::new(
            Arc::clone(&sampler),
            LinuxSensors::new("/nonexistent-root", Arc::new(ts_platform::hw::ProcessRunner)),
            Arc::new(Source(Mutex::new(tx))),
            // The wait between two stream starts ends the test's stream instead of sleeping.
            Arc::new(move |_: Duration| stopper.request()),
            stop,
        );
        assert!(rx.try_recv().is_err(), "nothing runs before the daemon asks");
        assert_eq!(sensors.gpu().temp, None);
        sensors.ensure_gpu_stream();
        sensors.ensure_gpu_stream();
        rx.recv_timeout(Duration::from_secs(5)).expect("the stream started");
        // After the first start the thread ends (the sleeper requested the stop); a second
        // start would show up here as a second message.
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err(), "idempotent");
        let g = sensors.gpu();
        assert_eq!(g.temp, Some(70.0));
        assert!(g.util > 0.0 && g.power > 0.0);
        assert_eq!(g.seen, Some(100.0));
    }

    use crate::daemon::testkit::TempDir;
    use std::fs;

    #[test]
    fn the_cpu_clock_cap_is_written_and_lifted_through_the_port() {
        let dir = TempDir::new();
        for cpu in ["cpu0", "cpu1"] {
            let policy = dir.join(&format!("{cpu}/cpufreq"));
            fs::create_dir_all(&policy).unwrap();
            fs::write(policy.join("scaling_max_freq"), "5000000").unwrap();
            fs::write(policy.join("cpuinfo_max_freq"), "5100000").unwrap();
        }
        let cap: Box<dyn ClockCap> = Box::new(CapWriter::new(&dir.0));
        assert!(cap.apply(3_000_000).is_empty());
        for cpu in ["cpu0", "cpu1"] {
            assert_eq!(
                fs::read_to_string(dir.join(&format!("{cpu}/cpufreq/scaling_max_freq"))).unwrap(),
                "3000000"
            );
        }
        assert!(cap.uncap().is_empty());
        assert_eq!(
            fs::read_to_string(dir.join("cpu1/cpufreq/scaling_max_freq")).unwrap(),
            "5100000",
            "its own maximum"
        );
    }

    #[test]
    fn the_process_list_reads_a_proc_tree() {
        let dir = TempDir::new();
        fs::create_dir_all(dir.join("4242")).unwrap();
        fs::write(dir.join("4242/comm"), "hl2_linux\n").unwrap();
        fs::write(dir.join("4242/cmdline"), b"/home/u/hl2_linux\0-game\0").unwrap();
        let procs: Box<dyn ProcessList> = Box::new(ProcFs::new(&dir.0));
        let list = procs.list();
        assert_eq!(list.len(), 1);
        assert!(list[0].names().contains("hl2_linux"));
        assert!(ProcFs::new(dir.join("missing")).list().is_empty());
    }

    #[test]
    fn the_touchpad_port_reads_and_switches_the_kernel_file() {
        let dir = TempDir::new();
        let devices = dir.join("devices");
        fs::write(&devices, "N: Name=\"SYNA2BA6:00 06CB:CD7E Touchpad\"\nH: Handlers=mouse1 event13 \n")
            .unwrap();
        fs::create_dir_all(dir.join("input/event13/device")).unwrap();
        fs::write(dir.join("input/event13/device/inhibited"), "0").unwrap();
        let pad: Box<dyn TouchpadPort> = Box::new(Touchpad::new(&devices, dir.join("input")));
        assert_eq!(pad.enabled(), Some(true));
        assert_eq!(pad.set_enabled(false), Ok(false));
        assert_eq!(pad.enabled(), Some(false));
        let none = Touchpad::new(dir.join("nothing"), dir.join("input"));
        assert_eq!(TouchpadPort::enabled(&none), None);
        assert!(TouchpadPort::set_enabled(&none, true).unwrap_err().message().contains("touchpad not found"));
    }

    #[test]
    fn a_failure_keeps_the_message_of_the_crate_it_came_from() {
        let e = PortError::of(&"the CPU kept its stock voltage");
        assert_eq!(e.message(), "the CPU kept its stock voltage");
        assert_eq!(e.to_string(), "the CPU kept its stock voltage");
    }
}
