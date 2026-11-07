//! truesense: the one program behind every command (the daemon, the command line, the terminal
//! UI, the window, the key service and the `truesense-fast` status-bar client).
//!
//! `main` only hands the arguments to [`app::run`] and exits with its code; everything that
//! decides lives in the library crates.
#![deny(unsafe_code)]

mod app;
mod hooks;
mod launcher;
mod signals;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    std::process::exit(app::run(&args));
}
