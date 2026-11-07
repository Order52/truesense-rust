//! The fake sound system the audio tests run against: `pactl`, `wpctl`, `amixer`, `systemctl` and
//! `acerultrax-cli` as they behaved on the PH16-71, with enough state for the logic above them to
//! be driven end to end. It records every command so that tests can assert exactly what was sent,
//! and it is the ONLY thing the audio tests ever run: no real program is started.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use ts_foundation::config::Config;
use ts_foundation::userstate;
use ts_platform::hw::CommandRunner;

use super::{Audio, Clock, Osd, Paths, Quiet};

pub(crate) const CARD: &str = "alsa_card.pci-0000_00_1f.3-platform-skl_hda_dsp_generic";
pub(crate) const HW_SPEAKER: &str =
    "alsa_output.pci-0000_00_1f.3-platform-skl_hda_dsp_generic.HiFi__Speaker__sink";
pub(crate) const HW_HEADPHONES: &str =
    "alsa_output.pci-0000_00_1f.3-platform-skl_hda_dsp_generic.HiFi__Headphones__sink";
pub(crate) const SRC_ARRAY: &str =
    "alsa_input.pci-0000_00_1f.3-platform-skl_hda_dsp_generic.HiFi__Mic1__source";
pub(crate) const SRC_HEADSET: &str =
    "alsa_input.pci-0000_00_1f.3-platform-skl_hda_dsp_generic.HiFi__Mic2__source";
pub(crate) const HIFI_SPEAKERS: &str = "HiFi (Mic1, Mic2, Speaker)";
pub(crate) const HIFI_HEADPHONES: &str = "HiFi (Headphones, Mic1, Mic2)";
pub(crate) const USB_HEADSET: &str = "alsa_output.usb-GeneralPlus_USB_Audio_Device-00.analog-stereo";
pub(crate) const DUALSENSE: &str = "alsa_output.usb-Sony_DualSense-00.Direct__Direct__sink";

/// `acerultrax-cli devices` as the real helper prints it (after its output is stripped).
pub(crate) const REAL_DEVICES: &str = "  Laptop speakers                                    speakers           Music\n    @speakers\n  Headphones (3.5 mm jack)                           headphones         Music\n    @headphones\n  DualSense wireless controller (PS5) Direct DualSense Wireless Controller other              Plain\n    alsa_output.usb-Sony_DualSense-00.Direct__Direct__sink\n* Headset PC38X                                      other              Plain\n    alsa_output.usb-GeneralPlus_USB_Audio_Device-00.analog-stereo\n  Some Speaker                                       bluetooth-speaker  Plain\n    bluez_output.AA.1\n";

pub(crate) struct State {
    pub calls: Vec<Vec<String>>,
    // card
    pub card_present: bool,
    pub offered: Vec<String>,
    pub profile: String,
    pub sink_up: bool,
    pub restart_fixes: bool,
    pub restart_offers_only: bool,
    pub restarted: u32,
    pub extra_sinks: Vec<String>,
    pub no_front_sink: bool,
    pub card_missing_from_long_list: bool,
    pub default_sink: String,
    pub sink_inputs: Vec<String>,
    pub sink_descs: BTreeMap<String, String>,
    // microphones
    pub default_source: String,
    pub muted: BTreeMap<String, bool>,
    pub volume: String,
    pub source_outputs: Vec<String>,
    pub extra_sources: Vec<String>,
    pub conf_dir: Option<PathBuf>,
    // ALSA
    pub headphone_jack: String,
    pub headset_mic_jack: String,
    pub boost: u8,
    pub boost_broken: bool,
    pub no_boost_control: bool,
    pub no_jack_controls: bool,
    // AcerUltraX
    pub dts_devices_text: Option<String>,
    pub dts_profile: BTreeMap<String, String>,
    pub dts_eq: BTreeSet<String>,
    pub dts_fail: BTreeSet<String>,
    pub dts_config: Option<PathBuf>,
}

pub(crate) struct FakeSystem {
    state: Mutex<State>,
}

