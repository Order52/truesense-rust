//! session: the graphical session's environment, for programs that outlive it.
//!
//! A user service (the PREDATOR / Fn key service) starts at boot, before the desktop has told
//! systemd where its display is, and keeps the environment it started with for weeks. When a key
//! then runs `truesense gui`, the window finds no `WAYLAND_DISPLAY`, falls back to X11 and
//! aborts. This module works out what a child should be given so that it finds the live session
//! whatever the service was started with:
//!
//! 1. the user manager's environment (`systemctl --user show-environment`): the desktop exports
//!    its variables there when the session starts, so it is the live truth;
//! 2. the process's own environment;
//! 3. the sockets in `$XDG_RUNTIME_DIR` and `/tmp/.X11-unix`, for a desktop that never told
//!    systemd anything.
//!
//! The decision is pure ([`Plan`]): the file system, the environment and the manager's answer are
//! passed in, so every case is tested without a desktop.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The variables that describe a graphical session, besides the `QT_` and `XCURSOR_` families.
const SESSION_VARS: &[&str] = &[
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "XAUTHORITY",
    "HYPRLAND_INSTANCE_SIGNATURE",
    "SWAYSOCK",
    "I3SOCK",
    "NIRI_SOCKET",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_TYPE",
    "XDG_SESSION_DESKTOP",
    "XDG_SESSION_ID",
    "DBUS_SESSION_BUS_ADDRESS",
];

/// Where X11 sockets live.
const X11_DIR: &str = "/tmp/.X11-unix";

/// Whether `name` is part of the graphical session's description: the display, the desktop, the
/// toolkit settings and the locale (a window started without `LANG` runs in the C locale).
pub fn is_session_var(name: &str) -> bool {
    SESSION_VARS.contains(&name)
        || name == "LANG"
        || name == "LANGUAGE"
        || ["QT_", "XCURSOR_", "LC_"].iter().any(|prefix| name.starts_with(prefix))
}

/// Which environment is believed when the process's own and the user manager's differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prefer {
    /// The process's own: a command typed in a terminal keeps the `DISPLAY` it was given.
    Process,
    /// The manager's: a service that has been running since before the session existed.
    Manager,
}

/// What the decision needs from the machine.
pub struct World<'a> {
    /// The process's environment, by name.
    pub env: &'a dyn Fn(&str) -> Option<String>,
    /// The output of `systemctl --user show-environment`, when it could be asked.
    pub manager: Option<&'a str>,
    /// `$XDG_RUNTIME_DIR`.
    pub runtime_dir: Option<&'a Path>,
    /// Whether a path exists.
    pub exists: &'a dyn Fn(&Path) -> bool,
    /// The names in a directory (empty when it cannot be read).
    pub list: &'a dyn Fn(&Path) -> Vec<String>,
}

/// Parses `systemctl show-environment`: `NAME=value` lines. A value systemd quoted for a shell
/// (`$'...'`) is skipped: none of the session variables needs quoting.
pub fn parse_environment(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (name, value) = line.split_once('=')?;
            let ok_name = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            (ok_name && !value.starts_with("$'")).then(|| (name.to_owned(), value.to_owned()))
        })
        .collect()
}

/// The Wayland socket a `WAYLAND_DISPLAY` value names: absolute, or inside the runtime directory.
fn wayland_socket(value: &str, runtime_dir: Option<&Path>) -> Option<PathBuf> {
    let path = Path::new(value);
    if path.is_absolute() { Some(path.to_path_buf()) } else { runtime_dir.map(|d| d.join(value)) }
}

/// The X11 socket a `DISPLAY` value (`:0`, `:1.0`, `localhost:10.0`) names, when it is a local one.
fn x11_socket(value: &str) -> Option<PathBuf> {
    let number = value.strip_prefix(':')?;
    let number = number.split('.').next()?;
    (!number.is_empty() && number.chars().all(|c| c.is_ascii_digit()))
        .then(|| Path::new(X11_DIR).join(format!("X{number}")))
}

/// The numbers in `names` of the form `<prefix><number>` (sockets, not their `.lock` files).
fn numbered(names: &[String], prefix: &str) -> Vec<(u32, String)> {
    let mut found: Vec<(u32, String)> = names
        .iter()
        .filter_map(|n| Some((n.strip_prefix(prefix)?.parse::<u32>().ok()?, n.clone())))
        .collect();
    found.sort();
    found
}

