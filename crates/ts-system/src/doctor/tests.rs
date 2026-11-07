#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{HashMap, HashSet};
use std::fs;
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::sync::{Barrier, Mutex, MutexGuard};
use std::time::{Duration, UNIX_EPOCH};

use ts_platform::firmware::{Device, Simulator};
use ts_platform::hw::{Duty, FanMode};

use super::*;
use crate::models::testutil::TempDir;
use crate::services::Output;

const KERNEL: &str = "7.3.0-test";

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap()
}

// ---- fakes ------------------------------------------------------------------------------------

/// Answers programs from a table keyed by the whole command line and records every call. An
/// unknown command behaves like a missing program.
#[derive(Default)]
struct FakeRunner {
    table: Mutex<HashMap<String, String>>,
    calls: Mutex<Vec<String>>,
}

impl FakeRunner {
    fn set(&self, line: &str, out: &str) {
        lock(&self.table).insert(line.to_owned(), out.to_owned());
    }

    fn remove(&self, line: &str) {
        lock(&self.table).remove(line);
    }

    fn calls(&self) -> Vec<String> {
        lock(&self.calls).clone()
    }

    fn called(&self, prefix: &str) -> bool {
        self.calls().iter().any(|c| c.starts_with(prefix))
    }
}

impl Runner for FakeRunner {
    fn run(&self, program: &str, args: &[&str]) -> Output {
        let line = std::iter::once(program).chain(args.iter().copied()).collect::<Vec<_>>().join(" ");
        lock(&self.calls).push(line.clone());
        match lock(&self.table).get(&line) {
            Some(out) => Output::success(format!("{out}\n")),
            None => Output::not_runnable(format!("{program}: not installed")),
        }
    }
}

struct HostState {
    euid: u32,
    env: HashMap<String, String>,
    uids: HashMap<String, u32>,
    programs: HashSet<String>,
    deny_access: bool,
}

struct FakeHost {
    state: Mutex<HostState>,
    now: SystemTime,
}

impl FakeHost {
    fn state(&self) -> MutexGuard<'_, HostState> {
        lock(&self.state)
    }
}

impl Host for FakeHost {
    fn env_var(&self, name: &str) -> Option<String> {
        self.state().env.get(name).cloned()
    }

    fn euid(&self) -> u32 {
        self.state().euid
    }

    fn lookup_uid(&self, user: &str) -> Option<u32> {
        self.state().uids.get(user).copied()
    }

    fn program_on_path(&self, name: &str) -> bool {
        self.state().programs.contains(name)
    }

    fn can_access(&self, _: &Path, _: bool) -> bool {
        !self.state().deny_access
    }

    fn now(&self) -> SystemTime {
        self.now
    }
}

struct FwState {
    caps: Capabilities,
    caps_err: Option<String>,
    fan: FanState,
    fan_err: Option<String>,
}

struct FakeFirmware(Mutex<FwState>);

impl Firmware for FakeFirmware {
    fn capabilities(&self) -> Result<Capabilities, HwError> {
        let s = lock(&self.0);
        s.caps_err.clone().map_or(Ok(s.caps), |e| Err(HwError::Invalid(e)))
    }

    fn fan_state(&self) -> Result<FanState, HwError> {
        let s = lock(&self.0);
        s.fan_err.clone().map_or_else(|| Ok(s.fan.clone()), |e| Err(HwError::Invalid(e)))
    }
}

#[derive(Default)]
struct DaemonFake {
    state: DaemonState,
    state_err: Option<String>,
    read_err: HashMap<String, String>,
    reads: Vec<String>,
}

struct FakeDaemon(Mutex<DaemonFake>);

impl Daemon for FakeDaemon {
    fn state(&self) -> Result<DaemonState, CallError> {
        let d = lock(&self.0);
        d.state_err.clone().map_or_else(|| Ok(d.state.clone()), |e| Err(CallError::new(e)))
    }

    fn read(&self, command: &str) -> Result<(), CallError> {
        let mut d = lock(&self.0);
        d.reads.push(command.to_owned());
        d.read_err.get(command).cloned().map_or(Ok(()), |e| Err(CallError::new(e)))
    }
}

struct AudioFake {
    rnnoise: bool,
    dts: bool,
    jack: Result<Jack, String>,
    sound: Result<SoundState, String>,
    bare: Result<Vec<String>, String>,
    sound_asked: bool,
}

struct FakeAudio(Mutex<AudioFake>);

impl AudioChecks for FakeAudio {
    fn has_rnnoise(&self) -> bool {
        lock(&self.0).rnnoise
    }

    fn has_dts(&self) -> bool {
        lock(&self.0).dts
    }

    fn jack_status(&self) -> Result<Jack, CallError> {
        lock(&self.0).jack.clone().map_err(CallError::new)
    }

    fn sound_state(&self) -> Result<SoundState, CallError> {
        let mut a = lock(&self.0);
        a.sound_asked = true;
        a.sound.clone().map_err(CallError::new)
    }

    fn plain_headsets(&self) -> Result<Vec<String>, CallError> {
        lock(&self.0).bare.clone().map_err(CallError::new)
    }
}

struct ProbesFake {
    node: Option<PathBuf>,
    low: usize,
}

struct FakeProbes(Mutex<ProbesFake>);

impl DomainProbes for FakeProbes {
    fn fn_key_node(&self) -> Option<PathBuf> {
        lock(&self.0).node.clone()
    }

    fn low_frequency_ceilings(&self) -> usize {
        lock(&self.0).low
    }
}

/// A healthy machine in a temporary root; tests break one thing at a time.
struct Rig {
    dir: TempDir,
    runner: Arc<FakeRunner>,
    host: Arc<FakeHost>,
    fw: Arc<FakeFirmware>,
    daemon: Arc<FakeDaemon>,
    audio: Arc<FakeAudio>,
    probes: Arc<FakeProbes>,
    config: Arc<Mutex<Result<Config, String>>>,
    env: Env,
}

fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_800_000_000)
}

fn set_mtime(path: &Path, when: SystemTime) {
    fs::File::options().write(true).open(path).unwrap().set_modified(when).unwrap();
}

