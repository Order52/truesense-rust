//! The command table, the error replies and the shape of `state`: what every client depends on.
//!
//! The wire protocol is the Python daemon's (`daemon.py`, `handlers()`): the Python and Go
//! clients keep working against this daemon only if the names, the argument names and the result
//! shapes are the same. These tests fix the table to the Python one and look at the `state`
//! answer key by key.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;

use serde_json::{Value, json};
use ts_daemon::daemon::COMMANDS;
use ts_ipc::client::ClientError;
use ts_ipc::wire::Args;

use super::rig::{Rig, Setup};

/// The names of `daemon.py`'s `handlers()` table, in its order.
const PYTHON_COMMANDS: [&str; 17] = [
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
    "touchpad",
];

#[test]
fn the_daemon_knows_the_python_commands_and_the_ones_added_since() {
    // The Python table first, in its order; what this edition added follows it.
    assert_eq!(COMMANDS[..PYTHON_COMMANDS.len()], PYTHON_COMMANDS);
    assert_eq!(COMMANDS[PYTHON_COMMANDS.len()..], ["mode_bindings"]);
    let rig = Rig::new();
    for cmd in PYTHON_COMMANDS {
        // Whatever the arguments, a known command never answers "unknown command".
        if let Err(e) = rig.client.call(cmd, Args::new()) {
            assert!(!e.is_unknown_command(), "{cmd}: {e}");
        }
    }
}

#[test]
fn an_unknown_command_is_the_old_daemon_signal_for_the_clients() {
    let rig = Rig::new();
    let err = rig.client.call("no_such_command", Args::new()).unwrap_err();
    assert!(matches!(&err, ClientError::UnknownCommand { cmd } if cmd == "no_such_command"), "{err:?}");
    // The application layer turns the same reply into its "service too old" text.
    assert!(err.to_string().contains("older than this app"), "{err}");
}

#[test]
fn ping_answers_pong_and_the_api_agrees() {
    let rig = Rig::new();
    assert_eq!(rig.client.call("ping", Args::new()).unwrap(), json!("pong"));
    assert!(rig.client.ping());
    assert!(rig.api.daemon_available());
    assert!(rig.api.daemon_responds());
}

#[test]
fn a_refusal_comes_back_as_the_daemons_own_words() {
    let rig = Rig::new();
    let err = rig.client.call("set", Args::new().with("key", "colour").with("value", true)).unwrap_err();
    // Python: `raise hw.HwError(f"unknown setting {key!r}")`.
    assert!(matches!(&err, ClientError::Daemon(t) if t == "unknown setting 'colour'"), "{err:?}");
}

#[test]
fn state_has_the_python_live_keys_with_python_types() {
    let rig = Rig::new();
    let state = rig.client.call("state", Args::new()).unwrap();
    let object = state.as_object().unwrap();
    assert_eq!(
        object.keys().cloned().collect::<std::collections::BTreeSet<_>>(),
        ["hold_left_s", "live", "settings"].map(String::from).into()
    );
    assert!(object["hold_left_s"].is_i64(), "{state}");
    assert_eq!(object["hold_left_s"], json!(0));

    // `daemon.py` publish(): the keys of `self.live`, with the type each has there.
    let live = object["live"].as_object().unwrap();
    let ints = ["tcpu", "tgpu", "chassis", "cpu", "gpu", "gpuw", "rpm1", "rpm2", "cap"];
    let strings =
        ["mode", "activity", "fan", "power_mode", "auto_mode", "fan_curve", "smart_profile", "mode_reason"];
    let bools = ["ac", "auto", "fan_off", "hw_ok"];
    let mut expected: Vec<&str> = ints.iter().chain(&strings).chain(&bools).copied().collect();
    expected.sort_unstable();
    let mut got: Vec<&str> = live.keys().map(String::as_str).collect();
    got.sort_unstable();
    assert_eq!(got, expected, "the live keys differ from daemon.py publish()");
    for k in ints {
        assert!(live[k].is_i64(), "live.{k} must be a whole number, got {}", live[k]);
    }
    for k in strings {
        assert!(live[k].is_string(), "live.{k} must be text, got {}", live[k]);
    }
    for k in bools {
        assert!(live[k].is_boolean(), "live.{k} must be true or false, got {}", live[k]);
    }
    // The values of the idle simulated laptop on AC.
    assert_eq!(live["ac"], json!(true));
    assert_eq!(live["hw_ok"], json!(true));
    assert_eq!(live["fan"], json!("auto"));
    assert_eq!(live["power_mode"], json!("eco"));
    assert_eq!(live["fan_curve"], json!("native"));
    assert_eq!(live["smart_profile"], json!("native"));
}

