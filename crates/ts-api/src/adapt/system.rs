//! The real [`Services`]: every concrete component a user-run client needs, built once from a
//! [`Machine`].
//!
//! The command line, the terminal UI, the window and the key service all start from
//! [`Services::system`]. Nothing below this layer knows about the others, so this is the one place
//! that decides which real type stands behind each field of [`Services`], which directories the
//! stores live in, and how the pieces that depend on each other are tied together:
//!
//! * the action runner reaches the laptop through [`ApiFacade`], which is an `Api` of its own
//!   (see the `facade` module for why), and the whole-setup profiles apply their settings by
//!   running actions while the `profile:` action applies profiles: a `RunnerSlot` breaks that
//!   cycle;
//! * the key bindings are validated with the runner's validator, which knows the saved looks;
//! * the sound system shows its messages through the same on-screen display as everything else.
//!
//! The daemon's own assembly (`ts_daemon::assembly`) builds the root service the same way; what is
//! different here is that nothing needs root and every store lives in the home directory.
#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::sync::Arc;

use ts_decision::{curves, smartprofile};
use ts_domain::actions::{
    ActionRunner, ActionSettings, ActionState, Env as ActionEnv, HostError, HostResult, LocalTouchpad,
    ProfilesFacade, Runner, RunnerSlot,
};
use ts_domain::audio::{Audio, Paths};
use ts_domain::designs::{self, DesignStore};
use ts_domain::gpuundervolt;
use ts_domain::keybinds::{self, KeyProfiles};
use ts_domain::keyboard::{HidOpener, HidrawOpener, Keyboard, SimHid};
use ts_domain::keymap::KeyMap;
use ts_domain::keyprofiles::{self, ProfileStore as LightingProfiles};
use ts_domain::looks::{self, LookStore};
use ts_domain::perkeyfx::FxStore;
use ts_domain::power::{HostSysfs, MsrReader, PowerLimits};
use ts_domain::powermgr::{self, PowerManagers};
use ts_domain::profiles::{
    Bundle, BundleError, BundleFiles, BundleHost, BundleSource, BundleStore, default_sys_dirs,
};
use ts_domain::undervolt::{self, DevMsr};
use ts_foundation::config::{self, SYSTEM_PATH};
use ts_foundation::{buildinfo, userstate};
use ts_ipc::client::Client;
use ts_ipc::wire;
use ts_platform::firmware::DeviceKind;
use ts_platform::hw::{Hw, LinuxSensors, Sleeper};
use ts_system::models::ProbeOptions;
use ts_system::osd::{Osd, PasswdFile, ProcessSessionRunner};
use ts_system::sensors::Sampler;
use ts_system::services::{Manager, Pinger};
use ts_system::touchpad::Touchpad;
use ts_system::update::{CurlFetcher, DEFAULT_REPO, SystemHost, Updater, system_repo};

use super::facade::{ApiFacade, DirectTouchpad, NoActions, ShellSpawner};
use super::machine::Machine;
use super::osd::LiveOsd;
use crate::api::{
    Api, ApiError, ApiResult, ConfigFiles, Daemon, ProfileBundles, RearMemoryInUserState, Services,
    UpdateService,
};

/// Where every store keeps its files below the home directory: the place the Python edition used,
/// so the editions can replace each other without moving anything.
const CONFIG_DIR: &str = ".config/truesense";

/// What every `Services` of one process shares: the firmware, the keyboard, the control socket,
/// the sound system and the memory files. Cloning a handle gives the same object, never a second
/// device, so the private `Api` of the actions and the public one cannot disagree.
struct Shared {
    user: Arc<userstate::Store>,
    hw: Hw,
    keyboard: Arc<Keyboard>,
    daemon: Arc<dyn Daemon>,
    sampler: Arc<Sampler>,
    audio: Arc<Audio>,
    notifier: LiveOsd,
    power_managers: Arc<PowerManagers>,
    updates: Arc<dyn UpdateService>,
    service_checks: Arc<Manager>,
}

/// Pings the daemon for the service manager (`true` only for a `pong`).
struct SocketPing(Client);

impl Pinger for SocketPing {
    fn ping(&self) -> bool {
        self.0.ping()
    }
}

/// What the private `Api` of the actions is given as its profiles: none, because no method it
/// uses touches them.
struct NoBundles;

impl ProfileBundles for NoBundles {
    fn names(&self) -> Vec<String> {
        Vec::new()
    }

    fn source(&self, _name: &str) -> Option<BundleSource> {
        None
    }

