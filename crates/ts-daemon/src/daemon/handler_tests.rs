//! The command table through the real wire path: the `ts_ipc` server on a temporary socket and
//! the `ts_ipc` client, the way the CLI, the window and the key service talk to the daemon.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use serde_json::{Value, json};
use ts_ipc::client::{Client, ClientError};
use ts_ipc::server::{Router, Server, ServerConfig};
use ts_ipc::wire::Args;
use ts_platform::firmware::SimFan;

use super::COMMANDS;
use super::testkit::{Rig, Setup, my_uid};

fn args() -> Args {
    Args::new()
}

fn serve(rig: &Rig) -> (Server, Client) {
    let path = rig.path("daemon.sock");
    let server =
        Server::start(ServerConfig::new(&path, rig.daemon.allowed_uids()), rig.daemon.router()).unwrap();
    (server, Client::new(path).with_timeout(Duration::from_secs(3)))
}

#[test]
fn the_table_has_every_command_of_the_python_daemon_and_no_other() {
    let rig = Rig::new();
    let mut want: Vec<&str> = COMMANDS.to_vec();
    want.sort_unstable();
    let router: &Router = &rig.router;
    assert_eq!(router.commands(), want);
    assert_eq!(
        COMMANDS,
        [
            "ping",
            "state",
            "set",
            "fan",
            "fan_curve",
            "smart_profile",
            "undervolt",
            "gpu_undervolt",
            "battery_full",
            "power",
            "boot_look",
            "hold",
            "game_rules",
            "power_managers",
            "cpu_limits",
            "cpu_info",
            "touchpad"
        ]
    );
}

#[test]
fn requests_work_through_the_socket_and_errors_come_back_as_error_replies() {
    let rig = Rig::new();
    let (_server, client) = serve(&rig);
    assert!(client.ping());
    assert_eq!(client.call("ping", args()).unwrap(), json!("pong"));
    // Python clients send floats for whole numbers and null for "not given".
    let r =
        client.call("fan", args().with("mode", "manual").with("cpu", 55.0).with("gpu", Value::Null)).unwrap();
    assert_eq!(r, json!("manual"));
    assert_eq!(rig.snap().duty, [55, 55]);
    let e = client.call("fan", args().with("mode", "manual").with("cpu", "fast")).unwrap_err();
    assert!(matches!(&e, ClientError::Daemon(text) if text.contains("argument \"cpu\"")), "{e}");
    let e = client.call("fan", args().with("mode", "off")).unwrap_err();
    assert!(
        matches!(&e, ClientError::Daemon(text) if text == "stopping a fan needs explicit confirmation"),
        "{e}"
    );
    let e = client.call("nope", args()).unwrap_err();
    assert!(e.is_unknown_command(), "an old daemon is recognisable by this: {e}");
    let state = client.call("state", args()).unwrap();
    assert!(state["settings"]["smart_fan"].is_boolean());
    // set -> the whole settings snapshot comes back.
    let set = client.call("set", args().with("key", "smart_fan").with("value", false)).unwrap();
    assert_eq!(set["smart_fan"], json!(false));
    let set = client.call("set", args().with("key", "smart_fan").with("value", true)).unwrap();
    assert_eq!(set["smart_fan"], json!(true));
}

#[test]
fn every_command_answers_every_nonsense_request_with_one_valid_reply_and_changes_nothing_unsafe() {
    let rig = Rig::new();
    let (_server, client) = serve(&rig);
    rig.drive(30, 45, 45, 2.0);
    let before = (rig.snap().duty, rig.snap().fan);
    for cmd in COMMANDS {
        let junk = [
            args().with("action", "nonsense").with("mode", "nonsense").with("key", "nonsense"),
            args().with("action", 5).with("mode", 7).with("key", true).with("data", "x").with("look", 3),
            args().with("pl1", "a").with("pl2", Value::Null).with("minutes", "soon").with("cpu", [1, 2]),
            args(),
        ];
        for request in junk {
            match client.call(cmd, request.clone()) {
                Ok(_) | Err(ClientError::Daemon(_)) => {}
                Err(other) => panic!("{cmd} {request:?}: {other}"),
            }
        }
    }
    assert_eq!((rig.snap().duty, rig.snap().fan), before, "nonsense never moves the fans");
    assert!(rig.state.get_bool("smart_fan", false));
    assert_eq!(client.call("ping", args()).unwrap(), json!("pong"), "the daemon is still there");
}

