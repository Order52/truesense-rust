//! cli: the `truesense` command line.
//!
//! A command here only parses its words, calls one operation of [`ts_api::api::Api`] and prints
//! the answer, as text for a person or as JSON (`--json`) for a script. What an operation does,
//! and which safety rule applies, is decided in the application layer, so the terminal UI and the
//! window cannot disagree with it.
//!
//! # Compatibility
//!
//! The commands, options, help pages, error messages and exit codes are those of the Python
//! edition (`cli.py`); its `argparse` behaviour is reproduced in `argparse` (abbreviated long
//! options, `argument mode: invalid choice: ...`, exit code 2 for a wrong command line, the way a
//! long usage line wraps). The `--json` field names are the Python edition's too. The shell
//! completions are generated from the same command tree.
//!
//! # Running it
//!
//! [`run`] takes the words after the program name, the application layer, a [`Launcher`] (the
//! commands that start other programs or take over the terminal) and the standard streams as
//! [`Io`], and returns the exit code. Tests drive it with `Services::simulated` and in-memory
//! buffers.
#![forbid(unsafe_code)]

mod alerts;
mod argparse;
mod completion;
mod handlers;
mod launcher;
mod modebind;
mod out;
mod tree;

#[cfg(test)]
mod tests;

use ts_api::api::Api;
use ts_foundation::buildinfo;

use argparse::help::format_help;
use argparse::parse::{Sinks, Stop, parse};

pub use launcher::Launcher;
pub use out::Io;
pub use tree::{GUI_PAGES, TUI_PAGES};

/// The name the usage lines carry.
const PROGRAM: &str = "truesense";

/// Runs one command line: `args` are the words after the program name. Returns the exit code
/// (0 done, 1 the command failed, 2 the command line was wrong, 130 interrupted).
///
/// `--json` works anywhere on the line, as it did in the Python edition.
pub fn run(args: &[String], api: &Api, launcher: &dyn Launcher, io: &mut Io<'_>) -> i32 {
    let json = args.iter().any(|a| a == "--json");
    let words: Vec<String> = args.iter().filter(|a| *a != "--json").cloned().collect();
    if matches!(words.first().map(String::as_str), Some("-V" | "--version")) {
        let _ = writeln!(io.out, "{PROGRAM} {}", buildinfo::VERSION);
        return 0;
    }
    let root = tree::build(buildinfo::VERSION);
    let parsed = {
        let mut sinks = Sinks { out: &mut *io.out, err: &mut *io.err, columns: io.columns };
        parse(&root, PROGRAM, &words, &mut sinks)
    };
    let matches = match parsed {
        Ok(m) => m,
        Err(Stop::Exit(code)) => return code,
    };
    if matches.path.is_empty() {
        let _ = io.out.write_all(format_help(&root, PROGRAM, io.columns).as_bytes());
        return 0;
    }
    handlers::dispatch(api, launcher, io, json, &matches)
}

/// The usage line of every command, one `truesense area verb` per line: the command table the
/// documentation and the packaging read.
pub fn command_table() -> Vec<String> {
    let root = tree::build(buildinfo::VERSION);
    let mut rows = Vec::new();
    if let Some(subs) = &root.subs {
        for area in &subs.commands {
            match &area.subs {
                Some(verbs) => {
                    rows.extend(verbs.commands.iter().map(|v| format!("{PROGRAM} {} {}", area.name, v.name)));
                }
                None => rows.push(format!("{PROGRAM} {}", area.name)),
            }
        }
    }
    rows
}