fn install_dkms(dir: &TempDir, package: &str, kernels: &[&str]) {
    for k in kernels {
        dir.mkdir(&format!("var/lib/dkms/{package}/0.5.0/{k}/x86_64"));
        std::os::unix::fs::symlink(
            format!("0.5.0/{k}/x86_64"),
            dir.path().join(format!("var/lib/dkms/{package}/kernel-{k}-x86_64")),
        )
        .unwrap();
    }
}

fn install_kernels(dir: &TempDir, kernels: &[&str]) {
    for k in kernels {
        dir.mkdir(&format!("lib/modules/{k}/build"));
    }
}

fn rig() -> Rig {
    let dir = TempDir::new("doctor");
    for (name, value) in [
        ("sys_vendor", "Acer"),
        ("product_name", "Predator PH16-71"),
        ("bios_version", "V1.18"),
        ("bios_date", "06/09/2026"),
    ] {
        dir.write(&format!("sys/class/dmi/id/{name}"), &format!("{value}\n"));
    }
    install_kernels(&dir, &[KERNEL]);
    install_dkms(&dir, "truesense-wmi", &[KERNEL]);
    dir.write("sys/module/truesense_wmi/version", "0.5.0\n");
    dir.write("etc/modprobe.d/truesense.conf", "blacklist acer_wmi\n");
    dir.write("dev/truesense", "");
    dir.write("etc/udev/rules.d/70-truesense.rules", "# rule\n");
    dir.write("sys/devices/system/cpu/intel_pstate/no_turbo", "0\n");
    dir.write("run/truesense/state", "{}");
    set_mtime(&dir.path().join("run/truesense/state"), now() - Duration::from_secs(1));

    let runner = Arc::new(FakeRunner::default());
    for (line, out) in [
        ("modinfo -F version truesense_wmi", "0.5.0"),
        ("modinfo -F signer truesense_wmi", ""),
        ("mokutil --sb-state", "SecureBoot disabled"),
        ("systemctl is-active truesense.service", "active"),
        ("systemctl --user is-active truesense-keys.service", "active"),
        ("systemctl --user is-active truesense-audio.service", "active"),
        ("systemctl --user is-active truesense-startup.service", "active"),
        ("systemctl --user is-active pipewire.service", "active"),
    ] {
        runner.set(line, out);
    }
    let host = Arc::new(FakeHost {
        state: Mutex::new(HostState {
            euid: 1000,
            env: HashMap::new(),
            uids: HashMap::from([("alice".to_owned(), 1234)]),
            programs: HashSet::from(["xdg-open".to_owned(), "truesense-fast".to_owned()]),
            deny_access: false,
        }),
        now: now(),
    });
    let fw = Arc::new(FakeFirmware(Mutex::new(FwState {
        caps: Capabilities { game: true, apge: true, battery: true },
        caps_err: None,
        fan: FanState {
            behavior_raw: "0x41".to_owned(),
            mode: FanMode::Auto,
            duty_register_pct: Duty { cpu: 30, gpu: 30 },
        },
        fan_err: None,
    })));
    let daemon = Arc::new(FakeDaemon(Mutex::new(DaemonFake {
        state: DaemonState {
            live: DaemonLive { hw_ok: None, activity: Some("idle".to_owned()) },
            settings: DaemonSettings { smart_fan: false, auto_profile: true },
        },
        ..DaemonFake::default()
    })));
    let audio = Arc::new(FakeAudio(Mutex::new(AudioFake {
        rnnoise: true,
        dts: true,
        jack: Ok(Jack { headphones: false, headset_mic: false }),
        sound: Ok(SoundState::Healthy),
        bare: Ok(vec![]),
        sound_asked: false,
    })));
    let probes =
        Arc::new(FakeProbes(Mutex::new(ProbesFake { node: Some(PathBuf::from("/dev/hidraw3")), low: 0 })));
    let config: Arc<Mutex<Result<Config, String>>> = Arc::new(Mutex::new(Ok(Config::default())));
    let cfg = Arc::clone(&config);
    let env = Env {
        root: dir.path().to_path_buf(),
        runner: runner.clone(),
        host: host.clone(),
        firmware: fw.clone(),
        daemon: daemon.clone(),
        audio: audio.clone(),
        probes: probes.clone(),
        config: Arc::new(move || {
            lock(&cfg)
                .clone()
                .map_err(|message| ConfigError::Syntax { path: PathBuf::from("config.toml"), message })
        }),
        kernel: KERNEL.to_owned(),
        version: "0.5.0".to_owned(),
        workers: 4,
    };
    Rig { dir, runner, host, fw, daemon, audio, probes, config, env }
}

impl Rig {
    /// Runs a single check by title.
    fn one(&self, title: &str) -> CheckResult {
        let all = checks();
        let check =
            all.iter().find(|c| c.title == title).unwrap_or_else(|| panic!("no check called {title:?}"));
        run_one(&self.env, check)
    }

    fn state_age(&self, seconds: u64) {
        set_mtime(&self.dir.path().join("run/truesense/state"), now() - Duration::from_secs(seconds));
    }

    fn set_daemon_state(&self, f: impl FnOnce(&mut DaemonFake)) {
        f(&mut lock(&self.daemon.0));
    }

    fn remove(&self, rel: &str) {
        let path = self.dir.path().join(rel);
        if path.is_dir() { fs::remove_dir_all(path).unwrap() } else { fs::remove_file(path).unwrap() }
    }
}

/// Asserts the grade and that the detail and the fix contain the given parts.
#[track_caller]
fn want(got: CheckResult, status: Status, detail: &str, fix: &str) {
    assert!(
        got.status == status && got.detail.contains(detail) && got.fix.contains(fix),
        "{} = {{{:?} | {:?} | fix {:?}}}, want {status:?}, detail containing {detail:?}, fix containing {fix:?}",
        got.check,
        got.status,
        got.detail,
        got.fix
    );
}

// ---- the list and the runner ------------------------------------------------------------------

