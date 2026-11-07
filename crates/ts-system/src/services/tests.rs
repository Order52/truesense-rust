#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

const KERNEL: &str = "6.1.0-test";

#[derive(Clone)]
struct UnitFake {
    enabled: String,
    active: String,
}

struct Inner {
    calls: Vec<Vec<String>>,
    units: HashMap<(bool, String), UnitFake>,
    module: bool,
    socket: bool,
    dkms_fixes: bool,
    restart_fixes: bool,
    dkms_ran: bool,
    /// Extra failure text per "program verb" (for example `systemctl daemon-reload`).
    failing: HashMap<String, String>,
}

/// systemctl, modprobe and dkms as far as the repair cares. `calls` is every command issued.
struct FakeSystem(Mutex<Inner>);

struct Options {
    svc: &'static str,
    enabled: &'static str,
    module: bool,
    dkms_fixes: bool,
    restart_fixes: bool,
    user: Vec<(&'static str, &'static str, &'static str)>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            svc: "active",
            enabled: "enabled",
            module: true,
            dkms_fixes: false,
            restart_fixes: true,
            user: vec![],
        }
    }
}

impl FakeSystem {
    fn new(o: Options) -> Arc<Self> {
        let mut units = HashMap::new();
        for u in SYSTEM_UNITS {
            let active = if u == "truesense.service" { o.svc } else { "active" };
            units
                .insert((false, u.to_owned()), UnitFake { enabled: o.enabled.into(), active: active.into() });
        }
        for u in USER_UNITS {
            let mut f = UnitFake { enabled: "enabled".into(), active: "active".into() };
            if let Some((_, enabled, active)) = o.user.iter().find(|(name, _, _)| *name == u) {
                f = UnitFake { enabled: (*enabled).into(), active: (*active).into() };
            }
            units.insert((true, u.to_owned()), f);
        }
        Arc::new(Self(Mutex::new(Inner {
            calls: vec![],
            units,
            module: o.module,
            socket: o.svc == "active",
            dkms_fixes: o.dkms_fixes,
            restart_fixes: o.restart_fixes,
            dkms_ran: false,
            failing: HashMap::new(),
        })))
    }

    fn calls(&self) -> Vec<Vec<String>> {
        self.0.lock().unwrap().calls.clone()
    }

    fn clear(&self) {
        self.0.lock().unwrap().calls.clear();
    }

    fn remove_user_unit(&self, name: &str) {
        self.0.lock().unwrap().units.remove(&(true, name.to_owned()));
    }

    fn fail(&self, key: &str, text: &str) {
        self.0.lock().unwrap().failing.insert(key.to_owned(), text.to_owned());
    }

    fn module(&self) -> bool {
        self.0.lock().unwrap().module
    }

    fn exists(&self, path: &Path) -> bool {
        let s = self.0.lock().unwrap();
        match path.to_str() {
            Some("/dev/truesense") => s.module,
            Some(SOCKET_PATH) => s.socket,
            _ => false,
        }
    }

    /// The calls that change something.
    fn changes(&self) -> Vec<Vec<String>> {
        self.calls()
            .into_iter()
            .filter(|c| {
                matches!(c[0].as_str(), "dkms" | "modprobe")
                    || c.iter().any(|a| matches!(a.as_str(), "restart" | "enable" | "start"))
            })
            .collect()
    }
}

