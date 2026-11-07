#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io;
use std::sync::Mutex;

use serde_json::Value;
use ts_platform::firmware::{Device, FirmwareError, READ_ONLY_COMMANDS, Simulator};

use super::testutil::TempDir;
use super::*;

/// A DMI tree with the given identity.
fn dmi(product: &str, vendor: &str, bios: &str) -> TempDir {
    let dir = TempDir::new("dmi");
    for (name, value) in [
        ("product_name", product),
        ("sys_vendor", vendor),
        ("bios_version", bios),
        ("bios_date", "06/09/2026"),
    ] {
        dir.write(name, &format!("{value}\n"));
    }
    dir
}

fn identify_as(product: &str, vendor: &str, bios: &str) -> Model {
    identify(dmi(product, vendor, bios).path())
}

/// A `/sys` tree with the given WMI device directory names, and the module directory when asked.
fn sysroot(guids: &[&str], module: bool) -> TempDir {
    let dir = TempDir::new("sys");
    dir.mkdir("bus/wmi/devices");
    for g in guids {
        dir.mkdir(&format!("bus/wmi/devices/{g}"));
    }
    if module {
        dir.mkdir("module/truesense_wmi");
    }
    dir
}

const GAMING_UPPER: &str = "7A4DDFE7-5B5D-40B4-8595-4408E0CC7F56";
const OTHER_GUID: &str = "05901221-D566-11D1-B2F0-00A0C9062910-1";

struct Rig {
    // Kept alive so the directories exist for the duration of the test.
    _dmi: TempDir,
    _sys: TempDir,
    _dev: TempDir,
    options: ProbeOptions,
}

/// Probe options over temp trees; `bound` creates the device node.
fn rig(product: &str, guids: &[&str], bound: bool) -> Rig {
    let dmi = dmi(product, "Acer", "V1.18");
    let sys = sysroot(guids, bound);
    let dev = TempDir::new("dev");
    if bound {
        dev.write("truesense", "");
    }
    let options = ProbeOptions {
        dmi_root: dmi.path().to_path_buf(),
        sys_root: sys.path().to_path_buf(),
        device_path: dev.path().join("truesense"),
        kernel: "7.3.0-test".to_owned(),
        version: "0.5.0".to_owned(),
    };
    Rig { _dmi: dmi, _sys: sys, _dev: dev, options }
}

/// Records every command and answers with `reply` (or fails with an I/O error when `None`).
struct Spy {
    sent: Mutex<Vec<String>>,
    reply: Option<String>,
}

impl Spy {
    fn new(reply: Option<&str>) -> Self {
        Self { sent: Mutex::default(), reply: reply.map(str::to_owned) }
    }

    fn sent(&self) -> Vec<String> {
        self.sent.lock().unwrap().clone()
    }
}

impl Device for Spy {
    fn run(&self, command: &str) -> Result<String, FirmwareError> {
        self.sent.lock().unwrap().push(command.to_owned());
        self.reply.clone().ok_or_else(|| FirmwareError::Io {
            operation: command.to_owned(),
            source: io::Error::other("boom"),
        })
    }
}

// ---- tiers ------------------------------------------------------------------------------------

#[test]
fn the_verified_model_is_supported_and_a_new_bios_is_only_a_note() {
    let m = identify_as("Predator PH16-71", "Acer", "V1.18");
    assert_eq!((m.tier, m.bios_verified), (Tier::Supported, Some(true)));
    assert_eq!(m.family, "Predator Helios 16");
    assert_eq!(m.message, "Predator PH16-71, BIOS V1.18");
    let m = identify_as("Predator PH16-71", "Acer", "V1.25");
    assert_eq!((m.tier, m.bios_verified), (Tier::Supported, Some(false)));
    assert!(m.message.contains("V1.18") && m.message.contains("tell us if it does not"), "{}", m.message);
}

#[test]
fn a_supported_model_without_a_bios_version_is_unverified_on_it_not_unsupported() {
    let m = identify_as("Predator PH16-71", "Acer", "");
    assert_eq!((m.tier, m.bios_verified, m.bios.as_str()), (Tier::Supported, Some(false), "?"));
}

