#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::*;

// ---- Reply ----------------------------------------------------------------------------------

#[test]
fn parse_table() {
    let ok = |line: &str| Reply::parse("cmd", line).unwrap();
    let pairs = |r: &Reply| r.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
    let s = |k: &str, v: &str| (k.to_string(), v.to_string());

    assert_eq!(pairs(&ok("ok a=1 b=0x2\n")), vec![s("a", "1"), s("b", "0x2")]);
    assert!(ok("ok\n").is_empty());
    assert_eq!(pairs(&ok("ok a=b=c")), vec![s("a", "b=c")]);
    assert_eq!(pairs(&ok("ok =")), vec![s("", "")]);
    assert_eq!(pairs(&ok("ok stray a=1 other")), vec![s("a", "1")]);
    // A repeated key keeps its first place and takes its last value.
    assert_eq!(pairs(&ok("ok a=1 b=2 a=3")), vec![s("a", "3"), s("b", "2")]);
    assert_eq!(ok("  ok   a=1  \n\n").get("a"), Some("1"));

    let rejected = |line: &str| match Reply::parse("cmd", line) {
        Err(FirmwareError::Rejected { command, reason }) => (command, reason),
        other => panic!("{line:?}: {other:?}"),
    };
    assert_eq!(rejected("err invalid argument\n"), ("cmd".into(), "invalid argument".into()));
    assert_eq!(rejected("err\n").1, "error");
    assert_eq!(rejected("err   \n").1, "error");
    assert_eq!(rejected("err get_mode failed (-5)\n").1, "get_mode failed (-5)");

    for line in
        ["hello", "", "   ", "okay a=1", "error: x", "OK a=1", "errx", "\0", "\u{1F600} ok", "ok\u{a0}a=1x"]
    {
        let r = Reply::parse("cmd", line);
        match line {
            // A non-breaking space is Unicode whitespace: "ok" then one field.
            "ok\u{a0}a=1x" => assert_eq!(r.unwrap().get("a"), Some("1x")),
            _ => assert!(matches!(r, Err(FirmwareError::BadReply { .. })), "{line:?}: {r:?}"),
        }
    }
}

#[test]
fn hostile_replies_never_panic() {
    let long = format!("ok {}", "k=v ".repeat(10_000));
    let lines =
        ["ok \0\0=\0", "ok =====", "err \u{202e}x", long.as_str(), "ok a=\u{1F600}", "\u{feff}ok a=1"];
    for line in lines {
        if let Ok(reply) = Reply::parse("cmd", line) {
            for (key, _) in reply.iter() {
                let _ = reply.int(key);
            }
        }
    }
}

#[test]
fn int_reads_decimal_hex_and_negative() {
    let r = Reply::parse("c", "ok d=42 h=0xC3 n=-1 p=+7 z=0x0 big=0x7fffffffffffffff").unwrap();
    assert_eq!(r.int("d").unwrap(), 42);
    assert_eq!(r.int("h").unwrap(), 0xC3);
    assert_eq!(r.int("n").unwrap(), -1);
    assert_eq!(r.int("p").unwrap(), 7);
    assert_eq!(r.int("z").unwrap(), 0);
    assert_eq!(r.int("big").unwrap(), i64::MAX);
}

#[test]
fn int_refuses_malformed_fields() {
    let r = Reply::parse(
        "c",
        "ok bad=x empty= hexempty=0x hexneg=0x-5 hexplus=0x+5 under=1_0 over=99999999999999999999 \
         hexover=0xffffffffffffffff space=1x dot=1.5 upper=0XFF",
    )
    .unwrap();
    for key in [
        "bad", "empty", "hexempty", "hexneg", "hexplus", "under", "over", "hexover", "space", "dot", "upper",
        "missing",
    ] {
        match r.int(key) {
            Err(FirmwareError::BadReply { detail }) => assert!(detail.contains(key), "{detail}"),
            other => panic!("{key}: {other:?}"),
        }
    }
}