    fn load(&self, _name: &str) -> Result<Bundle, BundleError> {
        Err(BundleError::NoProfiles)
    }

    fn apply(&self, _name: &str) -> Result<String, BundleError> {
        Err(BundleError::NoProfiles)
    }

    fn next_name(&self) -> Result<String, BundleError> {
        Err(BundleError::NoProfiles)
    }

    fn capture(&self, _description: &str) -> HostResult<Bundle> {
        Err(HostError::new("profiles are not available here"))
    }

    fn save(&self, _name: &str, _bundle: &Bundle) -> Result<PathBuf, BundleError> {
        Err(BundleError::NoProfiles)
    }

    fn delete(&self, _name: &str) -> Result<(), BundleError> {
        Err(BundleError::NoProfiles)
    }
}

/// Where the action runner reads its settings from: the configuration as it is on every action,
/// so an edit of the file applies to the next key press without a restart.
fn settings_source(config: ConfigFiles) -> impl Fn() -> HostResult<ActionSettings> + Send + Sync + 'static {
    move || Ok(ActionSettings::from_config(&config.load_or_default()))
}

impl Services {
    /// The services of the real machine (or of the simulator under `TRUESENSE_SIM`): the daemon
    /// socket from `TRUESENSE_SOCKET`, the kernel device, the stores in the home directory, the
    /// real sound system, power limits, service manager and updater.
    ///
    /// It fails only if a built-in key profile were invalid, which the tests of the key bindings
    /// rule out.
    pub fn system() -> ApiResult<Services> {
        Self::assemble(&Machine::real())
    }

    /// The same assembly over any [`Machine`], which is how it is tested.
    pub fn assemble(machine: &Machine) -> ApiResult<Services> {
        let shared = shared(machine)?;

        // The private `Api` the actions run on: the same hardware, a runner that refuses and no
        // profiles. Built first because the runner needs it.
        let private = Arc::new(Api::new(services(
            machine,
            &shared,
            Arc::new(NoActions),
            Arc::new(NoBundles),
            keybinds::syntax_check(),
        )?));
        let facade = Arc::new(ApiFacade(private.clone()));
        let config = config_files(machine);

        // The profiles apply settings by running actions, and the `profile:` action applies
        // profiles: the slot is filled once the runner exists.
        let slot = RunnerSlot::new();
        let state: Arc<dyn ActionState> = shared.user.clone();
        let bundles = Arc::new(BundleStore::new(
            BundleFiles::new(
                BundleFiles::default_user_dir(&machine.home()),
                default_sys_dirs(machine.env.as_ref()),
            ),
            BundleHost {
                actions: Arc::new(slot.clone()),
                power: facade.clone(),
                daemon: facade.clone(),
                lighting: facade.clone(),
                audio: facade.clone(),
                state: state.clone(),
            },
        ));
        let profiles: Arc<dyn ProfilesFacade> = bundles.clone();
        let local_touchpad: Option<Arc<dyn LocalTouchpad>> = (machine.euid == 0).then(|| {
            Arc::new(DirectTouchpad(Touchpad::new(
                machine.path("/proc/bus/input/devices"),
                machine.path("/sys/class/input"),
            ))) as Arc<dyn LocalTouchpad>
        });
        let runner = Arc::new(
            Runner::new(ActionEnv {
                settings: Arc::new(settings_source(config)),
                power: facade.clone(),
                daemon: facade.clone(),
                lighting: facade.clone(),
                audio: facade,
                state,
                profiles,
                local_touchpad,
                spawner: Some(Arc::new(ShellSpawner)),
            })
            // `favourite:N` and `favourite-next` read the favourites file and start a Smart profile
            // through the same private application layer the other actions use.
            .with_favourites(private),
        );
        slot.set(runner.clone());

        let check = keybinds::check_with(runner.validator().clone());
        services(machine, &shared, runner, bundles, check)
    }
}

/// The configuration files of this machine: the system file below its root, the user's own from
/// the environment.
fn config_files(machine: &Machine) -> ConfigFiles {
    ConfigFiles {
        system: Some(machine.path(SYSTEM_PATH)),
        user: config::user_path_from(machine.env.as_ref()),
        env: Arc::clone(&machine.env),
    }
}