#[test]
fn similar_acer_gaming_laptops_are_unverified_not_supported() {
    for p in [
        "Predator PH16-72",
        "Predator PH18-71",
        "Predator PHN16-71",
        "Predator PT16-51",
        "Nitro AN16-41",
        "Predator PHN18-71",
    ] {
        let m = identify_as(p, "Acer", "V1.18");
        assert_eq!(m.tier, Tier::Unverified, "{p}");
        assert!(m.message.contains("truesense probe") && m.message.contains("new_model_report"), "{p}");
        assert_eq!((m.family.as_str(), m.bios_verified), ("similar Acer gaming laptop", None));
    }
}

#[test]
fn other_machines_do_not_apply() {
    for (p, v) in [
        ("Inspiron 15", "Dell Inc."),
        ("Aspire 5", "Acer"),
        ("", ""),
        ("Predator PH16-71", "Not Acer Ltd"),
        ("Predator PH16-71", ""),
    ] {
        assert_eq!(identify_as(p, v, "V1.18").tier, Tier::Unsupported, "{p:?} {v:?}");
    }
    let m = identify(Path::new("/nonexistent/dmi"));
    assert_eq!((m.tier, m.product.as_str(), m.bios.as_str()), (Tier::Unsupported, "unknown", "?"));
    assert_eq!(m.message, "'unknown' (unknown vendor) is not an Acer Predator: TrueSense does not apply");
}

#[test]
fn the_vendor_is_matched_by_its_acer_prefix_in_any_case() {
    assert_eq!(identify_as("Predator PH16-71", "ACER Inc.", "V1.18").tier, Tier::Supported);
    assert_eq!(identify_as("Predator PH16-71", "acer", "V1.18").tier, Tier::Supported);
    assert_eq!(identify_as("Predator PH16-71", " Acer", "V1.18").tier, Tier::Supported, "values are trimmed");
}

#[test]
fn the_product_name_pattern_table() {
    for (name, want) in [
        ("Predator PH16-71", true),
        ("predator ph18-71", true),
        ("PREDATOR PHN16-71", true),
        ("Nitro AN16-41", true),
        ("Nitro\tAN16-41", true),
        ("Predator   PH16-71", true),
        ("Predator PH16-71 extra words", true),
        ("Predator PHNX16-71", true),
        ("PredatorPH16-71", false),
        ("Predator PHNXY16-71", false),
        ("Predator 16-71", false),
        ("Predator PH1-71", false),
        ("Predator PH16-7", false),
        ("Predator PH16_71", false),
        ("Predator PH16-7x", false),
        ("Predator", false),
        ("Predator ", false),
        ("Nitro", false),
        ("Aspire A515-51", false),
        ("", false),
        // Hostile: multi-byte characters around the cut points must not panic.
        ("Predato\u{e9} PH16-71", false),
        ("Pr\u{e9}dator PH16-71", false),
        ("Nitr\u{e9} AN16-41", false),
        ("Predator \u{e9}\u{e9}16-71", false),
        ("Predator PH\u{661}\u{662}-71", false),
        ("\u{1f4a5}\u{1f4a5}\u{1f4a5}", false),
    ] {
        assert_eq!(looks_like_gaming_laptop(name), want, "{name:?}");
    }
}

#[test]
fn only_a_verified_report_can_add_a_model() {
    assert_eq!(SUPPORTED.iter().map(|k| k.product).collect::<Vec<_>>(), ["Predator PH16-71"]);
    assert!(SUPPORTED.iter().all(|k| !k.verified_bios.is_empty()));
}

#[test]
fn a_dmi_value_that_is_not_text_is_empty_not_an_error() {
    let dir = dmi("Predator PH16-71", "Acer", "V1.18");
    std::fs::write(dir.path().join("sys_vendor"), [0xff, 0xfe, 0x00]).unwrap();
    assert_eq!(identify(dir.path()).tier, Tier::Unsupported);
}

