//! Profiles: saved settings under a name, in two shapes.
//!
//! * [`store`]: the one-shape JSON store every kind of profile uses (fan curves, Smart auto,
//!   PREDATOR key bindings, CPU and GPU undervolt, lighting): read-only built-ins, always
//!   including `native`, and the user's own in `~/.config/truesense/<kind>/NAME.json`, with
//!   name rules, validation, listing, saving, deleting and copying.
//! * [`bundle`]: the whole-setup profiles (power, fans, lighting, battery, audio in one go), TOML
//!   files applied through the action runner and the facades.
#![forbid(unsafe_code)]

pub mod bundle;
pub mod store;

pub use bundle::{
    Bundle, BundleError, BundleFiles, BundleHost, BundleSource, BundleStore, default_sys_dirs, describe,
    parse as parse_bundle,
};
pub use store::{
    Kind, NAME_MAX, NATIVE, NameRule, ProfileError, ProfileStore, Source, Validator, default_user_dir,
    sanitise_name,
};

#[cfg(test)]
mod bundle_tests;

#[cfg(test)]
pub(crate) mod testutil {
    //! A temporary directory removed on drop, for the tests of this module and `keybinds`.
    #![allow(clippy::expect_used)]

    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A fresh directory under the system temp dir.
    pub(crate) struct Tmp(pub PathBuf);

    impl Tmp {
        pub(crate) fn new(tag: &str) -> Tmp {
            static N: AtomicU32 = AtomicU32::new(0);
            let p = std::env::temp_dir().join(format!(
                "ts-domain-{tag}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&p).expect("temp dir");
            Tmp(p)
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