#[test]
fn the_titles_are_the_ones_users_and_docs_refer_to() {
    let titles: Vec<&str> = checks().iter().map(|c| c.title).collect();
    assert_eq!(
        titles.join("|"),
        "Model|Kernel|Kernel headers|DKMS|Kernel module|Device|Conflicts|Power managers|CPU turbo|Daemon|Service version|User services|udev / keyboard|Config|Secure Boot|Audio|Fans|Window|Status bars"
    );
}

#[test]
fn a_healthy_machine_has_no_warnings_and_no_failures_and_the_doctor_only_reads() {
    let r = rig();
    let report = run(&r.env, &checks());
    for c in &report.checks {
        assert!(
            matches!(c.status, Status::Ok | Status::Info),
            "{}: {:?} {} (fix: {})",
            c.check,
            c.status,
            c.detail,
            c.fix
        );
    }
    assert!(
        report.ok && report.failures == 0 && report.warnings == 0 && report.exit_code() == 0,
        "{report:?}"
    );
    for call in r.runner.calls() {
        let allowed =
            ["systemctl is-active", "systemctl --user is-active", "modinfo", "mokutil", "dkms status"];
        assert!(allowed.iter().any(|p| call.starts_with(p)), "the doctor ran {call:?}");
    }
    assert_eq!(report.version, "0.5.0");
    assert_eq!(report.kernel, KERNEL);
}

#[test]
fn checks_run_side_by_side_and_are_reported_in_order() {
    // Five checks that cannot finish until all five have started prove that they overlap.
    let barrier = Arc::new(Barrier::new(5));
    let mut list: Vec<Check> = (0..5)
        .map(|i| {
            let barrier = Arc::clone(&barrier);
            Check::new(["T0", "T1", "T2", "T3", "T4"][i], move |_| {
                barrier.wait();
                Outcome::ok(format!("c{i}"))
            })
        })
        .collect();
    list.push(Check::new("Broken", |_| panic!("boom")));
    list.push(Check::new("Last", |_| Outcome::ok("last")));
    let r = rig();
    let mut env = r.env;
    env.workers = 8;
    let report = run(&env, &list);
    let titles: Vec<&str> = report.checks.iter().map(|c| c.check.as_str()).collect();
    assert_eq!(titles, ["T0", "T1", "T2", "T3", "T4", "Broken", "Last"]);
    assert!(report.checks[..5].iter().all(|c| c.status == Status::Ok));
    let broken = &report.checks[5];
    assert_eq!(broken.status, Status::Warn);
    assert!(broken.detail.contains("boom") && broken.detail.contains("check crashed"), "{broken:?}");
    assert_eq!((report.warnings, report.failures, report.ok), (1, 0, true));
}

#[test]
fn a_panic_with_a_formatted_message_is_reported_too() {
    let r = rig();
    let list = [
        Check::new("Odd", |_| panic!("{}", String::from("formatted"))),
        Check::new("Odder", |_| std::panic::panic_any(42)),
    ];
    let report = run(&r.env, &list);
    assert!(report.checks[0].detail.contains("formatted"), "{:?}", report.checks[0]);
    assert!(report.checks[1].detail.contains("unknown panic"), "{:?}", report.checks[1]);
}

#[test]
fn the_worker_limit_is_honoured() {
    let in_flight = Arc::new(AtomicI32::new(0));
    let peak = Arc::new(AtomicI32::new(0));
    let list: Vec<Check> = (0..12)
        .map(|_| {
            let (in_flight, peak) = (Arc::clone(&in_flight), Arc::clone(&peak));
            Check::new("c", move |_| {
                let n = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(n, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(5));
                in_flight.fetch_sub(1, Ordering::SeqCst);
                Outcome::ok("")
            })
        })
        .collect();
    let r = rig();
    let mut env = r.env;
    env.workers = 3;
    run(&env, &list);
    let p = peak.load(Ordering::SeqCst);
    assert!((2..=3).contains(&p), "peak concurrency {p}");
}

#[test]
fn no_checks_and_zero_workers_are_fine() {
    let r = rig();
    let mut env = r.env;
    env.workers = 0;
    let report = run(&env, &[]);
    assert!(report.checks.is_empty() && report.ok && report.exit_code() == 0);
    let ran = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&ran);
    run(
        &env,
        &[Check::new("x", move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            Outcome::ok("")
        })],
    );
    assert_eq!(ran.load(Ordering::SeqCst), 1, "0 workers means the default, not none");
}

// ---- the checks -------------------------------------------------------------------------------

#[test]
fn model_grades() {
    for (product, vendor, bios, status) in [
        ("Predator PH16-71", "Acer", "V1.18", Status::Ok),
        ("Predator PH16-71", "Acer", "V1.99", Status::Info),
        ("Predator PH18-71", "Acer", "V1", Status::Warn),
        ("Latitude", "Dell Inc.", "A1", Status::Fail),
    ] {
        let r = rig();
        r.dir.write("sys/class/dmi/id/product_name", product);
        r.dir.write("sys/class/dmi/id/sys_vendor", vendor);
        r.dir.write("sys/class/dmi/id/bios_version", bios);
        let got = r.one("Model");
        assert_eq!(got.status, status, "{product}: {got:?}");
        match status {
            Status::Warn => assert!(got.fix.contains("truesense probe"), "{got:?}"),
            Status::Fail => assert!(got.fix.contains("Acer Predator"), "{got:?}"),
            _ => {}
        }
    }
}

#[test]
fn kernel_and_headers() {
    let mut r = rig();
    for (release, status) in [
        ("7.3.0-cachyos", Status::Ok),
        ("6.6.0", Status::Ok),
        ("6.5.9", Status::Warn),
        ("5.15.0", Status::Warn),
        ("6.18.1-lts", Status::Ok),
        ("10.0", Status::Ok),
        ("weird", Status::Warn),
        ("", Status::Warn),
        (".6", Status::Warn),
        ("6.", Status::Warn),
        ("6.x", Status::Warn),
        ("99999999999999999999.1", Status::Warn),
    ] {
        r.env.kernel = release.to_owned();
        assert_eq!(r.one("Kernel").status, status, "kernel {release:?}");
    }
    r.env.kernel = KERNEL.to_owned();
    want(r.one("Kernel headers"), Status::Ok, "present", "");
    r.env.kernel = "9.9.9-missing".to_owned();
    want(r.one("Kernel headers"), Status::Fail, "MISSING", "dkms autoinstall");
}

