//! The firmware simulator: `TRUESENSE_SIM=1`.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

use super::{Device, FirmwareError, length_reply};
use ts_foundation::sync::locked;

/// The fan control state the simulated firmware is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimFan {
    /// The firmware's own control.
    Auto,
    /// Both fans at full speed.
    Max,
    /// The duty registers are what was last written.
    Manual,
}

/// A copy of the simulated firmware's state, so a test can check what was really written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Power mode code (0 quiet, 1 balanced, 4 performance, 5 turbo, 6 eco).
    pub mode: u8,
    /// GPU mode code (1 optimus, 2 nvidia, 3 auto).
    pub gpu: u8,
    /// Boot sound on (1) or off (0).
    pub boot: u8,
    /// LCD overdrive on (1) or off (0).
    pub overdrive: u8,
    /// USB charging stop level while off: 0, 10, 20 or 30.
    pub usb: u8,
    /// Keyboard backlight timeout flag.
    pub kbtimeout: u8,
    /// Fan control state.
    pub fan: SimFan,
    /// Fan duty registers: CPU, GPU.
    pub duty: [u8; 2],
    /// Battery flags: the 80 % limit, then calibration.
    pub battery: [u8; 2],
    /// Rear light zone colours by zone bit (1, 2, 4), as RRGGBB text.
    pub zones: BTreeMap<u8, String>,
    /// What `get_rear` returns: a status byte, then the first 15 bytes of the last `set_rear`.
    pub rear: [u8; 16],
    /// Temperatures reported by `sensors`: CPU, system, GPU.
    pub temps: [u32; 3],
    /// Fan speeds reported by `sensors`: CPU, GPU.
    pub rpm: [u32; 2],
}

impl Snapshot {
    /// The power-on state of the tested laptop (eco mode, NVIDIA-only GPU, USB charging to 30 %,
    /// brightness 0 on the rear light).
    fn power_on() -> Self {
        Self {
            mode: 6,
            gpu: 2,
            boot: 0,
            overdrive: 1,
            usb: 30,
            kbtimeout: 1,
            fan: SimFan::Auto,
            duty: [30, 30],
            battery: [0, 0],
            zones: BTreeMap::new(),
            rear: [0x00, 0x00, 0x03, 0x00, 0x00, 0x01, 0x00, 0xae, 0xc7, 0x03, 0, 0, 0, 0, 0, 0],
            temps: [50, 52, 48],
            rpm: [0, 0],
        }
    }
}

/// An in-memory firmware that answers the module's protocol and rejects what the kernel
/// rejects, argument by argument (including the kernel's quirks, such as accepting a leading `+`
/// in a number). It backs the unit tests and `TRUESENSE_SIM=1`, so everything above runs
/// unchanged without hardware.
#[derive(Debug)]
pub struct Simulator {
    state: Mutex<Snapshot>,
}

impl Default for Simulator {
    fn default() -> Self {
        Self::new()
    }
}

const INVALID: &str = "err invalid argument\n";

impl Simulator {
    /// A simulator in the power-on state.
    pub fn new() -> Self {
        Self { state: Mutex::new(Snapshot::power_on()) }
    }

    fn lock(&self) -> MutexGuard<'_, Snapshot> {
        // The state is plain data and every command updates it in one step, so it is consistent
        // even if a panicking test thread poisoned the lock.
        locked(&self.state)
    }

    /// Puts the simulated firmware back to its power-on state.
    pub fn reset(&self) {
        *self.lock() = Snapshot::power_on();
    }

    /// Chooses the temperatures (CPU, system, GPU) and fan speeds (CPU, GPU) that `sensors` reports.
    pub fn set_sensors(&self, temps: [u32; 3], rpm: [u32; 2]) {
        let mut state = self.lock();
        state.temps = temps;
        state.rpm = rpm;
    }

    /// A copy of the current state.
    pub fn snapshot(&self) -> Snapshot {
        self.lock().clone()
    }
}

impl Device for Simulator {
    /// Mirrors `ts_write` of the kernel module: the same trimming, the same command table, the
    /// same argument checks, the same reply texts.
    fn run(&self, command: &str) -> Result<String, FirmwareError> {
        if let Some(refusal) = length_reply(command) {
            return Ok(refusal);
        }
        let (name, arg) = split_command(command);
        let Some(&(_, takes_arg)) = COMMANDS.iter().find(|(n, _)| *n == name) else {
            return Ok("err unknown command\n".to_string());
        };
        let arg = match (takes_arg, arg) {
            (true, None) => return Ok("err missing argument\n".to_string()),
            (false, Some(_)) => return Ok("err unexpected argument\n".to_string()),
            (_, arg) => arg.unwrap_or(""),
        };
        let mut state = self.lock();
        Ok(dispatch(&mut state, name, arg).unwrap_or_else(|| INVALID.to_string()))
    }
}

