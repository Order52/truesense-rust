#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io;
use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::json;

use super::helper::{CALL_TIMEOUT, Locator, find_helper, helper_candidates, is_executable_file, parse_reply};
use super::*;
use crate::power::fake::FakeRunner;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ts-domain-gpu-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ---- the controller over a fake NVML ---------------------------------------------------------

#[derive(Default)]
struct GpuState {
    offset: i32,
    range: Option<(i32, i32)>,
    locked: Option<(u32, u32)>,
    calls: Vec<String>,
    fail: Option<(&'static str, i32)>, // call name -> NVML code
    ignore_offset: bool,
    no_connection: bool,
}

struct FakeNvml(Mutex<GpuState>);

impl FakeNvml {
    fn new(f: impl FnOnce(&mut GpuState)) -> Arc<Self> {
        let mut state = GpuState { range: Some((-1000, 1000)), ..GpuState::default() };
        f(&mut state);
        Arc::new(Self(Mutex::new(state)))
    }

    fn log(&self, call: &str) -> Result<(), NvmlError> {
        let mut s = self.0.lock().unwrap();
        s.calls.push(call.to_owned());
        if s.no_connection {
            return Err(NvmlError::LibraryNotFound);
        }
        match s.fail {
            Some((name, code)) if call.starts_with(name) => Err(NvmlError::Code(code)),
            _ => Ok(()),
        }
    }

    fn calls(&self) -> Vec<String> {
        self.0.lock().unwrap().calls.clone()
    }

    fn state(&self) -> (i32, Option<(u32, u32)>) {
        let s = self.0.lock().unwrap();
        (s.offset, s.locked)
    }
}

impl Nvml for FakeNvml {
    fn offset(&self) -> Result<i32, NvmlError> {
        self.log("offset")?;
        Ok(self.0.lock().unwrap().offset)
    }

    fn offset_range(&self) -> Result<(i32, i32), NvmlError> {
        self.log("range")?;
        self.0.lock().unwrap().range.ok_or(NvmlError::Code(3))
    }

    fn set_offset(&self, mhz: i32) -> Result<(), NvmlError> {
        self.log(&format!("set-offset {mhz}"))?;
        let mut s = self.0.lock().unwrap();
        if !s.ignore_offset {
            s.offset = mhz;
        }
        Ok(())
    }

    fn lock_clocks(&self, min_mhz: u32, max_mhz: u32) -> Result<(), NvmlError> {
        self.log(&format!("lock {min_mhz} {max_mhz}"))?;
        self.0.lock().unwrap().locked = Some((min_mhz, max_mhz));
        Ok(())
    }