#[test]
fn the_kernel_release_parser() {
    for (text, want) in [
        ("6.6", Some((6, 6))),
        ("6.18.1-lts", Some((6, 18))),
        ("7.3.0-4-cachyos", Some((7, 3))),
        ("6.", None),
        (".6", None),
        ("a.1", None),
        ("6.a", None),
        ("-6.1", None),
        ("", None),
    ] {
        assert_eq!(checks::parse_release(text), want, "{text:?}");
    }
}

#[test]
fn dkms_reads_its_own_directories() {
    let mut r = rig();
    let root = TempDir::new("dkms");
    r.env.root = root.path().to_path_buf();
    install_dkms(&root, "truesense-wmi", &[KERNEL, "6.18.1-lts"]);
    install_kernels(&root, &[KERNEL, "6.18.1-lts"]);
    let got = r.one("DKMS");
    want(got.clone(), Status::Ok, "6.18.1-lts", "");
    want(got, Status::Ok, KERNEL, "");
    assert!(!r.runner.called("dkms"), "the command is not needed when the directories answer");
}

#[test]
fn dkms_not_registered_not_built_for_this_kernel_and_missing_for_another() {
    let mut r = rig();
    let root = TempDir::new("dkms");
    r.env.root = root.path().to_path_buf();
    root.mkdir("var/lib/dkms"); // DKMS is there, the package is not
    install_kernels(&root, &[KERNEL]);
    want(r.one("DKMS"), Status::Fail, "not registered", "tools/install.sh");

    install_dkms(&root, "truesense-wmi", &["6.18.1-lts"]);
    install_kernels(&root, &["6.18.1-lts"]);
    want(r.one("DKMS"), Status::Fail, &format!("running kernel {KERNEL}"), "dkms autoinstall");

    install_dkms(&root, "truesense-wmi", &[KERNEL]);
    install_kernels(&root, &["7.4.0-new"]);
    want(r.one("DKMS"), Status::Warn, "7.4.0-new", "dkms autoinstall");

    // a dangling kernel link does not count as built
    fs::remove_dir_all(root.path().join(format!("var/lib/dkms/truesense-wmi/0.5.0/{KERNEL}"))).unwrap();
    want(r.one("DKMS"), Status::Fail, "running kernel", "");
}

#[test]
fn a_system_with_no_dkms_directory_falls_back_to_the_command() {
    let mut r = rig();
    let root = TempDir::new("dkms");
    r.env.root = root.path().to_path_buf();
    install_kernels(&root, &[KERNEL]);
    r.runner.set("dkms status truesense-wmi", &format!("truesense-wmi/0.5.0, {KERNEL}, x86_64: installed"));
    want(r.one("DKMS"), Status::Ok, KERNEL, "");
    r.runner.set("dkms status truesense-wmi", "");
    want(r.one("DKMS"), Status::Fail, "not registered", "");
    r.runner.remove("dkms status truesense-wmi"); // dkms not even installed
    want(r.one("DKMS"), Status::Fail, "not registered", "");
}

#[test]
fn the_dkms_status_parser() {
    let text = "truesense-wmi/0.5.0, 6.18.1, x86_64: installed\n\
                truesense-wmi/0.5.0, 6.19.0-arch1, x86_64: built\n\
                other/1.0, 7.0,x86_64:installed (WARNING! Diff between built and installed module!)\n\
                garbage\n\
                , , x86_64: installed\n\
                a, b";
    assert_eq!(checks::parse_dkms_status(text), ["6.18.1", "7.0"]);
    assert!(checks::parse_dkms_status("").is_empty());
    assert!(checks::parse_dkms_status(",,,,").is_empty());
    assert!(checks::parse_dkms_status("a, b, x86_64:").is_empty());
}

#[test]
fn the_dkms_link_names() {
    for (name, want) in [
        ("kernel-6.18.1-lts-x86_64", Some("6.18.1-lts")),
        ("kernel-6.1.0-x86_64", Some("6.1.0")),
        ("kernel-6.1.0-aarch64", Some("6.1.0")),
        ("kernel--x86_64", None),
        ("kernel-6.1.0-", None),
        ("kernel-6.1.0", None),
        ("kernel-x86_64", None),
        ("kernel-6.1.0-x86 64", None),
        ("original_module", None),
        ("", None),
    ] {
        assert_eq!(checks::dkms_link_kernel(name), want, "{name:?}");
    }
}

#[test]
fn old_dkms_packages_are_found_from_the_directory_listing() {
    let r = rig();
    install_dkms(&r.dir, "linuwu-sense", &[KERNEL]);
    let got = r.one("Conflicts");
    want(got.clone(), Status::Warn, "old DKMS", "migrate-legacy.sh");
    want(got, Status::Warn, "linuwu-sense", "");
    r.remove("var/lib/dkms/linuwu-sense");
    assert!(!r.one("Conflicts").detail.contains("old DKMS"));
    install_dkms(&r.dir, "acer-rear-wmi", &[KERNEL]);
    want(r.one("Conflicts"), Status::Warn, "acer-rear-wmi", "");
}

#[test]
fn conflicts() {
    let mut r = rig();
    want(r.one("Conflicts"), Status::Ok, "old drivers are blocked", "");
    r.dir.mkdir("sys/module/acer_wmi");
    want(r.one("Conflicts"), Status::Fail, "acer_wmi", "migrate-legacy.sh");
    r.remove("sys/module/acer_wmi");
    r.dir.mkdir("sys/module/linuwu_sense");
    want(r.one("Conflicts"), Status::Fail, "linuwu_sense", "");
    r.remove("sys/module/linuwu_sense");
    r.remove("etc/modprobe.d/truesense.conf");
    want(r.one("Conflicts"), Status::Warn, "no modprobe block", "tools/install.sh");
    // with no DKMS directory the legacy packages are looked up with the command
    let root = TempDir::new("conflicts");
    root.write("etc/modprobe.d/truesense.conf", "");
    r.env.root = root.path().to_path_buf();
    r.runner.set(
        "dkms status",
        "linuwu-sense/1.0, 7.3.0, x86_64: installed\ntruesense-wmi/0.5.0, 7.3.0, x86_64: installed",
    );
    want(r.one("Conflicts"), Status::Warn, "linuwu-sense", "");
}