/// The module's command table (`ts_cmds`): name and whether it takes an argument.
const COMMANDS: [(&str, bool); 28] = [
    ("version", false),
    ("caps", false),
    ("sensors", false),
    ("sensors_temps", false),
    ("get_mode", false),
    ("get_modes", false),
    ("set_mode", true),
    ("get_gpu", false),
    ("set_gpu", true),
    ("get_boot", false),
    ("set_boot", true),
    ("get_misc", true),
    ("get_overdrive", false),
    ("set_overdrive", true),
    ("get_fan", false),
    ("set_fan_auto", false),
    ("set_fan_max", false),
    ("set_fan_manual", true),
    ("get_rear", false),
    ("get_slot", false),
    ("set_rear", true),
    ("set_color", true),
    ("get_usb", false),
    ("set_usb", true),
    ("get_kbtimeout", false),
    ("set_kbtimeout", true),
    ("get_battery", true),
    ("set_battery", true),
];

/// The whitespace the kernel's `isspace` knows.
fn is_c_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r')
}

/// Splits a written command like the kernel: the C string ends at the first NUL, surrounding
/// whitespace is trimmed, the name ends at the first SPACE (only a space: a tab stays in the
/// name), and the argument starts after any further whitespace.
fn split_command(command: &str) -> (&str, Option<&str>) {
    let line = command.split('\0').next().unwrap_or("").trim_matches(is_c_space);
    match line.split_once(' ') {
        Some((name, rest)) => (name, Some(rest.trim_start_matches(is_c_space))),
        None => (line, None),
    }
}

/// `kstrtou8`/`kstrtouint` in base 10: an optional `+`, then digits only, no overflow past `max`.
fn kstrtou(text: &[u8], max: u64) -> Option<u64> {
    let digits = text.strip_prefix(b"+").unwrap_or(text);
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(digits).ok()?.parse::<u64>().ok().filter(|v| *v <= max)
}

/// `sscanf(text, "%u %u", ...) == 2`: two runs of digits, any whitespace between them (also
/// none, if the digits end), anything after them ignored. The kernel truncates a value to 32
/// bits; the simulator is stricter and refuses one that does not fit, which no caller sends.
fn scan_two_uints(text: &str) -> Option<(u32, u32)> {
    let bytes = text.as_bytes();
    let first_end = bytes.iter().position(|b| !b.is_ascii_digit()).unwrap_or(bytes.len());
    let rest = &bytes[first_end..];
    let skipped = rest.iter().position(|b| !is_c_space(char::from(*b))).unwrap_or(rest.len());
    let rest = &rest[skipped..];
    let second_end = rest.iter().position(|b| !b.is_ascii_digit()).unwrap_or(rest.len());
    let first = u32::try_from(kstrtou(&bytes[..first_end], u64::from(u32::MAX))?).ok()?;
    let second = u32::try_from(kstrtou(&rest[..second_end], u64::from(u32::MAX))?).ok()?;
    Some((first, second))
}

/// `hex2bin`: pairs of hex digits (either case) into bytes.
fn hex2bin(text: &[u8]) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 {
        return None;
    }
    text.chunks(2)
        .map(|pair| {
            let hi = char::from(pair[0]).to_digit(16)?;
            let lo = char::from(pair[1]).to_digit(16)?;
            u8::try_from(hi * 16 + lo).ok()
        })
        .collect()
}