impl Runner for FakeSystem {
    fn run(&self, program: &str, args: &[&str]) -> Output {
        let mut s = self.0.lock().unwrap();
        let mut line = vec![program.to_owned()];
        line.extend(args.iter().map(|a| (*a).to_owned()));
        s.calls.push(line);
        let key = format!("{program} {}", args.iter().find(|a| **a != "--user").unwrap_or(&""));
        if let Some(text) = s.failing.get(&key) {
            return Output::failure(1, text.clone());
        }
        match program {
            "systemctl" => {
                let user = args.contains(&"--user");
                let rest: Vec<&str> = args.iter().copied().filter(|a| *a != "--user").collect();
                let (verb, unit) = (rest[0], rest[rest.len() - 1].to_owned());
                let missing = !s.units.contains_key(&(user, unit.clone()));
                match verb {
                    "is-enabled" | "is-active" => match s.units.get(&(user, unit)) {
                        Some(u) => Output::success(format!(
                            "{}\n",
                            if verb == "is-enabled" { &u.enabled } else { &u.active }
                        )),
                        None => Output::failure(4, "not-found"),
                    },
                    "enable" | "start" | "restart" if missing => {
                        Output::failure(1, format!("Unit {unit} does not exist"))
                    }
                    "enable" => {
                        let now = rest.contains(&"--now");
                        let u = s.units.get_mut(&(user, unit)).unwrap();
                        u.enabled = "enabled".into();
                        if now {
                            u.active = "active".into();
                        }
                        Output::success("")
                    }
                    "start" => {
                        s.units.get_mut(&(user, unit)).unwrap().active = "active".into();
                        Output::success("")
                    }
                    "restart" => {
                        let fixes = s.restart_fixes;
                        s.units.get_mut(&(user, unit)).unwrap().active =
                            if fixes { "active" } else { "failed" }.into();
                        s.socket = fixes;
                        Output::success("")
                    }
                    _ => Output::success(""),
                }
            }
            "dkms" => {
                s.dkms_ran = s.dkms_fixes;
                Output::success("")
            }
            "modprobe" => {
                if s.dkms_ran {
                    s.module = true;
                }
                Output::success("")
            }
            other => panic!("unexpected program {other}"),
        }
    }
}

/// A pinger that follows a script, then repeats its last answer.
struct ScriptedPing {
    answers: Vec<bool>,
    next: AtomicUsize,
}

impl ScriptedPing {
    fn always(answer: bool) -> Arc<Self> {
        Arc::new(Self { answers: vec![answer], next: AtomicUsize::new(0) })
    }

    fn script(answers: &[bool]) -> Arc<Self> {
        Arc::new(Self { answers: answers.to_vec(), next: AtomicUsize::new(0) })
    }
}

impl Pinger for ScriptedPing {
    fn ping(&self) -> bool {
        let i = self.next.fetch_add(1, Ordering::SeqCst).min(self.answers.len() - 1);
        self.answers[i]
    }
}

type Sleeps = Arc<Mutex<Vec<Duration>>>;

fn manager(f: &Arc<FakeSystem>, ping: Arc<dyn Pinger>) -> (Manager, Sleeps) {
    let sleeps = Sleeps::default();
    let log = Arc::clone(&sleeps);
    let fake = Arc::clone(f);
    let m = Manager::new(ping, KERNEL)
        .with_runner(f.clone())
        .with_exists(Arc::new(move |p| fake.exists(p)))
        .with_sleeper(Arc::new(move |d| log.lock().unwrap().push(d)));
    (m, sleeps)
}

fn programs(calls: &[Vec<String>]) -> BTreeSet<String> {
    calls.iter().map(|c| c[0].clone()).collect()
}

fn texts(steps: &[Step]) -> Vec<&str> {
    steps.iter().map(|s| s.what.as_str()).collect()
}

// ---- check ------------------------------------------------------------------------------------

#[test]
fn a_healthy_system_is_all_ok() {
    let f = FakeSystem::new(Options::default());
    let (m, _) = manager(&f, ScriptedPing::always(true));
    let rows = m.check();
    assert!(rows.iter().all(|r| r.ok), "{rows:?}");
    assert_eq!(rows.len(), SYSTEM_UNITS.len() + USER_UNITS.len() + 2);
    assert_eq!(programs(&f.calls()), BTreeSet::from(["systemctl".to_owned()]));
    assert!(f.changes().is_empty(), "check never changes anything");
    assert!(f.calls().iter().all(|c| c.iter().any(|a| a == "is-enabled" || a == "is-active")));
}