#[test]
fn a_long_list_of_old_packages_is_cut() {
    let r = rig();
    for i in 0..12 {
        r.dir.mkdir(&format!("var/lib/dkms/linuwu-sense-package-number-{i}"));
    }
    let got = r.one("Conflicts");
    let listed = got.detail.strip_prefix("old DKMS package(s) still registered: ").unwrap();
    assert_eq!(listed.chars().count(), 80, "{got:?}");
}

#[test]
fn kernel_module() {
    let r = rig();
    want(r.one("Kernel module"), Status::Ok, "truesense_wmi 0.5.0 loaded", "");
    r.runner.set("modinfo -F version truesense_wmi", "0.5.1");
    want(r.one("Kernel module"), Status::Warn, "loaded 0.5.0 but 0.5.1 is installed", "reboot");
    r.runner.remove("modinfo -F version truesense_wmi"); // modinfo missing: nothing to compare
    want(r.one("Kernel module"), Status::Ok, "", "");
    r.remove("sys/module/truesense_wmi/version");
    want(r.one("Kernel module"), Status::Ok, "truesense_wmi ? loaded", "");
    r.remove("sys/module/truesense_wmi");
    want(r.one("Kernel module"), Status::Fail, "not loaded", "modprobe truesense_wmi");
}

#[test]
fn device() {
    let r = rig();
    want(r.one("Device"), Status::Ok, "interfaces: game=yes, apge=yes, battery=yes", "");
    lock(&r.fw.0).caps.battery = false;
    want(r.one("Device"), Status::Warn, "battery=NO", "the firmware lacks battery");
    lock(&r.fw.0).caps = Capabilities { game: false, apge: true, battery: false };
    want(r.one("Device"), Status::Warn, "game=NO, apge=yes, battery=NO", "the firmware lacks game, battery");
    lock(&r.fw.0).caps_err = Some("device busy".to_owned());
    want(r.one("Device"), Status::Fail, "device busy", "dmesg");
    r.host.state().deny_access = true;
    want(r.one("Device"), Status::Warn, "no access", "uaccess");
    r.remove("dev/truesense");
    want(r.one("Device"), Status::Fail, "/dev/truesense missing", "dmesg | grep truesense");
}

#[test]
fn power_managers() {
    let r = rig();
    want(r.one("Power managers"), Status::Ok, "no other power manager", "");
    r.runner.set("systemctl is-active thermald.service", "active");
    want(r.one("Power managers"), Status::Info, "also running: thermald", "");
    r.runner.set("systemctl is-active tlp.service", "active");
    r.runner.set("systemctl is-active power-profiles-daemon.service", "active");
    want(
        r.one("Power managers"),
        Status::Warn,
        "also running: tlp, thermald, power-profiles-daemon",
        "truesense power managers stop",
    );
}

#[test]
fn cpu_turbo() {
    let r = rig();
    want(r.one("CPU turbo"), Status::Ok, "turbo boost is on", "");
    lock(&r.probes.0).low = 2;
    want(r.one("CPU turbo"), Status::Warn, "2 core(s) have a frequency ceiling", "power managers stop");
    r.dir.write("sys/devices/system/cpu/intel_pstate/no_turbo", "1\n");
    want(r.one("CPU turbo"), Status::Warn, "turbo boost is OFF", "no_turbo");
    r.remove("sys/devices/system/cpu/intel_pstate/no_turbo");
    want(r.one("CPU turbo"), Status::Info, "no intel_pstate", "");
}

#[test]
fn daemon() {
    let r = rig();
    want(r.one("Daemon"), Status::Ok, "running; smart_fan=off, auto_profile=on, activity=idle", "");
    r.set_daemon_state(|d| {
        d.state.settings.smart_fan = true;
        d.state.live.activity = None;
    });
    want(r.one("Daemon"), Status::Ok, "smart_fan=on, auto_profile=on, activity=?", "");
    r.set_daemon_state(|d| d.state.live.activity = Some(String::new()));
    want(r.one("Daemon"), Status::Ok, "activity=?", "");

    r.set_daemon_state(|d| d.state.live.hw_ok = Some(false));
    want(r.one("Daemon"), Status::Warn, "cannot reach the kernel device", "module");
    r.set_daemon_state(|d| d.state.live.hw_ok = Some(true));
    want(r.one("Daemon"), Status::Ok, "", "");
    r.set_daemon_state(|d| d.state.live.hw_ok = None);

    r.state_age(42);
    want(r.one("Daemon"), Status::Warn, "daemon state is 42s old", "restart truesense");
    r.state_age(10);
    want(r.one("Daemon"), Status::Ok, "", ""); // exactly 10 s is still fresh
    r.remove("run/truesense/state"); // no state file: no age to judge
    want(r.one("Daemon"), Status::Ok, "", "");

    r.set_daemon_state(|d| d.state_err = Some("timeout".to_owned()));
    want(r.one("Daemon"), Status::Warn, "control socket does not answer: timeout", "restart truesense");
    r.runner.set("systemctl is-active truesense.service", "failed");
    want(r.one("Daemon"), Status::Fail, "not running", "journalctl -u truesense");
}

#[test]
fn a_state_file_from_the_future_is_not_stale() {
    let r = rig();
    set_mtime(&r.dir.path().join("run/truesense/state"), now() + Duration::from_secs(3600));
    want(r.one("Daemon"), Status::Ok, "running", "");
}