impl FakeSystem {
    fn new(broken: bool) -> Self {
        let offered = if broken {
            vec!["off".to_owned(), "pro-audio".to_owned()]
        } else {
            ["off", HIFI_HEADPHONES, HIFI_SPEAKERS, "pro-audio"].map(str::to_owned).to_vec()
        };
        Self {
            state: Mutex::new(State {
                calls: Vec::new(),
                card_present: true,
                offered,
                profile: if broken { "off".into() } else { HIFI_SPEAKERS.into() },
                sink_up: !broken,
                restart_fixes: true,
                restart_offers_only: false,
                restarted: 0,
                extra_sinks: Vec::new(),
                no_front_sink: false,
                card_missing_from_long_list: false,
                default_sink: "acerultrax_sink".into(),
                sink_inputs: Vec::new(),
                sink_descs: BTreeMap::new(),
                default_source: SRC_ARRAY.into(),
                muted: BTreeMap::new(),
                volume: "Volume: front-left: 65536 / 100% / 0.00 dB,   front-right: 65536 / 100% / 0.00 dB\n        balance 0.00\n".into(),
                source_outputs: Vec::new(),
                extra_sources: Vec::new(),
                conf_dir: None,
                headphone_jack: "off".into(),
                headset_mic_jack: "off".into(),
                boost: 1,
                boost_broken: false,
                no_boost_control: false,
                no_jack_controls: false,
                dts_devices_text: None,
                dts_profile: BTreeMap::new(),
                dts_eq: BTreeSet::new(),
                dts_fail: BTreeSet::new(),
                dts_config: None,
            }),
        }
    }

    /// The fake's state; never hold the guard across a call into the audio code.
    pub fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    pub fn clear(&self) {
        self.state().calls.clear();
    }

    /// Every recorded command as one string each.
    pub fn sent(&self) -> Vec<String> {
        self.state().calls.iter().map(|c| c.join(" ")).collect()
    }

    /// Whether a command that starts with these words was sent.
    pub fn did(&self, prefix: &[&str]) -> bool {
        self.count(prefix) > 0
    }

    pub fn count(&self, prefix: &[&str]) -> usize {
        self.state()
            .calls
            .iter()
            .filter(|c| c.len() >= prefix.len() && c.iter().zip(prefix).all(|(a, b)| a == b))
            .count()
    }

    fn handle(&self, program: &str, args: &[String]) -> io::Result<Vec<u8>> {
        let mut s = self.state();
        let mut call = vec![program.to_owned()];
        call.extend(args.iter().cloned());
        s.calls.push(call);
        let words: Vec<&str> = args.iter().map(String::as_str).collect();
        let text = if program.ends_with("acerultrax-cli") {
            return s.acerultrax(&words);
        } else {
            match program {
                "pactl" => s.pactl(&words),
                "wpctl" => s.wpctl(&words),
                "systemctl" => s.systemctl(&words),
                "amixer" => s.amixer(&words),
                _ => String::new(),
            }
        };
        Ok(text.into_bytes())
    }
}

impl State {
    fn sinks(&self) -> Vec<String> {
        let mut out = if self.no_front_sink { Vec::new() } else { vec!["acerultrax_sink".to_owned()] };
        if self.sink_up {
            out.push(if self.profile == HIFI_HEADPHONES { HW_HEADPHONES } else { HW_SPEAKER }.to_owned());
        }
        out.extend(self.extra_sinks.iter().cloned());
        out
    }

    fn sources(&self) -> Vec<String> {
        let mut out = vec![SRC_ARRAY.to_owned(), SRC_HEADSET.to_owned(), format!("{HW_SPEAKER}.monitor")];
        out.extend(self.extra_sources.iter().cloned());
        if let Some(dir) = &self.conf_dir {
            for dev in ["headset", "array"] {
                if dir.join(format!("truesense-pv-{dev}.conf")).exists() {
                    out.push(format!("truesense_pv_{dev}"));
                }
            }
        }
        out.sort();
        out
    }

