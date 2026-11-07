#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::io;
use std::sync::Mutex;

use super::*;
use crate::power::fake::{FakeRunner, MemSysfs, OnWrite};

const CPU: &str = "/sys/devices/system/cpu";

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ts-domain-pm-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// systemd as far as this module sees it: units with an active and an enabled flag.
struct Systemd {
    units: Mutex<BTreeMap<String, (bool, bool)>>,
    refuse_disable: bool,
    disable_errors_but_stops: bool,
}

type Rig = (PowerManagers, Arc<MemSysfs>, Arc<FakeRunner>, Arc<Systemd>, PathBuf);

impl Systemd {
    fn handle(&self, args: &[String]) -> io::Result<Vec<u8>> {
        assert!(args.len() >= 2, "systemctl is always called with a verb and a unit: {args:?}");
        let unit = args.last().unwrap().trim_end_matches(".service").to_owned();
        let verb = args[0].as_str();
        let mut units = self.units.lock().unwrap();
        let state = units.entry(unit).or_insert((false, false));
        let text = match verb {
            "is-active" => {
                if state.0 {
                    "active\n"
                } else {
                    "inactive\n"
                }
            }
            "is-enabled" => {
                if state.1 {
                    "enabled\n"
                } else {
                    "disabled\n"
                }
            }
            "disable" if self.refuse_disable => return Ok(b"Access denied\n".to_vec()),
            "disable" => {
                *state = (false, false);
                if self.disable_errors_but_stops {
                    return Err(io::Error::other("timed out after stopping it"));
                }
                ""
            }
            "enable" => {
                state.1 = true;
                ""
            }
            "start" => {
                state.0 = true;
                ""
            }
            other => panic!("unexpected systemctl verb {other}"),
        };
        Ok(text.as_bytes().to_vec())
    }
}

/// A laptop with auto-cpufreq running, turbo left off, three cores whose ceiling turbo-off lowered,
/// and an unrelated service that must be left alone.
fn rig(tag: &str, tweak: impl FnOnce(&mut Systemd)) -> Rig {
    let mut systemd = Systemd {
        units: Mutex::new(BTreeMap::from([
            ("auto-cpufreq".to_owned(), (true, true)),
            ("tlp".to_owned(), (false, false)),
            ("ananicy-cpp".to_owned(), (true, true)),
        ])),
        refuse_disable: false,
        disable_errors_but_stops: false,
    };
    tweak(&mut systemd);
    let systemd = Arc::new(systemd);
    let sys = Arc::new(MemSysfs::default());
    sys.put(&format!("{CPU}/intel_pstate/no_turbo"), "1\n");
    for n in 0..3 {
        let base = format!("{CPU}/cpu{n}/cpufreq");
        sys.put(&format!("{base}/scaling_governor"), "performance\n");
        sys.put(&format!("{base}/energy_performance_preference"), "performance\n");
        sys.put(&format!("{base}/cpuinfo_max_freq"), "5000000\n");
        sys.put(&format!("{base}/scaling_max_freq"), "2100000\n"); // the ceiling turbo-off left behind
    }
    sys.put(&format!("{CPU}/cpufreq/policy0"), "x"); // not a core
    let s = systemd.clone();
    let runner = Arc::new(FakeRunner::new(move |program, args| {
        assert_eq!(program, "systemctl");
        s.handle(args)
    }));
    let dir = temp_dir(tag);
    let record = dir.join("rec.json");
    let manager = PowerManagers::new(runner.clone(), sys.clone(), CPU, &record);
    (manager, sys, runner, systemd, dir)
}

fn unit(systemd: &Systemd, name: &str) -> (bool, bool) {
    systemd.units.lock().unwrap()[name]
}

fn cpu(sys: &MemSysfs, n: u32, file: &str) -> String {
    sys.get(&format!("{CPU}/cpu{n}/cpufreq/{file}")).unwrap()
}

#[test]
fn status_lists_only_known_running_or_enabled_managers() {
    let (pm, _, runner, _, dir) = rig("status", |_| {});
    assert_eq!(
        pm.status(),
        [UnitStatus { unit: "auto-cpufreq".into(), active: true, enabled: true }],
        "tlp (off) and ananicy-cpp (unknown) are not listed"
    );
    let asked: Vec<String> = runner.lines();
    assert_eq!(asked.len(), UNITS.len() * 2);
    assert!(asked.iter().all(|l| UNITS.iter().any(|u| l.ends_with(&format!("{u}.service")))), "{asked:?}");
    std::fs::remove_dir_all(dir).unwrap();
}

