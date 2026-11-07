//! The contract between the page, `COMMANDS.md` and the command table: the three must agree, so
//! a button can never call a command that does not exist.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;

use super::harness::Machine;
use crate::gui::assets::FILES;

const COMMANDS_MD: &str = include_str!("../COMMANDS.md");

fn web_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("web")
}

/// The (area, verb) rows of the table in `COMMANDS.md`: `| `area` | `verb` [/ `verb`] |`.
fn documented() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in COMMANDS_MD.lines().filter(|l| l.starts_with("| `")) {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        let [_, area, verbs, ..] = cells.as_slice() else { continue };
        let word = |s: &str| s.strip_prefix('`').and_then(|s| s.strip_suffix('`')).map(str::to_owned);
        let Some(area) = word(area) else { continue };
        for verb in verbs.split(" / ").filter_map(word) {
            out.insert(format!("{area}/{verb}"));
        }
    }
    out
}

fn files_below(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read_dir").flatten() {
        let path = entry.path();
        if path.is_dir() {
            files_below(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// The (area, verb) pairs the front end calls: `call("area", "verb"` and `tryCall(...)`.
fn used() -> BTreeSet<String> {
    let mut files = Vec::new();
    files_below(&web_dir().join("js"), &mut files);
    let mut out = BTreeSet::new();
    for file in files.iter().filter(|f| f.extension().is_some_and(|e| e == "js")) {
        let text = fs::read_to_string(file).expect("read js");
        for name in ["call(", "tryCall("] {
            for (at, _) in text.match_indices(name) {
                let boundary =
                    text[..at].chars().next_back().is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
                if !boundary {
                    continue;
                }
                if let Some(pair) = literal_pair(&text[at + name.len()..]) {
                    out.insert(pair);
                }
            }
        }
    }
    out
}

/// `"area", "verb"` at the start of `rest` (whitespace allowed), as `area/verb`.
fn literal_pair(rest: &str) -> Option<String> {
    let rest = rest.trim_start().strip_prefix('"')?;
    let (area, rest) = rest.split_once('"')?;
    let rest = rest.trim_start().strip_prefix(',')?.trim_start().strip_prefix('"')?;
    let (verb, _) = rest.split_once('"')?;
    let word = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c == '-');
    (word(area) && word(verb)).then(|| format!("{area}/{verb}"))
}

#[test]
fn the_table_and_the_scanner_see_the_whole_contract() {
    let (doc, calls) = (documented(), used());
    assert!(doc.len() >= 85, "only {} documented commands: the table parser is broken", doc.len());
    assert!(calls.len() >= 60, "only {} commands found in the front end: the scanner is broken", calls.len());
    for known in ["status/get", "fan/set", "keyboard/set-cells", "power/smart", "gui/ping"] {
        assert!(doc.contains(known), "{known}");
    }
    assert_eq!(literal_pair("\"fan\",\n  \"set\", {cpu}"), Some("fan/set".to_owned()));
    assert_eq!(literal_pair("name, \"set\""), None);
}

/// Documented commands the server answers itself or that the page never sends. The favourites
/// belong to the Qt app and the PREDATOR key; the web page has no star buttons.
const SERVER_ONLY: [&str; 7] = [
    "gui/ping",
    "gui/quit",
    "favourite/list",
    "favourite/toggle",
    "favourite/remove",
    "favourite/move",
    "favourite/apply",
];

/// Commands of the native apps: the web page is being retired and never calls them, but they are
/// implemented and still checked by `every_documented_command_exists_in_the_implementation`.
const NATIVE_APP_ONLY: [&str; 15] = [
    "alerts/get",
    "alerts/set",
    "battery/info",
    "capabilities/get",
    "mode/bindings",
    "mode/bind",
    "mode/unbind",
    "mode/apply",
    "theme/list",
    "theme/show",
    "theme/apply",
    "theme/stop",
    "theme/save",
    "theme/delete",
    "theme/preview",
];

/// Documented commands that only the native (Qt) app calls, so the web front end never does. They
/// must still be documented and implemented (the tests below check both); this list keeps the
/// "every documented command is called by the page" rule for everything else.
const NATIVE_ONLY: [&str; 8] = [
    "monitor/overview",
    "monitor/cpu",
    "monitor/gpu",
    "monitor/memory",
    "monitor/network",
    "monitor/disks",
    "monitor/reset",
    "system/info",
];

#[test]
fn every_command_the_page_calls_is_documented_and_every_documented_one_is_called() {
    let (doc, calls) = (documented(), used());
    for c in &calls {
        assert!(doc.contains(c), "the front end calls {c}, which COMMANDS.md does not document");
    }
    for c in &doc {
        assert!(
            calls.contains(c)
                || SERVER_ONLY.contains(&c.as_str())
                || NATIVE_APP_ONLY.contains(&c.as_str())
                || NATIVE_ONLY.contains(&c.as_str()),
            "COMMANDS.md documents {c} but the front end never calls it: the table must stay minimal"
        );
    }
    for c in NATIVE_ONLY {
        assert!(doc.contains(c), "{c} is exempt as native-only but COMMANDS.md does not document it");
        assert!(!calls.contains(c), "the web front end calls {c}: take it off the native-only list");
    }
}

#[test]
fn every_documented_command_exists_in_the_implementation() {
    let machine = Machine::start();
    for c in documented() {
        if SERVER_ONLY.contains(&c.as_str()) {
            continue;
        }
        let (area, verb) = c.split_once('/').expect("area/verb");
        let (status, body) = machine.run(area, verb, &json!({}));
        assert_ne!(status, 404, "{c} is documented but not implemented: {body}");
        assert!(body["ok"].is_boolean(), "{c}: {body}");
    }
}

#[test]
fn the_embedded_front_end_is_exactly_the_web_directory() {
    let mut files = Vec::new();
    files_below(&web_dir(), &mut files);
    let on_disk: BTreeSet<String> = files
        .iter()
        .map(|f| f.strip_prefix(web_dir()).expect("below web").to_string_lossy().into_owned())
        .collect();
    let embedded: BTreeSet<String> = FILES.iter().map(|(p, _)| (*p).to_owned()).collect();
    assert_eq!(
        embedded, on_disk,
        "a file of web/ is missing from the table in assets.rs (or the other way round)"
    );
    for (path, bytes) in FILES {
        assert_eq!(fs::read(web_dir().join(path)).expect("read"), *bytes, "{path} differs from the file");
    }
}

#[test]
fn the_embedded_front_end_is_the_go_editions_unchanged_in_content_type_and_entry_point() {
    let paths: BTreeSet<&str> = FILES.iter().map(|(p, _)| *p).collect();
    for needed in
        ["index.html", "app.css", "logo.svg", "js/app.js", "js/api.js", "js/sse.js", "js/pages/home.js"]
    {
        assert!(paths.contains(needed), "{needed}");
    }
    let api =
        FILES.iter().find(|(p, _)| *p == "js/api.js").map(|(_, b)| String::from_utf8_lossy(b).into_owned());
    let api = api.expect("api.js");
    // The three endpoints the server must provide for this page.
    for endpoint in ["/api/gui/session", "/api/stream", "/api/${area}/${verb}"] {
        assert!(api.contains(endpoint), "{endpoint}");
    }
    assert!(api.contains("Authorization") && api.contains("event === \"show\""));
}