    fn pactl(&mut self, args: &[&str]) -> String {
        match args {
            ["list", "short", "cards"] => {
                if self.card_present { format!("70\talsa_card.usb-Other\talsa\n71\t{CARD}\talsa\n") } else { String::new() }
            }
            ["list", "cards"] if self.card_missing_from_long_list => {
                "Card #70\n\tName: alsa_card.usb-Other\n\tProfiles:\n\t\toff: Off (sinks: 0, sources: 0, priority: 0, available: yes)\n\tActive Profile: off\n".into()
            }
            ["list", "cards"] => {
                let profiles: String = self
                    .offered
                    .iter()
                    .map(|p| format!("\t\t{p}: {p} (sinks: 1, sources: 2, priority: 1, available: yes)\n"))
                    .collect();
                format!(
                    "Card #70\n\tName: alsa_card.usb-Other\n\tProfiles:\n\t\toff: Off (sinks: 0, sources: 0, priority: 0, available: yes)\n\tActive Profile: off\n\tPorts:\n\nCard #71\n\tName: {CARD}\n\tDriver: alsa\n\tProfiles:\n{profiles}\tActive Profile: {}\n\tPorts:\n\t\tanalog-output-speaker: Speaker (type: Speaker)\n",
                    self.profile
                )
            }
            ["list", "short", "sinks"] => {
                self.sinks().iter().enumerate().map(|(i, s)| format!("{}\t{s}\tPipeWire\ts32le 2ch 48000Hz\tSUSPENDED\n", 30 + i)).collect()
            }
            ["list", "sinks"] => self
                .sinks()
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let desc = self.sink_descs.get(s).cloned().unwrap_or_else(|| format!("Sink {s}"));
                    format!("Sink #{}\n\tState: SUSPENDED\n\tName: {s}\n\tDescription: {desc}\n\tDriver: PipeWire\n", 30 + i)
                })
                .collect(),
            ["list", "short", "sources"] => {
                self.sources().iter().enumerate().map(|(i, s)| format!("{}\t{s}\tPipeWire\ts32le 2ch 48000Hz\tSUSPENDED\n", 40 + i)).collect()
            }
            ["list", "short", "sink-inputs"] => self.sink_inputs.join("\n"),
            ["list", "short", "source-outputs"] => self.source_outputs.join("\n"),
            ["get-default-sink"] => format!("{}\n", self.default_sink),
            ["get-default-source"] => format!("{}\n", self.default_source),
            ["set-default-sink", name] => {
                self.default_sink = (*name).to_owned();
                String::new()
            }
            ["set-default-source", name] => {
                self.default_source = (*name).to_owned();
                String::new()
            }
            ["set-card-profile", _, profile] => {
                if self.offered.iter().any(|p| p == profile) {
                    self.profile = (*profile).to_owned();
                    self.sink_up = *profile == HIFI_SPEAKERS || *profile == HIFI_HEADPHONES;
                }
                String::new()
            }
            ["get-source-mute", name] => {
                if self.muted.get(*name).copied().unwrap_or(false) { "Mute: yes\n".into() } else { "Mute: no\n".into() }
            }
            ["set-source-mute", name, value] => {
                self.muted.insert((*name).to_owned(), *value == "1");
                String::new()
            }
            ["get-source-volume", _] => self.volume.clone(),
            _ => String::new(),
        }
    }

    fn wpctl(&mut self, args: &[&str]) -> String {
        if let ["set-volume", _, value] = args {
            if let Some(pct) = value.strip_suffix('%').and_then(|v| v.parse::<u32>().ok()) {
                self.volume = format!(
                    "Volume: front-left: {} / {pct}% / -3.00 dB,   front-right: {} / {pct}% / -3.00 dB\n",
                    pct * 655,
                    pct * 655
                );
            }
        }
        String::new()
    }

    fn systemctl(&mut self, args: &[&str]) -> String {
        if args == ["--user", "restart", "wireplumber.service"] {
            self.restarted += 1;
            if self.restart_fixes {
                self.offered =
                    ["off", HIFI_HEADPHONES, HIFI_SPEAKERS, "pro-audio"].map(str::to_owned).to_vec();
                self.profile = HIFI_SPEAKERS.into();
                self.sink_up = true;
            } else if self.restart_offers_only {
                self.offered =
                    ["off", HIFI_HEADPHONES, HIFI_SPEAKERS, "pro-audio"].map(str::to_owned).to_vec();
            }
        }
        String::new()
    }

    fn amixer(&mut self, args: &[&str]) -> String {
        let args: &[&str] = if args.first().is_some_and(|a| a.starts_with("-c")) { &args[1..] } else { args };
        let [verb, control, rest @ ..] = args else { return String::new() };
        match *verb {
            "cget" if control.contains("name=Headphone Jack") && !self.no_jack_controls => format!(
                "numid=33,iface=CARD,name='Headphone Jack'\n  ; type=BOOLEAN,access=r-------,values=1\n  : values={}\n",
                self.headphone_jack
            ),
            "cget" if control.contains("name=Headset Mic Jack") && !self.no_jack_controls => format!(
                "numid=34,iface=CARD,name='Headset Mic Jack'\n  ; type=BOOLEAN,access=r-------,values=1\n  : values={}\n",
                self.headset_mic_jack
            ),
            "cget" if control.contains("name=Headset Mic Boost Volume") && !self.no_boost_control => format!(
                "numid=20,iface=MIXER,name='Headset Mic Boost Volume'\n  ; type=INTEGER,access=rw---R--,values=2,min=0,max=3,step=0\n  : values={0},{0}\n  | dBscale-min=0.00dB,step=10.00dB,mute=0\n",
                self.boost
            ),
            "cset" => {
                if control.contains("Headset Mic Boost Volume") && !self.boost_broken {
                    if let Some(level) =
                        rest.first().and_then(|v| v.split(',').next()).and_then(|v| v.parse().ok())
                    {
                        self.boost = level;
                    }
                }
                "numid=20\n".into()
            }
            _ => String::new(),
        }
    }

    fn acerultrax(&mut self, args: &[&str]) -> io::Result<Vec<u8>> {
        let Some(verb) = args.first() else { return Err(io::Error::other("usage")) };
        if self.dts_fail.contains(*verb) {
            return Err(io::Error::other(format!("acerultrax-cli {verb} failed")));
        }
        let device =
            args.iter().position(|a| *a == "--device").and_then(|i| args.get(i + 1)).map(|d| (*d).to_owned());
        match *verb {
            "devices" => {
                let mut text = self.dts_devices_text.clone().unwrap_or_else(|| REAL_DEVICES.to_owned());
                for (reference, profile) in &self.dts_profile {
                    text = text.replacen(
                        &format!("Plain\n    {reference}"),
                        &format!("{profile}\n    {reference}"),
                        1,
                    );
                }
                Ok(format!("{}\n", text.trim()).into_bytes())
            }
            "mode" => {
                if let (Some(device), Some(name)) = (device, args.get(1)) {
                    let mut chars = name.chars();
                    let capitalised: String = chars
                        .next()
                        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
                        .unwrap_or_default();
                    self.dts_profile.insert(device, capitalised);
                }
                Ok(b"ok\n".to_vec())
            }
            "headphone" => {
                if let (Some(device), Some(&"default")) = (device, args.get(1)) {
                    // AcerUltraX keeps the EQ in its config file; the audio code reads it from there.
                    if let Some(path) = &self.dts_config {
                        let mut text = std::fs::read_to_string(path).unwrap_or_default();
                        text.push_str(&format!("[headphones.\"{device}\"]\n"));
                        std::fs::write(path, text).unwrap();
                    }
                    self.dts_eq.insert(device);
                }
                Ok(b"ok\n".to_vec())
            }
            _ => Ok(b"ok\n".to_vec()),
        }
    }
}