#[test]
fn reply_serialises_as_an_ordered_object() {
    let r = Reply::parse("c", "ok version=0.4.0 b=2 a=1").unwrap();
    assert_eq!(serde_json::to_string(&r).unwrap(), r#"{"version":"0.4.0","b":"2","a":"1"}"#);
}

#[test]
fn error_texts_match_the_python_edition() {
    let e = FirmwareError::NotLoaded { path: PathBuf::from("/dev/truesense") };
    assert_eq!(
        e.to_string(),
        "/dev/truesense not found: the truesense_wmi kernel module is not loaded (sudo modprobe truesense_wmi, or run: truesense doctor)"
    );
    let e = FirmwareError::PermissionDenied { path: PathBuf::from("/dev/truesense") };
    assert_eq!(
        e.to_string(),
        "no permission on /dev/truesense (log in on the laptop's seat, or use sudo; see: truesense doctor)"
    );
    let e = Reply::parse("c", "garbage").unwrap_err();
    assert_eq!(e.to_string(), "unexpected reply \"garbage\"");
}

// ---- Simulator ------------------------------------------------------------------------------

fn reply(sim: &Simulator, command: &str) -> Result<Reply, FirmwareError> {
    sim.execute(command)
}

fn reason(sim: &Simulator, command: &str) -> Option<String> {
    match sim.execute(command) {
        Err(FirmwareError::Rejected { reason, .. }) => Some(reason),
        Ok(_) => None,
        Err(other) => panic!("{command:?}: {other:?}"),
    }
}

#[test]
fn simulator_rejects_what_the_kernel_rejects() {
    let sim = Simulator::new();
    // The Python edition's list.
    let python = [
        "set_mode 3",
        "set_fan_manual 10 10",
        "set_usb 15",
        "set_gpu 0",
        "set_battery 3 1",
        "set_battery 1 2",
        "set_boot 7",
        "get_misc 99",
        "get_battery 0",
        "set_rear 00",
        "set_color 123",
        "set_mode",
        "version extra",
        "frobnicate",
    ];
    // The Go edition's list.
    let go = [
        "set_mode 2",
        "set_mode x",
        "set_gpu 4",
        "set_boot 2",
        "set_overdrive 2",
        "get_misc 0",
        "get_misc 17",
        "set_fan_manual 30",
        "set_fan_manual 101 30",
        "set_fan_manual 19 30",
        "set_usb 5",
        "set_kbtimeout 2",
        "get_battery 3",
        "set_battery 3 1",
        "set_color 12345",
        "set_color zzzzzz",
        "set_color ffffff 3",
        "get_mode 1",
        "nonsense",
    ];
    for command in python.iter().chain(go.iter()) {
        assert!(reason(&sim, command).is_some(), "{command:?} must be rejected");
    }
    assert_eq!(reason(&sim, "set_mode").unwrap(), "missing argument");
    assert_eq!(reason(&sim, "version extra").unwrap(), "unexpected argument");
    assert_eq!(reason(&sim, "frobnicate").unwrap(), "unknown command");
    assert_eq!(reason(&sim, "set_mode 3").unwrap(), "invalid argument");
    assert_eq!(sim.snapshot(), Simulator::new().snapshot(), "a rejected command must change nothing");
}

#[test]
fn simulator_accepts_what_the_kernel_accepts() {
    let sim = Simulator::new();
    for command in [
        "version",
        "caps",
        "sensors",
        "get_mode",
        "get_modes",
        "set_mode 5",
        "get_gpu",
        "set_gpu 3",
        "get_boot",
        "set_boot 1",
        "get_misc 1",
        "get_misc 16",
        "get_overdrive",
        "set_overdrive 0",
        "get_fan",
        "set_fan_auto",
        "set_fan_max",
        "set_fan_manual 0 100",
        "set_fan_manual 20 100",
        "set_fan_manual 100 0",
        "set_fan_manual 0 0",
        "get_rear",
        "get_slot",
        "set_usb 20",
        "get_usb",
        "set_color ffffff",
        "set_color ffffff 4",
        "get_kbtimeout",
        "set_kbtimeout 0",
        "get_battery 2",
        "get_battery 1",
        "set_battery 1 1",
        "set_battery 2 0",
    ] {
        assert!(reason(&sim, command).is_none(), "{command:?} must be accepted");
    }
}

#[test]
fn simulator_parses_numbers_like_the_kernel() {
    let sim = Simulator::new();
    // kstrtou8: an optional '+', leading zeros fine, 0x and a minus sign not, 255 is the ceiling.
    for ok in ["set_mode +5", "set_mode 05", "set_mode 0005", "set_usb +030", "get_misc +0016"] {
        assert!(reason(&sim, ok).is_none(), "{ok}");
    }
    for bad in [
        "set_mode -0",
        "set_mode 0x5",
        "set_mode 5.0",
        "set_mode 256",
        "set_mode 99999999999999999999",
        "set_mode +",
        "set_mode ++5",
        "set_mode 5 6",
        "set_mode \u{661}",
        "set_mode 5\u{a0}",
        "get_misc 0x10",
    ] {
        assert!(reason(&sim, bad).is_some(), "{bad}");
    }
    // sscanf("%u %u"): digits, whitespace, digits; the rest of the line is ignored; no sign.
    for ok in [
        "set_fan_manual 40 55",
        "set_fan_manual 40  55",
        "set_fan_manual 40\t55",
        "set_fan_manual 40 55 extra",
    ] {
        assert!(reason(&sim, ok).is_none(), "{ok}");
    }
    assert_eq!(sim.snapshot().duty, [40, 55]);
    for bad in [
        "set_fan_manual 40",
        "set_fan_manual 4055",
        "set_fan_manual +40 55",
        "set_fan_manual 40 +55",
        "set_fan_manual 40x55",
        "set_fan_manual -40 55",
        "set_fan_manual 40 -55",
        "set_fan_manual 4294967296 20",
        "set_fan_manual 20 4294967296",
        "set_battery 1",
        "set_battery x 1",
        "set_battery 1 x",
    ] {
        assert!(reason(&sim, bad).is_some(), "{bad}");
    }
}

#[test]
fn simulator_splits_commands_like_the_kernel() {
    let sim = Simulator::new();
    // The kernel trims, cuts at a NUL and splits the name at a SPACE only.
    for ok in ["  get_mode  ", "get_mode\n", "get_mode\0set_mode 5", "set_mode    5", "set_mode \t 5"] {
        assert!(reason(&sim, ok).is_none(), "{ok:?}");
    }
    assert_eq!(sim.snapshot().mode, 5);
    for bad in ["set_mode\t5", "GET_MODE", "get_mode;set_mode 1", "set_mode\0 5", "\0get_mode", " "] {
        assert!(reason(&sim, bad).is_some(), "{bad:?}");
    }
    assert_eq!(sim.snapshot().mode, 5);
}

#[test]
fn simulator_enforces_the_command_length() {
    let sim = Simulator::new();
    assert!(reason(&sim, "").unwrap().contains("1 to 95"));
    assert!(reason(&sim, &"a".repeat(96)).unwrap().contains("1 to 95"));
    // 95 bytes is the longest the kernel takes: unknown, but not refused for its length.
    assert_eq!(reason(&sim, &"a".repeat(95)).unwrap(), "unknown command");
}

#[test]
fn simulator_checks_every_byte_of_the_rear_packet() {
    let sim = Simulator::new();
    // Bytes 8 and 9 must be 03 02 and the last six zero.
    let packet = |mode: u8, speed: u8, bright: u8, b3: u8, dir: u8, b8: u8, b9: u8, tail: u8| {
        format!(
            "set_rear {mode:02x}{speed:02x}{bright:02x}{b3:02x}{dir:02x}aabbcc{b8:02x}{b9:02x}{tail:02x}0000000000"
        )
    };
    assert!(reason(&sim, &packet(0, 3, 100, 0, 1, 3, 2, 0)).is_none());
    for mode in [0, 1, 2, 3, 7] {
        assert!(reason(&sim, &packet(mode, 3, 100, 0, 1, 3, 2, 0)).is_none(), "mode {mode}");
    }
    for mode in [4, 5, 6, 8, 255] {
        assert!(reason(&sim, &packet(mode, 3, 100, 0, 1, 3, 2, 0)).is_some(), "mode {mode}");
    }
    for (name, cmd) in [
        ("speed 0", packet(0, 0, 100, 0, 1, 3, 2, 0)),
        ("speed 6", packet(0, 6, 100, 0, 1, 3, 2, 0)),
        ("brightness 101", packet(0, 3, 101, 0, 1, 3, 2, 0)),
        ("byte 3", packet(0, 3, 100, 1, 1, 3, 2, 0)),
        ("direction 2", packet(0, 3, 100, 0, 2, 3, 2, 0)),
        ("byte 8", packet(0, 3, 100, 0, 1, 4, 2, 0)),
        ("byte 9", packet(0, 3, 100, 0, 1, 3, 3, 0)),
        ("tail", packet(0, 3, 100, 0, 1, 3, 2, 1)),
        ("short", "set_rear 0003".to_string()),
        ("long", format!("set_rear {}", "00".repeat(17))),
        ("not hex", format!("set_rear {}", "zz".repeat(16))),
        ("odd", format!("set_rear {}", "0".repeat(31))),
        ("multibyte", format!("set_rear {}\u{e9}", "0".repeat(30))),
    ] {
        assert!(reason(&sim, &cmd).is_some(), "{name}");
    }
    // Upper-case hex is fine (hex2bin takes both).
    assert!(
        reason(&sim, &packet(0, 3, 100, 0, 1, 3, 2, 0).to_uppercase().replace("SET_REAR", "set_rear"))
            .is_none()
    );
}

#[test]
fn simulator_checks_the_light_bar_colour_command() {
    let sim = Simulator::new();
    for (cmd, zone, color) in [
        ("set_color ABCDEF", 1, "abcdef"),
        ("set_color 000102 2", 2, "000102"),
        ("set_color ffffff 04", 4, "ffffff"),
        ("set_color ffffff +1", 1, "ffffff"),
    ] {
        assert!(reason(&sim, cmd).is_none(), "{cmd}");
        assert_eq!(sim.snapshot().zones[&zone], color);
    }
    for bad in [
        "set_color ffffff  4",
        "set_color ffffff 3",
        "set_color ffffff 0",
        "set_color ffffff 8",
        "set_color ffffffx",
        "set_color ffffff4",
        "set_color fffff",
        "set_color gggggg",
        "set_color ffffff 4 4",
        "set_color ffffff 4x",
        "set_color \u{e9}\u{e9}\u{e9}f",
        "set_color ffffff \u{e9}",
        "set_color ffff\u{e9}f",
    ] {
        assert!(reason(&sim, bad).is_some(), "{bad}");
    }
}

#[test]
fn simulator_keeps_state_and_resets() {
    let sim = Simulator::new();
    assert_eq!(sim.snapshot().mode, 6);
    reply(&sim, "set_mode 5").unwrap();
    assert_eq!(reply(&sim, "get_mode").unwrap().int("mode").unwrap(), 5);
    reply(&sim, "set_fan_manual 40 55").unwrap();
    let fan = reply(&sim, "get_fan").unwrap();
    assert_eq!(
        (fan.int("behavior").unwrap(), fan.int("cpu").unwrap(), fan.int("gpu").unwrap()),
        (0xC3, 40, 55)
    );
    reply(&sim, "set_fan_max").unwrap();
    assert_eq!(reply(&sim, "get_fan").unwrap().int("behavior").unwrap(), 0x82);
    reply(&sim, "set_battery 2 1").unwrap();
    assert_eq!(reply(&sim, "get_battery 2").unwrap().int("battery").unwrap(), 1);
    assert_eq!(reply(&sim, "get_battery 1").unwrap().int("battery").unwrap(), 0);
    sim.set_sensors([61, 58, 55], [2400, 2300]);
    let r = reply(&sim, "sensors").unwrap();
    let got: Vec<i64> =
        ["cpu", "sys", "gpu", "cpu_rpm", "gpu_rpm"].iter().map(|k| r.int(k).unwrap()).collect();
    assert_eq!(got, [61, 58, 55, 2400, 2300]);
    sim.reset();
    let s = sim.snapshot();
    assert_eq!((s.mode, s.fan, s.duty, s.battery), (6, SimFan::Auto, [30, 30], [0, 0]));
}

#[test]
fn simulator_power_on_state_and_fixed_replies() {
    let sim = Simulator::new();
    assert_eq!(reply(&sim, "version").unwrap().get("version"), Some("sim"));
    let caps = reply(&sim, "caps").unwrap();
    assert!(["game", "apge", "battery"].iter().all(|k| caps.get(k) == Some("1")));
    assert_eq!(reply(&sim, "get_modes").unwrap().int("mask").unwrap(), 0x73);
    assert_eq!(reply(&sim, "get_rear").unwrap().get("raw"), Some("00000300000100aec703000000000000"));
    assert_eq!(reply(&sim, "get_slot").unwrap().get("raw"), Some("00000564000100aec700000000000000"));
    assert_eq!(reply(&sim, "get_misc 7").unwrap().int("idx").unwrap(), 7);
    let s = sim.snapshot();
    assert_eq!((s.gpu, s.boot, s.overdrive, s.usb, s.kbtimeout), (2, 0, 1, 30, 1));
}

#[test]
fn simulator_serialises_concurrent_commands() {
    let sim = Arc::new(Simulator::new());
    let threads: Vec<_> = (0..8u8)
        .map(|t| {
            let sim = Arc::clone(&sim);
            std::thread::spawn(move || {
                for i in 0..200u32 {
                    let level = [0, 10, 20, 30][((u32::from(t) + i) % 4) as usize];
                    let set = sim.execute(&format!("set_usb {level}")).unwrap();
                    assert_eq!(set.int("usb").unwrap(), level);
                    assert!([0, 10, 20, 30].contains(&sim.execute("get_usb").unwrap().int("usb").unwrap()));
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
}

// ---- KernelDevice ---------------------------------------------------------------------------

/// What the scripted device does with the next write.
#[derive(Clone, Copy)]
enum Act {
    /// The command works: this is the reply.
    Reply(&'static str),
    /// The write fails with EINVAL and the module leaves this reply behind.
    Reject(&'static str),
    /// The write fails with ENODEV and no reply exists.
    WriteFails,
    /// The write works but the reply read is empty.
    EmptyReply,
    /// The thread dies in the middle of the write (the lock is held).
    Panic,
}

#[derive(Default)]
struct Script {
    acts: Mutex<VecDeque<Act>>,
    writes: Mutex<Vec<String>>,
    opens: AtomicUsize,
}

struct Fake {
    script: Arc<Script>,
    reply: Option<Vec<u8>>,
}

impl Write for Fake {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.script.writes.lock().unwrap().push(String::from_utf8_lossy(buf).into_owned());
        let act = self.script.acts.lock().unwrap().pop_front().unwrap_or(Act::Reply("ok\n"));
        self.reply = None;
        match act {
            Act::Reply(r) => {
                self.reply = Some(r.as_bytes().to_vec());
                Ok(buf.len())
            }
            Act::Reject(r) => {
                self.reply = Some(r.as_bytes().to_vec());
                Err(io::Error::from_raw_os_error(22))
            }
            Act::WriteFails => Err(io::Error::from_raw_os_error(19)),
            Act::EmptyReply => Ok(buf.len()),
            Act::Panic => panic!("a thread dies while holding the channel"),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Read for Fake {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // Like the kernel: the reply is handed over once, a second read is end of file.
        match self.reply.take() {
            Some(r) => {
                let n = r.len().min(buf.len());
                buf[..n].copy_from_slice(&r[..n]);
                Ok(n)
            }
            None => Ok(0),
        }
    }
}

fn scripted(acts: &[Act]) -> (KernelDevice, Arc<Script>) {
    let script = Arc::new(Script::default());
    script.acts.lock().unwrap().extend(acts.iter().copied());
    let for_opener = Arc::clone(&script);
    let device = KernelDevice::with_opener(
        "/dev/truesense",
        Box::new(move || {
            for_opener.opens.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(Fake { script: Arc::clone(&for_opener), reply: None }) as Box<dyn Channel>)
        }),
    );
    (device, script)
}

#[test]
fn the_device_is_opened_once_and_kept() {
    let (device, script) =
        scripted(&[Act::Reply("ok version=0.4.0\n"), Act::Reply("ok mode=6\n"), Act::Reply("ok mode=5\n")]);
    assert_eq!(device.execute("version").unwrap().get("version"), Some("0.4.0"));
    assert_eq!(device.execute("get_mode").unwrap().int("mode").unwrap(), 6);
    assert_eq!(device.execute("get_mode").unwrap().int("mode").unwrap(), 5);
    assert_eq!(script.opens.load(Ordering::SeqCst), 1);
    assert_eq!(*script.writes.lock().unwrap(), ["version", "get_mode", "get_mode"]);
}

#[test]
fn nothing_is_opened_before_the_first_command() {
    let (_device, script) = scripted(&[]);
    assert_eq!(script.opens.load(Ordering::SeqCst), 0);
}

#[test]
fn a_rejected_command_returns_the_modules_reason_and_keeps_the_file() {
    let (device, script) = scripted(&[Act::Reject("err invalid argument\n"), Act::Reply("ok mode=6\n")]);
    match device.execute("set_mode 3") {
        Err(FirmwareError::Rejected { command, reason }) => {
            assert_eq!((command.as_str(), reason.as_str()), ("set_mode 3", "invalid argument"))
        }
        other => panic!("{other:?}"),
    }
    assert!(device.execute("get_mode").is_ok());
    assert_eq!(script.opens.load(Ordering::SeqCst), 1, "a rejection is not a stale file");
}

#[test]
fn a_rejection_without_a_reason_still_says_something() {
    let (device, _script) = scripted(&[Act::Reject("err\n")]);
    assert_eq!(device.execute("set_mode 3").unwrap_err().to_string(), "error");
}

#[test]
fn a_stale_file_is_reopened_once_and_the_command_repeated() {
    let (device, script) = scripted(&[Act::Reply("ok mode=6\n"), Act::WriteFails, Act::Reply("ok mode=1\n")]);
    assert!(device.execute("get_mode").is_ok());
    assert_eq!(device.execute("get_mode").unwrap().int("mode").unwrap(), 1);
    assert_eq!(script.opens.load(Ordering::SeqCst), 2);
    assert_eq!(script.writes.lock().unwrap().len(), 3);
    assert!(device.execute("get_mode").is_ok());
    assert_eq!(script.opens.load(Ordering::SeqCst), 2, "the new file is kept");
}

#[test]
fn an_empty_reply_counts_as_stale() {
    let (device, script) = scripted(&[Act::Reply("ok\n"), Act::EmptyReply, Act::Reply("ok mode=4\n")]);
    assert!(device.execute("version").is_ok());
    assert_eq!(device.execute("get_mode").unwrap().int("mode").unwrap(), 4);
    assert_eq!(script.opens.load(Ordering::SeqCst), 2);
}

#[test]
fn a_file_that_fails_right_after_opening_is_an_error_not_a_retry_loop() {
    let (device, script) = scripted(&[Act::WriteFails, Act::WriteFails, Act::WriteFails]);
    match device.execute("get_mode") {
        Err(FirmwareError::Io { operation, .. }) => assert_eq!(operation, "get_mode"),
        other => panic!("{other:?}"),
    }
    assert_eq!(script.opens.load(Ordering::SeqCst), 1);
    assert_eq!(script.writes.lock().unwrap().len(), 1, "a fresh file that fails is not retried");
    // The failed file was dropped: the next command opens another one.
    assert!(device.execute("get_mode").is_err());
    assert_eq!(script.opens.load(Ordering::SeqCst), 2);
}

#[test]
fn a_kept_file_that_stays_stale_after_one_reopen_is_an_error() {
    let (device, script) = scripted(&[Act::Reply("ok\n"), Act::WriteFails, Act::WriteFails]);
    assert!(device.execute("version").is_ok());
    assert!(matches!(device.execute("get_mode"), Err(FirmwareError::Io { .. })));
    assert_eq!(script.opens.load(Ordering::SeqCst), 2);
    assert_eq!(script.writes.lock().unwrap().len(), 3, "one retry, no more");
}

#[test]
fn open_errors_are_classified() {
    let device_with = |kind: io::ErrorKind| {
        KernelDevice::with_opener("/dev/x", Box::new(move || Err(io::Error::new(kind, "boom"))))
    };
    assert!(matches!(
        device_with(io::ErrorKind::NotFound).run("version"),
        Err(FirmwareError::NotLoaded { path }) if path.as_os_str() == "/dev/x"
    ));
    assert!(matches!(
        device_with(io::ErrorKind::PermissionDenied).run("version"),
        Err(FirmwareError::PermissionDenied { .. })
    ));
    match device_with(io::ErrorKind::Other).run("version") {
        Err(e @ FirmwareError::Io { .. }) => {
            assert!(e.to_string().starts_with("open /dev/x"), "{e}");
            assert!(std::error::Error::source(&e).is_some());
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_real_opener_reports_a_missing_or_unreadable_device() {
    let missing = KernelDevice::new("/nonexistent/truesense");
    assert!(matches!(missing.run("version"), Err(FirmwareError::NotLoaded { .. })));

    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("ts-platform-fw-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("locked");
    std::fs::write(&file, b"").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
    let readable_anyway = std::fs::File::open(&file).is_ok(); // running as root
    if !readable_anyway {
        assert!(matches!(
            KernelDevice::new(&file).run("version"),
            Err(FirmwareError::PermissionDenied { .. })
        ));
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn commands_the_kernel_would_drop_are_answered_without_touching_the_device() {
    let (device, script) = scripted(&[]);
    for command in ["", &"x".repeat(96)] {
        let line = device.run(command).unwrap();
        assert!(line.starts_with("err "), "{line}");
        assert!(matches!(Reply::parse(command, &line), Err(FirmwareError::Rejected { .. })));
    }
    assert_eq!(script.opens.load(Ordering::SeqCst), 0);
    assert!(script.writes.lock().unwrap().is_empty());
}

#[test]
fn concurrent_commands_never_swap_replies() {
    // A reply is only valid for the write before it: if two threads could interleave a write
    // and a read on the one shared file, a thread would read another thread's answer.
    struct Echo {
        last: Option<String>,
    }
    impl Write for Echo {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            std::thread::yield_now();
            self.last = Some(String::from_utf8_lossy(buf).into_owned());
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl Read for Echo {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            std::thread::yield_now();
            let line = format!("ok echo={}\n", self.last.take().unwrap_or_default().replace(' ', "_"));
            buf[..line.len()].copy_from_slice(line.as_bytes());
            Ok(line.len())
        }
    }
    let device = Arc::new(KernelDevice::with_opener(
        "/dev/echo",
        Box::new(|| Ok(Box::new(Echo { last: None }) as Box<dyn Channel>)),
    ));
    let threads: Vec<_> = (0..8)
        .map(|t| {
            let device = Arc::clone(&device);
            std::thread::spawn(move || {
                for i in 0..200 {
                    let command = format!("set_usb {t}{i}");
                    let reply = device.execute(&command).unwrap();
                    assert_eq!(reply.get("echo"), Some(command.replace(' ', "_").as_str()));
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
}

#[test]
fn a_panic_in_one_thread_does_not_wedge_the_device() {
    let (device, script) = scripted(&[Act::Reply("ok\n"), Act::Panic]);
    let device = Arc::new(device);
    assert!(device.execute("version").is_ok());
    let dying = Arc::clone(&device);
    assert!(std::thread::spawn(move || dying.run("get_mode")).join().is_err());
    assert!(device.execute("version").is_ok());
    assert_eq!(script.opens.load(Ordering::SeqCst), 2, "a channel left by a panic is reopened");
}

// ---- ReadOnly -------------------------------------------------------------------------------

struct Recorder {
    inner: Simulator,
    sent: Mutex<Vec<String>>,
}

impl Device for Recorder {
    fn run(&self, command: &str) -> Result<String, FirmwareError> {
        self.sent.lock().unwrap().push(command.to_string());
        self.inner.run(command)
    }
}

#[test]
fn the_read_only_guard_lets_reads_through_and_nothing_else() {
    let recorder = Recorder { inner: Simulator::new(), sent: Mutex::new(Vec::new()) };
    let guard = ReadOnly::new(&recorder);
    for name in READ_ONLY_COMMANDS {
        let command =
            if matches!(name, "get_misc" | "get_battery") { format!("{name} 1") } else { name.to_string() };
        assert!(guard.execute(&command).is_ok(), "{command}");
    }
    assert_eq!(recorder.sent.lock().unwrap().len(), READ_ONLY_COMMANDS.len());

    // Every writing command of the module's table, with and without a valid argument.
    let writers = [
        "set_mode 1",
        "set_mode",
        "set_gpu 1",
        "set_boot 1",
        "set_overdrive 1",
        "set_fan_auto",
        "set_fan_max",
        "set_fan_manual 50 50",
        "set_rear 00",
        "set_color ffffff",
        "set_usb 0",
        "set_kbtimeout 0",
        "set_battery 1 1",
        // tricks
        "SET_MODE 1",
        " set_mode 1",
        "get_mode;set_mode 1",
        "get_mode\nset_mode 1",
        "get_mode\0set_mode 1",
        "get_mode\tset_mode 1",
        "get_mode\rset_mode 1",
        "\0",
        "",
        " ",
        "get_",
        "get_modex",
        "getmode",
        "versions",
        "force=1",
        "modprobe",
        "get_\u{200b}mode",
        "\u{ff47}et_mode",
    ];
    let sent_before = recorder.sent.lock().unwrap().len();
    let state_before = recorder.inner.snapshot();
    for command in writers {
        match guard.run(command) {
            Err(FirmwareError::Refused { command: refused }) => assert_eq!(refused, command),
            other => panic!("{command:?} must be refused, got {other:?}"),
        }
        assert!(guard.execute(command).is_err());
    }
    assert_eq!(
        recorder.sent.lock().unwrap().len(),
        sent_before,
        "a refused command must never reach the device"
    );
    assert_eq!(recorder.inner.snapshot(), state_before);
}

#[test]
fn the_read_only_list_has_no_writer_and_every_reader_of_the_table() {
    for name in READ_ONLY_COMMANDS {
        assert!(
            name == "version" || name == "caps" || name.starts_with("sensors") || name.starts_with("get_"),
            "{name}"
        );
        assert!(!name.starts_with("set_"));
    }
    let table = include_str!("../../../../kernel/truesense-wmi/truesense_wmi.c");
    let readers: Vec<&str> = table
        .lines()
        .flat_map(|l| l.split("{ \"").skip(1))
        .filter_map(|c| c.split('"').next())
        .filter(|n| !n.starts_with("set_"))
        .collect();
    assert_eq!(readers.len(), READ_ONLY_COMMANDS.len(), "{readers:?}");
    for name in readers {
        assert!(READ_ONLY_COMMANDS.contains(&name), "{name}");
    }
}

// ---- DeviceKind -----------------------------------------------------------------------------

#[test]
fn the_environment_selects_the_device() {
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| (*v).to_string())
    };
    assert_eq!(DeviceKind::from_env(&env(&[("TRUESENSE_SIM", "1")])), DeviceKind::Simulator);
    // As in the Python edition any non-empty value counts, even "0".
    assert_eq!(DeviceKind::from_env(&env(&[("TRUESENSE_SIM", "0")])), DeviceKind::Simulator);
    assert_eq!(
        DeviceKind::from_env(&env(&[("TRUESENSE_SIM", ""), ("TRUESENSE_DEVICE", "/x/y")])),
        DeviceKind::Kernel(PathBuf::from("/x/y"))
    );
    assert_eq!(DeviceKind::from_env(&env(&[])), DeviceKind::Kernel(PathBuf::from(DEVICE_PATH)));
    assert_eq!(
        DeviceKind::from_env(&env(&[("TRUESENSE_DEVICE", "")])),
        DeviceKind::Kernel(PathBuf::from(DEVICE_PATH))
    );
    assert_eq!(
        DeviceKind::from_env(&env(&[("TRUESENSE_SIM", "1"), ("TRUESENSE_DEVICE", "/x")])),
        DeviceKind::Simulator
    );
    let device = DeviceKind::Simulator.open();
    assert_eq!(device.execute("version").unwrap().get("version"), Some("sim"));
    let missing = DeviceKind::Kernel(PathBuf::from("/nonexistent/truesense")).open();
    assert!(matches!(missing.run("version"), Err(FirmwareError::NotLoaded { .. })));
}
