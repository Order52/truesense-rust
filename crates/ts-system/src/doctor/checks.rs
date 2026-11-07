//! The checks themselves. Each reads through [`Env`] only and returns an [`Outcome`].
//!
//! The list is the Go edition's: the Python edition's checks of the Python interpreter, its
//! bytecode cache and PyQt do not exist in a static binary, and the window and the status bars are
//! checked for what the Rust edition needs (`xdg-open`, `truesense-fast`).

use super::{Check, Env, Outcome, SoundState};
use crate::models::{self, Tier};
use std::fs;
use std::path::Path;

/// The old drivers that fight TrueSense over the fans and power modes.
const LEGACY_MODULES: [&str; 3] = ["acer_wmi", "linuwu_sense", "acer_rear_wmi"];

/// Where DKMS keeps its state; one directory per package.
const DKMS_ROOT: &str = "/var/lib/dkms";

const MODULE: &str = "truesense_wmi";
const DEVICE: &str = "/dev/truesense";
const STATE_FILE: &str = "/run/truesense/state";
const MODPROBE_BLOCK: &str = "/etc/modprobe.d/truesense.conf";

/// How old the daemon's state file may be before the daemon counts as stalled.
const STALL_SECONDS: f64 = 10.0;

/// The checks in the order the report shows them.
pub fn checks() -> Vec<Check> {
    type Entry = (&'static str, fn(&Env) -> Outcome);
    let list: [Entry; 19] = [
        ("Model", model),
        ("Kernel", kernel),
        ("Kernel headers", headers),
        ("DKMS", dkms),
        ("Kernel module", module),
        ("Device", device),
        ("Conflicts", conflicts),
        ("Power managers", power_managers),
        ("CPU turbo", turbo),
        ("Daemon", daemon),
        ("Service version", service_version),
        ("User services", user_services),
        ("udev / keyboard", udev),
        ("Config", config),
        ("Secure Boot", secure_boot),
        ("Audio", audio),
        ("Fans", fans),
        ("Window", window),
        ("Status bars", status_bars),
    ];
    list.into_iter().map(|(title, run)| Check::new(title, run)).collect()
}

// ---- helpers ----------------------------------------------------------------------------------

impl Env {
    fn loaded(&self, module: &str) -> bool {
        self.path(&format!("/sys/module/{module}")).is_dir()
    }

    /// Runs a program and returns its trimmed standard output (empty when it could not run).
    fn sh(&self, program: &str, args: &[&str]) -> String {
        self.runner.run(program, args).stdout.trim().to_owned()
    }

    /// Wraps a user-session command for the person who invoked sudo: a root doctor would
    /// otherwise inspect root's empty session. An unknown `SUDO_USER` leaves the command alone
    /// (the doctor then inspects its own session rather than failing the check).
    pub(super) fn as_user(&self, cmd: Vec<String>) -> Vec<String> {
        let user = self.host.env_var("SUDO_USER").unwrap_or_default();
        if self.host.euid() != 0 || user.is_empty() || user == "root" {
            return cmd;
        }
        let Some(uid) = self.host.lookup_uid(&user) else { return cmd };
        let run = format!("/run/user/{uid}");
        let mut wrapped: Vec<String> = ["runuser", "-u", &user, "--", "env"].map(str::to_owned).into();
        wrapped.push(format!("XDG_RUNTIME_DIR={run}"));
        wrapped.push(format!("DBUS_SESSION_BUS_ADDRESS=unix:path={run}/bus"));
        wrapped.extend(cmd);
        wrapped
    }

    /// Whether a systemd unit is active, in the system or in the user's session.
    pub(crate) fn active(&self, unit: &str, user: bool) -> bool {
        let cmd: Vec<String> = if user {
            self.as_user(["systemctl", "--user", "is-active", unit].map(str::to_owned).into())
        } else {
            ["systemctl", "is-active", unit].map(str::to_owned).into()
        };
        let args: Vec<&str> = cmd[1..].iter().map(String::as_str).collect();
        self.sh(&cmd[0], &args) == "active"
    }

    /// Whether a program of this name can be found.
    fn has_program(&self, name: &str) -> bool {
        self.host.program_on_path(name)
    }
}

fn on_off<'a>(flag: bool, on: &'a str, off: &'a str) -> &'a str {
    if flag { on } else { off }
}

fn limit_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

fn read_trim(path: &Path) -> Option<String> {
    ts_foundation::fsx::read_trim_opt(path)
}

// ---- checks -----------------------------------------------------------------------------------

fn model(env: &Env) -> Outcome {
    let m = models::identify(&env.path(models::DMI_ROOT));
    match m.tier {
        Tier::Supported if m.bios_verified == Some(true) => Outcome::ok(m.message),
        Tier::Supported => Outcome::info(m.message, ""),
        Tier::Unverified => Outcome::warn(
            m.message,
            "truesense probe   (read-only) and share the report; the kernel module does not bind on an unverified model unless forced",
        ),
        Tier::Unsupported => Outcome::fail(
            m.message,
            "TrueSense is for Acer Predator laptops; the kernel module refuses to bind elsewhere",
        ),
    }
}

/// The major and minor number at the start of a kernel release (`6.18.1-lts` is 6, 18).
pub(super) fn parse_release(release: &str) -> Option<(u64, u64)> {
    let (major, rest) = release.split_once('.')?;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let major_ok = !major.is_empty() && major.bytes().all(|b| b.is_ascii_digit());
    if !major_ok || digits == 0 {
        return None;
    }
    Some((major.parse().ok()?, rest[..digits].parse().ok()?))
}

fn kernel(env: &Env) -> Outcome {
    const FIX: &str = "kernels older than 6.6 are untested";
    match parse_release(&env.kernel) {
        None => Outcome::warn(format!("kernel {:?} is not a release number I understand", env.kernel), FIX),
        Some(release) => Outcome::graded(release >= (6, 6), format!("kernel {}", env.kernel), FIX),
    }
}

fn headers(env: &Env) -> Outcome {
    let present = env.path(&format!("/lib/modules/{}/build", env.kernel)).exists();
    let detail = format!("kernel headers for {}: {}", env.kernel, on_off(present, "present", "MISSING"));
    if present {
        return Outcome::ok(detail);
    }
    Outcome::fail(
        detail,
        "install your kernel's -headers package (e.g. linux-cachyos-headers), then: sudo dkms autoinstall",
    )
}

/// The kernel release of a DKMS link name `kernel-<release>-<arch>`: the architecture is what
/// follows the last dash, and both parts must be present.
pub(super) fn dkms_link_kernel(name: &str) -> Option<&str> {
    let (release, arch) = name.strip_prefix("kernel-")?.rsplit_once('-')?;
    let arch_ok = !arch.is_empty() && arch.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    (arch_ok && !release.is_empty()).then_some(release)
}

/// The kernels a DKMS package is built and installed for, read from DKMS's own directories (one
/// `kernel-<release>-<arch>` link per installed kernel): the `dkms` command takes about a second
/// for the same answer. `None` when this system keeps its DKMS state elsewhere (the caller then
/// asks the command); an empty list when the package is not registered at all.
pub(crate) fn dkms_installed(env: &Env, package: &str) -> Option<Vec<String>> {
    if !env.path(DKMS_ROOT).is_dir() {
        return None;
    }
    let base = env.path(&format!("{DKMS_ROOT}/{package}"));
    let Ok(entries) = fs::read_dir(&base) else { return Some(Vec::new()) };
    let mut kernels: Vec<String> = entries
        .filter_map(Result::ok)
        // A link that points nowhere (its build was deleted) does not count as built.
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().to_str().and_then(dkms_link_kernel).map(str::to_owned))
        .collect();
    kernels.sort();
    Some(kernels)
}