/// The newest Hyprland instance in `hypr/`: names are `<hash>_<unix time>_<n>`.
fn newest_hypr_instance(names: &[String]) -> Option<String> {
    names
        .iter()
        .filter_map(|n| Some((n.split('_').nth(1)?.parse::<u64>().ok()?, n.clone())))
        .max()
        .map(|(_, name)| name)
}

/// The variables to give a child, and the reason there is nothing to give when no display exists.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// Variables to set on the child (only those that differ from the process's own).
    pub set: Vec<(String, String)>,
    /// Whether the session has a display at all, after the plan: a child that needs one will fail
    /// to start otherwise.
    pub has_display: bool,
}

impl Plan {
    /// Works out the child's session variables.
    pub fn decide(world: &World<'_>, prefer: Prefer) -> Self {
        let manager = world.manager.map(parse_environment).unwrap_or_default();
        let from_manager = |name: &str| manager.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
        // Every session variable either side has, resolved by preference.
        let mut names: Vec<String> =
            manager.iter().map(|(n, _)| n.clone()).filter(|n| is_session_var(n)).collect();
        names.extend(SESSION_VARS.iter().map(|s| (*s).to_owned()));
        names.sort();
        names.dedup();
        let mut chosen: Vec<(String, String)> = Vec::new();
        for name in names {
            let own = (world.env)(&name).filter(|v| !v.is_empty());
            let live = from_manager(&name).filter(|v| !v.is_empty());
            let value = match prefer {
                Prefer::Process => own.or(live),
                Prefer::Manager => live.or(own),
            };
            if let Some(v) = value {
                chosen.push((name, v));
            }
        }
        let get = |chosen: &Vec<(String, String)>, name: &str| -> Option<String> {
            chosen.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone())
        };
        let put = |chosen: &mut Vec<(String, String)>, name: &str, value: String| {
            chosen.retain(|(n, _)| n != name);
            chosen.push((name.to_owned(), value));
        };

        // A display that is named but not there (a Wayland socket of a compositor that has been
        // restarted) is replaced by what is there.
        let wayland_ok = get(&chosen, "WAYLAND_DISPLAY")
            .and_then(|v| wayland_socket(&v, world.runtime_dir))
            .is_some_and(|p| (world.exists)(&p));
        if !wayland_ok {
            chosen.retain(|(n, _)| n != "WAYLAND_DISPLAY");
            if let Some(dir) = world.runtime_dir {
                let names = (world.list)(dir);
                if let Some((_, socket)) = numbered(&names, "wayland-").into_iter().find(|(_, n)| {
                    // `wayland-1.lock` parses as no number, but a stray socket file with a
                    // number suffix is still only believed when it exists.
                    (world.exists)(&dir.join(n))
                }) {
                    put(&mut chosen, "WAYLAND_DISPLAY", socket);
                }
            }
        }
        let x11_ok = get(&chosen, "DISPLAY").and_then(|v| x11_socket(&v)).is_some_and(|p| (world.exists)(&p));
        if !x11_ok {
            // Only a local, missing X display is dropped; a remote one (`host:10`) is the
            // person's own business.
            if get(&chosen, "DISPLAY").as_deref().is_some_and(|v| x11_socket(v).is_some()) {
                chosen.retain(|(n, _)| n != "DISPLAY");
            }
            if get(&chosen, "DISPLAY").is_none() {
                let names = (world.list)(Path::new(X11_DIR));
                if let Some((n, _)) = numbered(&names, "X").into_iter().next() {
                    put(&mut chosen, "DISPLAY", format!(":{n}"));
                }
            }
        }
        // The compositor's own control socket follows its session.
        if get(&chosen, "XDG_CURRENT_DESKTOP").is_some_and(|d| d.eq_ignore_ascii_case("hyprland"))
            || get(&chosen, "HYPRLAND_INSTANCE_SIGNATURE").is_some()
        {
            let valid = get(&chosen, "HYPRLAND_INSTANCE_SIGNATURE")
                .zip(world.runtime_dir)
                .is_some_and(|(sig, dir)| (world.exists)(&dir.join("hypr").join(sig)));
            if !valid && let Some(dir) = world.runtime_dir {
                if let Some(sig) = newest_hypr_instance(&(world.list)(&dir.join("hypr"))) {
                    put(&mut chosen, "HYPRLAND_INSTANCE_SIGNATURE", sig);
                }
            }
        }
        let has_display = get(&chosen, "WAYLAND_DISPLAY").is_some()
            || get(&chosen, "DISPLAY").is_some_and(|v| x11_socket(&v).is_none_or(|p| (world.exists)(&p)));
        let mut set: Vec<(String, String)> =
            chosen.into_iter().filter(|(n, v)| (world.env)(n).as_deref() != Some(v.as_str())).collect();
        set.sort();
        Self { set, has_display }
    }

    /// The plan for this machine, right now.
    pub fn system(prefer: Prefer) -> Self {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_absolute());
        let manager = ask_manager();
        let env = |name: &str| std::env::var(name).ok();
        let exists = |p: &Path| p.exists();
        let list = |dir: &Path| -> Vec<String> {
            std::fs::read_dir(dir)
                .map(|rd| rd.filter_map(|e| e.ok()?.file_name().into_string().ok()).collect())
                .unwrap_or_default()
        };
        Self::decide(
            &World {
                env: &env,
                manager: manager.as_deref(),
                runtime_dir: runtime.as_deref(),
                exists: &exists,
                list: &list,
            },
            prefer,
        )
    }

    /// Gives `command` the variables of this plan.
    pub fn apply(&self, command: &mut Command) {
        command.envs(self.set.iter().map(|(n, v)| (n, v)));
    }
}

