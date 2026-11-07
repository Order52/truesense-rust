//! Doubles and a hermetic [`Services`] for tests of the application layer and of the clients that
//! sit on it (command line, terminal UI, window).
//!
//! [`Services::simulated`] builds every dependency under one directory the caller owns: the
//! firmware is the in-memory simulator, the keyboard the simulated controller, the daemon a
//! scripted [`FakeDaemon`], the sound system and the power limits run over a runner that starts
//! nothing, and every file lives below the given root. Nothing here touches the real machine.
#![forbid(unsafe_code)]

use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};
use ts_decision::{curves, smartprofile};
use ts_domain::actions::{ActionError, ActionRunner, HostResult};
use ts_domain::audio::{Audio, Paths};
use ts_domain::designs::DesignStore;
use ts_domain::gpuundervolt;
use ts_domain::keybinds::{KeyProfiles, syntax_check};
use ts_domain::keyboard::{Keyboard, SimHid};
use ts_domain::keymap::KeyMap;
use ts_domain::keyprofiles::ProfileStore as LightingProfiles;
use ts_domain::looks::LookStore;
use ts_domain::perkeyfx::FxStore;
use ts_domain::power::{HostSysfs, PowerLimits};
use ts_domain::powermgr::PowerManagers;
use ts_domain::profiles::{Bundle, BundleError, BundleSource};
use ts_domain::undervolt;
use ts_foundation::buildinfo;
use ts_foundation::userstate;
use ts_ipc::client::ClientError;
use ts_ipc::wire::Args;
use ts_platform::firmware::Simulator;
use ts_platform::hw::{CommandRunner, Hw, LinuxSensors, Sleeper};
use ts_system::doctor::{self, Host};
use ts_system::models::ProbeOptions;
use ts_system::sensors::Sampler;
use ts_system::services::{Output, Runner, Step, Unit};
use ts_system::touchpad::Touchpad;
use ts_system::update::{ApplyOptions, ApplyResult, Info, Status as UpdateStatus, UpdateError};

use super::{
    ConfigFiles, Daemon, NoNotifier, Notifier, ProfileBundles, RearMemoryInUserState, ServiceChecks,
    Services, UpdateService,
};

/// What a [`FakeDaemon`] answers to a command.
#[derive(Debug, Clone)]
pub enum Reply {
    /// A result.
    Ok(Value),
    /// The daemon said no, with this text.
    Error(String),
    /// The daemon does not know the command (it is older than the program).
    UnknownCommand,
    /// The daemon answered something that is not what was expected.
    Unreachable(String),
}

impl Reply {
    fn into_result(self, cmd: &str) -> Result<Value, ClientError> {
        match self {
            Self::Ok(v) => Ok(v),
            Self::Error(text) => Err(ClientError::Daemon(text)),
            Self::UnknownCommand => Err(ClientError::UnknownCommand { cmd: cmd.to_owned() }),
            Self::Unreachable(why) => Err(ClientError::Unreachable(why)),
        }
    }
}

/// A scripted control socket: it records every call and answers from a table (a command with no
/// scripted answer is "unknown command", like a daemon that is too old). Switch it off with
/// [`FakeDaemon::set_available`] to test the daemon-down fallbacks.
#[derive(Debug, Default)]
pub struct FakeDaemon {
    available: AtomicBool,
    replies: Mutex<BTreeMap<String, VecDeque<Reply>>>,
    calls: Mutex<Vec<(String, Args)>>,
}

impl FakeDaemon {
    /// A daemon that is running and knows no command yet.
    pub fn running() -> Arc<Self> {
        let d = Self::default();
        d.available.store(true, Ordering::SeqCst);
        Arc::new(d)
    }

    /// A daemon that is not running.
    pub fn stopped() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Starts or stops the daemon.
    pub fn set_available(&self, on: bool) {
        self.available.store(on, Ordering::SeqCst);
    }

    /// Answers `cmd` with `reply` every time (until [`FakeDaemon::then`] queues others).
    pub fn answer(&self, cmd: &str, reply: Reply) -> &Self {
        self.replies
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(cmd.to_owned(), VecDeque::from([reply]));
        self
    }

    /// Answers `cmd` with a result every time.
    pub fn answer_ok(&self, cmd: &str, result: Value) -> &Self {
        self.answer(cmd, Reply::Ok(result))
    }