#[test]
fn state_settings_hold_the_python_default_keys() {
    let rig = Rig::new();
    let st = rig.api.daemon_state().unwrap();
    // `Store` defaults of the Python daemon (`State(...)` in `Daemon.__init__`).
    for key in [
        "smart_fan",
        "auto_profile",
        "auto_mode",
        "fan_curve",
        "fan_curve_data",
        "pl_user",
        "smart_profile",
        "smart_profile_data",
        "operating_follow",
        "profile_hold_until",
    ] {
        assert!(st.settings.contains_key(key), "settings lack {key}: {:?}", st.settings);
    }
    assert_eq!(st.setting_bool("smart_fan"), Some(false));
    assert_eq!(st.setting_str("auto_mode"), Some("off"));
    assert_eq!(st.setting_str("fan_curve"), Some("native"));
    assert_eq!(st.hold_left_s, 0);
    assert_eq!(st.live_str("power_mode"), Some("eco"));
    assert_eq!(st.live_bool("ac"), Some(true));
}

#[test]
fn the_live_numbers_follow_the_firmware() {
    let rig = Rig::new();
    rig.heat(61, 55, 52);
    let st = rig.api.daemon_state().unwrap();
    assert_eq!(st.live_f64("tcpu"), Some(61.0));
    assert_eq!(st.live_f64("tgpu"), Some(52.0));
    assert_eq!(st.live_f64("chassis"), Some(55.0));
    assert_eq!(st.live_f64("rpm1"), Some(2400.0));
    assert_eq!(st.live_f64("rpm2"), Some(2200.0));
    // The window's `live()` reads the same numbers through the service.
    let live = rig.api.live();
    let s = live.sensors.unwrap();
    assert_eq!((s.cpu_temp, s.gpu_temp, s.sys_temp, s.cpu_fan_rpm, s.gpu_fan_rpm), (61, 52, 55, 2400, 2200));
    assert_eq!(live.power_mode.as_deref(), Some("eco"));
    assert_eq!(live.ac, Some(true));
}

#[test]
fn the_live_state_file_is_the_python_key_value_text() {
    let rig = Rig::new();
    let text = std::fs::read_to_string(rig.dir.0.join("run/state")).unwrap();
    // `publish()`: `key=value` lines, booleans as 0/1.
    assert!(text.contains("\nac=1\n") || text.starts_with("ac=1\n"), "{text}");
    assert!(text.contains("power_mode=eco\n"), "{text}");
    assert!(text.contains("hw_ok=1\n"), "{text}");
    let rpm: Value = json!(text.lines().filter(|l| l.starts_with("rpm1=")).count());
    assert_eq!(rpm, json!(1));
}

/// One request line as raw bytes, one reply line back, like `socket.makefile().readline()`.
fn exchange(rig: &Rig, request: &[u8]) -> String {
    let mut stream = UnixStream::connect(&rig.sock).unwrap();
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
    stream.write_all(request).unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    line
}

#[test]
fn the_bytes_python_s_ipc_call_sends_are_understood() {
    let rig = Rig::new();
    // `json.dumps({"cmd": cmd, "args": args}).encode() + b"\n"`, with its spaces.
    let reply = exchange(&rig, b"{\"cmd\": \"ping\", \"args\": {}}\n");
    assert!(reply.ends_with('\n'));
    assert_eq!(serde_json::from_str::<Value>(&reply).unwrap(), json!({"ok": true, "result": "pong"}));
    // `req.get("args", {})`: a request without `args` is a request without arguments.
    let reply = exchange(&rig, b"{\"cmd\": \"ping\"}\n");
    assert_eq!(serde_json::from_str::<Value>(&reply).unwrap(), json!({"ok": true, "result": "pong"}));
    let reply = exchange(&rig, b"{\"cmd\": \"hold\", \"args\": {\"minutes\": null}}\n");
    assert_eq!(serde_json::from_str::<Value>(&reply).unwrap()["ok"], json!(true));
    let reply = exchange(&rig, b"{\"cmd\": \"set\", \"args\": {\"key\": \"smart_fan\", \"value\": true}}\n");
    let reply: Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(reply["ok"], json!(true));
    assert_eq!(reply["result"]["smart_fan"], json!(true), "{reply}");
}

#[test]
fn a_reply_to_an_error_is_ok_false_with_the_text_as_python_sends_it() {
    let rig = Rig::new();
    let reply = exchange(&rig, b"{\"cmd\": \"nope\", \"args\": {}}\n");
    let reply: Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(reply["ok"], json!(false));
    // The client recognises an old daemon by this prefix; Python spells the rest `'nope'`.
    assert!(reply["error"].as_str().unwrap().starts_with("unknown command"), "{reply}");
    // Python answers a request with no `cmd` with "unknown command None"; this server is
    // stricter ("bad request") and says why. No client sends one, so the difference is harmless.
    let reply = exchange(&rig, b"{}\n");
    let reply: Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(reply["ok"], json!(false));
    assert!(reply["error"].as_str().unwrap().starts_with("bad request"), "{reply}");
    for garbage in [&b"not json\n"[..], &b"[1, 2]\n"[..], &b"\"ping\"\n"[..], &b"{\"cmd\": 7}\n"[..]] {
        let reply: Value = serde_json::from_str(&exchange(&rig, garbage)).unwrap();
        assert_eq!(reply["ok"], json!(false), "{}", String::from_utf8_lossy(garbage));
        assert!(reply["error"].is_string());
    }
    // The daemon is still there afterwards.
    assert!(rig.client.ping());
}

