//! One path per app through AcerUltraX. When an app picks its own output device (a browser call
//! set to "USB headset" instead of "Default"), PipeWire links the app straight to that device, and
//! AcerUltraX then adds its own path (the app into its DTS engine, the engine out to the same
//! device) without removing the direct one: every voice is heard twice, a few milliseconds apart.
//! The audio watcher removes the direct links of every app port that also feeds AcerUltraX.
#![forbid(unsafe_code)]

use super::Audio;

/// A link to remove: its id, and the two ports for the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Duplicate {
    pub link: String,
    pub from: String,
    pub to: String,
}

/// Whether a port belongs to a place sound is played: a device, or the AcerUltraX sink.
fn is_playback(port: &str) -> bool {
    let node = port.rsplit_once(':').map_or(port, |(n, _)| n);
    node.starts_with("alsa_output.") || node.starts_with("bluez_output.") || node == "acerultrax_sink"
}

/// The duplicate links in `pw-link -lI` (`ID port`, then `LINK   |->  ID port` per link): for
/// every output port that feeds an AcerUltraX DTS engine (`acerultrax_in...`), its other links to a
/// device or to the AcerUltraX sink.
pub(crate) fn duplicates(listing: &str) -> Vec<Duplicate> {
    let mut out = Vec::new();
    let mut port = String::new();
    let mut targets: Vec<(String, String)> = Vec::new();
    let mut flush = |port: &str, targets: &mut Vec<(String, String)>| {
        if targets.iter().any(|(_, t)| t.starts_with("acerultrax_in")) {
            for (link, to) in targets.iter().filter(|(_, t)| is_playback(t)) {
                out.push(Duplicate { link: link.clone(), from: port.to_owned(), to: to.clone() });
            }
        }
        targets.clear();
    };
    for line in listing.lines() {
        let mut words = line.split_whitespace();
        let Some(first) = words.next() else { continue };
        match words.next() {
            Some("|->") => {
                // `LINK |-> ID port`
                let _target_id = words.next();
                if let Some(to) = words.next() {
                    targets.push((first.to_owned(), to.to_owned()));
                }
            }
            Some("|<-") => {}
            Some(name) if first.bytes().all(|b| b.is_ascii_digit()) => {
                flush(&port, &mut targets);
                port = name.to_owned();
            }
            _ => {}
        }
    }
    flush(&port, &mut targets);
    out
}

impl Audio {
    /// Removes every direct link of an app that also plays through AcerUltraX (see the module
    /// note) and answers what it removed. One `pw-link` call when there is nothing to do; without
    /// AcerUltraX no port feeds a DTS engine, so nothing is ever removed.
    pub fn single_dts_path(&self) -> Vec<Duplicate> {
        let found = duplicates(&self.out("pw-link", &["-lI"]));
        for d in &found {
            self.run("pw-link", &["-d", &d.link]);
            self.log(&format!(
                "removed a second path: {} -> {} (the app also plays through AcerUltraX)",
                d.from, d.to
            ));
        }
        found
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::audio::testing::*;

    /// The listing as it was on the laptop during the call (ids shortened).
    const DOUBLED: &str = "\
 186 Firefox:output_FL
 188   |->  137 acerultrax_in_ready63:playback_FL
 141 Firefox:output_FL
 154   |->  300 alsa_output.usb-GeneralPlus_USB_Audio_Device-00.analog-stereo:playback_FL
 169   |->  301 acerultrax_sink:playback_FL
 160   |->  302 acerultrax_in_ready64:playback_FL
 142 Firefox:output_FR
 160   |->  303 acerultrax_in_ready64:playback_FR
 151   |->  304 alsa_output.usb-GeneralPlus_USB_Audio_Device-00.analog-stereo:playback_FR
 400 acerultrax_output:output_FL
 401   |->  300 alsa_output.usb-GeneralPlus_USB_Audio_Device-00.analog-stereo:playback_FL
 500 obs:output_FL
 501   |->  300 alsa_output.usb-GeneralPlus_USB_Audio_Device-00.analog-stereo:playback_FL
 502   |->  600 recorder:input_FL
  97 input.filter-chain-8460-29:input_FL
 102   |<-  139 alsa_input.pci-0000_00_1f.3-platform-skl_hda_dsp_generic.HiFi__Mic1__source:capture_FL
";

    #[test]
    fn only_the_direct_links_of_ports_that_feed_acerultrax_are_duplicates() {
        let d = duplicates(DOUBLED);
        let links: Vec<&str> = d.iter().map(|x| x.link.as_str()).collect();
        assert_eq!(links, ["154", "169", "151"], "{d:?}");
        assert_eq!(d[0].from, "Firefox:output_FL");
        assert!(d[0].to.starts_with("alsa_output.usb-GeneralPlus"));
        // AcerUltraX's own output, an app that does not go through it and a recorder stay.
        assert!(duplicates("").is_empty());
        assert!(duplicates(" 1 app:out\n 2   |->  3 alsa_output.x:playback_FL\n").is_empty());
    }

    #[test]
    fn the_guard_removes_the_duplicates_and_nothing_else() {
        let rig = Rig::new(false);
        rig.sys.state().links = DOUBLED.into();
        let removed = rig.audio.single_dts_path();
        assert_eq!(removed.len(), 3);
        for id in ["154", "169", "151"] {
            assert!(rig.sys.did(&["pw-link", "-d", id]), "{id}");
        }
        assert!(!rig.sys.did(&["pw-link", "-d", "401"]) && !rig.sys.did(&["pw-link", "-d", "501"]));
        // Without AcerUltraX (no DTS engine input) an app linked to a device is left alone.
        let rig = Rig::new(false);
        rig.sys.state().no_front_sink = true;
        rig.sys.state().links = " 1 Firefox:output_FL\n 2   |->  3 alsa_output.x:playback_FL\n".into();
        assert!(rig.audio.single_dts_path().is_empty());
        assert!(!rig.sys.did(&["pw-link", "-d"]));
    }
}