    /// Queues one more answer for `cmd`: the earlier answers are used up first, the last one stays.
    pub fn then(&self, cmd: &str, reply: Reply) -> &Self {
        self.replies
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(cmd.to_owned())
            .or_default()
            .push_back(reply);
        self
    }

    /// Every call so far, in order.
    pub fn calls(&self) -> Vec<(String, Args)> {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// The commands called so far, in order.
    pub fn commands(&self) -> Vec<String> {
        self.calls().into_iter().map(|(c, _)| c).collect()
    }

    /// The arguments of the last call of `cmd`.
    pub fn last_args(&self, cmd: &str) -> Option<Args> {
        self.calls().into_iter().rev().find(|(c, _)| c == cmd).map(|(_, a)| a)
    }

    /// Forgets the recorded calls.
    pub fn clear_calls(&self) {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).clear();
    }
}

impl Daemon for FakeDaemon {
    fn available(&self) -> bool {
        self.available.load(Ordering::SeqCst)
    }

    fn call(&self, cmd: &str, args: Args) -> Result<Value, ClientError> {
        if !self.available() {
            return Err(ClientError::NotRunning);
        }
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).push((cmd.to_owned(), args));
        let mut replies = self.replies.lock().unwrap_or_else(PoisonError::into_inner);
        let reply = match replies.get_mut(cmd) {
            Some(queue) if queue.len() > 1 => queue.pop_front(),
            Some(queue) => queue.front().cloned(),
            None => None,
        };
        reply.unwrap_or(Reply::UnknownCommand).into_result(cmd)
    }
}

/// A key-action runner that records what it was asked to run.
#[derive(Debug, Default)]
pub struct FakeActions {
    calls: Mutex<Vec<String>>,
    replies: Mutex<BTreeMap<String, Result<String, String>>>,
}

impl FakeActions {
    /// What `run(spec)` answers: a message, or (with `Err`) a failure.
    pub fn answer(&self, spec: &str, reply: Result<&str, &str>) {
        self.replies
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(spec.to_owned(), reply.map(str::to_owned).map_err(str::to_owned));
    }

    /// The actions run so far.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl ActionRunner for FakeActions {
    fn run(&self, spec: &str) -> Result<String, ActionError> {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).push(spec.to_owned());
        match self.replies.lock().unwrap_or_else(PoisonError::into_inner).get(spec) {
            Some(Ok(message)) => Ok(message.clone()),
            Some(Err(message)) => Err(ActionError::Invalid(message.clone())),
            None => Ok(format!("ran {spec}")),
        }
    }
}

/// A notifier that remembers what it was asked to show.
#[derive(Debug, Default)]
pub struct RecordingNotifier {
    shown: Mutex<Vec<(String, String)>>,
}