impl CommandRunner for FakeSystem {
    fn output(&self, program: &str, args: &[&str], _timeout: Duration) -> io::Result<Vec<u8>> {
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        self.handle(program, &args)
    }
}

type SleepHook = Arc<dyn Fn(Duration) + Send + Sync>;

/// Time that advances only when something sleeps, so timeouts and waits are deterministic.
#[derive(Default)]
pub(crate) struct FakeClock {
    millis: AtomicU64,
    pub sleeps: Mutex<Vec<Duration>>,
    hook: Mutex<Option<SleepHook>>,
}

impl FakeClock {
    /// Calls `hook` after every sleep (on whichever thread slept), for tests that must stop a loop.
    pub fn on_sleep(&self, hook: impl Fn(Duration) + Send + Sync + 'static) {
        *self.hook.lock().unwrap() = Some(Arc::new(hook));
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Duration {
        Duration::from_millis(self.millis.load(Ordering::SeqCst))
    }

    fn sleep(&self, d: Duration) {
        self.millis.fetch_add(u64::try_from(d.as_millis()).unwrap_or(u64::MAX), Ordering::SeqCst);
        self.sleeps.lock().unwrap().push(d);
        let hook = self.hook.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook(d);
        }
    }
}

/// An OSD that records what it was asked to show and how deeply `quiet` guards nested.
#[derive(Default)]
pub(crate) struct FakeOsd {
    pub shown: Mutex<Vec<String>>,
    depth: Arc<AtomicU64>,
    pub max_depth: AtomicU64,
}

