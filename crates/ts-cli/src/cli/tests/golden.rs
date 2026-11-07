//! Transcripts captured from the Python edition (simulator, no daemon, a temporary home): the
//! same commands run against the simulated services must print the same text and the same JSON.
//!
//! The fixtures `testdata/golden-N.txt` were made from the command lists `testdata/cases-N.txt`
//! with `TRUESENSE_SIM=1 TRUESENSE_SOCKET=/nonexistent/x TRUESENSE_USERSTATE=<temp>/user.json
//! HOME=<temp>/home PATH=/nonexistent python3 -m truesense <line>` (also the daemon's own file
//! variables pointed into the temp directory), one process per line, in order. The key order of
//! JSON objects and `52` against `52.0` are not compared; everything else is.
//!
//! Every command of a transcript is a new process in the original, so the firmware simulator starts
//! over for each of them; only the files (user state, configuration, saved profiles) carry over.
//! Here that is the same directory with fresh services for every command.

use ts_api::api::{Api, Services};

use super::{FakeLauncher, Ran, TempDir};
use crate::cli::{Io, run};

/// One recorded command.
struct Case {
    line: String,
    code: i32,
    out: String,
    err: String,
}

fn parse(text: &str) -> Vec<Case> {
    let mut cases = Vec::new();
    for block in text.split("=== end\n").filter(|b| !b.trim().is_empty()) {
        let (head, rest) = block.split_once("\n--- out\n").expect("a transcript block has an out part");
        let (out, err) = rest.split_once("--- err\n").expect("a transcript block has an err part");
        let mut head = head.lines();
        let line = head.next().and_then(|l| l.strip_prefix("$ ")).expect("a command line").to_owned();
        let code = head
            .next()
            .and_then(|l| l.strip_prefix("rc "))
            .and_then(|c| c.parse().ok())
            .expect("an exit code");
        cases.push(Case { line, code, out: out.to_owned(), err: err.to_owned() });
    }
    cases
}

/// Runs one command line over fresh simulated services in `dir`, with no daemon.
fn run_offline(dir: &TempDir, line: &str, input: &str) -> Ran {
    let (services, rig) = Services::simulated(&dir.0).expect("services");
    rig.daemon.set_available(false);
    let api = Api::new(services);
    let launcher = FakeLauncher::default();
    let args: Vec<String> = shlex(line);
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut stdin = std::io::Cursor::new(input.as_bytes().to_vec());
    let code = {
        let mut io = Io::new(&mut out, &mut err, &mut stdin);
        run(&args, &api, &launcher, &mut io)
    };
    let tmp = dir.0.display().to_string();
    Ran {
        code,
        out: String::from_utf8_lossy(&out).replace(&tmp, "<TMP>"),
        err: String::from_utf8_lossy(&err).replace(&tmp, "<TMP>"),
    }
}

/// Splits a line the way a shell does for the simple quoting the transcripts use.
pub(super) fn shlex(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for c in line.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started || !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if started || !cur.is_empty() {
        words.push(cur);
    }
    words
}

/// A path under the temporary directory differs between the editions (their directory layouts are
/// not the same), so only the fact that there is one is compared.
fn mask_paths(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("<TMP>") {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let end = tail.find(|c: char| c.is_whitespace() || matches!(c, '\'' | '"')).unwrap_or(tail.len());
        out.push_str("<PATH>");
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// JSON values equal when their numbers are equal as numbers (`52` and `52.0`) and objects as sets of keys.
fn same_json(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    use serde_json::Value::{Array, Number, Object};
    match (a, b) {
        (Number(x), Number(y)) => x.as_f64() == y.as_f64(),
        (Array(x), Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same_json(p, q)),
        (Object(x), Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same_json(v, w)))
        }
        _ => a == b,
    }
}

/// Readings of the machine this ran on (the Python edition looked at the real touchpad).
fn drop_machine_readings(value: &mut serde_json::Value) {
    if let serde_json::Value::Object(map) = value {
        map.remove("touchpad");
    }
}

fn same_output(want: &str, got: &str) -> bool {
    let (want, got) = (mask_paths(want), mask_paths(got));
    if want == got {
        return true;
    }
    match (serde_json::from_str::<serde_json::Value>(&want), serde_json::from_str::<serde_json::Value>(&got))
    {
        (Ok(mut a), Ok(mut b)) => {
            drop_machine_readings(&mut a);
            drop_machine_readings(&mut b);
            same_json(&a, &b)
        }
        _ => false,
    }
}

/// Replays a transcript; `skip` names the command lines whose output depends on things the
/// doubles do not model (the key-action runner, the sound system).
pub(super) fn replay(text: &str, skip: &dyn Fn(&str) -> bool) -> Vec<String> {
    let dir = TempDir::new();
    let mut differences = Vec::new();
    for case in parse(text) {
        let (input, line) = match case.line.strip_prefix('!') {
            Some(rest) => {
                rest.split_once('|').map(|(i, l)| (format!("{i}\n"), l.to_owned())).unwrap_or_default()
            }
            None => (String::new(), case.line.clone()),
        };
        let got = run_offline(&dir, &line, &input);
        if skip(&line) {
            continue;
        }
        if got.code != case.code || !same_output(&case.out, &got.out) || !same_output(&case.err, &got.err) {
            differences.push(format!(
                "$ truesense {line}\n  want rc {} out {:?} err {:?}\n  got  rc {} out {:?} err {:?}",
                case.code, case.out, case.err, got.code, got.out, got.err
            ));
        }
    }
    differences
}

/// Commands whose output depends on something the doubles do not model, with the reason; each of
/// them has a test of its own with its own double.
fn not_comparable(line: &str) -> bool {
    // The key-action runner answers `ran <action>` instead of the real message.
    line.starts_with("keys test") || line == "power next" || line == "power prev" ||
    // The reference read the real touchpad, RAPL files and sound plugins of the machine it ran on.
    line == "touchpad" || line == "touchpad on" || line.ends_with("cpu info") || line == "audio speaker-filter on" ||
    // No picture decoder in the simulated services.
    line.starts_with("keyboard image") ||
    // The reference's version and file layout are not this program's.
    line.ends_with("version") || line.ends_with("config path") ||
    // A scripted service manager and an in-memory set of profiles stand in for the real ones.
    line == "service check" || line.starts_with("profile ") ||
    // The reference program's version, a Python traceback where this program answers (a verb-less
    // `display`, `boot`, `service`: the original crashed), and programs it tried to launch.
    line.starts_with("--ver") || line == "-V status" || matches!(line, "display" | "boot" | "service" | "gui --pag keys" | "tui --page=fans") ||
    // The self-test of the reference prints Python dictionaries.
    line.starts_with("selftest")
}

#[test]
fn the_first_transcript_matches_the_python_edition() {
    let differences = replay(include_str!("../testdata/golden-1.txt"), &not_comparable);
    assert!(differences.is_empty(), "{} differences:\n{}", differences.len(), differences.join("\n"));
}

#[test]
fn the_second_transcript_matches_the_python_edition() {
    let differences = replay(include_str!("../testdata/golden-2.txt"), &not_comparable);
    assert!(differences.is_empty(), "{} differences:\n{}", differences.len(), differences.join("\n"));
}

#[test]
fn the_third_transcript_of_command_line_mistakes_matches_the_python_edition() {
    let differences = replay(include_str!("../testdata/golden-3.txt"), &not_comparable);
    assert!(differences.is_empty(), "{} differences:\n{}", differences.len(), differences.join("\n"));
}
