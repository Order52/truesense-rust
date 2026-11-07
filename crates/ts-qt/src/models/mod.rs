//! Page-specific Qt objects. A page agent that needs its own `QObject` (a model for a long list, a
//! worker) adds `src/models/<page>.rs` with a `#[cxx_qt::bridge]`; `build.rs` finds the file, builds
//! the bridge and lists the module in `models_gen.rs`, so no shared file is edited. Most pages need
//! none: they use `Api`, `Live` and `Bus.vm` (see crates/ts-qt/README or the page contract).
//!
//! Every bridge registers its types in the QML module `org.truesense.app`.

include!(concat!(env!("OUT_DIR"), "/models_gen.rs"));