#[test]
fn every_problem_is_listed_with_its_fix() {
    let f = FakeSystem::new(Options {
        svc: "failed",
        module: false,
        user: vec![("truesense-keys.service", "enabled", "inactive")],
        ..Options::default()
    });
    let (m, _) = manager(&f, ScriptedPing::always(true));
    let rows = m.check();
    let bad: HashMap<&str, &Unit> = rows.iter().filter(|r| !r.ok).map(|r| (r.unit.as_str(), r)).collect();
    assert!(bad.contains_key("truesense.service"));
    assert!(bad.contains_key("kernel module (/dev/truesense)"));
    assert!(bad.contains_key("truesense-keys.service"));
    assert_eq!(bad["truesense.service"].fix, "sudo truesense service repair");
    assert_eq!(
        bad["truesense-keys.service"].fix, "truesense service repair",
        "a user service is fixed without sudo"
    );
    assert!(render(&rows).contains("!!"));
    let socket = rows.iter().find(|r| r.unit == "control socket").unwrap();
    assert_eq!((socket.active.as_str(), socket.ok), ("missing", false));
}

#[test]
fn a_socket_that_exists_but_does_not_answer_is_flagged() {
    let f = FakeSystem::new(Options::default());
    let (m, _) = manager(&f, ScriptedPing::always(false));
    let rows = m.check();
    let socket = rows.iter().find(|r| r.unit == "control socket").unwrap();
    assert_eq!((socket.active.as_str(), socket.ok), ("silent", false));
    assert_eq!(socket.fix, "sudo truesense service repair");
}

#[test]
fn a_missing_socket_is_not_pinged() {
    let f = FakeSystem::new(Options { svc: "failed", ..Options::default() });
    let ping = ScriptedPing::always(true);
    let (m, _) = manager(&f, ping.clone());
    m.check();
    assert_eq!(ping.next.load(Ordering::SeqCst), 0);
}

#[test]
fn a_unit_systemd_does_not_know_is_unknown_not_an_error() {
    let f = FakeSystem::new(Options::default());
    f.remove_user_unit("truesense-update.timer");
    let (m, _) = manager(&f, ScriptedPing::always(true));
    let row = m.check().into_iter().find(|r| r.unit == "truesense-update.timer").unwrap();
    assert_eq!((row.enabled.as_str(), row.active.as_str(), row.ok), ("unknown", "unknown", false));
}

#[test]
fn what_a_healthy_unit_looks_like() {
    let unit = |enabled: &str, active: &str| Unit {
        unit: "x".into(),
        scope: Scope::System,
        enabled: enabled.into(),
        active: active.into(),
        ok: false,
        fix: String::new(),
    };
    for enabled in ["enabled", "static", "linked", "enabled-runtime", "alias"] {
        assert!(wanted(&unit(enabled, "active")), "{enabled}");
        assert!(!wanted(&unit(enabled, "inactive")), "{enabled} but stopped");
        assert!(!wanted(&unit(enabled, "failed")));
    }
    for enabled in ["disabled", "masked", "unknown", "", "indirect", "generated"] {
        assert!(!wanted(&unit(enabled, "active")), "{enabled}");
    }
}

