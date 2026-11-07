//! truesense: thin main; dispatch lives in the library crates.

fn main() {
    println!("truesense {}", env!("CARGO_PKG_VERSION"));
}