/// The kernels `dkms status` lists as installed: lines like
/// `truesense-wmi/0.5.0, 6.18.1, x86_64: installed`.
pub(super) fn parse_dkms_status(output: &str) -> Vec<String> {
    let mut kernels = Vec::new();
    for line in output.lines() {
        let parts: Vec<&str> = line.split(',').collect();
        for i in 1..parts.len().saturating_sub(1) {
            let kernel = parts[i].trim();
            let tail = parts[i + 1].trim_start();
            let installed =
                tail.strip_prefix("x86_64:").is_some_and(|rest| rest.trim_start().starts_with("installed"));
            if installed && !kernel.is_empty() {
                kernels.push(kernel.to_owned());
            }
        }
    }
    kernels
}

/// Every kernel with headers installed (`/lib/modules/*/build`, a dangling link included, as
/// `glob` finds it), sorted.
fn installed_kernels(env: &Env) -> Vec<String> {
    let Ok(entries) = fs::read_dir(env.path("/lib/modules")) else { return Vec::new() };
    let mut kernels: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().join("build").symlink_metadata().is_ok())
        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
        .collect();
    kernels.sort();
    kernels
}

fn dkms(env: &Env) -> Outcome {
    let not_registered =
        || Outcome::fail("truesense-wmi is not registered with DKMS", "sudo tools/install.sh");
    let built = match dkms_installed(env, "truesense-wmi") {
        // An unusual layout: ask the command.
        None => {
            let out = env.sh("dkms", &["status", "truesense-wmi"]);
            if out.is_empty() {
                return not_registered();
            }
            parse_dkms_status(&out)
        }
        Some(_) if !env.path(&format!("{DKMS_ROOT}/truesense-wmi")).is_dir() => return not_registered(),
        Some(kernels) => kernels,
    };
    let missing: Vec<String> = installed_kernels(env).into_iter().filter(|k| !built.contains(k)).collect();
    if !built.contains(&env.kernel) {
        return Outcome::fail(
            format!("not built for the running kernel {}", env.kernel),
            "sudo dkms autoinstall   (or: sudo tools/install.sh)",
        );
    }
    if !missing.is_empty() {
        return Outcome::warn(
            format!("built for {}; missing for installed kernel(s) {}", built.join(", "), missing.join(", ")),
            "sudo dkms autoinstall",
        );
    }
    Outcome::ok(format!("installed for {}", built.join(", ")))
}