#[test]
fn the_unit_rows_serialise_with_the_python_names_in_order() {
    let f = FakeSystem::new(Options::default());
    let (m, _) = manager(&f, ScriptedPing::always(true));
    let json = serde_json::to_string(&m.check()[0]).unwrap();
    assert_eq!(
        json,
        r#"{"unit":"truesense.service","scope":"system","enabled":"enabled","active":"active","ok":true,"fix":""}"#
    );
    let back: Unit = serde_json::from_str(&json).unwrap();
    assert_eq!(back, m.check()[0]);
    let user = serde_json::to_string(&m.unit_state("truesense-keys.service", true)).unwrap();
    assert!(user.contains(r#""scope":"user""#), "{user}");
}

#[test]
fn a_step_serialises_as_a_pair_and_reads_back() {
    let step = Step::new("did it", true);
    assert_eq!(serde_json::to_string(&step).unwrap(), r#"["did it",true]"#);
    assert_eq!(serde_json::from_str::<Step>(r#"["did it",false]"#).unwrap(), Step::new("did it", false));
    for hostile in ["[]", r#"["a"]"#, r#"[1,true]"#, r#"{"what":"a"}"#, "null", r#"["a",true,3]"#] {
        assert!(serde_json::from_str::<Step>(hostile).is_err(), "{hostile}");
    }
}

#[test]
fn the_render_layout() {
    let rows = vec![
        Unit {
            unit: "truesense.service".into(),
            scope: Scope::System,
            enabled: "enabled".into(),
            active: "active".into(),
            ok: true,
            fix: String::new(),
        },
        Unit {
            unit: "truesense-keys.service".into(),
            scope: Scope::User,
            enabled: "disabled".into(),
            active: "inactive".into(),
            ok: false,
            fix: "truesense service repair".into(),
        },
    ];
    assert_eq!(
        render(&rows),
        "  [ok ] truesense.service                  system enabled   active\n  [!! ] truesense-keys.service             user   disabled  inactive   -> truesense service repair"
    );
    assert_eq!(render(&[]), "");
}

// ---- repair: system ---------------------------------------------------------------------------

#[test]
fn a_healthy_system_is_left_alone() {
    let f = FakeSystem::new(Options::default());
    let (m, sleeps) = manager(&f, ScriptedPing::always(true));
    let steps = m.repair_system();
    assert!(steps.iter().all(|s| s.ok), "{steps:?}");
    assert!(f.changes().is_empty(), "{:?}", f.changes());
    assert!(texts(&steps).iter().any(|s| s.contains("left alone")));
    assert!(sleeps.lock().unwrap().is_empty());
}

#[test]
fn a_dead_daemon_is_restarted_and_awaited() {
    let f = FakeSystem::new(Options { svc: "failed", ..Options::default() });
    let (m, sleeps) = manager(&f, ScriptedPing::script(&[false, false, true]));
    let steps = m.repair_system();
    assert!(steps.iter().all(|s| s.ok), "{steps:?}");
    assert!(f.calls().iter().any(|c| c[..2] == ["systemctl", "restart"]));
    assert!(texts(&steps).iter().any(|s| s.contains("answers")));
    assert_eq!(
        *sleeps.lock().unwrap(),
        vec![Duration::from_millis(500); 2],
        "two polls failed before the third answered"
    );
}

#[test]
fn a_daemon_that_cannot_start_is_reported_with_where_to_look() {
    let f = FakeSystem::new(Options { svc: "failed", restart_fixes: false, ..Options::default() });
    let (m, sleeps) = manager(&f, ScriptedPing::always(false));
    let steps = m.repair_system();
    let last = steps.last().unwrap();
    assert!(last.what.contains("journalctl -u truesense") && !last.ok, "{steps:?}");
    assert_eq!(sleeps.lock().unwrap().len(), 30, "15 s in half-second polls");
}

#[test]
fn the_wait_is_configurable() {
    let f = FakeSystem::new(Options { svc: "failed", restart_fixes: false, ..Options::default() });
    let (m, sleeps) = manager(&f, ScriptedPing::always(false));
    m.with_wait(Duration::from_secs(2)).repair_system();
    assert_eq!(sleeps.lock().unwrap().len(), 4);
}

#[test]
fn a_disabled_unit_is_enabled() {
    let f = FakeSystem::new(Options { enabled: "disabled", ..Options::default() });
    let (m, _) = manager(&f, ScriptedPing::always(true));
    m.repair_system();
    let calls = f.calls();
    assert!(calls.contains(&vec!["systemctl".into(), "enable".into(), "truesense.service".into()]));
    assert!(calls.contains(&vec!["systemctl".into(), "enable".into(), "truesense-health.timer".into()]));
}

#[test]
fn a_static_unit_cannot_be_enabled_so_it_is_not_tried() {
    let f = FakeSystem::new(Options { enabled: "static", ..Options::default() });
    let (m, _) = manager(&f, ScriptedPing::always(true));
    m.repair_system();
    assert!(f.changes().is_empty());
}

#[test]
fn a_stopped_health_timer_is_started() {
    let f = FakeSystem::new(Options::default());
    f.0.lock().unwrap().units.get_mut(&(false, "truesense-health.timer".to_owned())).unwrap().active =
        "inactive".into();
    let (m, _) = manager(&f, ScriptedPing::always(true));
    let steps = m.repair_system();
    assert!(texts(&steps).contains(&"started the health timer"), "{steps:?}");
    assert!(f.calls().contains(&vec!["systemctl".into(), "start".into(), "truesense-health.timer".into()]));
}

#[test]
fn a_missing_module_is_rebuilt_with_dkms_for_this_kernel_then_loaded() {
    let f = FakeSystem::new(Options { module: false, dkms_fixes: true, ..Options::default() });
    let (m, _) = manager(&f, ScriptedPing::always(true));
    let steps = m.repair_system();
    assert!(f.calls().contains(&vec!["dkms".into(), "autoinstall".into(), "-k".into(), KERNEL.into()]));
    assert!(f.module());
    assert!(steps.iter().any(|s| s.what == "kernel module loaded" && s.ok), "{steps:?}");
    assert!(steps.iter().any(|s| s.what == format!("rebuilt the kernel module for {KERNEL} (DKMS)") && s.ok));
}

#[test]
fn a_module_that_a_plain_modprobe_finds_is_not_rebuilt() {
    let f = FakeSystem::new(Options { module: false, ..Options::default() });
    f.0.lock().unwrap().dkms_ran = true; // the module is installed; only loading it was missing
    let (m, _) = manager(&f, ScriptedPing::always(true));
    let steps = m.repair_system();
    assert!(!f.calls().iter().any(|c| c[0] == "dkms"));
    assert!(steps.iter().any(|s| s.what == "kernel module loaded" && s.ok));
}

#[test]
fn a_module_that_never_appears_is_a_clear_failure() {
    let f = FakeSystem::new(Options { module: false, dkms_fixes: false, ..Options::default() });
    let (m, _) = manager(&f, ScriptedPing::always(true));
    let steps = m.repair_system();
    let bad: Vec<&str> = steps.iter().filter(|s| !s.ok).map(|s| s.what.as_str()).collect();
    assert!(bad.iter().any(|s| s.contains("kernel module still missing")), "{steps:?}");
    assert!(bad.iter().any(|s| s.contains("supported laptop")));
}

#[test]
fn the_second_run_changes_nothing() {
    let f = FakeSystem::new(Options { svc: "failed", enabled: "disabled", ..Options::default() });
    let (m, _) = manager(&f, ScriptedPing::script(&[false, true]));
    let first = m.repair_system();
    assert!(!first.is_empty() && !f.changes().is_empty());
    f.clear();
    let second = m.repair_system();
    assert!(f.changes().is_empty(), "idempotent: {:?}", f.changes());
    assert!(second.iter().all(|s| s.ok));
}

#[test]
fn the_repair_only_ever_runs_systemctl_modprobe_and_dkms_and_never_touches_a_setting() {
    let f = FakeSystem::new(Options {
        svc: "failed",
        enabled: "disabled",
        module: false,
        dkms_fixes: true,
        user: vec![("truesense-keys.service", "disabled", "inactive")],
        ..Options::default()
    });
    let (m, _) = manager(&f, ScriptedPing::script(&[false, true]));
    m.repair_system();
    m.repair_user();
    m.check();
    assert!(programs(&f.calls()).is_subset(&BTreeSet::from([
        "systemctl".to_owned(),
        "modprobe".to_owned(),
        "dkms".to_owned()
    ])));
    for call in f.calls() {
        let text = call.join(" ");
        assert!(
            !text.contains("fan") && !text.contains("/dev/truesense") && !text.contains("set_"),
            "{text}"
        );
    }
}

#[test]
fn a_failed_step_says_why_and_is_cut_to_100_characters() {
    let f = FakeSystem::new(Options::default());
    f.fail("systemctl daemon-reload", &format!("{}\n", "x".repeat(300)));
    let (m, _) = manager(&f, ScriptedPing::always(true));
    let first = &m.repair_system()[0];
    assert!(!first.ok);
    assert_eq!(first.what, format!("systemd re-reads the unit files ({})", "x".repeat(100)));
    f.fail("systemctl daemon-reload", "");
    assert_eq!(m.repair_system()[0].what, "systemd re-reads the unit files (failed)");
}

#[test]
fn a_failure_that_is_only_on_standard_output_is_still_explained() {
    struct Loud;
    impl Runner for Loud {
        fn run(&self, _: &str, _: &[&str]) -> Output {
            Output { stdout: "  from stdout \n".into(), stderr: String::new(), code: 3 }
        }
    }
    let m = Manager::new(ScriptedPing::always(false), KERNEL)
        .with_runner(Arc::new(Loud))
        .with_exists(Arc::new(|_| true))
        .with_sleeper(Arc::new(|_| {}));
    assert_eq!(m.repair_system()[0].what, "systemd re-reads the unit files (from stdout)");
}

// ---- repair: user -----------------------------------------------------------------------------

#[test]
fn stopped_services_are_enabled_and_started_and_running_ones_left_alone() {
    let f = FakeSystem::new(Options {
        user: vec![("truesense-keys.service", "disabled", "inactive")],
        ..Options::default()
    });
    let (m, _) = manager(&f, ScriptedPing::always(true));
    let steps = m.repair_user();
    assert!(f.calls().contains(&vec![
        "systemctl".into(),
        "--user".into(),
        "enable".into(),
        "--now".into(),
        "truesense-keys.service".into()
    ]));
    assert_eq!(
        f.calls().iter().filter(|c| c.iter().any(|a| a == "enable")).count(),
        1,
        "only the broken one"
    );
    assert!(steps.iter().all(|s| s.ok), "{steps:?}");
    assert!(texts(&steps).contains(&"enabled and started truesense-keys.service"));
    f.clear();
    let steps = m.repair_user();
    assert!(f.changes().is_empty());
    assert!(texts(&steps).iter().any(|s| s.contains("already running")));
    assert_eq!(steps.len(), 2);
}

#[test]
fn every_user_command_goes_through_the_user_manager() {
    let f = FakeSystem::new(Options {
        user: vec![("truesense-audio.service", "enabled", "failed")],
        ..Options::default()
    });
    let (m, _) = manager(&f, ScriptedPing::always(true));
    m.repair_user();
    assert!(f.calls().iter().all(|c| c[1] == "--user"), "{:?}", f.calls());
}

#[test]
fn a_unit_that_is_not_installed_is_reported_not_raised() {
    let f = FakeSystem::new(Options::default());
    f.remove_user_unit("truesense-audio.service");
    let (m, _) = manager(&f, ScriptedPing::always(true));
    let steps = m.repair_user();
    let bad = steps.iter().find(|s| !s.ok).unwrap();
    assert_eq!(
        bad.what,
        "could not start truesense-audio.service (Unit truesense-audio.service does not exist)"
    );
}

#[test]
fn a_unit_that_fails_silently_says_it_may_not_be_installed() {
    struct Silent;
    impl Runner for Silent {
        fn run(&self, _: &str, args: &[&str]) -> Output {
            if args.contains(&"enable") { Output::failure(1, "") } else { Output::success("") }
        }
    }
    let m = Manager::new(ScriptedPing::always(true), KERNEL).with_runner(Arc::new(Silent));
    let steps = m.repair_user();
    assert!(steps[1..].iter().all(|s| !s.ok && s.what.ends_with("(is it installed?)")), "{steps:?}");
    assert!(steps[0].ok);
}

#[test]
fn a_unit_that_came_up_despite_an_error_counts() {
    // `enable --now` returned an error but the unit is active afterwards: that is a success.
    struct Late(Mutex<bool>);
    impl Runner for Late {
        fn run(&self, _: &str, args: &[&str]) -> Output {
            let mut enabled = self.0.lock().unwrap();
            match args.get(1).copied() {
                Some("enable") => {
                    *enabled = true;
                    Output::failure(1, "warning")
                }
                Some("is-enabled") => Output::success(if *enabled { "enabled" } else { "disabled" }),
                Some("is-active") => Output::success(if *enabled { "active" } else { "inactive" }),
                _ => Output::success(""),
            }
        }
    }
    let m = Manager::new(ScriptedPing::always(true), KERNEL).with_runner(Arc::new(Late(Mutex::new(false))));
    let steps = m.repair_user();
    assert!(steps[1].ok && steps[1].what.starts_with("enabled and started"), "{steps:?}");
}

#[test]
fn a_failed_user_reload_is_a_failed_first_step() {
    let f = FakeSystem::new(Options::default());
    f.fail("systemctl daemon-reload", "no bus");
    let (m, _) = manager(&f, ScriptedPing::always(true));
    assert!(!m.repair_user()[0].ok);
}

// ---- the real runner (child processes of /bin/sh only; nothing system-wide) ----------------------

#[test]
fn the_system_runner_reports_output_status_and_missing_programs() {
    let r = SystemRunner::new(Duration::from_secs(10));
    let out = r.run("/bin/sh", &["-c", "printf out; printf err >&2; exit 3"]);
    assert_eq!(out, Output { stdout: "out".into(), stderr: "err".into(), code: 3 });
    assert_eq!(r.run("/bin/sh", &["-c", "true"]).code, 0);
    let missing = r.run("/nonexistent/program", &[]);
    assert_eq!(missing.code, NOT_RUNNABLE);
    assert!(missing.stderr.contains("/nonexistent/program"), "{missing:?}");
    assert_eq!(r.run("/bin/sh", &["-c", "kill -9 $$"]).code, -1, "ended by a signal");
}

#[test]
fn the_system_runner_kills_a_program_that_takes_too_long() {
    let r = SystemRunner::new(Duration::from_millis(200));
    let started = Instant::now();
    let out = r.run("/bin/sh", &["-c", "sleep 30"]);
    assert_eq!(out.code, NOT_RUNNABLE);
    assert!(out.stderr.contains("did not finish in time"), "{out:?}");
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn a_timeout_does_not_wait_for_a_grandchild_that_keeps_the_pipes_open() {
    let r = SystemRunner::new(Duration::from_millis(200));
    let started = Instant::now();
    // The shell forks `sleep` (it is not the last command) and the kill of the shell leaves it behind.
    let out = r.run("/bin/sh", &["-c", "sleep 3; true"]);
    assert_eq!(out.code, NOT_RUNNABLE);
    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
}

#[test]
fn the_system_runner_survives_a_program_that_prints_a_lot() {
    let r = SystemRunner::new(Duration::from_secs(20));
    // 8 MiB of output: more than a pipe holds and more than is kept.
    let out = r.run("/bin/sh", &["-c", "head -c 8388608 /dev/zero | tr '\\0' x"]);
    assert_eq!(out.code, 0);
    assert_eq!(out.stdout.len() as u64, OUTPUT_LIMIT);
}

#[test]
fn the_defaults_are_the_real_machine() {
    let m = Manager::new(ScriptedPing::always(true), KERNEL);
    assert_eq!(m.socket_path, Path::new("/run/truesense/truesense.sock"));
    assert_eq!(m.device_path, Path::new("/dev/truesense"));
    assert_eq!(m.wait, Duration::from_secs(15));
    assert!(format!("{m:?}").contains(KERNEL));
}
