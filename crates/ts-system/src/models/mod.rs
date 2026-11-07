//! models: which laptops TrueSense knows, and a probe that learns about the rest WITHOUT
//! changing anything.
//!
//! Three tiers, because firmware control must never be guessed:
//!
//! * `supported`: verified on real hardware (a maintainer or a user report); everything works
//!   and is tested.
//! * `unverified`: an Acer Predator or Nitro with no verified report. The kernel module refuses
//!   to bind (it can be forced, at your own risk). `truesense probe` only READS, and its output
//!   is what a maintainer needs to move the model to "supported" in a release.
//! * `unsupported`: not an Acer; TrueSense has nothing to do here.
//!
//! The probe runs behind [`ts_platform::firmware::ReadOnly`] (inside
//! [`ts_platform::hw::probe_firmware`]), so "nothing is written" holds by construction.
#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use ts_foundation::{buildinfo, fsx};
use ts_platform::firmware::{DEVICE_PATH, Device, Reply};
use ts_platform::hw::probe_firmware;

pub use ts_platform::hw::READ_ONLY;

/// Where the kernel publishes the machine's identity.
pub const DMI_ROOT: &str = "/sys/class/dmi/id";

/// Where a user of an unverified model sends the probe report.
pub const REPORT_URL: &str = "https://github.com/Order52/truesense/issues/new?template=new_model_report.yml";

/// The Acer gaming WMI interface TrueSense talks to; its presence is the first sign that a model
/// has it.
pub const GAMING_WMI_GUID: &str = "7a4ddfe7-5b5d-40b4-8595-4408e0cc7f56";

/// How much of a firmware error message the report keeps (the Python edition cut at 120).
const ERROR_LIMIT: usize = 120;

/// The note shown when the module is not bound, so the firmware was not asked anything.
const UNBOUND_NOTE: &str = "the kernel module is not bound (expected on an unverified model). To let the probe read the firmware: sudo modprobe truesense_wmi force=1   (reads only; undo with: sudo modprobe -r truesense_wmi)";

/// How far TrueSense trusts a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tier {
    /// Verified on real hardware.
    Supported,
    /// A similar Acer gaming laptop nobody has verified.
    Unverified,
    /// Not an Acer Predator or Nitro.
    Unsupported,
}

impl Tier {
    /// The word reports and the doctor use: `supported`, `unverified` or `unsupported`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Unverified => "unverified",
            Self::Unsupported => "unsupported",
        }
    }

    /// The one-line verdict of the probe report.
    fn verdict(self) -> &'static str {
        match self {
            Self::Supported => "supported: verified on this model",
            Self::Unverified => "unverified: share this report",
            Self::Unsupported => "not an Acer Predator",
        }
    }
}

impl Serialize for Tier {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// One verified model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Known {
    /// The DMI product name the kernel module also matches.
    pub product: &'static str,
    /// The marketing family, shown in the window.
    pub family: &'static str,
    /// The BIOS versions the model was checked on. Another BIOS is only a note.
    pub verified_bios: &'static [&'static str],
    /// Where the verification report lives.
    pub report: &'static str,
}

/// The verified models. Add a model here only with a verified hardware report
/// (`docs/STATUS.md`): the kernel module matches the same names, and firmware control must never
/// be guessed.
pub const SUPPORTED: &[Known] = &[Known {
    product: "Predator PH16-71",
    family: "Predator Helios 16",
    verified_bios: &["V1.18"],
    report: "docs/STATUS.md",
}];

/// What [`identify`] learns from the DMI tables. The JSON names are the Python edition's probe's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Model {
    /// `sys_vendor`, or empty.
    pub vendor: String,
    /// `product_name`, or `unknown`.
    pub product: String,
    /// `bios_version`, or `?`.
    pub bios: String,
    /// `bios_date`, or empty.
    pub bios_date: String,
    /// The family of a supported model, `similar Acer gaming laptop` for an unverified one.
    pub family: String,
    /// Whether the BIOS is one the model was verified on; `None` unless the model is supported.
    pub bios_verified: Option<bool>,
    /// How far this model is trusted.
    pub tier: Tier,
    /// What to tell the person.
    pub message: String,
}

