//! Golden snapshots: every page at several sizes, with the service and without, and the states a
//! page passes through (a change waiting, a question, the help, the activity list, a prompt, a
//! terminal that is too small). The text is what a person would see; a change of layout shows up
//! as a diff. Accept an intended change with `UPDATE_GOLDEN=1 cargo test -p ts-tui golden`.

use super::*;
use crate::tui::pages::PAGE_NAMES;

/// A fixed laptop: the same readings every run, so a page is the same text.
fn fixed(service: bool, w: u16, h: u16) -> Harness {
    let mut h = Harness::with(w, h, Setup { service, ..Setup::default() });
    h.rig.sim.set_sensors([55, 40, 50], [2400, 2100]);
    h.fast();
    h
}

fn check(name: &str, got: &str) {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/tui/tests/golden").join(format!("{name}.txt"));
    let got = format!("{got}\n");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(&path, &got).expect("write the snapshot");
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e} (run with UPDATE_GOLDEN=1 to create it)", path.display()));
    assert!(
        want == got,
        "{name} differs from its golden snapshot (UPDATE_GOLDEN=1 accepts):\n--- want\n{want}\n--- got\n{got}"
    );
}

#[test]
fn every_page_at_every_size_matches_its_snapshot() {
    for service in [true, false] {
        for (w, h) in [(120, 40), (100, 30), (80, 24), (60, 20), (44, 12), (40, 12)] {
            let mut screen = fixed(service, w, h);
            let mut all = String::new();
            for (i, name) in PAGE_NAMES.iter().enumerate() {
                screen.goto(i);
                all.push_str(&format!(
                    "===== {name} ({}, {w}x{h}) =====\n{}\n",
                    if service { "service" } else { "direct" },
                    screen.text()
                ));
            }
            check(&format!("pages_{}_{w}x{h}", if service { "service" } else { "direct" }), all.trim_end());
        }
    }
}

#[test]
fn the_states_a_page_passes_through_match_their_snapshots() {
    let mut out = String::new();
    let mut section = |title: &str, text: String| out.push_str(&format!("===== {title} =====\n{text}\n"));

    let mut h = fixed(true, 100, 30);
    h.goto(1);
    h.keys("jlll");
    section("a change waiting (Fans, CPU fan slider)", h.text());

    let mut h = fixed(true, 100, 30);
    h.goto(1);
    h.keys("jhhhhhhhh");
    h.key(Key::Enter);
    section("the question before a fan stops", h.text());

    let mut h = fixed(true, 100, 30);
    h.keys("?");
    section("the help", h.text());

    let mut h = fixed(true, 100, 40);
    h.keys("l");
    h.key(Key::Enter);
    h.goto(2);
    h.sel("Boot sound", 0);
    h.keys(" ");
    h.goto(0);
    section("what you did (Dashboard, 100x40)", h.text());

    let mut h = fixed(true, 100, 30);
    h.goto(3);
    h.sel("Custom colour", 0);
    h.key(Key::Enter);
    section("the colour prompt", h.text());

    let mut h = fixed(true, 100, 30);
    h.goto(6);
    h.sel("Run the check", 0);
    h.key(Key::Enter);
    section("after the health check (System)", h.text());

    let h = &mut fixed(true, 30, 8);
    section("a terminal that is too small", h.text());

    let mut h = Harness::with(100, 30, Setup { service: true, unicode: false, ..Setup::default() });
    h.goto(3);
    section("plain ASCII (Lighting)", h.text());

    check("states_100x30", out.trim_end());
}
