use ts_system::update::Info;

use super::Fx;
use crate::api::{Api, describe_apply, describe_update};

fn newer() -> Fx {
    let fx = Fx::new();
    *fx.rig.updates.status.lock().unwrap() =
        crate::api::testing::FakeUpdates::new("0.4.0", "0.5.1").status.lock().unwrap().clone();
    fx
}

#[test]
fn the_check_words_each_case() {
    let fx = newer();
    let st = fx.api.check_update(false);
    assert_eq!(describe_update(&st), "TrueSense 0.4.0: update 0.5.1 is available   (truesense update apply)");
    assert_eq!(fx.api.update_repo(), "example/truesense");
    let mut same = st.clone();
    same.available = false;
    same.latest = Some("0.4.0".into());
    assert_eq!(describe_update(&same), "TrueSense 0.4.0 is up to date (newest 0.4.0)");
    same.latest = None;
    same.error = Some("could not reach GitHub: offline".into());
    assert_eq!(describe_update(&same), "TrueSense 0.4.0 is up to date   [could not reach GitHub: offline]");
}

#[test]
fn the_plan_picks_the_newest_or_the_version_asked_for() {
    let fx = newer();
    let plan = fx.api.plan_update(None).unwrap();
    assert_eq!((plan.tag.as_str(), plan.needed), ("v0.5.1", true));
    let plan = fx.api.plan_update(Some("0.3.1")).unwrap();
    assert_eq!((plan.tag.as_str(), plan.needed), ("v0.3.1", true));
    assert_eq!(fx.api.plan_update(Some("v0.3.1")).unwrap().tag, "v0.3.1");
    fx.rig.updates.status.lock().unwrap().available = false;
    assert!(!fx.api.plan_update(None).unwrap().needed);
}

#[test]
fn a_failed_lookup_with_nothing_cached_is_an_error_and_with_a_cache_is_not() {
    let fx = newer();
    {
        let mut st = fx.rig.updates.status.lock().unwrap();
        st.error = Some("could not reach GitHub: offline".into());
        st.info = None;
        st.latest = None;
        st.available = false;
    }
    assert_eq!(fx.api.plan_update(None).unwrap_err().to_string(), "could not reach GitHub: offline");
    {
        let mut st = fx.rig.updates.status.lock().unwrap();
        st.info = Some(Info {
            version: "0.4.0".into(),
            tag: "v0.4.0".into(),
            url: String::new(),
            notes: String::new(),
        });
    }
    assert_eq!(fx.api.plan_update(None).unwrap().tag, "v0.4.0");
    {
        let mut st = fx.rig.updates.status.lock().unwrap();
        st.error = None;
        st.info = None;
    }
    assert!(fx.api.plan_update(None).is_err(), "nothing known to install");
}

#[test]
fn applying_returns_the_result_even_when_the_installer_failed() {
    let fx = newer();
    let ok = fx.api.apply_update(Some("v0.5.1")).unwrap();
    assert_eq!(ok.returncode, 0);
    assert_eq!(
        describe_apply(&ok),
        "updated to 0.5.1 (download sha256 0123456789abcdef\u{2026}, checksum matches the release); restart the window and the key service if they are open"
    );
    assert_eq!(fx.rig.updates.applied.lock().unwrap()[0].tag.as_deref(), Some("v0.5.1"));
    *fx.rig.updates.returncode.lock().unwrap() = 3;
    let failed = fx.api.apply_update(None).unwrap();
    assert_eq!(
        describe_apply(&failed),
        "the installer exited with code 3 (see /var/log/truesense-install.log)"
    );
    let mut old = ok;
    old.verified = None;
    assert!(describe_apply(&old).contains("this release predates published checksums"));
}

#[test]
fn the_daily_check_is_a_switch_in_the_users_configuration() {
    let fx = Fx::new();
    assert!(!fx.api.auto_update_check().unwrap());
    assert!(fx.api.set_auto_update_check(true).unwrap());
    assert!(fx.api.auto_update_check().unwrap());
    assert!(fx.user_config().contains("auto_check = true"));
    assert!(!fx.api.set_auto_update_check(false).unwrap());
}

#[test]
fn a_new_release_is_announced_once() {
    let fx = newer();
    assert_eq!(fx.api.notify_new_update().unwrap(), None, "automatic checks are off");
    fx.api.set_auto_update_check(true).unwrap();
    assert_eq!(fx.api.notify_new_update().unwrap(), Some("0.5.1".to_owned()));
    let shown = fx.rig.notifier.shown();
    assert_eq!(shown.len(), 1);
    assert_eq!(
        shown[0].0,
        "TrueSense 0.5.1 is available (truesense update apply, or Settings in the window)"
    );
    assert_eq!(shown[0].1, "update");
    assert_eq!(fx.api.notify_new_update().unwrap(), None, "told once per version");
    assert_eq!(fx.rig.notifier.shown().len(), 1);
    fx.rig.updates.status.lock().unwrap().available = false;
    assert_eq!(fx.api.notify_new_update().unwrap(), None);
    let _ = Api::version();
}

#[test]
fn the_version_is_this_builds() {
    assert_eq!(Api::version(), ts_foundation::buildinfo::VERSION);
}
