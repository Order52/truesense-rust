#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::json;

use super::*;

/// The voltage mailbox: a write of a request is remembered, the next read returns the answer.
/// It can refuse writes (a BIOS lock) or accept them without effect.
#[derive(Default)]
struct FakeMsr {
    state: Mutex<MailboxState>,
}

#[derive(Default)]
struct MailboxState {
    planes: BTreeMap<u64, u64>, // plane -> raw 11-bit offset field
    answer: u64,
    writes: Vec<u64>,
    refuse: bool,
    ignore: bool,
    read_error: bool,
}

impl FakeMsr {
    fn with(f: impl FnOnce(&mut MailboxState)) -> Arc<Self> {
        let fake = Arc::new(Self::default());
        f(&mut fake.state.lock().unwrap());
        fake
    }

    fn writes(&self) -> Vec<u64> {
        self.state.lock().unwrap().writes.clone()
    }

    fn set_flags(&self, refuse: bool, ignore: bool) {
        let mut s = self.state.lock().unwrap();
        s.refuse = refuse;
        s.ignore = ignore;
    }
}

impl Msr for FakeMsr {
    fn read(&self, register: u32) -> io::Result<u64> {
        assert_eq!(register, MAILBOX_MSR);
        let s = self.state.lock().unwrap();
        if s.read_error {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        Ok(s.answer)
    }

    fn write(&self, register: u32, value: u64) -> io::Result<()> {
        assert_eq!(register, MAILBOX_MSR, "only the mailbox is ever written");
        let mut s = self.state.lock().unwrap();
        if s.refuse {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        s.writes.push(value);
        let (plane, op) = ((value >> 40) & 0xFF, (value >> 32) & 0xFF);
        match op {
            0x11 if !s.ignore => {
                s.planes.insert(plane, (value >> 21) & 0x7FF);
            }
            0x11 => {}
            0x10 => s.answer = s.planes.get(&plane).copied().unwrap_or(0) << 21,
            other => panic!("bad opcode {other:#x}"),
        }
        Ok(())
    }
}

fn controller(msr: &Arc<FakeMsr>) -> Undervolt {
    Undervolt::new(msr.clone())
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ts-domain-uv-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// Values computed with the Python _uv_cmd.
#[test]
fn command_encoding_matches_python() {
    let table: [(Plane, bool, i32, u64); 10] = [
        (Plane::Core, false, 0, 0x8000_0010_0000_0000),
        (Plane::Cache, false, 0, 0x8000_0210_0000_0000),
        (Plane::Core, true, -50, 0x8000_0011_f9a0_0000),
        (Plane::Cache, true, -40, 0x8000_0211_fae0_0000),
        (Plane::Core, true, 0, 0x8000_0011_0000_0000),
        (Plane::Core, true, -120, 0x8000_0011_f0a0_0000),
        (Plane::Cache, true, -1, 0x8000_0211_ffe0_0000),
        (Plane::Core, true, -30, 0x8000_0011_fc20_0000),
        (Plane::Core, true, -5, 0x8000_0011_ff60_0000),
        (Plane::Core, true, -10, 0x8000_0011_fec0_0000),
    ];
    for (plane, write, mv, want) in table {
        assert_eq!(command(plane, write, mv), want, "command({plane:?}, {write}, {mv})");
    }
}

#[test]
fn decode_table() {
    for (raw, want) in [(0u64, 0), (0x7cd, -50), (0x400, -1000), (0x7ff, -1), (0x3ff, 999), (1, 1)] {
        assert_eq!(decode(raw << 21), want, "raw {raw:#x}");
    }
    // Bits outside the 11-bit field never leak into the answer.
    assert_eq!(decode(!(0x7FF_u64 << 21)), 0);
}

#[test]
fn every_valid_offset_reads_back_as_itself_within_the_tolerance() {
    for mv in (-MAX_MV..=0).rev() {
        let raw = (command(Plane::Core, true, mv) >> 21) & 0x7FF;
        let got = decode(raw << 21);
        assert!((got - mv).abs() <= VERIFY_TOLERANCE_MV, "{mv} mV reads back as {got}");
    }
}

// Python: test_undervolt_needs_confirmation_and_boot_has_a_crash_guard (the part that is this module).
#[test]
fn set_needs_confirmation_and_the_zone() {
    let msr = FakeMsr::with(|_| {});
    let uv = controller(&msr);
    assert!(matches!(uv.set(Plane::Core, -50, false), Err(UndervoltError::NeedsConfirmation)));
    assert!(uv.set(Plane::Core, -50, false).unwrap_err().to_string().contains("explicit --confirm"));
    for mv in [-300, -121, 1, 50, i32::MIN, i32::MAX] {
        let err = uv.set(Plane::Core, mv, true).unwrap_err();
        assert!(matches!(err, UndervoltError::OutOfRange { .. }), "set({mv}): {err}");
        assert_eq!(err.to_string(), "offset must be between 0 and -120 mV (start with -10)");
    }
    assert!(msr.writes().is_empty(), "a refused request still wrote to the MSR: {:#x?}", msr.writes());

    assert_eq!(uv.apply(-50, -40, true).unwrap(), Offsets { core_mv: -50, cache_mv: -40 });
    assert_eq!(uv.read().unwrap(), Offsets { core_mv: -50, cache_mv: -40 });
    assert_eq!(uv.reset().unwrap(), Offsets::default());
    assert_eq!(uv.read().unwrap(), Offsets::default());
}

#[test]
fn set_edges_are_inside_the_zone() {
    for mv in [0, -1, -60, -120] {
        let msr = FakeMsr::with(|_| {});
        let uv = controller(&msr);
        let got = uv.set(Plane::Cache, mv, true).unwrap();
        assert!((got - mv).abs() <= VERIFY_TOLERANCE_MV);
    }
}

#[test]
fn apply_checks_both_offsets_before_writing_either() {
    let msr = FakeMsr::with(|_| {});
    let uv = controller(&msr);
    for (core, cache) in [(-50, -121), (-121, -50), (1, 0), (0, 5), (-50, i32::MIN)] {
        assert!(
            matches!(uv.apply(core, cache, true), Err(UndervoltError::OutOfRange { .. })),
            "({core}, {cache})"
        );
    }
    assert!(matches!(uv.apply(-10, -10, false), Err(UndervoltError::NeedsConfirmation)));
    assert!(msr.writes().is_empty(), "nothing may be written when the pair is not acceptable as a whole");
}

#[test]
fn get_is_a_harmless_read_request() {
    let msr = FakeMsr::with(|s| {
        s.planes.insert(0, 0x7cd);
    });
    let uv = controller(&msr);
    assert_eq!(uv.get(Plane::Core).unwrap(), -50);
    assert_eq!(msr.writes(), [0x8000_0010_0000_0000], "only a READ request (opcode 0x10) is sent");
}

#[test]
fn a_bios_lock_that_ignores_the_write_is_detected_and_rolled_back() {
    let msr = FakeMsr::with(|s| s.ignore = true);
    let err = controller(&msr).set(Plane::Core, -50, true).unwrap_err();
    // The CPU still reads 0, so the "rollback" to 0 verifies too.
    assert!(
        matches!(err, UndervoltError::Readback { got_mv: 0, wanted_mv: -50, rolled_back: true }),
        "{err}"
    );
    assert_eq!(err.to_string(), "readback 0 mV != -50 mV (BIOS lock?); rolled back to stock");
}

#[test]
fn a_readback_that_is_wrong_is_put_back_to_stock() {
    // A CPU that stores half of what it is told: the mismatch must trigger a write of 0.
    struct Halving(Mutex<(u64, u64)>);
    impl Msr for Halving {
        fn read(&self, _: u32) -> io::Result<u64> {
            Ok(self.0.lock().unwrap().1)
        }
        fn write(&self, _: u32, value: u64) -> io::Result<()> {
            let mut s = self.0.lock().unwrap();
            match (value >> 32) & 0xFF {
                0x11 => {
                    let raw = (value >> 21) & 0x7FF;
                    // Half of a negative 11-bit number, still two's complement.
                    let signed = if raw & 0x400 != 0 { raw as i64 - 0x800 } else { raw as i64 };
                    s.0 = ((signed / 2) as u64) & 0x7FF;
                }
                _ => s.1 = s.0 << 21,
            }
            Ok(())
        }
    }
    let uv = Undervolt::new(Arc::new(Halving(Mutex::new((0, 0)))));
    let err = uv.set(Plane::Core, -100, true).unwrap_err();
    assert!(matches!(err, UndervoltError::Readback { wanted_mv: -100, rolled_back: true, .. }), "{err}");
    assert_eq!(uv.get(Plane::Core).unwrap(), 0, "the plane is back at stock");
}

#[test]
fn msr_not_accessible() {
    let msr = FakeMsr::with(|s| s.refuse = true);
    let uv = controller(&msr);
    let err = uv.get(Plane::Core).unwrap_err();
    assert!(matches!(err, UndervoltError::NotAccessible(_)));
    assert!(err.to_string().starts_with("MSR 0x150 not accessible: "));
    assert!(err.to_string().ends_with("(sudo modprobe msr; run as root)"));
    assert!(uv.read().is_err());

    let err = uv.set(Plane::Core, -10, true).unwrap_err();
    assert!(matches!(err, UndervoltError::WriteRefused(_)));
    assert!(err.to_string().contains("BIOS may lock undervolting"));

    let msr = FakeMsr::with(|s| s.read_error = true);
    assert!(matches!(controller(&msr).get(Plane::Core), Err(UndervoltError::NotAccessible(_))));
}

#[test]
fn apply_rolls_back_a_half_applied_pair() {
    let msr = FakeMsr::with(|_| {});
    let uv = controller(&msr);
    uv.apply(-30, -30, true).unwrap();
    // Now the cache plane stops taking writes while the core plane would.
    struct CacheLocked(Arc<FakeMsr>);
    impl Msr for CacheLocked {
        fn read(&self, r: u32) -> io::Result<u64> {
            self.0.read(r)
        }
        fn write(&self, r: u32, v: u64) -> io::Result<()> {
            let (plane, op) = ((v >> 40) & 0xFF, (v >> 32) & 0xFF);
            if plane == 2 && op == 0x11 && (v >> 21) & 0x7FF != 0 {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            self.0.write(r, v)
        }
    }
    let uv = Undervolt::new(Arc::new(CacheLocked(msr.clone())));
    let err = uv.apply(-70, -50, true).unwrap_err();
    assert!(matches!(err, UndervoltError::WriteRefused(_)), "{err}");
    assert_eq!(uv.read().unwrap(), Offsets::default(), "the core offset that did take was reset");
}

#[test]
fn reset_reports_what_did_not_take() {
    let msr = FakeMsr::with(|_| {});
    let uv = controller(&msr);
    uv.apply(-50, -40, true).unwrap();
    msr.set_flags(false, true); // the writes are accepted and ignored
    let err = uv.reset().unwrap_err();
    match err {
        UndervoltError::ResetIncomplete { offsets, note } => {
            assert_eq!(offsets, Offsets { core_mv: -50, cache_mv: -40 });
            assert!(note.is_none());
        }
        other => panic!("{other}"),
    }
    assert_eq!(uv.reset().unwrap_err().to_string(), "reset did not take: core -50 mV, cache -40 mV");
}

#[test]
fn a_refused_write_does_not_stop_the_other_plane_from_being_reset() {
    let msr = FakeMsr::with(|_| {});
    let uv = controller(&msr);
    uv.apply(-50, -40, true).unwrap();
    struct CoreRefuses(Arc<FakeMsr>);
    impl Msr for CoreRefuses {
        fn read(&self, r: u32) -> io::Result<u64> {
            self.0.read(r)
        }
        fn write(&self, r: u32, v: u64) -> io::Result<()> {
            if (v >> 40) & 0xFF == 0 && (v >> 32) & 0xFF == 0x11 {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            self.0.write(r, v)
        }
    }
    let err = Undervolt::new(Arc::new(CoreRefuses(msr.clone()))).reset().unwrap_err();
    match err {
        UndervoltError::ResetIncomplete { offsets, note } => {
            assert_eq!(offsets, Offsets { core_mv: -50, cache_mv: 0 }, "the cache plane did reset");
            assert!(note.unwrap().contains("permission denied"));
        }
        other => panic!("{other}"),
    }
}

#[test]
fn dev_msr_reads_and_writes_at_the_register_offset() {
    let dir = temp_dir("devmsr");
    let path = dir.join("msr");
    let mut image = vec![0u8; 0x200];
    image[0x150..0x158].copy_from_slice(&0x1122_3344_5566_7788u64.to_le_bytes());
    std::fs::write(&path, &image).unwrap();
    let dev = DevMsr::new(&path);
    assert_eq!(dev.read(0x150).unwrap(), 0x1122_3344_5566_7788);
    dev.write(MAILBOX_MSR, 0x8000_0011_f9a0_0000).unwrap();
    assert_eq!(dev.read(0x150).unwrap(), 0x8000_0011_f9a0_0000);
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[0x150..0x158], &0x8000_0011_f9a0_0000u64.to_le_bytes());
    assert_eq!(bytes[..0x150], image[..0x150], "nothing else changed");
    // A register past the end of the device is an error, not a short read.
    assert!(dev.read(0x1fc).is_err());
    assert!(DevMsr::new(dir.join("missing")).read(0x150).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn dev_msr_refuses_to_write_any_other_register() {
    let dir = temp_dir("devmsr-guard");
    let path = dir.join("msr");
    std::fs::write(&path, vec![0u8; 0x800]).unwrap();
    let dev = DevMsr::new(&path);
    for register in [0x610u32, 0x1a0, 0x1b0, 0x0, 0x14f, 0x151] {
        let err = dev.write(register, u64::MAX).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{register:#x}");
    }
    assert!(std::fs::read(&path).unwrap().iter().all(|&b| b == 0), "no byte of the device changed");
    assert_eq!(DevMsr::cpu0().path, Path::new(DEFAULT_MSR_PATH));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_msr_is_also_a_read_only_view_for_the_power_limit_lock_bit() {
    use crate::power::MsrReader;
    let msr = FakeMsr::default();
    msr.state.lock().unwrap().answer = 7;
    assert_eq!(MsrReader::read_msr(&msr, MAILBOX_MSR).unwrap(), 7);
}

// Python: test_undervolt_risk_zones.
#[test]
fn risk_zones() {
    let table = [
        (0, 0, Level::Safe, "Stock voltage."),
        (-50, -40, Level::Safe, "Inside the safe range."),
        (-60, -50, Level::Safe, "Inside the safe range."),
        (-61, -40, Level::Caution, ""),
        (-40, -51, Level::Caution, ""),
        (-75, -40, Level::Caution, ""),
        (-90, -70, Level::Caution, ""),
        (-91, -40, Level::Danger, ""),
        (-40, -71, Level::Danger, ""),
        (-100, -40, Level::Danger, ""),
        (i32::MIN, 0, Level::Danger, ""),
        (7, 0, Level::Safe, "Inside the safe range."),
    ];
    for (core, cache, level, message) in table {
        let rating = risk(core, cache);
        assert_eq!(rating.level, level, "risk({core}, {cache})");
        if !message.is_empty() {
            assert_eq!(rating.message, message);
        }
    }
    assert_eq!(
        risk(-75, -40).message,
        "Past the safety line (core 60 mV / cache 50 mV). It may be unstable: test under load before trusting it."
    );
    assert_eq!(
        risk(-100, -40).message,
        "Beyond the danger line (core 90 mV / cache 70 mV). Freezes and reboots are likely; unsaved work can be lost."
    );
}

#[test]
fn builtins_are_the_python_ones_and_all_valid() {
    let want: [(&str, i32, i32); 5] = [
        ("native", 0, 0),
        ("truesense", -50, -40),
        ("mild", -30, -30),
        ("balanced", -70, -50),
        ("aggressive", -100, -80),
    ];
    assert_eq!(builtin_names(), want.map(|w| w.0));
    for (name, core, cache) in want {
        let p = builtin(name).unwrap();
        assert_eq!((p.core, p.cache), (core, cache), "{name}");
        assert!(!p.description.is_empty());
        p.check().unwrap();
        assert!(is_builtin(name));
        let level = match name {
            "balanced" => Level::Caution,
            "aggressive" => Level::Danger, // offered, but the decision layer asks for a typed confirmation
            _ => Level::Safe,
        };
        assert_eq!(risk(p.core, p.cache).level, level, "{name}");
    }
    assert!(builtin("nosuch").is_none() && !is_builtin("stock"));
}

#[test]
fn profile_validation_table() {
    let invalid_shape = "an undervolt profile has 'core' and 'cache' offsets in mV";
    let ok = |v: serde_json::Value, want: (&str, i32, i32)| {
        let p = Profile::from_value(&v).unwrap_or_else(|e| panic!("{v}: {e}"));
        assert_eq!((p.description.as_str(), p.core, p.cache), want, "{v}");
    };
    ok(json!({"core": -50, "cache": -40, "description": "d"}), ("d", -50, -40));
    ok(json!({"core": 0, "cache": 0}), ("", 0, 0));
    ok(json!({"core": -120, "cache": -120}), ("", -120, -120));
    ok(json!({"core": -50.9, "cache": -0.5}), ("", -50, 0));
    ok(json!({"core": -1, "cache": -1, "x": 1}), ("", -1, -1));
    ok(json!({"core": -1, "cache": -1, "description": 5}), ("", -1, -1));

    let bad: [(serde_json::Value, &str); 14] = [
        (json!({"core": -121, "cache": -40}), "core: 0 to -120 mV"),
        (json!({"core": -1, "cache": -121}), "cache: 0 to -120 mV"),
        (json!({"core": 1, "cache": 0}), "core: 0 to -120 mV"),
        (json!({"core": 0, "cache": 0.5}), "cache: 0 to -120 mV"),
        (json!({"cache": -10}), "core: 0 to -120 mV"),
        (json!({"core": -10}), "cache: 0 to -120 mV"),
        (json!({"core": "-50", "cache": 0}), "core: 0 to -120 mV"),
        (json!({"core": false, "cache": 0}), "core: 0 to -120 mV"),
        (json!({"core": null, "cache": 0}), "core: 0 to -120 mV"),
        (json!({"core": 1e300, "cache": 0}), "core: 0 to -120 mV"),
        (json!({"core": -1e300, "cache": 0}), "core: 0 to -120 mV"),
        (json!([-50]), invalid_shape),
        (json!(null), invalid_shape),
        (json!("x"), invalid_shape),
    ];
    for (value, message) in bad {
        let err = Profile::from_value(&value).unwrap_err();
        assert!(matches!(err, ProfileError::Invalid(_)));
        assert!(err.to_string().contains(message), "{value}: {err}");
    }
}

#[test]
fn stock_is_the_old_name_of_native() {
    let dir = temp_dir("stock");
    let store = Store::new(&dir);
    let p = store.load("stock").unwrap();
    assert_eq!((p.core, p.cache), (0, 0));
    std::fs::remove_dir_all(&dir).unwrap();
}

// Python: undervolt.save("truesense", ...) raises HwError.
#[test]
fn store_round_trip_copy_and_delete() {
    let dir = temp_dir("store");
    let store = Store::new(dir.join("not-yet"));
    assert_eq!(store.names().unwrap(), builtin_names(), "a missing directory lists only the built-ins");

    let mine = Profile { description: "mine".into(), core: -45, cache: -35 };
    assert_eq!(store.save("uvmine", &mine).unwrap(), mine);
    assert_eq!(store.load("uvmine").unwrap(), mine);
    let text = std::fs::read_to_string(store.dir().join("uvmine.json")).unwrap();
    // The Python edition's json.dump(indent=1): key order and one space of indent.
    assert_eq!(text, "{\n \"description\": \"mine\",\n \"core\": -45,\n \"cache\": -35\n}");
    assert_eq!(store.names().unwrap(), ["native", "truesense", "mild", "balanced", "aggressive", "uvmine"]);

    assert_eq!(store.copy("truesense", Some("uvcopy")).unwrap(), "uvcopy");
    assert_eq!(store.load("uvcopy").unwrap(), builtin("truesense").unwrap());
    assert_eq!(store.copy("uvmine", None).unwrap(), "uvmine-copy");
    assert_eq!(store.copy("uvmine", None).unwrap(), "uvmine-copy-2");
    assert_eq!(store.copy("uvmine", Some("")).unwrap(), "uvmine-copy-3");
    assert_eq!(store.copy("uvmine", Some("uvmine")).unwrap(), "uvmine-2");
    assert_eq!(store.copy("stock", Some("fromstock")).unwrap(), "fromstock");

    store.delete("uvmine").unwrap();
    assert!(matches!(store.load("uvmine"), Err(ProfileError::NotFound { .. })));
    assert!(!store.names().unwrap().contains(&"uvmine".to_owned()));
    assert_eq!(store.delete("uvmine").unwrap_err().to_string(), "no undervolt profile 'uvmine'");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn store_refuses_builtins_bad_names_and_bad_values() {
    let dir = temp_dir("refuse");
    let store = Store::new(&dir);
    let p = Profile { description: String::new(), core: -10, cache: -10 };
    let err = store.save("truesense", &p).unwrap_err();
    assert_eq!(
        err.to_string(),
        "'truesense' is built in and read-only; copy it (cpu undervolt copy truesense) and edit the copy"
    );
    assert_eq!(store.delete("native").unwrap_err().to_string(), "'native' is built in and cannot be deleted");
    for bad in ["", "a/b", "../x", "x y", "a\n", "é", &"n".repeat(41), "a\0b", "/etc/passwd"] {
        assert!(matches!(store.save(bad, &p), Err(ProfileError::BadName)), "{bad:?}");
        assert!(matches!(store.load(bad), Err(ProfileError::BadName)), "{bad:?}");
        assert!(matches!(store.delete(bad), Err(ProfileError::BadName)), "{bad:?}");
    }
    assert_eq!(ProfileError::BadName.to_string(), "profile names use letters, digits, . _ - (max 40)");
    store.save(&"n".repeat(40), &p).unwrap();
    store.save("a.b_c-D9", &p).unwrap();
    for bad in [Profile { core: -121, ..p.clone() }, Profile { cache: 1, ..p.clone() }] {
        assert!(matches!(store.save("x", &bad), Err(ProfileError::Invalid(_))));
    }
    assert!(!dir.join("x.json").exists(), "an invalid profile is never written");
    // The hard zone is the same one the controller enforces.
    assert!(Profile { core: -MAX_MV, cache: -MAX_MV, ..p }.check().is_ok());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn store_names_and_broken_files() {
    let dir = temp_dir("broken");
    let store = Store::new(&dir);
    std::fs::write(dir.join("zeta.json"), r#"{"core": -10, "cache": -10}"#).unwrap();
    std::fs::write(dir.join("alpha.json"), r#"{"core": -10, "cache": -10}"#).unwrap();
    std::fs::write(dir.join("truesense.json"), r#"{"core": -99, "cache": -99}"#).unwrap();
    std::fs::write(dir.join("notes.txt"), "x").unwrap();
    std::fs::write(dir.join("bad name.json"), "{}").unwrap();
    std::fs::write(dir.join("junk.json"), "{ not json").unwrap();
    std::fs::write(dir.join("big.json"), r#"{"core": -500, "cache": 0}"#).unwrap();
    std::fs::write(dir.join("array.json"), "[1, 2]").unwrap();
    std::fs::write(dir.join("empty.json"), "").unwrap();
    std::fs::create_dir(dir.join("subdir.json")).unwrap();

    let names = store.names().unwrap();
    assert_eq!(&names[..5], builtin_names());
    assert!(!names.contains(&"bad name".to_owned()) && !names.iter().any(|n| n == "notes"));
    assert_eq!(names.iter().filter(|n| *n == "truesense").count(), 1, "a file never shadows a built-in");
    assert_eq!(store.load("truesense").unwrap().core, -50, "the built-in wins over a file of the same name");

    assert_eq!(store.load("junk").unwrap_err().to_string(), "undervolt profile 'junk' is not valid JSON");
    assert_eq!(store.load("empty").unwrap_err().to_string(), "undervolt profile 'empty' is not valid JSON");
    assert!(store.load("big").unwrap_err().to_string().contains("core: 0 to -120 mV"));
    assert!(store.load("array").unwrap_err().to_string().contains("has 'core' and 'cache'"));
    assert!(matches!(store.load("subdir"), Err(ProfileError::Io(_))));
    assert_eq!(
        store.load("nosuch").unwrap_err().to_string(),
        "no undervolt profile 'nosuch' (truesense cpu undervolt list)"
    );
    assert_eq!(store.load("alpha").unwrap().core, -10);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_huge_profile_file_is_refused_not_read() {
    let dir = temp_dir("huge");
    let store = Store::new(&dir);
    let mut text = String::from(r#"{"core": -10, "cache": -10, "description": ""#);
    text.push_str(&"x".repeat(2 << 20));
    text.push_str("\"}");
    std::fs::write(dir.join("huge.json"), text).unwrap();
    assert!(matches!(store.load("huge"), Err(ProfileError::Io(_))));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn json_names_match_python() {
    let v = serde_json::to_value(builtin("mild").unwrap()).unwrap();
    assert_eq!(v, json!({"description": "Very safe: small offset", "core": -30, "cache": -30}));
    let v = serde_json::to_value(Offsets { core_mv: -50, cache_mv: -40 }).unwrap();
    assert_eq!(v, json!({"core_mv": -50, "cache_mv": -40}));
}

#[test]
fn default_dir_follows_home() {
    let env = |k: &str| (k == "HOME").then(|| "/home/x".to_owned());
    assert_eq!(default_dir_from(&env), Some(PathBuf::from("/home/x/.config/truesense/undervolt")));
    assert_eq!(default_dir_from(&|_| None), None);
}