fn read_dmi(root: &Path, name: &str) -> String {
    // A missing, unreadable or non-UTF-8 field is an empty one: the machine is then graded
    // unsupported, never an error.
    fsx::read_trim_opt(&root.join(name)).unwrap_or_default()
}

fn is_acer(vendor: &str) -> bool {
    vendor.to_lowercase().starts_with("acer")
}

/// Whether `product` looks like an Acer gaming laptop name: `Predator` or `Nitro`, whitespace,
/// one to four letters, then two digits, a dash and two digits (`Predator PHN16-71`). Matched by
/// hand, case-insensitively, as a prefix (the Python edition used a regular expression with `re.match`).
fn looks_like_gaming_laptop(product: &str) -> bool {
    let rest = ["predator", "nitro"].iter().find_map(|word| {
        let head = product.get(..word.len())?;
        head.eq_ignore_ascii_case(word).then(|| &product[word.len()..])
    });
    let Some(rest) = rest else { return false };
    let after_space = rest.trim_start_matches(char::is_whitespace);
    if after_space.len() == rest.len() {
        return false; // at least one whitespace character is required
    }
    let letters = after_space.bytes().take_while(u8::is_ascii_alphabetic).count();
    if !(1..=4).contains(&letters) {
        return false;
    }
    let tail = after_space.as_bytes();
    let tail = &tail[letters..];
    // dd-dd
    matches!(tail, [a, b, b'-', c, d, ..] if [a, b, c, d].iter().all(|x| x.is_ascii_digit()))
}

/// Reads the machine's DMI identity under `root` ([`DMI_ROOT`] on a real system) and grades it.
/// A missing directory is an unsupported machine, never an error.
pub fn identify(root: &Path) -> Model {
    let vendor = read_dmi(root, "sys_vendor");
    let product = read_dmi(root, "product_name");
    let bios = read_dmi(root, "bios_version");
    let bios_date = read_dmi(root, "bios_date");
    let mut model = Model {
        vendor: vendor.clone(),
        product: if product.is_empty() { "unknown".to_owned() } else { product.clone() },
        bios: if bios.is_empty() { "?".to_owned() } else { bios.clone() },
        bios_date,
        family: String::new(),
        bios_verified: None,
        tier: Tier::Unsupported,
        message: String::new(),
    };
    // The kernel module matches vendor AND name: so does this.
    let known = SUPPORTED.iter().find(|k| k.product == product).filter(|_| is_acer(&vendor));
    if let Some(known) = known {
        let verified = known.verified_bios.contains(&bios.as_str());
        model.tier = Tier::Supported;
        model.family = known.family.to_owned();
        model.bios_verified = Some(verified);
        model.message = format!("{product}, BIOS {}", model.bios);
        if !verified {
            model.message.push_str(&format!(
                " (verified on BIOS {}: yours is newer or older, which is expected to work: tell us if it does not)",
                known.verified_bios.join(", ")
            ));
        }
    } else if is_acer(&vendor) && looks_like_gaming_laptop(&product) {
        model.tier = Tier::Unverified;
        model.family = "similar Acer gaming laptop".to_owned();
        model.message = format!(
            "{product} is a similar Acer laptop that TrueSense has not been verified on. Nothing is written to its firmware. `truesense probe` reads what it answers (read-only); share the report so it can be supported: {REPORT_URL}"
        );
    } else {
        let shown_vendor = if vendor.is_empty() { "unknown vendor" } else { vendor.as_str() };
        model.message =
            format!("'{}' ({shown_vendor}) is not an Acer Predator: TrueSense does not apply", model.product);
    }
    model
}

