//! End-to-end tests of the real program, offscreen.
//!
//! Qt allows one application object per process, so each test starts `truesense-qt` as a child
//! process: with `TRUESENSE_QT_TEST=1` the shell reports what it looks like after the first frame
//! (`REPORT {json}` on stdout) and quits. Everything runs against the firmware simulator
//! (`TRUESENSE_SIM=1`) in a temporary home, with the daemon socket pointing nowhere and no session
//! bus, so no real laptop, daemon, desktop, tray or user setting is touched.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;

/// A short private directory (unix socket paths are limited to about 100 bytes).
struct Home(PathBuf);

impl Home {
    fn new(ui_json: Option<&str>) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = PathBuf::from("/tmp").join(format!(
            "tsqt-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("run")).unwrap();
        std::fs::create_dir_all(dir.join(".config/truesense")).unwrap();
        std::fs::set_permissions(dir.join("run"), std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .unwrap();
        if let Some(text) = ui_json {
            std::fs::write(dir.join(".config/truesense/ui.json"), text).unwrap();
        }
        Self(dir)
    }

    fn run_dir(&self) -> PathBuf {
        self.0.join("run")
    }

    fn command(&self, exit_after_ms: u32, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_truesense-qt"));
        c.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.0)
            .env("XDG_RUNTIME_DIR", self.run_dir())
            .env("TRUESENSE_SIM", "1")
            .env("TRUESENSE_SOCKET", "/nonexistent/truesense.sock")
            .env("TRUESENSE_USERSTATE", self.0.join("userstate.json"))
            .env("QT_QPA_PLATFORM", "offscreen")
            .env("QT_FORCE_STDERR_LOGGING", "1")
            .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent/bus")
            .env("TRUESENSE_QT_TEST", "1")
            .env("TRUESENSE_QT_EXIT_AFTER_MS", exit_after_ms.to_string())
            .args(args)
            .stdin(Stdio::null());
        c
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn report_of(out: &Output) -> Value {
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find_map(|l| l.strip_prefix("REPORT ")).unwrap_or_else(|| {
        panic!(
            "no REPORT line; status {:?}\nstdout: {text}\nstderr: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        )
    });
    serde_json::from_str(line).unwrap()
}

/// Starts the program, waits for its report and exit.
fn run_once(home: &Home, exit_after_ms: u32, args: &[&str]) -> Value {
    let out = home.command(exit_after_ms, args).output().unwrap();
    assert!(
        out.status.success(),
        "status {:?}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    report_of(&out)
}

#[test]
fn the_real_main_qml_loads_and_the_bus_round_trip_works() {
    let home = Home::new(None);
    let r = run_once(&home, 1800, &[]);
    assert_eq!(r["page"], "home");
    assert_eq!(r["page_qml"], "HomePage");
    assert_eq!(r["pages"], 11);
    assert_eq!(r["groups"], 5);
    assert_eq!(r["title_home"], "Home", "English catalogue is loaded: {r}");
    assert_eq!(r["language"], "en");
    assert_eq!(r["theme_dark"], true, "the default look is dark");
    assert_eq!(r["width"], 1000);
    assert_eq!(r["height"], 680);
    // A command through the real in-process bus, on a worker thread, answered through a signal.
    assert_eq!(r["version_result"]["truesense"], ts_foundation::buildinfo::VERSION, "{r}");
    // A refusal arrives verbatim with its code.
    assert_eq!(r["refusal"]["code"], "unknown-command");
    assert_eq!(r["refusal"]["message"], "unknown command: nope nothing");
    assert_eq!(r["pending"], 0);
    // Live values flowed from the simulator while the window was visible.
    assert_eq!(r["live_ok"], true);
    assert!(r["live_frames"].as_u64().unwrap() >= 1, "{r}");
    assert!(r["cpu_temp"].is_number(), "{r}");
    assert!(r["caps"].as_u64().unwrap() > 5, "capabilities were read: {r}");
    // The first frame came fast and the process is small (offscreen, debug build: loose bounds).
    assert!(r["first_frame_ms"].as_u64().unwrap() < 5000, "{r}");
    assert!(r["rss_kb"].as_u64().unwrap() < 400_000, "{r}");
}

#[test]
fn page_argument_selects_the_start_page_and_aliases_work() {
    let home = Home::new(None);
    assert_eq!(run_once(&home, 300, &["--page", "settings"])["page"], "settings");
    assert_eq!(run_once(&home, 300, &["--page", "curves"])["page"], "cooling");
    assert_eq!(run_once(&home, 300, &["--page", "no-such-page"])["page"], "home");
}

#[test]
fn every_page_loads_without_a_qml_error() {
    // A script error in a page is invisible offscreen but real on the desktop: a popup whose
    // `parent: Overlay.overlay` threw (a file without the QtQuick.Controls import) opened inside
    // the scrolled tab instead of over the window, and a binding loop rebuilt a gallery's pictures
    // over and over. Every page, in parallel, must load without one.
    const PAGES: &[&str] = &[
        "home",
        "monitor",
        "performance",
        "cooling",
        "lighting",
        "keys",
        "battery",
        "audio",
        "profiles",
        "health",
        "settings",
    ];
    let runs: Vec<_> = PAGES
        .iter()
        .map(|page| {
            let home = Home::new(Some(r#"{"onboarded": true}"#));
            let child = home
                .command(1500, &["--page", page])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            (page, home, child)
        })
        .collect();
    let mut problems = Vec::new();
    for (page, _home, child) in runs {
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{page}: {}", String::from_utf8_lossy(&out.stderr));
        for line in String::from_utf8_lossy(&out.stderr).lines() {
            let qml = line.contains(".qml:");
            if qml
                && ["Error", "Binding loop", "is not defined", "Cannot assign", "Unable to assign"]
                    .iter()
                    .any(|w| line.contains(w))
            {
                problems.push(format!("{page}: {line}"));
            }
        }
    }
    assert!(problems.is_empty(), "QML errors:\n{}", problems.join("\n"));
}

#[test]
fn window_size_and_light_theme_come_from_ui_json() {
    let home = Home::new(Some(r#"{"width": 700, "height": 560, "theme": "light"}"#));
    let r = run_once(&home, 300, &[]);
    assert_eq!(r["width"], 700);
    assert_eq!(r["height"], 560);
    assert_eq!(r["theme_dark"], false);
}

#[test]
fn the_software_renderer_is_the_default_and_a_gpu_one_falls_back_when_it_cannot_start() {
    let home = Home::new(None);
    assert_eq!(run_once(&home, 300, &[])["renderer"], "software");

    // Asked for in the settings file: used when a context can be made (a developer machine), else the
    // program must still start, on software, and say so. Offscreen has no GL, so either is fine here.
    let home = Home::new(Some(r#"{"renderer": "opengl"}"#));
    let r = run_once(&home, 300, &[]);
    assert!(matches!(r["renderer"].as_str(), Some("opengl" | "software-fallback")), "{r}");
    assert_eq!(r["live_ok"], true, "the window works on whichever backend it got");

    // The environment beats the file, and an unknown word is ignored.
    let mut c = home.command(300, &[]);
    c.env("TRUESENSE_QT_RENDER", "software");
    assert_eq!(report_of(&c.output().unwrap())["renderer"], "software");
    let mut c = home.command(300, &[]);
    c.env("TRUESENSE_QT_RENDER", "direct3d");
    let r = report_of(&c.output().unwrap());
    assert!(matches!(r["renderer"].as_str(), Some("opengl" | "software-fallback")), "{r}");

    // Qt's own switch wins over both.
    let mut c = home.command(300, &[]);
    c.env("QT_QUICK_BACKEND", "software");
    assert_eq!(report_of(&c.output().unwrap())["renderer"], "qt:software");
}

#[test]
fn the_smallest_window_is_small_enough_for_a_tile() {
    let home = Home::new(Some(r#"{"width": 320, "height": 260}"#));
    let r = run_once(&home, 300, &[]);
    assert_eq!((r["width"].as_u64(), r["height"].as_u64()), (Some(320), Some(260)));
    let home = Home::new(Some(r#"{"width": 100, "height": 100}"#));
    let r = run_once(&home, 300, &[]);
    assert_eq!((r["width"].as_u64(), r["height"].as_u64()), (Some(320), Some(260)), "raised to the minimum");
}

#[test]
fn a_narrow_window_still_loads_with_the_menu_layout() {
    let home = Home::new(Some(r#"{"width": 420, "height": 700}"#));
    let r = run_once(&home, 300, &[]);
    assert_eq!(r["width"], 420);
    assert_eq!(r["page"], "home");
}

/// A `ui.json` written when Arabic still existed (`"language": "ar"`), or by hand with any
/// language the app does not know, must load without an error and give English.
#[test]
fn a_settings_file_that_still_says_arabic_loads_in_english() {
    for text in [r#"{"language": "ar"}"#, r#"{"language": "fr", "theme": "light"}"#] {
        let home = Home::new(Some(text));
        let out = home.command(300, &[]).output().unwrap();
        assert!(out.status.success(), "{text}: {}", String::from_utf8_lossy(&out.stderr));
        let r = report_of(&out);
        assert_eq!(r["language"], "en", "{text}: {r}");
        assert_eq!(r["title_home"], "Home", "{text}: {r}");
        // Only a note in the log: the file is readable, one entry of it is not used.
        let log = String::from_utf8_lossy(&out.stderr);
        assert!(log.contains("ignored entry"), "{log}");
        assert!(!log.contains("cannot be read"), "{log}");
    }
}

#[test]
fn a_desktop_in_arabic_gets_english() {
    let home = Home::new(None);
    let mut cmd = home.command(300, &[]);
    cmd.env("LC_ALL", "ar_EG.UTF-8").env("LANG", "ar_EG.UTF-8").env("LANGUAGE", "ar");
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let r = report_of(&out);
    assert_eq!(r["language"], "en", "{r}");
    assert_eq!(r["title_home"], "Home");
}

#[test]
fn the_pseudo_locale_marks_every_translated_string() {
    let home = Home::new(Some(r#"{"language": "pseudo"}"#));
    let r = run_once(&home, 300, &[]);
    let title = r["title_home"].as_str().unwrap();
    assert!(title.starts_with('⟦') && title.ends_with('⟧'), "{title}");
}

#[test]
fn an_unreadable_settings_file_is_reported_and_left_alone() {
    let home = Home::new(Some("{ not json"));
    let out = home.command(300, &[]).output().unwrap();
    assert!(out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("ui.json"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(std::fs::read_to_string(home.0.join(".config/truesense/ui.json")).unwrap(), "{ not json");
}

#[test]
fn a_qml_file_that_does_not_load_ends_the_program_with_status_one() {
    let home = Home::new(None);
    let out = home
        .command(300, &[])
        .env("TRUESENSE_QT_QML_URL", "qrc:/qt/qml/org/truesense/app/qml/DoesNotExist.qml")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn command_line_errors_and_help() {
    let home = Home::new(None);
    let bad = home.command(0, &["--bogus"]).output().unwrap();
    assert_eq!(bad.status.code(), Some(2));
    let help = home.command(0, &["--help"]).output().unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("--page"));
    let version = home.command(0, &["--version"]).output().unwrap();
    assert!(String::from_utf8_lossy(&version.stdout).starts_with("truesense-qt "));
}

fn wait_for_instance(dir: &Path, child: &mut Child) {
    let start = Instant::now();
    while !ts_ui_model::single::ping(dir) {
        assert!(child.try_wait().unwrap().is_none(), "the first instance ended early");
        assert!(start.elapsed() < Duration::from_secs(20), "the first instance never listened");
    }
}

#[test]
fn a_second_start_shows_the_page_in_the_first_window_and_exits_at_once() {
    let home = Home::new(None);
    // The first window lives for a while, then reports which page it ended on.
    let mut first = home.command(3500, &[]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    wait_for_instance(&home.run_dir(), &mut first);

    let started = Instant::now();
    let second = home.command(0, &["--page", "keys"]).output().unwrap();
    assert!(second.status.success(), "{}", String::from_utf8_lossy(&second.stderr));
    assert!(second.stdout.is_empty(), "the second start must not open a window of its own");
    assert!(started.elapsed() < Duration::from_secs(5), "the second start must return quickly");

    let out = first.wait_with_output().unwrap();
    assert!(out.status.success());
    let r = report_of(&out);
    assert_eq!(r["page"], "keys", "the first window navigated: {r}");
}

#[test]
fn the_socket_is_removed_when_the_window_ends_and_a_stale_one_is_replaced() {
    let home = Home::new(None);
    // A stale socket file from a crashed instance.
    drop(
        std::os::unix::net::UnixListener::bind(home.run_dir().join(ts_ui_model::single::SOCKET_NAME))
            .unwrap(),
    );
    let r = run_once(&home, 300, &[]);
    assert_eq!(r["page"], "home");
    assert!(
        !home.run_dir().join(ts_ui_model::single::SOCKET_NAME).exists(),
        "the socket must be removed on exit"
    );
}

/// Runs the Lighting page with test-hook steps (`TRUESENSE_QT_SET`) and returns the probed values.
fn lighting_probe(set: &str, probe: &str, exit_after_ms: u32) -> Value {
    let home = Home::new(Some(r#"{"onboarded": true, "width": 1268, "height": 1551}"#));
    let out = home
        .command(exit_after_ms, &["--page", "lighting"])
        .env("TRUESENSE_QT_SET", set)
        .env("TRUESENSE_QT_PROBE", probe)
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    report_of(&out)["probe"].clone()
}

#[test]
fn applying_a_theme_keeps_the_page_where_it_was() {
    // The owner pressed Apply far down the gallery and the page jumped: the reload after Apply
    // handed the gallery a new list (all cards rebuilt, the grid collapsed for a frame) and moved
    // the running theme to the front. Now the scroll position must be exactly what it was.
    let set = r#"{"lightingPage": {"tab": "themes"},
                  "@2500": {"lightingPage": {"flick.contentY": 1500}},
                  "@3000": {"themesTab": {"applyCard()": ["hearth", 0]}}}"#;
    let probe = r#"["lightingPage.flick.contentY", "themesTab.active.name", "themeGallery.rebuilds"]"#;
    let after = lighting_probe(set, probe, 5000);
    assert_eq!(after["themesTab.active.name"], "hearth", "the theme was applied: {after}");
    assert_eq!(after["lightingPage.flick.contentY"], 1500.0, "the page did not move: {after}");
    let without = lighting_probe(
        r#"{"lightingPage": {"tab": "themes"}, "@2500": {"lightingPage": {"flick.contentY": 1500}}}"#,
        probe,
        5000,
    );
    assert_eq!(after["themeGallery.rebuilds"], without["themeGallery.rebuilds"], "Apply rebuilt the cards");
}

#[test]
fn an_effect_that_makes_its_own_colours_shows_no_colour_row() {
    for (effect, shown) in [("wave", false), ("neon", false), ("static", true), ("breathing", true)] {
        let set = format!(
            r#"{{"lightingPage": {{"tab": "keyboard"}}, "@1500": {{"keyboardTab": {{"kind": "effect", "effectId": "{effect}"}}}}}}"#
        );
        let p = lighting_probe(&set, r#"["kbColourRow.visible", "keyboardTab.effectId"]"#, 2500);
        assert_eq!(p["keyboardTab.effectId"], effect, "{p}");
        assert_eq!(p["kbColourRow.visible"], shown, "{effect}: {p}");
    }
}