#[test]
fn the_tier_serialises_as_its_word() {
    assert_eq!(serde_json::to_string(&Tier::Unverified).unwrap(), "\"unverified\"");
    assert_eq!(Tier::Supported.as_str(), "supported");
    assert_eq!(Tier::Unsupported.as_str(), "unsupported");
}

// ---- the probe --------------------------------------------------------------------------------

#[test]
fn it_only_ever_sends_reads() {
    let r = rig("Predator PH18-71", &[GAMING_UPPER, OTHER_GUID], true);
    let spy = Spy::new(Some("ok a=1\n"));
    let p = probe(Some(&spy), &r.options);
    assert_eq!(spy.sent(), READ_ONLY.to_vec(), "exactly the list, in order");
    assert_eq!(p.writes, 0);
    assert!(
        spy.sent().iter().all(|c| c == "version"
            || c == "caps"
            || c == "sensors"
            || (c.starts_with("get_") && READ_ONLY_COMMANDS.contains(&c.split(' ').next().unwrap()))),
        "{:?}",
        spy.sent()
    );
    assert!(spy.sent().iter().all(|c| !c.starts_with("set") && !c.contains("force")));
    assert_eq!(p.answered, format!("{0}/{0}", READ_ONLY.len()));
}

#[test]
fn the_read_only_list_has_the_python_commands() {
    assert_eq!(
        READ_ONLY,
        [
            "version",
            "caps",
            "sensors",
            "get_fan",
            "get_mode",
            "get_modes",
            "get_overdrive",
            "get_boot",
            "get_kbtimeout",
            "get_usb",
            "get_battery 1",
            "get_battery 2",
            "get_gpu",
            "get_rear"
        ]
    );
}

#[test]
fn the_gaming_interface_is_detected_from_the_wmi_devices() {
    let r = rig("Predator PH18-71", &[GAMING_UPPER, OTHER_GUID], false);
    let p = probe(None, &r.options);
    assert!(p.gaming_interface_present);
    assert!(p.wmi_guids.contains(&GAMING_WMI_GUID.to_owned()));
    assert_eq!(p.wmi_guids.len(), 2, "{:?}", p.wmi_guids);
    let r = rig("Predator PH18-71", &["05901221-D566-11D1-B2F0-00A0C9062910"], false);
    assert!(!probe(None, &r.options).gaming_interface_present);
}

#[test]
fn wmi_devices_are_lower_cased_deduplicated_and_sorted_and_junk_is_ignored() {
    let r = rig(
        "Predator PH18-71",
        &[
            "7A4DDFE7-5B5D-40B4-8595-4408E0CC7F56-1",
            "7A4DDFE7-5B5D-40B4-8595-4408E0CC7F56-2",
            "05901221-D566-11D1-B2F0-00A0C9062910",
            "not-a-guid",
            "7a4ddfe7-5b5d-40b4-8595-4408e0cc7f5",
            "7a4ddfe7_5b5d_40b4_8595_4408e0cc7f56",
            "7a4ddfe7-5b5d-40b4-8595-4408e0cc7g56",
            "\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}-5b5d-40b4-8595-4408e0cc7f56",
        ],
        false,
    );
    assert_eq!(
        probe(None, &r.options).wmi_guids,
        ["05901221-d566-11d1-b2f0-00a0c9062910", "7a4ddfe7-5b5d-40b4-8595-4408e0cc7f56"]
    );
}

#[test]
fn a_missing_wmi_directory_is_an_empty_list() {
    let mut r = rig("Predator PH18-71", &[], false);
    r.options.sys_root = PathBuf::from("/nonexistent/sys");
    let p = probe(None, &r.options);
    assert!(p.wmi_guids.is_empty() && !p.gaming_interface_present && !p.module.loaded);
}

#[test]
fn without_the_module_it_explains_how_to_read_safely_and_does_not_touch_the_firmware() {
    let r = rig("Predator PH18-71", &[GAMING_UPPER], false);
    let spy = Spy::new(Some("ok\n"));
    let p = probe(Some(&spy), &r.options);
    assert!(spy.sent().is_empty(), "no device node, no commands");
    assert!(p.note.as_deref().unwrap().contains("force=1"));
    assert_eq!(p.answered, "0/0");
    assert!(p.firmware.is_empty() && !p.module.device);
}

