//! The parser: Python's `argparse` messages and exit codes.
use super::Fx;

#[test]
fn a_wrong_choice_is_exit_two_with_the_usage_and_the_choices() {
    let fx = Fx::offline();
    let ran = fx.run("power set foo");
    assert_eq!(ran.code, 2);
    assert_eq!(ran.out, "");
    assert_eq!(
        ran.err,
        "usage: truesense power set [-h] {quiet,balanced,performance,turbo,eco}\ntruesense power set: error: argument mode: invalid choice: 'foo' (choose from 'quiet', 'balanced', 'performance', 'turbo', 'eco')\n"
    );
}

#[test]
fn an_unknown_area_lists_every_area() {
    let fx = Fx::offline();
    let ran = fx.run("foo");
    assert_eq!(ran.code, 2);
    assert_eq!(
        ran.err,
        "usage: truesense [-h] [--json] [-V] area ...\ntruesense: error: argument area: invalid choice: 'foo' (choose from 'status', 'sensors', 'power', 'fan', 'battery', 'display', 'boot', 'gpu', 'rear', 'update', 'look', 'startup', 'keyboard', 'audio', 'keys', 'profile', 'cpu', 'touchpad', 'alerts', 'service', 'config', 'doctor', 'selftest', 'completion', 'gui', 'probe', 'tui', 'daemon', 'version')\n"
    );
}

#[test]
fn a_missing_word_is_reported_by_the_verb() {
    let fx = Fx::offline();
    let ran = fx.run("look save");
    assert_eq!(ran.code, 2);
    assert_eq!(
        ran.err,
        "usage: truesense look save [-h] name\ntruesense look save: error: the following arguments are required: name\n"
    );
    let ran = fx.run("keyboard paint");
    assert!(
        ran.err.ends_with("truesense keyboard paint: error: the following arguments are required: items\n")
    );
    let ran = fx.run("startup set keyboard");
    assert!(ran.err.ends_with("error: the following arguments are required: value\n"));
    let ran = fx.run("keyboard gradient ff0000");
    assert!(ran.err.ends_with("error: the following arguments are required: to_color\n"));
}

#[test]
fn a_bad_number_names_the_argument() {
    let fx = Fx::offline();
    let ran = fx.run("rear set breathing --speed x");
    assert_eq!(ran.code, 2);
    assert!(ran.err.ends_with("truesense rear set: error: argument --speed: invalid int value: 'x'\n"));
    let ran = fx.run("power hold abc");
    assert!(ran.err.ends_with("truesense power hold: error: argument minutes: invalid float value: 'abc'\n"));
    let ran = fx.run("rear set breathing --direction 5");
    assert!(ran.err.ends_with("argument --direction: invalid choice: '5' (choose from '0', '1')\n"));
}

#[test]
fn an_option_without_its_word_is_an_error() {
    let fx = Fx::offline();
    let ran = fx.run("rear set breathing --speed");
    assert_eq!(ran.code, 2);
    assert!(ran.err.ends_with("truesense rear set: error: argument --speed: expected one argument\n"));
}

#[test]
fn an_extra_word_is_unrecognized_and_shown_with_the_program_usage() {
    let fx = Fx::offline();
    let ran = fx.run("status now");
    assert_eq!(ran.code, 2);
    assert_eq!(
        ran.err,
        "usage: truesense [-h] [--json] [-V] area ...\ntruesense: error: unrecognized arguments: now\n"
    );
    let ran = fx.run("fan status --bogus");
    assert!(ran.err.ends_with("unrecognized arguments: --bogus\n"));
}

#[test]
fn long_options_may_be_abbreviated() {
    let fx = Fx::offline();
    // `--col` is the only option of `rear set` that starts so.
    let ran = fx.run("rear set static --col ff0000");
    assert_eq!(ran.code, 0, "{}", ran.err);
    // `--c` is both `--cpu` and `--couple` of `fan curve`.
    let ran = fx.run("fan curve --c 1:0");
    assert_eq!(ran.code, 2);
    assert!(ran.err.ends_with("error: ambiguous option: --c could match --cpu, --couple\n"));
}

#[test]
fn an_option_may_be_glued_to_its_word_with_an_equals_sign() {
    let fx = Fx::offline();
    let ran = fx.run("rear set static --color=00ff00");
    assert_eq!(ran.code, 0, "{}", ran.err);
    assert!(ran.out.contains("00ff00"), "{}", ran.out);
    let ran = fx.run("fan off --confirm=yes");
    assert_eq!(ran.code, 2);
    assert!(ran.err.ends_with("argument --confirm: ignored explicit argument 'yes'\n"));
}