#[test]
fn the_socket_is_connectable_by_every_local_user() {
    let rig = Rig::new();
    let mode = std::fs::metadata(&rig.sock).unwrap().permissions().mode();
    // Python binds under `umask(0)`; the uid check, not the file mode, decides who may do things.
    assert_eq!(mode & 0o666, 0o666, "{mode:o}");
}

#[test]
fn another_user_is_refused_with_python_s_sentence() {
    let rig = Rig::with(Setup { user: Some("somebody-else"), ..Setup::default() });
    let uid = std::fs::metadata(&rig.dir.0).unwrap().uid_of();
    if uid == 0 {
        return; // root is always allowed
    }
    let reply: Value = serde_json::from_str(&exchange(&rig, b"{\"cmd\": \"ping\", \"args\": {}}\n")).unwrap();
    assert_eq!(
        reply,
        json!({"ok": false, "error": format!("uid {uid} is not allowed to control TrueSense")})
    );
    let err = rig.client.call("fan", Args::new().with("mode", "max")).unwrap_err();
    assert!(
        matches!(&err, ClientError::Daemon(t) if t.contains("is not allowed to control TrueSense")),
        "{err:?}"
    );
    assert!(!rig.api.daemon_responds());
    assert_eq!(rig.sim.snapshot().fan, ts_platform::firmware::SimFan::Auto);
}

trait UidOf {
    fn uid_of(&self) -> u32;
}

impl UidOf for std::fs::Metadata {
    fn uid_of(&self) -> u32 {
        std::os::unix::fs::MetadataExt::uid(self)
    }
}

/// The Go edition builds its numbers as floats (`"cpu": 45.0`, `"core": -100.0`, `"hold_minutes":
/// 30.0`), where Python sends whole numbers as integers. Both must work.
#[test]
fn the_requests_the_go_edition_sends_are_understood() {
    let rig = Rig::new();
    let call = |cmd: &str, args: Args| rig.client.call(cmd, args).unwrap_or_else(|e| panic!("{cmd}: {e}"));
    assert_eq!(call("fan", Args::new().with("mode", "manual").with("cpu", 45.0)), json!("manual"));
    assert_eq!(rig.sim.snapshot().duty, [45, 45]);
    assert_eq!(
        call(
            "fan",
            Args::new().with("mode", "manual").with("cpu", 0.0).with("gpu", 60.0).with("confirm", true)
        ),
        json!("partial-off")
    );
    assert_eq!(rig.sim.snapshot().duty, [0, 60]);
    // Go sends `confirm` even when it is false, and `cpu` without `gpu`.
    assert_eq!(call("fan", Args::new().with("mode", "auto")), json!("auto"));
    assert_eq!(call("fan", Args::new().with("mode", "max")), json!("max"));
    assert_eq!(call("fan", Args::new().with("mode", "smart")), json!("smart"));
    assert_eq!(call("power", Args::new().with("mode", "turbo").with("hold_minutes", 30.0)), json!("turbo"));
    assert_eq!(call("power", Args::new().with("mode", "eco")), json!("eco"), "no hold_minutes at all");
    let applied = call("cpu_limits", Args::new().with("pl1", 70.0).with("pl2", 90.0));
    assert_eq!(applied, json!({"pl1_w": 70.0, "pl2_w": 90.0}));
    call("cpu_limits", Args::new().with("reset", true));
    let uv =
        Args::new().with("action", "apply").with("core", -50.0).with("cache", -40.0).with("confirm", true);
    assert_eq!(call("undervolt", uv.with("name", "truesense"))["core_mv"], json!(-50));
    let uv =
        Args::new().with("action", "boot").with("core", -30.0).with("cache", -30.0).with("confirm", true);
    assert_eq!(call("undervolt", uv.with("name", "mild"))["core"], json!(-30));
    let danger = Args::new().with("action", "apply").with("core", -100.0).with("cache", -60.0);
    call("undervolt", danger.with("confirm", true).with("force", true));
    let gpu =
        Args::new().with("action", "apply").with("offset", -100.0).with("cap", 2000.0).with("confirm", true);
    assert_eq!(call("gpu_undervolt", gpu.with("name", "x")), json!({"offset_mhz": -100, "cap_mhz": 2000}));
    let gpu = Args::new().with("action", "boot").with("offset", -50.0).with("cap", 0.0).with("confirm", true);
    assert_eq!(call("gpu_undervolt", gpu)["offset"], json!(-50));
    assert_eq!(call("hold", Args::new().with("minutes", 30.0)), json!(30.0));
    assert_eq!(call("hold", Args::new().with("minutes", Value::Null)), json!(0));
    assert_eq!(
        call(
            "game_rules",
            Args::new().with("action", "add").with("name", "HL2_Linux").with("mode", "performance")
        )["rules"]["hl2_linux"],
        json!("performance")
    );
    assert_eq!(call("touchpad", Args::new().with("action", "status")), json!(true));
    // `ipc.Args(nil)`: no arguments at all, for the commands that take none.
    assert!(call("cpu_info", Args::new())["pl1_w"].is_f64());
}