/// `systemctl --user show-environment`, or `None` when it cannot be run or fails (no user
/// manager, no `systemctl`).
fn ask_manager() -> Option<String> {
    let out = Command::new("systemctl")
        .args(["--user", "show-environment"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    struct Machine {
        env: BTreeMap<&'static str, &'static str>,
        manager: Option<&'static str>,
        files: BTreeSet<&'static str>,
        dirs: BTreeMap<&'static str, Vec<&'static str>>,
    }

    impl Machine {
        fn new() -> Self {
            Self { env: BTreeMap::new(), manager: None, files: BTreeSet::new(), dirs: BTreeMap::new() }
        }
        fn plan(&self, prefer: Prefer) -> Plan {
            let env = |n: &str| self.env.get(n).map(|v| (*v).to_owned());
            let exists = |p: &Path| self.files.contains(p.to_str().unwrap());
            let list = |d: &Path| {
                self.dirs
                    .get(d.to_str().unwrap())
                    .map(|v| v.iter().map(|s| (*s).to_owned()).collect())
                    .unwrap_or_default()
            };
            Plan::decide(
                &World {
                    env: &env,
                    manager: self.manager,
                    runtime_dir: Some(Path::new("/run/user/1000")),
                    exists: &exists,
                    list: &list,
                },
                prefer,
            )
        }
    }

    fn set(plan: &Plan) -> Vec<(&str, &str)> {
        plan.set.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect()
    }

    const MANAGER: &str = "DISPLAY=:0\nLANG=en_US.UTF-8\nLC_TIME=en_US.UTF-8\nHYPRLAND_CMD=$'Hyprland --watchdog-fd 4'\nHYPRLAND_INSTANCE_SIGNATURE=abc_1794544996_1\nPATH=/usr/bin\nQT_QPA_PLATFORMTHEME=qt6ct\nWAYLAND_DISPLAY=wayland-1\nXDG_CURRENT_DESKTOP=Hyprland\n";

    #[test]
    fn a_service_started_before_the_session_gets_the_managers_variables() {
        // The real failure: the key service's environment held only XDG_RUNTIME_DIR and D-Bus.
        let mut m = Machine::new();
        m.env.insert("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/1000/bus");
        m.manager = Some(MANAGER);
        m.files.extend([
            "/run/user/1000/wayland-1",
            "/tmp/.X11-unix/X0",
            "/run/user/1000/hypr/abc_1794544996_1",
        ]);
        let plan = m.plan(Prefer::Manager);
        assert!(plan.has_display);
        assert_eq!(
            set(&plan),
            [
                ("DISPLAY", ":0"),
                ("HYPRLAND_INSTANCE_SIGNATURE", "abc_1794544996_1"),
                ("LANG", "en_US.UTF-8"),
                ("LC_TIME", "en_US.UTF-8"),
                ("QT_QPA_PLATFORMTHEME", "qt6ct"),
                ("WAYLAND_DISPLAY", "wayland-1"),
                ("XDG_CURRENT_DESKTOP", "Hyprland"),
            ],
            "session variables and the locale only: not PATH, not the shell-quoted HYPRLAND_CMD"
        );
    }

    #[test]
    fn a_desktop_that_never_told_systemd_is_found_by_its_sockets() {
        let mut m = Machine::new();
        m.manager = None;
        m.files.extend(["/run/user/1000/wayland-1", "/run/user/1000/wayland-1.lock", "/tmp/.X11-unix/X0"]);
        m.dirs.insert("/run/user/1000", vec!["wayland-1", "wayland-1.lock", "bus", "hypr"]);
        m.dirs.insert("/tmp/.X11-unix", vec!["X0"]);
        let plan = m.plan(Prefer::Manager);
        assert!(plan.has_display);
        assert_eq!(set(&plan), [("DISPLAY", ":0"), ("WAYLAND_DISPLAY", "wayland-1")]);
    }

    #[test]
    fn a_hyprland_session_gets_its_newest_control_socket() {
        let mut m = Machine::new();
        m.env.insert("XDG_CURRENT_DESKTOP", "Hyprland");
        m.env.insert("HYPRLAND_INSTANCE_SIGNATURE", "old_1000_1");
        m.files.insert("/run/user/1000/wayland-1");
        m.dirs.insert("/run/user/1000", vec!["wayland-1"]);
        m.dirs.insert("/run/user/1000/hypr", vec!["old_1000_1", "new_2000_1", "lock"]);
        let plan = m.plan(Prefer::Manager);
        assert!(set(&plan).contains(&("HYPRLAND_INSTANCE_SIGNATURE", "new_2000_1")), "{plan:?}");
    }

    #[test]
    fn a_terminals_own_display_is_kept_when_it_works() {
        let mut m = Machine::new();
        m.env.insert("DISPLAY", ":1");
        m.env.insert("WAYLAND_DISPLAY", "wayland-9");
        m.manager = Some(MANAGER);
        m.files.extend([
            "/tmp/.X11-unix/X1",
            "/run/user/1000/wayland-9",
            "/run/user/1000/wayland-1",
            "/tmp/.X11-unix/X0",
        ]);
        let plan = m.plan(Prefer::Process);
        assert!(plan.has_display);
        let s = set(&plan);
        assert!(!s.iter().any(|(n, _)| *n == "DISPLAY" || *n == "WAYLAND_DISPLAY"), "{s:?}");
    }

    #[test]
    fn a_display_that_is_named_but_gone_is_replaced_by_the_one_that_is_there() {
        let mut m = Machine::new();
        m.env.insert("WAYLAND_DISPLAY", "wayland-0");
        m.env.insert("DISPLAY", ":7");
        m.files.insert("/run/user/1000/wayland-1");
        m.dirs.insert("/run/user/1000", vec!["wayland-1"]);
        let plan = m.plan(Prefer::Process);
        assert_eq!(set(&plan), [("WAYLAND_DISPLAY", "wayland-1")], "the dead X display is dropped, not kept");
        assert!(plan.has_display);
    }

    #[test]
    fn a_remote_x_display_is_the_persons_own_business() {
        let mut m = Machine::new();
        m.env.insert("DISPLAY", "localhost:10.0");
        let plan = m.plan(Prefer::Process);
        assert!(plan.has_display, "an ssh -X display is believed");
        assert!(plan.set.is_empty());
    }

    #[test]
    fn no_session_at_all_says_so() {
        let mut m = Machine::new();
        m.env.insert("XDG_RUNTIME_DIR", "/run/user/1000");
        let plan = m.plan(Prefer::Manager);
        assert!(!plan.has_display);
        assert!(plan.set.is_empty());
    }

    #[test]
    fn the_managers_text_is_parsed_without_its_shell_quoting() {
        let parsed = parse_environment(MANAGER);
        assert!(parsed.contains(&("WAYLAND_DISPLAY".to_owned(), "wayland-1".to_owned())));
        assert!(!parsed.iter().any(|(n, _)| n == "HYPRLAND_CMD"));
        assert!(parse_environment("garbage\n=x\nA B=c\n").is_empty());
        assert!(is_session_var("QT_QPA_PLATFORM") && is_session_var("XCURSOR_SIZE"));
        assert!(is_session_var("LANG") && is_session_var("LC_ALL") && is_session_var("LANGUAGE"));
        assert!(!is_session_var("PATH") && !is_session_var("HOME") && !is_session_var("LANGUAGE_X"));
    }

    #[test]
    fn a_plan_is_applied_to_a_command() {
        let plan = Plan { set: vec![("WAYLAND_DISPLAY".into(), "wayland-1".into())], has_display: true };
        let mut command = Command::new("true");
        plan.apply(&mut command);
        let envs: Vec<_> = command.get_envs().collect();
        assert_eq!(envs.len(), 1);
    }
}