#[test]
fn a_negative_number_is_a_word_not_an_option() {
    let fx = Fx::offline();
    let ran = fx.run("fan set -5");
    // Parsed as the duty -5 and refused by the safety rule, not by the parser.
    assert_eq!(ran.code, 1);
    assert_eq!(ran.err, "error: manual fan duty is 20..100 % per fan, or 0 to stop that fan\n");
    let ran = fx.run("cpu undervolt set -10 --plane 2");
    assert_ne!(ran.code, 2, "{}", ran.err);
}

#[test]
fn positionals_after_options_are_still_used() {
    let fx = Fx::offline();
    // The optional positionals must wait for `save x` that comes after the option.
    let ran = fx.run("fan curve --cpu 52:0,89:100 --gpu 48:0,78:100 save mine");
    assert_eq!(ran.code, 0, "{}", ran.err);
    assert!(ran.out.contains("saved fan curve 'mine'"));
}

#[test]
fn version_prints_the_program_and_version() {
    let fx = Fx::offline();
    let ran = fx.run("-V");
    assert_eq!(ran.out, format!("truesense {}\n", ts_foundation::buildinfo::VERSION));
    assert_eq!(ran.code, 0);
    let ran = fx.run("--version");
    assert_eq!(ran.out, format!("truesense {}\n", ts_foundation::buildinfo::VERSION));
    let ran = fx.run("--json version");
    assert_eq!(ran.out, format!("{{\n  \"truesense\": \"{}\"\n}}\n", ts_foundation::buildinfo::VERSION));
}

#[test]
fn json_works_anywhere_on_the_line() {
    let fx = Fx::offline();
    let before = fx.run("--json sensors");
    let middle = fx.run("sensors --json");
    assert_eq!(before.out, middle.out);
    assert!(before.out.starts_with("{\n  \"cpu_temp\""), "{}", before.out);
}

#[test]
fn a_hostile_command_line_never_panics() {
    let fx = Fx::offline();
    for line in [
        "-",
        "--",
        "---",
        "- -",
        "-h",
        "-hV",
        "-Vh",
        "--he",
        "--=",
        "--json=1",
        "power --",
        "power set --",
        "power set -- turbo",
        "fan curve -- list",
        "keyboard paint --",
        "a b c d e f",
        "\u{0}",
        "power set \u{1f980}",
        "--x=y=z status",
        "fan set 5 6 7 8",
        "rear set static --zones",
        "cpu undervolt --plane",
        "keys profile --tap",
    ] {
        let ran = fx.run(line);
        assert!(ran.code <= 2 || ran.code == 130, "{line:?}: {}", ran.code);
    }
    let ran = fx.run_args(
        &["power".to_owned(), "set".to_owned(), String::from_utf8_lossy(&[0xff, 0xfe]).into_owned()],
        "",
    );
    assert_eq!(ran.code, 2);
}

#[test]
fn double_dash_ends_the_options() {
    let fx = Fx::offline();
    let ran = fx.run("power set -- turbo");
    assert_eq!(ran.code, 0, "{}", ran.err);
    assert!(ran.out.contains("power mode: turbo"));
}

/// Every command line of up to four words from a hostile vocabulary goes through the parser alone
/// (no command runs): it ends with a result or an exit code and never panics.
#[test]
fn the_parser_survives_every_short_line_of_hostile_words() {
    use crate::cli::argparse::parse::{Sinks, parse};

    let root = crate::cli::tree::build("test");
    let words =
        ["power", "set", "fan", "curve", "--cpu=1", "-5", "--", "-hV", "--c", "turbo", "", "\u{1f980}"];
    let mut count = 0usize;
    let mut line: Vec<String> = Vec::new();
    fn walk(
        root: &crate::cli::argparse::Command,
        words: &[&str],
        line: &mut Vec<String>,
        depth: usize,
        count: &mut usize,
    ) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut sinks = Sinks { out: &mut out, err: &mut err, columns: 80 };
        let _ = parse(root, "truesense", line, &mut sinks);
        *count += 1;
        if depth == 4 {
            return;
        }
        for w in words {
            line.push((*w).to_owned());
            walk(root, words, line, depth + 1, count);
            line.pop();
        }
    }
    walk(&root, &words, &mut line, 0, &mut count);
    assert!(count > 20_000, "{count}");
}

#[test]
fn the_command_table_lists_every_area_and_verb() {
    let table = crate::cli::command_table();
    for want in [
        "truesense status",
        "truesense power set",
        "truesense fan curve",
        "truesense keyboard paint",
        "truesense audio dts",
        "truesense keys profile",
        "truesense cpu undervolt",
        "truesense service repair",
        "truesense touchpad",
        "truesense version",
        "truesense update auto-run",
    ] {
        assert!(table.iter().any(|row| row == want), "missing {want}");
    }
    assert!(table.len() > 100, "{}", table.len());
}
