//! A configuration that is wrong must not stop the fan control and must not lock the desktop
//! user out of the daemon (evidence for `docs/RESILIENCE.md`, C1 to C4).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use ts_foundation::config::Config;

struct Dir(PathBuf);
impl Dir {
    fn new() -> Dir {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "tsf-cfg-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&p).unwrap();
        Dir(p)
    }
    fn put(&self, name: &str, text: &str) -> PathBuf {
        let p = self.0.join(name);
        fs::write(&p, text).unwrap();
        p
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn no_env(_: &str) -> Option<String> {
    None
}

#[test]
fn a_good_file_loads_as_it_is() {
    let d = Dir::new();
    let sys = d.put("config.toml", "user = \"alice\"\n[keys]\nprofile = \"native\"\n");
    let (cfg, problems) = Config::load_or_defaults(Some(&sys), None, &no_env);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(cfg.user, "alice");
}

#[test]
fn a_value_out_of_range_falls_back_to_the_defaults_but_keeps_the_user() {
    let d = Dir::new();
    let sys = d.put(
        "config.toml",
        "user = \"alice\"\n[fan]\nlevels = [10, 40]\ncpu_up = [50, 60]\ngpu_up = [50, 60]\n",
    );
    let (cfg, problems) = Config::load_or_defaults(Some(&sys), None, &no_env);
    assert!(problems.iter().any(|p| p.contains("[fan] levels must be 20..100")), "{problems:?}");
    assert_eq!(cfg.fan, Config::default().fan, "the defaults are in force");
    assert_eq!(cfg.user, "alice", "the desktop user can still use the daemon");
}

#[test]
fn a_file_that_does_not_parse_keeps_the_user_read_leniently() {
    let d = Dir::new();
    for (name, text, user) in [
        ("broken-after", "user = \"alice\"\n[keys\ntap = \n", "alice"),
        ("comment", "user = \"bob\" # the login\n[fan\n", "bob"),
        ("not-in-a-section", "[fan\nuser = \"mallory\"\n", ""),
        ("hostile-name", "user = \"a b; rm\"\n[x\n", ""),
        ("empty-name", "user = \"\"\n[x\n", ""),
        ("binary", "\u{0}\u{1}\u{2}", ""),
    ] {
        let sys = d.put(name, text);
        let (cfg, problems) = Config::load_or_defaults(Some(&sys), None, &no_env);
        assert!(!problems.is_empty(), "{name}");
        assert_eq!(cfg.user, user, "{name}");
        assert!(cfg.validate().is_empty(), "{name}: the fallback itself is valid");
    }
}

#[test]
fn a_wrong_type_in_the_users_file_is_blamed_on_that_file() {
    let d = Dir::new();
    let sys = d.put("sys.toml", "user = \"alice\"\n");
    let usr = d.put("usr.toml", "[daemon]\nsmart_fan = \"yes\"\n");
    let (cfg, problems) = Config::load_or_defaults(Some(&sys), Some(&usr), &no_env);
    assert!(problems[0].contains("usr.toml"), "{problems:?}");
    assert_eq!(cfg.user, "alice");
    assert!(!cfg.daemon.smart_fan);
}

#[test]
fn without_any_user_the_environment_decides_as_before() {
    let d = Dir::new();
    let sys = d.put("sys.toml", "[fan\n");
    let env = |k: &str| (k == "SUDO_USER").then(|| "carol".to_owned());
    assert_eq!(Config::load_or_defaults(Some(&sys), None, &env).0.user, "carol");
    assert_eq!(Config::load_or_defaults(Some(&d.0.join("absent")), None, &env).0.user, "carol");
}

#[test]
fn a_missing_file_is_not_a_problem() {
    let d = Dir::new();
    let (cfg, problems) = Config::load_or_defaults(Some(&d.0.join("absent.toml")), None, &no_env);
    assert!(problems.is_empty());
    assert_eq!(cfg, Config::default());
}
