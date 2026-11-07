//! Discovers the view-model modules in `src/vm/` so that a page agent adds one file and edits no
//! shared one: every `src/vm/<name>.rs` (except `mod.rs`) becomes `pub mod <name>` and an entry of
//! `vm::dispatch`. The generated code is `$OUT_DIR/vm_gen.rs`, included by `src/vm/mod.rs`.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::{env, fs};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let dir = manifest.join("src/vm");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut names: Vec<String> = fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let stem = name.strip_suffix(".rs")?;
            (stem != "mod" && !stem.is_empty() && stem.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
                .then(|| stem.to_owned())
        })
        .collect();
    names.sort();
    let mut code = String::new();
    for n in &names {
        let path = dir.join(format!("{n}.rs"));
        let _ = writeln!(code, "#[path = {:?}]\npub mod {n};", path.display().to_string());
    }
    let list: Vec<String> = names.iter().map(|n| format!("{n:?}")).collect();
    let _ = writeln!(
        code,
        "\n/// The names of the view-model modules, for tests and diagnostics.\npub const MODULES: &[&str] = &[{}];",
        list.join(", ")
    );
    code.push_str(
        "\n/// Calls `verb` of the view-model module `module` with the JSON `args`.\n///\n/// # Errors\n/// The text to show when the module or the verb does not exist, or the module's own error.\n\
         pub fn dispatch(module: &str, verb: &str, args: &serde_json::Value) -> Result<serde_json::Value, String> {\n    match module {\n",
    );
    for n in &names {
        let _ = writeln!(code, "        {n:?} => {n}::call(verb, args),");
    }
    code.push_str("        _ => Err(format!(\"unknown view-model module: {module}\")),\n    }\n}\n");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap_or_default()).join("vm_gen.rs");
    if let Err(e) = fs::write(&out, code) {
        println!("cargo:warning=cannot write {}: {e}", out.display());
        std::process::exit(1);
    }
}
