//! Everything here is a pure function of the text a tool printed (`pactl`, `amixer`,
//! `acerultrax-cli`, `/proc/asound/cards`, a PipeWire config). It never runs anything and never
//! panics on any input, so every format the sound system can produce is covered by a table test
//! with hostile inputs.

use serde::Serialize;

/// The sound profiles AcerUltraX knows.
pub const DTS_PROFILES: [&str; 9] =
    ["Off", "Music", "Movie", "Voice", "Game1", "Game2", "Game3", "Custom Audio", "Plain"];

/// The kinds of output AcerUltraX lists.
const DTS_KINDS: [&str; 5] = ["speakers", "headphones", "bluetooth-speaker", "bluetooth", "other"];

/// The ALSA card index of the laptop's sound card (`sof-hda-dsp` in `/proc/asound/cards`); `"1"`
/// when the text does not name it (it is card 1 on the PH16-71). A line that names the card but
/// has no index returns the first word, as the Python edition did.
pub(crate) fn card_index(cards: &str) -> String {
    cards
        .lines()
        .filter(|l| l.contains("sof-hda-dsp"))
        .find_map(|l| l.split_whitespace().next())
        .unwrap_or("1")
        .to_owned()
}

/// The value of an `amixer cget` control: the text after `: values=` on its last line (`on`,
/// `off`, `2`, `1,1`). `None` when amixer printed nothing.
pub(crate) fn control_value(out: &str) -> Option<String> {
    let last = out.trim().lines().next_back()?.trim();
    if last.is_empty() {
        return None;
    }
    Some(last.replace(": values=", ""))
}

/// The `: values=N` line of a volume control, not the `values=2` channel count in its header.
pub(crate) fn boost_value(out: &str) -> Option<u32> {
    out.lines().find_map(|line| {
        let digits = line.trim_start().strip_prefix(": values=")?;
        let end = digits.find(|c: char| !c.is_ascii_digit()).unwrap_or(digits.len());
        digits[..end].parse().ok()
    })
}

/// Column `col` (0-based, tab separated) of every line of a `pactl list short` output that has more
/// than one column.
pub(crate) fn short_column(out: &str, col: usize) -> Vec<String> {
    out.lines()
        .filter_map(|l| {
            let fields: Vec<&str> = l.split('\t').collect();
            (fields.len() > 1).then(|| fields.get(col).map(|f| (*f).to_owned())).flatten()
        })
        .collect()
}

/// `(id, name)` of every real input of `pactl list short sources`: monitors of outputs are not
/// microphones.
pub(crate) fn short_sources(out: &str) -> Vec<(String, String)> {
    out.lines()
        .filter(|l| !l.contains("monitor"))
        .filter_map(|l| {
            let mut fields = l.split('\t');
            Some((fields.next()?.to_owned(), fields.next()?.to_owned()))
        })
        .collect()
}

/// The first whitespace-separated word of every non-empty line (the id of
/// `pactl list short sink-inputs`).
pub(crate) fn first_fields(out: &str) -> Vec<String> {
    out.lines().filter_map(|l| l.split_whitespace().next()).map(str::to_owned).collect()
}

/// The PipeWire name of the card whose short listing contains `matching`.
pub(crate) fn card_name(short_cards: &str, matching: &str) -> Option<String> {
    short_cards.lines().filter(|l| l.contains(matching)).find_map(|l| l.split('\t').nth(1).map(str::to_owned))
}

/// The part of `pactl list cards` that follows `Name: <card>`, up to the next card when
/// `until_next` is set. `None` when the card is not listed.
pub(crate) fn card_block(list_cards: &str, card: &str, until_next: bool) -> Option<String> {
    let (_, rest) = list_cards.split_once(&format!("Name: {card}"))?;
    let rest = if until_next { rest.split_once("\nCard #").map_or(rest, |(head, _)| head) } else { rest };
    Some(rest.to_owned())
}