/// The running kernel's release (`uname -r`), read from `/proc/sys/kernel/osrelease` so no
/// system call needs `unsafe`. Empty when it cannot be read.
pub fn kernel_release() -> String {
    fsx::read_trim_opt(Path::new("/proc/sys/kernel/osrelease")).unwrap_or_default()
}

/// Whether the kernel module is loaded and whether its device node exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ModuleState {
    /// `/sys/module/truesense_wmi` exists.
    pub loaded: bool,
    /// The device node exists, which means the module bound to this laptop.
    pub device: bool,
}

/// What one read-only command answered: the reply, or why there was none (cut to 120 characters).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// The command, exactly as sent.
    pub command: &'static str,
    /// The reply, or the error text.
    pub result: Result<Reply, String>,
}

/// The probe's result; its JSON is what a maintainer reads to verify a model. The field order
/// and names are the Python edition's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    /// This build's version.
    pub truesense: String,
    /// What the DMI tables say.
    pub model: Model,
    /// The running kernel.
    pub kernel: String,
    /// How many writes the probe made: always 0, kept in the report so a reader can see the claim.
    pub writes: u32,
    /// The kernel module's state.
    pub module: ModuleState,
    /// The GUIDs of the WMI devices the kernel found, sorted and lower-case.
    pub wmi_guids: Vec<String>,
    /// Whether the Acer gaming interface is among them.
    pub gaming_interface_present: bool,
    /// The firmware's answers in the order the commands were sent; empty when the module is not bound.
    #[serde(serialize_with = "serialize_answers")]
    pub firmware: Vec<Answer>,
    /// Why the firmware was not asked, when it was not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// `answered/total`, or `0/0` when nothing was asked.
    pub answered: String,
    /// One line: is this model supported, unverified or not an Acer Predator.
    pub verdict: String,
}

fn serialize_answers<S: Serializer>(answers: &[Answer], serializer: S) -> Result<S::Ok, S::Error> {
    struct Outcome<'a>(&'a Result<Reply, String>);
    impl Serialize for Outcome<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let mut map = serializer.serialize_map(Some(2))?;
            match self.0 {
                Ok(reply) => {
                    map.serialize_entry("ok", &true)?;
                    map.serialize_entry("value", reply)?;
                }
                Err(error) => {
                    map.serialize_entry("ok", &false)?;
                    map.serialize_entry("error", error)?;
                }
            }
            map.end()
        }
    }
    let mut map = serializer.serialize_map(Some(answers.len()))?;
    for answer in answers {
        map.serialize_entry(answer.command, &Outcome(&answer.result))?;
    }
    map.end()
}

/// Where the probe looks. [`ProbeOptions::system`] is the real machine; tests point the paths at
/// a temporary tree.
#[derive(Debug, Clone)]
pub struct ProbeOptions {
    /// The DMI directory ([`DMI_ROOT`]).
    pub dmi_root: PathBuf,
    /// The `/sys` tree (module and WMI devices are looked up below it).
    pub sys_root: PathBuf,
    /// The kernel module's device node; the firmware is only asked when it exists.
    pub device_path: PathBuf,
    /// The running kernel release.
    pub kernel: String,
    /// This build's version.
    pub version: String,
}

impl ProbeOptions {
    /// The real machine.
    pub fn system() -> Self {
        Self {
            dmi_root: PathBuf::from(DMI_ROOT),
            sys_root: PathBuf::from("/sys"),
            device_path: PathBuf::from(DEVICE_PATH),
            kernel: kernel_release(),
            version: buildinfo::VERSION.to_owned(),
        }
    }
}