impl RecordingNotifier {
    /// The messages shown so far as `(text, icon)`.
    pub fn shown(&self) -> Vec<(String, String)> {
        self.shown.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl Notifier for RecordingNotifier {
    fn show(&self, text: &str, icon: &str) {
        self.shown.lock().unwrap_or_else(PoisonError::into_inner).push((text.to_owned(), icon.to_owned()));
    }
}

/// An in-memory set of whole-setup profiles.
#[derive(Debug, Default)]
pub struct FakeBundles {
    saved: Mutex<BTreeMap<String, Bundle>>,
    applied: Mutex<Vec<String>>,
}

impl FakeBundles {
    /// Adds a profile with this description.
    pub fn add(&self, name: &str, description: &str) {
        let mut b = Bundle::new();
        b.insert("description".to_owned(), ts_foundation::config::Value::String(description.to_owned()));
        self.saved.lock().unwrap_or_else(PoisonError::into_inner).insert(name.to_owned(), b);
    }

    /// The profiles applied so far.
    pub fn applied(&self) -> Vec<String> {
        self.applied.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl ProfileBundles for FakeBundles {
    fn names(&self) -> Vec<String> {
        self.saved.lock().unwrap_or_else(PoisonError::into_inner).keys().cloned().collect()
    }

    fn source(&self, name: &str) -> Option<BundleSource> {
        self.saved
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(name)
            .then_some(BundleSource::Yours)
    }

    fn load(&self, name: &str) -> Result<Bundle, BundleError> {
        self.saved
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
            .cloned()
            .ok_or_else(|| BundleError::NotFound(format!("no profile '{name}'")))
    }

    fn apply(&self, name: &str) -> Result<String, BundleError> {
        self.load(name)?;
        self.applied.lock().unwrap_or_else(PoisonError::into_inner).push(name.to_owned());
        Ok(format!("applied {name}"))
    }

    fn next_name(&self) -> Result<String, BundleError> {
        let names = self.names();
        let last = self.applied().last().cloned();
        let at = last.and_then(|l| names.iter().position(|n| *n == l));
        names.get(at.map_or(0, |i| (i + 1) % names.len())).cloned().ok_or(BundleError::NoProfiles)
    }

    fn capture(&self, description: &str) -> HostResult<Bundle> {
        let mut b = Bundle::new();
        b.insert("description".to_owned(), ts_foundation::config::Value::String(description.to_owned()));
        Ok(b)
    }

    fn save(&self, name: &str, bundle: &Bundle) -> Result<PathBuf, BundleError> {
        self.saved.lock().unwrap_or_else(PoisonError::into_inner).insert(name.to_owned(), bundle.clone());
        Ok(PathBuf::from(format!("{name}.toml")))
    }

    fn delete(&self, name: &str) -> Result<(), BundleError> {
        self.saved
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(name)
            .map(drop)
            .ok_or_else(|| BundleError::NotFound(format!("no profile '{name}'")))
    }
}

/// A scripted updater.
#[derive(Debug)]
pub struct FakeUpdates {
    /// What `check` answers.
    pub status: Mutex<UpdateStatus>,
    /// The options of every `apply`.
    pub applied: Mutex<Vec<ApplyOptions>>,
    /// The exit code the fake installer returns.
    pub returncode: Mutex<i32>,
    notified: Mutex<Option<String>>,
}

impl FakeUpdates {
    /// An updater whose newest release is `latest` while `current` runs.
    pub fn new(current: &str, latest: &str) -> Arc<Self> {
        let info = Info {
            version: latest.to_owned(),
            tag: format!("v{latest}"),
            url: format!("https://example.invalid/releases/v{latest}"),
            notes: String::new(),
        };
        let status = UpdateStatus {
            current: current.to_owned(),
            latest: Some(latest.to_owned()),
            available: latest != current,
            checked_at: 0.0,
            error: None,
            info: Some(info),
        };
        Arc::new(Self {
            status: Mutex::new(status),
            applied: Mutex::new(Vec::new()),
            returncode: Mutex::new(0),
            notified: Mutex::new(None),
        })
    }
}

impl UpdateService for FakeUpdates {
    fn repo(&self) -> String {
        "example/truesense".to_owned()
    }

    fn check(&self, _force: bool, _hours: Option<u64>) -> UpdateStatus {
        self.status.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn apply(&self, options: &ApplyOptions) -> Result<ApplyResult, UpdateError> {
        self.applied.lock().unwrap_or_else(PoisonError::into_inner).push(options.clone());
        let tag = options.tag.clone().unwrap_or_else(|| "v0.0.0".to_owned());
        Ok(ApplyResult {
            version: tag.trim_start_matches('v').to_owned(),
            sha256: "0123456789abcdef0123456789abcdef".to_owned(),
            verified: Some(true),
            returncode: *self.returncode.lock().unwrap_or_else(PoisonError::into_inner),
            via: "installer",
            installed: None,
        })
    }

    fn notified(&self) -> Option<String> {
        self.notified.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn mark_notified(&self, version: &str) {
        *self.notified.lock().unwrap_or_else(PoisonError::into_inner) = Some(version.to_owned());
    }
}

/// A service manager with fixed answers.
#[derive(Debug, Default)]
pub struct FakeServices {
    /// The rows `check` answers.
    pub units: Mutex<Vec<Unit>>,
}

impl ServiceChecks for FakeServices {
    fn check(&self) -> Vec<Unit> {
        self.units.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn repair_system(&self) -> Vec<Step> {
        vec![Step { what: "daemon restarted".to_owned(), ok: true }]
    }

    fn repair_user(&self) -> Vec<Step> {
        vec![Step { what: "user services restarted".to_owned(), ok: true }]
    }
}

/// A runner that starts nothing: every program "is not installed".
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct NullRunner;

impl CommandRunner for NullRunner {
    fn output(&self, program: &str, _args: &[&str], _timeout: Duration) -> io::Result<Vec<u8>> {
        Err(io::Error::new(io::ErrorKind::NotFound, format!("{program}: not started in tests")))
    }
}

impl Runner for NullRunner {
    fn run(&self, program: &str, _args: &[&str]) -> Output {
        Output::not_runnable(format!("{program}: not started in tests"))
    }
}

/// A clock for the sound system that advances only when something sleeps, so its waits and timeouts
/// are deterministic and instant.
#[derive(Debug, Default)]
struct StepClock(AtomicU64);

impl ts_domain::audio::Clock for StepClock {
    fn now(&self) -> Duration {
        Duration::from_millis(self.0.load(Ordering::SeqCst))
    }

    fn sleep(&self, d: Duration) {
        let millis = u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
        self.0.fetch_add(millis, Ordering::SeqCst);
    }
}

/// An operating system with nobody logged in and nothing installed.
struct EmptyHost;

impl Host for EmptyHost {
    fn env_var(&self, _name: &str) -> Option<String> {
        None
    }

    fn euid(&self) -> u32 {
        1000
    }

    fn lookup_uid(&self, _user: &str) -> Option<u32> {
        None
    }

    fn program_on_path(&self, _name: &str) -> bool {
        false
    }

    fn can_access(&self, _path: &Path, _write: bool) -> bool {
        false
    }

    fn now(&self) -> std::time::SystemTime {
        std::time::UNIX_EPOCH
    }
}

/// The doubles behind a simulated [`Services`], for the test to look at and steer.
pub struct Rig {
    /// The directory everything lives in.
    pub root: PathBuf,
    /// The firmware.
    pub sim: Arc<Simulator>,
    /// The keyboard controller.
    pub hid: SimHid,
    /// The control socket.
    pub daemon: Arc<FakeDaemon>,
    /// The key-action runner.
    pub actions: Arc<FakeActions>,
    /// The on-screen messages.
    pub notifier: Arc<RecordingNotifier>,
    /// The whole-setup profiles.
    pub bundles: Arc<FakeBundles>,
    /// The updater.
    pub updates: Arc<FakeUpdates>,
    /// The service manager.
    pub service_checks: Arc<FakeServices>,
    /// The pauses the code asked for.
    pub sleeps: Arc<Mutex<Vec<Duration>>>,
    /// The wall clock, in whole seconds; advance it with `store`.
    pub clock: Arc<AtomicU64>,
}

impl Services {
    /// A hermetic set of services below `root` (the caller creates it and removes it), with a
    /// running fake daemon. See the module documentation for what is simulated. It fails only if a
    /// built-in profile of the key bindings were invalid.
    pub fn simulated(root: &Path) -> Result<(Services, Rig), super::ApiError> {
        let sim = Arc::new(Simulator::new());
        let no_sleep: Sleeper = Arc::new(|_| {});
        let user = Arc::new(userstate::Store::new(root.join("user.json")));
        let (keyboard, hid) = Keyboard::simulated(Arc::clone(&user));
        let keyboard = Arc::new(keyboard);
        let hw = Hw::new(sim.clone())
            .with_sleeper(no_sleep)
            .with_kb_timeout(Arc::new(keyboard.timeout_flag()))
            .with_rear_memory(Arc::new(RearMemoryInUserState(Arc::clone(&user))));
        let config = ConfigFiles::at(Some(root.join("etc/config.toml")), Some(root.join("home/config.toml")));
        let home = root.join("home");
        let daemon = FakeDaemon::running();
        let actions = Arc::new(FakeActions::default());
        let notifier = Arc::new(RecordingNotifier::default());
        let bundles = Arc::new(FakeBundles::default());
        let updates = FakeUpdates::new(buildinfo::VERSION, buildinfo::VERSION);
        let service_checks = Arc::new(FakeServices::default());
        let sleeps = Arc::new(Mutex::new(Vec::new()));
        let clock = Arc::new(AtomicU64::new(1_700_000_000));
        let now_clock = Arc::clone(&clock);
        let now: ts_system::sensors::Clock = Arc::new(move || now_clock.load(Ordering::SeqCst) as f64);
        let recorded = Arc::clone(&sleeps);
        let sleep: Sleeper =
            Arc::new(move |d| recorded.lock().unwrap_or_else(PoisonError::into_inner).push(d));
        let cfg_for_audio = config.clone();
        let audio = Audio::new(
            Arc::new(NullRunner),
            Arc::new(move || cfg_for_audio.load_or_default()),
            Paths {
                proc_cards: root.join("proc/asound/cards"),
                conf_dir: root.join("pipewire"),
                plugin: None,
                dts_config: root.join("acerultrax.toml"),
                output_lock: root.join("output.lock"),
            },
        )
        .with_dts_locator(Arc::new(|| None))
        .with_logger(Arc::new(|_| {}))
        .with_clock(Arc::new(StepClock::default()))
        .with_state(Arc::clone(&user));
        let sys: Arc<HostSysfs> = Arc::new(HostSysfs::new(root));
        let power_managers = Arc::new(PowerManagers::new(
            Arc::new(NullRunner),
            sys.clone(),
            PathBuf::from("/sys/devices/system/cpu"),
            root.join("powermgr.json"),
        ));
        let probe_root = root.to_path_buf();
        let doctor_root = root.to_path_buf();
        let services = Services {
            daemon: daemon.clone(),
            hw: hw.clone(),
            linux: LinuxSensors::new(root, Arc::new(NullRunner)),
            sampler: Arc::new(Sampler::new(root, Arc::clone(&now))),
            keyboard: Arc::clone(&keyboard),
            user: Arc::clone(&user),
            config,
            keymap: KeyMap::verified(),
            curves: curves::Store::new(home.join("curves")),
            smart: smartprofile::Store::new(home.join("smart")),
            cpu_undervolt: undervolt::Store::new(home.join("undervolt")),
            gpu_undervolt: gpuundervolt::Store::new(home.join("gpu-undervolt")),
            looks: LookStore::new(
                home.join("looks"),
                root.join("boot-look.json"),
                Arc::clone(&user),
                keyboard.clone(),
                Arc::new(hw),
            ),
            lighting: LightingProfiles::new(home.join("lighting"), keyboard.clone()),
            designs: DesignStore::new(home.join("designs")),
            fx: FxStore::new(Arc::clone(&user), keyboard.clone(), Arc::new(|| 1_700_000_000.0)),
            image: None,
            audio: Arc::new(audio),
            power_limits: PowerLimits::new(sys, None, Arc::new(NullRunner)),
            power_managers,
            touchpad: Touchpad::new(root.join("proc/bus/input/devices"), root.join("sys/class/input")),
            actions: actions.clone(),
            key_profiles: KeyProfiles::new(home.join("keys"), home.join("config.toml"), syntax_check())?,
            bundles: bundles.clone(),
            updates: updates.clone(),
            service_checks: Some(service_checks.clone()),
            notifier: notifier.clone(),
            fn_key_node: Arc::new(|| None),
            doctor_env: Arc::new(move |a| {
                let mut env = doctor::Env::system(a.firmware, a.daemon, a.audio, a.probes);
                env.root = doctor_root.clone();
                env.runner = Arc::new(NullRunner);
                env.host = Arc::new(EmptyHost);
                env.config = Arc::new(|| Ok(ts_foundation::config::Config::default()));
                env.kernel = "test".to_owned();
                env
            }),
            probe: ProbeOptions {
                dmi_root: probe_root.join("dmi"),
                sys_root: probe_root.join("sys"),
                device_path: probe_root.join("dev/truesense"),
                kernel: "test".to_owned(),
                version: buildinfo::VERSION.to_owned(),
            },
            root: root.to_path_buf(),
            autostart_dir: Some(home.join("autostart")),
            euid: 1000,
            now,
            sleep,
        };
        let rig = Rig {
            root: root.to_path_buf(),
            sim,
            hid,
            daemon,
            actions,
            notifier,
            bundles,
            updates,
            service_checks,
            sleeps,
            clock,
        };
        Ok((services, rig))
    }
}

impl Services {
    /// The same services without any on-screen messages and without a service manager, for a
    /// client test that only needs the data operations.
    pub fn without_desktop(mut self) -> Self {
        self.notifier = Arc::new(NoNotifier);
        self.service_checks = None;
        self
    }
}

/// A `state` answer of a running daemon, for [`FakeDaemon::answer_ok`].
pub fn daemon_state(settings: Value, live: Value, hold_left_s: i64) -> Value {
    json!({"live": live, "settings": settings, "hold_left_s": hold_left_s})
}