// Python: test_takeover_stops_only_known_managers_fixes_the_cpu_and_restore_undoes_it.
#[test]
fn takeover_stops_only_known_managers_fixes_the_cpu_and_restore_undoes_it() {
    let (pm, sys, runner, systemd, dir) = rig("takeover", |_| {});
    let r = pm.takeover().unwrap();
    assert_eq!(r.found, ["auto-cpufreq"]);
    assert_eq!(r.stopped, ["auto-cpufreq"]);
    assert!(r.failed.is_empty() && r.cpu_not_set.is_empty() && r.ceiling_fixed);
    assert_eq!(unit(&systemd, "auto-cpufreq"), (false, false));
    assert_eq!(unit(&systemd, "ananicy-cpp"), (true, true), "unrelated services are left alone");
    assert_eq!(
        pm.cpu_state(),
        CpuState {
            no_turbo: Some("0".into()),
            governor: Some("powersave".into()),
            epp: Some("balance_performance".into())
        }
    );
    assert_eq!(r.cpu, pm.cpu_state());
    for line in runner.lines() {
        let name = line.split(' ').next_back().unwrap().trim_end_matches(".service").to_owned();
        assert!(UNITS.contains(&name.as_str()), "touched {line}");
    }
    assert_eq!(
        pm.load_record().cpu,
        Some(CpuState {
            no_turbo: Some("1".into()),
            governor: Some("performance".into()),
            epp: Some("performance".into())
        })
    );
    assert!(pm.ceiling_low().is_empty(), "the ceilings turbo-off left low are raised to each core's maximum");
    assert_eq!(cpu(&sys, 1, "scaling_max_freq"), "5000000");

    // A second takeover keeps the ORIGINAL state to go back to.
    pm.takeover().unwrap();
    assert_eq!(pm.load_record().cpu.unwrap().governor.as_deref(), Some("performance"));

    let back = pm.restore();
    assert_eq!(back.restored, ["auto-cpufreq"]);
    assert_eq!(unit(&systemd, "auto-cpufreq"), (true, true));
    assert_eq!(
        pm.cpu_state(),
        CpuState {
            no_turbo: Some("1".into()),
            governor: Some("performance".into()),
            epp: Some("performance".into())
        }
    );
    assert!(back.cpu_not_set.is_empty() && back.note.is_none());
    assert!(pm.load_record().is_empty() && !dir.join("rec.json").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

// Python: test_a_hand_edited_record_cannot_touch_other_services_and_failures_are_reported.
#[test]
fn a_hand_edited_record_cannot_touch_other_services() {
    let (pm, _, runner, _, dir) = rig("handedited", |_| {});
    let hostile = [
        r#"{"units": [{"unit": "sshd", "was_active": true, "was_enabled": true}], "cpu": null}"#,
        r#"{"units": [{"unit": "../../etc/passwd", "was_active": true}, {"unit": "sshd.service", "was_enabled": true}]}"#,
        r#"{"units": [{"unit": "ananicy-cpp", "was_active": 1, "was_enabled": "yes"}]}"#,
        r#"{"units": [{"unit": "auto-cpufreq --now", "was_active": true}]}"#,
    ];
    for text in hostile {
        std::fs::write(dir.join("rec.json"), text).unwrap();
        runner.calls.lock().unwrap().clear();
        let report = pm.restore();
        assert!(
            runner.lines().is_empty(),
            "{text}: only units from the fixed list are ever started or enabled: {:?}",
            runner.lines()
        );
        assert!(report.restored.is_empty(), "{text}");
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn failures_are_reported() {
    let (pm, _, _, systemd, dir) = rig("failed", |s| s.refuse_disable = true);
    let r = pm.takeover().unwrap();
    assert!(r.stopped.is_empty());
    assert_eq!(r.failed, ["auto-cpufreq: Access denied"]);
    assert_eq!(unit(&systemd, "auto-cpufreq"), (true, true));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_disable_that_errored_but_left_the_unit_stopped_counts_as_stopped() {
    let (pm, _, _, systemd, dir) = rig("errstop", |s| s.disable_errors_but_stops = true);
    let r = pm.takeover().unwrap();
    assert_eq!((r.stopped, r.failed), (vec!["auto-cpufreq".to_owned()], Vec::<String>::new()));
    assert!(!unit(&systemd, "auto-cpufreq").0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_long_complaint_is_cut_to_80_characters() {
    let (pm, _, _, _, dir) = rig("long", |_| {});
    let runner = Arc::new(FakeRunner::new(|_, args| {
        Ok(if args[0] == "is-active" || args[0] == "is-enabled" {
            b"active\n".to_vec()
        } else {
            "é".repeat(500).into_bytes()
        })
    }));
    let pm2 = PowerManagers::new(runner, Arc::new(MemSysfs::default()), CPU, dir.join("rec2.json"));
    let r = pm2.takeover().unwrap();
    assert_eq!(r.failed.len(), UNITS.len());
    assert!(
        r.failed.iter().all(|f| f.chars().count() <= 80 + "power-profiles-daemon: ".len()),
        "{:?}",
        r.failed
    );
    drop(pm);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_record_is_written_before_anything_is_stopped() {
    let (_, sys, _, systemd, dir) = rig("order", |_| {});
    let record = dir.join("rec.json");
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let (s, r, seen2) = (systemd.clone(), record.clone(), seen.clone());
    let runner = Arc::new(FakeRunner::new(move |_, args| {
        if args[0] == "disable" {
            seen2.lock().unwrap().push(std::fs::read_to_string(&r).unwrap_or_default());
        }
        s.handle(args)
    }));
    let pm = PowerManagers::new(runner, sys, CPU, &record);
    pm.takeover().unwrap();
    let at_disable = seen.lock().unwrap().clone();
    assert_eq!(at_disable.len(), 1);
    let on_disk: Value = serde_json::from_str(&at_disable[0])
        .expect("the record existed and was valid when the unit was stopped");
    assert_eq!(on_disk["units"][0]["unit"], "auto-cpufreq");
    assert_eq!(on_disk["units"][0]["was_active"], true);
    assert_eq!(on_disk["cpu"]["no_turbo"], "1");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn takeover_changes_nothing_when_the_record_cannot_be_written() {
    let (_, sys, runner, systemd, dir) = rig("norecord", |_| {});
    std::fs::write(dir.join("blocker"), "a file where a directory is needed").unwrap();
    let pm = PowerManagers::new(runner.clone(), sys.clone(), CPU, dir.join("blocker/rec.json"));
    runner.calls.lock().unwrap().clear();
    let err = pm.takeover().unwrap_err();
    assert!(err.to_string().starts_with("could not remember what was stopped ("), "{err}");
    assert!(err.to_string().ends_with("nothing was changed"));
    assert!(std::error::Error::source(&err).is_some());
    assert_eq!(unit(&systemd, "auto-cpufreq"), (true, true));
    assert!(sys.writes.lock().unwrap().is_empty(), "no CPU setting was written");
    assert!(
        runner.lines().iter().all(|l| l.contains("is-active") || l.contains("is-enabled")),
        "{:?}",
        runner.lines()
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_low_ceiling_is_repaired_even_when_no_manager_is_running() {
    let (pm, sys, _, systemd, dir) = rig("ceiling", |s| {
        *s.units.lock().unwrap().get_mut("auto-cpufreq").unwrap() = (false, false);
    });
    sys.put(&format!("{CPU}/intel_pstate/no_turbo"), "0");
    for n in 0..3 {
        sys.put(&format!("{CPU}/cpu{n}/cpufreq/scaling_governor"), "schedutil");
    }
    assert_eq!(pm.ceiling_low().len(), 3);
    let r = pm.takeover().unwrap();
    assert!(r.found.is_empty() && r.ceiling_fixed);
    assert_eq!(
        pm.cpu_state().governor.as_deref(),
        Some("schedutil"),
        "your governor is not touched when nothing was in charge and turbo is on"
    );
    assert_eq!(unit(&systemd, "auto-cpufreq"), (false, false));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn turbo_left_off_is_fixed_even_without_a_manager() {
    let (pm, sys, _, _, dir) = rig("turbo", |s| {
        *s.units.lock().unwrap().get_mut("auto-cpufreq").unwrap() = (false, false);
    });
    let r = pm.takeover().unwrap();
    assert!(r.found.is_empty());
    assert_eq!(sys.get(&format!("{CPU}/intel_pstate/no_turbo")).unwrap(), "0");
    assert_eq!(pm.cpu_state().governor.as_deref(), Some("powersave"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cpu_not_set_reports_what_did_not_take() {
    let (pm, sys, _, _, dir) = rig("notset", |_| {});
    sys.on_write(|path, _| {
        let p = path.to_string_lossy();
        if p.ends_with("no_turbo") || p.ends_with("cpu1/cpufreq/scaling_governor") {
            OnWrite::Ignore
        } else {
            OnWrite::Keep
        }
    });
    let r = pm.takeover().unwrap();
    assert_eq!(r.cpu_not_set, ["turbo", "governor"]);
    // The EPP went in on every core, the governor on all but one, turbo not at all.
    assert_eq!(cpu(&sys, 0, "energy_performance_preference"), "balance_performance");
    assert_eq!(cpu(&sys, 1, "scaling_governor"), "performance\n");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn set_cpu_without_intel_pstate_or_files() {
    let sys = Arc::new(MemSysfs::default());
    let dir = temp_dir("bare");
    let runner = Arc::new(FakeRunner::new(|_, _| Ok(b"inactive\n".to_vec())));
    let pm = PowerManagers::new(runner, sys.clone(), CPU, dir.join("rec.json"));
    assert_eq!(pm.cpu_state(), CpuState::default());
    let r = pm.takeover().unwrap();
    assert!(r.cpu_not_set.is_empty() && r.ceiling_fixed && r.found.is_empty());
    assert!(sys.writes.lock().unwrap().is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn ceiling_low_ignores_unreadable_and_non_numeric_files() {
    let (pm, sys, _, _, dir) = rig("odd", |_| {});
    let base = |n: u32, f: &str| format!("{CPU}/cpu{n}/cpufreq/{f}");
    sys.put(&base(0, "scaling_max_freq"), "5000000"); // fine
    sys.put(&base(1, "scaling_max_freq"), "-1"); // a sign is not digits
    sys.put(&base(2, "cpuinfo_max_freq"), "max\n");
    sys.put(&format!("{CPU}/cpu3/cpufreq/cpuinfo_max_freq"), "5000000"); // no scaling_max_freq at all
    sys.put(&format!("{CPU}/cpu4/cpufreq/cpuinfo_max_freq"), "99999999999999999999999999");
    sys.put(&format!("{CPU}/cpu4/cpufreq/scaling_max_freq"), "1");
    sys.put(&format!("{CPU}/cpu5/cpufreq/cpuinfo_max_freq"), "");
    sys.put(&format!("{CPU}/cpu5/cpufreq/scaling_max_freq"), "");
    assert!(pm.ceiling_low().is_empty());
    sys.put(&base(0, "scaling_max_freq"), "4999999");
    assert_eq!(pm.ceiling_low(), [PathBuf::from(format!("{CPU}/cpu0/cpufreq"))]);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn fix_ceiling_reports_cores_that_did_not_take_it() {
    let (pm, sys, _, _, dir) = rig("fixceil", |_| {});
    sys.on_write(
        |path, _| {
            if path.to_string_lossy().contains("cpu2/") { OnWrite::Ignore } else { OnWrite::Keep }
        },
    );
    assert_eq!(pm.fix_ceiling(), ["cpu2"]);
    assert_eq!(cpu(&sys, 0, "scaling_max_freq"), "5000000");
    assert_eq!(cpu(&sys, 2, "scaling_max_freq"), "2100000\n");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cores_are_matched_like_the_glob_cpu_digit_star() {
    let sys = Arc::new(MemSysfs::default());
    for name in ["cpu0", "cpu1", "cpu10", "cpu2", "cpufreq", "cpuidle", "cpu", "cpuX", "online"] {
        sys.put(&format!("{CPU}/{name}/cpufreq/scaling_governor"), "x");
    }
    let dir = temp_dir("glob");
    let runner = Arc::new(FakeRunner::new(|_, _| Ok(Vec::new())));
    let pm = PowerManagers::new(runner, sys, CPU, dir.join("r.json"));
    assert_eq!(pm.cores(), ["cpu0", "cpu1", "cpu2", "cpu10"]);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn restore_with_nothing_on_record() {
    let (pm, _, runner, _, dir) = rig("empty", |_| {});
    runner.calls.lock().unwrap().clear();
    let r = pm.restore();
    assert_eq!(r.note.as_deref(), Some("nothing was changed by TrueSense"));
    assert!(r.restored.is_empty());
    assert!(runner.lines().is_empty());
    assert!(!pm.recorded());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn report_json_shapes_match_python() {
    let (pm, _, _, _, dir) = rig("json", |_| {});
    let t = serde_json::to_value(pm.takeover().unwrap()).unwrap();
    let mut keys: Vec<&str> = t.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["ceiling_fixed", "cpu", "cpu_not_set", "failed", "found", "stopped"]);
    assert_eq!(t["cpu"]["no_turbo"], "0");
    assert_eq!(t["found"], serde_json::json!(["auto-cpufreq"]));

    assert!(pm.recorded());
    let r = serde_json::to_value(pm.restore()).unwrap();
    let mut keys: Vec<&str> = r.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["cpu", "cpu_not_set", "restored"]);

    let n = serde_json::to_value(pm.restore()).unwrap();
    let mut keys: Vec<&str> = n.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["cpu", "note", "restored"]);
    assert_eq!(n["note"], "nothing was changed by TrueSense");

    // An unreadable value is null, like Python's None.
    let bare = serde_json::to_value(CpuState::default()).unwrap();
    assert_eq!(bare, serde_json::json!({"no_turbo": null, "governor": null, "epp": null}));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn record_reading_is_lenient_and_never_panics() {
    let table: [(&str, usize, bool); 14] = [
        (
            r#"{"units":[{"unit":"tlp","was_active":true,"was_enabled":false}],"cpu":{"no_turbo":"1","governor":"performance","epp":null}}"#,
            1,
            true,
        ),
        (r#"{"cpu":{"no_turbo":1,"governor":["x"],"epp":"balance_power"}}"#, 0, true),
        (r#"{"units":[{"unit":"tlp"},"junk",5,null,{"was_active":true},{"unit":""},{"unit":7}]}"#, 1, false),
        (r#"{}"#, 0, false),
        ("null", 0, false),
        ("[]", 0, false),
        (r#"{"units":"x"}"#, 0, false),
        (r#"{"cpu":"x"}"#, 0, false),
        (r#"{"cpu":{"no_turbo":[1]}}"#, 0, true),
        (r#"{"units":[{"unit":"sshd","was_active":true}]}"#, 1, false), // read, but restore will never touch it
        ("{", 0, false),
        ("", 0, false),
        ("\u{0}\u{1}", 0, false),
        (r#"{"units":[{"unit":"tlp","was_active":{"a":1},"was_enabled":[1]}]}"#, 1, false),
    ];
    let dir = temp_dir("lenient");
    let pm = PowerManagers::new(
        Arc::new(FakeRunner::new(|_, _| Ok(Vec::new()))),
        Arc::new(MemSysfs::default()),
        CPU,
        dir.join("rec.json"),
    );
    for (text, units, has_cpu) in table {
        std::fs::write(dir.join("rec.json"), text).unwrap();
        let record = pm.load_record();
        assert_eq!(record.units.len(), units, "{text}");
        assert_eq!(record.cpu.is_some(), has_cpu, "{text}");
    }
    let r = Record::from_value(&serde_json::from_str(table[0].0).unwrap());
    assert_eq!(r.cpu.unwrap().epp, None, "a null value is not remembered as text");
    // A missing file is an empty record; so is one that is too big.
    std::fs::remove_file(dir.join("rec.json")).unwrap();
    assert!(pm.load_record().is_empty());
    std::fs::write(dir.join("rec.json"), vec![b' '; (MAX_RECORD + 1) as usize]).unwrap();
    assert!(pm.load_record().is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn record_path_follows_the_environment() {
    assert_eq!(record_path_from(&|_| None), PathBuf::from("/var/lib/truesense/power_managers.json"));
    assert_eq!(
        record_path_from(&|k| (k == "TRUESENSE_PM_RECORD").then(|| "/tmp/x.json".to_owned())),
        PathBuf::from("/tmp/x.json")
    );
}

#[test]
fn the_unit_list_is_the_fixed_one() {
    assert_eq!(
        UNITS,
        ["auto-cpufreq", "tlp", "tuned", "tuned-ppd", "power-profiles-daemon", "system76-power"]
    );
    assert!(
        is_known_unit("tlp") && !is_known_unit("sshd") && !is_known_unit("tlp.service") && !is_known_unit("")
    );
}
