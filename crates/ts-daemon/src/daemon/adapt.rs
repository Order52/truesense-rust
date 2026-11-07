//! adapt: the ports of [`super::deps`] implemented for the concrete domain and system types.
//!
//! These are one-line forwarders that turn each crate's own error enum into a [`PortError`]
//! carrying the same message. They live here, not in the assembly layer, so that putting the
//! daemon together is only a matter of building the concrete values (the sysfs tree, the
//! command runner, the directories) and handing them over.
//!
//! The one adapter with behaviour of its own is [`SystemSensors`], which owns the thread that
//! feeds the GPU sampler: it starts only when asked, ends when it is released, and the sampler's
//! own policy keeps it away from a sleeping card.

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
use ts_system::sensors::{GpuFeed, GpuReading, GpuStreamEnv, LineSource, Sampler};
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
pub(crate) struct ProcFs {
    root: PathBuf,
}

impl ProcFs {
    /// The processes listed below `root` (`/proc` in production, a fake tree in tests).
    pub(crate) fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The real `/proc`.
    pub(crate) fn system() -> Self {
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
/// `nvidia-smi` stream that only runs while the daemon asks for it.
pub(crate) struct SystemSensors {
    sampler: Arc<Sampler>,
    linux: LinuxSensors,
    source: Arc<dyn LineSource>,
    sleeper: FnSleeper,
    stop: StopToken,
    /// A smart feature wants the readings now.
    wanted: Arc<AtomicBool>,
    /// The stream thread is alive.
    running: Arc<AtomicBool>,
}

impl SystemSensors {
    /// Sensors over `sampler` and `linux`; the GPU stream reads lines from `source`, waits
    /// between restarts with `sleeper`, and ends when `stop` is requested.
    pub(crate) fn new(
        sampler: Arc<Sampler>,
        linux: LinuxSensors,
        source: Arc<dyn LineSource>,
        sleeper: FnSleeper,
        stop: StopToken,
    ) -> Self {
        Self {
            sampler,
            linux,
            source,
            sleeper,
            stop,
            wanted: Arc::new(AtomicBool::new(false)),
            running: Arc::new(AtomicBool::new(false)),
        }
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
        self.wanted.store(true, Ordering::SeqCst);
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let sampler = Arc::clone(&self.sampler);
        let source = Arc::clone(&self.source);
        let sleeper = Arc::clone(&self.sleeper);
        let stop = self.stop.clone();
        let wanted = Arc::clone(&self.wanted);
        let running = Arc::clone(&self.running);
        let spawned = thread::Builder::new().name("gpu-sampler".to_owned()).spawn(move || {
            loop {
                sampler.run_gpu_stream(
                    source.as_ref(),
                    &GpuStreamEnv {
                        stop: &|| stop.is_requested(),
                        wanted: &|| wanted.load(Ordering::SeqCst),
                        sleeper: &sleeper,
                    },
                );
                running.store(false, Ordering::SeqCst);
                // Asked for again between the end of the stream and the line above: carry on
                // (unless an `ensure` already started a new thread for it).
                if stop.is_requested()
                    || !wanted.load(Ordering::SeqCst)
                    || running.swap(true, Ordering::SeqCst)
                {
                    break;
                }
            }
        });
        if spawned.is_err() {
            // The next smart tick may try again.
            self.running.store(false, Ordering::SeqCst);
        }
    }

    fn release_gpu_stream(&self) {
        self.wanted.store(false, Ordering::SeqCst);
    }

    fn gpu_state(&self) -> GpuFeed {
        self.sampler.gpu_feed()
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

    /// A line source that keeps feeding lines until it is told to end, like `nvidia-smi -l 2`.
    struct Endless(Mutex<mpsc::Sender<()>>);

    impl LineSource for Endless {
        fn stream(&self, _p: &str, _a: &[&str], on_line: &mut dyn FnMut(&str) -> bool) -> io::Result<()> {
            let _ = self.0.lock().unwrap().send(());
            while on_line("70, 40, 55.5") {
                thread::sleep(Duration::from_millis(1));
            }
            Ok(())
        }
    }

    /// A machine with one NVIDIA display card in the given runtime power state.
    fn machine(status: &str) -> TempDir {
        let dir = TempDir::new();
        let card = dir.join("sys/bus/pci/devices/0000:01:00.0");
        fs::create_dir_all(card.join("power")).unwrap();
        fs::write(card.join("vendor"), "0x10de\n").unwrap();
        fs::write(card.join("class"), "0x030200\n").unwrap();
        fs::write(card.join("power/runtime_status"), format!("{status}\n")).unwrap();
        dir
    }

    fn sensors_over(
        dir: &TempDir,
        source: Arc<dyn LineSource>,
        sleeper: FnSleeper,
        stop: StopToken,
    ) -> SystemSensors {
        let sampler = Arc::new(Sampler::new(&dir.0, Arc::new(|| 100.0)));
        SystemSensors::new(
            sampler,
            LinuxSensors::new(&dir.0, Arc::new(ts_platform::hw::ProcessRunner)),
            source,
            sleeper,
            stop,
        )
    }

    fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        for _ in 0..5000 {
            if done() {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("timed out waiting for {what}");
    }

    #[test]
    fn the_gpu_stream_starts_once_and_only_when_asked() {
        let (tx, rx) = mpsc::channel();
        let stop = StopToken::new();
        let stopper = stop.clone();
        let dir = machine("active");
        let sensors = sensors_over(
            &dir,
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
        wait_until("the thread to end", || !sensors.running.load(Ordering::SeqCst));
    }

    #[test]
    fn a_sleeping_card_is_not_woken_to_read_it() {
        let (tx, rx) = mpsc::channel();
        let stop = StopToken::new();
        let dir = machine("suspended");
        let sensors = sensors_over(
            &dir,
            Arc::new(Source(Mutex::new(tx))),
            Arc::new(|_: Duration| thread::sleep(Duration::from_millis(1))),
            stop.clone(),
        );
        sensors.ensure_gpu_stream();
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err(), "no nvidia-smi for a sleeping card");
        assert_eq!(sensors.gpu_state(), GpuFeed::Suspended);
        stop.request();
        wait_until("the thread to end", || !sensors.running.load(Ordering::SeqCst));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn releasing_the_request_ends_the_stream_and_asking_again_restarts_it() {
        let (tx, rx) = mpsc::channel();
        let stop = StopToken::new();
        let dir = machine("active");
        let sensors = sensors_over(
            &dir,
            Arc::new(Endless(Mutex::new(tx))),
            Arc::new(|_: Duration| thread::sleep(Duration::from_millis(1))),
            stop.clone(),
        );
        sensors.ensure_gpu_stream();
        rx.recv_timeout(Duration::from_secs(5)).expect("started");
        wait_until("a reading", || sensors.gpu().temp == Some(70.0));
        sensors.release_gpu_stream();
        wait_until("the thread to end", || !sensors.running.load(Ordering::SeqCst));
        assert_eq!(sensors.gpu().temp, None, "a released stream leaves no stale reading behind");
        assert!(rx.try_recv().is_err(), "and no second start");
        sensors.ensure_gpu_stream();
        rx.recv_timeout(Duration::from_secs(5)).expect("restarted on demand");
        stop.request();
        wait_until("the thread to end", || !sensors.running.load(Ordering::SeqCst));
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