#[test]
fn service_version() {
    let r = rig();
    want(r.one("Service version"), Status::Ok, "same version (0.5.0)", "");
    assert_eq!(lock(&r.daemon.0).reads, ["fan_curve", "battery_full"]);
    r.set_daemon_state(|d| {
        d.read_err.insert("battery_full".to_owned(), "unknown command: battery_full".to_owned());
    });
    want(r.one("Service version"), Status::Warn, "older than the installed app", "restart truesense");
    r.set_daemon_state(|d| {
        d.read_err.clear();
        d.read_err.insert("fan_curve".to_owned(), "socket closed".to_owned());
    });
    want(r.one("Service version"), Status::Warn, "did not answer a version probe: socket closed", "");
    r.runner.set("systemctl is-active truesense.service", "inactive");
    want(r.one("Service version"), Status::Info, "service not running", "");
}

#[test]
fn user_services() {
    let r = rig();
    want(
        r.one("User services"),
        Status::Ok,
        "truesense-keys=active, truesense-audio=active, truesense-startup=active",
        "",
    );
    r.runner.set("systemctl --user is-active truesense-audio.service", "inactive");
    r.runner.remove("systemctl --user is-active truesense-startup.service");
    want(
        r.one("User services"),
        Status::Warn,
        "truesense-audio=inactive, truesense-startup=inactive",
        "systemctl --user enable --now truesense-audio.service truesense-startup.service",
    );
}

#[test]
fn udev() {
    let r = rig();
    want(r.one("udev / keyboard"), Status::Ok, "Fn-key interface /dev/hidraw3 readable", "");
    r.host.state().deny_access = true;
    want(r.one("udev / keyboard"), Status::Warn, "NOT readable", "");
    lock(&r.probes.0).node = None;
    want(r.one("udev / keyboard"), Status::Warn, "Fn-key interface not found", "04f2:0117");
    r.remove("etc/udev/rules.d/70-truesense.rules");
    want(r.one("udev / keyboard"), Status::Fail, "udev rule missing", "tools/install.sh");
    r.dir.write("usr/lib/udev/rules.d/70-truesense.rules", ""); // the packaged location counts too
    want(r.one("udev / keyboard"), Status::Warn, "Fn-key interface not found", "");
}

#[test]
fn config() {
    let r = rig();
    want(r.one("Config"), Status::Ok, "configuration valid", "");
    let mut bad = Config::default();
    bad.audio.jack = "sideways".to_owned();
    bad.update.check_hours = 0;
    *lock(&r.config) = Ok(bad);
    let got = r.one("Config");
    want(got.clone(), Status::Fail, "[audio] jack must be auto, headphones or headset", "config.toml");
    want(got, Status::Fail, "[update] check_hours must be 1..168", "");
    *lock(&r.config) = Err("line 3: bad".to_owned());
    want(r.one("Config"), Status::Fail, "config does not parse: config.toml: line 3: bad", "TOML syntax");
}

#[test]
fn secure_boot() {
    let r = rig();
    want(r.one("Secure Boot"), Status::Info, "Secure Boot off or unknown; module signer: none", "");
    r.runner.set("mokutil --sb-state", "SecureBoot enabled");
    want(r.one("Secure Boot"), Status::Warn, "module signer: NONE", "mokutil --import");
    r.runner.set("modinfo -F signer truesense_wmi", "DKMS module signing key");
    want(r.one("Secure Boot"), Status::Ok, "module signer: DKMS module signing key", "");
}

#[test]
fn audio() {
    let r = rig();
    let set = |f: &dyn Fn(&mut AudioFake)| f(&mut lock(&r.audio.0));
    set(&|a| a.jack = Ok(Jack { headphones: true, headset_mic: true }));
    want(
        r.one("Audio"),
        Status::Ok,
        "PipeWire running; output sink present; RNNoise plugin found; AcerUltraX found; jack: headphones in, headset mic detected",
        "",
    );

    set(&|a| {
        a.sound = Ok(SoundState::Broken(
            "the sound card is switched off; its speaker and headphone profiles are missing".to_owned(),
        ))
    });
    let got = r.one("Audio");
    want(
        got.clone(),
        Status::Warn,
        "NO SOUND OUTPUT: the sound card is switched off",
        "truesense audio repair",
    );
    want(got, Status::Warn, "PipeWire running", "");

    set(&|a| {
        a.sound = Ok(SoundState::Unknown); // no card to judge
        a.bare = Ok(vec!["Headset PC38X".to_owned()]);
    });
    want(
        r.one("Audio"),
        Status::Warn,
        "DTS is off (Plain) for: Headset PC38X; PipeWire running",
        "truesense audio dts all",
    );
    r.host.state().euid = 0; // the DTS service is per user: a root doctor does not ask
    want(r.one("Audio"), Status::Ok, "PipeWire running", "");
    r.host.state().euid = 1000;
    set(&|a| a.dts = false);
    want(r.one("Audio"), Status::Ok, "AcerUltraX not installed (optional)", "");
    set(&|a| a.dts = true);
    set(&|a| a.bare = Err("cli failed".to_owned()));
    want(r.one("Audio"), Status::Ok, "", "");
    set(&|a| a.jack = Err("no amixer".to_owned()));
    assert!(!r.one("Audio").detail.contains("jack:"));
    set(&|a| a.sound = Err("pactl failed".to_owned()));
    want(r.one("Audio"), Status::Ok, "PipeWire running", "");

    // A dead PipeWire is not a dead card: the card is not even asked.
    r.runner.set("systemctl --user is-active pipewire.service", "inactive");
    set(&|a| {
        a.sound = Ok(SoundState::Broken("x".to_owned()));
        a.sound_asked = false;
    });
    let got = r.one("Audio");
    want(got.clone(), Status::Warn, "PipeWire NOT running", "systemctl --user start pipewire");
    assert!(!got.detail.contains("NO SOUND OUTPUT") && !lock(&r.audio.0).sound_asked, "{got:?}");
    set(&|a| {
        a.rnnoise = false;
        a.dts = false;
    });
    want(
        r.one("Audio"),
        Status::Warn,
        "RNNoise plugin missing (Purified Voice off); AcerUltraX not installed (optional)",
        "",
    );
}