/// Builds the handles every `Services` of this process shares.
fn shared(machine: &Machine) -> ApiResult<Shared> {
    let env = machine.env.as_ref();
    let config = config_files(machine);
    let user = Arc::new(userstate::Store::new(userstate::default_path_from(env, &std::env::temp_dir())));
    let opener: Arc<dyn HidOpener> = if let Some(hid) = &machine.hid {
        Arc::clone(hid)
    } else if machine.simulated() {
        Arc::new(SimHid::shared())
    } else {
        Arc::new(HidrawOpener::with_dirs(machine.path("/sys/class/hidraw"), machine.path("/dev")))
    };
    let keyboard =
        Arc::new(Keyboard::new(opener, Arc::clone(&user)).with_sleeper(Arc::clone(&machine.sleep)));
    let device = machine.device.clone().unwrap_or_else(|| DeviceKind::from_env(env).open());
    // The keyboard's own timeout flag and the rear light's memory are plugged into the hardware
    // layer so a read of either is answered by the part that really knows.
    let hw = Hw::new(device)
        .with_sleeper(Arc::clone(&machine.sleep))
        .with_kb_timeout(Arc::new(keyboard.timeout_flag()))
        .with_rear_memory(Arc::new(RearMemoryInUserState(Arc::clone(&user))));

    let osd_base =
        Osd::new(Arc::new(ProcessSessionRunner), Arc::new(PasswdFile::new(machine.path("/etc/passwd"))))
            .with_root(machine.euid == 0)
            .with_env({
                let lookup = Arc::clone(&machine.env);
                move |key| lookup(key)
            });
    let notifier = LiveOsd::new(osd_base, config.clone());

    let audio_config = config.clone();
    let mut paths = Paths::from_env(env, machine.euid, &|p| machine.path(&p.display().to_string()).exists());
    paths.proc_cards = machine.path("/proc/asound/cards");
    let audio = Arc::new(
        Audio::new(Arc::clone(&machine.runner), Arc::new(move || audio_config.load_or_default()), paths)
            .with_osd(Arc::new(notifier.clone()))
            .with_state(Arc::clone(&user))
            .with_env(Arc::clone(&machine.env))
            .with_dts_locator(Arc::clone(&machine.dts_locator))
            // A client's standard output may be machine-readable (`--json`): the log goes to
            // standard error, which the key and audio services' journal collects as well.
            .with_logger(Arc::new(|line| eprintln!("{line}"))),
    );

    let socket = (machine.env)(wire::SOCKET_ENV);
    let client = Client::new(wire::socket_path_from(socket.as_deref().map(std::ffi::OsStr::new)));
    let sys = Arc::new(HostSysfs::new(machine.root.clone()));
    let power_managers = Arc::new(PowerManagers::new(
        Arc::clone(&machine.runner),
        sys,
        // Absolute: `HostSysfs` resolves it below the machine's root itself.
        PathBuf::from(powermgr::DEFAULT_CPU_DIR),
        powermgr::record_path_from(env),
    ));

    // The repository comes from the root-owned system file only, never from the user's: a file
    // the user can write must not point the updater somewhere else. A broken system file falls
    // back to the built-in repository, which is the safe choice.
    let repo = system_repo(&machine.path(SYSTEM_PATH)).unwrap_or_else(|_| DEFAULT_REPO.to_owned());
    let user_agent = format!("truesense/{}", buildinfo::VERSION);
    let build = |repo: &str| {
        Updater::new(
            repo,
            Arc::new(CurlFetcher::new(Arc::clone(&machine.runner), user_agent.clone())),
            Arc::new(SystemHost::new(Arc::clone(&machine.runner))),
        )
    };
    // `DEFAULT_REPO` is valid, so the second try cannot fail.
    let updater = build(&repo)
        .or_else(|_| build(DEFAULT_REPO))
        .map_err(|e| ApiError::domain("update", e))?
        .with_clock(Arc::clone(&machine.clock))
        .with_cache_path(ts_system::update::default_cache_path(env));
    let updates: Arc<dyn UpdateService> = Arc::new(updater);

    let kernel =
        ts_foundation::fsx::read_trim_opt(&machine.path("/proc/sys/kernel/osrelease")).unwrap_or_default();
    let service_checks = Arc::new(
        Manager::new(Arc::new(SocketPing(client.clone())), kernel)
            .with_runner(Arc::clone(&machine.units))
            .with_sleeper(Arc::clone(&machine.sleep))
            .with_paths(client.path().to_path_buf(), machine.path(ts_platform::firmware::DEVICE_PATH)),
    );

    Ok(Shared {
        user,
        hw,
        keyboard,
        daemon: Arc::new(client),
        sampler: Arc::new(Sampler::new(machine.root.clone(), Arc::clone(&machine.clock))),
        audio,
        notifier,
        power_managers,
        updates,
        service_checks,
    })
}