    fn reset_clocks(&self) -> Result<(), NvmlError> {
        self.log("reset-clocks")?;
        self.0.lock().unwrap().locked = None;
        Ok(())
    }
}

#[test]
fn apply_and_reset() {
    let nv = FakeNvml::new(|_| {});
    let gpu = GpuUndervolt::new(nv.clone());
    assert_eq!(gpu.apply(120, 2400).unwrap(), Applied { offset_mhz: 120, cap_mhz: 2400 });
    assert_eq!(nv.state(), (120, Some((210, 2400))), "the idle clock is the floor of the locked range");
    assert_eq!(gpu.read().unwrap(), State { offset_mhz: 120, range_mhz: [-1000, 1000] });
    // No cap: the old lock is lifted.
    assert_eq!(gpu.apply(60, 0).unwrap(), Applied { offset_mhz: 60, cap_mhz: 0 });
    assert_eq!(nv.state(), (60, None));
    gpu.apply(100, 2100).unwrap();
    assert_eq!(gpu.reset().unwrap(), Applied { offset_mhz: 0, cap_mhz: 0 });
    assert_eq!(nv.state(), (0, None));
}

#[test]
fn apply_refuses_values_outside_the_limits_before_touching_the_gpu() {
    let nv = FakeNvml::new(|_| {});
    let gpu = GpuUndervolt::new(nv.clone());
    let refused = [
        (251, 2400, "offset: -300..250 MHz"),
        (-301, 0, "offset: -300..250 MHz"),
        (i32::MAX, 0, "offset: -300..250 MHz"),
        (100, 599, "cap: 0 (none) or 600..3300 MHz"),
        (100, 3301, "cap: 0 (none) or 600..3300 MHz"),
        (100, -1, "cap: 0 (none) or 600..3300 MHz"),
        (100, i32::MIN, "cap: 0 (none) or 600..3300 MHz"),
    ];
    for (offset, cap, message) in refused {
        let err = gpu.apply(offset, cap).unwrap_err();
        assert!(matches!(err, GpuError::Invalid(_)), "({offset}, {cap}): {err}");
        assert_eq!(err.to_string(), message);
    }
    assert!(nv.calls().is_empty(), "a refused value never reaches NVML: {:?}", nv.calls());
    for (offset, cap) in [(-300, 600), (250, 3300), (0, 0), (-300, 0)] {
        gpu.apply(offset, cap).unwrap();
    }
}

#[test]
fn apply_enforces_the_driver_range() {
    let nv = FakeNvml::new(|s| s.range = Some((-100, 200)));
    let gpu = GpuUndervolt::new(nv.clone());
    let err = gpu.apply(250, 0).unwrap_err();
    assert!(matches!(err, GpuError::DriverRange { lo: -100, hi: 200 }));
    assert_eq!(err.to_string(), "the driver allows offsets -100..200 MHz");
    assert!(gpu.apply(-200, 0).is_err());
    assert!(!nv.calls().iter().any(|c| c.starts_with("set-offset")), "nothing was written");
    gpu.apply(200, 0).unwrap();
}

#[test]
fn an_unknown_range_turns_the_range_check_off() {
    let nv = FakeNvml::new(|s| s.range = None);
    let gpu = GpuUndervolt::new(nv.clone());
    assert_eq!(gpu.read().unwrap().range_mhz, [0, 0]);
    gpu.apply(250, 0).unwrap();
}

#[test]
fn apply_verifies_the_offset_and_puts_the_gpu_back_to_stock() {
    let nv = FakeNvml::new(|s| s.ignore_offset = true);
    let err = GpuUndervolt::new(nv.clone()).apply(100, 2100).unwrap_err();
    assert!(matches!(err, GpuError::OffsetNotKept { got: 0, wanted: 100 }), "{err}");
    assert_eq!(err.to_string(), "the GPU kept offset 0 MHz instead of 100 (driver limit?)");
    assert_eq!(nv.state().1, None, "the clock cap that did go in is lifted again");
}

#[test]
fn a_failed_cap_resets_the_offset_that_was_written() {
    let nv = FakeNvml::new(|s| s.fail = Some(("lock", 4)));
    let err = GpuUndervolt::new(nv.clone()).apply(100, 2100).unwrap_err();
    assert_eq!(err.to_string(), "capping the GPU clock: needs root (the TrueSense service)");
    assert_eq!(nv.state().0, 0, "the offset is back at stock");
}

#[test]
fn driver_error_codes_are_named() {
    for (code, text) in [
        (2, "invalid argument"),
        (3, "not supported by this GPU or driver"),
        (4, "needs root (the TrueSense service)"),
        (9, "driver not loaded"),
        (15, "GPU is lost"),
        (999, "unknown error"),
        (77, "NVML error 77"),
        (-1, "NVML error -1"),
    ] {
        assert_eq!(NvmlError::Code(code).to_string(), text);
    }
    let nv = FakeNvml::new(|s| s.fail = Some(("set-offset", 4)));
    assert_eq!(
        GpuUndervolt::new(nv).apply(10, 0).unwrap_err().to_string(),
        "setting the GPU offset: needs root (the TrueSense service)"
    );
    let nv = FakeNvml::new(|s| s.fail = Some(("offset", 15)));
    assert_eq!(GpuUndervolt::new(nv).read().unwrap_err().to_string(), "reading the GPU offset: GPU is lost");
}

#[test]
fn reaching_the_gpu_fails_in_plain_words() {
    let nv = FakeNvml::new(|s| s.no_connection = true);
    let gpu = GpuUndervolt::new(nv);
    assert_eq!(gpu.read().unwrap_err().to_string(), "NVIDIA driver library (libnvidia-ml) not found");
    assert_eq!(gpu.apply(10, 0).unwrap_err().to_string(), "NVIDIA driver library (libnvidia-ml) not found");
    assert!(gpu.reset().is_err());
}

#[test]
fn reset_tries_both_steps_and_tolerates_unsupported_clock_lock() {
    // Both fail: both are reported, and the cap step was still tried.
    let nv = FakeNvml::new(|s| s.fail = Some(("", 4)));
    let err = GpuUndervolt::new(nv.clone()).reset().unwrap_err();
    assert!(matches!(&err, GpuError::Several(e) if e.len() == 2), "{err}");
    assert_eq!(
        err.to_string(),
        "resetting the GPU offset: needs root (the TrueSense service); lifting the GPU clock cap: needs root (the TrueSense service)"
    );
    assert_eq!(nv.calls(), ["set-offset 0", "reset-clocks"]);

    // Only the offset fails: the cap is lifted anyway.
    let nv = FakeNvml::new(|s| {
        s.fail = Some(("set-offset", 15));
        s.locked = Some((210, 2000));
    });
    assert!(GpuUndervolt::new(nv.clone()).reset().is_err());
    assert_eq!(nv.state().1, None);

    // "Not supported" while lifting a cap is success: there is nothing to lift.
    let nv = FakeNvml::new(|s| s.fail = Some(("reset-clocks", 3)));
    assert_eq!(GpuUndervolt::new(nv.clone()).reset().unwrap(), Applied { offset_mhz: 0, cap_mhz: 0 });
    assert_eq!(GpuUndervolt::new(nv).apply(50, 0).unwrap().offset_mhz, 50);
}

// ---- risk ------------------------------------------------------------------------------------

// Python: test_undervolt_risk_zones (the GPU part).
#[test]
fn risk_zones() {
    let table = [
        (120, 2400, Level::Safe),
        (180, 2400, Level::Caution),
        (220, 2400, Level::Danger),
        (100, 0, Level::Caution),
        (150, 2400, Level::Safe),
        (151, 2400, Level::Caution),
        (200, 2400, Level::Caution),
        (201, 2400, Level::Danger),
        (-101, 0, Level::Caution),
        (-100, 0, Level::Safe),
        (50, 1499, Level::Caution),
        (50, 1500, Level::Safe),
        (0, 0, Level::Safe),
        (0, 1000, Level::Caution),
        (i32::MAX, 0, Level::Danger),
        (i32::MIN, 0, Level::Caution),
    ];
    for (offset, cap, level) in table {
        assert_eq!(risk(offset, cap).level, level, "risk({offset}, {cap})");
    }
    assert_eq!(risk(0, 0).message, "Stock.");
    assert_eq!(risk(120, 2400).message, "Inside the safe range.");
    assert_eq!(
        risk(220, 2400).message,
        "Beyond the danger line (+200 MHz). Games and the driver can crash; the screen can glitch."
    );
    assert_eq!(
        risk(180, 2400).message,
        "Past the safety line (+150 MHz). It may crash in some games: test under load."
    );
    assert_eq!(risk(50, 1000).message, "A cap this low will cost a lot of speed in games.");
}

// ---- profiles --------------------------------------------------------------------------------

#[test]
fn builtins_are_the_python_ones() {
    let want = [
        ("native", 0, 0),
        ("truesense", 120, 2400),
        ("mild", 60, 2500),
        ("cool", 100, 2100),
        ("efficient", 150, 1800),
    ];
    assert_eq!(builtin_names(), want.map(|w| w.0));
    for (name, offset, cap) in want {
        let p = builtin(name).unwrap();
        assert_eq!((p.offset, p.cap), (offset, cap), "{name}");
        assert!(!p.description.is_empty());
        p.check().unwrap();
        assert_eq!(risk(p.offset, p.cap).level, Level::Safe, "{name}");
        assert!(is_builtin(name));
    }
    assert!(builtin("aggressive").is_none());
}

#[test]
fn profile_validation_table() {
    let ok = |v: serde_json::Value, want: (&str, i32, i32)| {
        let p = Profile::from_value(&v).unwrap_or_else(|e| panic!("{v}: {e}"));
        assert_eq!((p.description.as_str(), p.offset, p.cap), want, "{v}");
    };
    ok(json!({"offset": 120, "cap": 2400, "description": "d"}), ("d", 120, 2400));
    ok(json!({"offset": 0}), ("", 0, 0)); // a missing cap is "no cap"
    ok(json!({"offset": -300, "cap": 600}), ("", -300, 600));
    ok(json!({"offset": 250, "cap": 3300}), ("", 250, 3300));
    ok(json!({"offset": 80.9, "cap": 2300.7}), ("", 80, 2300));
    ok(json!({"offset": -80.9, "cap": 0.0}), ("", -80, 0));
    ok(json!({"offset": 1, "x": 5, "description": 9}), ("", 1, 0));

    let offset = "offset: -300..250 MHz";
    let cap = "cap: 0 (none) or 600..3300 MHz";
    let shape = "a GPU undervolt profile has 'offset' (MHz) and 'cap' (MHz, 0 = none)";
    let bad: [(serde_json::Value, &str); 16] = [
        (json!({"offset": 251}), offset),
        (json!({"offset": 250.5}), offset),
        (json!({"offset": -301}), offset),
        (json!({}), offset),
        (json!({"offset": "50"}), offset),
        (json!({"offset": true}), offset),
        (json!({"offset": null}), offset),
        (json!({"offset": 1e300}), offset),
        (json!({"offset": 10, "cap": 100}), cap),
        (json!({"offset": 10, "cap": 3301}), cap),
        (json!({"offset": 10, "cap": 0.5}), cap),
        (json!({"offset": 10, "cap": null}), cap),
        (json!({"offset": 10, "cap": "2400"}), cap),
        (json!([1]), shape),
        (json!(null), shape),
        (json!(5), shape),
    ];
    for (value, message) in bad {
        let err = Profile::from_value(&value).unwrap_err();
        assert_eq!(err.to_string(), message, "{value}");
    }
}

#[test]
fn store_round_trip_copy_delete_and_messages() {
    let dir = temp_dir("store");
    let store = Store::new(&dir);
    assert_eq!(store.names().unwrap(), builtin_names());
    assert_eq!(store.load("stock").unwrap().offset, 0);

    let mine = Profile { description: "mine".into(), offset: 80, cap: 2300 };
    store.save("gmine2", &mine).unwrap();
    assert_eq!(store.load("gmine2").unwrap(), mine);
    assert_eq!(
        std::fs::read_to_string(dir.join("gmine2.json")).unwrap(),
        "{\n \"description\": \"mine\",\n \"offset\": 80,\n \"cap\": 2300\n}"
    );
    assert_eq!(store.copy("truesense", Some("gmine")).unwrap(), "gmine");
    assert_eq!(store.copy("truesense", Some("gmine")).unwrap(), "gmine-2");
    assert_eq!(store.copy("mild", None).unwrap(), "mild-copy");
    assert_eq!(store.names().unwrap()[5..], ["gmine", "gmine-2", "gmine2", "mild-copy"]);
    store.delete("gmine2").unwrap();
    store.delete("gmine").unwrap();

    assert_eq!(
        store.save("truesense", &mine).unwrap_err().to_string(),
        "'truesense' is built in and read-only; copy it (gpu undervolt copy truesense) and edit the copy"
    );
    assert_eq!(store.delete("cool").unwrap_err().to_string(), "'cool' is built in and cannot be deleted");
    assert_eq!(store.delete("nosuch").unwrap_err().to_string(), "no GPU undervolt profile 'nosuch'");
    assert_eq!(
        store.load("nosuch").unwrap_err().to_string(),
        "no GPU undervolt profile 'nosuch' (truesense gpu undervolt list)"
    );
    assert!(store.save("x", &Profile { offset: 900, ..mine.clone() }).is_err());
    assert!(store.save("x", &Profile { cap: 100, ..mine.clone() }).is_err());
    assert!(!dir.join("x.json").exists());
    for bad in ["", "../x", "a/b", "a b", &"n".repeat(41)] {
        assert!(matches!(store.save(bad, &mine), Err(ProfileError::BadName)), "{bad:?}");
    }

    std::fs::write(dir.join("junk.json"), "{ nope").unwrap();
    std::fs::write(dir.join("high.json"), r#"{"offset": 999}"#).unwrap();
    assert_eq!(store.load("junk").unwrap_err().to_string(), "GPU undervolt profile 'junk' is not valid JSON");
    assert!(store.load("high").unwrap_err().to_string().contains("offset: -300..250 MHz"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn json_names_match_python() {
    let v = serde_json::to_value(builtin("mild").unwrap()).unwrap();
    assert_eq!(v, json!({"description": "Very safe: small shift, high cap", "offset": 60, "cap": 2500}));
    assert_eq!(
        serde_json::to_value(State { offset_mhz: -5, range_mhz: [-1000, 1000] }).unwrap(),
        json!({"offset_mhz": -5, "range_mhz": [-1000, 1000]})
    );
    assert_eq!(
        serde_json::to_value(Applied { offset_mhz: 60, cap_mhz: 0 }).unwrap(),
        json!({"offset_mhz": 60, "cap_mhz": 0})
    );
    let env = |k: &str| (k == "HOME").then(|| "/home/x".to_owned());
    assert_eq!(default_dir_from(&env), Some(PathBuf::from("/home/x/.config/truesense/gpu-undervolt")));
    assert_eq!(default_dir_from(&|_| None), None);
}

// ---- the helper protocol ---------------------------------------------------------------------

fn located() -> Locator {
    Arc::new(|| Ok(PathBuf::from("/usr/lib/truesense/truesense-nvml")))
}

fn helper_with(
    answer: impl Fn(&[String]) -> io::Result<Vec<u8>> + Send + Sync + 'static,
) -> (HelperNvml, Arc<FakeRunner>) {
    let runner = Arc::new(FakeRunner::new(move |_, args| answer(args)));
    (HelperNvml::new(runner.clone(), located()), runner)
}

#[test]
fn the_helper_is_run_with_exactly_the_documented_arguments() {
    let (nv, runner) = helper_with(|args| {
        Ok(match args[0].as_str() {
            "offset" => b"ok -25\n".to_vec(),
            "range" => b"ok -300 300\n".to_vec(),
            _ => b"ok\n".to_vec(),
        })
    });
    assert_eq!(nv.offset().unwrap(), -25);
    assert_eq!(nv.offset_range().unwrap(), (-300, 300));
    nv.set_offset(-120).unwrap();
    nv.lock_clocks(210, 2400).unwrap();
    nv.reset_clocks().unwrap();
    assert_eq!(
        runner.lines(),
        [
            "/usr/lib/truesense/truesense-nvml offset",
            "/usr/lib/truesense/truesense-nvml range",
            "/usr/lib/truesense/truesense-nvml set-offset -120",
            "/usr/lib/truesense/truesense-nvml lock 210 2400",
            "/usr/lib/truesense/truesense-nvml reset-clocks",
        ]
    );
}

#[test]
fn reply_parsing_table() {
    assert_eq!(parse_reply("ok\n").unwrap(), Vec::<i64>::new());
    assert_eq!(parse_reply("ok 5").unwrap(), [5]);
    assert_eq!(parse_reply("  ok -300   300 \r\n").unwrap(), [-300, 300]);

    let words = |s: &str| parse_reply(s).unwrap_err().to_string();
    assert_eq!(words("err code 4\n"), "needs root (the TrueSense service)");
    assert_eq!(words("err code 3"), "not supported by this GPU or driver");
    assert_eq!(words("err code 12345"), "NVML error 12345");
    assert_eq!(words("err library"), "NVIDIA driver library (libnvidia-ml) not found");
    assert_eq!(words("err init"), "could not start NVML (is the NVIDIA driver loaded?)");
    assert_eq!(words("err nodevice"), "no NVIDIA GPU found");
    assert_eq!(words("err usage"), "invalid argument");
    assert_eq!(words(""), "the GPU helper gave no answer");
    assert_eq!(words("  \n "), "the GPU helper gave no answer");

    // Hostile or damaged answers: an error each, never a panic.
    let hostile = [
        "ok x",
        "ok 1 two",
        "ok 1.5",
        "ok 99999999999999999999999",
        "err",
        "err code",
        "err code x",
        "err code 1 2",
        "err weird",
        "err library extra",
        "okay 5",
        "OK",
        "5",
        "{\"ok\": 1}",
        "ok\u{0}5",
        "\u{ff}\u{fe}",
        "err code 99999999999",
        "ok --5",
        "ok +",
    ];
    for text in hostile {
        assert!(parse_reply(text).is_err(), "{text:?} was accepted");
    }
    let long = format!("garbage {}", "x".repeat(5000));
    assert!(words(&long).len() < 300, "an answer is quoted only in part");
}

#[test]
fn numbers_of_the_wrong_count_or_size_are_refused() {
    for (answer, what) in [
        ("ok\n", "offset"),
        ("ok 1 2\n", "offset"),
        ("ok 99999999999\n", "offset"),
        ("ok 1\n", "range"),
        ("ok 1 2 3\n", "range"),
    ] {
        let (nv, _) = helper_with(move |_| Ok(answer.as_bytes().to_vec()));
        let result = if what == "offset" { nv.offset().map(drop) } else { nv.offset_range().map(drop) };
        assert!(matches!(result, Err(NvmlError::BadReply(_))), "{answer:?} for {what}: {result:?}");
    }
}

#[test]
fn helper_failures_are_errors_in_words() {
    let (nv, _) = helper_with(|_| Err(io::Error::from(io::ErrorKind::TimedOut)));
    let err = nv.offset().unwrap_err();
    assert!(matches!(err, NvmlError::Timeout));
    assert_eq!(err.to_string(), "the NVIDIA driver did not answer within 10s");

    let (nv, _) = helper_with(|_| Err(io::Error::from(io::ErrorKind::NotFound)));
    assert!(matches!(nv.offset(), Err(NvmlError::HelperMissing)));
    assert!(nv.offset().unwrap_err().to_string().contains("run the TrueSense installer again"));

    let (nv, _) = helper_with(|_| Err(io::Error::other("killed by signal 11")));
    let err = nv.set_offset(5).unwrap_err();
    assert_eq!(err.to_string(), "the GPU helper failed: killed by signal 11");
    assert!(std::error::Error::source(&err).is_some());

    let (nv, _) = helper_with(|_| Ok(Vec::new()));
    assert!(matches!(nv.reset_clocks(), Err(NvmlError::NoAnswer)));
}

#[test]
fn a_missing_helper_never_runs_anything() {
    let runner = Arc::new(FakeRunner::new(|_, _| Ok(b"ok\n".to_vec())));
    let nv = HelperNvml::new(runner.clone(), Arc::new(|| Err(NvmlError::HelperMissing)));
    assert!(matches!(nv.reset_clocks(), Err(NvmlError::HelperMissing)));
    assert!(runner.lines().is_empty());
    assert_eq!(CALL_TIMEOUT, std::time::Duration::from_secs(10));
}

#[test]
fn the_controller_works_end_to_end_over_the_helper_protocol() {
    // A fake helper that keeps state, driven only through its text protocol.
    let state = Arc::new(Mutex::new((0i32, Option::<(i64, i64)>::None)));
    let s = state.clone();
    let (nv, runner) = helper_with(move |args| {
        let mut st = s.lock().unwrap();
        let text = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
            ["offset"] => format!("ok {}\n", st.0),
            ["range"] => "ok -1000 1000\n".to_owned(),
            ["set-offset", n] => {
                st.0 = n.parse().unwrap();
                "ok\n".to_owned()
            }
            ["lock", lo, hi] => {
                st.1 = Some((lo.parse().unwrap(), hi.parse().unwrap()));
                "ok\n".to_owned()
            }
            ["reset-clocks"] => {
                st.1 = None;
                "ok\n".to_owned()
            }
            _ => "err usage\n".to_owned(),
        };
        Ok(text.into_bytes())
    });
    let gpu = GpuUndervolt::new(Arc::new(nv));
    assert_eq!(gpu.apply(120, 2400).unwrap(), Applied { offset_mhz: 120, cap_mhz: 2400 });
    assert_eq!(*state.lock().unwrap(), (120, Some((210, 2400))));
    assert_eq!(gpu.reset().unwrap(), Applied { offset_mhz: 0, cap_mhz: 0 });
    assert_eq!(*state.lock().unwrap(), (0, None));
    assert!(runner.lines().iter().all(|l| l.starts_with("/usr/lib/truesense/truesense-nvml ")));
}

#[test]
fn helper_lookup() {
    let exe = PathBuf::from("/opt/ts/bin/truesense");
    let list = helper_candidates(Some("/custom/nvml"), Some(&exe));
    assert_eq!(
        list,
        [
            PathBuf::from("/custom/nvml"),
            PathBuf::from("/opt/ts/bin/../lib/truesense/truesense-nvml"),
            PathBuf::from("/usr/local/lib/truesense/truesense-nvml"),
            PathBuf::from("/usr/lib/truesense/truesense-nvml"),
        ]
    );
    assert_eq!(
        helper_candidates(Some(""), None),
        [PathBuf::from("/usr/local/lib/truesense/truesense-nvml"), PathBuf::from(HELPER_PATH)]
    );

    let dir = temp_dir("lookup");
    let plain = dir.join("plain");
    let exec = dir.join("exec");
    std::fs::write(&plain, "x").unwrap();
    std::fs::write(&exec, "#!/bin/sh\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::set_permissions(&exec, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    assert!(
        !is_executable_file(&plain)
            && is_executable_file(&exec)
            && !is_executable_file(&dir)
            && !is_executable_file(&dir.join("none"))
    );
    let found = find_helper(&[dir.join("none"), plain.clone(), exec.clone()], &is_executable_file).unwrap();
    assert_eq!(found, exec);
    assert!(matches!(
        find_helper(&[plain, dir.join("none")], &is_executable_file),
        Err(NvmlError::HelperMissing)
    ));
    assert!(matches!(find_helper(&[], &is_executable_file), Err(NvmlError::HelperMissing)));
    std::fs::remove_dir_all(&dir).unwrap();
}