#[test]
fn a_whole_session_through_the_socket() {
    let rig = Rig::new();
    let (_server, client) = serve(&rig);
    rig.drive(30, 45, 45, 2.0);
    assert_eq!(client.call("fan", args().with("mode", "off").with("confirm", true)).unwrap(), json!("off"));
    assert_eq!(rig.snap().duty, [0, 0]);
    assert_eq!(client.call("fan", args().with("mode", "auto")).unwrap(), json!("auto"));
    assert_eq!(rig.snap().fan, SimFan::Auto);
    assert_eq!(
        client.call("power", args().with("mode", "turbo").with("hold_minutes", 5)).unwrap(),
        json!("turbo")
    );
    assert_eq!(rig.snap().mode, 5);
    let hold = client.call("state", args()).unwrap()["hold_left_s"].as_i64().unwrap();
    assert!((299..=300).contains(&hold), "{hold}");
    assert_eq!(client.call("hold", args().with("minutes", Value::Null)).unwrap(), json!(0));
    let curve = client.call("fan_curve", args()).unwrap();
    assert!(curve["curve"]["cpu"].is_array());
    let profile = client.call("smart_profile", args()).unwrap();
    assert_eq!(profile["profile"]["ac_busy"], json!("performance"));
    let uv = client
        .call(
            "undervolt",
            args().with("action", "apply").with("core", -40.0).with("cache", -30.0).with("confirm", true),
        )
        .unwrap();
    assert_eq!(uv, json!({"core_mv": -40, "cache_mv": -30}));
    let info = client.call("cpu_info", args()).unwrap();
    assert_eq!(info["pl1_w"], json!(80.0));
    assert_eq!(client.call("touchpad", args()).unwrap(), json!(true));
    assert_eq!(client.call("boot_look", args()).unwrap(), Value::Null);
    assert_eq!(client.call("game_rules", args()).unwrap(), json!({"rules": {}, "running": null}));
    assert_eq!(client.call("power_managers", args()).unwrap()["on"], json!(false));
    assert_eq!(client.call("battery_full", args()).unwrap()["active"], json!(false));
    assert_eq!(client.call("gpu_undervolt", args()).unwrap()["offset_mhz"], json!(0));
    assert_eq!(
        client.call("cpu_limits", args().with("pl1", 60).with("pl2", 80)).unwrap()["pl1_w"],
        json!(60.0)
    );
}

#[test]
fn only_root_and_the_configured_desktop_user_may_use_the_socket() {
    let rig = Rig::new();
    assert_eq!(
        rig.daemon.allowed_uids(),
        vec![my_uid()],
        "the configured user's uid is let in (root always is)"
    );
    let (_server, client) = serve(&rig);
    assert!(client.ping());
    // An unknown user leaves only root, so the test user is refused.
    let config = ts_foundation::config::Config { user: "ghost".to_owned(), ..Default::default() };
    let other = Rig::with(Setup { config, ..Setup::default() });
    assert!(other.daemon.allowed_uids().is_empty());
    let path = other.path("daemon.sock");
    let _server =
        Server::start(ServerConfig::new(&path, other.daemon.allowed_uids()), other.daemon.router()).unwrap();
    let client = Client::new(path).with_timeout(Duration::from_secs(3));
    let e = client.call("ping", args()).unwrap_err();
    assert!(e.to_string().contains("is not allowed to control TrueSense") || my_uid() == 0, "{e}");
}

#[test]
fn a_raw_request_with_a_bad_line_gets_an_error_reply_not_a_crash() {
    let rig = Rig::new();
    let (_server, _client) = serve(&rig);
    for line in [
        &b"not json\n"[..],
        b"{}\n",
        b"{\"cmd\":\"fan\",\"args\":[]}\n",
        b"{\"cmd\":\"ping\",\"args\":null}\n",
    ] {
        let mut stream = UnixStream::connect(rig.path("daemon.sock")).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        stream.write_all(line).unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        assert!(reply.ends_with('\n') && reply.starts_with("{\"ok\":"), "{reply:?}");
    }
}