/// One `Services`: the shared handles plus every store, built from the paths of this machine.
/// The stores hold no state beyond their directory, so building them twice is free.
fn services(
    machine: &Machine,
    shared: &Shared,
    actions: Arc<dyn ActionRunner>,
    bundles: Arc<dyn ProfileBundles>,
    key_check: keybinds::ActionCheck,
) -> ApiResult<Services> {
    let env = machine.env.as_ref();
    let home = machine.home();
    let config_dir = home.join(CONFIG_DIR);
    let config = config_files(machine);
    let keyboard = &shared.keyboard;
    let user = &shared.user;
    let sys = Arc::new(HostSysfs::new(machine.root.clone()));
    // Reading the lock bit needs root; for anyone else it reports why it could not, as before.
    let msr: Arc<dyn MsrReader> = Arc::new(DevMsr::new(machine.path("/dev/cpu/0/msr")));
    let xdg_config = env("XDG_CONFIG_HOME");
    let sleep: Sleeper = Arc::clone(&machine.sleep);

    let hw = shared.hw.clone();
    let key_profiles = KeyProfiles::new(
        config_dir.join("keys"),
        config.user.clone().unwrap_or_else(|| config_dir.join("config.toml")),
        key_check,
    )
    .map_err(|e| ApiError::domain("key profiles", e))?;
    let doctor_env = Arc::clone(&machine.doctor_env);

    Ok(Services {
        daemon: Arc::clone(&shared.daemon),
        hw: hw.clone(),
        linux: LinuxSensors::new(machine.root.clone(), Arc::clone(&machine.runner)),
        sampler: Arc::clone(&shared.sampler),
        keyboard: Arc::clone(keyboard),
        user: Arc::clone(user),
        config,
        keymap: KeyMap::verified(),
        curves: curves::Store::new(
            curves::Store::default_dir_from(env).unwrap_or_else(|| config_dir.join("curves")),
        ),
        smart: smartprofile::Store::for_home(&home),
        cpu_undervolt: undervolt::Store::new(
            undervolt::default_dir_from(env).unwrap_or_else(|| config_dir.join("undervolt")),
        ),
        gpu_undervolt: gpuundervolt::Store::new(
            gpuundervolt::default_dir_from(env).unwrap_or_else(|| config_dir.join("gpu-undervolt")),
        ),
        looks: LookStore::new(
            looks::default_dir(&home),
            looks::default_boot_file(env),
            Arc::clone(user),
            Arc::clone(keyboard) as _,
            Arc::new(hw),
        ),
        lighting: LightingProfiles::new(keyprofiles::default_dir(&home), Arc::clone(keyboard) as _),
        designs: DesignStore::new(designs::user_dir(xdg_config.as_deref(), &home)),
        fx: FxStore::new(Arc::clone(user), Arc::clone(keyboard) as _, Arc::clone(&machine.clock)),
        // Pictures need a decoder the standard library does not have; `keyboard image` says so.
        image: None,
        audio: Arc::clone(&shared.audio),
        power_limits: PowerLimits::new(sys, Some(msr), Arc::clone(&machine.runner)),
        power_managers: Arc::clone(&shared.power_managers),
        touchpad: Touchpad::new(machine.path("/proc/bus/input/devices"), machine.path("/sys/class/input")),
        actions,
        key_profiles,
        bundles,
        updates: Arc::clone(&shared.updates),
        service_checks: Some(shared.service_checks.clone()),
        notifier: Arc::new(shared.notifier.clone()),
        fn_key_node: super::node::finder(machine),
        doctor_env,
        probe: probe_options(machine),
        root: machine.root.clone(),
        autostart_dir: (machine.env)("HOME").map(|h| PathBuf::from(h).join(".config/autostart")),
        euid: machine.euid,
        now: Arc::clone(&machine.clock),
        sleep,
    })
}

/// Where the model probe looks: the real one below the machine's root.
fn probe_options(machine: &Machine) -> ProbeOptions {
    if machine.root == std::path::Path::new("/") {
        return ProbeOptions::system();
    }
    ProbeOptions {
        dmi_root: machine.path(ts_system::models::DMI_ROOT),
        sys_root: machine.path("/sys"),
        device_path: machine.path(ts_platform::firmware::DEVICE_PATH),
        kernel: ts_foundation::fsx::read_trim_opt(&machine.path("/proc/sys/kernel/osrelease"))
            .unwrap_or_default(),
        version: buildinfo::VERSION.to_owned(),
    }
}