/// The `Active Profile:` of a card block (`"?"` when it has none).
pub(crate) fn active_profile(block: &str) -> String {
    block
        .lines()
        .find(|l| l.contains("Active Profile:"))
        .and_then(|l| l.split_once(':'))
        .map_or_else(|| "?".to_owned(), |(_, v)| v.trim().to_owned())
}

/// The profile names under `Profiles:` of a card block.
pub(crate) fn profiles_offered(block: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut on = false;
    for line in block.lines() {
        let s = line.trim();
        if s.starts_with("Profiles:") {
            on = true;
        } else if s.starts_with("Active Profile:") {
            break;
        } else if on && !s.is_empty() {
            if let Some((name, _)) = s.split_once(": ") {
                out.push(name.to_owned());
            }
        }
    }
    out
}

/// The `Description:` of the named sink in `pactl list sinks`, or the name itself when there is none.
pub(crate) fn sink_description(list_sinks: &str, name: &str) -> String {
    let Some((_, rest)) = list_sinks.split_once(&format!("Name: {name}")) else { return name.to_owned() };
    rest.lines()
        .map(str::trim)
        .find(|l| l.starts_with("Description:"))
        .and_then(|l| l.split_once(':'))
        .map_or_else(|| name.to_owned(), |(_, v)| v.trim().to_owned())
}

/// Whether `pactl get-source-mute` said muted (`Mute: yes`).
pub(crate) fn is_muted(out: &str) -> bool {
    out.contains("yes")
}

/// The first percentage of `pactl get-source-volume` (digits followed by `%`).
pub(crate) fn volume_percent(out: &str) -> Option<u32> {
    let bytes = out.as_bytes();
    for (i, byte) in bytes.iter().enumerate() {
        if *byte != b'%' {
            continue;
        }
        let start = bytes[..i].iter().rposition(|c| !c.is_ascii_digit()).map_or(0, |p| p + 1);
        if start < i {
            return out[start..i].parse().ok();
        }
    }
    None
}

/// The VAD threshold of a Purified Voice config; `None` when absent or unreadable.
pub(crate) fn voice_threshold(conf: &str) -> Option<u32> {
    let (_, rest) = conf.split_once("\"VAD Threshold (%)\" = ")?;
    let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// One output AcerUltraX knows. The JSON names are the Python edition's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DtsDevice {
    /// The name shown to the user.
    pub label: String,
    /// `speakers`, `headphones`, `bluetooth-speaker`, `bluetooth` or `other`.
    pub kind: String,
    /// The DTS profile now on it, one of [`DTS_PROFILES`].
    pub profile: String,
    /// What `--device` takes.
    #[serde(rename = "ref")]
    pub reference: String,
    /// Whether this is the output playing now.
    pub current: bool,
}

/// Whether a label names a headset-like output (those get a headphone EQ).
pub(crate) fn is_headset_label(label: &str) -> bool {
    const WORDS: [&str; 11] = [
        "headset",
        "headphone",
        "earphone",
        "earbud",
        "earpiece",
        "buds",
        "airpods",
        "galea",
        "gaming",
        "dualsense",
        "pc38",
    ];
    let lower = label.to_lowercase();
    WORDS.iter().any(|w| lower.contains(w))
}

/// Splits `LABEL  KIND  PROFILE` (the label can run straight into the kind column).
fn parse_dts_row(line: &str) -> Option<(String, String, String)> {
    let line = line.trim_end();
    let before_profile = DTS_PROFILES
        .iter()
        .find_map(|p| line.strip_suffix(p).filter(|r| r.ends_with(char::is_whitespace)))?;
    let before_kind = before_profile.trim_end();
    let (label, kind) = DTS_KINDS.iter().find_map(|k| {
        let label = before_kind.strip_suffix(k)?;
        label.ends_with(char::is_whitespace).then_some((label.trim_end(), *k))
    })?;
    let profile = line[before_profile.len()..].to_owned();
    (!label.is_empty()).then(|| (label.to_owned(), kind.to_owned(), profile))
}