#[test]
fn fans() {
    let r = rig();
    let set = |mode: FanMode, raw: &str| {
        let mut s = lock(&r.fw.0);
        s.fan.mode = mode;
        s.fan.behavior_raw = raw.to_owned();
    };
    want(r.one("Fans"), Status::Ok, "fans: firmware auto", "");
    set(FanMode::Manual, "0xc3");
    want(r.one("Fans"), Status::Ok, "fans: manual", "");
    set(FanMode::Unknown, "0x7");
    want(r.one("Fans"), Status::Ok, "fans: 0x7", "");
    set(FanMode::Max, "0x82");
    want(r.one("Fans"), Status::Ok, "fans: MAX", "");
    r.set_daemon_state(|d| d.state.settings.smart_fan = true);
    want(r.one("Fans"), Status::Warn, "MAX while the smart curve is on", "truesense fan smart on");
    r.set_daemon_state(|d| d.state_err = Some("down".to_owned())); // no daemon: nothing to contradict
    want(r.one("Fans"), Status::Ok, "fans: MAX", "");
    lock(&r.fw.0).fan_err = Some("busy".to_owned());
    want(r.one("Fans"), Status::Info, "fan state unreadable", "");
}

#[test]
fn window_and_status_bars() {
    let r = rig();
    want(r.one("Window"), Status::Ok, "launcher found", "");
    want(r.one("Status bars"), Status::Ok, "fast path", "");
    r.host.state().programs.clear();
    want(r.one("Window"), Status::Info, "xdg-open", "xdg-utils");
    want(r.one("Status bars"), Status::Info, "truesense-fast not found", "installer");
}

#[test]
fn a_user_session_command_is_run_as_the_sudo_user() {
    let r = rig();
    {
        let mut h = r.host.state();
        h.euid = 0;
        h.env.insert("SUDO_USER".to_owned(), "alice".to_owned());
    }
    r.runner.set(
        "runuser -u alice -- env XDG_RUNTIME_DIR=/run/user/1234 DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1234/bus systemctl --user is-active truesense-keys.service",
        "active",
    );
    assert!(r.env.active("truesense-keys.service", true), "{:?}", r.runner.calls());
    // system units are never wrapped
    r.env.active("truesense.service", false);
    assert!(r.runner.called("systemctl is-active truesense.service"), "{:?}", r.runner.calls());
    // root itself, no SUDO_USER, an unknown user, or not being root: our own session
    let own = |r: &Rig| r.env.as_user(vec!["x".to_owned()]);
    r.host.state().env.insert("SUDO_USER".to_owned(), "root".to_owned());
    assert_eq!(own(&r), ["x"]);
    r.host.state().env.remove("SUDO_USER");
    assert_eq!(own(&r), ["x"]);
    r.host.state().env.insert("SUDO_USER".to_owned(), "ghost".to_owned());
    assert_eq!(own(&r), ["x"], "an unknown user is not an error");
    r.host.state().env.insert("SUDO_USER".to_owned(), "alice".to_owned());
    r.host.state().euid = 1000;
    assert_eq!(own(&r), ["x"], "only root needs to borrow a session");
}

// ---- the report -------------------------------------------------------------------------------

fn sample_report() -> Report {
    let result = |check: &str, status, detail: &str, fix: &str| CheckResult {
        check: check.to_owned(),
        status,
        detail: detail.to_owned(),
        fix: fix.to_owned(),
    };
    Report {
        version: "0.5.0".to_owned(),
        kernel: "7.3.0".to_owned(),
        ok: false,
        failures: 1,
        warnings: 1,
        checks: vec![
            result("Model", Status::Ok, "Predator PH16-71, BIOS V1.18", ""),
            result("Audio", Status::Warn, "no output", "truesense audio repair"),
            result("DKMS", Status::Fail, "not built", "sudo dkms autoinstall"),
            result("Window", Status::Info, "no browser", "install one"),
        ],
    }
}

#[test]
fn the_report_renders_like_the_old_one() {
    let report = sample_report();
    assert_eq!(
        render_text(&report, false),
        "TrueSense 0.5.0 doctor (7.3.0)\n\
         \x20 [OK  ] Model            Predator PH16-71, BIOS V1.18\n\
         \x20 [WARN] Audio            no output\n\
         \x20        fix: truesense audio repair\n\
         \x20 [FAIL] DKMS             not built\n\
         \x20        fix: sudo dkms autoinstall\n\
         \x20 [INFO] Window           no browser\n\
         \nPROBLEMS FOUND: 1 failure(s), 1 warning(s)\n"
    );
    let coloured = render_text(&report, true);
    assert!(
        coloured.contains("[\x1b[32mOK  \x1b[0m]") && coloured.contains("\x1b[31mFAIL\x1b[0m"),
        "{coloured:?}"
    );
    assert!(coloured.contains("\x1b[33mWARN\x1b[0m") && coloured.contains("\x1b[36mINFO\x1b[0m"));
    for (failures, warnings, verdict) in [
        (0, 0, "ALL GOOD: 0 failure(s), 0 warning(s)"),
        (0, 2, "OK with warnings: 0 failure(s), 2 warning(s)"),
        (3, 0, "PROBLEMS FOUND: 3 failure(s), 0 warning(s)"),
    ] {
        let r = Report { failures, warnings, checks: vec![], ..sample_report() };
        assert!(render_text(&r, false).contains(verdict), "{verdict}");
    }
}

#[test]
fn the_report_is_json_with_the_python_names_in_order() {
    let report = sample_report();
    let json = render_json(&report).unwrap();
    let positions: Vec<usize> = ["version", "kernel", "ok", "failures", "warnings", "checks"]
        .iter()
        .map(|k| json.find(&format!("\n  \"{k}\"")).unwrap())
        .collect();
    assert!(positions.windows(2).all(|w| w[0] < w[1]), "{json}");
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        value["checks"][1],
        serde_json::json!({"check": "Audio", "status": "WARN", "detail": "no output", "fix": "truesense audio repair"})
    );
    assert_eq!(serde_json::from_str::<Report>(&json).unwrap(), report);
    assert!(json.contains("\"check\": \"Model\",\n      \"status\": \"OK\""), "{json}");
    assert_eq!(report.exit_code(), 1);
    assert_eq!(
        Report { failures: 0, warnings: 5, ..report }.exit_code(),
        0,
        "exit code: 1 only for failures"
    );
}

