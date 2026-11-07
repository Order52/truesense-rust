//! The help pages against the reference output captured from the Python edition (`-h` of every
//! area and verb, at 80 columns), and the way a usage line wraps.

use super::{Fx, Ran};

const SPEC: &str = include_str!("../testdata/help-spec.txt");

/// One captured page: what was asked and the text.
struct Page {
    words: Vec<String>,
    text: String,
}

/// Splits the reference file into its pages. Pages of a verb of an area that has no verbs are
/// the area's own page printed again (an artefact of how the file was made) and are skipped.
fn pages() -> Vec<Page> {
    let mut pages = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut text: Vec<&str> = Vec::new();
    let mut skip = false;
    let flush = |pages: &mut Vec<Page>, words: &Vec<String>, text: &Vec<&str>, skip: bool| {
        if !skip {
            pages.push(Page { words: words.clone(), text: text.join("\n").trim_end().to_owned() });
        }
    };
    let mut started = false;
    let mut area = String::new();
    for line in SPEC.lines() {
        if line.starts_with("=====") || line.starts_with("-----") {
            if started {
                flush(&mut pages, &words, &text, skip);
            }
            started = true;
            text.clear();
            words.clear();
            skip = false;
            continue;
        }
        if started && text.is_empty() && words.is_empty() {
            if let Some(a) = line.strip_prefix("AREA ") {
                area = a.to_owned();
                words = vec![area.clone()];
                continue;
            }
            if let Some(v) = line.strip_prefix("VERB ") {
                words = v.split(' ').map(str::to_owned).collect();
                skip = matches!(area.as_str(), "touchpad" | "completion" | "gui" | "tui");
                continue;
            }
        }
        text.push(line);
    }
    if started {
        flush(&mut pages, &words, &text, skip);
    }
    pages
}

#[test]
fn the_program_page_is_the_reference() {
    let fx = Fx::offline();
    let ran = fx.run("--help");
    let first_block: Vec<&str> = SPEC.lines().take_while(|l| !l.starts_with("=====")).collect();
    assert_eq!(ran.out.trim_end(), first_block.join("\n").trim_end());
    assert_eq!(ran.code, 0);
}

#[test]
fn every_area_and_verb_page_is_the_reference() {
    let fx = Fx::offline();
    let all = pages();
    assert!(all.len() > 100, "found {} pages", all.len());
    let mut wrong = Vec::new();
    for page in &all {
        let mut args = page.words.clone();
        args.push("-h".to_owned());
        let Ran { code, out, err } = fx.run_args(&args, "");
        if code != 0 || out.trim_end() != page.text || !err.is_empty() {
            wrong.push(format!(
                "--- truesense {} -h\nwant:\n{}\ngot:\n{}\nerr: {err}",
                page.words.join(" "),
                page.text,
                out.trim_end()
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} pages differ:\n{}",
        wrong.len(),
        wrong.iter().take(3).cloned().collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn no_arguments_prints_the_program_page() {
    let fx = Fx::offline();
    let ran = fx.run("");
    assert_eq!(ran.code, 0);
    assert!(ran.out.starts_with("usage: truesense [-h] [--json] [-V] area ..."));
}

#[test]
fn the_help_width_follows_the_terminal() {
    let fx = Fx::offline();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut stdin = std::io::Cursor::new(Vec::new());
    let mut io = crate::cli::Io::new(&mut out, &mut err, &mut stdin);
    io.columns = 120;
    let code = crate::cli::run(&["fan".to_owned(), "-h".to_owned()], &fx.api, &fx.launcher, &mut io);
    assert_eq!(code, 0);
    let text = String::from_utf8_lossy(&out).into_owned();
    // The descriptions are laid out for 118 columns, not 78.
    assert!(text.lines().any(|l| l.chars().count() > 90), "{text}");
}