/// Whether `text` is a GUID written as 8-4-4-4-12 lower-case hex digits.
fn is_guid(text: &str) -> bool {
    let groups: Vec<&str> = text.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(g, n)| g.len() == n && g.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// The WMI devices the kernel found. A directory is named after the GUID, with a `-N` suffix
/// when the same GUID appears twice, so the GUID is the first 36 characters.
fn wmi_guids(sys_root: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(sys_root.join("bus/wmi/devices")) else { return Vec::new() };
    let mut guids: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().to_str().map(str::to_lowercase))
        .map(|name| name.chars().take(36).collect::<String>())
        .filter(|g| is_guid(g))
        .collect();
    guids.sort();
    guids.dedup();
    guids
}

fn truncate_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// Everything worth knowing about this machine, read-only; it never fails. `device` is the
/// firmware, asked only when the kernel module's device node exists (so that nothing is sent to a
/// laptop whose module is not bound) and only the commands of [`READ_ONLY`], in that order, behind
/// a guard that refuses any command that could write.
pub fn probe(device: Option<&dyn Device>, options: &ProbeOptions) -> Report {
    let model = identify(&options.dmi_root);
    let module = ModuleState {
        loaded: options.sys_root.join("module/truesense_wmi").is_dir(),
        device: options.device_path.exists(),
    };
    let wmi_guids = wmi_guids(&options.sys_root);
    let gaming_interface_present = wmi_guids.iter().any(|g| g == GAMING_WMI_GUID);
    let mut firmware = Vec::new();
    let mut note = None;
    match device {
        Some(device) if module.device => {
            for entry in probe_firmware(device).entries() {
                let result = match &entry.outcome {
                    Ok(reply) => Ok(reply.clone()),
                    Err(e) => Err(truncate_chars(&e.to_string(), ERROR_LIMIT)),
                };
                firmware.push(Answer { command: entry.command, result });
            }
        }
        _ => note = Some(UNBOUND_NOTE.to_owned()),
    }
    let answered = firmware.iter().filter(|a| a.result.is_ok()).count();
    Report {
        truesense: options.version.clone(),
        verdict: model.tier.verdict().to_owned(),
        model,
        kernel: options.kernel.clone(),
        writes: 0,
        module,
        wmi_guids,
        gaming_interface_present,
        answered: if firmware.is_empty() {
            "0/0".to_owned()
        } else {
            format!("{answered}/{}", READ_ONLY.len())
        },
        firmware,
        note,
    }
}

/// The probe as text for a person.
pub fn render(report: &Report) -> String {
    let m = &report.model;
    let vendor = if m.vendor.is_empty() { "unknown vendor" } else { m.vendor.as_str() };
    let yes_no = |b: bool| if b { "yes" } else { "no" };
    let mut lines = vec![
        format!("Model     {} ({vendor}), BIOS {} {}", m.product, m.bios, m.bios_date).trim_end().to_owned(),
        format!("Verdict   {}", report.verdict),
        format!("Kernel    {}   module bound: {}", report.kernel, yes_no(report.module.device)),
        format!(
            "WMI       Acer gaming interface {} ({} WMI devices)",
            if report.gaming_interface_present { "PRESENT" } else { "not seen" },
            report.wmi_guids.len()
        ),
    ];
    for answer in &report.firmware {
        lines.push(match &answer.result {
            Ok(reply) => {
                let fields: Vec<String> = reply.iter().map(|(k, v)| format!("{k}={v}")).collect();
                format!("  {:<14} {}", answer.command, fields.join(" "))
            }
            Err(error) => format!("  {:<14} (no answer: {error})", answer.command),
        });
    }
    if let Some(note) = &report.note {
        lines.push(String::new());
        lines.push(note.clone());
    }
    let share = if m.tier == Tier::Unverified {
        format!("Share it: truesense probe --json, then {REPORT_URL}")
    } else {
        String::new()
    };
    lines.push(String::new());
    lines.push(format!("Nothing was written ({} writes). {share}", report.writes));
    lines.join("\n")
}

/// The probe as indented JSON, for `truesense probe --json` and issue reports.
pub fn to_json(report: &Report) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(report)
}

#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) mod testutil;
