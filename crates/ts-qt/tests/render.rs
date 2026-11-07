//! The rules that keep the window cheap while it sits idle, pinned as file scans (no Qt):
//! nothing is drawn by script, nothing animates by itself, and the scene is opaque.
//!
//! Background (see docs/QT-APP.md, "Rendering"): the window was repainting about 50 times a second on
//! an idle page because the gauges eased between two live readings (a 420 ms animation every second,
//! at the display's refresh rate) and every change redrew a script-drawn Canvas. These tests make a
//! return of either fail the build.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("qml")
}

fn qml_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let p = entry.path();
        if p.is_dir() {
            qml_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "qml") {
            out.push(p);
        }
    }
}

/// (relative path, line number, code line) of every non-comment line of every QML file.
fn code_lines() -> Vec<(String, usize, String)> {
    let mut files = Vec::new();
    qml_files(&root(), &mut files);
    let mut out = Vec::new();
    for f in files {
        let rel = f.strip_prefix(root()).unwrap().to_string_lossy().into_owned();
        for (i, line) in fs::read_to_string(&f).unwrap().lines().enumerate() {
            if !line.trim_start().starts_with("//") {
                out.push((rel.clone(), i + 1, line.to_owned()));
            }
        }
    }
    out
}

fn offenders(pattern: &str, allowed: &[&str]) -> Vec<String> {
    code_lines()
        .into_iter()
        .filter(|(file, _, line)| line.contains(pattern) && !allowed.contains(&file.as_str()))
        .map(|(file, n, line)| format!("{file}:{n}: {}", line.trim()))
        .collect()
}

#[test]
fn nothing_is_drawn_by_a_script_canvas() {
    // A Canvas re-runs JavaScript, fills an image and uploads it as a texture for every change; the
    // charts and gauges are QtQuick.Shapes (scene-graph geometry) instead.
    let found = offenders("Canvas {", &[]);
    assert!(found.is_empty(), "a Canvas is back:\n{}", found.join("\n"));
    let found = offenders("getContext(", &[]);
    assert!(found.is_empty(), "script drawing is back:\n{}", found.join("\n"));
}

#[test]
fn only_the_long_running_bar_animates_forever() {
    // An infinite animation keeps the window rendering at the display's refresh rate for as long as
    // it runs: only LongRunning has one, and it runs only while a job runs.
    let found = offenders("Animation.Infinite", &["components/LongRunning.qml"]);
    assert!(found.is_empty(), "an endless animation:\n{}", found.join("\n"));
    let found = offenders("loops:", &["components/LongRunning.qml"]);
    assert!(found.is_empty(), "a looping animation:\n{}", found.join("\n"));
}

#[test]
fn live_numbers_do_not_ease() {
    // Gauges and bars get a new reading every second (the monitor's every half second): an eased
    // change is a few hundred ms of full-rate frames per reading. They snap; only things the person
    // touches (hover, press, a switch) may ease, and those are short.
    let gauge = fs::read_to_string(root().join("components/Gauge.qml")).unwrap();
    assert!(gauge.contains("enabled: root.easeMs > 0"), "the Gauge easing must be opt-in");
    let found = offenders("Behavior on width", &[]);
    assert!(found.is_empty(), "a bar that eases its width:\n{}", found.join("\n"));
}

#[test]
fn animations_are_short() {
    // Every duration is `Theme.animMs` (140 ms, 0 with reduced motion), a small multiple of it, or a
    // plain literal of at most 200 ms (the test hook and the long-running bar excepted).
    for (file, n, line) in code_lines() {
        if let Some(i) = line.find("duration:") {
            if file == "components/LongRunning.qml" {
                continue;
            }
            let value = line[i + 9..].trim_start();
            let ok = value.starts_with("Theme.animMs") || value.starts_with("Math.min(200,") || {
                let digits: String = value.chars().take_while(char::is_ascii_digit).collect();
                digits.parse::<u32>().is_ok_and(|ms| ms <= 200)
            };
            assert!(ok, "{file}:{n}: a long animation: {}", line.trim());
        }
    }
}

#[test]
fn the_window_is_opaque() {
    // A transparent window needs an alpha buffer and makes the compositor blend (and, with blur on,
    // blur) behind it on every frame.
    let main = fs::read_to_string(root().join("Main.qml")).unwrap();
    assert!(main.contains("color: Theme.bg"), "the window background must be an opaque Theme colour");
    let found = offenders("Qt.transparent", &[]);
    assert!(found.is_empty(), "a transparent colour at the window level:\n{}", found.join("\n"));
}