impl FakeOsd {
    /// How many `quiet` guards are alive now.
    pub fn quiet_depth(&self) -> u64 {
        self.depth.load(Ordering::SeqCst)
    }
}

impl Osd for FakeOsd {
    fn show(&self, text: &str, icon: &str) {
        self.shown.lock().unwrap().push(format!("{icon}: {text}"));
    }

    fn quiet(&self) -> Quiet {
        let depth = self.depth.clone();
        let now = depth.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_depth.fetch_max(now, Ordering::SeqCst);
        Quiet::new(move || {
            depth.fetch_sub(1, Ordering::SeqCst);
        })
    }
}

/// An audio system wired to the fakes, with a temporary directory for its files.
pub(crate) struct Rig {
    pub audio: Arc<Audio>,
    pub sys: Arc<FakeSystem>,
    pub clock: Arc<FakeClock>,
    pub osd: Arc<FakeOsd>,
    pub dir: PathBuf,
    config: Arc<Mutex<Config>>,
    pub env: Arc<Mutex<BTreeMap<String, String>>>,
    pub logs: Arc<Mutex<Vec<String>>>,
    pub dts_installed: Arc<AtomicBool>,
}

impl Rig {
    pub fn new(broken: bool) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ts-domain-audio-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("cards"),
            " 0 [sofhdadsp     ]: sof-hda-dsp - sof-hda-dsp\n                      Intel Corp.-Predator PH16-71\n 1 [Other          ]: USB-Audio - Other\n",
        )
        .unwrap();
        let sys = Arc::new(FakeSystem::new(broken));
        sys.state().conf_dir = Some(dir.join("pw"));
        sys.state().dts_config = Some(dir.join("acerultrax.toml"));
        let clock = Arc::new(FakeClock::default());
        let osd = Arc::new(FakeOsd::default());
        let config = Arc::new(Mutex::new(Config::default()));
        let env = Arc::new(Mutex::new(BTreeMap::new()));
        let logs = Arc::new(Mutex::new(Vec::new()));
        let dts_installed = Arc::new(AtomicBool::new(true));
        let paths = Paths {
            proc_cards: dir.join("cards"),
            conf_dir: dir.join("pw"),
            plugin: Some(PathBuf::from("/usr/lib/ladspa/librnnoise_ladspa.so")),
            dts_config: dir.join("acerultrax.toml"),
            output_lock: dir.join("output.lock"),
        };
        let (c, e, l, d) = (config.clone(), env.clone(), logs.clone(), dts_installed.clone());
        let audio = Audio::new(sys.clone(), Arc::new(move || c.lock().unwrap().clone()), paths)
            .with_osd(osd.clone())
            .with_state(Arc::new(userstate::Store::new(dir.join("user.json"))))
            .with_clock(clock.clone())
            .with_env(Arc::new(move |k| e.lock().unwrap().get(k).cloned()))
            .with_logger(Arc::new(move |line| l.lock().unwrap().push(line.to_owned())))
            .with_dts_locator(Arc::new(move || {
                d.load(Ordering::SeqCst).then(|| PathBuf::from("/x/acerultrax-cli"))
            }));
        Self { audio: Arc::new(audio), sys, clock, osd, dir, config, env, logs, dts_installed }
    }

    /// The files the audio code uses, for a test that changes one (before any other holder of the
    /// audio value exists).
    pub fn paths_mut(&mut self) -> &mut Paths {
        &mut Arc::get_mut(&mut self.audio).unwrap().paths
    }

    /// The configuration the audio code reads; changes apply at once, like a config edit.
    pub fn config(&self) -> MutexGuard<'_, Config> {
        self.config.lock().unwrap()
    }

    pub fn shown(&self) -> Vec<String> {
        self.osd.shown.lock().unwrap().clone()
    }

    pub fn log_lines(&self) -> Vec<String> {
        self.logs.lock().unwrap().clone()
    }

    /// The text of a file below the rig's directory.
    pub fn read(&self, relative: &str) -> Option<String> {
        std::fs::read_to_string(self.dir.join(relative)).ok()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