#[test]
fn the_statuses_are_the_four_words() {
    assert_eq!(
        [Status::Ok, Status::Warn, Status::Fail, Status::Info].map(Status::as_str),
        ["OK", "WARN", "FAIL", "INFO"]
    );
}

// ---- the real host and the simulator --------------------------------------------------------------

#[test]
fn the_firmware_trait_reads_through_hw() {
    let hw = Hw::new(Arc::new(Simulator::new()) as Arc<dyn Device>);
    let fw: &dyn Firmware = &hw;
    assert_eq!(fw.capabilities().unwrap(), Capabilities { game: true, apge: true, battery: true });
    assert_eq!(fw.fan_state().unwrap().mode, FanMode::Auto);
}

#[test]
fn the_device_and_fans_checks_work_over_the_simulator() {
    let mut r = rig();
    r.env.firmware = Arc::new(Hw::new(Arc::new(Simulator::new()) as Arc<dyn Device>));
    want(r.one("Device"), Status::Ok, "game=yes, apge=yes, battery=yes", "");
    want(r.one("Fans"), Status::Ok, "fans: firmware auto", "");
}

#[test]
fn the_daemon_state_deserialises_the_daemons_reply() {
    let state: DaemonState = serde_json::from_str(
        r#"{"live": {"hw_ok": false, "activity": "gaming", "cpu_temp": 50}, "settings": {"smart_fan": true, "auto_profile": false, "x": 1}}"#,
    )
    .unwrap();
    assert_eq!(state.live, DaemonLive { hw_ok: Some(false), activity: Some("gaming".to_owned()) });
    assert_eq!(state.settings, DaemonSettings { smart_fan: true, auto_profile: false });
    assert_eq!(serde_json::from_str::<DaemonState>("{}").unwrap(), DaemonState::default());
}

#[test]
fn an_unknown_command_is_recognised_in_any_wording() {
    assert!(CallError::new("unknown command 'fan_curve': the service is older").is_unknown_command());
    assert!(CallError::new("err unknown command").is_unknown_command());
    assert!(!CallError::new("socket closed").is_unknown_command());
    assert_eq!(CallError::new("x").to_string(), "x");
}

#[test]
fn the_system_env_is_the_real_machine_and_runs_nothing_by_itself() {
    let r = rig();
    let env = Env::system(r.fw.clone(), r.daemon.clone(), r.audio.clone(), r.probes.clone());
    assert_eq!(env.root, Path::new("/"));
    assert!(
        !env.kernel.is_empty()
            && env.version == ts_foundation::buildinfo::VERSION
            && env.workers == DEFAULT_WORKERS
    );
    assert_eq!(env.path("/sys/module/x"), Path::new("/sys/module/x"));
    assert_eq!(r.env.path("/etc/x"), r.dir.path().join("etc/x"));
    assert!(r.runner.calls().is_empty());
}

#[test]
fn the_effective_user_id_is_the_one_the_kernel_reports() {
    let status = fs::read_to_string("/proc/self/status").unwrap();
    let uid_line = status.lines().find(|l| l.starts_with("Uid:")).unwrap();
    let effective: u32 = uid_line.split_whitespace().nth(2).unwrap().parse().unwrap();
    assert_eq!(sys::effective_uid(), effective);
    assert_eq!(SystemHost::new().euid(), effective);
}

#[test]
fn access_follows_the_files_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new("access");
    dir.write("rw", "x");
    dir.write("ro", "x");
    dir.write("none", "x");
    fs::set_permissions(dir.path().join("ro"), fs::Permissions::from_mode(0o444)).unwrap();
    fs::set_permissions(dir.path().join("none"), fs::Permissions::from_mode(0o000)).unwrap();
    let host = SystemHost::new();
    assert!(host.can_access(&dir.path().join("rw"), true));
    assert!(host.can_access(&dir.path().join("ro"), false));
    assert!(!host.can_access(&dir.path().join("missing"), false));
    assert!(!host.can_access(Path::new("/nonexistent/x\0y"), false), "a NUL cannot be in a path");
    if sys::effective_uid() != 0 {
        assert!(!host.can_access(&dir.path().join("ro"), true));
        assert!(!host.can_access(&dir.path().join("none"), false));
    }
}

#[test]
fn passwd_lookups_survive_hostile_files() {
    let passwd = "# comment\nroot:x:0:0:root:/root:/bin/bash\n\nbroken\nbad:x:notanumber:1\nshort:x\nalice:x:1234:1234::/home/alice:/bin/fish\n:x:5:5\n";
    assert_eq!(sys::passwd_uid(passwd, "alice"), Some(1234));
    assert_eq!(sys::passwd_uid(passwd, "root"), Some(0));
    for user in ["bad", "short", "broken", "ghost", "", "#", "ali", "alice:x"] {
        let want = if user.is_empty() { Some(5) } else { None };
        assert_eq!(sys::passwd_uid(passwd, user), want, "{user:?}");
    }
    assert_eq!(sys::passwd_uid("", "alice"), None);
    let dir = TempDir::new("passwd");
    dir.write("passwd", passwd);
    let host = SystemHost::new().with_passwd(dir.path().join("passwd"));
    assert_eq!(host.lookup_uid("alice"), Some(1234));
    assert_eq!(SystemHost::new().with_passwd("/nonexistent/passwd").lookup_uid("alice"), None);
    assert_eq!(SystemHost::new().lookup_uid("root"), Some(0));
}

#[test]
fn programs_are_found_on_the_path_by_name_only() {
    let host = SystemHost::new();
    assert!(host.program_on_path("sh"));
    assert!(!host.program_on_path("definitely-not-a-program-truesense"));
    assert!(!host.program_on_path(""));
    assert!(!host.program_on_path("../bin/sh"));
    assert!(!host.program_on_path("/bin/sh"));
    assert!(host.now() > UNIX_EPOCH);
}

#[test]
fn the_environment_variable_lookup_ignores_empty_values() {
    let host = SystemHost::new();
    assert_eq!(host.env_var("TRUESENSE_DOCTOR_TEST_SURELY_UNSET"), None);
    assert!(host.env_var("PATH").is_some());
}
