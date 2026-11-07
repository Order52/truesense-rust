//! `truesense-qt`: the TrueSense window.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(ts_qt::run(&args));
}