fn module(env: &Env) -> Outcome {
    if !env.loaded(MODULE) {
        return Outcome::fail(
            "truesense_wmi is not loaded",
            "sudo modprobe truesense_wmi   (or reinstall: sudo tools/install.sh)",
        );
    }
    let version =
        read_trim(&env.path(&format!("/sys/module/{MODULE}/version"))).unwrap_or_else(|| "?".to_owned());
    let on_disk = env.sh("modinfo", &["-F", "version", MODULE]);
    if !on_disk.is_empty() && version != on_disk {
        return Outcome::warn(
            format!("loaded {version} but {on_disk} is installed"),
            "reboot (or: sudo modprobe -r truesense_wmi && sudo modprobe truesense_wmi)",
        );
    }
    Outcome::ok(format!("truesense_wmi {version} loaded"))
}

fn device(env: &Env) -> Outcome {
    let node = env.path(DEVICE);
    if !node.exists() {
        return Outcome::fail(
            format!("{DEVICE} missing"),
            "the module did not bind: check `dmesg | grep truesense`",
        );
    }
    if !env.host.can_access(&node, true) {
        return Outcome::warn(
            format!("{DEVICE} exists but you have no access"),
            "log in on the laptop's seat (uaccess) or use sudo; check /etc/udev/rules.d/70-truesense.rules",
        );
    }
    let caps = match env.firmware.capabilities() {
        Ok(caps) => caps,
        Err(e) => return Outcome::fail(e.to_string(), "see dmesg"),
    };
    // In the order the module prints them.
    let interfaces = [("game", caps.game), ("apge", caps.apge), ("battery", caps.battery)];
    let detail = format!(
        "interfaces: {}",
        interfaces
            .iter()
            .map(|(k, v)| format!("{k}={}", on_off(*v, "yes", "NO")))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let missing: Vec<&str> = interfaces.iter().filter(|(_, v)| !v).map(|(k, _)| *k).collect();
    if missing.is_empty() {
        Outcome::ok(detail)
    } else {
        Outcome::warn(detail, format!("the firmware lacks {}", missing.join(", ")))
    }
}

fn is_legacy_dkms(name: &str) -> bool {
    name.contains("linuwu") || name.contains("acer-rear")
}

fn conflicts(env: &Env) -> Outcome {
    let bad: Vec<&str> = LEGACY_MODULES.iter().copied().filter(|m| env.loaded(m)).collect();
    if !bad.is_empty() {
        return Outcome::fail(
            format!("conflicting kernel module(s) loaded: {}", bad.join(", ")),
            "sudo tools/migrate-legacy.sh   (they fight over the fans and power modes)",
        );
    }
    let blocked = env.path(MODPROBE_BLOCK).exists();
    let legacy_dkms: Vec<String> = match fs::read_dir(env.path(DKMS_ROOT)) {
        Ok(entries) => {
            let mut names: Vec<String> = entries
                .filter_map(Result::ok)
                .filter_map(|e| e.file_name().to_str().map(str::to_owned))
                .filter(|n| is_legacy_dkms(n))
                .collect();
            names.sort();
            names
        }
        Err(_) => {
            env.sh("dkms", &["status"]).lines().filter(|l| is_legacy_dkms(l)).map(str::to_owned).collect()
        }
    };
    if !legacy_dkms.is_empty() {
        return Outcome::warn(
            format!("old DKMS package(s) still registered: {}", limit_chars(&legacy_dkms.join("; "), 80)),
            "sudo tools/migrate-legacy.sh",
        );
    }
    if !blocked {
        return Outcome::warn("no modprobe block for the old drivers", "sudo tools/install.sh");
    }
    Outcome::ok("no conflicting driver loaded; old drivers are blocked from loading")
}

/// Other daemons that change CPU governors or power limits can fight the power modes (they do not
/// touch the firmware path).
fn power_managers(env: &Env) -> Outcome {
    const UNITS: [&str; 6] =
        ["tuned", "tlp", "auto-cpufreq", "system76-power", "thermald", "power-profiles-daemon"];
    let running: Vec<&str> =
        UNITS.into_iter().filter(|u| env.active(&format!("{u}.service"), false)).collect();
    if running.is_empty() {
        return Outcome::ok("no other power manager is running");
    }
    let risky = running.iter().any(|u| matches!(*u, "tuned" | "tlp" | "auto-cpufreq" | "system76-power"));
    if risky {
        return Outcome::warn(
            format!("also running: {}", running.join(", ")),
            "these can override CPU frequency, turbo or limits. Let TrueSense stop them (undoable): truesense power managers stop   (or Settings, TrueSense manages power)",
        );
    }
    Outcome::info(
        format!("also running: {} (they do not touch the firmware power modes)", running.join(", ")),
        "",
    )
}

fn turbo(env: &Env) -> Outcome {
    let Some(no_turbo) = read_trim(&env.path("/sys/devices/system/cpu/intel_pstate/no_turbo")) else {
        return Outcome::info("no intel_pstate turbo switch on this CPU", "");
    };
    if no_turbo == "1" {
        return Outcome::warn(
            "CPU turbo boost is OFF: the CPU is held at its base clock",
            "a power manager may have left it off: truesense power managers stop   (or: echo 0 | sudo tee /sys/devices/system/cpu/intel_pstate/no_turbo)",
        );
    }
    let low = env.probes.low_frequency_ceilings();
    if low > 0 {
        return Outcome::warn(
            format!(
                "CPU turbo is on but {low} core(s) have a frequency ceiling below their maximum (left over from turbo being off, or a thermal cap)"
            ),
            "truesense power managers stop   (raises the ceilings; the service re-applies a thermal cap if it needs one)",
        );
    }
    Outcome::ok("CPU turbo boost is on")
}

/// How old the daemon's state file is, if there is one and the clock is not before it.
fn state_age_seconds(env: &Env) -> Option<f64> {
    let modified = fs::metadata(env.path(STATE_FILE)).and_then(|m| m.modified()).ok()?;
    let age = env.host.now().duration_since(modified).unwrap_or_default();
    Some(age.as_secs_f64())
}

fn daemon(env: &Env) -> Outcome {
    if !env.active("truesense.service", false) {
        return Outcome::fail(
            "truesense.service is not running",
            "sudo systemctl enable --now truesense.service ; journalctl -u truesense -b",
        );
    }
    let state = match env.daemon.state() {
        Ok(state) => state,
        Err(e) => {
            return Outcome::warn(
                format!("service running but the control socket does not answer: {e}"),
                "sudo systemctl restart truesense",
            );
        }
    };
    if state.live.hw_ok == Some(false) {
        return Outcome::warn("daemon cannot reach the kernel device", "check the module (above)");
    }
    if let Some(age) = state_age_seconds(env).filter(|a| *a > STALL_SECONDS) {
        return Outcome::warn(
            format!("daemon state is {age:.0}s old (stalled?)"),
            "sudo systemctl restart truesense",
        );
    }
    let activity = state.live.activity.as_deref().filter(|a| !a.is_empty()).unwrap_or("?");
    Outcome::ok(format!(
        "running; smart_fan={}, auto_profile={}, activity={activity}",
        on_off(state.settings.smart_fan, "on", "off"),
        on_off(state.settings.auto_profile, "on", "off"),
    ))
}

/// The running service must know every command the app and CLI use; after an upgrade it has to be
/// restarted (the installer does that).
fn service_version(env: &Env) -> Outcome {
    if !env.active("truesense.service", false) {
        return Outcome::info("service not running", "");
    }
    for command in ["fan_curve", "battery_full"] {
        if let Err(e) = env.daemon.read(command) {
            if e.is_unknown_command() {
                return Outcome::warn(
                    "the running service is older than the installed app",
                    "sudo systemctl restart truesense   (or re-run sudo tools/install.sh)",
                );
            }
            return Outcome::warn(format!("service did not answer a version probe: {e}"), "");
        }
    }
    Outcome::ok(format!("service and app are the same version ({})", env.version))
}

fn user_services(env: &Env) -> Outcome {
    const UNITS: [&str; 3] =
        ["truesense-keys.service", "truesense-audio.service", "truesense-startup.service"];
    let states: Vec<(&str, bool)> = UNITS.into_iter().map(|u| (u, env.active(u, true))).collect();
    let detail = states
        .iter()
        .map(|(u, active)| {
            format!("{}={}", u.split('.').next().unwrap_or(u), on_off(*active, "active", "inactive"))
        })
        .collect::<Vec<_>>()
        .join(", ");
    let bad: Vec<&str> = states.iter().filter(|(_, active)| !active).map(|(u, _)| *u).collect();
    if bad.is_empty() {
        Outcome::ok(detail)
    } else {
        Outcome::warn(detail, format!("systemctl --user enable --now {}", bad.join(" ")))
    }
}

fn udev(env: &Env) -> Outcome {
    let rule = env.path("/etc/udev/rules.d/70-truesense.rules").exists()
        || env.path("/usr/lib/udev/rules.d/70-truesense.rules").exists();
    if !rule {
        return Outcome::fail("udev rule missing", "sudo tools/install.sh");
    }
    let Some(node) = env.probes.fn_key_node() else {
        return Outcome::warn(
            "keyboard Fn-key interface not found",
            "is the internal keyboard (04f2:0117) present?",
        );
    };
    let readable = env.host.can_access(&node, false);
    Outcome::graded(
        readable,
        format!(
            "rule installed; Fn-key interface {} {}",
            node.display(),
            on_off(readable, "readable", "NOT readable")
        ),
        "",
    )
}

fn config(env: &Env) -> Outcome {
    let cfg = match (env.config)() {
        Ok(cfg) => cfg,
        Err(e) => return Outcome::fail(format!("config does not parse: {e}"), "fix the TOML syntax"),
    };
    let problems = cfg.validate();
    if problems.is_empty() {
        return Outcome::ok("configuration valid");
    }
    Outcome::fail(problems.join("; "), "edit /etc/truesense/config.toml or ~/.config/truesense/config.toml")
}

fn secure_boot(env: &Env) -> Outcome {
    let state = env.sh("mokutil", &["--sb-state"]).to_lowercase();
    let signer = env.sh("modinfo", &["-F", "signer", MODULE]);
    if state.contains("enabled") {
        return Outcome::graded(
            !signer.is_empty(),
            format!("Secure Boot on; module signer: {}", if signer.is_empty() { "NONE" } else { &signer }),
            "enrol the DKMS key: sudo mokutil --import /var/lib/dkms/mok.pub, then reboot",
        );
    }
    Outcome::info(
        format!(
            "Secure Boot off or unknown; module signer: {}",
            if signer.is_empty() { "none" } else { &signer }
        ),
        "",
    )
}

fn audio(env: &Env) -> Outcome {
    let pipewire = env.active("pipewire.service", true);
    let mut bits = vec![
        format!("PipeWire {}", on_off(pipewire, "running", "NOT running")),
        format!(
            "RNNoise plugin {}",
            on_off(env.audio.has_rnnoise(), "found", "missing (Purified Voice off)")
        ),
        format!("AcerUltraX {}", on_off(env.audio.has_dts(), "found", "not installed (optional)")),
    ];
    if let Ok(jack) = env.audio.jack_status() {
        bits.push(format!(
            "jack: headphones {}, headset mic {}",
            on_off(jack.headphones, "in", "out"),
            on_off(jack.headset_mic, "detected", "not detected")
        ));
    }
    // A dead PipeWire is not a dead card: the card is only judged while PipeWire runs.
    let sound =
        if pipewire { env.audio.sound_state().unwrap_or(SoundState::Unknown) } else { SoundState::Unknown };
    match sound {
        SoundState::Broken(problem) => {
            return Outcome::warn(
                format!("NO SOUND OUTPUT: {problem}; {}", bits.join("; ")),
                "truesense audio repair   (restarts WirePlumber so it probes the card again)",
            );
        }
        SoundState::Healthy => bits.insert(1, "output sink present".to_owned()),
        SoundState::Unknown => {}
    }
    // The DTS service is per user: ask it as the user, not as root.
    if env.host.euid() != 0 && env.audio.has_dts() {
        if let Ok(bare) = env.audio.plain_headsets() {
            if !bare.is_empty() {
                return Outcome::warn(
                    format!("DTS is off (Plain) for: {}; {}", bare.join(", "), bits.join("; ")),
                    "truesense audio dts all   (gives every output DTS; new ones get it by themselves)",
                );
            }
        }
    }
    if pipewire {
        Outcome::ok(bits.join("; "))
    } else {
        Outcome::warn(bits.join("; "), "systemctl --user start pipewire")
    }
}

fn fans(env: &Env) -> Outcome {
    use ts_platform::hw::FanMode;
    let Ok(state) = env.firmware.fan_state() else { return Outcome::info("fan state unreadable", "") };
    let mode = match state.mode {
        FanMode::Auto => "firmware auto".to_owned(),
        FanMode::Max => "MAX".to_owned(),
        FanMode::Manual => "manual".to_owned(),
        FanMode::Unknown => state.behavior_raw.clone(),
    };
    // No daemon: nothing to contradict.
    let smart = env.daemon.state().is_ok_and(|s| s.settings.smart_fan);
    if state.mode == FanMode::Max && smart {
        return Outcome::warn(
            "fans are on MAX while the smart curve is on (the daemon should have taken over)",
            "truesense fan smart on",
        );
    }
    Outcome::ok(format!("fans: {mode}"))
}

/// The window (`truesense gui`) is built into the binary and opens in a web browser as an app
/// window; it needs a way to open one.
fn window(env: &Env) -> Outcome {
    if !env.has_program("xdg-open") {
        return Outcome::info(
            "the window (truesense gui) opens in your web browser and needs xdg-open to find it; everything works from the command line without it",
            "sudo pacman -S xdg-utils",
        );
    }
    Outcome::ok("web browser launcher found (truesense gui)")
}

/// Status-bar buttons call `truesense-fast`, a link to the same binary that skips everything but
/// the control socket.
fn status_bars(env: &Env) -> Outcome {
    if !env.has_program("truesense-fast") {
        return Outcome::info(
            "truesense-fast not found: status-bar buttons fall back to the slower `truesense` command",
            "re-run the installer: sudo tools/install.sh",
        );
    }
    Outcome::ok("truesense-fast found: status-bar buttons use the fast path")
}
