use ts_ipc::client::ClientError;
use ts_platform::hw::HwError;

use crate::api::{ApiError, Refusal};

#[test]
fn a_lower_errors_text_is_shown_unchanged() {
    let e = ApiError::from(HwError::Invalid("power mode must be one of: quiet".into()));
    assert_eq!(e.to_string(), "power mode must be one of: quiet");
    let e = ApiError::from(ClientError::NotRunning);
    assert!(e.daemon_missing());
    assert_eq!(e.to_string(), "the TrueSense daemon is not running (sudo systemctl start truesense)");
    let e = ApiError::from(ts_decision::curves::CurveError::BadName);
    assert!(matches!(e, ApiError::Domain { area: "fan curve", .. }));
    assert_eq!(e.to_string(), ts_decision::curves::CurveError::BadName.to_string());
    assert!(std::error::Error::source(&e).is_some());
}

#[test]
fn a_refusal_is_told_apart_from_a_failure() {
    let e = ApiError::from(Refusal::StopNeedsService);
    assert_eq!(e.refusal(), Some(&Refusal::StopNeedsService));
    assert!(!e.daemon_missing());
    assert_eq!(ApiError::invalid("x").refusal(), None);
    assert!(std::error::Error::source(&e).is_none());
    assert_eq!(Refusal::FanDuty.to_string(), "manual fan duty is 20..100 % per fan, or 0 to stop that fan");
    assert_eq!(Refusal::SmartNeedsService.to_string(), "the smart curve needs the TrueSense daemon");
    assert!(ApiError::unavailable("no decoder").to_string().contains("no decoder"));
}

#[test]
fn a_daemon_that_is_too_old_says_how_to_update() {
    let e = ApiError::from(ClientError::UnknownCommand { cmd: "fan".into() });
    assert!(
        e.to_string()
            .starts_with("unknown command 'fan': the running TrueSense service is older than this app.")
    );
}

#[test]
fn the_layer_can_be_shared_between_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<crate::api::Api>();
    assert_send_sync::<crate::api::ApiError>();
    assert_send_sync::<crate::api::Services>();
}