#[test]
fn a_bound_module_without_a_device_handle_is_not_asked_either() {
    let r = rig("Predator PH18-71", &[GAMING_UPPER], true);
    let p = probe(None, &r.options);
    assert!(p.note.is_some() && p.firmware.is_empty() && p.module.device);
}

#[test]
fn a_firmware_that_does_not_answer_is_reported_not_fatal() {
    let r = rig("Predator PH18-71", &[GAMING_UPPER], true);
    let spy = Spy::new(Some("err no such command\n"));
    let p = probe(Some(&spy), &r.options);
    assert_eq!(p.answered, format!("0/{}", READ_ONLY.len()));
    assert_eq!(p.firmware.len(), READ_ONLY.len());
    let fan = p.firmware.iter().find(|a| a.command == "get_fan").unwrap();
    assert_eq!(fan.result, Err("no such command".to_owned()));
    assert!(p.note.is_none());
}

#[test]
fn a_transport_failure_and_a_garbled_reply_are_answers_too() {
    let r = rig("Predator PH18-71", &[], true);
    let p = probe(Some(&Spy::new(None)), &r.options);
    assert_eq!(p.answered, format!("0/{}", READ_ONLY.len()));
    let p = probe(Some(&Spy::new(Some("garbage\n"))), &r.options);
    assert_eq!(p.answered, format!("0/{}", READ_ONLY.len()));
    assert!(p.firmware.iter().all(|a| a.result.as_ref().unwrap_err().contains("unexpected reply")));
}

#[test]
fn a_long_error_is_cut_to_120_characters() {
    let r = rig("Predator PH18-71", &[], true);
    let long = format!("err {}", "\u{e9}".repeat(300));
    let p = probe(Some(&Spy::new(Some(&long))), &r.options);
    for a in &p.firmware {
        assert_eq!(a.result.as_ref().unwrap_err().chars().count(), 120);
    }
}

#[test]
fn some_answers_and_some_not_are_counted() {
    struct Half;
    impl Device for Half {
        fn run(&self, command: &str) -> Result<String, FirmwareError> {
            Ok(if command.starts_with("get_battery") {
                "err nope\n".to_owned()
            } else {
                "ok x=1\n".to_owned()
            })
        }
    }
    let r = rig("Predator PH18-71", &[], true);
    assert_eq!(
        probe(Some(&Half), &r.options).answered,
        format!("{}/{}", READ_ONLY.len() - 2, READ_ONLY.len())
    );
}

#[test]
fn the_probe_runs_against_the_simulator() {
    let r = rig("Predator PH16-71", &[GAMING_UPPER], true);
    let sim = Simulator::new();
    let p = probe(Some(&sim), &r.options);
    assert_eq!(p.answered, format!("{0}/{0}", READ_ONLY.len()), "{:?}", p.firmware);
    assert_eq!(p.model.tier, Tier::Supported);
    assert_eq!(p.verdict, "supported: verified on this model");
    let before = sim.snapshot();
    let _ = probe(Some(&sim), &r.options);
    assert_eq!(sim.snapshot(), before, "a probe changes nothing");
    let text = render(&p);
    assert!(text.contains("Verdict   supported: verified on this model"), "{text}");
    assert!(text.contains("get_fan        behavior=0x41 cpu=30 gpu=30"), "{text}");
}

// ---- the report ---------------------------------------------------------------------------------