fn byte(arg: &str, allowed: impl Fn(u8) -> bool) -> Option<u8> {
    let value = u8::try_from(kstrtou(arg.as_bytes(), 255)?).ok()?;
    allowed(value).then_some(value)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Runs one validated command. `None` means the kernel would answer `-EINVAL`
/// ("err invalid argument").
fn dispatch(s: &mut Snapshot, name: &str, arg: &str) -> Option<String> {
    let reply = match name {
        "version" => "ok version=sim\n".to_string(),
        "caps" => "ok game=1 apge=1 battery=1\n".to_string(),
        "sensors" => format!(
            "ok cpu={} sys={} gpu={} cpu_rpm={} gpu_rpm={}\n",
            s.temps[0], s.temps[1], s.temps[2], s.rpm[0], s.rpm[1]
        ),
        "sensors_temps" => format!("ok cpu={} sys={} gpu={}\n", s.temps[0], s.temps[1], s.temps[2]),
        "get_mode" => format!("ok mode={}\n", s.mode),
        // Bits 0, 1, 4, 5 and 6: the five modes of the PH16-71.
        "get_modes" => "ok mask=0x73\n".to_string(),
        "set_mode" => {
            s.mode = byte(arg, |m| matches!(m, 0 | 1 | 4 | 5 | 6))?;
            format!("ok mode={}\n", s.mode)
        }
        "get_gpu" => format!("ok gpu={}\n", s.gpu),
        "set_gpu" => {
            s.gpu = byte(arg, |m| (1..=3).contains(&m))?;
            format!("ok gpu={}\n", s.gpu)
        }
        "get_boot" => format!("ok boot={}\n", s.boot),
        "set_boot" => {
            s.boot = byte(arg, |m| m <= 1)?;
            format!("ok boot={}\n", s.boot)
        }
        "get_misc" => {
            let idx = byte(arg, |i| (1..=16).contains(&i))?;
            format!("ok idx={idx} status=0 value=0\n")
        }
        "get_overdrive" => format!("ok overdrive={} raw=0x0\n", s.overdrive),
        "set_overdrive" => {
            s.overdrive = byte(arg, |m| m <= 1)?;
            format!("ok overdrive={}\n", s.overdrive)
        }
        "get_fan" => {
            let behavior = match s.fan {
                SimFan::Auto => 0x41,
                SimFan::Max => 0x82,
                SimFan::Manual => 0xC3,
            };
            format!("ok behavior=0x{behavior:x} cpu={} gpu={}\n", s.duty[0], s.duty[1])
        }
        "set_fan_auto" => {
            s.fan = SimFan::Auto;
            "ok fan=auto\n".to_string()
        }
        "set_fan_max" => {
            s.fan = SimFan::Max;
            "ok fan=max\n".to_string()
        }
        "set_fan_manual" => {
            // Per fan 20..100, or 0 = that fan stopped (the daemon guards a stop; the module allows it).
            let (cpu, gpu) = scan_two_uints(arg)?;
            let duty = |v: u32| u8::try_from(v).ok().filter(|v| *v == 0 || (20..=100).contains(v));
            let (cpu, gpu) = (duty(cpu)?, duty(gpu)?);
            s.fan = SimFan::Manual;
            s.duty = [cpu, gpu];
            format!("ok fan=manual cpu={cpu} gpu={gpu}\n")
        }
        "get_rear" => format!("ok raw={}\n", hex(&s.rear)),
        "get_slot" => "ok raw=00000564000100aec700000000000000\n".to_string(),
        "set_rear" => {
            let b = if arg.len() == 32 { hex2bin(arg.as_bytes())? } else { return None };
            // Modes 0 static, 1 breathing, 2 neon, 3 wave, 7 twinkling; speed 1..5; brightness
            // 0..100; bytes 8 and 9 are fixed; the last six are zero.
            let valid = (b[0] <= 3 || b[0] == 7)
                && (1..=5).contains(&b[1])
                && b[2] <= 100
                && b[3] == 0
                && b[4] <= 1
                && b[8] == 3
                && b[9] == 2
                && b[10..].iter().all(|x| *x == 0);
            if !valid {
                return None;
            }
            s.rear[0] = 0;
            s.rear[1..].copy_from_slice(&b[..15]);
            "ok\n".to_string()
        }
        "set_color" => {
            let text = arg.as_bytes();
            if text.len() < 6 {
                return None;
            }
            let color = hex2bin(&text[..6])?;
            let zone = if text.len() > 6 {
                if text[6] != b' ' {
                    return None;
                }
                u8::try_from(kstrtou(&text[7..], u64::from(u32::MAX))?)
                    .ok()
                    .filter(|z| matches!(z, 1 | 2 | 4))?
            } else {
                1
            };
            s.zones.insert(zone, hex(&color));
            "ok\n".to_string()
        }
        // The firmware answers -1 for a raw value it does not know; the simulator always knows.
        "get_usb" => format!("ok usb={} raw=0\n", s.usb),
        "set_usb" => {
            s.usb = byte(arg, |v| matches!(v, 0 | 10 | 20 | 30))?;
            format!("ok usb={}\n", s.usb)
        }
        "get_kbtimeout" => format!("ok kbtimeout={} raw=0x0\n", s.kbtimeout),
        "set_kbtimeout" => {
            s.kbtimeout = byte(arg, |v| v <= 1)?;
            format!("ok kbtimeout={}\n", s.kbtimeout)
        }
        "get_battery" => {
            let which = byte(arg, |w| matches!(w, 1 | 2))?;
            format!("ok battery={} list=3\n", s.battery[usize::from(which) - 1])
        }
        "set_battery" => {
            let (which, value) = scan_two_uints(arg)?;
            let which = u8::try_from(which).ok().filter(|w| matches!(w, 1 | 2))?;
            let value = u8::try_from(value).ok().filter(|v| *v <= 1)?;
            s.battery[usize::from(which) - 1] = value;
            format!("ok battery={which} value={value}\n")
        }
        _ => return None,
    };
    Some(reply)
}