/// Reads `acerultrax-cli devices`: a row per output (`* ` marks the current one) followed by an
/// indented line with the reference `--device` takes. A row without a reference is dropped.
pub(crate) fn parse_dts_devices(out: &str) -> Vec<DtsDevice> {
    let mut devices: Vec<DtsDevice> = Vec::new();
    for line in out.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with("    ") && !devices.is_empty() {
            if let Some(last) = devices.last_mut() {
                last.reference = line.trim().to_owned();
            }
        } else if !line.trim().is_empty() {
            let row = line.trim_start_matches(['*', ' ']);
            if let Some((label, kind, profile)) = parse_dts_row(row) {
                devices.push(DtsDevice {
                    label,
                    kind,
                    profile,
                    reference: String::new(),
                    current: line.starts_with('*'),
                });
            }
        }
    }
    devices.retain(|d| !d.reference.is_empty());
    devices
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::audio::voice::VoiceMode;

    pub(crate) const REAL_CARDS: &str = " 0 [sofhdadsp     ]: sof-hda-dsp - sof-hda-dsp\n                      Intel Corp.-Predator PH16-71-V1.18-Type1Sku0\n 1 [Device         ]: USB-Audio - USB Audio Device\n                      GeneralPlus USB Audio Device at usb-0000:00:14.0-3, full speed\n";

    const SINKS: &str = "37\tacerultrax_sink\tPipeWire\ts32le 2ch 48000Hz\tRUNNING\n4126\talsa_output.pci-0000_00_1f.3-platform-skl_hda_dsp_generic.HiFi__Speaker__sink\tPipeWire\ts32le 2ch 48000Hz\tSUSPENDED\n55\talsa_output.pci-0000_00_03.0.hdmi-stereo\tPipeWire\ts32le 2ch 48000Hz\tSUSPENDED\n";

    #[test]
    fn card_index_table() {
        let table = [
            (REAL_CARDS, "0"),
            (" 0 [PCH]: HDA-Intel - HDA Intel PCH\n 3 [sofhdadsp]: sof-hda-dsp - sof-hda-dsp\n", "3"),
            (" 0 [PCH]: HDA-Intel - HDA Intel PCH\n", "1"),
            ("", "1"),
            ("\n\n", "1"),
            ("sof-hda-dsp", "sof-hda-dsp"), // the Python edition's split()[0]
            ("   sof-hda-dsp  \n", "sof-hda-dsp"),
            ("\u{0}sof-hda-dsp\u{0}", "\u{0}sof-hda-dsp\u{0}"),
        ];
        for (text, want) in table {
            assert_eq!(card_index(text), want, "{text:?}");
        }
    }

    #[test]
    fn control_value_table() {
        let table: [(&str, Option<&str>); 7] = [
            (
                "numid=33,iface=CARD,name='Headphone Jack'\n  ; type=BOOLEAN,access=r-------,values=1\n  : values=on\n",
                Some("on"),
            ),
            (
                "numid=34,iface=CARD,name='Headset Mic Jack'\n  ; type=BOOLEAN,access=r-------,values=1\n  : values=off\n",
                Some("off"),
            ),
            (
                "numid=20\n  ; type=INTEGER,values=2,min=0,max=3\n  : values=2,2\n  | dBscale\n",
                Some("| dBscale"),
            ), // the LAST line is what counts
            (": values=on", Some("on")),
            ("", None),
            (" \n\t\n", None),
            ("amixer: Cannot find the given element\n", Some("amixer: Cannot find the given element")),
        ];
        for (text, want) in table {
            assert_eq!(control_value(text).as_deref(), want, "{text:?}");
        }
    }

    #[test]
    fn boost_value_reads_the_value_line_not_the_channel_count() {
        let header = "numid=8,iface=MIXER,name='Headset Mic Boost Volume'\n  ; type=INTEGER,access=rw---R--,values=2,min=0,max=3,step=0\n  : values=1,1\n  | dBscale-min=0.00dB,step=10.00dB,mute=0\n";
        let table: [(&str, Option<u32>); 9] = [
            (header, Some(1)),
            ("  : values=3\n", Some(3)),
            ("  : values=0,0\n", Some(0)),
            ("amixer: Cannot find the given element\n", None),
            ("", None),
            ("  ; type=INTEGER,values=2\n", None),
            ("  : values=\n", None),
            ("  : values=x\n", None),
            ("  : values=99999999999999999999\n", None),
        ];
        for (text, want) in table {
            assert_eq!(boost_value(text), want, "{text:?}");
        }
    }

    #[test]
    fn short_listings() {
        let want = [
            "acerultrax_sink",
            "alsa_output.pci-0000_00_1f.3-platform-skl_hda_dsp_generic.HiFi__Speaker__sink",
            "alsa_output.pci-0000_00_03.0.hdmi-stereo",
        ];
        assert_eq!(short_column(SINKS, 1), want);
        assert!(short_column("a\tb\n", 5).is_empty() && short_column("", 1).is_empty());
        assert!(short_column("no tab here\n", 0).is_empty());
        let sources = "50\talsa_input.x.HiFi__Mic1__source\tPipeWire\n51\talsa_output.x.sink.monitor\tPipeWire\n52\ttruesense_pv_headset\tPipeWire\n53\n";
        assert_eq!(
            short_sources(sources),
            [
                ("50".to_owned(), "alsa_input.x.HiFi__Mic1__source".to_owned()),
                ("52".to_owned(), "truesense_pv_headset".to_owned())
            ]
        );
        assert_eq!(first_fields("12\t77\tPipeWire\tFirefox\n\n  34  x\n"), ["12", "34"]);
        assert!(first_fields("").is_empty());
    }

    #[test]
    fn card_name_table() {
        let card = "alsa_card.pci-0000_00_1f.3-platform-skl_hda_dsp_generic";
        let cards = format!("70\talsa_card.usb-Other\talsa\n71\t{card}\talsa\n");
        assert_eq!(card_name(&cards, "pci-0000_00_1f.3-platform-skl_hda_dsp_generic").as_deref(), Some(card));
        for bad in [
            "",
            "70\talsa_card.usb-Other\talsa\n",
            "pci-0000_00_1f.3-platform-skl_hda_dsp_generic with no tab\n",
        ] {
            assert_eq!(card_name(bad, "pci-0000_00_1f.3-platform-skl_hda_dsp_generic"), None, "{bad:?}");
        }
    }

    const LIST_CARDS: &str = "Card #70\n\tName: alsa_card.usb-Other\n\tProfiles:\n\t\toff: Off (sinks: 0, sources: 0, priority: 0, available: yes)\n\tActive Profile: off\nCard #71\n\tName: alsa_card.x\n\tDriver: alsa\n\tProfiles:\n\t\toff: Off (sinks: 0, sources: 0, priority: 0, available: yes)\n\t\tHiFi (Headphones, Mic1, Mic2): Play HiFi quality Music (sinks: 1, sources: 2, priority: 8000, available: yes)\n\t\tHiFi (Mic1, Mic2, Speaker): Play HiFi quality Music (sinks: 1, sources: 2, priority: 8000, available: yes)\n\t\tpro-audio: Pro Audio (sinks: 4, sources: 4, priority: 1, available: yes)\n\tActive Profile: HiFi (Mic1, Mic2, Speaker)\n\tPorts:\n\t\tanalog-output-speaker: Speaker (type: Speaker, priority: 100)\nCard #72\n\tName: alsa_card.pci-0000_00_03.0\n\tActive Profile: output:hdmi-stereo\n";

    #[test]
    fn card_block_active_profile_and_profiles_offered() {
        let block = card_block(LIST_CARDS, "alsa_card.x", true).unwrap();
        assert!(
            !block.contains("Card #72") && !block.contains("hdmi-stereo"),
            "the block runs into the next card"
        );
        assert_eq!(active_profile(&block), "HiFi (Mic1, Mic2, Speaker)");
        assert_eq!(
            profiles_offered(&block),
            ["off", "HiFi (Headphones, Mic1, Mic2)", "HiFi (Mic1, Mic2, Speaker)", "pro-audio"]
        );
        // The un-cut block (what the output status reads) still has the right first Active Profile.
        assert_eq!(
            active_profile(&card_block(LIST_CARDS, "alsa_card.x", false).unwrap()),
            "HiFi (Mic1, Mic2, Speaker)"
        );
        assert_eq!(card_block(LIST_CARDS, "alsa_card.nonexistent", true), None);
        assert_eq!(active_profile("\tDriver: alsa\n"), "?");
        assert_eq!(active_profile("Active Profile:"), "");
        assert!(profiles_offered("\tDriver: alsa\n").is_empty());
        assert_eq!(profiles_offered("\tProfiles:\n\t\toff: Off (x)\n"), ["off"]);
    }

    #[test]
    fn sink_description_table() {
        let sinks = "Sink #37\n\tState: RUNNING\n\tName: acerultrax_sink\n\tDescription: AcerUltraX\n\nSink #38\n\tName: bluez_output.AA.1\n\tDescription: Some Speaker\n\tDriver: PipeWire\n";
        assert_eq!(sink_description(sinks, "bluez_output.AA.1"), "Some Speaker");
        assert_eq!(sink_description(sinks, "gone"), "gone");
        assert_eq!(sink_description("Sink #1\n\tName: nodesc\n\tDriver: x\n", "nodesc"), "nodesc");
    }

    #[test]
    fn mute_and_volume() {
        assert!(is_muted("Mute: yes\n") && !is_muted("Mute: no\n") && !is_muted(""));
        let table: [(&str, Option<u32>); 9] = [
            (
                "Volume: front-left: 65536 / 100% / 0.00 dB,   front-right: 65536 / 100% / 0.00 dB\n        balance 0.00\n",
                Some(100),
            ),
            ("Volume: mono: 98304 / 150% / 10.57 dB", Some(150)),
            ("Volume: 0%", Some(0)),
            ("Volume: unavailable", None),
            ("", None),
            ("%", None),
            ("% 40%", Some(40)),
            ("99999999999999999999%", None),
            ("x%y 7%", Some(7)),
        ];
        for (text, want) in table {
            assert_eq!(volume_percent(text), want, "{text:?}");
        }
    }

    #[test]
    fn voice_threshold_reads_what_the_config_writes() {
        use crate::audio::voice::{VoiceDevice, voice_conf};
        for mode in [VoiceMode::Conference, VoiceMode::Personal] {
            let conf = voice_conf(VoiceDevice::Headset, mode, "/p.so").unwrap();
            assert_eq!(voice_threshold(&conf), mode.gate().map(|g| g.0), "{mode:?}");
        }
        for bad in ["nothing here", "\"VAD Threshold (%)\" = x", "", "\"VAD Threshold (%)\" = "] {
            assert_eq!(voice_threshold(bad), None, "{bad:?}");
        }
    }

    pub(crate) const REAL_DEVICES: &str = "  Laptop speakers                                    speakers           Music\n    @speakers\n  Headphones (3.5 mm jack)                           headphones         Music\n    @headphones\n  DualSense wireless controller (PS5) Direct DualSense Wireless Controller other              Plain\n    alsa_output.usb-Sony_DualSense-00.Direct__Direct__sink\n* Headset PC38X                                      other              Plain\n    alsa_output.usb-GeneralPlus_USB_Audio_Device-00.analog-stereo\n  Some Speaker                                       bluetooth-speaker  Plain\n    bluez_output.AA.1\n";

    fn device(label: &str, kind: &str, profile: &str, reference: &str, current: bool) -> DtsDevice {
        DtsDevice {
            label: label.into(),
            kind: kind.into(),
            profile: profile.into(),
            reference: reference.into(),
            current,
        }
    }

    fn real_devices() -> Vec<DtsDevice> {
        vec![
            device("Laptop speakers", "speakers", "Music", "@speakers", false),
            device("Headphones (3.5 mm jack)", "headphones", "Music", "@headphones", false),
            device(
                "DualSense wireless controller (PS5) Direct DualSense Wireless Controller",
                "other",
                "Plain",
                "alsa_output.usb-Sony_DualSense-00.Direct__Direct__sink",
                false,
            ),
            device(
                "Headset PC38X",
                "other",
                "Plain",
                "alsa_output.usb-GeneralPlus_USB_Audio_Device-00.analog-stereo",
                true,
            ),
            device("Some Speaker", "bluetooth-speaker", "Plain", "bluez_output.AA.1", false),
        ]
    }

    #[test]
    fn dts_devices_are_read_including_labels_that_run_into_the_kind_column() {
        // The real helper's output is stripped, so the first row loses its indent.
        assert_eq!(parse_dts_devices(REAL_DEVICES.trim()), real_devices());
        assert_eq!(parse_dts_devices(REAL_DEVICES), real_devices());
        let crlf = REAL_DEVICES.replace('\n', "\r\n")
            + "  Orphan                                             other              Plain\r\n";
        assert_eq!(parse_dts_devices(&crlf), real_devices(), "CRLF line ends and a row without a reference");
        for text in [
            "",
            "\n\n",
            "No devices found",
            "    ref-without-a-row\n",
            "  Row without kind    Plain\n    ref\n",
            "other Plain\n    r\n",
            "  other  Plain\n    r\n",
            "\u{0}\n    x\n",
        ] {
            assert!(parse_dts_devices(text).is_empty(), "{text:?}");
        }
        for profile in DTS_PROFILES {
            let devices = parse_dts_devices(&format!("  Dev  other  {profile}\n    ref1\n"));
            assert_eq!(devices.len(), 1, "{profile}");
            assert_eq!(devices[0].profile, profile);
        }
        assert_eq!(parse_dts_devices("  Dev  bluetooth  Game2\n    r\n")[0].kind, "bluetooth");
        // Only the last reference line before the next row counts, as in Python.
        assert_eq!(parse_dts_devices("  A  other  Plain\n    r1\n    r2\n")[0].reference, "r2");
        let json = serde_json::to_value(&real_devices()[3]).unwrap();
        assert_eq!(json["ref"], "alsa_output.usb-GeneralPlus_USB_Audio_Device-00.analog-stereo");
        assert_eq!(json["current"], true);
    }

    #[test]
    fn headset_labels() {
        let table = [
            ("Headset PC38X", true),
            ("Logitech G Pro Gaming", true),
            ("AirPods Pro", true),
            ("Galea", true),
            ("Sony Earbuds", true),
            ("Wired Headphones", true),
            ("Mi Buds", true),
            ("DualSense wireless controller", true),
            ("Laptop speakers", false),
            ("HDMI / DisplayPort", false),
            ("Some Speaker", false),
            ("Dock", false),
            ("", false),
            ("HEADSET", true),
        ];
        for (label, want) in table {
            assert_eq!(is_headset_label(label), want, "{label:?}");
        }
    }

    #[test]
    fn no_parser_panics_on_hostile_input() {
        let nasty = [
            String::new(),
            "\u{0}".repeat(50),
            "\t\t\t\n\t".into(),
            "Name: ".into(),
            "Card #1\n\tName: x\n\tActive Profile:".into(),
            "%".repeat(100),
            ": values=".repeat(50),
            "é".repeat(1000),
            "\n".repeat(1000),
            "Profiles:\n".repeat(10),
            "\u{ffff}%\u{ffff}".into(),
            "* ".repeat(500),
            "    ".repeat(100),
            format!("{} other Plain\n    r", "x".repeat(10_000)),
        ];
        for text in &nasty {
            let _ = (
                card_index(text),
                control_value(text),
                boost_value(text),
                short_column(text, 1),
                short_sources(text),
                first_fields(text),
            );
            let _ = (
                card_name(text, "x"),
                card_block(text, "x", true),
                active_profile(text),
                profiles_offered(text),
                sink_description(text, "x"),
            );
            let _ = (
                is_muted(text),
                volume_percent(text),
                voice_threshold(text),
                parse_dts_devices(text),
                is_headset_label(text),
            );
        }
    }
}