#[test]
fn the_report_is_json_with_the_python_field_order_and_no_serial_numbers() {
    let r = rig("Predator PH18-71", &[GAMING_UPPER], true);
    let p = probe(Some(&Spy::new(Some("ok v=1\n"))), &r.options);
    let json = to_json(&p).unwrap();
    let keys = [
        "truesense",
        "model",
        "kernel",
        "writes",
        "module",
        "wmi_guids",
        "gaming_interface_present",
        "firmware",
        "answered",
        "verdict",
    ];
    let positions: Vec<usize> = keys.iter().map(|k| json.find(&format!("\n  \"{k}\"")).unwrap()).collect();
    assert!(positions.windows(2).all(|w| w[0] < w[1]), "{json}");
    let value: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["model"]["product"], "Predator PH18-71");
    assert_eq!(value["model"]["bios_verified"], Value::Null);
    assert_eq!(value["model"]["tier"], "unverified");
    assert_eq!(value["writes"], 0);
    assert_eq!(value["module"], serde_json::json!({"loaded": true, "device": true}));
    assert_eq!(value["firmware"]["get_fan"], serde_json::json!({"ok": true, "value": {"v": "1"}}));
    assert!(value.get("note").is_none(), "no note when the module answered");
    let lowered = json.to_lowercase();
    for word in ["serial", "uuid"] {
        assert!(!lowered.contains(word), "{word}");
    }
    let order: Vec<usize> = READ_ONLY.iter().map(|c| json.find(&format!("\"{c}\": {{")).unwrap()).collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "firmware answers keep the command order");
}

#[test]
fn a_failed_answer_serialises_with_its_error_and_an_unbound_probe_with_its_note() {
    let r = rig("Predator PH18-71", &[], true);
    let value: Value =
        serde_json::from_str(&to_json(&probe(Some(&Spy::new(Some("err bad\n"))), &r.options)).unwrap())
            .unwrap();
    assert_eq!(value["firmware"]["caps"], serde_json::json!({"ok": false, "error": "bad"}));
    let r = rig("Predator PH18-71", &[], false);
    let value: Value = serde_json::from_str(&to_json(&probe(None, &r.options)).unwrap()).unwrap();
    assert_eq!(value["firmware"], serde_json::json!({}));
    assert!(value["note"].as_str().unwrap().contains("modprobe truesense_wmi force=1"));
    assert_eq!(value["answered"], "0/0");
}

#[test]
fn the_text_report_for_an_unverified_model() {
    let r = rig("Predator PH18-71", &[GAMING_UPPER], true);
    let p = probe(Some(&Spy::new(Some("ok v=1\n"))), &r.options);
    let text = render(&p);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "Model     Predator PH18-71 (Acer), BIOS V1.18 06/09/2026");
    assert_eq!(lines[1], "Verdict   unverified: share this report");
    assert_eq!(lines[2], "Kernel    7.3.0-test   module bound: yes");
    assert_eq!(lines[3], "WMI       Acer gaming interface PRESENT (1 WMI devices)");
    assert_eq!(lines[4], "  version        v=1");
    assert!(text.contains("Nothing was written (0 writes). Share it: truesense probe --json, then https://"));
}

#[test]
fn the_text_report_for_other_cases() {
    let r = rig("Predator PH16-71", &[], false);
    let text = render(&probe(None, &r.options));
    assert!(
        text.contains("module bound: no") && text.contains("Acer gaming interface not seen (0 WMI devices)")
    );
    assert!(text.contains("force=1"));
    assert!(text.ends_with("Nothing was written (0 writes). "), "{text:?}");
    assert!(!text.contains("Share it"));
    let mut options = r.options.clone();
    options.dmi_root = PathBuf::from("/nonexistent/dmi");
    let text = render(&probe(None, &options));
    assert!(text.starts_with("Model     unknown (unknown vendor), BIOS ?\n"), "{text}");
    assert!(text.contains("Verdict   not an Acer Predator"));
    let r = rig("Predator PH18-71", &[], true);
    let text = render(&probe(Some(&Spy::new(Some("err bad\n"))), &r.options));
    assert!(text.contains("  get_fan        (no answer: bad)"), "{text}");
}

#[test]
fn the_kernel_release_is_read_from_proc() {
    assert!(!kernel_release().is_empty());
    assert!(ProbeOptions::system().kernel == kernel_release());
    assert_eq!(ProbeOptions::system().version, ts_foundation::buildinfo::VERSION);
}
