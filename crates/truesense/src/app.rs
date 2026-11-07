//! The entry point behind `main`: picks what to run from the program name and the first
//! argument, and builds only what that needs, so a status-bar button (`truesense-fast`) or the
//! daemon never pays for the application layer a command line needs.

use std::io::{self, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ts_cli::cli::{self, Io};
use ts_cli::fast::Fast;
use ts_foundation::buildinfo;
use ts_ipc::client::Client;
use ts_platform::firmware::DeviceKind;
use ts_platform::hw::Hw;

use crate::launcher::SystemLauncher;
use crate::signals;

/// Exit code of a usage error (no arguments at all).
const EXIT_USAGE: i32 = 2;

/// Runs the program with `args` (`args[0]` is the program name) and returns the exit code.
pub fn run(args: &[String]) -> i32 {
    let Some(program) = args.first() else { return EXIT_USAGE };
    let rest = &args[1..];
    let name = Path::new(program).file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name == "truesense-fast" {
        return run_fast(rest);
    }
    match rest.first().map(String::as_str) {
        // Nothing to build for these.
        Some("-V" | "--version") => {
            println!("truesense {}", buildinfo::VERSION);
            0
        }
        Some("daemon") if rest.len() == 1 => run_daemon(),
        _ => run_cli(rest),
    }
}

/// The root service: it builds its own components (see `ts_daemon::assembly`).
pub fn run_daemon() -> i32 {
    match ts_daemon::assembly::run_daemon() {
        Ok(()) => 0,
        Err(e) => {
            let _ = writeln!(io::stderr(), "truesense daemon: {e}");
            1
        }
    }
}

/// The status-bar client: only the daemon socket and the firmware.
fn run_fast(args: &[String]) -> i32 {
    let hardware = Hw::new(DeviceKind::from_process_env().open());
    let daemon = Client::from_env();
    let sleep = |d: Duration| std::thread::sleep(d);
    Fast { daemon: &daemon, hardware: &hardware, sleep: &sleep }.run(args, &mut io::stderr())
}

/// The command line, over the application layer.
fn run_cli(args: &[String]) -> i32 {
    let api = match ts_api::adapt::system_api() {
        Ok(api) => Arc::new(api),
        Err(e) => {
            let _ = writeln!(io::stderr(), "error: {e}");
            return 1;
        }
    };
    let launcher = SystemLauncher::new(api.clone());
    // Not locked for the whole command: a held lock would block every other thread that prints
    // (the key service's log lines from its action and lighting threads, the audio watcher's),
    // for as long as the command runs. Each write locks for itself.
    let (mut out, mut err) = (io::stdout(), io::stderr());
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut io = Io::new(&mut out, &mut err, &mut input);
    io.columns = terminal_columns();
    io.colour = signals::is_terminal(1);
    cli::run(args, &api, &launcher, &mut io)
}

/// The width help text is wrapped to: `COLUMNS` when it is a sensible number, else 80.
fn terminal_columns() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|c| c.parse::<usize>().ok())
        .filter(|c| (20..=400).contains(c))
        .unwrap_or(80)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn no_arguments_at_all_is_a_usage_error() {
        assert_eq!(run(&[]), EXIT_USAGE);
    }

    #[test]
    fn the_version_needs_nothing_built() {
        assert_eq!(run(&args(&["truesense", "--version"])), 0);
        assert_eq!(run(&args(&["/usr/bin/truesense", "-V"])), 0);
    }

    #[test]
    fn columns_come_from_the_environment_when_sensible() {
        assert!((20..=400).contains(&terminal_columns()));
    }
}
